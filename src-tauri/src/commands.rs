use crate::activity::{ActivityReport, LabelSource};
use crate::exporter::build_bundle;
use crate::labeller::{self, LabelProgress};
use crate::metrics::{ByDay, ByModel, ByProject, ByTool, Filters, Totals, Workflow};
use crate::parser::parse_str;
use crate::pricing::PriceTable;
use crate::scanner::scan_dir;
use crate::sessions::{SessionDetail, SessionSummary};
use crate::settings::AppSettings;
use crate::store::Store;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub store: Mutex<Store>,
    pub price: Mutex<PriceTable>,
    pub claude_root: PathBuf,
    pub settings: Mutex<AppSettings>,
    pub price_path: PathBuf,
    pub settings_path: PathBuf,
    /// The labelling pass's progress, polled by the settings card.
    pub label_progress: Mutex<LabelProgress>,
    /// Set by Stop; cleared when a pass starts.
    pub label_stop: Arc<AtomicBool>,
}

#[derive(Debug, Serialize)]
pub struct RefreshSummary {
    pub files_scanned: usize,
    pub files_parsed: usize,
}

/// Core refresh logic, decoupled from Tauri so it can be unit-tested.
pub fn do_refresh(store: &mut Store, root: &Path) -> rusqlite::Result<RefreshSummary> {
    let scanned = scan_dir(root);
    let mut parsed_count = 0;
    for sf in &scanned {
        let path = sf.path.to_string_lossy().to_string();
        if !store.needs_parse(&path, sf.size, sf.mtime) {
            continue;
        }
        let content = match std::fs::read_to_string(&sf.path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let parsed = parse_str(&sf.project, &content);
        store.replace_file(sf, &parsed)?;
        parsed_count += 1;
    }
    store.rebuild_sessions()?;
    Ok(RefreshSummary { files_scanned: scanned.len(), files_parsed: parsed_count })
}

#[tauri::command]
pub fn refresh(state: tauri::State<AppState>) -> Result<RefreshSummary, String> {
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    do_refresh(&mut store, &state.claude_root).map_err(|e| e.to_string())
}

macro_rules! metric_cmd {
    ($name:ident, $ret:ty, $call:ident) => {
        #[tauri::command]
        pub fn $name(state: tauri::State<AppState>, filters: Filters) -> Result<$ret, String> {
            let store = state.store.lock().map_err(|e| e.to_string())?;
            let price = state.price.lock().map_err(|e| e.to_string())?;
            store.$call(&price, &filters).map_err(|e| e.to_string())
        }
    };
}

metric_cmd!(get_totals, Totals, totals);
metric_cmd!(get_by_model, Vec<ByModel>, by_model);
metric_cmd!(get_by_day, Vec<ByDay>, by_day);
metric_cmd!(get_by_project, Vec<ByProject>, by_project);

#[tauri::command]
pub fn get_by_tool(state: tauri::State<AppState>, filters: Filters) -> Result<Vec<ByTool>, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    store.by_tool(&filters).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_workflow(state: tauri::State<AppState>, filters: Filters) -> Result<Workflow, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    store.workflow(&filters).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_price_table(state: tauri::State<AppState>) -> Result<PriceTable, String> {
    let price = state.price.lock().map_err(|e| e.to_string())?;
    Ok(price.clone())
}

#[tauri::command]
pub fn set_price_table(state: tauri::State<AppState>, table: PriceTable) -> Result<(), String> {
    let mut price = state.price.lock().map_err(|e| e.to_string())?;
    table.save(&state.price_path).map_err(|e| e.to_string())?;
    *price = table;
    Ok(())
}

#[tauri::command]
pub fn get_activity(state: tauri::State<AppState>, filters: Filters, label_source: Option<LabelSource>) -> Result<ActivityReport, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    let price = state.price.lock().map_err(|e| e.to_string())?;
    let settings = state.settings.lock().map_err(|e| e.to_string())?;
    store.activity_by(&price, &filters, &settings, label_source.unwrap_or_default()).map_err(|e| e.to_string())
}

/// Start the optional labelling pass on a background thread. Refused when it is
/// turned off, no model is set, or the URL is remote without the allow box.
#[tauri::command]
pub fn run_model_labels(app: tauri::AppHandle, state: tauri::State<AppState>, filters: Filters) -> Result<(), String> {
    use tauri::Manager;
    let settings = state.settings.lock().map_err(|e| e.to_string())?.clone();
    let mut endpoint = labeller::prepare(&settings)?;
    {
        let mut p = state.label_progress.lock().map_err(|e| e.to_string())?;
        if p.running {
            return Err("a labelling pass is already running".into());
        }
        *p = LabelProgress { running: true, ..Default::default() };
    }
    state.label_stop.store(false, Ordering::SeqCst);
    endpoint.stop = state.label_stop.clone();
    let price = state.price.lock().map_err(|e| e.to_string())?.clone();
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let model = settings.ollama_model.clone();
        let num_ctx = labeller::request_num_ctx(settings.local_context_limit);
        let result = labeller::run(&st.store, &price, &filters, &settings, &st.label_progress, &st.label_stop,
            || crate::ollama::load_model(&endpoint, &model, num_ctx, crate::ollama::LOAD_TIMEOUT),
            |summary| crate::ollama::chat_label(&endpoint, &model, summary, num_ctx, crate::ollama::LABEL_TIMEOUT));
        if let Err(e) = result {
            if let Ok(mut p) = st.label_progress.lock() {
                p.running = false;
                p.stopped = Some(e);
            }
        }
    });
    Ok(())
}

/// Stop the labelling pass: a waiting request ends at once, and no new one starts.
#[tauri::command]
pub fn stop_model_labels(state: tauri::State<AppState>) {
    state.label_stop.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub fn get_label_progress(state: tauri::State<AppState>) -> Result<LabelProgress, String> {
    Ok(state.label_progress.lock().map_err(|e| e.to_string())?.clone())
}

#[tauri::command]
pub fn get_settings(state: tauri::State<AppState>) -> Result<AppSettings, String> {
    Ok(state.settings.lock().map_err(|e| e.to_string())?.clone())
}

#[tauri::command]
pub fn set_settings(state: tauri::State<AppState>, settings: AppSettings) -> Result<(), String> {
    settings.save(&state.settings_path).map_err(|e| e.to_string())?;
    *state.settings.lock().map_err(|e| e.to_string())? = settings;
    Ok(())
}

#[tauri::command]
pub fn get_sessions(state: tauri::State<AppState>, filters: Filters) -> Result<Vec<SessionSummary>, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    let price = state.price.lock().map_err(|e| e.to_string())?;
    store.get_sessions(&price, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_session_detail(state: tauri::State<AppState>, session_id: String) -> Result<SessionDetail, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    let price = state.price.lock().map_err(|e| e.to_string())?;
    store.get_session_detail(&price, &session_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_bundle(state: tauri::State<AppState>, filters: Filters) -> Result<String, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    let price = state.price.lock().map_err(|e| e.to_string())?;
    let generated_at = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let bundle = build_bundle(&store, &price, &filters, &generated_at).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_export(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn refresh_ingests_a_fixture_tree() {
        let dir = std::env::temp_dir().join(format!("ci_refresh_{}", std::process::id()));
        let proj = dir.join("-Users-me-projA");
        fs::create_dir_all(&proj).unwrap();
        let sample = include_str!("../tests/fixtures/sample.jsonl");
        fs::write(proj.join("s1.jsonl"), sample).unwrap();

        let mut store = Store::open_in_memory();
        let summary = do_refresh(&mut store, &dir).unwrap();
        assert_eq!(summary.files_scanned, 1);
        assert_eq!(summary.files_parsed, 1);

        let t = store.totals(&PriceTable::seed(), &Filters::default()).unwrap();
        assert_eq!(t.messages, 3);

        // Second refresh with no file change parses nothing.
        let summary2 = do_refresh(&mut store, &dir).unwrap();
        assert_eq!(summary2.files_parsed, 0);

        let sessions = store.get_sessions(&PriceTable::seed(), &Filters::default()).unwrap();
        assert_eq!(sessions.len(), 1);
        let detail = store.get_session_detail(&PriceTable::seed(), &sessions[0].session_id).unwrap();
        assert_eq!(detail.summary.message_count, 3);
        assert!(!detail.events.is_empty());

        fs::remove_dir_all(&dir).ok();
    }
}
