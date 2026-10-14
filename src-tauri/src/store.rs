use crate::models::ParsedFile;
use crate::scanner::ScannedFile;
use rusqlite::{params, Connection};
use std::path::Path;

pub struct Store {
    pub conn: Connection,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    parsed_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file TEXT NOT NULL,
    session_id TEXT NOT NULL,
    project TEXT NOT NULL,
    ts TEXT NOT NULL,
    role TEXT,
    model TEXT,
    version TEXT,
    git_branch TEXT,
    service_tier TEXT,
    stop_reason TEXT,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cache_creation INTEGER NOT NULL,
    cache_read INTEGER NOT NULL,
    ephemeral_1h INTEGER NOT NULL,
    ephemeral_5m INTEGER NOT NULL,
    message_id TEXT,
    agent_id TEXT,
    web_searches INTEGER NOT NULL DEFAULT 0,
    dup INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_messages_file ON messages(file);
CREATE INDEX IF NOT EXISTS idx_messages_message_id ON messages(message_id);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
CREATE INDEX IF NOT EXISTS idx_messages_project ON messages(project);
CREATE INDEX IF NOT EXISTS idx_messages_ts ON messages(ts);
CREATE TABLE IF NOT EXISTS tool_calls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file TEXT NOT NULL,
    session_id TEXT NOT NULL,
    project TEXT NOT NULL,
    ts TEXT NOT NULL,
    name TEXT NOT NULL,
    is_mcp INTEGER NOT NULL,
    user_modified INTEGER,
    tool_use_id TEXT,
    tag TEXT,
    fail_count INTEGER,
    test_key TEXT,
    dup INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_tool_calls_tool_use_id ON tool_calls(tool_use_id);
CREATE INDEX IF NOT EXISTS idx_tool_calls_project ON tool_calls(project);
CREATE INDEX IF NOT EXISTS idx_tool_calls_file ON tool_calls(file);
CREATE TABLE IF NOT EXISTS prompts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file TEXT NOT NULL,
    session_id TEXT NOT NULL,
    project TEXT NOT NULL,
    agent_id TEXT,
    ts TEXT NOT NULL,
    kind TEXT NOT NULL,
    dup INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_prompts_file ON prompts(file);
CREATE TABLE IF NOT EXISTS subagents (
    agent_id TEXT PRIMARY KEY,
    file TEXT NOT NULL,
    session_id TEXT,
    parent_tool_use_id TEXT,
    agent_type TEXT
);
CREATE TABLE IF NOT EXISTS segment_model_labels (
    segment_key TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt_version TEXT NOT NULL,
    label TEXT NOT NULL,
    PRIMARY KEY (segment_key, model, prompt_version)
);
CREATE TABLE IF NOT EXISTS cost_states (
    file TEXT NOT NULL,
    session_id TEXT NOT NULL,
    total_cost_usd REAL NOT NULL,
    from_ts TEXT NOT NULL,
    to_ts TEXT NOT NULL,
    PRIMARY KEY (file, session_id)
);
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT,
    message_count INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS session_titles (
    session_id TEXT PRIMARY KEY,
    title TEXT
);
"#;

/// Bump when a stored table changes shape. An older cache is dropped and every
/// transcript re-read (the cache is derived data; the transcripts are the truth).
const SCHEMA_VERSION: &str = "9";
const DATA_TABLES: &str = "DROP TABLE IF EXISTS messages; DROP TABLE IF EXISTS tool_calls;
    DROP TABLE IF EXISTS sessions; DROP TABLE IF EXISTS session_titles; DROP TABLE IF EXISTS files;
    DROP TABLE IF EXISTS prompts; DROP TABLE IF EXISTS subagents; DROP TABLE IF EXISTS cost_states;
    DROP TABLE IF EXISTS segment_model_labels;";

impl Store {
    fn init(conn: Connection) -> rusqlite::Result<Store> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);")?;
        let version: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
            .ok();
        if version.as_deref() != Some(SCHEMA_VERSION) {
            conn.execute_batch(DATA_TABLES)?;
            conn.execute(
                "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![SCHEMA_VERSION],
            )?;
        }
        conn.execute_batch(SCHEMA)?;
        let store = Store { conn };
        store.ensure_meta("machine_id")?;
        store.ensure_meta("export_salt")?;
        Ok(store)
    }

    pub fn open_in_memory() -> Store {
        Self::init(Connection::open_in_memory().unwrap()).unwrap()
    }

    pub fn open(path: &Path) -> rusqlite::Result<Store> {
        Self::init(Connection::open(path)?)
    }

    fn ensure_meta(&self, key: &str) -> rusqlite::Result<()> {
        let exists: bool = self
            .conn
            .query_row("SELECT 1 FROM meta WHERE key = ?1", params![key], |_| Ok(true))
            .unwrap_or(false);
        if !exists {
            let val = uuid::Uuid::new_v4().to_string();
            self.conn
                .execute("INSERT INTO meta(key, value) VALUES (?1, ?2)", params![key, val])?;
        }
        Ok(())
    }

    fn meta(&self, key: &str) -> String {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0))
            .unwrap_or_default()
    }

    pub fn machine_id(&self) -> String {
        self.meta("machine_id")
    }

    pub fn export_salt(&self) -> String {
        self.meta("export_salt")
    }

    pub fn needs_parse(&self, path: &str, size: i64, mtime: i64) -> bool {
        let row: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT size, mtime FROM files WHERE path = ?1",
                params![path],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        match row {
            Some((s, m)) => s != size || m != mtime,
            None => true,
        }
    }

    pub fn replace_file(&mut self, sf: &ScannedFile, parsed: &ParsedFile) -> rusqlite::Result<()> {
        let path = sf.path.to_string_lossy().to_string();
        let tx = self.conn.transaction()?;
        // Delete this file's prior rows by FILE, not by session: a sub-agent's
        // transcript shares its parent's sessionId, so deleting by session would
        // wipe the other file's rows.
        for table in ["messages", "tool_calls", "prompts", "subagents", "cost_states"] {
            tx.execute(&format!("DELETE FROM {table} WHERE file = ?1"), params![path])?;
        }
        for m in &parsed.messages {
            tx.execute(
                "INSERT INTO messages(file, session_id, project, ts, role, model, version, git_branch, service_tier, stop_reason, input_tokens, output_tokens, cache_creation, cache_read, ephemeral_1h, ephemeral_5m, message_id, agent_id, web_searches)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
                params![
                    path, m.session_id, sf.project, m.ts, m.role, m.model, m.version, m.git_branch,
                    m.service_tier, m.stop_reason, m.input_tokens, m.output_tokens,
                    m.cache_creation, m.cache_read, m.ephemeral_1h, m.ephemeral_5m,
                    m.message_id, m.agent_id, m.web_searches
                ],
            )?;
        }
        for t in &parsed.tool_calls {
            tx.execute(
                "INSERT INTO tool_calls(file, session_id, project, ts, name, is_mcp, user_modified, tool_use_id, tag, fail_count, test_key)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![path, t.session_id, sf.project, t.ts, t.name, t.is_mcp as i64, t.user_modified, t.tool_use_id, t.tag, t.fail_count, t.test_key],
            )?;
        }
        for p in &parsed.prompts {
            tx.execute(
                "INSERT INTO prompts(file, session_id, project, agent_id, ts, kind)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![path, p.session_id, sf.project, p.agent_id, p.ts, p.kind],
            )?;
        }
        if let Some(meta) = &sf.subagent {
            let session = parsed.messages.first().map(|m| m.session_id.clone());
            tx.execute(
                "INSERT INTO subagents(agent_id, file, session_id, parent_tool_use_id, agent_type) VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(agent_id) DO UPDATE SET file=excluded.file, session_id=excluded.session_id,
                   parent_tool_use_id=excluded.parent_tool_use_id, agent_type=excluded.agent_type",
                params![meta.agent_id, path, session, meta.parent_tool_use_id, meta.agent_type],
            )?;
        }
        for c in &parsed.cost_states {
            tx.execute(
                "INSERT INTO cost_states(file, session_id, total_cost_usd, from_ts, to_ts) VALUES (?1,?2,?3,?4,?5)",
                params![path, c.session_id, c.total_usd, c.from_ts, c.to_ts],
            )?;
        }
        for (sid, title) in &parsed.titles {
            tx.execute(
                "INSERT INTO session_titles(session_id, title) VALUES (?1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET title=excluded.title",
                params![sid, title],
            )?;
        }
        tx.execute(
            "INSERT INTO files(path, size, mtime, parsed_at) VALUES (?1,?2,?3, strftime('%s','now'))
             ON CONFLICT(path) DO UPDATE SET size=excluded.size, mtime=excluded.mtime, parsed_at=excluded.parsed_at",
            params![path, sf.size, sf.mtime],
        )?;
        tx.commit()
    }

    /// The same record can sit in several files (a session file present under two
    /// project folders; history copied into a resumed session). Keep one copy live
    /// and mark the rest `dup = 1`; every query reads `dup = 0`. The copy kept is in
    /// the most recently written file (a stale copy must not take the project),
    /// then the first by path, so it stays the same after a re-read. Re-run after
    /// each refresh.
    pub fn mark_duplicates(&self) -> rusqlite::Result<()> {
        let mark = |table: &str, key: &str| {
            format!(
                "UPDATE {table} SET dup = 0;
                 UPDATE {table} SET dup = 1 WHERE id IN (
                   SELECT id FROM (
                     SELECT t.id, ROW_NUMBER() OVER (
                       PARTITION BY t.{key} ORDER BY COALESCE(f.mtime, 0) DESC, t.file, t.id) AS rank
                     FROM {table} t LEFT JOIN files f ON f.path = t.file
                     WHERE t.{key} IS NOT NULL)
                   WHERE rank > 1);"
            )
        };
        self.conn.execute_batch(&(mark("messages", "message_id") + &mark("tool_calls", "tool_use_id")))
    }

    pub fn rebuild_sessions(&self) -> rusqlite::Result<()> {
        self.mark_duplicates()?;
        self.conn.execute("DELETE FROM sessions", [])?;
        self.conn.execute(
            "INSERT INTO sessions(session_id, project, started_at, ended_at, message_count)
             SELECT session_id, MIN(project), MIN(ts), MAX(ts), COUNT(*)
             FROM messages WHERE dup = 0 GROUP BY session_id",
            [],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_str;
    use crate::scanner::SubagentMeta;

    fn seed() -> Store {
        let mut store = Store::open_in_memory();
        let content = include_str!("../tests/fixtures/sample.jsonl");
        let parsed = parse_str("proj-a", content);
        let sf = ScannedFile {
            path: std::path::PathBuf::from("/x/proj-a/s1.jsonl"),
            project: "proj-a".into(),
            size: 123,
            mtime: 456,
            subagent: None,
        };
        store.replace_file(&sf, &parsed).unwrap();
        store.rebuild_sessions().unwrap();
        store
    }

    #[test]
    fn inserts_messages_and_tool_calls() {
        let store = seed();
        let mc: i64 = store.conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
        let tc: i64 = store.conn.query_row("SELECT COUNT(*) FROM tool_calls", [], |r| r.get(0)).unwrap();
        assert_eq!(mc, 3);
        assert_eq!(tc, 2);
    }

    #[test]
    fn ledger_tracks_changes() {
        let store = seed();
        assert!(!store.needs_parse("/x/proj-a/s1.jsonl", 123, 456));
        assert!(store.needs_parse("/x/proj-a/s1.jsonl", 999, 456));
        assert!(store.needs_parse("/new/file.jsonl", 1, 1));
    }

    #[test]
    fn rebuild_sessions_aggregates() {
        let store = seed();
        let count: i64 = store.conn.query_row("SELECT message_count FROM sessions WHERE session_id='s1'", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn reparse_replaces_not_duplicates() {
        let mut store = seed();
        let content = include_str!("../tests/fixtures/sample.jsonl");
        let parsed = parse_str("proj-a", content);
        let sf = ScannedFile { path: "/x/proj-a/s1.jsonl".into(), project: "proj-a".into(), size: 999, mtime: 456, subagent: None };
        store.replace_file(&sf, &parsed).unwrap();
        let mc: i64 = store.conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
        assert_eq!(mc, 3, "reparsing the same session must replace, not duplicate");
    }

    #[test]
    fn meta_ids_are_stable_and_nonempty() {
        let store = Store::open_in_memory();
        assert!(!store.machine_id().is_empty());
        assert_eq!(store.machine_id(), store.machine_id());
        assert_ne!(store.machine_id(), store.export_salt());
    }

    fn sub_file(path: &str, sub: Option<SubagentMeta>) -> ScannedFile {
        ScannedFile { path: path.into(), project: "proj-a".into(), size: 1, mtime: 1, subagent: sub }
    }

    #[test]
    fn reparsing_a_subagent_file_keeps_its_parents_rows() {
        let mut store = Store::open_in_memory();
        let parent = parse_str("proj-a", include_str!("../tests/fixtures/sample.jsonl"));
        // A sub-agent's records share the parent's sessionId.
        let child = parse_str("proj-a", r#"{"type":"assistant","sessionId":"s1","agentId":"a1","isSidechain":true,"timestamp":"2026-07-24T18:11:30.000Z","message":{"id":"m_sub","role":"assistant","model":"claude-haiku-4-5","usage":{"input_tokens":7,"output_tokens":3}}}"#);
        let meta = SubagentMeta { agent_id: "a1".into(), parent_tool_use_id: Some("tu_1".into()), agent_type: Some("Explore".into()) };
        store.replace_file(&sub_file("/x/proj-a/s1.jsonl", None), &parent).unwrap();
        store.replace_file(&sub_file("/x/proj-a/s1/subagents/agent-a1.jsonl", Some(meta.clone())), &child).unwrap();
        store.replace_file(&sub_file("/x/proj-a/s1/subagents/agent-a1.jsonl", Some(meta)), &child).unwrap();
        let mc: i64 = store.conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
        assert_eq!(mc, 4, "3 parent rows + 1 sub-agent row, after re-parsing the child");
        store.replace_file(&sub_file("/x/proj-a/s1.jsonl", None), &parent).unwrap();
        let mc: i64 = store.conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
        assert_eq!(mc, 4, "re-parsing the parent keeps the child");
        let link: (String, String) = store.conn
            .query_row("SELECT parent_tool_use_id, agent_type FROM subagents WHERE agent_id='a1'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(link, ("tu_1".to_string(), "Explore".to_string()));
    }

    #[test]
    fn prompts_tags_and_cost_state_are_stored() {
        let mut store = Store::open_in_memory();
        let parsed = parse_str("proj-a", include_str!("../tests/fixtures/activity.jsonl"));
        store.replace_file(&sub_file("/x/proj-a/sa.jsonl", None), &parsed).unwrap();
        let prompts: i64 = store.conn.query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0)).unwrap();
        assert_eq!(prompts, 7);
        let tests: i64 = store.conn.query_row("SELECT COUNT(*) FROM tool_calls WHERE tag='test'", [], |r| r.get(0)).unwrap();
        assert_eq!(tests, 2);
        let cost: f64 = store.conn.query_row("SELECT total_cost_usd FROM cost_states WHERE session_id='sa'", [], |r| r.get(0)).unwrap();
        assert!((cost - 0.75).abs() < 1e-9);
        let calls: i64 = store.conn.query_row("SELECT COUNT(*) FROM messages WHERE message_id IS NOT NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(calls, 2);
    }

    #[test]
    fn an_older_database_is_rebuilt_from_scratch_and_keeps_its_machine_id() {
        let path = std::env::temp_dir().join(format!("ci_old_{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta VALUES ('machine_id','m-1');
                 CREATE TABLE files (path TEXT PRIMARY KEY, size INTEGER NOT NULL, mtime INTEGER NOT NULL, parsed_at INTEGER NOT NULL);
                 INSERT INTO files VALUES ('/x/a.jsonl', 1, 1, 1);
                 CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT NOT NULL);
                 INSERT INTO messages(session_id) VALUES ('old');",
            ).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.machine_id(), "m-1");
        assert!(store.needs_parse("/x/a.jsonl", 1, 1), "the ledger is cleared so every file is re-read");
        let mc: i64 = store.conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
        assert_eq!(mc, 0);
        drop(store);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_call_copied_into_a_resumed_sessions_file_counts_once() {
        let mut store = Store::open_in_memory();
        let original = parse_str("proj-a", include_str!("../tests/fixtures/activity.jsonl"));
        // Resuming copies the history into a new file; the copies keep their ids.
        store.replace_file(&sub_file("/x/proj-a/sa.jsonl", None), &original).unwrap();
        store.replace_file(&sub_file("/x/proj-a/sa-resumed.jsonl", None), &original).unwrap();
        store.rebuild_sessions().unwrap();
        let price = crate::pricing::PriceTable::seed();
        let t = store.totals(&price, &crate::metrics::Filters::default()).unwrap();
        assert_eq!(t.output_tokens, 120, "each call once, not once per file");
        let tools: i64 = store.by_tool(&crate::metrics::Filters::default()).unwrap().iter().map(|x| x.count).sum();
        assert_eq!(tools, 5);
        // Re-reading the first file must leave exactly one live copy.
        store.replace_file(&sub_file("/x/proj-a/sa.jsonl", None), &original).unwrap();
        store.rebuild_sessions().unwrap();
        assert_eq!(store.totals(&price, &crate::metrics::Filters::default()).unwrap().output_tokens, 120);
        let detail = store.get_session_detail(&price, "sa").unwrap();
        assert_eq!(detail.tokens.output, 120);
    }

    #[test]
    fn the_live_copy_does_not_depend_on_which_file_was_read_last() {
        let mut store = Store::open_in_memory();
        let parsed = parse_str("proj-a", include_str!("../tests/fixtures/activity.jsonl"));
        let a = ScannedFile { path: "/x/a/sa.jsonl".into(), project: "a".into(), size: 1, mtime: 1, subagent: None };
        let b = ScannedFile { path: "/x/b/sa.jsonl".into(), project: "b".into(), size: 1, mtime: 1, subagent: None };
        let live = |store: &Store| -> String {
            store.conn.query_row("SELECT DISTINCT project FROM messages WHERE dup = 0 AND message_id = 'msg_1'", [], |r| r.get(0)).unwrap()
        };
        store.replace_file(&a, &parsed).unwrap();
        store.replace_file(&b, &parsed).unwrap();
        store.rebuild_sessions().unwrap();
        assert_eq!(live(&store), "a");
        store.replace_file(&a, &parsed).unwrap(); // re-read: a's rows now have higher ids
        store.rebuild_sessions().unwrap();
        assert_eq!(live(&store), "a", "the copy kept is chosen by file, not by insert order");
    }

    #[test]
    fn the_newest_copy_is_kept_so_a_stale_copy_does_not_take_the_project() {
        let mut store = Store::open_in_memory();
        let parsed = parse_str("proj-a", include_str!("../tests/fixtures/activity.jsonl"));
        // An old copy under a "-test" folder sorts first by path ('-' < '/') but
        // was last written long ago; the live file keeps growing.
        let stale = ScannedFile { path: "/x/-Users-me-app-test/sa.jsonl".into(), project: "-Users-me-app-test".into(), size: 1, mtime: 100, subagent: None };
        let live = ScannedFile { path: "/x/-Users-me-app/sa.jsonl".into(), project: "-Users-me-app".into(), size: 2, mtime: 200, subagent: None };
        store.replace_file(&stale, &parsed).unwrap();
        store.replace_file(&live, &parsed).unwrap();
        store.rebuild_sessions().unwrap();
        let project: String = store.conn
            .query_row("SELECT DISTINCT project FROM messages WHERE dup = 0 AND message_id = 'msg_1'", [], |r| r.get(0)).unwrap();
        assert_eq!(project, "-Users-me-app");
    }

    #[test]
    fn persists_session_title() {
        let store = seed();
        let title: String = store
            .conn
            .query_row("SELECT title FROM session_titles WHERE session_id='s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "Refactor the parser");
    }
}
