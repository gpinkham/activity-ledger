use crate::pricing::{without_long, PriceTable, TokenBreakdown};
use crate::store::Store;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Filters {
    pub from: Option<String>,
    pub to: Option<String>,
    pub project: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Totals {
    pub sessions: i64,
    pub messages: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation: i64,
    pub cache_read: i64,
    pub est_cost_usd: f64,
    pub saved_usd: f64,
    pub cache_hit_ratio: f64,
}

#[derive(Debug, Serialize)]
pub struct ByModel { pub model: String, pub messages: i64, pub tokens: i64, pub cost_usd: f64 }

#[derive(Debug, Serialize)]
pub struct ByTool { pub tool: String, pub count: i64, pub is_mcp: bool }

#[derive(Debug, Serialize)]
pub struct ByDay { pub day: String, pub messages: i64, pub cost_usd: f64 }

#[derive(Debug, Serialize)]
pub struct ByProject {
    pub project: String,
    pub sessions: i64,
    pub tokens: i64,
    pub cost_usd: f64,
    pub tool_calls: i64,
    pub edit_write: i64,
    pub web: i64,
}

#[derive(Debug, Serialize)]
pub struct HeatCell { pub weekday: i64, pub hour: i64, pub count: i64 }

#[derive(Debug, Serialize)]
pub struct ModelDayCount { pub day: String, pub model: String, pub messages: i64 }

#[derive(Debug, Serialize)]
pub struct Workflow {
    pub bash: i64,
    pub edit_write: i64,
    pub user_modified: i64,
    pub edit_write_total: i64,
    pub web_search: i64,
    pub web_fetch: i64,
    pub heatmap: Vec<HeatCell>,
    pub model_mix: Vec<ModelDayCount>,
}

/// Build a SQL `WHERE` fragment + bound params for the given filters against a
/// table that has `project`, `ts` and `dup` columns. `ts` is an ISO-8601 string, so
/// date comparison uses `substr(ts,1,10)`. Copies of a call held in a second file
/// (`dup = 1`, see `Store::mark_duplicates`) are always left out.
pub(crate) fn where_clause(f: &Filters) -> (String, Vec<(String, String)>) {
    let mut clauses = vec!["dup = 0".to_string()];
    let mut binds: Vec<(String, String)> = Vec::new();
    if let Some(p) = &f.project {
        clauses.push("project = :project".to_string());
        binds.push((":project".into(), p.clone()));
    }
    if let Some(from) = &f.from {
        clauses.push("substr(ts,1,10) >= :from".to_string());
        binds.push((":from".into(), from.clone()));
    }
    if let Some(to) = &f.to {
        clauses.push("substr(ts,1,10) <= :to".to_string());
        binds.push((":to".into(), to.clone()));
    }
    let sql = format!("WHERE {}", clauses.join(" AND "));
    (sql, binds)
}

pub(crate) fn bind<'a>(binds: &'a [(String, String)]) -> Vec<(&'a str, &'a dyn rusqlite::ToSql)> {
    binds.iter().map(|(k, v)| (k.as_str(), v as &dyn rusqlite::ToSql)).collect()
}

impl Store {
    pub fn totals(&self, price: &PriceTable, f: &Filters) -> rusqlite::Result<Totals> {
        let (w, binds) = where_clause(f);
        let m = price.model_sql();
        let sql = format!(
            "SELECT {m}, SUM(input_tokens), SUM(output_tokens), SUM(cache_creation), SUM(cache_read),
                    SUM(ephemeral_1h), SUM(ephemeral_5m), COUNT(*)
             FROM messages {w} GROUP BY {m}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(bind(&binds).as_slice(), |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?, r.get::<_, i64>(6)?, r.get::<_, i64>(7)?,
            ))
        })?;
        let mut t = Totals { sessions: 0, messages: 0, input_tokens: 0, output_tokens: 0,
            cache_creation: 0, cache_read: 0, est_cost_usd: 0.0, saved_usd: 0.0, cache_hit_ratio: 0.0 };
        for row in rows {
            let (model, input, output, cc, cr, e1h, e5m, msgs) = row?;
            t.input_tokens += input;
            t.output_tokens += output;
            t.cache_creation += cc;
            t.cache_read += cr;
            t.messages += msgs;
            let tb = TokenBreakdown { input, output, cache_creation_5m: e5m, cache_creation_1h: e1h, cache_read: cr, ..Default::default() };
            t.est_cost_usd += price.cost(&model, &tb);
            t.saved_usd += price.saved(&model, cr);
        }
        // sessions distinct across the whole filtered set.
        let sess_sql = format!("SELECT COUNT(DISTINCT session_id) FROM messages {w}");
        t.sessions = self.conn.query_row(&sess_sql, bind(&binds).as_slice(), |r| r.get(0))?;
        let cacheable = (t.cache_read + t.cache_creation) as f64;
        t.cache_hit_ratio = if cacheable > 0.0 { t.cache_read as f64 / cacheable } else { 0.0 };
        Ok(t)
    }

    pub fn by_model(&self, price: &PriceTable, f: &Filters) -> rusqlite::Result<Vec<ByModel>> {
        let (w, binds) = where_clause(f);
        let m = price.model_sql();
        let sql = format!(
            "SELECT {m}, COUNT(*), SUM(input_tokens+output_tokens), SUM(input_tokens), SUM(output_tokens),
                    SUM(cache_read), SUM(ephemeral_1h), SUM(ephemeral_5m)
             FROM messages {w} GROUP BY {m} ORDER BY 3 DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(bind(&binds).as_slice(), |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_else(|| "unknown".into()),
                r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?, r.get::<_, i64>(6)?, r.get::<_, i64>(7)?,
            ))
        })?;
        let mut out: Vec<ByModel> = Vec::new();
        for row in rows {
            let (model, msgs, tokens, input, output, cr, e1h, e5m) = row?;
            let tb = TokenBreakdown { input, output, cache_creation_5m: e5m, cache_creation_1h: e1h, cache_read: cr, ..Default::default() };
            let cost_usd = price.cost(&model, &tb);
            // A model's long prompts are priced apart but shown in its own row.
            let name = without_long(&model);
            match out.iter_mut().find(|b| b.model == name) {
                Some(b) => {
                    b.messages += msgs;
                    b.tokens += tokens;
                    b.cost_usd += cost_usd;
                }
                None => out.push(ByModel { model: name.to_string(), messages: msgs, tokens, cost_usd }),
            }
        }
        out.sort_by(|a, b| b.tokens.cmp(&a.tokens));
        Ok(out)
    }

    pub fn by_tool(&self, f: &Filters) -> rusqlite::Result<Vec<ByTool>> {
        let (w, binds) = where_clause(f);
        let sql = format!("SELECT name, COUNT(*), MAX(is_mcp) FROM tool_calls {w} GROUP BY name ORDER BY 2 DESC");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(bind(&binds).as_slice(), |r| {
            Ok(ByTool { tool: r.get(0)?, count: r.get(1)?, is_mcp: r.get::<_, i64>(2)? != 0 })
        })?;
        rows.collect()
    }

    pub fn by_day(&self, price: &PriceTable, f: &Filters) -> rusqlite::Result<Vec<ByDay>> {
        let (w, binds) = where_clause(f);
        let sql = format!(
            "SELECT substr(ts,1,10) AS day, {m}, COUNT(*), SUM(input_tokens), SUM(output_tokens),
                    SUM(cache_read), SUM(ephemeral_1h), SUM(ephemeral_5m)
             FROM messages {w} GROUP BY day, {m} ORDER BY day",
            m = price.model_sql()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(bind(&binds).as_slice(), |r| {
            Ok((
                r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?, r.get::<_, i64>(7)?,
            ))
        })?;
        use std::collections::BTreeMap;
        let mut acc: BTreeMap<String, (i64, f64)> = BTreeMap::new();
        for row in rows {
            let (day, model, msgs, input, output, cr, e1h, e5m) = row?;
            let tb = TokenBreakdown { input, output, cache_creation_5m: e5m, cache_creation_1h: e1h, cache_read: cr, ..Default::default() };
            let e = acc.entry(day).or_insert((0, 0.0));
            e.0 += msgs;
            e.1 += price.cost(&model, &tb);
        }
        Ok(acc.into_iter().map(|(day, (messages, cost_usd))| ByDay { day, messages, cost_usd }).collect())
    }

    pub fn by_project(&self, price: &PriceTable, f: &Filters) -> rusqlite::Result<Vec<ByProject>> {
        // Note: project filter is ignored for this rollup (it compares projects).
        let mut f2 = f.clone();
        f2.project = None;
        let (w, binds) = where_clause(&f2);
        let msql = format!(
            "SELECT project, {m}, SUM(input_tokens), SUM(output_tokens),
                    SUM(cache_read), SUM(ephemeral_1h), SUM(ephemeral_5m)
             FROM messages {w} GROUP BY project, {m}",
            m = price.model_sql()
        );
        let mut stmt = self.conn.prepare(&msql)?;
        let rows = stmt.query_map(bind(&binds).as_slice(), |r| {
            Ok((
                r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?, r.get::<_, i64>(6)?,
            ))
        })?;
        use std::collections::BTreeMap;
        let mut acc: BTreeMap<String, ByProject> = BTreeMap::new();
        for row in rows {
            let (project, model, input, output, cr, e1h, e5m) = row?;
            let tb = TokenBreakdown { input, output, cache_creation_5m: e5m, cache_creation_1h: e1h, cache_read: cr, ..Default::default() };
            let e = acc.entry(project.clone()).or_insert(ByProject {
                project, sessions: 0, tokens: 0, cost_usd: 0.0, tool_calls: 0, edit_write: 0, web: 0,
            });
            e.tokens += input + output;
            e.cost_usd += price.cost(&model, &tb);
        }
        // True per-project distinct session count (a project's sessions can span
        // multiple models, so this must not be derived from the per-model groups above).
        let ssql = format!(
            "SELECT project, COUNT(DISTINCT session_id) FROM messages {w} GROUP BY project"
        );
        let mut sstmt = self.conn.prepare(&ssql)?;
        let srows = sstmt.query_map(bind(&binds).as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in srows {
            let (project, sess) = row?;
            if let Some(e) = acc.get_mut(&project) {
                e.sessions = sess;
            }
        }
        // Tool counts per project.
        let tsql = format!(
            "SELECT project, COUNT(*),
                    SUM(CASE WHEN name IN ('Edit','Write') THEN 1 ELSE 0 END),
                    SUM(CASE WHEN name IN ('WebSearch','WebFetch') THEN 1 ELSE 0 END)
             FROM tool_calls {w} GROUP BY project"
        );
        let mut tstmt = self.conn.prepare(&tsql)?;
        let trows = tstmt.query_map(bind(&binds).as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
        })?;
        for row in trows {
            let (project, total, ew, web) = row?;
            if let Some(e) = acc.get_mut(&project) {
                e.tool_calls = total;
                e.edit_write = ew;
                e.web = web;
            }
        }
        let mut out: Vec<ByProject> = acc.into_values().collect();
        out.sort_by(|a, b| b.cost_usd.partial_cmp(&a.cost_usd).unwrap());
        Ok(out)
    }

    pub fn workflow(&self, f: &Filters) -> rusqlite::Result<Workflow> {
        let (w, binds) = where_clause(f);
        let tsql = format!(
            "SELECT
               COALESCE(SUM(CASE WHEN name='Bash' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN name IN ('Edit','Write') THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN name IN ('Edit','Write') AND user_modified=1 THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN name IN ('Edit','Write') AND user_modified IS NOT NULL THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN name='WebSearch' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN name='WebFetch' THEN 1 ELSE 0 END), 0)
             FROM tool_calls {w}"
        );
        let (bash, edit_write, user_modified, edit_write_total, web_search, web_fetch): (i64,i64,i64,i64,i64,i64) =
            self.conn.query_row(&tsql, bind(&binds).as_slice(), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
            })?;

        // Heatmap: derive weekday+hour in Rust from the ISO timestamp.
        let hsql = format!("SELECT ts FROM messages {w}");
        let mut hstmt = self.conn.prepare(&hsql)?;
        let ts_rows = hstmt.query_map(bind(&binds).as_slice(), |r| r.get::<_, String>(0))?;
        use std::collections::HashMap;
        let mut heat: HashMap<(i64, i64), i64> = HashMap::new();
        for ts in ts_rows {
            let ts = ts?;
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&ts) {
                use chrono::{Datelike, Timelike};
                let weekday = dt.weekday().num_days_from_monday() as i64;
                let hour = dt.hour() as i64;
                *heat.entry((weekday, hour)).or_insert(0) += 1;
            }
        }
        let heatmap = heat.into_iter().map(|((weekday, hour), count)| HeatCell { weekday, hour, count }).collect();

        // Model mix over time.
        let msql = format!(
            "SELECT substr(ts,1,10), COALESCE(model,'unknown'), COUNT(*)
             FROM messages {w} GROUP BY 1, 2 ORDER BY 1"
        );
        let mut mstmt = self.conn.prepare(&msql)?;
        let mrows = mstmt.query_map(bind(&binds).as_slice(), |r| {
            Ok(ModelDayCount { day: r.get(0)?, model: r.get(1)?, messages: r.get(2)? })
        })?;
        let model_mix = mrows.collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(Workflow { bash, edit_write, user_modified, edit_write_total, web_search, web_fetch, heatmap, model_mix })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_str;
    use crate::scanner::ScannedFile;

    fn seeded() -> Store {
        let mut store = Store::open_in_memory();
        let parsed = parse_str("proj-a", include_str!("../tests/fixtures/sample.jsonl"));
        let sf = ScannedFile { path: "/x/proj-a/s1.jsonl".into(), project: "proj-a".into(), size: 1, mtime: 1, subagent: None };
        store.replace_file(&sf, &parsed).unwrap();
        store.rebuild_sessions().unwrap();
        store
    }

    #[test]
    fn totals_sum_tokens_and_cost() {
        let store = seeded();
        let t = store.totals(&PriceTable::seed(), &Filters::default()).unwrap();
        assert_eq!(t.messages, 3);
        assert_eq!(t.sessions, 1);
        assert_eq!(t.input_tokens, 110); // 100 + 10
        assert_eq!(t.cache_read, 500);   // 200 + 300
        assert!(t.est_cost_usd > 0.0);
        assert!(t.saved_usd > 0.0);
        assert!(t.cache_hit_ratio > 0.9);
    }

    #[test]
    fn by_tool_orders_and_flags_mcp() {
        let store = seeded();
        let tools = store.by_tool(&Filters::default()).unwrap();
        assert_eq!(tools.len(), 2);
        let mcp = tools.iter().find(|t| t.is_mcp).unwrap();
        assert!(mcp.tool.starts_with("mcp__"));
    }

    #[test]
    fn workflow_counts_user_modified() {
        let store = seeded();
        let wf = store.workflow(&Filters::default()).unwrap();
        assert_eq!(wf.edit_write, 1);
        assert_eq!(wf.user_modified, 1);
        assert_eq!(wf.edit_write_total, 1);
        assert!(!wf.heatmap.is_empty());
    }

    #[test]
    fn project_filter_scopes_totals() {
        let store = seeded();
        let f = Filters { project: Some("nonexistent".into()), ..Default::default() };
        let t = store.totals(&PriceTable::seed(), &f).unwrap();
        assert_eq!(t.messages, 0);
    }

    #[test]
    fn workflow_empty_returns_zeros() {
        let store = Store::open_in_memory();
        let wf = store.workflow(&Filters::default()).unwrap();
        assert_eq!(wf.bash, 0);
        assert_eq!(wf.edit_write, 0);
        assert_eq!(wf.edit_write_total, 0);
        assert_eq!(wf.web_search, 0);
        assert!(wf.heatmap.is_empty());
    }

    #[test]
    fn by_project_counts_sessions_across_models() {
        let mut store = Store::open_in_memory();
        let parsed1 = parse_str("proj-a", include_str!("../tests/fixtures/sample.jsonl"));
        let sf1 = ScannedFile { path: "/x/proj-a/s1.jsonl".into(), project: "proj-a".into(), size: 1, mtime: 1, subagent: None };
        store.replace_file(&sf1, &parsed1).unwrap();

        let s2 = r#"{"type":"assistant","sessionId":"s2","timestamp":"2026-07-25T09:00:00.000Z","gitBranch":"main","version":"2.1.214","message":{"role":"assistant","model":"claude-haiku-4-5","stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":3,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},"content":[{"type":"text","text":"hi again"}]}}"#;
        let parsed2 = parse_str("proj-a", s2);
        let sf2 = ScannedFile { path: "/x/proj-a/s2.jsonl".into(), project: "proj-a".into(), size: 1, mtime: 1, subagent: None };
        store.replace_file(&sf2, &parsed2).unwrap();

        store.rebuild_sessions().unwrap();
        let projects = store.by_project(&PriceTable::seed(), &Filters::default()).unwrap();
        let proj_a = projects.iter().find(|p| p.project == "proj-a").unwrap();
        assert_eq!(proj_a.sessions, 2);
    }
}
