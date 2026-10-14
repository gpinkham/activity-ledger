//! Cost by type of work.
//!
//! A session is cut into SEGMENTS at each inbound prompt (a person typing, a task
//! notification, a compaction). Every API call belongs to the segment it answered;
//! a sub-agent's calls belong to the segment of the call that started it. Each
//! segment gets one label from fixed rules over its prompt kind and its tool mix.
//! Costs are summed per label. Only labels, counts and costs leave this module.

use crate::metrics::{bind, where_clause, Filters};
use crate::pricing::{PriceTable, TokenBreakdown};
use crate::settings::AppSettings;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub const OVERHEAD: [&str; 1] = ["compaction"];
pub const WORK: [&str; 8] =
    ["coding", "debugging", "code review", "research", "review / explore", "shell / ops", "chat / planning", "other work"];
/// Labels whose work a smaller local model might do, subject to the size limits.
/// Coding and debugging are not.
const LOCAL_TYPES: [&str; 3] = ["research", "review / explore", "chat / planning"];
/// Consecutive failing test runs that make a segment debugging rather than coding.
/// Test-driven work goes red, then green: it rarely fails three times running.
pub const DEBUG_FAIL_STREAK: i64 = 3;

/// Which labels the report groups by: the fixed rules (default), or the labels the
/// optional local-model pass stored (0.1.2). Only the grouping changes; every call
/// and its cost are counted the same way in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LabelSource {
    #[default]
    Rule,
    Model,
}

/// In the model view, the row for segments the local model has not labelled.
pub const UNLABELLED: &str = "not labelled by the model";

/// "overhead", "work" or "unlabelled" for a label in the model view. The model's
/// one overhead label is "overhead".
fn model_group(label: &str) -> &'static str {
    match label {
        UNLABELLED => "unlabelled",
        "overhead" => "overhead",
        _ => "work",
    }
}

/// "overhead" or "work" for a label.
pub fn group_of(label: &str) -> &'static str {
    if OVERHEAD.contains(&label) { "overhead" } else { "work" }
}
/// Tools that organise the turn rather than do the work.
const META_TOOLS: [&str; 6] = ["ToolSearch", "TodoWrite", "Skill", "Agent", "Task", "ExitPlanMode"];
const EDIT_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];
const READ_TOOLS: [&str; 4] = ["Read", "Grep", "Glob", "LS"];
const WEB_TOOLS: [&str; 2] = ["WebSearch", "WebFetch"];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ActivityRow {
    pub activity: String,
    /// "overhead" or "work"
    pub group: String,
    pub segments: i64,
    pub calls: i64,
    pub input: i64,
    pub output: i64,
    pub cache_write: i64,
    pub cache_read: i64,
    pub cost_usd: f64,
    /// Share of the total cost, 0..1.
    pub share: f64,
    /// Context per call (input + cache write + cache read), nearest-rank percentiles.
    pub ctx_p50: i64,
    pub ctx_p95: i64,
    pub out_p95: i64,
    pub local_candidate: bool,
    pub local_reasons: Vec<String>,
    /// The local model's labels for this row's segments (0.1.2, optional), with
    /// how many segments got each, largest first. Empty when the pass did not run,
    /// and in the model view.
    pub model_labels: Vec<LabelCount>,
    /// In the model view, the rule labels of this row's segments, largest first.
    /// Empty in the rule view.
    pub rule_labels: Vec<LabelCount>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LabelCount {
    pub label: String,
    pub segments: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DayActivity {
    pub day: String,
    pub activity: String,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct ActivityTotals {
    pub cost_usd: f64,
    pub overhead_usd: f64,
    pub local_candidate_usd: f64,
    pub calls: i64,
    pub segments: i64,
}

/// How much of the report the local model has labelled (with the configured model
/// and the current prompt version). The same in both views.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct LabelCoverage {
    /// The configured Ollama model; empty when none is set.
    pub model: String,
    pub segments: i64,
    pub labelled_segments: i64,
    pub calls: i64,
    pub labelled_calls: i64,
    pub cost_usd: f64,
    pub labelled_usd: f64,
}

/// Our cost for the sessions Claude Code also priced (its `cost-state` records).
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Reconcile {
    pub sessions: i64,
    pub ours_usd: f64,
    pub claude_code_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityReport {
    pub rows: Vec<ActivityRow>,
    pub by_day: Vec<DayActivity>,
    pub totals: ActivityTotals,
    pub reconcile: Reconcile,
    /// Model ids priced at the default row because the table does not know them.
    pub unknown_models: Vec<String>,
    /// "rule" or "model": which labels the rows and days are grouped by.
    pub label_source: String,
    pub coverage: LabelCoverage,
}

#[derive(Default)]
pub(crate) struct ToolMix {
    pub(crate) total: i64,
    pub(crate) meta: i64,
    pub(crate) edits: i64,
    pub(crate) tests: i64,
    pub(crate) web: i64,
    pub(crate) reads: i64,
    pub(crate) shell: i64,
    /// Notes or docs files written (not source edits), and pull-request commands.
    pub(crate) notes: i64,
    pub(crate) pr: i64,
    /// Consecutive failing test runs so far, and the longest such run.
    pub(crate) fail_streak: i64,
    pub(crate) max_fail_streak: i64,
}

impl ToolMix {
    /// Add one tool call, in time order. `failed` is a test run's outcome, when known.
    fn add(&mut self, name: &str, tag: Option<&str>, failed: Option<bool>) {
        self.total += 1;
        if tag == Some("test") {
            match failed {
                Some(true) => {
                    self.fail_streak += 1;
                    self.max_fail_streak = self.max_fail_streak.max(self.fail_streak);
                }
                // A passing run ends the streak; an unknown outcome is not a failure.
                _ => self.fail_streak = 0,
            }
        }
        match tag {
            Some("test") => self.tests += 1,
            Some("write") => self.edits += 1,
            Some("notes") => self.notes += 1,
            Some("review") => self.pr += 1,
            Some("read") => self.reads += 1,
            _ => {}
        }
        if META_TOOLS.contains(&name) {
            self.meta += 1;
        } else if EDIT_TOOLS.contains(&name) && !matches!(tag, Some("notes") | Some("scratch")) {
            self.edits += 1;
        } else if READ_TOOLS.contains(&name) {
            self.reads += 1;
        } else if WEB_TOOLS.contains(&name) {
            self.web += 1;
        }
        if name == "Bash" && tag != Some("read") {
            self.shell += 1;
        }
    }

    /// Tool calls that do the task itself, not turn plumbing.
    fn work(&self) -> i64 {
        self.total - self.meta
    }
}

pub(crate) struct Call {
    model: String,
    tb: TokenBreakdown,
    cache_write: i64,
}

pub(crate) struct Segment {
    pub(crate) session: String,
    pub(crate) kind: String,
    pub(crate) calls: Vec<Call>,
    pub(crate) tools: ToolMix,
    day: String,
    /// The opening prompt's timestamp and transcript file (None before any prompt).
    pub(crate) start: Option<(String, String)>,
}

/// A segment's stable key for stored model labels: session + opening prompt time.
pub fn segment_key(session: &str, start_ts: Option<&str>) -> String {
    format!("{session}|{}", start_ts.unwrap_or("none"))
}

/// The label for one segment. Pure, so each rule is testable on its own.
fn classify(kind: &str, t: &ToolMix) -> String {
    if kind == "compact" {
        return "compaction".into();
    }
    classify_by_tools(t).into()
}

fn classify_by_tools(t: &ToolMix) -> &'static str {
    let work = t.work();
    if work == 0 {
        return "chat / planning";
    }
    // A review reads and checks without editing source: a pull-request command or a
    // git range (then its notes do not make it coding), or reads plus test runs
    // with no notes (reads, notes and a test run is a docs change).
    if t.edits == 0 && (t.pr > 0 || (t.reads > 0 && t.tests > 0 && t.notes == 0)) {
        return "code review";
    }
    // Notes and docs count as writing when nothing marks the segment a review.
    let writes = t.edits + t.notes;
    if t.max_fail_streak >= DEBUG_FAIL_STREAK && writes >= 1 {
        "debugging"
    } else if writes >= 1 {
        "coding"
    } else if t.web * 10 >= work * 3 {
        "research"
    } else if t.reads * 10 >= work * 7 {
        "review / explore"
    } else if t.shell * 2 >= work {
        "shell / ops"
    } else {
        "other work"
    }
}

/// Nearest-rank percentile of an unsorted list (0 for an empty one).
fn percentile(values: &mut [i64], p: f64) -> i64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let rank = (p * values.len() as f64).ceil() as usize;
    values[rank.clamp(1, values.len()) - 1]
}

fn kilo(n: i64) -> String {
    if n >= 1000 { format!("{}k", (n as f64 / 1000.0).round() as i64) } else { n.to_string() }
}

fn with_clause(w: &str, extra: &str) -> String {
    if w.is_empty() { format!("WHERE {extra}") } else { format!("{w} AND {extra}") }
}

impl Store {
    /// Cut the filtered calls into segments, with each segment's calls and tool mix.
    /// Also returns the model ids the price table does not know.
    pub(crate) fn build_segments(&self, price: &PriceTable, f: &Filters) -> rusqlite::Result<(BTreeMap<(String, i64), Segment>, Vec<String>)> {
        let (w, binds) = where_clause(f);
        // Prompts are read without the date filter so a call just inside the range
        // still finds the prompt that started it.
        let undated = Filters { from: None, to: None, project: f.project.clone() };
        let (pw, pbinds) = where_clause(&undated);

        // Sub-agent transcripts: file -> (agent, the call that started it).
        let mut sub_by_file: HashMap<String, Option<String>> = HashMap::new();
        {
            let mut st = self.conn.prepare("SELECT file, parent_tool_use_id FROM subagents")?;
            for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))? {
                let (file, parent) = r?;
                sub_by_file.insert(file, parent);
            }
        }

        // Segment boundaries per session (main thread only).
        let mut bounds: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
        {
            let sql = format!("SELECT session_id, ts, kind, file FROM prompts {} ORDER BY ts", with_clause(&pw, "agent_id IS NULL"));
            let mut st = self.conn.prepare(&sql)?;
            for r in st.query_map(bind(&pbinds).as_slice(), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
            })? {
                let (sid, ts, kind, file) = r?;
                bounds.entry(sid).or_default().push((ts, kind, file));
            }
        }

        let mut segments: BTreeMap<(String, i64), Segment> = BTreeMap::new();
        let segment_for = |session: &str, ts: &str, segments: &mut BTreeMap<(String, i64), Segment>| -> (String, i64) {
            let list = bounds.get(session).map(|v| v.as_slice()).unwrap_or(&[]);
            let idx = list.partition_point(|(pts, _, _)| pts.as_str() <= ts) as i64 - 1;
            let key = (session.to_string(), idx);
            segments.entry(key.clone()).or_insert_with(|| {
                let (kind, start) = if idx >= 0 {
                    let (pts, k, file) = &list[idx as usize];
                    (k.clone(), Some((pts.clone(), file.clone())))
                } else {
                    ("none".to_string(), None)
                };
                Segment {
                    session: session.to_string(),
                    kind,
                    calls: Vec::new(),
                    tools: ToolMix::default(),
                    day: ts.get(..10).unwrap_or("").to_string(),
                    start,
                }
            });
            key
        };

        // Tool calls, parent and sub-agent together in time order, so a failing
        // streak follows what actually ran when. A sub-agent's call goes to the
        // segment of the Agent call that started it, which ran before it.
        type ToolRow = (String, String, String, Option<String>, Option<String>, String, Option<i64>, Option<String>);
        let mut tool_rows: Vec<ToolRow> = Vec::new();
        {
            let sql = format!("SELECT session_id, ts, name, tag, tool_use_id, file, fail_count, test_key FROM tool_calls {w} ORDER BY ts, id");
            let mut st = self.conn.prepare(&sql)?;
            for r in st.query_map(bind(&binds).as_slice(), |r| {
                Ok((
                    r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?,
                    r.get::<_, Option<i64>>(6)?, r.get::<_, Option<String>>(7)?,
                ))
            })? {
                tool_rows.push(r?);
            }
        }
        // A test command's baseline in a session: the fewest failures that command
        // reported there. A suite with known failures reads failing on every run, and
        // sessions mix full suites with clean focused runs, so the baseline is per
        // (session, command). Only a run ABOVE it counts as failing (-1: failed,
        // count unknown, always counts).
        let mut baseline: HashMap<(String, String), i64> = HashMap::new();
        for (sid, _, _, _, _, _, count, key) in &tool_rows {
            if let Some(n) = count.filter(|n| *n >= 0) {
                let b = baseline.entry((sid.clone(), key.clone().unwrap_or_default())).or_insert(n);
                *b = (*b).min(n);
            }
        }
        let mut tool_segment: HashMap<String, (String, i64)> = HashMap::new();
        for (sid, ts, name, tag, tuid, file, count, key) in tool_rows {
            let base = baseline.get(&(sid.clone(), key.unwrap_or_default())).copied().unwrap_or(0);
            // A suite with many known failures wobbles (19 vs 22): within 20% above a
            // baseline of 10 or more is the baseline. Clean suites stay exact.
            let tolerance = if base >= 10 { base / 5 } else { 0 };
            let failing = count.map(|n| n == -1 || n > base + tolerance);
            let key = match sub_by_file.get(&file) {
                Some(parent) => parent
                    .as_ref()
                    .and_then(|p| tool_segment.get(p).cloned())
                    .unwrap_or_else(|| segment_for(&sid, &ts, &mut segments)),
                None => segment_for(&sid, &ts, &mut segments),
            };
            segments.get_mut(&key).unwrap().tools.add(&name, tag.as_deref(), failing);
            if !sub_by_file.contains_key(&file) {
                if let Some(id) = tuid {
                    tool_segment.insert(id, key);
                }
            }
        }
        // A sub-agent file's API calls: its parent call's segment, else its own.
        let sub_segment = |file: &str, sid: &str, ts: &str, segments: &mut BTreeMap<(String, i64), Segment>| {
            sub_by_file
                .get(file)
                .and_then(|p| p.as_ref())
                .and_then(|p| tool_segment.get(p).cloned())
                .unwrap_or_else(|| segment_for(sid, ts, segments))
        };

        let mut unknown: Vec<String> = Vec::new();
        {
            let sql = format!(
                "SELECT session_id, ts, COALESCE(model,''), input_tokens, output_tokens, cache_creation, cache_read,
                        ephemeral_1h, ephemeral_5m, web_searches, file
                 FROM messages {}",
                with_clause(&w, "role = 'assistant'")
            );
            let mut st = self.conn.prepare(&sql)?;
            for r in st.query_map(bind(&binds).as_slice(), |r| {
                Ok((
                    r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?, r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?, r.get::<_, i64>(8)?, r.get::<_, i64>(9)?, r.get::<_, String>(10)?,
                ))
            })? {
                let (sid, ts, model, input, output, cc, cr, e1h, e5m, web, file) = r?;
                if !model.is_empty() && !model.starts_with('<') && !price.is_known(&model) && !unknown.contains(&model) {
                    unknown.push(model.clone());
                }
                let key = if sub_by_file.contains_key(&file) {
                    sub_segment(&file, &sid, &ts, &mut segments)
                } else {
                    segment_for(&sid, &ts, &mut segments)
                };
                let tb = TokenBreakdown { input, output, cache_creation_5m: e5m, cache_creation_1h: e1h, cache_read: cr, web_searches: web };
                segments.get_mut(&key).unwrap().calls.push(Call { model, tb, cache_write: cc });
            }
        }
        Ok((segments, unknown))
    }

    /// The report grouped by the rule labels.
    pub fn activity(&self, price: &PriceTable, f: &Filters, settings: &AppSettings) -> rusqlite::Result<ActivityReport> {
        self.activity_by(price, f, settings, LabelSource::Rule)
    }

    /// The report grouped by the rule labels or by the local model's labels.
    pub fn activity_by(&self, price: &PriceTable, f: &Filters, settings: &AppSettings, source: LabelSource) -> rusqlite::Result<ActivityReport> {
        let (segments, mut unknown) = self.build_segments(price, f)?;
        // The local model's labels for these segments, when the pass has run.
        let model = settings.ollama_model.trim().to_string();
        let model_labels = if model.is_empty() { HashMap::new() } else { self.model_labels(&settings.ollama_model)? };
        // Per row: the other view's labels of its segments, with counts.
        let mut other_counts: HashMap<String, HashMap<String, i64>> = HashMap::new();
        let mut coverage = LabelCoverage { model, ..Default::default() };

        // Roll segments up by label and day.
        struct Acc { row: ActivityRow, ctx: Vec<i64>, out: Vec<i64> }
        let mut by_label: HashMap<String, Acc> = HashMap::new();
        let mut by_day: BTreeMap<(String, String), f64> = BTreeMap::new();
        let mut totals = ActivityTotals::default();

        for seg in segments.values() {
            if seg.calls.is_empty() {
                continue; // no API call answered it, so it cost nothing
            }
            let rule = classify(&seg.kind, &seg.tools);
            let key = segment_key(&seg.session, seg.start.as_ref().map(|(ts, _)| ts.as_str()));
            let ml = model_labels.get(&key);
            let (label, group) = match source {
                LabelSource::Rule => { let g = group_of(&rule); (rule.clone(), g) }
                LabelSource::Model => {
                    let l = ml.cloned().unwrap_or_else(|| UNLABELLED.to_string());
                    let g = model_group(&l);
                    (l, g)
                }
            };
            let acc = by_label.entry(label.clone()).or_insert_with(|| Acc {
                row: ActivityRow {
                    activity: label.clone(), group: group.into(), segments: 0, calls: 0, input: 0, output: 0,
                    cache_write: 0, cache_read: 0, cost_usd: 0.0, share: 0.0, ctx_p50: 0, ctx_p95: 0, out_p95: 0,
                    local_candidate: false, local_reasons: Vec::new(), model_labels: Vec::new(), rule_labels: Vec::new(),
                },
                ctx: Vec::new(),
                out: Vec::new(),
            });
            acc.row.segments += 1;
            let other = match source {
                LabelSource::Rule => ml.cloned(),
                LabelSource::Model => Some(rule),
            };
            if let Some(o) = other {
                *other_counts.entry(label.clone()).or_default().entry(o).or_insert(0) += 1;
            }
            let mut seg_cost = 0.0;
            for c in &seg.calls {
                let cost = price.cost(&c.model, &c.tb);
                seg_cost += cost;
                acc.row.calls += 1;
                acc.row.input += c.tb.input;
                acc.row.output += c.tb.output;
                acc.row.cache_write += c.cache_write;
                acc.row.cache_read += c.tb.cache_read;
                acc.ctx.push(c.tb.input + c.cache_write + c.tb.cache_read);
                acc.out.push(c.tb.output);
            }
            acc.row.cost_usd += seg_cost;
            totals.cost_usd += seg_cost;
            totals.calls += seg.calls.len() as i64;
            totals.segments += 1;
            if group == "overhead" {
                totals.overhead_usd += seg_cost;
            }
            coverage.segments += 1;
            coverage.calls += seg.calls.len() as i64;
            coverage.cost_usd += seg_cost;
            if ml.is_some() {
                coverage.labelled_segments += 1;
                coverage.labelled_calls += seg.calls.len() as i64;
                coverage.labelled_usd += seg_cost;
            }
            *by_day.entry((seg.day.clone(), label)).or_insert(0.0) += seg_cost;
        }

        let mut rows: Vec<ActivityRow> = Vec::new();
        for (label, mut acc) in by_label {
            let r = &mut acc.row;
            r.share = if totals.cost_usd > 0.0 { r.cost_usd / totals.cost_usd } else { 0.0 };
            r.ctx_p50 = percentile(&mut acc.ctx, 0.50);
            r.ctx_p95 = percentile(&mut acc.ctx, 0.95);
            r.out_p95 = percentile(&mut acc.out, 0.95);
            let type_ok = r.group == "overhead" || LOCAL_TYPES.contains(&label.as_str());
            let ctx_ok = r.ctx_p95 <= settings.local_context_limit;
            let out_ok = r.out_p95 <= settings.local_output_limit;
            let mark = |ok: bool| if ok { "✓" } else { "✗" };
            r.local_reasons = if type_ok {
                vec![
                    format!("p95 context {} ≤ {} {}", kilo(r.ctx_p95), kilo(settings.local_context_limit), mark(ctx_ok)),
                    format!("p95 output {} ≤ {} {}", kilo(r.out_p95), kilo(settings.local_output_limit), mark(out_ok)),
                ]
            } else if r.group == "unlabelled" {
                vec!["no local-model label: run Label now".into()]
            } else if label == "triage" {
                vec!["reads code to find a root cause: keep on Claude".into()]
            } else {
                vec!["edits code: keep on Claude".into()]
            };
            r.local_candidate = type_ok && ctx_ok && out_ok;
            if let Some(counts) = other_counts.get(&label) {
                let mut list: Vec<LabelCount> = counts.iter().map(|(l, n)| LabelCount { label: l.clone(), segments: *n }).collect();
                list.sort_by(|x, y| y.segments.cmp(&x.segments).then_with(|| x.label.cmp(&y.label)));
                match source {
                    LabelSource::Rule => r.model_labels = list,
                    LabelSource::Model => r.rule_labels = list,
                }
            }
            if r.local_candidate {
                totals.local_candidate_usd += r.cost_usd;
            }
            rows.push(acc.row);
        }
        // Equal costs sort by name, so the order does not depend on hash order.
        rows.sort_by(|a, b| b.cost_usd.partial_cmp(&a.cost_usd).unwrap().then_with(|| a.activity.cmp(&b.activity)));

        let reconcile = self.reconcile(price, f)?;

        unknown.sort();
        Ok(ActivityReport {
            rows,
            by_day: by_day.into_iter().map(|((day, activity), cost_usd)| DayActivity { day, activity: activity.into(), cost_usd }).collect(),
            totals,
            reconcile,
            unknown_models: unknown,
            label_source: match source { LabelSource::Rule => "rule", LabelSource::Model => "model" }.into(),
            coverage,
        })
    }
}

impl Store {
    /// Our cost for the calls each Claude Code `cost-state` snapshot covers (from
    /// its process start to the snapshot), against the snapshot's own total. Honours
    /// the project filter only: a date range would cut sessions in two.
    fn reconcile(&self, price: &PriceTable, f: &Filters) -> rusqlite::Result<Reconcile> {
        let mut sql = String::from(
            // One window per (session, process start): an exact copy of a file holds
            // the same snapshot and must not count twice.
            "SELECT m.session_id || '|' || c.from_ts, COALESCE(m.model,''), m.input_tokens, m.output_tokens, m.cache_read,
                    m.ephemeral_1h, m.ephemeral_5m, m.web_searches, c.total
             FROM messages m
             JOIN (SELECT session_id, from_ts, MAX(to_ts) AS to_ts, MAX(total_cost_usd) AS total
                   FROM cost_states GROUP BY session_id, from_ts) c ON c.session_id = m.session_id
             WHERE m.role = 'assistant' AND m.dup = 0 AND m.ts >= c.from_ts AND m.ts <= c.to_ts",
        );
        let project = f.project.clone().unwrap_or_default();
        if f.project.is_some() {
            sql.push_str(" AND m.project = ?1");
        }
        let mut st = self.conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row| {
            Ok((
                r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?, r.get::<_, i64>(5)?, r.get::<_, i64>(6)?, r.get::<_, i64>(7)?, r.get::<_, f64>(8)?,
            ))
        };
        let rows = if f.project.is_some() { st.query_map([&project], map)? } else { st.query_map([], map)? };
        let mut out = Reconcile::default();
        let mut theirs: HashMap<String, f64> = HashMap::new();
        for r in rows {
            let (sid, model, input, output, cr, e1h, e5m, web, total) = r?;
            let tb = TokenBreakdown { input, output, cache_creation_5m: e5m, cache_creation_1h: e1h, cache_read: cr, web_searches: web };
            out.ours_usd += price.cost(&model, &tb);
            theirs.insert(sid, total);
        }
        out.sessions = theirs.keys().filter_map(|k| k.split('|').next()).collect::<std::collections::HashSet<_>>().len() as i64;
        out.claude_code_usd = theirs.values().sum();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_str;
    use crate::scanner::{ScannedFile, SubagentMeta};
    use serde_json::{json, Value};

    fn prompt(s: &str, ts: &str, text: &str) -> Value {
        json!({"type":"user","sessionId":s,"timestamp":ts,"message":{"role":"user","content":text}})
    }
    /// One API call on Haiku 4.5 ($1/MTok input), so a call's cost is input/1e6.
    fn call(s: &str, ts: &str, id: &str, input: i64, tools: &[(&str, &str, Value)]) -> Value {
        let blocks: Vec<Value> = tools.iter().map(|(tid, name, inp)| json!({"type":"tool_use","id":tid,"name":name,"input":inp})).collect();
        json!({"type":"assistant","sessionId":s,"timestamp":ts,"message":{"id":id,"role":"assistant","model":"claude-haiku-4-5-20251001","usage":{"input_tokens":input,"output_tokens":100},"content":blocks}})
    }
    /// Tool results, as Claude Code writes them back: (tool_use_id, output).
    fn results(s: &str, ts: &str, outs: &[(&str, &str)]) -> Value {
        let blocks: Vec<Value> = outs.iter().map(|(id, out)| json!({"type":"tool_result","tool_use_id":id,"content":out})).collect();
        json!({"type":"user","sessionId":s,"timestamp":ts,"message":{"role":"user","content":blocks}})
    }
    fn lines(v: &[Value]) -> String {
        v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("\n")
    }

    fn fixture() -> Store {
        let mut store = Store::open_in_memory();
        let add = |store: &mut Store, path: &str, sub: Option<SubagentMeta>, content: String| {
            let sf = ScannedFile { path: path.into(), project: "p".into(), size: 1, mtime: 1, subagent: sub };
            store.replace_file(&sf, &parse_str("p", &content)).unwrap();
        };
        let dev = lines(&[
            prompt("s-dev", "2026-10-01T10:00:00.000Z", "fix the parser"),
            call("s-dev", "2026-10-01T10:00:01.000Z", "d1", 1000, &[("e1", "Edit", json!({"file_path":"/r/a.rs"})), ("b1", "Bash", json!({"command":"cargo test"}))]),
            results("s-dev", "2026-10-01T10:00:01.500Z", &[("e1", "ok"), ("b1", "test result: FAILED. 3 passed; 2 failed")]),
            call("s-dev", "2026-10-01T10:00:02.000Z", "d2", 2000, &[("b2", "Bash", json!({"command":"cargo test"})), ("b3", "Bash", json!({"command":"cargo test 2>&1 | tail -3"}))]),
            results("s-dev", "2026-10-01T10:00:02.500Z", &[("b2", "test result: FAILED. 4 passed; 1 failed"), ("b3", "test result: FAILED. 4 passed; 1 failed")]),
            prompt("s-dev", "2026-10-01T12:00:00.000Z", "thanks, that is all for today"),
            call("s-dev", "2026-10-01T12:00:01.000Z", "d4", 4000, &[]),
            prompt("s-dev", "2026-10-02T09:00:00.000Z", "look around the repo"),
            call("s-dev", "2026-10-02T09:00:01.000Z", "d6", 6000, &[("ag", "Agent", json!({"prompt":"explore"}))]),
            prompt("s-dev", "2026-10-02T10:00:00.000Z", "what is new in pricing"),
            call("s-dev", "2026-10-02T10:00:01.000Z", "d7", 7000, &[("ws", "WebSearch", json!({"query":"q"})), ("wf", "WebFetch", json!({"url":"u"})), ("r3", "Read", json!({"file_path":"/r/a.rs"}))]),
            // Notes written and a passing test run: writing, so coding.
            prompt("s-dev", "2026-10-02T11:00:00.000Z", "write up the login fix"),
            call("s-dev", "2026-10-02T11:00:01.000Z", "d8", 20000, &[("he", "Edit", json!({"file_path":"/r/notes.md"})), ("ht", "Bash", json!({"command":"cargo test"}))]),
            // The suite passes later in the session: the baseline is 0 failures.
            results("s-dev", "2026-10-02T11:00:01.500Z", &[("ht", "test result: ok. 9 passed; 0 failed")]),
            prompt("s-dev", "2026-10-02T12:00:00.000Z", "fetch the release notes"),
            call("s-dev", "2026-10-02T12:00:01.000Z", "d9", 21000, &[("jf", "WebFetch", json!({"url":"u"}))]),
        ]);
        add(&mut store, "/x/p/s-dev.jsonl", None, dev);
        // The explore sub-agent's calls belong to the segment of the Agent call "ag".
        let sub = |ts: &str, id: &str, input: i64, tools: &[(&str, &str, Value)]| {
            let mut v = call("s-dev", ts, id, input, tools);
            v["agentId"] = json!("a1");
            v["isSidechain"] = json!(true);
            v
        };
        let child = lines(&[
            sub("2026-10-02T09:00:02.000Z", "x1", 8000, &[("x-r1", "Read", json!({"file_path":"/r/a.rs"})), ("x-g1", "Grep", json!({"pattern":"fn"}))]),
            sub("2026-10-02T09:00:03.000Z", "x2", 9000, &[("x-r2", "Read", json!({"file_path":"/r/b.rs"}))]),
        ]);
        add(&mut store, "/x/p/s-dev/subagents/agent-a1.jsonl",
            Some(SubagentMeta { agent_id: "a1".into(), parent_tool_use_id: Some("ag".into()), agent_type: Some("Explore".into()) }), child);
        let ops = lines(&[
            prompt("s-ops", "2026-10-01T10:00:00.000Z", "plan the week"),
            call("s-ops", "2026-10-01T10:00:01.000Z", "g1", 10000, &[]),
            prompt("s-ops", "2026-10-01T11:00:00.000Z", "restart the build agent"),
            call("s-ops", "2026-10-01T11:00:01.000Z", "g2", 11000, &[("gb1", "Bash", json!({"command":"launchctl kickstart -k gui/501/build"})), ("gb2", "Bash", json!({"command":"launchctl list"}))]),
            json!({"type":"user","sessionId":"s-ops","timestamp":"2026-10-01T13:00:00.000Z","message":{"role":"user","content":"<command-name>/compact</command-name>"}}),
            call("s-ops", "2026-10-01T13:00:01.000Z", "g4", 3000, &[]),
        ]);
        add(&mut store, "/x/p/s-ops.jsonl", None, ops);
        let solo = lines(&[
            // Before the process that wrote the snapshot started: not in its total.
            call("s-solo", "2026-10-02T07:00:00.000Z", "o0", 16000, &[]),
            prompt("s-solo", "2026-10-02T08:00:00.000Z", "add a button"),
            call("s-solo", "2026-10-02T08:00:01.000Z", "o1", 14000, &[("oe", "Edit", json!({"file_path":"/r/b.tsx"}))]),
            // Claude Code's own placeholder for an error: no usage, not a real model.
            json!({"type":"assistant","sessionId":"s-solo","timestamp":"2026-10-02T08:00:02.000Z","message":{"id":"syn","role":"assistant","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0},"content":[]}}),
            json!({"type":"cost-state","sessionId":"s-solo","totalCostUSD":0.02,"startTime":1790928000000i64}), // 08:00Z
            // After Claude Code's last snapshot: not in its total, so not compared.
            call("s-solo", "2026-10-02T08:05:00.000Z", "o2", 15000, &[("oe2", "Edit", json!({"file_path":"/r/c.tsx"}))]),
        ]);
        add(&mut store, "/x/p/s-solo.jsonl", None, solo);
        store.rebuild_sessions().unwrap();
        store
    }

    fn report(settings: &AppSettings) -> ActivityReport {
        fixture().activity(&PriceTable::seed(), &Filters::default(), settings).unwrap()
    }
    fn row<'a>(r: &'a ActivityReport, label: &str) -> &'a ActivityRow {
        r.rows.iter().find(|x| x.activity == label).unwrap_or_else(|| panic!("no row {label}: {:?}", r.rows.iter().map(|x| &x.activity).collect::<Vec<_>>()))
    }
    fn usd(input: i64) -> f64 {
        input as f64 / 1_000_000.0 + 0.0005 // + 100 output tokens at $5/MTok
    }

    #[test]
    fn each_segment_gets_one_label_and_its_calls_cost() {
        let r = report(&AppSettings::default());
        let close = |label: &str, inputs: &[i64]| {
            let want: f64 = inputs.iter().map(|i| usd(*i)).sum();
            let got = row(&r, label).cost_usd;
            assert!((got - want).abs() < 1e-9, "{label}: {got} != {want}");
        };
        close("debugging", &[1000, 2000]); // an edit and three failing test runs
        close("chat / planning", &[4000, 10000, 16000]); // no tools; the last came before any prompt
        close("review / explore", &[6000, 8000, 9000]); // the sub-agent's reads count here
        close("research", &[7000, 21000]);
        close("shell / ops", &[11000]);
        close("compaction", &[3000]);
        close("coding", &[14000, 15000, 20000]); // notes and docs count as writing
        assert_eq!(row(&r, "review / explore").segments, 1);
        assert_eq!(row(&r, "chat / planning").segments, 3);
        assert_eq!(r.rows.len(), 7);
        assert_eq!(r.rows.iter().map(|x| x.calls).sum::<i64>(), 16); // 7 dev + 2 sub-agent + 3 ops + 3 solo + 1 placeholder
    }

    #[test]
    fn overhead_and_work_are_grouped_and_totals_add_up() {
        let r = report(&AppSettings::default());
        assert_eq!(row(&r, "compaction").group, "overhead");
        assert_eq!(row(&r, "coding").group, "work");
        assert_eq!(row(&r, "chat / planning").group, "work");
        let sum: f64 = r.rows.iter().map(|x| x.cost_usd).sum();
        assert!((r.totals.cost_usd - sum).abs() < 1e-9);
        let over: f64 = r.rows.iter().filter(|x| x.group == "overhead").map(|x| x.cost_usd).sum();
        assert!((r.totals.overhead_usd - over).abs() < 1e-9);
        assert!((r.totals.overhead_usd - usd(3000)).abs() < 1e-9);
        assert!((r.rows.iter().map(|x| x.share).sum::<f64>() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn context_percentiles_and_the_local_flag_follow_the_configured_limit() {
        let r = report(&AppSettings::default());
        let review = row(&r, "review / explore");
        assert_eq!((review.ctx_p50, review.ctx_p95), (8000, 9000));
        assert!(review.local_candidate, "{:?}", review.local_reasons);
        assert!(!row(&r, "coding").local_candidate, "code edits are never flagged");

        let small = AppSettings { local_context_limit: 8500, ..Default::default() };
        let r = report(&small);
        let review = row(&r, "review / explore");
        assert!(!review.local_candidate);
        assert!(review.local_reasons[0].contains("✗"), "{:?}", review.local_reasons);
        assert!(row(&r, "compaction").local_candidate, "overhead is judged by size");
        let flagged: f64 = r.rows.iter().filter(|x| x.local_candidate).map(|x| x.cost_usd).sum();
        assert!((r.totals.local_candidate_usd - flagged).abs() < 1e-9);
    }

    #[test]
    fn by_day_and_reconciliation() {
        let r = report(&AppSettings::default());
        assert!(r.by_day.iter().any(|d| d.day == "2026-10-02" && d.activity == "coding"));
        assert!(r.by_day.iter().any(|d| d.day == "2026-10-01" && d.activity == "debugging"));
        assert!((r.by_day.iter().map(|d| d.cost_usd).sum::<f64>() - r.totals.cost_usd).abs() < 1e-9);
        assert_eq!(r.reconcile.sessions, 1);
        assert!((r.reconcile.ours_usd - usd(14000)).abs() < 1e-9);
        assert!((r.reconcile.claude_code_usd - 0.02).abs() < 1e-9);
        assert!(r.unknown_models.is_empty(), "the dated Haiku id matches its family");
    }

    #[test]
    fn date_filter_keeps_the_prompt_that_started_a_call() {
        let f = Filters { from: Some("2026-10-02".into()), ..Default::default() };
        let r = fixture().activity(&PriceTable::seed(), &f, &AppSettings::default()).unwrap();
        assert!(r.rows.iter().all(|x| x.activity != "debugging" && x.activity != "compaction"));
        assert!((row(&r, "research").cost_usd - usd(7000) - usd(21000)).abs() < 1e-9);
    }

    #[test]
    fn an_exact_copy_of_a_file_does_not_double_claude_codes_side_of_the_check() {
        let mut store = fixture();
        // The same s-solo file present under a second project folder.
        let content = lines(&[
            call("s-solo", "2026-10-02T08:00:01.000Z", "o1", 14000, &[]),
            json!({"type":"cost-state","sessionId":"s-solo","totalCostUSD":0.02,"startTime":1790928000000i64}),
        ]);
        let sf = ScannedFile { path: "/x/q/s-solo.jsonl".into(), project: "q".into(), size: 1, mtime: 1, subagent: None };
        store.replace_file(&sf, &parse_str("q", &content)).unwrap();
        store.rebuild_sessions().unwrap();
        let r = store.activity(&PriceTable::seed(), &Filters::default(), &AppSettings::default()).unwrap();
        assert!((r.reconcile.claude_code_usd - 0.02).abs() < 1e-9, "{}", r.reconcile.claude_code_usd);
        assert!((r.reconcile.ours_usd - usd(14000)).abs() < 1e-9);
    }

    #[test]
    fn an_orphan_subagents_tools_land_in_the_segment_of_their_own_time() {
        let mut store = fixture();
        let child = lines(&[{
            let mut v = call("s-dev", "2026-10-02T10:00:05.000Z", "z1", 500, &[("z-e", "Edit", json!({"file_path":"/r/z.rs"}))]);
            v["agentId"] = json!("a2");
            v
        }]);
        let sf = ScannedFile { path: "/x/p/s-dev/subagents/agent-a2.jsonl".into(), project: "p".into(), size: 1, mtime: 1,
            subagent: Some(SubagentMeta { agent_id: "a2".into(), parent_tool_use_id: None, agent_type: None }) };
        store.replace_file(&sf, &parse_str("p", &child)).unwrap();
        store.rebuild_sessions().unwrap();
        let r = store.activity(&PriceTable::seed(), &Filters::default(), &AppSettings::default()).unwrap();
        // 10:00:05 is inside the 10:00 research segment, so its edit makes that
        // segment coding; with no time the edit fell before every prompt and was lost.
        assert!((row(&r, "research").cost_usd - usd(21000)).abs() < 1e-9, "{}", row(&r, "research").cost_usd);
        let coding = usd(14000) + usd(15000) + usd(20000) + usd(7000) + usd(500);
        assert!((row(&r, "coding").cost_usd - coding).abs() < 1e-9, "{}", row(&r, "coding").cost_usd);
    }

    fn seq(steps: &[(&str, Option<&str>, Option<bool>)]) -> ToolMix {
        let mut t = ToolMix::default();
        for (name, tag, failed) in steps { t.add(name, *tag, *failed); }
        t
    }

    #[test]
    fn tdd_red_green_is_coding_and_a_stuck_red_suite_is_debugging() {
        let fail = ("Bash", Some("test"), Some(true));
        let pass = ("Bash", Some("test"), Some(false));
        let edit = ("Edit", None, None);
        // Test-driven: red, then green, cycle after cycle. Many runs, never stuck.
        let tdd = seq(&[edit, fail, edit, pass, edit, fail, edit, pass, edit, fail, edit, pass]);
        assert_eq!(classify("human", &tdd), "coding");
        // Three passing runs and an edit was "debugging" under the old rule.
        assert_eq!(classify("human", &seq(&[edit, pass, pass, pass])), "coding");
        // Fix attempts that keep failing: three red runs in a row, with edits between.
        let stuck = seq(&[fail, edit, fail, edit, fail, edit, pass]);
        assert_eq!(classify("human", &stuck), "debugging");
        // Unknown outcomes never count as failures.
        assert_eq!(classify("human", &seq(&[edit, ("Bash", Some("test"), None), ("Bash", Some("test"), None), ("Bash", Some("test"), None)])), "coding");
    }

    /// A store with one session per test: runs whose results report `counts`
    /// failures (in order), an edit, and, for `sub`, sub-agent runs interleaved.
    fn red_suite(sessions: &[(&str, &[u32])]) -> ActivityReport {
        let mut store = Store::open_in_memory();
        for (sid, counts) in sessions {
            let mut v = vec![prompt(sid, "2026-10-01T10:00:00.000Z", "work"),
                call(sid, "2026-10-01T10:00:00.500Z", &format!("{sid}-e"), 1000, &[(&format!("{sid}-edit"), "Edit", json!({"file_path":"/r/a.rs"}))])];
            for (i, n) in counts.iter().enumerate() {
                let ts = format!("2026-10-01T10:00:{:02}.000Z", i + 1);
                let id = format!("{sid}-t{i}");
                v.push(call(sid, &ts, &format!("{sid}-c{i}"), 1000, &[(&id, "Bash", json!({"command":"npm test"}))]));
                v.push(results(sid, &ts, &[(&id, &format!("# pass 9\n# fail {n}"))]));
            }
            let sf = ScannedFile { path: format!("/x/p/{sid}.jsonl").into(), project: "p".into(), size: 1, mtime: 1, subagent: None };
            store.replace_file(&sf, &parse_str("p", &lines(&v))).unwrap();
        }
        store.rebuild_sessions().unwrap();
        store.activity(&PriceTable::seed(), &Filters::default(), &AppSettings::default()).unwrap()
    }

    #[test]
    fn a_suite_with_known_failures_is_not_debugging_until_it_gets_worse() {
        // 22 known failures every run: the session's baseline, so no run is failing.
        let r = red_suite(&[("s-red", &[22, 22, 22, 22])]);
        assert!(r.rows.iter().any(|x| x.activity == "coding"));
        assert!(r.rows.iter().all(|x| x.activity != "debugging"));
        // Clearly above the baseline three times running, then back to it: debugging.
        let r = red_suite(&[("s-fix", &[32, 30, 28, 22])]);
        assert!(r.rows.iter().any(|x| x.activity == "debugging"), "{:?}", r.rows.iter().map(|x| &x.activity).collect::<Vec<_>>());
        // A known-failure count that wobbles (19 vs 22) is noise, not debugging:
        // within 20% above a baseline of 10 or more counts as the baseline.
        let r = red_suite(&[("s-flaky", &[19, 22, 22, 22])]);
        assert!(r.rows.iter().all(|x| x.activity != "debugging"), "{:?}", r.rows.iter().map(|x| &x.activity).collect::<Vec<_>>());
        // A clean suite failing three times running: debugging, as before.
        let r = red_suite(&[("s-clean", &[2, 1, 1, 0])]);
        assert!(r.rows.iter().any(|x| x.activity == "debugging"));
    }

    /// One session, real-shaped: runs of (command, failures reported), in order,
    /// after an edit.
    fn mixed_runs(runs: &[(&str, u32)]) -> ActivityReport {
        let mut store = Store::open_in_memory();
        let sid = "s-mix";
        let mut v = vec![prompt(sid, "2026-10-01T10:00:00.000Z", "work"),
            call(sid, "2026-10-01T10:00:00.500Z", "e", 1000, &[("edit", "Edit", json!({"file_path":"/r/a.rs"}))])];
        for (i, (cmd, n)) in runs.iter().enumerate() {
            let ts = format!("2026-10-01T10:00:{:02}.000Z", i + 1);
            let id = format!("t{i}");
            v.push(call(sid, &ts, &format!("c{i}"), 1000, &[(&id, "Bash", json!({ "command": cmd }))]));
            v.push(results(sid, &ts, &[(&id, &format!("# pass 9\n# fail {n}"))]));
        }
        let sf = ScannedFile { path: "/x/p/s-mix.jsonl".into(), project: "p".into(), size: 1, mtime: 1, subagent: None };
        store.replace_file(&sf, &parse_str("p", &lines(&v))).unwrap();
        store.rebuild_sessions().unwrap();
        store.activity(&PriceTable::seed(), &Filters::default(), &AppSettings::default()).unwrap()
    }

    #[test]
    fn the_baseline_is_per_test_command_so_a_clean_focused_run_does_not_reset_it() {
        let full = "cd /r && node --test test/*.test.cjs 2>&1 | tail -5";
        let focused = "node --test test/gate.test.cjs";
        // The full suite always shows its 22 known failures; focused runs are clean.
        // Under a per-session baseline (0, from the focused run) the three full runs in
        // a row would read as a failing streak.
        let r = mixed_runs(&[(focused, 0), (full, 22), (full, 22), (full, 22), (focused, 0), (full, 22)]);
        assert!(r.rows.iter().all(|x| x.activity != "debugging"), "{:?}", r.rows.iter().map(|x| &x.activity).collect::<Vec<_>>());
        // A focused run failing three times above its own baseline is debugging.
        let r = mixed_runs(&[(focused, 2), (focused, 1), (focused, 1), (focused, 0)]);
        assert!(r.rows.iter().any(|x| x.activity == "debugging"));
    }

    #[test]
    fn parent_and_sub_agent_test_runs_join_one_streak_in_time_order() {
        let mut store = Store::open_in_memory();
        let parent = lines(&[
            prompt("s-p", "2026-10-01T10:00:00.000Z", "work"),
            call("s-p", "2026-10-01T10:00:01.000Z", "p1", 1000, &[("ag2", "Agent", json!({})), ("pe", "Edit", json!({"file_path":"/r/a.rs"}))]),
            call("s-p", "2026-10-01T10:00:09.000Z", "p2", 1000, &[("pt", "Bash", json!({"command":"npm test"}))]),
            results("s-p", "2026-10-01T10:00:09.500Z", &[("pt", "# fail 1")]),
        ]);
        let sf = ScannedFile { path: "/x/p/s-p.jsonl".into(), project: "p".into(), size: 1, mtime: 1, subagent: None };
        store.replace_file(&sf, &parse_str("p", &parent)).unwrap();
        // The sub-agent fails twice, then passes, all BEFORE the parent's failing run.
        let sub = |ts: &str, id: &str, tid: &str, out: &str| -> Vec<Value> {
            let mut c = call("s-p", ts, id, 1000, &[(tid, "Bash", json!({"command":"npm test"}))]);
            c["agentId"] = json!("a9");
            let mut r = results("s-p", ts, &[(tid, out)]);
            r["agentId"] = json!("a9");
            vec![c, r]
        };
        let mut v = sub("2026-10-01T10:00:02.000Z", "x1", "st1", "# fail 1");
        v.extend(sub("2026-10-01T10:00:03.000Z", "x2", "st2", "# fail 1"));
        v.extend(sub("2026-10-01T10:00:04.000Z", "x3", "st3", "# fail 0"));
        let sf = ScannedFile { path: "/x/p/s-p/subagents/agent-a9.jsonl".into(), project: "p".into(), size: 1, mtime: 1,
            subagent: Some(SubagentMeta { agent_id: "a9".into(), parent_tool_use_id: Some("ag2".into()), agent_type: None }) };
        store.replace_file(&sf, &parse_str("p", &lines(&v))).unwrap();
        store.rebuild_sessions().unwrap();
        let r = store.activity(&PriceTable::seed(), &Filters::default(), &AppSettings::default()).unwrap();
        // In time order: fail, fail, pass, fail. The longest streak is 2: coding.
        assert!(r.rows.iter().all(|x| x.activity != "debugging"), "{:?}", r.rows.iter().map(|x| &x.activity).collect::<Vec<_>>());
        assert!(r.rows.iter().any(|x| x.activity == "coding"));
    }

    #[test]
    fn code_review_reads_and_tests_without_source_edits() {
        let read = ("Read", None, None);
        let pass = ("Bash", Some("test"), Some(false));
        let fail = ("Bash", Some("test"), Some(true));
        let pr = ("Bash", Some("review"), None);
        let notes = ("Write", Some("notes"), None);
        let edit = ("Edit", None, None);
        // A review that checks out the PR, reads it and runs the tests.
        assert_eq!(classify("human", &seq(&[pr, read, read, pass, fail, read])), "code review");
        // Reads plus a test run, no PR command, no edits: still a review.
        assert_eq!(classify("human", &seq(&[read, read, read, pass])), "code review");
        // Writing the review's notes file does not make it coding.
        assert_eq!(classify("human", &seq(&[pr, read, pass, notes, notes])), "code review");
        // Test-driven coding stays coding: it edits source.
        assert_eq!(classify("human", &seq(&[read, edit, fail, edit, pass, read])), "coding");
        // A local review done through the shell: shell reads, a git range, tests,
        // a scratch edit that was reverted, and a report written as notes.
        let sh_read = ("Bash", Some("read"), None);
        let range = ("Bash", Some("review"), None);
        let scratch = ("Bash", Some("scratch"), None);
        assert_eq!(classify("human", &seq(&[sh_read, range, sh_read, scratch, fail, pass, notes])), "code review");
        // A docs-only change with reads and a test run is writing docs, not a review.
        assert_eq!(classify("human", &seq(&[read, ("Edit", Some("notes"), None), read, pass])), "coding");
        // Reading alone is still review / explore.
        assert_eq!(classify("human", &seq(&[read, read, read])), "review / explore");
        assert_eq!(group_of("code review"), "work");
    }

    #[test]
    fn classify_rules() {
        let mix = |f: &dyn Fn(&mut ToolMix)| { let mut t = ToolMix::default(); f(&mut t); t };
        // (name, tag) shorthand for the cases below that carry no outcome.
        trait AddTwo { fn add2(&mut self, n: &str, t: Option<&str>); }
        impl AddTwo for ToolMix { fn add2(&mut self, n: &str, t: Option<&str>) { self.add(n, t, None) } }
        let none = ToolMix::default();
        assert_eq!(classify("human", &none), "chat / planning");
        assert_eq!(classify("task-notification", &none), "chat / planning");
        assert_eq!(classify("compact", &none), "compaction");
        assert_eq!(classify("compact", &mix(&|t| t.add2("Edit", None))), "compaction", "a compaction is overhead whatever ran");
        assert_eq!(classify("none", &mix(&|t| t.add2("Edit", None))), "coding", "calls before any prompt still classify by tools");
        assert_eq!(group_of("compaction"), "overhead");
        // Planning tools alone are not work.
        assert_eq!(classify("human", &mix(&|t| { t.add2("TodoWrite", None); t.add2("Agent", None); })), "chat / planning");
        let bash = mix(&|t| { t.add2("Bash", None); t.add2("Bash", None); });
        let misc = mix(&|t| { t.add2("AskUserQuestion", None); t.add2("Bash", None); t.add2("Monitor", None); });
        assert_eq!(classify("human", &misc), "other work");
        let two_tests = mix(&|t| { t.add2("Edit", None); t.add2("Bash", Some("test")); t.add2("Bash", Some("test")); });
        assert_eq!(classify("human", &two_tests), "coding", "under three test runs is not debugging");
        assert_eq!(classify("human", &bash), "shell / ops");
        let shell_edit = mix(&|t| { t.add2("Bash", Some("write")); t.add2("Bash", None); });
        assert_eq!(classify("human", &shell_edit), "coding", "a file written from the shell is an edit");
    }

    /// Store a model label for each segment whose rule label `pick` maps to one.
    fn label_segments(store: &Store, model: &str, pick: impl Fn(&str) -> Option<&'static str>) {
        let (segments, _) = store.build_segments(&PriceTable::seed(), &Filters::default()).unwrap();
        for seg in segments.values().filter(|s| !s.calls.is_empty()) {
            let rule = classify(&seg.kind, &seg.tools);
            if let Some(l) = pick(&rule) {
                store.save_model_label(&segment_key(&seg.session, seg.start.as_ref().map(|(ts, _)| ts.as_str())), model, l).unwrap();
            }
        }
    }
    fn with_model() -> AppSettings {
        AppSettings { ollama_model: "qwen3:8b".into(), ..Default::default() }
    }
    fn close(a: f64, b: f64) -> bool { (a - b).abs() < 1e-9 }

    #[test]
    fn model_labels_regroup_every_total_and_keep_the_cost() {
        let store = fixture();
        label_segments(&store, "qwen3:8b", |rule| match rule {
            "coding" | "debugging" => Some("debugging"),
            "chat / planning" | "compaction" => Some("overhead"),
            "research" => Some("research"),
            _ => None,
        });
        let rule = store.activity(&PriceTable::seed(), &Filters::default(), &with_model()).unwrap();
        let m = store.activity_by(&PriceTable::seed(), &Filters::default(), &with_model(), LabelSource::Model).unwrap();
        assert_eq!((rule.label_source.as_str(), m.label_source.as_str()), ("rule", "model"));

        // Same money, calls and segments, regrouped.
        assert!(close(m.totals.cost_usd, rule.totals.cost_usd));
        assert_eq!((m.totals.calls, m.totals.segments), (rule.totals.calls, rule.totals.segments));
        assert!(close(m.rows.iter().map(|x| x.cost_usd).sum(), m.totals.cost_usd));
        assert!(close(m.rows.iter().map(|x| x.share).sum(), 1.0));
        let cost = |r: &ActivityReport, l: &str| r.rows.iter().find(|x| x.activity == l).map(|x| x.cost_usd).unwrap_or(0.0);
        assert!(close(cost(&m, "debugging"), cost(&rule, "coding") + cost(&rule, "debugging")));
        assert!(close(cost(&m, "overhead"), cost(&rule, "chat / planning") + cost(&rule, "compaction")));

        // The model's "overhead" is overhead; what it did not label has its own row and group.
        assert_eq!(row(&m, "overhead").group, "overhead");
        assert!(close(m.totals.overhead_usd, cost(&m, "overhead")));
        let un = row(&m, UNLABELLED);
        assert_eq!(un.group, "unlabelled");
        assert!(!un.local_candidate);
        assert!(row(&m, "overhead").local_candidate, "{:?}", row(&m, "overhead").local_reasons);

        // Each model row says which rule labels it took, largest first.
        let d = row(&m, "debugging");
        let names: Vec<&str> = d.rule_labels.iter().map(|x| x.label.as_str()).collect();
        assert!(names.contains(&"coding") && names.contains(&"debugging"), "{names:?}");
        assert_eq!(d.rule_labels.iter().map(|x| x.segments).sum::<i64>(), d.segments);
        assert!(d.model_labels.is_empty());

        // Days follow the model labels too.
        assert!(m.by_day.iter().any(|x| x.activity == "debugging"));
        assert!(!m.by_day.iter().any(|x| x.activity == "coding"));
        assert!(close(m.by_day.iter().map(|x| x.cost_usd).sum(), m.totals.cost_usd));

        // Coverage: what the model labelled, the same in both views.
        let c = &m.coverage;
        assert_eq!(c, &rule.coverage);
        assert_eq!(c.model, "qwen3:8b");
        assert_eq!(c.segments, m.totals.segments);
        assert_eq!(c.segments - c.labelled_segments, un.segments);
        assert_eq!(c.calls - c.labelled_calls, un.calls);
        assert!(close(c.cost_usd - c.labelled_usd, un.cost_usd));
        assert!(c.labelled_segments > 0 && c.labelled_segments < c.segments);
    }

    #[test]
    fn the_rule_view_is_unchanged_and_coverage_is_empty_without_model_labels() {
        let store = fixture();
        let before = store.activity(&PriceTable::seed(), &Filters::default(), &with_model()).unwrap();
        assert_eq!(before.coverage.labelled_segments, 0);
        assert_eq!(before.coverage.segments, before.totals.segments);
        label_segments(&store, "qwen3:8b", |_| Some("research"));
        let after = store.activity(&PriceTable::seed(), &Filters::default(), &with_model()).unwrap();
        assert_eq!(after.rows.iter().map(|x| (&x.activity, x.cost_usd)).collect::<Vec<_>>(),
                   before.rows.iter().map(|x| (&x.activity, x.cost_usd)).collect::<Vec<_>>());
        assert!(after.rows.iter().all(|x| x.rule_labels.is_empty()));
        assert_eq!(after.coverage.labelled_segments, after.coverage.segments);
        assert!(close(after.coverage.labelled_usd, after.totals.cost_usd));
        // No model set: nothing counts as labelled, even with labels stored.
        let none = store.activity(&PriceTable::seed(), &Filters::default(), &AppSettings::default()).unwrap();
        assert_eq!((none.coverage.labelled_segments, none.coverage.model.as_str()), (0, ""));
        // Another model's labels are not this model's.
        let other = AppSettings { ollama_model: "llama3".into(), ..Default::default() };
        let m = store.activity_by(&PriceTable::seed(), &Filters::default(), &other, LabelSource::Model).unwrap();
        assert_eq!(m.rows.len(), 1);
        assert_eq!(m.rows[0].activity, UNLABELLED);
        assert!(close(m.totals.overhead_usd, 0.0));
    }

    #[test]
    fn the_label_source_reads_as_the_ui_sends_it() {
        assert_eq!(serde_json::from_str::<LabelSource>("\"rule\"").unwrap(), LabelSource::Rule);
        assert_eq!(serde_json::from_str::<LabelSource>("\"model\"").unwrap(), LabelSource::Model);
        assert!(serde_json::from_str::<LabelSource>("\"llm\"").is_err());
        assert_eq!(LabelSource::default(), LabelSource::Rule);
    }

    #[test]
    fn percentile_is_nearest_rank() {
        assert_eq!(percentile(&mut [], 0.5), 0);
        assert_eq!(percentile(&mut [5], 0.95), 5);
        let mut v: Vec<i64> = (1..=100).rev().collect();
        assert_eq!(percentile(&mut v, 0.50), 50);
        assert_eq!(percentile(&mut v, 0.95), 95);
    }
}
