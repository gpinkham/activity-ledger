pub mod commands;
pub mod activity;
pub mod exporter;
pub mod labeller;
pub mod metrics;
pub mod models;
pub mod ollama;
pub mod parser;
pub mod pricing;
pub mod scanner;
pub mod settings;
pub mod sessions;
pub mod store;
pub mod triggers;

use commands::AppState;
use pricing::PriceTable;
use std::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            use tauri::Manager;
            let data_dir = app.path().app_data_dir().expect("app data dir");
            std::fs::create_dir_all(&data_dir).ok();
            let db_path = data_dir.join("activity-ledger.sqlite");
            let mut store = store::Store::open(&db_path).expect("open db");
            let home = dirs::home_dir().expect("home dir");
            let claude_root = home.join(".claude").join("projects");
            let price_path = data_dir.join("prices.json");
            let settings_path = data_dir.join("settings.json");
            let settings = settings::AppSettings::load_or_default(&settings_path);
            let triggers_path = data_dir.join("triggers.json");
            let (triggers, triggers_error) = triggers::TriggerConfig::load_or_default(&triggers_path);
            // Standups and scheduled jobs are known from the first view, before any Refresh.
            store.sender_labels = triggers.sender_labels(Some(&home));
            app.manage(AppState {
                store: Mutex::new(store),
                price: Mutex::new(PriceTable::load_or_seed(&price_path)),
                claude_root,
                settings: Mutex::new(settings),
                price_path,
                settings_path,
                label_progress: Mutex::new(labeller::LabelProgress::default()),
                label_stop: Default::default(),
                triggers: Mutex::new(triggers),
                triggers_path,
                triggers_error: Mutex::new(triggers_error),
                home: Some(home),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::refresh,
            commands::get_totals,
            commands::get_by_model,
            commands::get_by_tool,
            commands::get_by_day,
            commands::get_by_project,
            commands::get_workflow,
            commands::get_sessions,
            commands::get_session_detail,
            commands::get_activity,
            commands::get_settings,
            commands::run_model_labels,
            commands::get_label_progress,
            commands::stop_model_labels,
            commands::get_triggers,
            commands::set_adapter_enabled,
            commands::set_settings,
            commands::get_price_table,
            commands::set_price_table,
            commands::export_bundle,
            commands::save_export,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
