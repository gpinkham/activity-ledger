use crate::metrics::{bind, where_clause, Filters};
use crate::pricing::{PriceTable, TokenBreakdown};
use crate::store::Store;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub project: String,
    pub title: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub duration_secs: i64,
    pub message_count: i64,
    pub tokens: i64,
    pub cost_usd: f64,
    pub tool_calls: i64,
    pub models: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionTokens {
    pub input: i64,
    pub output: i64,
    pub cache_creation: i64,
    pub cache_read: i64,
}

#[derive(Debug, Serialize)]
pub struct SessionModel {
    pub model: String,
    pub messages: i64,
    pub tokens: i64,
    pub cost_usd: f64,
}

#[derive(Debug, Serialize)]
pub struct TimelineEvent {
    pub ts: String,
    pub kind: String, // "message" | "tool"
    pub label: String,
    pub model: Option<String>,
    pub output_tokens: Option<i64>,
    pub is_mcp: Option<bool>,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct SessionDetail {
    pub summary: SessionSummary,
    pub cache_hit_ratio: f64,
    pub tokens: SessionTokens,
    pub by_model: Vec<SessionModel>,
    pub events: Vec<TimelineEvent>,
}

const EVENT_CAP: usize = 400;

fn duration_secs(started: &Option<String>, ended: &Option<String>) -> i64 {
    match (started, ended) {
        (Some(s), Some(e)) => {
            let ps = chrono::DateTime::parse_from_rfc3339(s);
            let pe = chrono::DateTime::parse_from_rfc3339(e);
            if let (Ok(s), Ok(e)) = (ps, pe) {
                (e - s).num_seconds().max(0)
            } else {
                0
            }
        }
        _ => 0,
    }
}

// Accumulator used while folding per-(session, model) rows into per-session summaries.
struct Acc {
    project: String,
    started: Option<String>,
    ended: Option<String>,
    message_count: i64,
    tokens: i64,
    cost_usd: f64,
    models: Vec<String>,
}

impl Store {
    pub fn get_sessions(&self, price: &PriceTable, f: &Filters) -> rusqlite::Result<Vec<SessionSummary>> {
        let (w, binds) = where_clause(f);

        // Per (session, model) aggregate over messages.
        let msql = format!(
            "SELECT session_id, project, COALESCE(model,'unknown'), COUNT(*),
                    SUM(input_tokens), SUM(output_tokens), SUM(cache_read),
                    SUM(ephemeral_1h), SUM(ephemeral_5m), MIN(ts), MAX(ts)
             FROM messages {w} GROUP BY session_id, model"
        );
        let mut stmt = self.conn.prepare(&msql)?;
        let rows = stmt.query_map(bind(&binds).as_slice(), |r| {
            Ok((
                r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?, r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?, r.get::<_, i64>(8)?, r.get::<_, String>(9)?, r.get::<_, String>(10)?,
            ))
        })?;

        let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
        for row in rows {
            let (sid, project, model, msgs, input, output, cr, e1h, e5m, min_ts, max_ts) = row?;
            let tb = TokenBreakdown { input, output, cache_creation_5m: e5m, cache_creation_1h: e1h, cache_read: cr, ..Default::default() };
            let a = acc.entry(sid).or_insert(Acc {
                project,
                started: None,
                ended: None,
                message_count: 0,
                tokens: 0,
                cost_usd: 0.0,
                models: Vec::new(),
            });
            a.message_count += msgs;
            a.tokens += input + output;
            a.cost_usd += price.cost(&model, &tb);
            if model != "unknown" && !a.models.contains(&model) {
                a.models.push(model);
            }
            a.started = Some(match &a.started {
                Some(s) if s.as_str() <= min_ts.as_str() => s.clone(),
                _ => min_ts,
            });
            a.ended = Some(match &a.ended {
                Some(s) if s.as_str() >= max_ts.as_str() => s.clone(),
                _ => max_ts,
            });
        }

        // Tool-call counts per session.
        let tsql = format!("SELECT session_id, COUNT(*) FROM tool_calls {w} GROUP BY session_id");
        let mut tstmt = self.conn.prepare(&tsql)?;
        let trows = tstmt.query_map(bind(&binds).as_slice(), |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let mut tool_counts: BTreeMap<String, i64> = BTreeMap::new();
        for row in trows {
            let (sid, n) = row?;
            tool_counts.insert(sid, n);
        }

        // Titles (unfiltered map).
        let mut title_stmt = self.conn.prepare("SELECT session_id, title FROM session_titles")?;
        let title_rows = title_stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
        let mut titles: BTreeMap<String, Option<String>> = BTreeMap::new();
        for row in title_rows {
            let (sid, t) = row?;
            titles.insert(sid, t);
        }

        let mut out: Vec<SessionSummary> = acc
            .into_iter()
            .map(|(sid, a)| SessionSummary {
                duration_secs: duration_secs(&a.started, &a.ended),
                tool_calls: tool_counts.get(&sid).copied().unwrap_or(0),
                title: titles.get(&sid).cloned().flatten(),
                session_id: sid,
                project: a.project,
                started_at: a.started,
                ended_at: a.ended,
                message_count: a.message_count,
                tokens: a.tokens,
                cost_usd: a.cost_usd,
                models: a.models,
            })
            .collect();

        // Newest first.
        out.sort_by(|x, y| y.started_at.cmp(&x.started_at));
        Ok(out)
    }

    pub fn get_session_detail(&self, price: &PriceTable, session_id: &str) -> rusqlite::Result<SessionDetail> {
        // Summary (reuse get_sessions with no filter, then pick this session).
        let summary = self
            .get_sessions(price, &Filters::default())?
            .into_iter()
            .find(|s| s.session_id == session_id)
            .unwrap_or_else(|| SessionSummary {
                session_id: session_id.to_string(),
                project: String::new(),
                title: None,
                started_at: None,
                ended_at: None,
                duration_secs: 0,
                message_count: 0,
                tokens: 0,
                cost_usd: 0.0,
                tool_calls: 0,
                models: Vec::new(),
            });

        // Token breakdown.
        let tokens = self.conn.query_row(
            "SELECT SUM(input_tokens), SUM(output_tokens), SUM(cache_creation), SUM(cache_read)
             FROM messages WHERE session_id = ?1 AND dup = 0",
            rusqlite::params![session_id],
            |r| {
                Ok(SessionTokens {
                    input: r.get::<_, Option<i64>>(0)?.unwrap_or(0),
                    output: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    cache_creation: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    cache_read: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                })
            },
        )?;
        let cacheable = (tokens.cache_read + tokens.cache_creation) as f64;
        let cache_hit_ratio = if cacheable > 0.0 { tokens.cache_read as f64 / cacheable } else { 0.0 };

        // Per-model within the session.
        let mut mstmt = self.conn.prepare(
            "SELECT COALESCE(model,'unknown'), COUNT(*), SUM(input_tokens), SUM(output_tokens),
                    SUM(cache_read), SUM(ephemeral_1h), SUM(ephemeral_5m)
             FROM messages WHERE session_id = ?1 AND dup = 0 GROUP BY model ORDER BY 3 DESC",
        )?;
        let mrows = mstmt.query_map(rusqlite::params![session_id], |r| {
            Ok((
                r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?, r.get::<_, i64>(5)?, r.get::<_, i64>(6)?,
            ))
        })?;
        let mut by_model = Vec::new();
        for row in mrows {
            let (model, msgs, input, output, cr, e1h, e5m) = row?;
            let tb = TokenBreakdown { input, output, cache_creation_5m: e5m, cache_creation_1h: e1h, cache_read: cr, ..Default::default() };
            by_model.push(SessionModel { cost_usd: price.cost(&model, &tb), model, messages: msgs, tokens: input + output });
        }

        // Events: assistant messages + tool calls, merged & time-ordered.
        let mut raw: Vec<TimelineEvent> = Vec::new();
        let mut amstmt = self.conn.prepare(
            "SELECT ts, COALESCE(model,'unknown'), output_tokens FROM messages
             WHERE session_id = ?1 AND role = 'assistant' AND dup = 0 ORDER BY ts",
        )?;
        let arows = amstmt.query_map(rusqlite::params![session_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })?;
        for row in arows {
            let (ts, model, out) = row?;
            raw.push(TimelineEvent {
                ts,
                kind: "message".into(),
                label: model.clone(),
                model: Some(model),
                output_tokens: Some(out),
                is_mcp: None,
                count: 1,
            });
        }
        let mut tstmt = self.conn.prepare(
            "SELECT ts, name, is_mcp FROM tool_calls WHERE session_id = ?1 AND dup = 0 ORDER BY ts",
        )?;
        let trows = tstmt.query_map(rusqlite::params![session_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0))
        })?;
        for row in trows {
            let (ts, name, is_mcp) = row?;
            raw.push(TimelineEvent {
                ts,
                kind: "tool".into(),
                label: name,
                model: None,
                output_tokens: None,
                is_mcp: Some(is_mcp),
                count: 1,
            });
        }
        raw.sort_by(|a, b| a.ts.cmp(&b.ts));

        // Group consecutive identical tool events into one with a count.
        let mut grouped: Vec<TimelineEvent> = Vec::new();
        for e in raw {
            if let Some(last) = grouped.last_mut() {
                if last.kind == "tool" && e.kind == "tool" && last.label == e.label {
                    last.count += 1;
                    continue;
                }
            }
            grouped.push(e);
        }

        // Cap.
        let events = if grouped.len() > EVENT_CAP {
            let omitted = (grouped.len() - EVENT_CAP) as i64;
            let mut capped: Vec<TimelineEvent> = grouped.into_iter().take(EVENT_CAP).collect();
            let last_ts = capped.last().map(|e| e.ts.clone()).unwrap_or_default();
            capped.push(TimelineEvent {
                ts: last_ts,
                kind: "tool".into(),
                // We keep the oldest EVENT_CAP events (list is ascending by ts),
                // so the omitted ones are the most recent. count stays 1 so the
                // UI doesn't render a redundant "×N" (the label already states N).
                label: format!("… ({} more recent events omitted)", omitted),
                model: None,
                output_tokens: None,
                is_mcp: None,
                count: 1,
            });
            capped
        } else {
            grouped
        };

        Ok(SessionDetail { summary, cache_hit_ratio, tokens, by_model, events })
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
    fn lists_session_with_title_and_metrics() {
        let store = seeded();
        let sessions = store.get_sessions(&PriceTable::seed(), &Filters::default()).unwrap();
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.session_id, "s1");
        assert_eq!(s.title.as_deref(), Some("Refactor the parser"));
        assert_eq!(s.message_count, 3);
        assert_eq!(s.tokens, 168); // input 110 + output 58
        assert_eq!(s.tool_calls, 2);
        assert!(s.models.contains(&"claude-opus-4-8".to_string()));
        assert!(s.models.contains(&"claude-sonnet-5".to_string()));
        assert!(s.cost_usd > 0.0);
        assert_eq!(s.duration_secs, 60); // 18:11:00 → 18:12:00
    }

    #[test]
    fn project_filter_scopes_sessions() {
        let store = seeded();
        let f = Filters { project: Some("nope".into()), ..Default::default() };
        assert_eq!(store.get_sessions(&PriceTable::seed(), &f).unwrap().len(), 0);
    }

    #[test]
    fn detail_has_breakdown_and_ordered_events() {
        let store = seeded();
        let d = store.get_session_detail(&PriceTable::seed(), "s1").unwrap();
        assert_eq!(d.summary.session_id, "s1");
        assert_eq!(d.summary.message_count, 3);
        assert_eq!(d.tokens.input, 110);
        assert_eq!(d.tokens.output, 58);
        assert!(d.cache_hit_ratio > 0.9);
        assert!(d.by_model.iter().any(|m| m.model == "claude-opus-4-8"));
        // 2 assistant turns + 2 distinct tool calls = 4 events, time-ordered.
        assert_eq!(d.events.len(), 4);
        assert!(d.events.iter().any(|e| e.kind == "tool" && e.label == "Edit"));
        assert!(d.events.iter().any(|e| e.kind == "tool" && e.is_mcp == Some(true)));
        assert!(d.events.iter().any(|e| e.kind == "message" && e.model.as_deref() == Some("claude-opus-4-8")));
        // ordered non-decreasing by ts
        let ts: Vec<&str> = d.events.iter().map(|e| e.ts.as_str()).collect();
        let mut sorted = ts.clone();
        sorted.sort();
        assert_eq!(ts, sorted);
    }
}
