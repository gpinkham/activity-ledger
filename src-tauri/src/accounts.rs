//! Which Claude accounts this machine's transcripts belong to.
//!
//! Claude Code keeps one config folder per login: `~/.claude` by default, or the
//! folder `CLAUDE_CONFIG_DIR` names (a second subscription is often run as
//! `CLAUDE_CONFIG_DIR=~/.claude-work claude`). Each folder holds its own
//! `projects/` transcripts. Folders are found in the home folder and, on Windows,
//! in every WSL user's home, plus any folder added in settings.
//!
//! A folder's login is read from the `oauthAccount` block of its `.claude.json`
//! (for the default `~/.claude`, the file beside it in the home folder). Only the
//! account and organisation ids, the organisation's name and kind, and the email
//! are read; the credentials file is never opened. Folders signed in to the same
//! account and organisation are one account. A folder with no login read is an
//! account of its own, and settings can move any folder to another account.
//!
//! A folder signed out and back in as someone else gives all its history to the
//! login it has now: the transcripts do not say which login wrote them.

use crate::settings::{AccountSettings, AppSettings};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The login a config folder is signed in to.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Identity {
    pub account_uuid: String,
    pub org_uuid: String,
    pub org_name: Option<String>,
    pub org_type: Option<String>,
    pub email: Option<String>,
}

impl Identity {
    pub fn account_id(&self) -> String {
        format!("acct:{}:{}", self.account_uuid, self.org_uuid)
    }

    /// A personal plan's organisation is named after its owner ("Ana's Organization").
    fn personal(&self) -> bool {
        self.org_name.as_deref().is_none_or(|n| n.ends_with("'s Organization") || n.ends_with("’s Organization"))
            || matches!(self.org_type.as_deref(), Some("claude_pro" | "claude_max" | "claude_free"))
    }
}

/// One config folder found on this machine.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigDir {
    /// The config folder (`…/.claude-work`).
    pub dir: PathBuf,
    /// Where it was found: "Windows", "macOS", "Linux", "WSL · Ubuntu", or "Added".
    pub origin: String,
    pub identity: Option<Identity>,
    /// Added by hand in settings, rather than found.
    pub custom: bool,
}

impl ConfigDir {
    pub fn projects(&self) -> PathBuf {
        self.dir.join("projects")
    }

    pub fn key(&self) -> String {
        self.dir.to_string_lossy().to_string()
    }

    /// The account this folder belongs to before any setting moves it.
    pub fn own_account(&self) -> String {
        match &self.identity {
            Some(i) => i.account_id(),
            None => format!("dir:{}", self.key()),
        }
    }
}

/// A folder of transcripts and the account it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    pub projects: PathBuf,
    pub account: String,
}

/// A home folder to look in, and what to call it.
#[derive(Debug, Clone, PartialEq)]
pub struct Home {
    pub path: PathBuf,
    pub origin: String,
}

fn local_origin() -> &'static str {
    if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    }
}

/// This user's home, and on Windows each WSL user's (`\\wsl.localhost\<distro>\home\<user>`, and root's).
pub fn homes(home: &Path) -> Vec<Home> {
    let mut out = vec![Home { path: home.to_path_buf(), origin: local_origin().into() }];
    for distro in crate::scanner::wsl_distros() {
        let base = PathBuf::from(format!(r"\\wsl.localhost\{distro}"));
        let users = std::fs::read_dir(base.join("home")).into_iter().flatten().flatten().map(|e| e.path());
        for h in users.chain([base.join("root")]) {
            out.push(Home { path: h, origin: format!("WSL · {distro}") });
        }
    }
    out
}

/// Is `name` a Claude Code config folder's name: `.claude`, `.claude-*` or `.claude_*`?
pub fn config_dir_name(name: &str) -> bool {
    name == ".claude" || name.strip_prefix(".claude-").or_else(|| name.strip_prefix(".claude_")).is_some_and(|rest| !rest.is_empty())
}

/// The config folders in one home: those named like one that hold a `projects` folder.
pub fn config_dirs_in(home: &Home) -> Vec<ConfigDir> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&home.path)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(config_dir_name))
        .map(|e| e.path())
        .filter(|p| p.join("projects").is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|dir| ConfigDir { identity: identity(&dir), dir, origin: home.origin.clone(), custom: false })
        .collect()
}

/// The `oauthAccount` block of a config folder's `.claude.json`. The default folder's
/// file sits beside it in the home folder; a `CLAUDE_CONFIG_DIR` folder's, inside it.
pub fn identity(dir: &Path) -> Option<Identity> {
    let inside = dir.join(".claude.json");
    let beside = dir.parent().map(|p| p.join(".claude.json"));
    let default_dir = dir.file_name().and_then(|n| n.to_str()) == Some(".claude");
    let order: Vec<PathBuf> = if default_dir { beside.into_iter().chain([inside]).collect() } else { vec![inside] };
    order.iter().find_map(|p| read_identity(p))
}

fn read_identity(path: &Path) -> Option<Identity> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let a = v.get("oauthAccount")?;
    let s = |k: &str| a.get(k).and_then(|x| x.as_str()).filter(|s| !s.is_empty()).map(str::to_string);
    Some(Identity {
        account_uuid: s("accountUuid")?,
        org_uuid: s("organizationUuid").unwrap_or_default(),
        org_name: s("organizationName"),
        org_type: s("organizationType"),
        email: s("emailAddress"),
    })
}

/// A folder added in settings: the config folder itself, or its `projects` folder.
pub fn custom_dir(path: &str) -> Option<ConfigDir> {
    let p = PathBuf::from(path.trim());
    let dir = if p.file_name().and_then(|n| n.to_str()) == Some("projects") { p.parent()?.to_path_buf() } else { p };
    if !dir.join("projects").is_dir() {
        return None;
    }
    Some(ConfigDir { identity: identity(&dir), dir, origin: "Added".into(), custom: true })
}

/// Every config folder: those found in `homes`, then those added in settings that
/// were not found already.
pub fn discover_in(homes: &[Home], extra: &[String]) -> Vec<ConfigDir> {
    let mut dirs: Vec<ConfigDir> = homes.iter().flat_map(config_dirs_in).collect();
    for path in extra {
        if let Some(d) = custom_dir(path) {
            if !dirs.iter().any(|x| x.dir == d.dir) {
                dirs.push(d);
            }
        }
    }
    dirs
}

/// What the app reads: the folders, and which account each belongs to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Discovery {
    pub dirs: Vec<ConfigDir>,
    /// Folder key -> account id, settings' moves applied.
    pub assigned: BTreeMap<String, String>,
}

impl Discovery {
    pub fn new(dirs: Vec<ConfigDir>, settings: &AppSettings) -> Discovery {
        let assigned = dirs
            .iter()
            .map(|d| {
                let id = settings.dir_accounts.get(&d.key()).cloned().unwrap_or_else(|| d.own_account());
                (d.key(), id)
            })
            .collect();
        Discovery { dirs, assigned }
    }

    /// Look on this machine.
    pub fn scan(home: &Path, settings: &AppSettings) -> Discovery {
        Discovery::new(discover_in(&homes(home), &settings.extra_config_dirs), settings)
    }

    pub fn account_of(&self, d: &ConfigDir) -> String {
        self.assigned.get(&d.key()).cloned().unwrap_or_else(|| d.own_account())
    }

    pub fn roots(&self) -> Vec<Root> {
        self.dirs.iter().map(|d| Root { projects: d.projects(), account: self.account_of(d) }).collect()
    }

    /// The accounts, in the order their first folder was found, with their settings.
    pub fn accounts(&self, settings: &AppSettings) -> Vec<Account> {
        let mut out: Vec<Account> = Vec::new();
        for d in &self.dirs {
            let id = self.account_of(d);
            let info = DirInfo { path: d.key(), origin: d.origin.clone(), custom: d.custom, own_account: d.own_account() };
            if let Some(a) = out.iter_mut().find(|a| a.id == id) {
                a.dirs.push(info);
                if a.email.is_none() && a.org_name.is_none() {
                    fill_identity(a, d);
                }
                continue;
            }
            let mut a = Account {
                id: id.clone(),
                label: String::new(),
                default_label: String::new(),
                plan: None,
                email: None,
                org_name: None,
                dirs: vec![info],
                settings: settings.accounts.get(&id).cloned().unwrap_or_default(),
                colour: String::new(),
            };
            fill_identity(&mut a, d);
            out.push(a);
        }
        for (i, a) in out.iter_mut().enumerate() {
            a.label = a.settings.label.clone().filter(|l| !l.trim().is_empty()).unwrap_or_else(|| a.default_label.clone());
            a.colour = a.settings.colour.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| PALETTE[i % PALETTE.len()].into());
        }
        // Two accounts with the same name would read as one: say where each lives.
        let labels: Vec<String> = out.iter().map(|a| a.label.clone()).collect();
        for a in out.iter_mut() {
            if a.settings.label.is_none() && labels.iter().filter(|l| **l == a.label).count() > 1 {
                if let Some(d) = a.dirs.first() {
                    a.label = format!("{} ({})", a.label, dir_label(Path::new(&d.path)));
                }
            }
        }
        out
    }
}

/// Distinct, readable on light and dark: the order accounts get them in.
pub const PALETTE: [&str; 6] = ["#3B82F6", "#F59E0B", "#10B981", "#EC4899", "#8B5CF6", "#14B8A6"];

fn dir_label(dir: &Path) -> String {
    dir.file_name().and_then(|n| n.to_str()).unwrap_or("").trim_start_matches('.').to_string()
}

fn plan_name(org_type: &str) -> Option<&'static str> {
    Some(match org_type {
        "claude_free" => "Free",
        "claude_pro" => "Pro",
        "claude_max" => "Max",
        "claude_team" => "Team",
        "claude_enterprise" => "Enterprise",
        _ => return None,
    })
}

fn fill_identity(a: &mut Account, d: &ConfigDir) {
    match &d.identity {
        Some(i) => {
            a.email = i.email.clone();
            a.org_name = i.org_name.clone().filter(|_| !i.personal());
            a.plan = i.org_type.as_deref().and_then(plan_name).map(str::to_string);
            a.default_label = match &a.org_name {
                Some(org) => org.clone(),
                None => "Personal".into(),
            };
        }
        None => a.default_label = dir_label(&d.dir),
    }
}

/// A config folder as the settings card shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DirInfo {
    pub path: String,
    pub origin: String,
    pub custom: bool,
    /// The account its own login makes it part of, before any move.
    pub own_account: String,
}

/// One account, as the app shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Account {
    pub id: String,
    /// The name shown: the one set in settings, else `default_label`.
    pub label: String,
    /// The organisation's name, "Personal" for a personal plan, or the folder's name.
    pub default_label: String,
    /// "Max", "Enterprise"…
    pub plan: Option<String>,
    pub email: Option<String>,
    pub org_name: Option<String>,
    pub dirs: Vec<DirInfo>,
    pub settings: AccountSettings,
    /// The colour set in settings, else one from `PALETTE`.
    pub colour: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("al_accounts_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn login(account: &str, org: &str, org_name: &str, org_type: &str) -> String {
        serde_json::json!({"numStartups": 3, "oauthAccount": {
            "accountUuid": account, "organizationUuid": org, "organizationName": org_name,
            "organizationType": org_type, "emailAddress": format!("{account}@example.com")}})
        .to_string()
    }

    fn home(path: &Path, origin: &str) -> Home {
        Home { path: path.to_path_buf(), origin: origin.into() }
    }

    #[test]
    fn config_folder_names() {
        for yes in [".claude", ".claude-work", ".claude_acme"] {
            assert!(config_dir_name(yes), "{yes}");
        }
        for no in [".claude.json", ".claude-", "claude", ".claudeX", ".config"] {
            assert!(!config_dir_name(no), "{no}");
        }
    }

    #[test]
    fn finds_each_config_folder_that_holds_transcripts_and_its_login() {
        let h = tmp("find");
        fs::create_dir_all(h.join(".claude").join("projects")).unwrap();
        fs::create_dir_all(h.join(".claude-work").join("projects")).unwrap();
        fs::create_dir_all(h.join(".claude-empty")).unwrap(); // no projects: not a config folder in use
        fs::write(h.join(".claude.json"), login("me", "o1", "Me's Organization", "claude_max")).unwrap();
        fs::write(h.join(".claude-work").join(".claude.json"), login("me2", "acme", "Acme", "claude_enterprise")).unwrap();
        fs::write(h.join(".claude.json.backup"), "{}").unwrap();

        let dirs = config_dirs_in(&home(&h, "Linux"));
        assert_eq!(dirs.iter().map(|d| dir_label(&d.dir)).collect::<Vec<_>>(), vec!["claude", "claude-work"]);
        assert_eq!(dirs[0].identity.as_ref().unwrap().account_uuid, "me", "the default folder's login is beside it");
        assert_eq!(dirs[1].identity.as_ref().unwrap().org_uuid, "acme", "a CLAUDE_CONFIG_DIR folder's is inside it");

        let accounts = Discovery::new(dirs, &AppSettings::default()).accounts(&AppSettings::default());
        assert_eq!(accounts.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), vec!["Personal", "Acme"]);
        assert_eq!(accounts[0].plan.as_deref(), Some("Max"));
        assert_eq!(accounts[1].plan.as_deref(), Some("Enterprise"));
        assert_ne!(accounts[0].colour, accounts[1].colour);
        fs::remove_dir_all(&h).ok();
    }

    #[test]
    fn the_default_folder_falls_back_to_a_file_inside_it_and_a_folder_with_no_login_is_its_own_account() {
        let h = tmp("fallback");
        fs::create_dir_all(h.join(".claude").join("projects")).unwrap();
        fs::write(h.join(".claude").join(".claude.json"), login("me", "o1", "Me's Organization", "claude_pro")).unwrap();
        fs::create_dir_all(h.join(".claude-old").join("projects")).unwrap();
        fs::write(h.join(".claude-old").join(".claude.json"), r#"{"numStartups":1}"#).unwrap();

        let dirs = config_dirs_in(&home(&h, "Linux"));
        assert_eq!(dirs[0].identity.as_ref().unwrap().account_uuid, "me");
        assert!(dirs[1].identity.is_none());
        assert!(dirs[1].own_account().starts_with("dir:"));
        let accounts = Discovery::new(dirs, &AppSettings::default()).accounts(&AppSettings::default());
        assert_eq!(accounts[1].label, "claude-old", "no login: named after its folder");
        fs::remove_dir_all(&h).ok();
    }

    #[test]
    fn two_folders_signed_in_to_one_account_are_one_account_unless_settings_split_them() {
        let win = tmp("win");
        let wsl = tmp("wsl");
        for h in [&win, &wsl] {
            fs::create_dir_all(h.join(".claude").join("projects")).unwrap();
            fs::write(h.join(".claude.json"), login("me", "o1", "Me's Organization", "claude_max")).unwrap();
        }
        fs::create_dir_all(wsl.join(".claude-work").join("projects")).unwrap();
        fs::write(wsl.join(".claude-work").join(".claude.json"), login("me", "acme", "Acme", "claude_enterprise")).unwrap();
        let homes = [home(&win, "Windows"), home(&wsl, "WSL · Ubuntu")];

        let settings = AppSettings::default();
        let found = Discovery::new(discover_in(&homes, &[]), &settings);
        let accounts = found.accounts(&settings);
        assert_eq!(accounts.len(), 2, "same user, another organisation: another account");
        assert_eq!(accounts[0].dirs.iter().map(|d| d.origin.as_str()).collect::<Vec<_>>(), vec!["Windows", "WSL · Ubuntu"]);
        let roots = found.roots();
        assert_eq!(roots[0].account, roots[1].account);
        assert_ne!(roots[1].account, roots[2].account);

        // Moved to an account of its own.
        let wsl_dir = wsl.join(".claude").to_string_lossy().to_string();
        let mut split = AppSettings::default();
        split.dir_accounts.insert(wsl_dir.clone(), format!("dir:{wsl_dir}"));
        let accounts = Discovery::new(discover_in(&homes, &[]), &split).accounts(&split);
        assert_eq!(accounts.len(), 3);
        assert_eq!(accounts[0].label, "Personal (claude)", "two of one name say which folder");
        assert_eq!(accounts[1].label, "Personal (claude)");

        // Moved into another account: its transcripts count there.
        let mut merged = AppSettings::default();
        let work = roots[2].account.clone();
        merged.dir_accounts.insert(wsl_dir, work.clone());
        let found = Discovery::new(discover_in(&homes, &[]), &merged);
        assert_eq!(found.roots()[1].account, work);
        assert_eq!(found.accounts(&merged).len(), 2);
        fs::remove_dir_all(&win).ok();
        fs::remove_dir_all(&wsl).ok();
    }

    #[test]
    fn a_folder_added_in_settings_is_read_once_and_a_projects_folder_means_its_parent() {
        let h = tmp("custom");
        let elsewhere = h.join("elsewhere").join("cfg");
        fs::create_dir_all(elsewhere.join("projects")).unwrap();
        fs::create_dir_all(h.join(".claude").join("projects")).unwrap();
        let extra = vec![
            elsewhere.join("projects").to_string_lossy().to_string(),
            h.join(".claude").to_string_lossy().to_string(), // already found
            h.join("missing").to_string_lossy().to_string(),
        ];
        let dirs = discover_in(&[home(&h, "Linux")], &extra);
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[1].dir, elsewhere);
        assert!(dirs[1].custom);
        assert_eq!(dirs[1].origin, "Added");
        fs::remove_dir_all(&h).ok();
    }

    #[test]
    fn a_name_and_colour_set_in_settings_take_precedence() {
        let h = tmp("named");
        fs::create_dir_all(h.join(".claude").join("projects")).unwrap();
        let dirs = config_dirs_in(&home(&h, "Linux"));
        let id = dirs[0].own_account();
        let mut s = AppSettings::default();
        s.accounts.insert(id, AccountSettings { label: Some("Side project".into()), colour: Some("#123456".into()), ..Default::default() });
        let a = &Discovery::new(dirs, &s).accounts(&s)[0];
        assert_eq!((a.label.as_str(), a.colour.as_str()), ("Side project", "#123456"));
        fs::remove_dir_all(&h).ok();
    }
}
