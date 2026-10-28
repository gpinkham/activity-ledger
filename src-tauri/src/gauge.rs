//! The menu bar gauge's running tally: what this month's Claude Code calls cost,
//! by local day, kept up to date by reading only what was added to a transcript
//! since the last poll.
//!
//! The figures are the day view's figures for the same calls. Each new chunk of a
//! transcript goes through the same parser, a call kept in several files counts
//! once (as `Store::mark_duplicates` does), and calls are summed by day and model
//! key before pricing, as `Store::by_day` does, and both count this Mac's local
//! days.

use crate::budget::first_of_month;
use crate::models::MessageRecord;
use crate::parser::parse_str;
use crate::pricing::{long_key, GaugePrices, PriceTable, TokenBreakdown};
use crate::scanner::scan_dir;
use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Local calendar dates, as this Mac sees them. Tests use a fixed zone.
pub trait Clock {
    fn today(&self) -> NaiveDate;
    fn date_of(&self, t: DateTime<Utc>) -> NaiveDate;
    /// The instant a local date begins.
    fn start_of(&self, d: NaiveDate) -> DateTime<Utc>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn today(&self) -> NaiveDate {
        Local::now().date_naive()
    }
    fn date_of(&self, t: DateTime<Utc>) -> NaiveDate {
        t.with_timezone(&Local).date_naive()
    }
    fn start_of(&self, d: NaiveDate) -> DateTime<Utc> {
        // Where a zone skips midnight, the day starts at the first hour that exists.
        (0..4)
            .find_map(|h| Local.from_local_datetime(&d.and_hms_opt(h, 0, 0)?).earliest())
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(|| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight")))
    }
}

/// An ISO-8601 instant as the transcripts write it, so it compares as text with `ts`.
pub fn iso(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// One API call's usage, merged across its lines and copies.
#[derive(Debug, Clone, PartialEq)]
struct Call {
    model: Option<String>,
    day: Option<NaiveDate>,
    input: i64,
    output: i64,
    cache_creation: i64,
    cache_read: i64,
    e1h: i64,
    e5m: i64,
}

impl Call {
    fn of(m: &MessageRecord, clock: &dyn Clock) -> Call {
        let day = DateTime::parse_from_rfc3339(&m.ts).ok().map(|t| clock.date_of(t.with_timezone(&Utc)));
        Call {
            model: m.model.clone(),
            day,
            input: m.input_tokens,
            output: m.output_tokens,
            cache_creation: m.cache_creation,
            cache_read: m.cache_read,
            e1h: m.ephemeral_1h,
            e5m: m.ephemeral_5m,
        }
    }

    /// A later line or copy of a call: keep each field's largest value, as the
    /// parser does within a file.
    fn merge(&mut self, o: &Call) {
        self.input = self.input.max(o.input);
        self.output = self.output.max(o.output);
        self.cache_creation = self.cache_creation.max(o.cache_creation);
        self.cache_read = self.cache_read.max(o.cache_read);
        self.e1h = self.e1h.max(o.e1h);
        self.e5m = self.e5m.max(o.e5m);
    }
}

struct FileState {
    /// Device and inode: a different pair is a replaced file.
    id: (u64, u64),
    size: u64,
    mtime: i64,
    /// Bytes read so far, always at a line boundary.
    offset: u64,
    /// The file's first bytes, to tell an append from a rewrite of the same file.
    head: Vec<u8>,
    keys: HashSet<String>,
}

const HEAD: usize = 256;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct PollStats {
    pub files_seen: usize,
    /// Files opened this poll. An unchanged file is not opened.
    pub opened: usize,
    /// Files read again from the start (truncated, replaced or rewritten).
    pub reread: usize,
    pub bytes_read: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spend {
    pub day: NaiveDate,
    pub today: f64,
    pub month_to_date: f64,
}

#[derive(Default)]
pub struct Tally {
    files: HashMap<PathBuf, FileState>,
    /// Call key -> the call, and the files holding a copy of it.
    calls: HashMap<String, (Call, HashSet<PathBuf>)>,
}

fn file_id(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

fn mtime_of(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Tally {
    pub fn new() -> Tally {
        Tally::default()
    }

    fn forget_file(&mut self, path: &Path) {
        if let Some(st) = self.files.remove(path) {
            for k in st.keys {
                if let Some((_, holders)) = self.calls.get_mut(&k) {
                    holders.remove(path);
                    if holders.is_empty() {
                        self.calls.remove(&k);
                    }
                }
            }
        }
    }

    /// Read what changed under `root` since the last poll. Only files written this
    /// local month are looked at: an older file holds only older calls.
    pub fn poll(&mut self, root: &Path, clock: &dyn Clock) -> PollStats {
        let month_start = first_of_month(clock.today());
        let since = clock.start_of(month_start).timestamp();
        let mut stats = PollStats::default();
        let mut live: HashSet<PathBuf> = HashSet::new();
        for sf in scan_dir(root) {
            if sf.mtime < since {
                continue;
            }
            stats.files_seen += 1;
            live.insert(sf.path.clone());
            self.read_file(&sf.path, &sf.project, clock, &mut stats);
        }
        let gone: Vec<PathBuf> = self.files.keys().filter(|p| !live.contains(*p)).cloned().collect();
        for p in gone {
            self.forget_file(&p);
        }
        // Calls from before this month no longer count; keep the map small.
        self.calls.retain(|_, (c, _)| c.day.is_none_or(|d| d >= month_start));
        stats
    }

    fn read_file(&mut self, path: &Path, project: &str, clock: &dyn Clock, stats: &mut PollStats) {
        let Ok(meta) = std::fs::metadata(path) else { return };
        let (id, size, mtime) = (file_id(&meta), meta.len(), mtime_of(&meta));
        if let Some(st) = self.files.get(path) {
            if st.id == id && st.size == size && st.mtime == mtime {
                return; // unchanged: not opened
            }
        }
        let Ok(mut f) = std::fs::File::open(path) else { return };
        stats.opened += 1;
        let mut head = vec![0u8; HEAD.min(size as usize)];
        if f.read_exact(&mut head).is_err() {
            return;
        }
        // An append keeps the same file, at least as long, starting with the same
        // bytes. Anything else is read again from the start.
        let append = self.files.get(path).is_some_and(|st| {
            st.id == id && size >= st.offset && head.len() >= st.head.len() && head[..st.head.len()] == st.head[..]
        });
        let offset = if append {
            self.files[path].offset
        } else {
            if self.files.contains_key(path) {
                stats.reread += 1;
            }
            self.forget_file(path);
            self.files.insert(path.to_path_buf(), FileState { id, size: 0, mtime: 0, offset: 0, head: Vec::new(), keys: HashSet::new() });
            0
        };
        let mut buf = Vec::new();
        if f.seek(SeekFrom::Start(offset)).is_err() || f.read_to_end(&mut buf).is_err() {
            return;
        }
        stats.bytes_read += buf.len() as u64;
        // Whole lines only. A last line with no newline is taken when it already
        // parses (the writer finished it); otherwise it waits for the next poll.
        let mut end = buf.iter().rposition(|b| *b == b'\n').map(|i| i + 1).unwrap_or(0);
        let tail = std::str::from_utf8(&buf[end..]).unwrap_or("").trim();
        if !tail.is_empty() && serde_json::from_str::<serde_json::Value>(tail).is_ok() {
            end = buf.len();
        }
        let chunk = String::from_utf8_lossy(&buf[..end]);
        let parsed = parse_str(project, &chunk);
        let st_keys: Vec<String> = parsed
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| m.message_id.clone().unwrap_or_else(|| format!("{}#{}#{}", path.display(), offset, i)))
            .collect();
        for (m, key) in parsed.messages.iter().zip(st_keys.iter()) {
            let call = Call::of(m, clock);
            let entry = self.calls.entry(key.clone()).or_insert_with(|| (call.clone(), HashSet::new()));
            entry.0.merge(&call);
            entry.1.insert(path.to_path_buf());
        }
        let st = self.files.get_mut(path).expect("inserted above");
        st.keys.extend(st_keys);
        st.offset = offset + end as u64;
        st.id = id;
        st.size = size;
        st.mtime = mtime;
        if st.head.len() < HEAD {
            st.head = head;
        }
    }

    /// Today's and this month's cost, priced the way the day view prices a day.
    pub fn spend(&self, prices: &GaugePrices, clock: &dyn Clock) -> Spend {
        let today = clock.today();
        let month_start = first_of_month(today);
        let mut sums: BTreeMap<(NaiveDate, String), TokenBreakdown> = BTreeMap::new();
        for (c, _) in self.calls.values() {
            let Some(day) = c.day else { continue };
            if day < month_start || day > today {
                continue;
            }
            let key = model_key(&prices.table, c);
            let tb = sums.entry((day, key)).or_default();
            tb.input += c.input;
            tb.output += c.output;
            tb.cache_read += c.cache_read;
            tb.cache_creation_1h += c.e1h;
            tb.cache_creation_5m += c.e5m;
        }
        let mut spend = Spend { day: today, today: 0.0, month_to_date: 0.0 };
        for ((day, key), tb) in &sums {
            let cost = prices.cost(key, tb);
            spend.month_to_date += cost;
            if *day == today {
                spend.today += cost;
            }
        }
        spend
    }
}

/// The rollup key `PriceTable::model_sql` gives a call: its model, marked long
/// when its prompt is over its row's size.
fn model_key(table: &PriceTable, c: &Call) -> String {
    let model = c.model.clone().unwrap_or_default();
    let prompt = c.input + c.cache_creation + c.cache_read;
    let long = table.models.iter().any(|(k, p)| {
        p.long_prompt.as_ref().is_some_and(|l| {
            let family = model == *k || model.starts_with(&format!("{k}-")) || model.starts_with(&format!("{k}:"));
            family && prompt > l.above_tokens
        })
    });
    if long { long_key(&model) } else { model }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::do_refresh;
    use crate::metrics::Filters;
    use crate::pricing::ManagedPricing;
    use crate::store::Store;
    use chrono::{Duration, FixedOffset};
    use std::fs;
    use std::io::Write;

    /// New York in 2026 without a time zone database: EDT (-4) until
    /// 2026-11-01 06:00 UTC, EST (-5) after; spring forward on 2026-03-08 07:00 UTC.
    struct NewYork2026 {
        today: NaiveDate,
    }
    impl NewYork2026 {
        fn offset(t: DateTime<Utc>) -> FixedOffset {
            let spring = Utc.with_ymd_and_hms(2026, 3, 8, 7, 0, 0).unwrap();
            let fall = Utc.with_ymd_and_hms(2026, 11, 1, 6, 0, 0).unwrap();
            let h = if t >= spring && t < fall { -4 } else { -5 };
            FixedOffset::east_opt(h * 3600).unwrap()
        }
    }
    impl Clock for NewYork2026 {
        fn today(&self) -> NaiveDate {
            self.today
        }
        fn date_of(&self, t: DateTime<Utc>) -> NaiveDate {
            t.with_timezone(&NewYork2026::offset(t)).date_naive()
        }
        fn start_of(&self, d: NaiveDate) -> DateTime<Utc> {
            // Midnight exists on every 2026 New York day: the offset that is in force
            // at the instant it names is the right one.
            [-4, -5]
                .iter()
                .map(|h| FixedOffset::east_opt(h * 3600).unwrap())
                .map(|o| o.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).unwrap().with_timezone(&Utc))
                .find(|t| self.date_of(*t) == d && self.date_of(*t - Duration::seconds(1)) < d)
                .unwrap()
        }
    }

    /// UTC as the local zone: the gauge's days are then the day view's days.
    struct UtcClock(NaiveDate);
    impl Clock for UtcClock {
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

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn call(id: &str, model: &str, ts: &str, input: i64, output: i64) -> String {
        serde_json::json!({"type":"assistant","sessionId":"s","timestamp":ts,
            "message":{"id":id,"role":"assistant","model":model,
                "usage":{"input_tokens":input,"output_tokens":output,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}})
        .to_string()
            + "\n"
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("al_gauge_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("-Users-me-proj")).unwrap();
        d
    }

    fn list() -> GaugePrices {
        GaugePrices { table: PriceTable::seed(), managed: None }
    }

    // An Opus 4.8 call of 1M input tokens costs $5; 1M output, $25.
    const M: i64 = 1_000_000;

    #[test]
    fn appending_to_a_transcript_adds_only_the_new_cost() {
        let root = dir("append");
        let f = root.join("-Users-me-proj").join("a.jsonl");
        fs::write(&f, call("m1", "claude-opus-4-8", "2026-10-15T12:00:00.000Z", M, 0)).unwrap();
        let clock = UtcClock(day(2026, 10, 15));
        let mut t = Tally::new();
        let s1 = t.poll(&root, &clock);
        assert_eq!((s1.opened, s1.reread), (1, 0));
        assert!((t.spend(&list(), &clock).today - 5.0).abs() < 1e-9);

        let before = fs::metadata(&f).unwrap().len();
        let mut w = fs::OpenOptions::new().append(true).open(&f).unwrap();
        w.write_all(call("m2", "claude-opus-4-8", "2026-10-15T13:00:00.000Z", 0, M).as_bytes()).unwrap();
        // A second line of m1 repeats its usage: it must not count twice.
        w.write_all(call("m1", "claude-opus-4-8", "2026-10-15T12:00:01.000Z", M, 0).as_bytes()).unwrap();
        drop(w);
        let s2 = t.poll(&root, &clock);
        assert_eq!((s2.opened, s2.reread), (1, 0));
        assert_eq!(s2.bytes_read, fs::metadata(&f).unwrap().len() - before, "only the appended bytes are read");
        assert!((t.spend(&list(), &clock).today - 30.0).abs() < 1e-9);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unchanged_file_is_not_reopened() {
        let root = dir("unchanged");
        fs::write(root.join("-Users-me-proj").join("a.jsonl"), call("m1", "claude-opus-4-8", "2026-10-15T12:00:00.000Z", M, 0)).unwrap();
        let clock = UtcClock(day(2026, 10, 15));
        let mut t = Tally::new();
        assert_eq!(t.poll(&root, &clock).opened, 1);
        let again = t.poll(&root, &clock);
        assert_eq!((again.files_seen, again.opened, again.bytes_read), (1, 0, 0));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_truncated_or_replaced_file_is_read_again_in_full() {
        let root = dir("reset");
        let f = root.join("-Users-me-proj").join("a.jsonl");
        let both = call("m1", "claude-opus-4-8", "2026-10-15T12:00:00.000Z", M, 0) + &call("m2", "claude-opus-4-8", "2026-10-15T12:05:00.000Z", M, 0);
        fs::write(&f, &both).unwrap();
        let clock = UtcClock(day(2026, 10, 15));
        let mut t = Tally::new();
        t.poll(&root, &clock);
        assert!((t.spend(&list(), &clock).today - 10.0).abs() < 1e-9);

        // Truncated to its first call.
        fs::write(&f, call("m1", "claude-opus-4-8", "2026-10-15T12:00:00.000Z", M, 0)).unwrap();
        let s = t.poll(&root, &clock);
        assert_eq!(s.reread, 1);
        assert!((t.spend(&list(), &clock).today - 5.0).abs() < 1e-9, "m2 is gone with its bytes");

        // Replaced by a new file under the same name (a new inode), longer than before.
        let tmp = root.join("-Users-me-proj").join("a.tmp");
        fs::write(&tmp, call("m9", "claude-opus-4-8", "2026-10-15T14:00:00.000Z", 0, M) + &call("m8", "claude-opus-4-8", "2026-10-15T14:01:00.000Z", 0, M) + &"\n".repeat(400)).unwrap();
        fs::rename(&tmp, &f).unwrap();
        let s = t.poll(&root, &clock);
        assert_eq!(s.reread, 1);
        assert!((t.spend(&list(), &clock).today - 50.0).abs() < 1e-9, "only the new file's two calls");

        // Rewritten in place, same inode, longer, different start.
        let mut w = fs::OpenOptions::new().write(true).truncate(true).open(&f).unwrap();
        w.write_all((call("m7", "claude-opus-4-8", "2026-10-15T15:00:00.000Z", M, 0) + &"\n".repeat(900)).as_bytes()).unwrap();
        drop(w);
        let s = t.poll(&root, &clock);
        assert_eq!(s.reread, 1);
        assert!((t.spend(&list(), &clock).today - 5.0).abs() < 1e-9);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_half_written_last_line_waits_and_a_finished_one_counts() {
        let root = dir("partial");
        let f = root.join("-Users-me-proj").join("a.jsonl");
        let line = call("m1", "claude-opus-4-8", "2026-10-15T12:00:00.000Z", M, 0);
        fs::write(&f, &line[..line.len() / 2]).unwrap();
        let clock = UtcClock(day(2026, 10, 15));
        let mut t = Tally::new();
        t.poll(&root, &clock);
        assert_eq!(t.spend(&list(), &clock).today, 0.0);
        let mut w = fs::OpenOptions::new().append(true).open(&f).unwrap();
        w.write_all(line[line.len() / 2..].trim_end().as_bytes()).unwrap(); // finished, no newline yet
        drop(w);
        t.poll(&root, &clock);
        assert!((t.spend(&list(), &clock).today - 5.0).abs() < 1e-9);
        let mut w = fs::OpenOptions::new().append(true).open(&f).unwrap();
        w.write_all(b"\n").unwrap();
        w.write_all(call("m2", "claude-opus-4-8", "2026-10-15T12:01:00.000Z", M, 0).as_bytes()).unwrap();
        drop(w);
        t.poll(&root, &clock);
        assert!((t.spend(&list(), &clock).today - 10.0).abs() < 1e-9, "m1 not counted twice");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_call_copied_into_a_second_file_counts_once_until_both_copies_go() {
        let root = dir("copies");
        let a = root.join("-Users-me-proj").join("a.jsonl");
        let b = root.join("-Users-me-proj").join("b.jsonl");
        let line = call("m1", "claude-opus-4-8", "2026-10-15T12:00:00.000Z", M, 0);
        fs::write(&a, &line).unwrap();
        fs::write(&b, &line).unwrap();
        let clock = UtcClock(day(2026, 10, 15));
        let mut t = Tally::new();
        t.poll(&root, &clock);
        assert!((t.spend(&list(), &clock).today - 5.0).abs() < 1e-9);
        fs::remove_file(&a).unwrap();
        t.poll(&root, &clock);
        assert!((t.spend(&list(), &clock).today - 5.0).abs() < 1e-9, "b still holds it");
        fs::remove_file(&b).unwrap();
        t.poll(&root, &clock);
        assert_eq!(t.spend(&list(), &clock).today, 0.0);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn days_are_local_dates_and_a_daylight_saving_day_rolls_over_at_local_midnight() {
        let root = dir("dst");
        let f = root.join("-Users-me-proj").join("a.jsonl");
        // New York, the 25 hour 2026-11-01.
        let lines = [
            ("m1", "2026-11-01T03:59:59.000Z"), // 23:59:59 EDT on Oct 31
            ("m2", "2026-11-01T04:00:00.000Z"), // 00:00 EDT on Nov 1
            ("m3", "2026-11-02T04:59:59.000Z"), // 23:59:59 EST on Nov 1
            ("m4", "2026-11-02T05:00:00.000Z"), // 00:00 EST on Nov 2
        ];
        fs::write(&f, lines.iter().map(|(id, ts)| call(id, "claude-opus-4-8", ts, M, 0)).collect::<String>()).unwrap();
        // Written on Nov 2, as the clock has it; only files written this month are read.
        let written = std::time::UNIX_EPOCH + std::time::Duration::from_secs(Utc.with_ymd_and_hms(2026, 11, 2, 5, 0, 1).unwrap().timestamp() as u64);
        fs::File::options().write(true).open(&f).unwrap().set_modified(written).unwrap();
        let mut t = Tally::new();
        let nov1 = NewYork2026 { today: day(2026, 11, 1) };
        t.poll(&root, &nov1);
        let s = t.spend(&list(), &nov1);
        assert!((s.today - 10.0).abs() < 1e-9, "m2 and m3: {}", s.today);
        assert!((s.month_to_date - 10.0).abs() < 1e-9, "m1 is October; m4 is tomorrow");
        let nov2 = NewYork2026 { today: day(2026, 11, 2) };
        let s = t.spend(&list(), &nov2);
        assert!((s.today - 5.0).abs() < 1e-9);
        assert!((s.month_to_date - 15.0).abs() < 1e-9);
        assert_eq!(nov1.start_of(day(2026, 11, 1)), Utc.with_ymd_and_hms(2026, 11, 1, 4, 0, 0).unwrap());
        assert_eq!(nov1.start_of(day(2026, 11, 2)), Utc.with_ymd_and_hms(2026, 11, 2, 5, 0, 0).unwrap());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_files_written_this_month_are_read_and_older_calls_do_not_count() {
        let root = dir("month");
        let f = root.join("-Users-me-proj").join("old.jsonl");
        fs::write(&f, call("m1", "claude-opus-4-8", "2026-09-30T12:00:00.000Z", M, 0)).unwrap();
        let clock = UtcClock(day(2026, 10, 15));
        let mut t = Tally::new();
        let s = t.poll(&root, &clock); // the file's mtime is now, so it is read
        assert_eq!(s.opened, 1);
        assert_eq!(t.spend(&list(), &clock).month_to_date, 0.0, "a September call is not October's");
        assert!(t.calls.is_empty(), "and is not kept");
        // A file last written before this month is skipped without being opened.
        let future = UtcClock(day(2099, 1, 1));
        let s = t.poll(&root, &future);
        assert_eq!((s.files_seen, s.opened), (0, 0));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn todays_figure_is_the_day_views_figure_to_the_cent() {
        let root = dir("parity");
        let proj = root.join("-Users-me-proj");
        let mut a = String::new();
        let mut copy = String::new();
        // Mixed models, cache reads and writes, a fast call, a long Haiku 5.5 prompt,
        // a call written over two lines, and one copied into a second file.
        a += &serde_json::json!({"type":"user","sessionId":"s","timestamp":"2026-10-15T09:59:00.000Z","uuid":"u1","message":{"role":"user","content":"go"}}).to_string();
        a += "\n";
        for (id, model, ts, i, o, cr, w1, w5, fast) in [
            ("m1", "claude-opus-4-8", "2026-10-15T10:00:00.000Z", 1234, 567, 89_000, 0, 3_000, false),
            ("m2", "claude-sonnet-4-6-20260101", "2026-10-15T11:00:00.000Z", 50, 9_999, 120_000, 4_000, 0, false),
            ("m3", "claude-opus-4-8", "2026-10-15T12:00:00.000Z", 10, 2_000, 0, 0, 0, true),
            ("m4", "claude-haiku-5-5", "2026-10-15T13:00:00.000Z", 150_000, 800, 0, 0, 0, false),
            ("m5", "claude-haiku-5-5", "2026-10-15T14:00:00.000Z", 9_000, 800, 0, 0, 0, false),
            ("m6", "claude-opus-5-5", "2026-10-14T23:00:00.000Z", 777, 777, 0, 0, 0, false),
            // An evening call in New York: Oct 15 there, Oct 16 in UTC.
            ("m7", "claude-opus-5-5", "2026-10-16T02:30:00.000Z", 100_000, 0, 0, 0, 0, false),
        ] {
            let mut usage = serde_json::json!({"input_tokens":i,"output_tokens":o,"cache_read_input_tokens":cr,
                "cache_creation_input_tokens":w1 + w5,"cache_creation":{"ephemeral_1h_input_tokens":w1,"ephemeral_5m_input_tokens":w5}});
            if fast {
                usage["speed"] = "fast".into();
            }
            let line = serde_json::json!({"type":"assistant","sessionId":"s","timestamp":ts,
                "message":{"id":id,"role":"assistant","model":model,"usage":usage}}).to_string() + "\n";
            a += &line;
            if id == "m1" {
                a += &line; // the same call's second content block
            }
            if id == "m2" {
                copy = line.clone(); // history copied into a resumed session
            }
        }
        fs::write(proj.join("a.jsonl"), &a).unwrap();
        fs::write(proj.join("b.jsonl"), &copy).unwrap();

        let db = root.join("cache.sqlite");
        let mut store = Store::open(&db).unwrap();
        do_refresh(&mut store, &root).unwrap();
        let table = PriceTable::seed();
        // The day view and the gauge count the same local days, wherever the Mac is.
        for d in ["2026-10-15", "2026-10-14"] {
            let date = NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap();
            let clocks: [&dyn Clock; 2] = [&UtcClock(date), &NewYork2026 { today: date }];
            for clock in clocks {
                let f = Filters { from: Some(d.into()), to: Some(d.into()), ..Default::default() };
                let day_view: f64 = store.by_day_with(&table, &f, clock).unwrap().iter().map(|r| r.cost_usd).sum();
                let mut t = Tally::new();
                t.poll(&root, clock);
                let gauge = t.spend(&GaugePrices { table: table.clone(), managed: None }, clock).today;
                assert!(day_view > 0.0);
                assert_eq!((gauge * 100.0).round(), (day_view * 100.0).round(), "{d}: gauge {gauge} vs day view {day_view}");
                assert!((gauge - day_view).abs() < 1e-9, "{d}: {gauge} vs {day_view}");
            }
        }
        // m7 is on Oct 15 in New York and on Oct 16 in UTC.
        let oct = |d: &str, clock: &dyn Clock| -> f64 {
            let f = Filters { from: Some(d.into()), to: Some(d.into()), ..Default::default() };
            store.by_day_with(&table, &f, clock).unwrap().iter().map(|r| r.cost_usd).sum()
        };
        let ny = NewYork2026 { today: day(2026, 10, 15) };
        let utc = UtcClock(day(2026, 10, 15));
        let m7 = oct("2026-10-16", &utc);
        assert!(m7 > 0.0);
        assert!((oct("2026-10-15", &ny) - oct("2026-10-15", &utc) - m7).abs() < 1e-9, "m7 is on New York's Oct 15");
        assert_eq!(oct("2026-10-16", &ny), 0.0);
        // The same local day, asked of the store by instants, is the same figure.
        let clock = NewYork2026 { today: day(2026, 10, 15) };
        let f = Filters {
            from_ts: Some(iso(clock.start_of(day(2026, 10, 15)))),
            to_ts: Some(iso(clock.start_of(day(2026, 10, 16)))),
            ..Default::default()
        };
        let by_instant: f64 = store.by_day(&table, &f).unwrap().iter().map(|r| r.cost_usd).sum();
        let mut t = Tally::new();
        t.poll(&root, &clock);
        let gauge = t.spend(&GaugePrices { table: table.clone(), managed: None }, &clock).today;
        assert!((gauge - by_instant).abs() < 1e-9, "{gauge} vs {by_instant}");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn configured_rates_price_the_same_calls() {
        let root = dir("managed");
        fs::write(root.join("-Users-me-proj").join("a.jsonl"), call("m1", "claude-opus-4-8", "2026-10-15T12:00:00.000Z", M, M)).unwrap();
        let clock = UtcClock(day(2026, 10, 15));
        let mut t = Tally::new();
        t.poll(&root, &clock);
        let managed = ManagedPricing::from_settings_json(
            r#"{"modelPricing":{"multiplier":0.5,"overrides":{"claude-opus-4-8":{"input":2,"output":10,"cacheRead":0.2,"cacheWrite":3}}}}"#,
        );
        let p = GaugePrices { table: PriceTable::seed(), managed };
        assert!((t.spend(&p, &clock).today - 6.0).abs() < 1e-9, "(2 + 10) * 0.5");
        fs::remove_dir_all(&root).ok();
    }
}
