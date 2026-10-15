//! The harness trigger file: how to tell a harness's own upkeep from the work.
//!
//! `triggers.json` in the app data folder lists adapters, one per agent harness.
//! An adapter that is on says which opening lines start upkeep (an inbox wake,
//! memory upkeep), which senders in the harness's own log mark a standup or a
//! scheduled job, and which files are its bookkeeping (inbox, memory, board).
//! A segment it starts is overhead only while it does nothing but bookkeeping or
//! planning; once it does the work, it is that work.
//!
//! The file holds phrases and paths only. The log is read in memory to pick a
//! label; its subjects and bodies are never stored.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The file the app writes on first run. Munder Difflin is the first adapter.
pub const DEFAULT: &str = include_str!("../triggers.default.json");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerConfig {
    pub version: u32,
    #[serde(default)]
    pub adapters: Vec<Adapter>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Adapter {
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    /// The harness's folder; `~` is the home folder. Its log is `<root>/log.jsonl`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default)]
    pub prompt_triggers: Vec<PromptTrigger>,
    #[serde(default)]
    pub log_senders: Vec<LogSender>,
    #[serde(default)]
    pub bookkeeping_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptTrigger {
    pub starts_with: String,
    pub label: String,
    /// Text after which the prompt names the log messages it delivers (ids,
    /// comma separated), so a sender can refine the label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids_after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogSender {
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_contains: Option<String>,
    pub label: String,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        serde_json::from_str(DEFAULT).expect("the bundled triggers.json parses")
    }
}

impl TriggerConfig {
    /// The saved file, or the bundled default. A missing file is written from the
    /// default; a broken one is left alone and the error is returned for the card.
    pub fn load_or_default(path: &Path) -> (TriggerConfig, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(c) => (c, None),
                Err(e) => (TriggerConfig::default(), Some(format!("{} could not be read ({e}); using the built-in default", path.display()))),
            },
            Err(_) => {
                std::fs::write(path, DEFAULT).ok();
                (TriggerConfig::default(), None)
            }
        }
    }

    /// Turn one adapter on or off in the file at `path`. A file that cannot be
    /// read is left as it is: it is the user's to fix.
    pub fn set_enabled(path: &Path, name: &str, enabled: bool) -> Result<TriggerConfig, String> {
        let (mut config, error) = TriggerConfig::load_or_default(path);
        if let Some(e) = error {
            return Err(format!("{e}. Fix or delete the file, then try again."));
        }
        let a = config.adapters.iter_mut().find(|a| a.name == name).ok_or_else(|| format!("no adapter named {name}"))?;
        a.enabled = enabled;
        config.save(path).map_err(|e| e.to_string())?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)
    }

    pub fn enabled(&self) -> impl Iterator<Item = &Adapter> {
        self.adapters.iter().filter(|a| a.enabled)
    }

    /// What the parser needs: the on adapters' opening lines, and their
    /// bookkeeping paths with the root they must be under.
    pub fn matcher(&self, home: Option<&Path>) -> Matcher {
        Matcher {
            prompts: self.enabled().flat_map(|a| a.prompt_triggers.iter().cloned()).collect(),
            paths: self
                .enabled()
                .filter(|a| !a.bookkeeping_paths.is_empty())
                .map(|a| Anchored {
                    root: a.root.as_deref().map(|r| expand(r, home).to_string_lossy().trim_end_matches('/').to_string()),
                    root_as_written: a.root.as_deref().map(|r| r.trim_end_matches('/').to_string()),
                    patterns: a.bookkeeping_paths.clone(),
                })
                .collect(),
        }
    }

    /// Message id -> label, from the logs of the on adapters that name senders.
    pub fn sender_labels(&self, home: Option<&Path>) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for a in self.enabled().filter(|a| !a.log_senders.is_empty()) {
            let Some(root) = a.root.as_deref().map(|r| expand(r, home)) else { continue };
            if let Ok(text) = std::fs::read_to_string(root.join("log.jsonl")) {
                read_log(&text, &a.log_senders, &mut out);
            }
        }
        out
    }

    /// Adapters that are off but whose harness folder is on this machine.
    pub fn found_off(&self, home: Option<&Path>) -> Vec<(String, String)> {
        self.adapters
            .iter()
            .filter(|a| !a.enabled)
            .filter_map(|a| {
                let root = a.root.as_deref()?;
                expand(root, home).join("log.jsonl").exists().then(|| (a.name.clone(), root.to_string()))
            })
            .collect()
    }
}

/// `~` or `~/x` against the home folder.
pub fn expand(root: &str, home: Option<&Path>) -> PathBuf {
    match (root.strip_prefix('~'), home) {
        (Some(rest), Some(h)) => h.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(root),
    }
}

/// Label each log message whose sender matches. The first matching rule wins.
fn read_log(text: &str, senders: &[LogSender], out: &mut HashMap<String, String>) {
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else { continue };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
        if s("kind") != "message" || s("id").is_empty() {
            continue;
        }
        let subject = s("subject").to_lowercase();
        let hit = senders.iter().find(|r| {
            r.from == s("from") && r.subject_contains.as_ref().map_or(true, |c| subject.contains(&c.to_lowercase()))
        });
        if let Some(r) = hit {
            out.insert(s("id").to_string(), r.label.clone());
        }
    }
}

/// The parse-time part of the config. Empty means no harness: today's behaviour.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Matcher {
    prompts: Vec<PromptTrigger>,
    paths: Vec<Anchored>,
}

/// One adapter's bookkeeping files and the folder they must be in.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Anchored {
    /// Expanded; None: anywhere.
    root: Option<String>,
    /// As in the file (`~/...`), for commands that name it that way.
    root_as_written: Option<String>,
    patterns: Vec<String>,
}

impl Anchored {
    /// Is `token` (a path as written, absolute, `~/` or relative to `cwd`) under the root?
    fn holds(&self, token: &str, cwd: &str) -> bool {
        let Some(root) = &self.root else { return true };
        let under = |p: &str, r: &str| p == r || p.starts_with(&format!("{r}/"));
        if token.starts_with('/') {
            under(token, root)
        } else if token.starts_with('~') {
            self.root_as_written.as_deref().is_some_and(|r| under(token, r))
        } else {
            under(cwd, root)
        }
    }
}

impl Matcher {
    /// The trigger label for a prompt's opening text, and the message ids it names.
    pub fn prompt(&self, text: &str) -> Option<(String, Vec<String>)> {
        let t = text.trim_start();
        let p = self.prompts.iter().find(|p| !p.starts_with.is_empty() && t.starts_with(&p.starts_with))?;
        let ids = p
            .ids_after
            .as_deref()
            .and_then(|after| t.split_once(after))
            .map(|(_, rest)| {
                // "a, b. Read your inbox": ids up to the first one followed by more text.
                let mut ids = Vec::new();
                for piece in rest.split(',') {
                    let p = piece.trim_start();
                    let id: String = p.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
                    if id.is_empty() {
                        break;
                    }
                    let more = p.len() > id.len();
                    ids.push(id);
                    if more {
                        break;
                    }
                }
                ids
            })
            .unwrap_or_default();
        Some((p.label.clone(), ids))
    }

    /// Does a path or shell command, run in `cwd`, touch a harness's bookkeeping
    /// files? Only files under that harness's root count.
    pub fn is_bookkeeping(&self, s: &str, cwd: &str) -> bool {
        self.paths.iter().any(|a| a.patterns.iter().any(|p| path_tokens(s, p).any(|t| a.holds(t, cwd))))
    }

    /// Changes whenever the parse result would: the cache is re-read then.
    pub fn fingerprint(&self) -> String {
        if self.prompts.is_empty() && self.paths.is_empty() {
            return String::new();
        }
        serde_json::to_string(&(&self.prompts, &self.paths)).unwrap_or_default()
    }
}

/// Characters that end a word in a shell command or a path list.
fn word_edge(c: char) -> bool {
    c.is_whitespace() || "\"'=(`;&|<>),".contains(c)
}

/// Each place `p` occurs in `s` at path boundaries, as the whole path written
/// there: "memory.md" matches "/a/memory.md" and "cat memory.md", not
/// "notmemory.md" or "memory.md.bak"; "/inbox/" also matches "inbox/a.json".
fn path_tokens<'a>(s: &'a str, p: &str) -> impl Iterator<Item = &'a str> + 'a {
    let core = p.trim_start_matches('/').to_string();
    let hits: Vec<(usize, usize)> = if core.is_empty() { Vec::new() } else { s.match_indices(core.as_str()).map(|(i, m)| (i, i + m.len())).collect() };
    hits.into_iter().filter_map(move |(i, end)| {
        let before_ok = s[..i].chars().last().map_or(true, |c| c == '/' || word_edge(c));
        let after_ok = core.ends_with('/') || s[end..].chars().next().map_or(true, |c| c == '/' || word_edge(c));
        if !(before_ok && after_ok) {
            return None;
        }
        let start = s[..i].rfind(word_edge).map_or(0, |k| k + s[k..].chars().next().map_or(1, char::len_utf8));
        Some(&s[start..end])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md_on() -> TriggerConfig {
        let mut c = TriggerConfig::default();
        c.adapters[0].enabled = true;
        c
    }

    #[test]
    fn the_default_is_munder_difflin_and_off() {
        let c = TriggerConfig::default();
        assert_eq!(c.adapters.len(), 1);
        assert_eq!(c.adapters[0].name, "Munder Difflin");
        assert!(!c.adapters[0].enabled, "off until the user turns it on");
        assert_eq!(c.matcher(None), Matcher::default(), "an adapter that is off matches nothing");
        assert_eq!(c.matcher(None).fingerprint(), "");
    }

    #[test]
    fn an_opening_line_gives_the_label_and_the_ids_it_names() {
        let m = md_on().matcher(None);
        // As Munder Difflin's inboxNudgeText writes it.
        let wake = "You have new hive inbox message(s) \u{2014} at least: 2026-01-02T03-04-05-678Z-abc123, m-2. Read your inbox, act on what is pending there.";
        assert_eq!(m.prompt(wake), Some(("inbox handling".into(), vec!["2026-01-02T03-04-05-678Z-abc123".into(), "m-2".into()])));
        assert_eq!(m.prompt("You have new hive inbox message(s). Read your inbox."), Some(("inbox handling".into(), vec![])));
        assert_eq!(m.prompt("You are compacting one AI agent's long-term memory file. You will receive:").map(|x| x.0), Some("memory upkeep".into()));
        assert_eq!(m.prompt("Please fix the inbox page"), None);
        assert_eq!(m.prompt("I said: You have new hive inbox message"), None, "an opening line, not anywhere");
    }

    #[test]
    fn bookkeeping_is_only_the_harness_files_under_its_root() {
        let home = Path::new("/home/me");
        let m = md_on().matcher(Some(home));
        let hive = "/home/me/HarnessAgents/hive";
        let agent = "/home/me/HarnessAgents/hive/agents/a";
        // Under the root: absolute paths, and relative paths or commands run there.
        for (s, cwd) in [
            ("/home/me/HarnessAgents/hive/agents/a/inbox/m.json", ""), ("/home/me/HarnessAgents/hive/agents/a/memory.md", "/r"),
            ("/home/me/HarnessAgents/hive/board.md", ""), ("/home/me/HarnessAgents/hive/tasks.json", ""),
            ("mv inbox/a.json inbox/.done/", agent), ("cat > outbox/reply.json <<EOF", agent), ("echo x >> memory.md", agent),
            ("tail log.jsonl", hive), ("inbox/m.json", agent),
            ("tail ~/HarnessAgents/hive/log.jsonl", "/r"), ("cat /home/me/HarnessAgents/hive/board.md", "/r"),
        ] {
            assert!(m.is_bookkeeping(s, cwd), "{s} (cwd {cwd})");
        }
        // Dwight's probes: the same names anywhere else are the work.
        for (s, cwd) in [
            ("/Users/me/app/src/inbox/List.tsx", ""), ("/r/.vscode/tasks.json", "/r"), ("/r/docs/memory.md", "/r"),
            ("/r/logs/log.jsonl", "/r"), ("mv inbox/a.json inbox/.done/", "/r"), ("echo x >> memory.md", "/r"),
            ("cargo test && echo done >> board.md", "/r"), ("/home/me/HarnessAgents/hive-old/board.md", ""),
            // End boundary: a backup is not the file.
            ("/home/me/HarnessAgents/hive/agents/a/memory.md.bak", ""), ("cp memory.md.bak x", agent),
            // Start boundary, as before.
            ("src/inbox.rs", agent), ("/home/me/HarnessAgents/hive/agents/a/notmemory.md", ""), ("/home/me/HarnessAgents/hive/dashboard.md", ""),
        ] {
            assert!(!m.is_bookkeeping(s, cwd), "{s} (cwd {cwd})");
        }
        assert!(!TriggerConfig::default().matcher(Some(home)).is_bookkeeping("/home/me/HarnessAgents/hive/memory.md", hive), "off: nothing is bookkeeping");
    }

    #[test]
    fn log_senders_label_messages_and_nothing_else_is_kept() {
        let log = [
            r#"{"kind":"message","id":"s1","from":"scheduler","subject":"Hourly ops STANDUP","body":"secret plan"}"#,
            r#"{"kind":"message","id":"j1","from":"scheduler","subject":"nightly sync"}"#,
            r#"{"kind":"message","id":"w1","from":"webhook","subject":"[webhook] new issue"}"#,
            r#"{"kind":"session","sessionId":"x","agentId":"a"}"#,
            "not json",
        ].join("\n");
        let mut out = HashMap::new();
        read_log(&log, &md_on().adapters[0].log_senders, &mut out);
        assert_eq!(out.get("s1").map(String::as_str), Some("standup"), "subject match ignores case; first rule wins");
        assert_eq!(out.get("j1").map(String::as_str), Some("scheduled job"));
        assert_eq!(out.len(), 2, "senders not listed are not labelled");
        assert!(!format!("{out:?}").contains("secret") && !format!("{out:?}").contains("nightly"), "only labels are kept");
    }

    #[test]
    fn a_missing_file_is_written_and_a_broken_one_falls_back_with_a_reason() {
        let dir = std::env::temp_dir().join(format!("al_triggers_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("triggers.json");
        let (c, err) = TriggerConfig::load_or_default(&path);
        assert_eq!((c, err), (TriggerConfig::default(), None));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT, "first run writes the default for the user to edit");
        md_on().save(&path).unwrap();
        assert!(TriggerConfig::load_or_default(&path).0.adapters[0].enabled, "saved settings win");
        std::fs::write(&path, "{ not json").unwrap();
        let (c, err) = TriggerConfig::load_or_default(&path);
        assert_eq!(c, TriggerConfig::default());
        assert!(err.unwrap().contains("using the built-in default"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json", "a broken file is the user's: not overwritten");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn turning_an_adapter_on_saves_it_but_never_over_a_broken_file() {
        let dir = std::env::temp_dir().join(format!("al_toggle_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("triggers.json");
        let on = TriggerConfig::set_enabled(&path, "Munder Difflin", true).unwrap();
        assert!(on.adapters[0].enabled);
        assert!(TriggerConfig::load_or_default(&path).0.adapters[0].enabled, "saved");
        assert!(TriggerConfig::set_enabled(&path, "Nobody", true).unwrap_err().contains("no adapter named Nobody"));
        std::fs::write(&path, "{ broken").unwrap();
        let err = TriggerConfig::set_enabled(&path, "Munder Difflin", false).unwrap_err();
        assert!(err.contains("Fix or delete the file"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ broken", "the user's file is not replaced");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn found_off_points_at_a_harness_folder_that_exists() {
        let home = std::env::temp_dir().join(format!("al_home_{}", std::process::id()));
        let c = TriggerConfig::default();
        assert!(c.found_off(Some(&home)).is_empty());
        std::fs::create_dir_all(home.join("HarnessAgents/hive")).unwrap();
        std::fs::write(home.join("HarnessAgents/hive/log.jsonl"), "").unwrap();
        assert_eq!(c.found_off(Some(&home)), vec![("Munder Difflin".to_string(), "~/HarnessAgents/hive".to_string())]);
        assert!(md_on().found_off(Some(&home)).is_empty(), "an adapter that is on needs no hint");
        std::fs::remove_dir_all(&home).ok();
    }
}
