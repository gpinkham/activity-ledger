//! The optional labelling pass (0.1.2): a local Ollama model gives each work
//! segment a second, model-made label, shown beside the rule label.
//!
//! Off by default. Only the configured Ollama (loopback unless the user allowed a
//! remote one; see ollama.rs). Each segment's summary (trigger, tool counts, a
//! short excerpt of its opening prompt) is built in memory and sent; only the
//! returned label is stored. Any failure keeps the rule label alone; three
//! failures in a row stop the pass for that run.

use crate::activity::{segment_key, Segment};
use crate::metrics::Filters;
use crate::ollama::{check_url, Endpoint, PROMPT_VERSION, SYSTEM_PROMPT};
use crate::pricing::PriceTable;
use crate::settings::AppSettings;
use crate::store::Store;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

thread_local! {
    /// How many transcript files this thread's excerpt step has read (tests check
    /// it is one per file, not one per segment).
    static TRANSCRIPT_READS: Cell<usize> = const { Cell::new(0) };
}

pub fn transcript_reads() -> usize {
    TRANSCRIPT_READS.with(|c| c.get())
}

/// Longest opening-prompt excerpt sent, in characters (about 300 tokens).
pub const EXCERPT_MAX_CHARS: usize = 1200;
/// The request (system + summary + reply) stays within this many tokens, and
/// within the configured context limit, at 4 characters a token.
pub const REQUEST_MAX_TOKENS: i64 = 2000;
/// The context the model is loaded with for labelling: room for one request and
/// its answer. The configured limit is the model's ceiling, not what a label
/// needs; asking for all of it makes Ollama reserve that much and load slowly.
pub const LABEL_NUM_CTX: i64 = 4096;
/// Failures in a row that stop the pass.
pub const MAX_FAILURES_IN_A_ROW: u32 = 3;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct LabelProgress {
    pub running: bool,
    pub total: usize,
    pub done: usize,
    pub labelled: usize,
    pub failed: usize,
    /// Waiting for Ollama to load the model, before the first label.
    pub loading: bool,
    /// Why the pass stopped early, for the settings card.
    pub stopped: Option<String>,
}

/// A segment to label, as collected under the store lock: no file is read yet.
pub struct RawJob {
    pub key: String,
    pub counts: String,
    /// The opening prompt's (timestamp, transcript file).
    pub start: Option<(String, String)>,
}

/// One segment to label: its key and the summary to send (built in memory).
pub struct LabelJob {
    pub key: String,
    pub summary: String,
}

/// Is the pass allowed to run with these settings? The endpoint, or why not.
pub fn prepare(settings: &AppSettings) -> Result<Endpoint, String> {
    if !settings.model_labels_enabled {
        return Err("labelling with the local model is turned off".into());
    }
    if settings.ollama_model.trim().is_empty() {
        return Err("no Ollama model is set".into());
    }
    check_url(&settings.ollama_url, settings.allow_remote_ollama)
}

/// The num_ctx sent with every label request (and the load): LABEL_NUM_CTX, or
/// the configured limit when that is smaller.
pub fn request_num_ctx(limit: i64) -> i64 {
    limit.min(LABEL_NUM_CTX)
}

/// How many characters of excerpt fit: none below a 1,000-token context, else up
/// to EXCERPT_MAX_CHARS within the request budget.
pub fn excerpt_budget(num_ctx: i64, counts_chars: usize) -> usize {
    if num_ctx < 1000 {
        return 0;
    }
    let tokens = num_ctx.min(REQUEST_MAX_TOKENS) - 64 - (SYSTEM_PROMPT.len() / 4) as i64 - (counts_chars / 4) as i64;
    if tokens <= 0 { 0 } else { ((tokens * 4) as usize).min(EXCERPT_MAX_CHARS) }
}

/// The prompt texts at the wanted timestamps of each transcript file, reading
/// every file once (big transcripts hold many segments).
fn prompt_texts(wanted: &HashMap<String, HashSet<String>>) -> HashMap<(String, String), String> {
    let mut out = HashMap::new();
    for (file, stamps) in wanted {
        TRANSCRIPT_READS.with(|c| c.set(c.get() + 1));
        let Ok(content) = std::fs::read_to_string(file) else { continue };
        for line in content.lines() {
            // Cheap filter before parsing: only user records at a wanted time.
            if !line.contains("\"type\":\"user\"") || !stamps.iter().any(|ts| line.contains(ts.as_str())) {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) else { continue };
            if stamps.contains(ts) && !out.contains_key(&(file.clone(), ts.to_string())) {
                if let Some(text) = crate::parser::prompt_text(&v) {
                    out.insert((file.clone(), ts.to_string()), text);
                }
            }
        }
    }
    out
}

/// Cut `text` to `max` characters at a line boundary (or a char boundary if the
/// first line is longer).
fn cut(text: &str, max: usize) -> Option<String> {
    if max == 0 {
        return None;
    }
    if text.chars().count() <= max {
        return Some(text.to_string());
    }
    let cut: String = text.chars().take(max).collect();
    Some(match cut.rfind('\n') { Some(i) if i > 0 => cut[..i].to_string(), _ => cut })
}

/// The opening prompt's text, read from the transcript in memory and cut to `max`.
pub fn excerpt(file: &str, ts: &str, max: usize) -> Option<String> {
    let wanted = HashMap::from([(file.to_string(), HashSet::from([ts.to_string()]))]);
    prompt_texts(&wanted).remove(&(file.to_string(), ts.to_string())).and_then(|t| cut(&t, max))
}

/// Build the summaries for collected jobs, reading each transcript file once.
pub fn build_jobs(raw: Vec<RawJob>, num_ctx: i64) -> Vec<LabelJob> {
    let mut wanted: HashMap<String, HashSet<String>> = HashMap::new();
    for j in &raw {
        if let Some((ts, file)) = &j.start {
            if excerpt_budget(num_ctx, j.counts.len()) > 0 {
                wanted.entry(file.clone()).or_default().insert(ts.clone());
            }
        }
    }
    let texts = prompt_texts(&wanted);
    raw.into_iter()
        .map(|j| {
            let budget = excerpt_budget(num_ctx, j.counts.len());
            let ex = j.start.as_ref().and_then(|(ts, file)| texts.get(&(file.clone(), ts.clone()))).and_then(|t| cut(t, budget));
            let summary = match ex {
                Some(e) => format!("{}\nOpening prompt (excerpt):\n{e}", j.counts),
                None => j.counts,
            };
            LabelJob { key: j.key, summary }
        })
        .collect()
}

fn trigger_words(seg: &Segment) -> &'static str {
    match seg.kind.as_str() {
        "human" => "human",
        "compact" | "resume" => "compaction",
        _ => "other",
    }
}

fn counts_line(seg: &Segment) -> String {
    let t = &seg.tools;
    format!(
        "Trigger: {}\nTools: edits {}, notes {}, test runs {} (longest failing streak {}), reads {}, web {}, shell {}, PR or git-range {}",
        trigger_words(seg), t.edits, t.notes, t.tests, t.max_fail_streak, t.reads, t.web, t.shell, t.pr
    )
}

impl Store {
    /// The segments to label (those with API calls), unlabelled by this model and
    /// prompt version. Reads no transcript: run it under the store lock, release
    /// the lock, then build_jobs.
    pub fn raw_label_jobs(&self, price: &PriceTable, f: &Filters, model: &str) -> rusqlite::Result<Vec<RawJob>> {
        let (segments, _) = self.build_segments(price, f)?;
        let done = self.model_labels(model)?;
        let mut jobs = Vec::new();
        for seg in segments.values() {
            if seg.calls.is_empty() {
                continue;
            }
            let key = segment_key(&seg.session, seg.start.as_ref().map(|(ts, _)| ts.as_str()));
            if !done.contains_key(&key) {
                jobs.push(RawJob { key, counts: counts_line(seg), start: seg.start.clone() });
            }
        }
        Ok(jobs)
    }

    /// Stored model labels for `model` at the current prompt version: key -> label.
    pub fn model_labels(&self, model: &str) -> rusqlite::Result<HashMap<String, String>> {
        let mut st = self.conn.prepare("SELECT segment_key, label FROM segment_model_labels WHERE model = ?1 AND prompt_version = ?2")?;
        let rows = st.query_map(rusqlite::params![model, PROMPT_VERSION], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect()
    }

    pub fn save_model_label(&self, key: &str, model: &str, label: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO segment_model_labels(segment_key, model, prompt_version, label) VALUES (?1,?2,?3,?4)
             ON CONFLICT(segment_key, model, prompt_version) DO UPDATE SET label = excluded.label",
            rusqlite::params![key, model, PROMPT_VERSION, label],
        )?;
        Ok(())
    }
}

/// Label every unlabelled segment, one request at a time. The store is locked
/// only to read the jobs and to save each label, never during a request. `load`
/// asks Ollama to load the model first (ollama::load_model), with a longer wait
/// than one label gets; it is skipped when there is nothing to label. `send` is
/// the model call (ollama::chat_label in the app). `stop` (the Stop button) is
/// checked before the load and before each label; a request it ends is not
/// counted as a failure. Returns the final progress.
pub fn run(
    store: &Mutex<Store>,
    price: &PriceTable,
    f: &Filters,
    settings: &AppSettings,
    progress: &Mutex<LabelProgress>,
    stop: &AtomicBool,
    load: impl FnOnce() -> Result<(), String>,
    mut send: impl FnMut(&str) -> Result<String, String>,
) -> Result<LabelProgress, String> {
    let model = settings.ollama_model.clone();
    // Under the lock: only what the cache knows. The transcripts are read after
    // it is released, once per file, so the app stays usable.
    let raw = {
        let s = store.lock().map_err(|e| e.to_string())?;
        s.raw_label_jobs(price, f, &model).map_err(|e| e.to_string())?
    };
    let jobs = build_jobs(raw, settings.local_context_limit);
    let set = |p: LabelProgress| { if let Ok(mut g) = progress.lock() { *g = p.clone(); } p };
    let mut p = set(LabelProgress { running: true, total: jobs.len(), loading: !jobs.is_empty(), ..Default::default() });
    let stopped = |p: &mut LabelProgress| {
        p.loading = false;
        p.stopped = Some(format!("Stopped. {} labelled; the rest keep their rule labels.", p.labelled));
    };
    if p.loading && stop.load(Ordering::SeqCst) {
        stopped(&mut p);
    } else if p.loading {
        let loaded = load();
        p.loading = false;
        if stop.load(Ordering::SeqCst) {
            stopped(&mut p);
        } else if let Err(e) = loaded {
            p.running = false;
            p.stopped = Some(format!("Could not load {model} in Ollama at {}: {e}. Rule labels unchanged.", settings.ollama_url));
            return Ok(set(p));
        }
        p = set(p);
    }
    let mut in_a_row = 0;
    for job in jobs {
        if p.stopped.is_some() || stop.load(Ordering::SeqCst) {
            if p.stopped.is_none() { stopped(&mut p); }
            break;
        }
        let answer = send(&job.summary);
        if stop.load(Ordering::SeqCst) && answer.is_err() {
            stopped(&mut p);
            break;
        }
        match answer {
            Ok(label) => {
                in_a_row = 0;
                let s = store.lock().map_err(|e| e.to_string())?;
                s.save_model_label(&job.key, &model, &label).map_err(|e| e.to_string())?;
                p.labelled += 1;
            }
            Err(e) => {
                in_a_row += 1;
                p.failed += 1;
                if in_a_row >= MAX_FAILURES_IN_A_ROW {
                    p.stopped = Some(format!("Stopped after {MAX_FAILURES_IN_A_ROW} failures in a row. Last: {e}. Rule labels unchanged."));
                    p.done += 1;
                    break;
                }
            }
        }
        p.done += 1;
        p = set(p);
    }
    p.running = false;
    Ok(set(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use crate::parser::parse_str;
    use crate::scanner::ScannedFile;
    use serde_json::json;

    const SECRET: &str = "please review PR 12 for the customer billing fix";

    fn fixture() -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("ci_label_{}_{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s1.jsonl");
        let mut lines = Vec::new();
        for i in 0..5 {
            let ts = format!("2026-10-01T1{i}:00:00.000Z");
            let call_ts = format!("2026-10-01T1{i}:00:01.000Z");
            let text = if i == 0 { format!("{SECRET}\nline two\nline three") } else { format!("task {i}") };
            lines.push(json!({"type":"user","sessionId":"s1","timestamp":ts,"message":{"role":"user","content":text}}).to_string());
            lines.push(json!({"type":"assistant","sessionId":"s1","timestamp":call_ts,"message":{"id":format!("m{i}"),"role":"assistant","model":"claude-haiku-4-5","usage":{"input_tokens":1000,"output_tokens":10},"content":[{"type":"tool_use","id":format!("t{i}"),"name":"Read","input":{"file_path":"/r/a.rs"}}]}}).to_string());
        }
        let content = lines.join("\n");
        std::fs::write(&path, &content).unwrap();
        let mut store = Store::open_in_memory();
        let sf = ScannedFile { path: path.clone(), project: "p".into(), size: 1, mtime: 1, subagent: None };
        store.replace_file(&sf, &parse_str("p", &content)).unwrap();
        store.rebuild_sessions().unwrap();
        (store, dir)
    }
    fn rand_suffix() -> u128 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() }
    fn on() -> AppSettings {
        AppSettings { model_labels_enabled: true, ollama_model: "qwen3:8b".into(), ..Default::default() }
    }

    #[test]
    fn the_pass_is_off_by_default_and_refuses_a_remote_url_without_the_box() {
        assert!(prepare(&AppSettings::default()).unwrap_err().contains("turned off"));
        assert!(prepare(&AppSettings { ollama_model: String::new(), ..on() }).is_err());
        let remote = AppSettings { ollama_url: "http://10.1.2.3:11434".into(), ..on() };
        assert!(prepare(&remote).unwrap_err().contains("Allow a remote Ollama"));
        assert!(prepare(&AppSettings { allow_remote_ollama: true, ..remote }).is_ok());
        assert!(prepare(&on()).is_ok(), "the default URL is loopback");
    }

    #[test]
    fn labels_are_stored_and_shown_beside_the_rule_label() {
        let (store, dir) = fixture();
        let store = Mutex::new(store);
        let progress = Mutex::new(LabelProgress::default());
        let mut sent = Vec::new();
        let p = run(&store, &PriceTable::seed(), &Filters::default(), &on(), &progress, &AtomicBool::new(false), || Ok(()), |s| {
            sent.push(s.to_string());
            Ok("code review".into())
        }).unwrap();
        assert_eq!((p.total, p.labelled, p.failed, p.running), (5, 5, 0, false));
        assert!(sent[0].contains("Trigger: human") && sent[0].contains("reads 1"), "{}", sent[0]);
        assert!(sent[0].contains(SECRET), "the opening prompt excerpt is sent to the local model");
        let s = store.lock().unwrap();
        let r = s.activity(&PriceTable::seed(), &Filters::default(), &on()).unwrap();
        let row = r.rows.iter().find(|x| x.activity == "review / explore").unwrap();
        assert_eq!(row.model_labels, vec![crate::activity::LabelCount { label: "code review".into(), segments: 5 }]);
        // Only labels are stored: the excerpt is nowhere in the cache.
        let n: i64 = s.conn.query_row("SELECT COUNT(*) FROM segment_model_labels WHERE label LIKE '%billing%'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        let dump: String = s.conn.query_row("SELECT group_concat(segment_key || label, ',') FROM segment_model_labels", [], |r| r.get(0)).unwrap();
        assert!(!dump.contains("billing"));
        drop(s);
        // A second run sends nothing: every segment already has this model's label.
        let p = run(&store, &PriceTable::seed(), &Filters::default(), &on(), &progress, &AtomicBool::new(false), || Ok(()), |_| panic!("sent again")).unwrap();
        assert_eq!(p.total, 0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn ollama_down_stops_after_three_failures_and_keeps_the_rule_labels() {
        let (store, dir) = fixture();
        let store = Mutex::new(store);
        let progress = Mutex::new(LabelProgress::default());
        let mut calls = 0;
        let p = run(&store, &PriceTable::seed(), &Filters::default(), &on(), &progress, &AtomicBool::new(false), || Ok(()), |_| {
            calls += 1;
            Err("connect: refused".into())
        }).unwrap();
        assert_eq!(calls, 3, "three failures in a row stop the pass");
        let why = p.stopped.as_deref().unwrap();
        assert!(why.contains("3 failures in a row") && why.contains("connect: refused"), "the last error is shown as it is: {why}");
        assert!(!why.contains("not reachable"), "not every failure is an unreachable Ollama: {why}");
        assert_eq!(progress.lock().unwrap().clone(), p);
        let s = store.lock().unwrap();
        let r = s.activity(&PriceTable::seed(), &Filters::default(), &on()).unwrap();
        let row = r.rows.iter().find(|x| x.activity == "review / explore").unwrap();
        assert_eq!(row.segments, 5, "the rule labels are untouched");
        assert!(row.model_labels.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_model_is_loaded_once_before_the_labels_and_a_failed_load_stops_the_pass() {
        let (store, dir) = fixture();
        let store = Mutex::new(store);
        let progress = Mutex::new(LabelProgress::default());
        let mut loads = 0;
        let p = run(&store, &PriceTable::seed(), &Filters::default(), &on(), &progress, &AtomicBool::new(false), || {
            loads += 1;
            let now = progress.lock().unwrap().clone();
            assert!(now.running && now.loading, "the card can say the model is loading: {now:?}");
            Err("Ollama answered 404: model \"qwen3:8b\" not found".into())
        }, |_| panic!("no label request after a failed load")).unwrap();
        assert_eq!(loads, 1);
        let why = p.stopped.as_deref().unwrap();
        assert!(why.contains("Could not load qwen3:8b") && why.contains("not found"), "{why}");
        assert!(!p.running && !p.loading);
        // Nothing to label: no load either.
        let mut calls = 0;
        run(&store, &PriceTable::seed(), &Filters::default(), &on(), &progress, &AtomicBool::new(false), || Ok(()), |_| Ok("coding".into())).unwrap();
        run(&store, &PriceTable::seed(), &Filters::default(), &on(), &progress, &AtomicBool::new(false), || { calls += 1; Ok(()) }, |_| panic!("sent again")).unwrap();
        assert_eq!(calls, 0, "every segment is labelled: Ollama is not asked to load anything");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn label_requests_ask_for_a_small_context_not_the_whole_limit() {
        // A label request is at most REQUEST_MAX_TOKENS in and 64 out. Asking
        // Ollama for the full 32k makes it reserve memory for 32k and load slowly.
        assert_eq!(request_num_ctx(32_000), LABEL_NUM_CTX);
        assert!(LABEL_NUM_CTX >= REQUEST_MAX_TOKENS + 64);
        assert_eq!(request_num_ctx(1_500), 1_500, "a smaller limit is kept");
    }

    #[test]
    fn stop_ends_the_pass_before_the_load_or_between_labels() {
        let (store, dir) = fixture();
        let store = Mutex::new(store);
        let progress = Mutex::new(LabelProgress::default());
        // Stopped before it starts: no load, no label.
        let stop = AtomicBool::new(true);
        let p = run(&store, &PriceTable::seed(), &Filters::default(), &on(), &progress, &stop,
            || panic!("no load after Stop"), |_| panic!("no label after Stop")).unwrap();
        assert!(p.stopped.as_deref().unwrap().starts_with("Stopped"), "{:?}", p.stopped);
        assert!(!p.running && !p.loading);
        // Stopped during the second label: that request ends with an error, which
        // is the Stop and not a failure, and nothing more is sent.
        let stop = AtomicBool::new(false);
        let mut sent = 0;
        let p = run(&store, &PriceTable::seed(), &Filters::default(), &on(), &progress, &stop, || Ok(()), |_| {
            sent += 1;
            if sent == 2 { stop.store(true, Ordering::SeqCst); return Err(crate::ollama::STOPPED.into()); }
            Ok("coding".into())
        }).unwrap();
        assert_eq!((sent, p.labelled, p.failed), (2, 1, 0));
        assert!(p.stopped.as_deref().unwrap().contains("1 labelled"), "{:?}", p.stopped);
        assert!(!p.running);
        // Stopped while a label succeeds: that label is kept, no next request.
        let stop = AtomicBool::new(false);
        let mut sent = 0;
        let p = run(&store, &PriceTable::seed(), &Filters::default(), &on(), &progress, &stop, || Ok(()), |_| {
            sent += 1;
            if sent == 2 { stop.store(true, Ordering::SeqCst); }
            Ok("coding".into())
        }).unwrap();
        assert_eq!((sent, p.labelled, p.failed), (2, 2, 0), "{p:?}");
        assert!(p.stopped.as_deref().unwrap().starts_with("Stopped"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn each_transcript_is_read_once_and_the_store_is_free_during_requests() {
        let (store, dir) = fixture();
        let store = Mutex::new(store);
        let progress = Mutex::new(LabelProgress::default());
        let before = transcript_reads();
        let mut free = 0;
        run(&store, &PriceTable::seed(), &Filters::default(), &on(), &progress, &AtomicBool::new(false), || Ok(()), |_| {
            // The UI's commands need the store while the pass runs.
            if store.try_lock().is_ok() { free += 1; }
            Ok("coding".into())
        }).unwrap();
        let reads = transcript_reads() - before;
        assert_eq!(reads, 1, "five segments in one file: one read");
        assert_eq!(free, 5, "the store is not locked during a request");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_failure_between_successes_does_not_stop_the_pass() {
        let (store, dir) = fixture();
        let store = Mutex::new(store);
        let progress = Mutex::new(LabelProgress::default());
        let mut i = 0;
        let p = run(&store, &PriceTable::seed(), &Filters::default(), &on(), &progress, &AtomicBool::new(false), || Ok(()), |_| {
            i += 1;
            if i % 2 == 0 { Err("timeout".into()) } else { Ok("coding".into()) }
        }).unwrap();
        assert_eq!((p.labelled, p.failed, p.stopped.is_none()), (3, 2, true));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_excerpt_fits_the_budget_and_small_contexts_get_none() {
        let (_, dir) = fixture();
        let file = dir.join("s1.jsonl").to_string_lossy().to_string();
        let full = excerpt(&file, "2026-10-01T10:00:00.000Z", 10_000).unwrap();
        assert!(full.starts_with(SECRET));
        let cut = excerpt(&file, "2026-10-01T10:00:00.000Z", SECRET.len() + 5).unwrap();
        assert_eq!(cut, SECRET, "cut at a line boundary");
        assert_eq!(excerpt_budget(800, 100), 0, "below a 1,000-token context: counts only");
        assert_eq!(excerpt_budget(32_000, 200), EXCERPT_MAX_CHARS);
        assert!(excerpt_budget(1_050, 3_000) < EXCERPT_MAX_CHARS, "a tight context leaves less room");
        std::fs::remove_dir_all(dir).ok();
    }
}
