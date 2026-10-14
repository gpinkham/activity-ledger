use crate::metrics::Filters;
use crate::pricing::PriceTable;
use crate::store::Store;
use serde::Serialize;
use sha2::{Digest, Sha256};

fn hash_id(salt: &str, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    hex::encode(digest)[..16].to_string()
}

#[derive(Debug, Serialize)]
pub struct BundleTotals {
    pub sessions: i64,
    pub messages: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_creation: i64,
    pub est_cost_usd: f64,
}

#[derive(Debug, Serialize)]
pub struct BundleModel { pub model: String, pub messages: i64, pub tokens: i64, pub cost_usd: f64 }

#[derive(Debug, Serialize)]
pub struct BundleTool { pub tool: String, pub count: i64 }

#[derive(Debug, Serialize)]
pub struct BundleDay { pub day: String, pub messages: i64, pub cost_usd: f64 }

#[derive(Debug, Serialize)]
pub struct BundleProject { pub project_id: String, pub sessions: i64, pub tokens: i64, pub cost_usd: f64 }

#[derive(Debug, Serialize)]
pub struct DateRange { pub from: Option<String>, pub to: Option<String> }

#[derive(Debug, Serialize)]
pub struct Bundle {
    pub schema_version: u32,
    pub generated_at: String,
    pub machine_id: String,
    pub date_range: DateRange,
    pub totals: BundleTotals,
    pub by_model: Vec<BundleModel>,
    pub by_tool: Vec<BundleTool>,
    pub by_day: Vec<BundleDay>,
    pub projects: Vec<BundleProject>,
}

pub fn build_bundle(
    store: &Store,
    price: &PriceTable,
    f: &Filters,
    generated_at: &str,
) -> rusqlite::Result<Bundle> {
    let salt = store.export_salt();
    let t = store.totals(price, f)?;
    let by_model = store.by_model(price, f)?.into_iter()
        .map(|m| BundleModel { model: m.model, messages: m.messages, tokens: m.tokens, cost_usd: m.cost_usd })
        .collect();
    let by_tool = store.by_tool(f)?.into_iter()
        .map(|t| BundleTool { tool: t.tool, count: t.count })
        .collect();
    let by_day = store.by_day(price, f)?.into_iter()
        .map(|d| BundleDay { day: d.day, messages: d.messages, cost_usd: d.cost_usd })
        .collect();
    let projects = store.by_project(price, f)?.into_iter()
        .map(|p| BundleProject {
            project_id: hash_id(&salt, &p.project),
            sessions: p.sessions,
            tokens: p.tokens,
            cost_usd: p.cost_usd,
        })
        .collect();

    Ok(Bundle {
        schema_version: 1,
        generated_at: generated_at.to_string(),
        machine_id: hash_id(&salt, &store.machine_id()),
        date_range: DateRange { from: f.from.clone(), to: f.to.clone() },
        totals: BundleTotals {
            sessions: t.sessions,
            messages: t.messages,
            input_tokens: t.input_tokens,
            output_tokens: t.output_tokens,
            cache_read: t.cache_read,
            cache_creation: t.cache_creation,
            est_cost_usd: t.est_cost_usd,
        },
        by_model,
        by_tool,
        by_day,
        projects,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_str;
    use crate::scanner::ScannedFile;

    fn seeded() -> Store {
        let mut store = Store::open_in_memory();
        let parsed = parse_str("proj-secret-name", include_str!("../tests/fixtures/sample.jsonl"));
        let sf = ScannedFile { path: "/x/proj/s1.jsonl".into(), project: "proj-secret-name".into(), size: 1, mtime: 1, subagent: None };
        store.replace_file(&sf, &parsed).unwrap();
        store.rebuild_sessions().unwrap();
        store
    }

    #[test]
    fn bundle_hashes_project_names() {
        let store = seeded();
        let b = build_bundle(&store, &PriceTable::seed(), &Filters::default(), "2026-07-24").unwrap();
        assert_eq!(b.schema_version, 1);
        assert!(!b.projects.is_empty());
        // The raw project name must never appear anywhere in the serialized bundle.
        let json = serde_json::to_string(&b).unwrap();
        assert!(!json.contains("proj-secret-name"), "raw project name leaked into export");
        assert_eq!(b.projects[0].project_id.len(), 16);
    }

    #[test]
    fn machine_id_is_hashed_not_raw() {
        let store = seeded();
        let b = build_bundle(&store, &PriceTable::seed(), &Filters::default(), "2026-07-24").unwrap();
        assert_ne!(b.machine_id, store.machine_id());
        assert_eq!(b.machine_id.len(), 16);
    }
}
