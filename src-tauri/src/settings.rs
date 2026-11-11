//! User settings for the activity view, saved beside the cache DB.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// What is set for one Claude account (accounts.rs): its name and colour, and its
/// budget. Everything else is the machine's, in `AppSettings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountSettings {
    /// A name of the user's own; none keeps the organisation's.
    pub label: Option<String>,
    /// `#RRGGBB`; none takes the next colour of the palette.
    pub colour: Option<String>,
    /// Left out of the account picker and of "All accounts".
    pub hidden: bool,
    /// This account's monthly budget in US dollars. 0 is "not set".
    pub monthly_budget_usd: f64,
    /// Its ring turns amber at this share of today's allowance, in percent.
    pub amber_pct: f64,
    /// And red at this one.
    pub red_pct: f64,
}

impl Default for AccountSettings {
    fn default() -> Self {
        AccountSettings { label: None, colour: None, hidden: false, monthly_budget_usd: 0.0, amber_pct: 80.0, red_pct: 100.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// The local model the "local candidate" flag is judged against. URL and model
    /// are also what the optional labelling pass uses.
    pub ollama_url: String,
    pub ollama_model: String,
    /// Largest context (input + cache) the local model can take, in tokens.
    pub local_context_limit: i64,
    /// Largest single answer the local model is trusted to write, in tokens.
    pub local_output_limit: i64,
    /// The optional labelling pass by the local model (labeller.rs). Off by default.
    pub model_labels_enabled: bool,
    /// Let the pass use a non-loopback Ollama URL. Segment excerpts then leave
    /// this machine; the card warns. Off by default.
    pub allow_remote_ollama: bool,
    /// The budget from before accounts: each account has its own now
    /// (`accounts`). Read once, moved to the first account, then 0.
    pub monthly_budget_usd: f64,
    pub amber_pct: f64,
    pub red_pct: f64,
    /// Show the budget gauge in the menu bar. Off by default.
    pub show_in_menu_bar: bool,
    /// Letter the projects in a saved weekly report ("Project A") instead of naming
    /// them. Off by default.
    pub report_hide_projects: bool,
    /// Account id -> what is set for it.
    pub accounts: BTreeMap<String, AccountSettings>,
    /// Config folders to read besides those found (a `CLAUDE_CONFIG_DIR` elsewhere).
    pub extra_config_dirs: Vec<String>,
    /// Config folder -> the account it is counted in, where the user moved it.
    pub dir_accounts: BTreeMap<String, String>,
    /// The account the menu bar ring follows; none is all accounts.
    pub tray_account: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            ollama_url: "http://127.0.0.1:11434".into(),
            ollama_model: String::new(),
            local_context_limit: 32_000,
            local_output_limit: 4_000,
            model_labels_enabled: false,
            allow_remote_ollama: false,
            monthly_budget_usd: 0.0,
            amber_pct: 80.0,
            red_pct: 100.0,
            show_in_menu_bar: false,
            report_hide_projects: false,
            accounts: BTreeMap::new(),
            extra_config_dirs: Vec::new(),
            dir_accounts: BTreeMap::new(),
            tray_account: None,
        }
    }
}

impl AppSettings {
    /// What is set for `account`, or the defaults.
    pub fn account(&self, account: &str) -> AccountSettings {
        self.accounts.get(account).cloned().unwrap_or_default()
    }

    /// Move a budget saved before accounts to `account`, the one the old figures
    /// were mostly read from, unless it already has one. Returns whether anything moved.
    pub fn migrate_budget(&mut self, account: &str) -> bool {
        let old = (self.monthly_budget_usd, self.amber_pct, self.red_pct);
        if old == (0.0, 80.0, 100.0) {
            return false;
        }
        let entry = self.accounts.entry(account.to_string()).or_default();
        if entry.monthly_budget_usd == 0.0 {
            entry.monthly_budget_usd = old.0;
            entry.amber_pct = old.1;
            entry.red_pct = old.2;
        }
        // The ring followed this budget, so it keeps following it.
        if self.tray_account.is_none() {
            self.tray_account = Some(account.to_string());
        }
        (self.monthly_budget_usd, self.amber_pct, self.red_pct) = (0.0, 80.0, 100.0);
        true
    }

    /// Saved settings, or defaults.
    pub fn load_or_default(path: &Path) -> AppSettings {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_until_saved_then_saved_settings_win() {
        let dir = std::env::temp_dir().join(format!("al_settings_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let s = AppSettings::load_or_default(&path);
        assert_eq!(s, AppSettings::default());
        assert_eq!(s.local_context_limit, 32_000);

        let mine = AppSettings { local_context_limit: 128_000, ollama_model: "qwen3:32b".into(), ..s };
        mine.save(&path).unwrap();
        assert_eq!(AppSettings::load_or_default(&path), mine);

        // A file saved before the gauge existed loads with the gauge off and no budget.
        std::fs::write(&path, r#"{"ollama_model":"qwen3:32b"}"#).unwrap();
        let old = AppSettings::load_or_default(&path);
        assert!(!old.show_in_menu_bar);
        assert_eq!((old.monthly_budget_usd, old.amber_pct, old.red_pct), (0.0, 80.0, 100.0));

        assert!(!old.report_hide_projects, "a file from before 0.3 names projects in the report");

        // The report's hide setting is kept.
        let hide = AppSettings { report_hide_projects: true, ..AppSettings::default() };
        hide.save(&path).unwrap();
        assert!(AppSettings::load_or_default(&path).report_hide_projects);

        // A file saved by an older build with a field this one no longer has still loads.
        std::fs::write(&path, r#"{"ollama_model":"qwen3:32b","retired_field":"x"}"#).unwrap();
        assert_eq!(AppSettings::load_or_default(&path).ollama_model, "qwen3:32b");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_budget_from_before_accounts_moves_to_the_first_account_once() {
        let old: AppSettings = serde_json::from_str(r#"{"monthly_budget_usd":300.0,"amber_pct":70.0,"red_pct":95.0,"show_in_menu_bar":true}"#).unwrap();
        assert!(old.accounts.is_empty() && old.tray_account.is_none(), "a 0.3 file loads with no accounts");
        let mut s = old.clone();
        assert!(s.migrate_budget("acct:me:o1"));
        let a = s.account("acct:me:o1");
        assert_eq!((a.monthly_budget_usd, a.amber_pct, a.red_pct), (300.0, 70.0, 95.0));
        assert_eq!(s.tray_account.as_deref(), Some("acct:me:o1"), "the ring keeps following that budget");
        assert_eq!(s.monthly_budget_usd, 0.0);
        assert!(s.show_in_menu_bar);
        assert!(!s.migrate_budget("acct:me:o1"), "once");
        assert_eq!(s.account("acct:other:o2"), AccountSettings::default());

        // An account that already has a budget keeps it.
        let mut t = old;
        t.accounts.insert("x".into(), AccountSettings { monthly_budget_usd: 50.0, ..Default::default() });
        t.migrate_budget("x");
        assert_eq!(t.account("x").monthly_budget_usd, 50.0);
    }
}
