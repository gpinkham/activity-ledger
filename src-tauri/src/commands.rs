use crate::activity::{ActivityReport, LabelSource};
use crate::budget::{self, BudgetView, Thresholds};
use crate::gauge::{iso, Clock, SystemClock, Tally};
use crate::exporter::build_bundle;
use crate::labeller::{self, LabelProgress};
use crate::metrics::{ByDay, ByModel, ByProject, ByTool, Filters, Totals, Workflow};
use crate::parser::parse_with;
use crate::pricing::{GaugePrices, ManagedPricing, PriceTable, RateSource};
use crate::scanner::scan_dir;
use crate::sessions::{SessionDetail, SessionSummary};
use crate::settings::AppSettings;
use crate::store::Store;
use crate::triggers::{Matcher, TriggerConfig};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub store: Mutex<Store>,
    pub price: Mutex<PriceTable>,
    /// Every folder Claude Code writes transcripts to (scanner::claude_roots).
    pub claude_roots: Vec<PathBuf>,
    pub settings: Mutex<AppSettings>,
    pub price_path: PathBuf,
    pub settings_path: PathBuf,
    /// The labelling pass's progress, polled by the settings card.
    pub label_progress: Mutex<LabelProgress>,
    /// Set by Stop; cleared when a pass starts.
    pub label_stop: Arc<AtomicBool>,
    /// The harness trigger file, its path, and why it could not be read, if so.
    pub triggers: Mutex<TriggerConfig>,
    pub triggers_path: PathBuf,
    pub triggers_error: Mutex<Option<String>>,
    pub home: Option<PathBuf>,
    /// The menu bar gauge's running tally (gauge.rs).
    pub gauge: Mutex<Tally>,
    /// Claude Code's managed settings file, read for `modelPricing` only.
    pub managed_settings: PathBuf,
}

/// The trigger file as the settings card shows it.
#[derive(Debug, Serialize)]
pub struct TriggerStatus {
    pub path: String,
    pub error: Option<String>,
    pub adapters: Vec<AdapterStatus>,
}

#[derive(Debug, Serialize)]
pub struct AdapterStatus {
    pub name: String,
    pub enabled: bool,
    pub root: Option<String>,
    /// Off, but its harness folder is on this machine: worth turning on.
    pub found: bool,
}

/// Read the trigger file again (the user may have edited it), keeping the result
/// and the reason it could not be read, if so.
fn reload_triggers(state: &AppState) -> Result<(TriggerConfig, Option<String>), String> {
    let (config, error) = TriggerConfig::load_or_default(&state.triggers_path);
    *state.triggers.lock().map_err(|e| e.to_string())? = config.clone();
    *state.triggers_error.lock().map_err(|e| e.to_string())? = error.clone();
    Ok((config, error))
}

fn trigger_status(state: &AppState, config: &TriggerConfig, error: Option<String>) -> TriggerStatus {
    let found = config.found_off(state.home.as_deref());
    TriggerStatus {
        path: state.triggers_path.to_string_lossy().to_string(),
        error,
        adapters: config
            .adapters
            .iter()
            .map(|a| AdapterStatus {
                name: a.name.clone(),
                enabled: a.enabled,
                root: a.root.clone(),
                found: found.iter().any(|(n, _)| *n == a.name),
            })
            .collect(),
    }
}

#[tauri::command]
pub fn get_triggers(state: tauri::State<AppState>) -> Result<TriggerStatus, String> {
    let (config, error) = reload_triggers(&state)?;
    Ok(trigger_status(&state, &config, error))
}

/// Turn an adapter on or off and save the file. The next refresh re-reads the
/// transcripts with it.
#[tauri::command]
pub fn set_adapter_enabled(state: tauri::State<AppState>, name: String, enabled: bool) -> Result<TriggerStatus, String> {
    TriggerConfig::set_enabled(&state.triggers_path, &name, enabled)?;
    let (config, error) = reload_triggers(&state)?;
    Ok(trigger_status(&state, &config, error))
}

#[derive(Debug, Serialize)]
pub struct RefreshSummary {
    pub files_scanned: usize,
    pub files_parsed: usize,
}

/// Core refresh logic, decoupled from Tauri so it can be unit-tested.
pub fn do_refresh(store: &mut Store, root: &Path) -> rusqlite::Result<RefreshSummary> {
    do_refresh_with(store, root, &Matcher::default())
}

/// do_refresh with a harness trigger file's parse settings. When they changed
/// since the cache was read, every transcript is read again.
pub fn do_refresh_with(store: &mut Store, root: &Path, matcher: &Matcher) -> rusqlite::Result<RefreshSummary> {
    do_refresh_roots(store, &[root.to_path_buf()], matcher)
}

/// do_refresh_with, over every transcript folder.
pub fn do_refresh_roots(store: &mut Store, roots: &[PathBuf], matcher: &Matcher) -> rusqlite::Result<RefreshSummary> {
    store.use_triggers(&matcher.fingerprint())?;
    let scanned: Vec<_> = roots.iter().flat_map(|r| scan_dir(r)).collect();
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
        let parsed = parse_with(&sf.project, &content, matcher);
        store.replace_file(sf, &parsed)?;
        parsed_count += 1;
    }
    store.rebuild_sessions()?;
    Ok(RefreshSummary { files_scanned: scanned.len(), files_parsed: parsed_count })
}

#[tauri::command]
pub fn refresh(state: tauri::State<AppState>) -> Result<RefreshSummary, String> {
    let (config, _) = reload_triggers(&state)?;
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    store.sender_labels = config.sender_labels(state.home.as_deref());
    do_refresh_roots(&mut store, &state.claude_roots, &config.matcher(state.home.as_deref())).map_err(|e| e.to_string())
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
pub fn set_settings(app: tauri::AppHandle, state: tauri::State<AppState>, settings: AppSettings) -> Result<(), String> {
    settings.save(&state.settings_path).map_err(|e| e.to_string())?;
    *state.settings.lock().map_err(|e| e.to_string())? = settings;
    // The menu bar icon follows "Show in menu bar"; a new budget redraws the ring.
    crate::tray::sync(&app).map_err(|e| e.to_string())?;
    crate::tray::refresh_icon(&app);
    Ok(())
}

/// Change only the weekly report's hide setting. The report button must not write back
/// a whole settings object: its copy can be older than a budget or Ollama change made
/// elsewhere in the app, and a full write would undo it.
pub fn set_report_hide(state: &AppState, hide: bool) -> Result<AppSettings, String> {
    let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
    let mut next = settings.clone();
    next.report_hide_projects = hide;
    next.save(&state.settings_path).map_err(|e| e.to_string())?;
    *settings = next.clone();
    Ok(next)
}

#[tauri::command]
pub fn set_report_hide_projects(state: tauri::State<AppState>, hide: bool) -> Result<AppSettings, String> {
    set_report_hide(&state, hide)
}

/// What the menu bar panel shows, apart from the work and overhead split.
#[derive(Debug, Serialize)]
pub struct GaugeView {
    /// Today's local date.
    pub day: String,
    pub budget: BudgetView,
    /// "configured" when Claude Code's managed modelPricing priced it, else "list".
    pub rate_source: RateSource,
}

/// Read what the transcripts gained (when `poll`), then price today and the month.
pub fn gauge_view(state: &AppState, clock: &dyn Clock, poll: bool) -> Result<GaugeView, String> {
    let settings = state.settings.lock().map_err(|e| e.to_string())?.clone();
    let prices = GaugePrices {
        table: state.price.lock().map_err(|e| e.to_string())?.clone(),
        managed: ManagedPricing::load(&state.managed_settings),
    };
    let mut tally = state.gauge.lock().map_err(|e| e.to_string())?;
    if poll {
        tally.poll_roots(&state.claude_roots, clock);
    }
    let spend = tally.spend(&prices, clock);
    let t = Thresholds { amber: settings.amber_pct / 100.0, red: settings.red_pct / 100.0 };
    Ok(GaugeView {
        day: spend.day.to_string(),
        budget: budget::view(settings.monthly_budget_usd, spend.day, spend.today, spend.month_to_date, t),
        rate_source: prices.source(),
    })
}

#[tauri::command]
pub fn get_gauge(state: tauri::State<AppState>) -> Result<GaugeView, String> {
    gauge_view(&state, &SystemClock, true)
}

/// Today's spend split into real work and overhead, as the Cost by activity view
/// labels it (its rules), at the price table's rates.
#[derive(Debug, Serialize, PartialEq)]
pub struct WorkSplit {
    pub work_usd: f64,
    pub overhead_usd: f64,
}

/// The split for one local day: the cache is brought up to date, then read for
/// the instants the day spans.
pub fn work_split(state: &AppState, clock: &dyn Clock) -> Result<WorkSplit, String> {
    let (config, _) = reload_triggers(state)?;
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    store.sender_labels = config.sender_labels(state.home.as_deref());
    do_refresh_roots(&mut store, &state.claude_roots, &config.matcher(state.home.as_deref())).map_err(|e| e.to_string())?;
    let today = clock.today();
    let f = Filters {
        from_ts: Some(iso(clock.start_of(today))),
        to_ts: Some(iso(clock.start_of(today.succ_opt().unwrap_or(today)))),
        ..Default::default()
    };
    let price = state.price.lock().map_err(|e| e.to_string())?;
    let settings = state.settings.lock().map_err(|e| e.to_string())?;
    let report = store.activity_by(&price, &f, &settings, LabelSource::Rule).map_err(|e| e.to_string())?;
    let overhead: f64 = report.rows.iter().filter(|r| r.group == "overhead").map(|r| r.cost_usd).sum();
    let all: f64 = report.rows.iter().map(|r| r.cost_usd).sum();
    Ok(WorkSplit { work_usd: all - overhead, overhead_usd: overhead })
}

#[tauri::command]
pub fn get_gauge_split(state: tauri::State<AppState>) -> Result<WorkSplit, String> {
    work_split(&state, &SystemClock)
}

#[tauri::command]
pub fn open_main_window(app: tauri::AppHandle) {
    crate::tray::show_main(&app);
}

/// The panel's "Quit Activity Ledger". The menu bar icon has no menu of its own
/// (tray.rs), so this is how the app is quit while it lives in the menu bar.
/// The panel page's content height in logical points; the panel window is sized
/// to it (tray.rs `fit_panel`), so it never needs a scrollbar.
#[tauri::command]
pub fn fit_panel(app: tauri::AppHandle, height: f64) {
    crate::tray::fit_panel(&app, height);
}

#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
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

/// The weekly report for the local week starting on `week_start` (a Monday,
/// `YYYY-MM-DD`), as Markdown. The front end saves it through `save_export`.
#[tauri::command]
pub fn weekly_report(state: tauri::State<AppState>, week_start: String, hide_projects: bool) -> Result<String, String> {
    let monday = crate::report::parse_week_start(&week_start)?;
    let store = state.store.lock().map_err(|e| e.to_string())?;
    let price = state.price.lock().map_err(|e| e.to_string())?;
    let written = chrono::Local::now().format("%-d %b %Y at %H:%M").to_string();
    crate::report::build_report(&store, &price, &SystemClock, monday, hide_projects, &written).map_err(|e| e.to_string())
}

/// The tray's "Save weekly report": bring up the main window and ask it to start
/// its save flow. The tray itself writes nothing.
#[tauri::command]
pub fn open_weekly_report(app: tauri::AppHandle) {
    use tauri::Emitter;
    crate::tray::show_main(&app);
    let _ = app.emit_to("main", "start-weekly-report", ());
}

#[tauri::command]
pub fn save_export(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, NaiveDate, TimeZone, Utc};
    use std::fs;

    struct UtcDay(NaiveDate);
    impl Clock for UtcDay {
        fn today(&self) -> NaiveDate {
            self.0
        }
        fn date_of(&self, t: DateTime<Utc>) -> NaiveDate {
            t.date_naive()
        }
        fn start_of(&self, d: NaiveDate) -> DateTime<Utc> {
            Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
        }
    }

    fn state_for(root: &Path, data: &Path, managed: &str) -> AppState {
        fs::create_dir_all(data).unwrap();
        let managed_settings = data.join("managed-settings.json");
        if !managed.is_empty() {
            fs::write(&managed_settings, managed).unwrap();
        }
        AppState {
            store: Mutex::new(Store::open(&data.join("cache.sqlite")).unwrap()),
            price: Mutex::new(PriceTable::seed()),
            claude_roots: vec![root.to_path_buf()],
            settings: Mutex::new(AppSettings { monthly_budget_usd: 310.0, ..Default::default() }),
            price_path: data.join("prices.json"),
            settings_path: data.join("settings.json"),
            label_progress: Mutex::new(LabelProgress::default()),
            label_stop: Default::default(),
            triggers: Mutex::new(TriggerConfig::default()),
            triggers_path: data.join("triggers.json"),
            triggers_error: Mutex::new(None),
            home: None,
            gauge: Mutex::new(Tally::new()),
            managed_settings,
        }
    }

    #[test]
    fn the_report_hide_toggle_changes_that_setting_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("ci_hide_cmd_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let state = state_for(&dir.join("projects"), &dir.join("data"), "");
        // A budget set elsewhere in the app since the report button last read settings.
        state.settings.lock().unwrap().show_in_menu_bar = true;
        let after = set_report_hide(&state, true).unwrap();
        assert!(after.report_hide_projects);
        assert_eq!((after.monthly_budget_usd, after.show_in_menu_bar), (310.0, true));
        let saved = AppSettings::load_or_default(&state.settings_path);
        assert_eq!(saved, after, "the file holds the same settings");
        assert_eq!(*state.settings.lock().unwrap(), after);
        let off = set_report_hide(&state, false).unwrap();
        assert!(!off.report_hide_projects);
        assert_eq!(off.monthly_budget_usd, 310.0);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_panels_work_and_overhead_add_up_to_the_gauges_today() {
        let dir = std::env::temp_dir().join(format!("ci_gauge_cmd_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let proj = dir.join("projects").join("-Users-me-projA");
        fs::create_dir_all(&proj).unwrap();
        // The fixture ends on a compaction; the call that answers it is the overhead.
        let after = r#"{"type":"assistant","sessionId":"sa","timestamp":"2026-10-01T10:07:00.000Z","message":{"id":"msg_after","role":"assistant","model":"claude-opus-5-5","usage":{"input_tokens":2000,"output_tokens":500}}}"#;
        fs::write(proj.join("sa.jsonl"), format!("{}{after}\n", include_str!("../tests/fixtures/activity.jsonl"))).unwrap();
        let state = state_for(&dir.join("projects"), &dir.join("data"), "");
        let clock = UtcDay(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());

        let v = gauge_view(&state, &clock, true).unwrap();
        assert_eq!(v.day, "2026-10-01");
        assert_eq!(v.rate_source, RateSource::List);
        assert!(v.budget.today > 0.0);
        assert!((v.budget.daily_allowance - 10.0).abs() < 1e-9, "310 over October's 31 days");

        let split = work_split(&state, &clock).unwrap();
        assert!(split.overhead_usd > 0.0, "the fixture's compaction is overhead");
        assert!(split.work_usd > 0.0);
        // The same calls, the fixture's 3 web searches ($10 per 1,000) included in both.
        assert!((split.work_usd + split.overhead_usd - v.budget.today).abs() < 1e-9, "{split:?} vs {}", v.budget.today);

        // The next day has none of it.
        let next = UtcDay(NaiveDate::from_ymd_opt(2026, 10, 2).unwrap());
        assert_eq!(gauge_view(&state, &next, true).unwrap().budget.today, 0.0);
        assert_eq!(work_split(&state, &next).unwrap(), WorkSplit { work_usd: 0.0, overhead_usd: 0.0 });
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_managed_model_pricing_file_switches_the_rates_and_the_label() {
        let dir = std::env::temp_dir().join(format!("ci_gauge_managed_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let proj = dir.join("projects").join("-Users-me-projA");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join("sa.jsonl"), include_str!("../tests/fixtures/activity.jsonl")).unwrap();
        let clock = UtcDay(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        let list = gauge_view(&state_for(&dir.join("projects"), &dir.join("list"), ""), &clock, true).unwrap();
        let half = gauge_view(&state_for(&dir.join("projects"), &dir.join("conf"), r#"{"modelPricing":{"multiplier":0.5}}"#), &clock, true).unwrap();
        assert_eq!(half.rate_source, RateSource::Configured);
        assert!((half.budget.today - list.budget.today / 2.0).abs() < 1e-9);
        fs::remove_dir_all(&dir).ok();
    }

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
