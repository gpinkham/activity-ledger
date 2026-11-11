use crate::accounts::{Account, Discovery, Root};
use crate::activity::{ActivityReport, LabelSource};
use crate::budget::{self, BudgetView, Thresholds};
use crate::gauge::{iso, Clock, SystemClock, Tally};
use crate::exporter::build_bundle;
use crate::labeller::{self, LabelProgress};
use crate::metrics::{ByAccount, ByDay, ByModel, ByProject, ByTool, Filters, Totals, Workflow};
use crate::parser::parse_with;
use crate::pricing::{GaugePrices, ManagedPricing, PriceTable, RateSource};
use crate::scanner::{scan_roots, unassigned};
use crate::sessions::{SessionDetail, SessionSummary};
use crate::settings::{AccountSettings, AppSettings};
use crate::store::Store;
use crate::triggers::{Matcher, TriggerConfig};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub store: Mutex<Store>,
    pub price: Mutex<PriceTable>,
    /// Every Claude config folder found, and the account each belongs to (accounts.rs).
    /// Looked for again on Refresh and when an account setting changes.
    pub discovery: Mutex<Discovery>,
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
    do_refresh_roots(store, &unassigned(root), matcher)
}

/// do_refresh_with, over every account's transcript folders. An unchanged file
/// whose folder now counts in another account is moved there without a re-read.
pub fn do_refresh_roots(store: &mut Store, roots: &[Root], matcher: &Matcher) -> rusqlite::Result<RefreshSummary> {
    store.use_triggers(&matcher.fingerprint())?;
    let scanned = scan_roots(roots);
    let mut parsed_count = 0;
    for sf in &scanned {
        let path = sf.path.to_string_lossy().to_string();
        if !store.needs_parse(&path, sf.size, sf.mtime) {
            store.reassign(&path, &sf.account)?;
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

/// The transcript folders to read, each with its account.
pub fn roots(state: &AppState) -> Result<Vec<Root>, String> {
    Ok(state.discovery.lock().map_err(|e| e.to_string())?.roots())
}

/// Look for config folders again, with the account settings as they are now.
pub fn rediscover(state: &AppState) -> Result<(), String> {
    let Some(home) = state.home.as_deref() else { return Ok(()) };
    let settings = state.settings.lock().map_err(|e| e.to_string())?.clone();
    *state.discovery.lock().map_err(|e| e.to_string())? = Discovery::scan(home, &settings);
    Ok(())
}

/// `f`, with the hidden accounts left out when it asks for every account.
pub fn scoped(state: &AppState, mut f: Filters) -> Result<Filters, String> {
    if f.account.is_none() {
        let settings = state.settings.lock().map_err(|e| e.to_string())?;
        f.exclude_accounts = settings.accounts.iter().filter(|(_, a)| a.hidden).map(|(id, _)| id.clone()).collect();
    }
    Ok(f)
}

/// Bring the cache up to date with every account's transcripts.
fn refresh_store(state: &AppState) -> Result<RefreshSummary, String> {
    let (config, _) = reload_triggers(state)?;
    let roots = roots(state)?;
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    store.sender_labels = config.sender_labels(state.home.as_deref());
    do_refresh_roots(&mut store, &roots, &config.matcher(state.home.as_deref())).map_err(|e| e.to_string())
}

/// Runs off the main thread (`async`): it looks for config folders again (on
/// Windows that runs `wsl.exe` and reads each WSL home) and then reads every new
/// transcript, which on the main thread held up every window call meanwhile.
#[tauri::command(async)]
pub fn refresh(state: tauri::State<AppState>) -> Result<RefreshSummary, String> {
    // A folder made since the last look (a new CLAUDE_CONFIG_DIR) is found here.
    rediscover(&state)?;
    refresh_store(&state)
}

macro_rules! metric_cmd {
    ($name:ident, $ret:ty, $call:ident) => {
        #[tauri::command]
        pub fn $name(state: tauri::State<AppState>, filters: Filters) -> Result<$ret, String> {
            let filters = scoped(&state, filters)?;
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
metric_cmd!(get_by_account, Vec<ByAccount>, by_account);

#[tauri::command]
pub fn get_by_tool(state: tauri::State<AppState>, filters: Filters) -> Result<Vec<ByTool>, String> {
    let filters = scoped(&state, filters)?;
    let store = state.store.lock().map_err(|e| e.to_string())?;
    store.by_tool(&filters).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_workflow(state: tauri::State<AppState>, filters: Filters) -> Result<Workflow, String> {
    let filters = scoped(&state, filters)?;
    let store = state.store.lock().map_err(|e| e.to_string())?;
    store.workflow(&filters).map_err(|e| e.to_string())
}

/// Every account found, hidden ones included (the settings card lists them).
#[tauri::command]
pub fn get_accounts(state: tauri::State<AppState>) -> Result<Vec<Account>, String> {
    accounts(&state)
}

pub fn accounts(state: &AppState) -> Result<Vec<Account>, String> {
    let settings = state.settings.lock().map_err(|e| e.to_string())?.clone();
    Ok(state.discovery.lock().map_err(|e| e.to_string())?.accounts(&settings))
}

/// What a settings change needs done to the folders found.
#[derive(PartialEq)]
enum Folders {
    /// A name, colour or budget: nothing.
    Same,
    /// A folder moved between accounts: the same folders, counted again, and the
    /// cache's rows moved with them.
    Reassign,
    /// A folder added or removed: look again, and read what is new.
    Rescan,
}

/// Change the settings with `edit`, save them, and bring the folders up to date.
/// Every command that calls it is `#[tauri::command(async)]`: a folder change
/// re-reads the cache (and on Windows can look in WSL), and each one repaints the
/// tray icon, which polls the transcript folders. On the main thread that would
/// hold up every window call meanwhile, as get_gauge_split did.
fn edit_settings(app: &tauri::AppHandle, state: &AppState, folders: Folders, edit: impl FnOnce(&mut AppSettings)) -> Result<Vec<Account>, String> {
    let next = {
        let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
        let mut next = settings.clone();
        edit(&mut next);
        next.save(&state.settings_path).map_err(|e| e.to_string())?;
        *settings = next.clone();
        next
    };
    match folders {
        Folders::Same => {}
        Folders::Reassign => {
            let mut d = state.discovery.lock().map_err(|e| e.to_string())?;
            *d = Discovery::new(d.dirs.clone(), &next);
        }
        Folders::Rescan => rediscover(state)?,
    }
    if folders != Folders::Same {
        refresh_store(state)?;
        // The gauge's tally keeps what it read; its next poll relabels the files.
    }
    crate::tray::refresh_icon(app);
    accounts(state)
}

/// Save one account's name, colour, hidden flag and budget. Only that account's
/// entry is written, so a change made elsewhere is not undone.
#[tauri::command(async)]
pub fn set_account_settings(app: tauri::AppHandle, state: tauri::State<AppState>, id: String, settings: AccountSettings) -> Result<Vec<Account>, String> {
    edit_settings(&app, &state, Folders::Same, |s| {
        let mut settings = settings;
        settings.label = settings.label.filter(|l| !l.trim().is_empty());
        s.accounts.insert(id, settings);
    })
}

/// Count a config folder in another account, or (none) in the one its login gives it.
#[tauri::command(async)]
pub fn set_dir_account(app: tauri::AppHandle, state: tauri::State<AppState>, dir: String, account: Option<String>) -> Result<Vec<Account>, String> {
    edit_settings(&app, &state, Folders::Reassign, |s| match account {
        Some(a) => {
            s.dir_accounts.insert(dir, a);
        }
        None => {
            s.dir_accounts.remove(&dir);
        }
    })
}

/// Read a config folder that was not found on its own (a `CLAUDE_CONFIG_DIR`
/// outside the home folder). Refused unless it holds a `projects` folder.
#[tauri::command(async)]
pub fn add_config_dir(app: tauri::AppHandle, state: tauri::State<AppState>, path: String) -> Result<Vec<Account>, String> {
    let dir = crate::accounts::custom_dir(&path)
        .ok_or_else(|| format!("{path} has no projects folder: not a Claude Code config folder"))?;
    let key = dir.key();
    edit_settings(&app, &state, Folders::Rescan, |s| {
        if !s.extra_config_dirs.contains(&key) {
            s.extra_config_dirs.push(key);
        }
    })
}

#[tauri::command(async)]
pub fn remove_config_dir(app: tauri::AppHandle, state: tauri::State<AppState>, path: String) -> Result<Vec<Account>, String> {
    let removed = path.clone();
    let accounts = edit_settings(&app, &state, Folders::Rescan, |s| {
        s.extra_config_dirs.retain(|p| *p != path);
        s.dir_accounts.remove(&path);
    })?;
    forget_unread_dir(&state, &removed)?;
    Ok(accounts)
}

/// Drop the cached rows of a config folder that is no longer read, so its
/// transcripts stop counting. Kept when the folder is still found on its own.
pub fn forget_unread_dir(state: &AppState, dir: &str) -> Result<(), String> {
    let still_read = state.discovery.lock().map_err(|e| e.to_string())?.dirs.iter().any(|d| d.key() == dir);
    if still_read {
        return Ok(());
    }
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    if store.forget_under(dir).map_err(|e| e.to_string())? > 0 {
        store.rebuild_sessions().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The account the menu bar ring follows; none is every account.
#[tauri::command(async)]
pub fn set_tray_account(app: tauri::AppHandle, state: tauri::State<AppState>, account: Option<String>) -> Result<Vec<Account>, String> {
    edit_settings(&app, &state, Folders::Same, |s| s.tray_account = account)
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
    let filters = scoped(&state, filters)?;
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
    let filters = scoped(&state, filters)?;
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
    {
        let mut current = state.settings.lock().map_err(|e| e.to_string())?;
        let next = with_accounts_of(settings, &current);
        next.save(&state.settings_path).map_err(|e| e.to_string())?;
        *current = next;
    }
    // The menu bar icon follows "Show in menu bar"; a new budget redraws the ring.
    crate::tray::sync(&app).map_err(|e| e.to_string())?;
    crate::tray::refresh_icon(&app);
    Ok(())
}

/// `incoming` with the account settings kept from `current`. Those have commands of
/// their own (set_account_settings and the rest); a card's whole-settings save
/// carries an older copy of them and must not undo a change made since.
pub fn with_accounts_of(incoming: AppSettings, current: &AppSettings) -> AppSettings {
    AppSettings {
        accounts: current.accounts.clone(),
        extra_config_dirs: current.extra_config_dirs.clone(),
        dir_accounts: current.dir_accounts.clone(),
        tray_account: current.tray_account.clone(),
        monthly_budget_usd: current.monthly_budget_usd,
        amber_pct: current.amber_pct,
        red_pct: current.red_pct,
        ..incoming
    }
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
    /// The account the ring follows; none is every account (shown hidden ones aside).
    pub account: Option<String>,
    /// That account's name, for the panel and the tooltip.
    pub account_label: Option<String>,
}

/// Read what the transcripts gained (when `poll`), then price today and the month,
/// for the account the ring follows (settings' `tray_account`).
pub fn gauge_view(state: &AppState, clock: &dyn Clock, poll: bool) -> Result<GaugeView, String> {
    let account = tray_account(state)?;
    gauge_view_for(state, clock, poll, account.as_deref())
}

/// The account the ring follows: settings' `tray_account` while it is found and
/// not hidden, else every account. The switchers offer only those, so the ring
/// never follows an account they cannot show.
pub fn tray_account(state: &AppState) -> Result<Option<String>, String> {
    let Some(id) = state.settings.lock().map_err(|e| e.to_string())?.tray_account.clone() else { return Ok(None) };
    let shown = accounts(state)?.iter().any(|a| a.id == id && !a.settings.hidden);
    Ok(shown.then_some(id))
}

/// gauge_view for `account`. Every account together has no budget, so no ring:
/// each account's budget is its own.
pub fn gauge_view_for(state: &AppState, clock: &dyn Clock, poll: bool, account: Option<&str>) -> Result<GaugeView, String> {
    let settings = state.settings.lock().map_err(|e| e.to_string())?.clone();
    let prices = GaugePrices {
        table: state.price.lock().map_err(|e| e.to_string())?.clone(),
        managed: ManagedPricing::load(&state.managed_settings),
    };
    let roots = if poll { roots(state)? } else { Vec::new() };
    let hidden = scoped(state, Filters::default())?.exclude_accounts;
    let spend = {
        let mut tally = state.gauge.lock().map_err(|e| e.to_string())?;
        if poll {
            tally.poll_roots(&roots, clock);
        }
        tally.spend_in(&prices, clock, |a| match account {
            Some(want) => a == want,
            None => !hidden.iter().any(|h| h == a),
        })
    };
    let (monthly, t) = match account {
        Some(id) => {
            let a = settings.account(id);
            (a.monthly_budget_usd, Thresholds { amber: a.amber_pct / 100.0, red: a.red_pct / 100.0 })
        }
        None => (0.0, Thresholds::default()),
    };
    let account_label = match account {
        Some(id) => accounts(state)?.into_iter().find(|a| a.id == id).map(|a| a.label),
        None => None,
    };
    Ok(GaugeView {
        day: spend.day.to_string(),
        budget: budget::view(monthly, spend.day, spend.today, spend.month_to_date, t),
        rate_source: prices.source(),
        account: account.map(str::to_string),
        account_label,
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
    refresh_store(state)?;
    let today = clock.today();
    let account = tray_account(state)?;
    let f = scoped(state, Filters {
        from_ts: Some(iso(clock.start_of(today))),
        to_ts: Some(iso(clock.start_of(today.succ_opt().unwrap_or(today)))),
        account,
        ..Default::default()
    })?;
    let store = state.store.lock().map_err(|e| e.to_string())?;
    let price = state.price.lock().map_err(|e| e.to_string())?;
    let settings = state.settings.lock().map_err(|e| e.to_string())?;
    let report = store.activity_by(&price, &f, &settings, LabelSource::Rule).map_err(|e| e.to_string())?;
    let overhead: f64 = report.rows.iter().filter(|r| r.group == "overhead").map(|r| r.cost_usd).sum();
    let all: f64 = report.rows.iter().map(|r| r.cost_usd).sum();
    Ok(WorkSplit { work_usd: all - overhead, overhead_usd: overhead })
}

/// Runs off the main thread (`async`): it brings the whole cache up to date, which
/// can take seconds, and on the main thread it held up every window call in the
/// meantime, the panel's `fit_panel` among them, so the panel sat at its loading
/// height with a scrollbar until the split came back.
#[tauri::command(async)]
pub fn get_gauge_split(state: tauri::State<AppState>) -> Result<WorkSplit, String> {
    work_split(&state, &SystemClock)
}

#[tauri::command]
pub fn open_main_window(app: tauri::AppHandle) {
    crate::tray::show_main(&app);
}

/// The panel page's content height in logical points; the panel window is sized
/// to it (tray.rs `fit_panel`), so it never needs a scrollbar.
#[tauri::command]
pub fn fit_panel(app: tauri::AppHandle, height: f64) {
    crate::tray::fit_panel(&app, height);
}

/// The panel's "Quit Activity Ledger". The menu bar icon has no menu of its own
/// (tray.rs), so this is how the app is quit while it lives in the menu bar.
#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
pub fn get_sessions(state: tauri::State<AppState>, filters: Filters) -> Result<Vec<SessionSummary>, String> {
    let filters = scoped(&state, filters)?;
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
    let filters = scoped(&state, filters)?;
    let store = state.store.lock().map_err(|e| e.to_string())?;
    let price = state.price.lock().map_err(|e| e.to_string())?;
    let generated_at = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let bundle = build_bundle(&store, &price, &filters, &generated_at).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())
}

/// The weekly report for the local week starting on `week_start` (a Monday,
/// `YYYY-MM-DD`), as Markdown. The front end saves it through `save_export`.
#[tauri::command]
pub fn weekly_report(state: tauri::State<AppState>, week_start: String, hide_projects: bool, account: Option<String>) -> Result<String, String> {
    let monday = crate::report::parse_week_start(&week_start)?;
    let scope = report_scope(&state, account)?;
    let store = state.store.lock().map_err(|e| e.to_string())?;
    let price = state.price.lock().map_err(|e| e.to_string())?;
    let written = chrono::Local::now().format("%-d %b %Y at %H:%M").to_string();
    crate::report::build_report(&store, &price, &SystemClock, monday, hide_projects, &scope, &written).map_err(|e| e.to_string())
}

/// The report's accounts: `account`, or every account not hidden, with their names.
pub fn report_scope(state: &AppState, account: Option<String>) -> Result<crate::report::Scope, String> {
    let exclude = scoped(state, Filters::default())?.exclude_accounts;
    let labels = accounts(state)?
        .into_iter()
        .filter(|a| account.as_ref() == Some(&a.id) || (account.is_none() && !exclude.contains(&a.id)))
        .map(|a| (a.id, a.label))
        .collect();
    Ok(crate::report::Scope { account, labels, exclude })
}

/// The tray's "Save weekly report": bring up the main window and ask it to start
/// its save flow, for the account the ring follows (none: every account). The tray
/// itself writes nothing.
#[tauri::command]
pub fn open_weekly_report(app: tauri::AppHandle, state: tauri::State<AppState>) {
    use tauri::Emitter;
    let account = tray_account(&state).ok().flatten();
    crate::tray::show_main(&app);
    let _ = app.emit_to("main", "start-weekly-report", serde_json::json!({ "account": account }));
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

    use crate::accounts::{ConfigDir, Identity};

    const ME: &str = "acct:me:o1";

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
        // One config folder, `root`'s parent, signed in to ME, whose budget is 310.
        let dir = ConfigDir {
            dir: root.parent().unwrap().to_path_buf(),
            origin: "Test".into(),
            identity: Some(Identity { account_uuid: "me".into(), org_uuid: "o1".into(), ..Default::default() }),
            custom: false,
        };
        let mut settings = AppSettings { tray_account: Some(ME.into()), ..Default::default() };
        settings.accounts.insert(ME.into(), AccountSettings { monthly_budget_usd: 310.0, ..Default::default() });
        AppState {
            store: Mutex::new(Store::open(&data.join("cache.sqlite")).unwrap()),
            price: Mutex::new(PriceTable::seed()),
            discovery: Mutex::new(Discovery::new(vec![dir], &settings)),
            settings: Mutex::new(settings),
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
        assert_eq!((after.account(ME).monthly_budget_usd, after.show_in_menu_bar), (310.0, true));
        let saved = AppSettings::load_or_default(&state.settings_path);
        assert_eq!(saved, after, "the file holds the same settings");
        assert_eq!(*state.settings.lock().unwrap(), after);
        let off = set_report_hide(&state, false).unwrap();
        assert!(!off.report_hide_projects);
        assert_eq!(off.account(ME).monthly_budget_usd, 310.0);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_whole_settings_save_keeps_the_account_settings_it_did_not_see() {
        let current = {
            let mut s = AppSettings { tray_account: Some(ME.into()), extra_config_dirs: vec!["/x".into()], ..Default::default() };
            s.accounts.insert(ME.into(), AccountSettings { monthly_budget_usd: 99.0, ..Default::default() });
            s
        };
        // A card's copy from before that budget was set, with its own change.
        let card = AppSettings { ollama_model: "qwen3:32b".into(), ..Default::default() };
        let next = with_accounts_of(card, &current);
        assert_eq!(next.ollama_model, "qwen3:32b");
        assert_eq!(next.account(ME).monthly_budget_usd, 99.0);
        assert_eq!(next.tray_account.as_deref(), Some(ME));
        assert_eq!(next.extra_config_dirs, vec!["/x".to_string()]);
    }

    /// Two accounts' folders, one call each today: Personal $5, Acme $25.
    fn two_accounts(name: &str) -> (PathBuf, AppState) {
        let dir = std::env::temp_dir().join(format!("ci_two_accounts_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let call = |id: &str, input: i64| {
            format!(r#"{{"type":"assistant","sessionId":"s-{id}","timestamp":"2026-10-01T10:00:00.000Z","message":{{"id":"{id}","role":"assistant","model":"claude-opus-4-8","usage":{{"input_tokens":{input}}}}}}}"#) + "\n"
        };
        for (cfg, id, input) in [(".claude", "m-personal", 1_000_000), (".claude-work", "m-work", 5_000_000)] {
            // The same project folder used from both accounts.
            let proj = dir.join("home").join(cfg).join("projects").join("-home-me-app");
            fs::create_dir_all(&proj).unwrap();
            fs::write(proj.join(format!("{id}.jsonl")), call(id, input)).unwrap();
        }
        let state = state_for(&dir.join("home").join(".claude").join("projects"), &dir.join("data"), "");
        let work = ConfigDir {
            dir: dir.join("home").join(".claude-work"),
            origin: "Test".into(),
            identity: Some(Identity { account_uuid: "me".into(), org_uuid: "acme".into(), org_name: Some("Acme".into()), ..Default::default() }),
            custom: false,
        };
        {
            let settings = state.settings.lock().unwrap().clone();
            let mut d = state.discovery.lock().unwrap();
            let mut dirs = d.dirs.clone();
            dirs.push(work);
            *d = Discovery::new(dirs, &settings);
        }
        (dir, state)
    }

    #[test]
    fn each_account_is_counted_apart_and_a_hidden_one_leaves_all_accounts() {
        let (dir, state) = two_accounts("split");
        refresh_store(&state).unwrap();
        let p = PriceTable::seed();
        let cost = |f: Filters| {
            let f = scoped(&state, f).unwrap();
            state.store.lock().unwrap().totals(&p, &f).unwrap().est_cost_usd
        };
        let only = |a: &str| Filters { account: Some(a.into()), ..Default::default() };
        assert!((cost(only(ME)) - 5.0).abs() < 1e-9);
        assert!((cost(only("acct:me:acme")) - 25.0).abs() < 1e-9);
        assert!((cost(Filters::default()) - 30.0).abs() < 1e-9);

        // One project folder, two accounts: two rows.
        let projects = state.store.lock().unwrap().by_project(&p, &Filters::default()).unwrap();
        let mut rows: Vec<(String, String)> = projects.iter().map(|r| (r.project.clone(), r.account.clone())).collect();
        rows.sort();
        assert_eq!(rows, vec![("-home-me-app".into(), "acct:me:acme".into()), ("-home-me-app".into(), ME.into())]);
        let sessions = state.store.lock().unwrap().get_sessions(&p, &only("acct:me:acme")).unwrap();
        assert_eq!(sessions.iter().map(|s| s.account.as_str()).collect::<Vec<_>>(), vec!["acct:me:acme"]);

        // Hidden: out of "all accounts", still there on its own.
        state.settings.lock().unwrap().accounts.insert("acct:me:acme".into(), AccountSettings { hidden: true, ..Default::default() });
        assert!((cost(Filters::default()) - 5.0).abs() < 1e-9);
        assert!((cost(only("acct:me:acme")) - 25.0).abs() < 1e-9);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_ring_follows_one_accounts_budget_and_all_accounts_has_none() {
        let (dir, state) = two_accounts("ring");
        let clock = UtcDay(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        let mine = gauge_view(&state, &clock, true).unwrap();
        assert_eq!(mine.account.as_deref(), Some(ME));
        assert_eq!(mine.account_label.as_deref(), Some("Personal"));
        assert!((mine.budget.today - 5.0).abs() < 1e-9, "only this account's call");
        assert_eq!(mine.budget.ring, budget::Ring::Green, "$5 of a $10 day");

        let work = gauge_view_for(&state, &clock, false, Some("acct:me:acme")).unwrap();
        assert!((work.budget.today - 25.0).abs() < 1e-9);
        assert_eq!(work.budget.ring, budget::Ring::None, "no budget set for it");

        let all = gauge_view_for(&state, &clock, false, None).unwrap();
        assert!((all.budget.today - 30.0).abs() < 1e-9);

        // The Overview's split, in one query: what totals gives for each account.
        refresh_store(&state).unwrap();
        let store = state.store.lock().unwrap();
        let split = store.by_account(&PriceTable::seed(), &Filters::default()).unwrap();
        assert_eq!(split.len(), 2);
        for row in &split {
            let one = store.totals(&PriceTable::seed(), &Filters { account: Some(row.account.clone()), ..Default::default() }).unwrap();
            assert!((row.cost_usd - one.est_cost_usd).abs() < 1e-9, "{}", row.account);
            assert_eq!(row.sessions, one.sessions);
        }
        drop(store);
        assert_eq!(all.budget.ring, budget::Ring::None);

        // The account it follows hidden: the switchers show every account, and so does the ring.
        state.settings.lock().unwrap().accounts.get_mut(ME).unwrap().hidden = true;
        let hidden = gauge_view(&state, &clock, false).unwrap();
        assert_eq!(hidden.account, None);
        assert!((hidden.budget.today - 25.0).abs() < 1e-9, "every account shown: the hidden one aside");
        assert_eq!(hidden.budget.ring, budget::Ring::None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_folder_moved_to_another_account_takes_its_rows_without_a_reread() {
        let (dir, state) = two_accounts("move");
        refresh_store(&state).unwrap();
        let work_dir = dir.join("home").join(".claude-work").to_string_lossy().to_string();
        {
            let mut s = state.settings.lock().unwrap();
            s.dir_accounts.insert(work_dir, ME.into());
            let mut d = state.discovery.lock().unwrap();
            *d = Discovery::new(d.dirs.clone(), &s);
        }
        let summary = refresh_store(&state).unwrap();
        assert_eq!(summary.files_parsed, 0, "moved, not read again");
        let t = state.store.lock().unwrap().totals(&PriceTable::seed(), &Filters { account: Some(ME.into()), ..Default::default() }).unwrap();
        assert!((t.est_cost_usd - 30.0).abs() < 1e-9);
        assert_eq!(accounts(&state).unwrap().len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_folder_no_longer_read_stops_counting_but_a_vanished_file_does_not() {
        let (dir, state) = two_accounts("forget");
        refresh_store(&state).unwrap();
        let work_dir = dir.join("home").join(".claude-work").to_string_lossy().to_string();
        let total = |s: &AppState| s.store.lock().unwrap().totals(&PriceTable::seed(), &Filters::default()).unwrap().est_cost_usd;
        let before = total(&state);

        // Still found: kept.
        forget_unread_dir(&state, &work_dir).unwrap();
        assert!((total(&state) - before).abs() < 1e-9);

        // A personal transcript deleted by Claude Code: its history stays.
        fs::remove_file(dir.join("home").join(".claude").join("projects").join("-home-me-app").join("m-personal.jsonl")).unwrap();
        refresh_store(&state).unwrap();
        assert!((total(&state) - before).abs() < 1e-9, "a deleted transcript keeps counting");

        // The work folder no longer read: its rows go.
        {
            let s = state.settings.lock().unwrap().clone();
            let mut d = state.discovery.lock().unwrap();
            let dirs = d.dirs.iter().filter(|x| x.key() != work_dir).cloned().collect();
            *d = Discovery::new(dirs, &s);
        }
        forget_unread_dir(&state, &work_dir).unwrap();
        let after = total(&state);
        assert!(after > 0.0 && after < before, "{after} of {before}");
        let sessions = state.store.lock().unwrap().totals(&PriceTable::seed(), &Filters::default()).unwrap();
        assert_eq!(sessions.sessions, 1, "only the personal session is left");
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
    fn the_split_runs_off_the_main_thread() {
        // A text check, not a behaviour one: on the main thread the split held up
        // fit_panel, and the panel sat at its loading height with a scrollbar.
        let src = include_str!("commands.rs");
        assert!(
            src.contains("#[tauri::command(async)]\npub fn get_gauge_split("),
            "get_gauge_split must stay #[tauri::command(async)]"
        );
    }

    #[test]
    fn the_account_settings_commands_run_off_the_main_thread() {
        // The same text check: each re-reads folders or polls them for the tray icon,
        // and refresh looks for config folders again, in WSL too.
        let src = include_str!("commands.rs");
        for cmd in ["refresh", "set_account_settings", "set_dir_account", "add_config_dir", "remove_config_dir", "set_tray_account"] {
            assert!(src.contains(&format!("#[tauri::command(async)]\npub fn {cmd}(")), "{cmd} must be #[tauri::command(async)]");
        }
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
