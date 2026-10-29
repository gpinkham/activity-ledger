//! The weekly report: one local week, Monday to Sunday, as a plain Markdown file.
//!
//! Every figure comes from the same rollups the main window shows (`totals`,
//! `by_day_with`, `by_model`, `by_project`, `get_sessions`), asked for the week's
//! exact instants, so the report's total is the Overview's total for those seven
//! local days. The report never carries prompt text, session titles, session ids,
//! paths or `cwd`; with `hide_projects` it carries no project name either.

use crate::gauge::{iso, Clock};
use crate::metrics::{bind, where_clause, Filters, Totals};
use crate::pricing::PriceTable;
use crate::store::Store;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc, Weekday};
use std::fmt::Write;

pub const CLOSING_LINE: &str =
    "Costs are estimates at public API prices, not a bill. Subscription plans are not billed this way.";

const TOP_PROJECTS: usize = 10;
const TOP_SESSIONS: usize = 5;

/// The Monday a report week starts on, from `YYYY-MM-DD`. Any other day is refused,
/// so a caller cannot ask for a week that does not run Monday to Sunday.
pub fn parse_week_start(s: &str) -> Result<NaiveDate, String> {
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| format!("not a date: {s}"))?;
    if d.weekday() != Weekday::Mon {
        return Err(format!("a report week starts on a Monday; {s} is a {:?}", d.weekday()));
    }
    Ok(d)
}

/// The Monday of the week holding `d`.
pub fn monday_of(d: NaiveDate) -> NaiveDate {
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

/// The instants a local week covers: from its Monday's local midnight to the next
/// Monday's.
fn week_filters(clock: &dyn Clock, monday: NaiveDate) -> Filters {
    Filters {
        from_ts: Some(iso(clock.start_of(monday))),
        to_ts: Some(iso(clock.start_of(monday + Duration::days(7)))),
        ..Default::default()
    }
}

struct Week {
    totals: Totals,
    web_searches: i64,
}

fn week(store: &Store, price: &PriceTable, f: &Filters) -> rusqlite::Result<Week> {
    let totals = store.totals(price, f)?;
    let (w, binds) = where_clause(f);
    let web_searches: i64 = store.conn.query_row(
        &format!("SELECT COALESCE(SUM(web_searches), 0) FROM messages {w}"),
        bind(&binds).as_slice(),
        |r| r.get(0),
    )?;
    Ok(Week { totals, web_searches })
}

/// "+12%", "-5%", "0%", or "new" when the week before had none.
pub fn change(now: f64, before: f64) -> String {
    if now == before {
        return "0%".into();
    }
    if before == 0.0 {
        return "new".into();
    }
    let pct = ((now - before) / before * 100.0).round() as i64;
    match pct {
        0 => "0%".into(),
        p if p > 0 => format!("+{p}%"),
        p => format!("{p}%"),
    }
}

fn usd(n: f64) -> String {
    format!("${n:.2}")
}

/// 1234567 as "1,234,567".
fn count(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn share(part: f64, whole: f64) -> String {
    if whole <= 0.0 { "0%".into() } else { format!("{}%", (part / whole * 100.0).round() as i64) }
}

/// "26 Oct 2026".
fn long_date(d: NaiveDate) -> String {
    d.format("%-d %b %Y").to_string()
}

/// "Mon 26 Oct".
fn short_date(d: NaiveDate) -> String {
    d.format("%a %-d %b").to_string()
}

fn duration(secs: i64) -> String {
    let mins = secs / 60;
    match mins {
        0 => "under 1 min".into(),
        m if m < 60 => format!("{m} min"),
        m => format!("{} h {:02} min", m / 60, m % 60),
    }
}

/// A table cell: a `|` in a project name must not end the cell.
fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// "A" .. "Z", then "AA", "AB" ..: a project's letter by its cost rank.
fn letter(rank: usize) -> String {
    let mut n = rank + 1;
    let mut s = String::new();
    while n > 0 {
        n -= 1;
        s.insert(0, (b'A' + (n % 26) as u8) as char);
        n /= 26;
    }
    s
}

/// `written` is the local time the report is written, already formatted, so the
/// golden test is stable.
pub fn build_report(
    store: &Store,
    price: &PriceTable,
    clock: &dyn Clock,
    monday: NaiveDate,
    hide_projects: bool,
    written: &str,
) -> rusqlite::Result<String> {
    let sunday = monday + Duration::days(6);
    let this = week_filters(clock, monday);
    let now = week(store, price, &this)?;
    let before = week(store, price, &week_filters(clock, monday - Duration::days(7)))?;

    // Projects by cost, each lettered by that rank, for hiding.
    let projects = store.by_project(price, &this)?;
    let name = |project: &str| -> String {
        if hide_projects {
            let rank = projects.iter().position(|p| p.project == project).unwrap_or(projects.len());
            format!("Project {}", letter(rank))
        } else {
            cell(project)
        }
    };

    let mut md = String::new();
    let _ = writeln!(md, "# Activity Ledger: week of {}", long_date(monday));
    let _ = writeln!(md);
    let so_far = if (monday..=sunday).contains(&clock.today()) { " This week so far." } else { "" };
    let _ = writeln!(
        md,
        "Monday {} to Sunday {}, local time.{so_far} Written {written}.",
        long_date(monday),
        long_date(sunday)
    );

    // 1. Totals.
    let (t, b) = (&now.totals, &before.totals);
    let _ = writeln!(md, "\n## Totals\n");
    let _ = writeln!(md, "| | This week | Week before | Change |");
    let _ = writeln!(md, "|---|---:|---:|---:|");
    let _ = writeln!(md, "| API-equivalent cost | {} | {} | {} |", usd(t.est_cost_usd), usd(b.est_cost_usd), change(round_cents(t.est_cost_usd), round_cents(b.est_cost_usd)));
    for (label, n, p) in [
        ("Sessions", t.sessions, b.sessions),
        ("Messages", t.messages, b.messages),
        ("Input tokens", t.input_tokens, b.input_tokens),
        ("Output tokens", t.output_tokens, b.output_tokens),
        ("Cache read tokens", t.cache_read, b.cache_read),
        ("Cache write tokens", t.cache_creation, b.cache_creation),
        ("Web searches", now.web_searches, before.web_searches),
    ] {
        let _ = writeln!(md, "| {label} | {} | {} | {} |", count(n), count(p), change(n as f64, p as f64));
    }

    // 2. By day: all seven, empty days as 0.
    let days = store.by_day_with(
        price,
        &Filters { from: Some(monday.to_string()), to: Some(sunday.to_string()), ..Default::default() },
        clock,
    )?;
    let _ = writeln!(md, "\n## By day\n");
    let _ = writeln!(md, "| Day | Cost | Messages |");
    let _ = writeln!(md, "|---|---:|---:|");
    for i in 0..7 {
        let d = monday + Duration::days(i);
        let key = d.to_string();
        let (cost, msgs) = days.iter().find(|r| r.day == key).map_or((0.0, 0), |r| (r.cost_usd, r.messages));
        let _ = writeln!(md, "| {} | {} | {} |", short_date(d), usd(cost), count(msgs));
    }

    // 3. By model, by cost.
    let mut models = store.by_model(price, &this)?;
    models.sort_by(|a, b| b.cost_usd.partial_cmp(&a.cost_usd).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.model.cmp(&b.model)));
    let _ = writeln!(md, "\n## By model\n");
    if models.is_empty() {
        let _ = writeln!(md, "No activity this week.");
    } else {
        let _ = writeln!(md, "| Model | Cost | Share | Messages |");
        let _ = writeln!(md, "|---|---:|---:|---:|");
        for m in &models {
            let _ = writeln!(md, "| {} | {} | {} | {} |", cell(&m.model), usd(m.cost_usd), share(m.cost_usd, t.est_cost_usd), count(m.messages));
        }
    }

    // 4. By project: the top ten, then the rest as one row. No budget or limit.
    let _ = writeln!(md, "\n## By project\n");
    if projects.is_empty() {
        let _ = writeln!(md, "No activity this week.");
    } else {
        let _ = writeln!(md, "| Project | Cost | Sessions | Share |");
        let _ = writeln!(md, "|---|---:|---:|---:|");
        for p in projects.iter().take(TOP_PROJECTS) {
            let _ = writeln!(md, "| {} | {} | {} | {} |", name(&p.project), usd(p.cost_usd), count(p.sessions), share(p.cost_usd, t.est_cost_usd));
        }
        let rest = &projects[projects.len().min(TOP_PROJECTS)..];
        if !rest.is_empty() {
            let cost: f64 = rest.iter().map(|p| p.cost_usd).sum();
            let sessions: i64 = rest.iter().map(|p| p.sessions).sum();
            let _ = writeln!(md, "| Other ({} projects) | {} | {} | {} |", rest.len(), usd(cost), count(sessions), share(cost, t.est_cost_usd));
        }
    }

    // 5. Busiest sessions: date, project, duration and cost. No title, id or prompt.
    let mut sessions = store.get_sessions(price, &this)?;
    sessions.sort_by(|a, b| b.cost_usd.partial_cmp(&a.cost_usd).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.started_at.cmp(&b.started_at)));
    let _ = writeln!(md, "\n## Busiest sessions\n");
    if sessions.is_empty() {
        let _ = writeln!(md, "No activity this week.");
    } else {
        let _ = writeln!(md, "| Date | Project | Duration | Cost |");
        let _ = writeln!(md, "|---|---|---:|---:|");
        for s in sessions.iter().take(TOP_SESSIONS) {
            let date = s
                .started_at
                .as_deref()
                .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
                .map(|t| short_date(clock.date_of(t.with_timezone(&Utc))))
                .unwrap_or_default();
            let _ = writeln!(md, "| {date} | {} | {} | {} |", name(&s.project), duration(s.duration_secs), usd(s.cost_usd));
        }
    }

    let _ = writeln!(md, "\n{CLOSING_LINE}");
    Ok(md)
}

/// Costs are compared as shown, to the cent, so "$1.00 against $1.00" is 0%.
fn round_cents(n: f64) -> f64 {
    (n * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gauge::Tally;
    use crate::pricing::GaugePrices;
    use chrono::TimeZone;

    /// A Mac four hours west of UTC, as in the metrics tests, with a settable today.
    struct West(NaiveDate);
    impl Clock for West {
        fn today(&self) -> NaiveDate {
            self.0
        }
        fn date_of(&self, t: DateTime<Utc>) -> NaiveDate {
            (t - Duration::hours(4)).date_naive()
        }
        fn start_of(&self, d: NaiveDate) -> DateTime<Utc> {
            d.and_hms_opt(4, 0, 0).unwrap().and_utc()
        }
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    const MONDAY: (i32, u32, u32) = (2026, 10, 26);

    /// Strings that must never reach a report with hiding on: names, paths, cwd,
    /// session ids. Prompt and reply text must never reach one at all.
    const PROMPT: &str = "PLEASE-REFACTOR-THE-SECRET-BILLING-CODE";
    const REPLY: &str = "HERE-IS-THE-SECRET-REPLY";

    struct Fixture {
        root: std::path::PathBuf,
        store: Store,
        projects: Vec<String>,
        session_ids: Vec<String>,
        cwds: Vec<String>,
    }

    fn assistant(sid: &str, cwd: &str, id: &str, model: &str, ts: &str, input: i64, output: i64, web: i64) -> String {
        serde_json::json!({"type":"assistant","sessionId":sid,"cwd":cwd,"timestamp":ts,
            "message":{"id":id,"role":"assistant","model":model,
                "content":[{"type":"text","text":REPLY}],
                "usage":{"input_tokens":input,"output_tokens":output,"cache_read_input_tokens":input / 2,
                    "cache_creation_input_tokens":0,"server_tool_use":{"web_search_requests":web}}}})
        .to_string()
            + "\n"
    }

    fn user(sid: &str, cwd: &str, ts: &str) -> String {
        serde_json::json!({"type":"user","sessionId":sid,"cwd":cwd,"timestamp":ts,"uuid":format!("u-{sid}-{ts}"),
            "message":{"role":"user","content":PROMPT}})
        .to_string()
            + "\n"
    }

    /// Twelve projects in the report week, so two fold into "Other", plus calls in
    /// the week before and just after the week. Written to disk so the gauge can
    /// read the same files.
    fn fixture(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("al_report_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut projects = Vec::new();
        let mut session_ids = Vec::new();
        let mut cwds = Vec::new();
        let names = [
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet", "kilo", "lima",
        ];
        for (i, n) in names.iter().enumerate() {
            let project = format!("-Users-me-dev-{n}");
            let cwd = format!("/Users/me/dev/{n}");
            let sid = format!("{n}-session-0000-1111");
            let dir = root.join(&project);
            std::fs::create_dir_all(&dir).unwrap();
            let scale = (12 - i) as i64; // alpha costs most, lima least
            let model = ["claude-opus-5-5", "claude-sonnet-4-6", "claude-haiku-5-5"][i % 3];
            let mut body = user(&sid, &cwd, "2026-10-27T13:00:00.000Z");
            body += &assistant(&sid, &cwd, &format!("{n}-1"), model, "2026-10-27T13:05:00.000Z", 100_000 * scale, 2_000 * scale, (i == 0) as i64 * 3);
            body += &assistant(&sid, &cwd, &format!("{n}-2"), model, "2026-10-27T14:20:00.000Z", 50_000 * scale, 1_000 * scale, 0);
            std::fs::write(dir.join(format!("{sid}.jsonl")), body).unwrap();
            projects.push(project);
            session_ids.push(sid);
            cwds.push(cwd);
        }
        // alpha: the week before (counts there only), Sunday 23:30 local (this
        // week), and Monday 00:30 local the week after (neither).
        let alpha = root.join("-Users-me-dev-alpha");
        let cwd = "/Users/me/dev/alpha";
        let edges = [
            ("alpha-prev-0000-2222", "2026-10-20T15:00:00.000Z", 200_000),
            ("alpha-sun-0000-3333", "2026-11-02T03:30:00.000Z", 300_000),
            ("alpha-next-0000-4444", "2026-11-02T04:30:00.000Z", 900_000),
        ];
        for (sid, ts, input) in edges {
            let body = user(sid, cwd, ts) + &assistant(sid, cwd, &format!("{sid}-m"), "claude-opus-5-5", ts, input, 1_000, 0);
            std::fs::write(alpha.join(format!("{sid}.jsonl")), body).unwrap();
            session_ids.push(sid.to_string());
        }
        // Each file as last written after the week, as it would be on a real Mac: the
        // gauge reads only files written in its month.
        let written = std::time::UNIX_EPOCH + std::time::Duration::from_secs(
            Utc.with_ymd_and_hms(2026, 11, 2, 5, 0, 0).unwrap().timestamp() as u64,
        );
        for entry in walkdir::WalkDir::new(&root).into_iter().flatten().filter(|e| e.file_type().is_file()) {
            std::fs::File::options().write(true).open(entry.path()).unwrap().set_modified(written).unwrap();
        }
        let mut store = Store::open(&root.join("cache.sqlite")).unwrap();
        crate::commands::do_refresh(&mut store, &root).unwrap();
        Fixture { root, store, projects, session_ids, cwds }
    }

    fn report(fx: &Fixture, hide: bool) -> String {
        let monday = day(MONDAY.0, MONDAY.1, MONDAY.2);
        build_report(&fx.store, &PriceTable::seed(), &West(day(2026, 11, 3)), monday, hide, "3 Nov 2026 at 09:15").unwrap()
    }

    #[test]
    fn the_fixture_week_matches_the_golden_file() {
        let fx = fixture("golden");
        let got = report(&fx, false);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/weekly-report.golden.md");
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &got).unwrap();
        }
        let want = std::fs::read_to_string(&path).expect("golden file; run with UPDATE_GOLDEN=1 to write it");
        assert_eq!(got, want, "report differs from {}", path.display());
        std::fs::remove_dir_all(&fx.root).ok();
    }

    #[test]
    fn sunday_2330_local_counts_and_monday_0030_local_does_not() {
        let fx = fixture("edges");
        let md = report(&fx, false);
        // 03:30 UTC on Nov 2 is 23:30 on Sunday Nov 1 four hours west.
        assert!(md.contains("| Sun 1 Nov | $"), "{md}");
        let sunday_row = md.lines().find(|l| l.starts_with("| Sun 1 Nov |")).unwrap();
        assert!(sunday_row.ends_with("| 2 |"), "the 23:30 prompt and call are Sunday's: {sunday_row}");
        // Messages count prompts and calls, as the Overview does: 12 sessions of 3, plus
        // Sunday's 2. The 04:30 UTC pair is Monday Nov 2 there, next week's.
        let messages = md.lines().find(|l| l.starts_with("| Messages |")).unwrap();
        assert!(messages.starts_with("| Messages | 38 | 2 |"), "{messages}");
        std::fs::remove_dir_all(&fx.root).ok();
    }

    #[test]
    fn the_total_is_get_totals_and_the_gauge_for_the_same_local_days() {
        let fx = fixture("parity");
        let table = PriceTable::seed();
        let monday = day(MONDAY.0, MONDAY.1, MONDAY.2);
        let clock = West(day(2026, 11, 3));
        let f = week_filters(&clock, monday);
        let report_total = fx.store.totals(&table, &f).unwrap().est_cost_usd;
        // get_totals for the same seven local dates, asked by date.
        let by_days: f64 = fx
            .store
            .by_day_with(&table, &Filters { from: Some("2026-10-26".into()), to: Some("2026-11-01".into()), ..Default::default() }, &clock)
            .unwrap()
            .iter()
            .map(|r| r.cost_usd)
            .sum();
        assert!(report_total > 0.0);
        assert!((report_total - by_days).abs() < 1e-9, "{report_total} vs {by_days}");
        // The gauge's "today" for each of the seven days, summed.
        let mut gauge = 0.0;
        for i in 0..7 {
            let today = West(monday + Duration::days(i));
            let mut t = Tally::new();
            t.poll(&fx.root, &today);
            gauge += t.spend(&GaugePrices { table: table.clone(), managed: None }, &today).today;
        }
        assert!((report_total - gauge).abs() < 1e-9, "{report_total} vs gauge {gauge}");
        // And the report prints that figure.
        assert!(report(&fx, false).contains(&format!("| API-equivalent cost | {} |", usd(report_total))));
        std::fs::remove_dir_all(&fx.root).ok();
    }

    #[test]
    fn change_is_new_from_zero_and_zero_when_equal() {
        assert_eq!(change(5.0, 0.0), "new");
        assert_eq!(change(0.0, 0.0), "0%");
        assert_eq!(change(7.0, 7.0), "0%");
        assert_eq!(change(112.0, 100.0), "+12%");
        assert_eq!(change(95.0, 100.0), "-5%");
        assert_eq!(change(0.0, 4.0), "-100%");
        assert_eq!(change(1000.4, 1000.0), "0%");
    }

    #[test]
    fn more_than_ten_projects_fold_into_other() {
        let fx = fixture("fold");
        let md = report(&fx, false);
        let rows: Vec<&str> = md
            .split("## By project")
            .nth(1)
            .unwrap()
            .split("## Busiest")
            .next()
            .unwrap()
            .lines()
            .filter(|l| l.starts_with("| ") && !l.starts_with("| Project"))
            .collect();
        assert_eq!(rows.len(), 11, "{rows:?}");
        assert!(rows[10].starts_with("| Other (2 projects) |"), "{}", rows[10]);
        let ranked = fx.store.by_project(&PriceTable::seed(), &week_filters(&West(day(2026, 11, 3)), day(MONDAY.0, MONDAY.1, MONDAY.2))).unwrap();
        assert_eq!(ranked.len(), 12);
        for p in &ranked[10..] {
            assert!(!md.contains(p.project.as_str()), "{} is folded into Other", p.project);
        }
        for p in &ranked[..10] {
            assert!(md.contains(p.project.as_str()), "{} is in the top ten", p.project);
        }
        std::fs::remove_dir_all(&fx.root).ok();
    }

    #[test]
    fn hiding_leaves_no_project_name_path_cwd_or_session_id() {
        let fx = fixture("hide");
        let md = report(&fx, true);
        for s in fx.projects.iter().chain(&fx.session_ids).chain(&fx.cwds) {
            assert!(!md.contains(s.as_str()), "{s} leaked:\n{md}");
        }
        for n in ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet", "kilo", "lima", "/Users", "-Users", "me-dev"] {
            assert!(!md.contains(n), "{n} leaked:\n{md}");
        }
        // Lettered by cost rank, and the sessions use the same letters.
        assert!(md.contains("| Project A | "), "{md}");
        assert!(md.contains("| Project J | "), "{md}");
        let busiest = md.split("## Busiest sessions").nth(1).unwrap();
        assert!(busiest.contains("| Project A |"), "{busiest}");
        std::fs::remove_dir_all(&fx.root).ok();
    }

    #[test]
    fn no_prompt_or_reply_text_with_hiding_on_or_off() {
        let fx = fixture("prompt");
        for hide in [false, true] {
            let md = report(&fx, hide);
            assert!(!md.contains(PROMPT) && !md.contains(REPLY), "hide={hide}:\n{md}");
            assert!(!md.contains("SECRET"), "hide={hide}");
            for sid in &fx.session_ids {
                assert!(!md.contains(sid.as_str()), "session id {sid} leaked, hide={hide}");
            }
        }
        std::fs::remove_dir_all(&fx.root).ok();
    }

    #[test]
    fn the_closing_line_is_always_there_even_for_an_empty_week() {
        let store = Store::open_in_memory();
        let md = build_report(&store, &PriceTable::seed(), &West(day(2026, 11, 3)), day(2026, 10, 26), false, "now").unwrap();
        assert!(md.trim_end().ends_with(CLOSING_LINE), "{md}");
        assert!(md.contains("| Mon 26 Oct | $0.00 | 0 |"));
        assert!(md.contains("| Sun 1 Nov | $0.00 | 0 |"));
        assert!(md.contains("No activity this week."));
        let fx = fixture("closing");
        assert!(report(&fx, true).trim_end().ends_with(CLOSING_LINE));
        std::fs::remove_dir_all(&fx.root).ok();
    }

    #[test]
    fn this_week_so_far_is_said_when_today_is_in_the_week() {
        let store = Store::open_in_memory();
        let md = build_report(&store, &PriceTable::seed(), &West(day(2026, 10, 28)), day(2026, 10, 26), false, "now").unwrap();
        assert!(md.contains("This week so far."));
        let md = build_report(&store, &PriceTable::seed(), &West(day(2026, 11, 3)), day(2026, 10, 26), false, "now").unwrap();
        assert!(!md.contains("so far"));
    }

    #[test]
    fn a_week_must_start_on_a_monday() {
        assert_eq!(parse_week_start("2026-10-26").unwrap(), day(2026, 10, 26));
        assert!(parse_week_start("2026-10-27").is_err());
        assert!(parse_week_start("last week").is_err());
        assert_eq!(monday_of(day(2026, 11, 1)), day(2026, 10, 26));
        assert_eq!(monday_of(day(2026, 10, 26)), day(2026, 10, 26));
    }

    #[test]
    fn the_report_template_has_no_en_or_em_dash() {
        let src = include_str!("report.rs");
        assert!(!src.contains('\u{2013}') && !src.contains('\u{2014}'));
        let fx = fixture("dash");
        for hide in [false, true] {
            let md = report(&fx, hide);
            assert!(!md.contains('\u{2013}') && !md.contains('\u{2014}'), "{md}");
        }
        std::fs::remove_dir_all(&fx.root).ok();
    }

    #[test]
    fn formats() {
        assert_eq!(count(1_234_567), "1,234,567");
        assert_eq!(count(999), "999");
        assert_eq!(count(0), "0");
        assert_eq!(duration(30), "under 1 min");
        assert_eq!(duration(45 * 60), "45 min");
        assert_eq!(duration(65 * 60), "1 h 05 min");
        assert_eq!(letter(0), "A");
        assert_eq!(letter(25), "Z");
        assert_eq!(letter(26), "AA");
        assert_eq!(cell("a|b"), "a\\|b");
    }
}
