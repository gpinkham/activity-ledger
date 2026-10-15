use crate::models::{CostStateRecord, MessageRecord, ParsedFile, PromptRecord, RawUsage, ToolCallRecord};
use crate::pricing::fast_key;
use crate::triggers::Matcher;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// Parse one file's text into normalized records. Tolerant: malformed lines and
/// records without a usable message are skipped.
///
/// Prompt text, tool inputs and outputs are read here only to decide a label (a
/// prompt's kind, a tool call's tag); none of it is copied into a record.
pub fn parse_str(project: &str, content: &str) -> ParsedFile {
    parse_with(project, content, &Matcher::default())
}

/// parse_str with a harness trigger file: a matching opening line gets its
/// trigger label, and a tool call on a bookkeeping file the tag "bookkeeping".
pub fn parse_with(project: &str, content: &str, matcher: &Matcher) -> ParsedFile {
    let mut messages: Vec<MessageRecord> = Vec::new();
    let mut tool_calls = Vec::new();
    let mut prompts = Vec::new();
    // tool_use_id -> userModified, collected from user records' toolUseResult.
    let mut user_modified: HashMap<String, bool> = HashMap::new();
    // tool_use_id -> the failure count a test runner reported (-1: is_error with
    // no count). Read from the output in memory; only the number is kept.
    let mut outcome: HashMap<String, Option<i64>> = HashMap::new();
    let mut title_map: HashMap<String, String> = HashMap::new();
    let mut cost_map: BTreeMap<String, CostStateRecord> = BTreeMap::new();
    // Claude Code writes its cost-state snapshot now and then; it covers only the
    // records before it, so remember the last timestamp seen.
    let mut last_ts = String::new();
    // Claude Code writes one assistant line per content block and repeats the
    // call's full usage on each, so a call is kept once, keyed by message.id.
    let mut call_index: HashMap<String, usize> = HashMap::new();
    // Source files written since the last prompt (path, tool call index). A file a
    // later command in the same turn puts back was a scratch edit (a mutation
    // check, a probe), not a change: its call is retagged "scratch".
    let mut turn_writes: Vec<(String, usize)> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // skip non-JSON lines
        };
        let rec_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let session_id = v.get("sessionId").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let ts = v.get("timestamp").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let git_branch = v.get("gitBranch").and_then(|s| s.as_str()).map(|s| s.to_string());
        let version = v.get("version").and_then(|s| s.as_str()).map(|s| s.to_string());
        let cwd = v.get("cwd").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let agent_id = v.get("agentId").and_then(|s| s.as_str()).map(|s| s.to_string());
        if !ts.is_empty() && ts > last_ts {
            last_ts = ts.clone();
        }

        // Collect userModified from any toolUseResult that names a tool_use_id.
        if let Some(tur) = v.get("toolUseResult") {
            if let Some(um) = tur.get("userModified").and_then(|b| b.as_bool()) {
                if let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                    for block in content {
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                            if let Some(id) = block.get("tool_use_id").and_then(|s| s.as_str()) {
                                user_modified.insert(id.to_string(), um);
                            }
                        }
                    }
                }
            }
        }

        if rec_type == "user" {
            if let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                for block in content.iter().filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result")) {
                    let Some(id) = block.get("tool_use_id").and_then(|s| s.as_str()) else { continue };
                    let text = match block.get("content") {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"),
                        _ => String::new(),
                    };
                    let is_error = block.get("is_error").and_then(|b| b.as_bool()) == Some(true);
                    let count = test_output_fail_count(&text).or(if is_error { Some(-1) } else { None });
                    outcome.insert(id.to_string(), count);
                }
            }
        }

        if rec_type == "ai-title" {
            if let Some(title) = v.get("aiTitle").and_then(|s| s.as_str()) {
                if !session_id.is_empty() {
                    title_map.insert(session_id.clone(), title.to_string());
                }
            }
            continue;
        }
        if rec_type == "cost-state" {
            if let Some(c) = v.get("totalCostUSD").and_then(|c| c.as_f64()) {
                if !session_id.is_empty() {
                    let from_ts = v
                        .get("startTime")
                        .and_then(|t| t.as_i64())
                        .and_then(chrono::DateTime::from_timestamp_millis)
                        .map(|d| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
                        .unwrap_or_default();
                    cost_map.insert(
                        session_id.clone(),
                        CostStateRecord { session_id: session_id.clone(), total_usd: c, from_ts, to_ts: last_ts.clone() },
                    );
                }
            }
            continue;
        }
        if rec_type == "system" {
            if v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary") {
                prompts.push(PromptRecord { session_id, agent_id, ts, kind: "compact".into(), ..Default::default() });
            }
            continue;
        }

        if rec_type != "assistant" && rec_type != "user" {
            continue;
        }
        let msg = match v.get("message").and_then(|m| m.as_object()) {
            Some(m) => m,
            None => continue,
        };
        let role = msg.get("role").and_then(|s| s.as_str()).map(|s| s.to_string());
        let stop_reason = msg.get("stop_reason").and_then(|s| s.as_str()).map(|s| s.to_string());
        // An assistant row is keyed by its API call; a user row by its record uuid,
        // so a copy of either in another file can be found.
        let message_id = if rec_type == "assistant" {
            msg.get("id").and_then(|s| s.as_str()).map(|s| s.to_string())
        } else {
            v.get("uuid").and_then(|s| s.as_str()).map(|s| s.to_string())
        };

        if rec_type == "user" {
            if let Some(kind) = prompt_kind(&v) {
                turn_writes.clear();
                let hit = if kind == "human" { prompt_text(&v).and_then(|t| matcher.prompt(&t)) } else { None };
                let (trigger, msg_ids) = match hit {
                    Some((label, ids)) => (Some(label), (!ids.is_empty()).then(|| ids.join(","))),
                    None => (None, None),
                };
                prompts.push(PromptRecord { session_id: session_id.clone(), agent_id: agent_id.clone(), ts: ts.clone(), kind, trigger, msg_ids });
            }
        }

        // Only assistant records carry usage worth recording; still record the
        // message row so counts are correct.
        let usage: RawUsage = msg
            .get("usage")
            .cloned()
            .map(|u| serde_json::from_value(u).unwrap_or_default())
            .unwrap_or_default();
        let model = msg.get("model").and_then(|s| s.as_str()).map(|m| {
            if usage.speed.as_deref() == Some("fast") { fast_key(m) } else { m.to_string() }
        });

        let record = MessageRecord {
            session_id: session_id.clone(),
            project: project.to_string(),
            ts: ts.clone(),
            role,
            model,
            version: version.clone(),
            git_branch: git_branch.clone(),
            service_tier: usage.service_tier.clone(),
            stop_reason,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation: usage.cache_creation_input_tokens,
            cache_read: usage.cache_read_input_tokens,
            ephemeral_1h: usage.cache_creation.ephemeral_1h_input_tokens,
            ephemeral_5m: usage.cache_creation.ephemeral_5m_input_tokens,
            message_id: message_id.clone(),
            agent_id: agent_id.clone(),
            web_searches: usage.server_tool_use.web_search_requests,
        };
        match message_id.as_ref().and_then(|id| call_index.get(id)) {
            // A later line of a call already seen: keep the largest value of each
            // field (they are equal in practice) instead of adding them again.
            Some(&i) => {
                let m = &mut messages[i];
                m.input_tokens = m.input_tokens.max(record.input_tokens);
                m.output_tokens = m.output_tokens.max(record.output_tokens);
                m.cache_creation = m.cache_creation.max(record.cache_creation);
                m.cache_read = m.cache_read.max(record.cache_read);
                m.ephemeral_1h = m.ephemeral_1h.max(record.ephemeral_1h);
                m.ephemeral_5m = m.ephemeral_5m.max(record.ephemeral_5m);
                m.web_searches = m.web_searches.max(record.web_searches);
                if m.stop_reason.is_none() {
                    m.stop_reason = record.stop_reason;
                }
            }
            None => {
                if let Some(id) = &message_id {
                    call_index.insert(id.clone(), messages.len());
                }
                messages.push(record);
            }
        }

        // tool_use blocks in the content array become tool_call rows.
        if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    let name = block.get("name").and_then(|s| s.as_str()).unwrap_or("?").to_string();
                    let tool_use_id = block.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());
                    let is_mcp = name.starts_with("mcp__");
                    let mut tag = tool_tag_in(&name, block.get("input"), &cwd);
                    let input = block.get("input");
                    // The harness's own files: bookkeeping, unless it is a test run or a review.
                    if !matches!(tag.as_deref(), Some("test") | Some("review")) {
                        let target = ["command", "file_path", "path", "notebook_path"]
                            .iter()
                            .find_map(|k| input.and_then(|i| i.get(*k)).and_then(|p| p.as_str()))
                            .unwrap_or("");
                        if matcher.is_bookkeeping(target) {
                            tag = Some("bookkeeping".into());
                        }
                    }
                    let idx = tool_calls.len();
                    if name == "Bash" {
                        let cmd = input.and_then(|i| i.get("command")).and_then(|c| c.as_str()).unwrap_or("");
                        let restored = restore_targets(cmd);
                        let mut put_back: Vec<String> = Vec::new();
                        for r in &restored {
                            for (path, at) in &turn_writes {
                                if same_path(path, r) {
                                    let t: &mut ToolCallRecord = &mut tool_calls[*at];
                                    t.tag = Some("scratch".into());
                                    put_back.push(r.clone());
                                }
                            }
                        }
                        if tag.as_deref() == Some("write") {
                            let targets: Vec<String> = write_targets_in(cmd, &cwd).into_iter().filter(|t| !is_temp_path(t)).collect();
                            // Only putting files back (cp a backup over a file written this
                            // turn) is part of the scratch edit, not a new one.
                            if !targets.is_empty() && targets.iter().all(|t| put_back.iter().any(|b| same_path(b, t))) {
                                tag = Some("scratch".into());
                            } else {
                                turn_writes.extend(targets.into_iter().map(|t| (t, idx)));
                            }
                        }
                    } else if tag.is_none() && ["Edit", "Write", "MultiEdit", "NotebookEdit"].contains(&name.as_str()) {
                        if let Some(path) = input.and_then(|i| i.get("file_path")).and_then(|p| p.as_str()) {
                            turn_writes.push((path.to_string(), idx));
                        }
                    }
                    let test_key = if tag.as_deref() == Some("test") {
                        input.and_then(|i| i.get("command")).and_then(|c| c.as_str()).and_then(|c| test_command_key(c, &cwd))
                    } else {
                        None
                    };
                    tool_calls.push(ToolCallRecord {
                        session_id: session_id.clone(),
                        project: project.to_string(),
                        ts: ts.clone(),
                        tool_use_id,
                        name,
                        is_mcp,
                        user_modified: None,
                        tag,
                        fail_count: None,
                        test_key,
                    });
                }
            }
        }
    }

    // Second pass: attach userModified, and a test run's outcome, by tool_use_id.
    for tc in tool_calls.iter_mut() {
        if let Some(id) = &tc.tool_use_id {
            if let Some(um) = user_modified.get(id) {
                tc.user_modified = Some(*um);
            }
            if tc.tag.as_deref() == Some("test") {
                tc.fail_count = outcome.get(id).copied().flatten();
            }
        }
    }

    let titles: Vec<(String, String)> = title_map.into_iter().collect();
    let cost_states: Vec<CostStateRecord> = cost_map.into_values().collect();
    ParsedFile { messages, tool_calls, titles, prompts, cost_states }
}

/// The text of a user record, or None when it is a tool result or a meta record.
pub(crate) fn prompt_text(v: &Value) -> Option<String> {
    if v.get("isMeta").and_then(|b| b.as_bool()) == Some(true) {
        return None;
    }
    match v.pointer("/message/content")? {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            if blocks.iter().any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result")) {
                return None;
            }
            let text: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect();
            Some(text.join("\n"))
        }
        _ => None,
    }
}

/// Lines Claude Code writes into the transcript that are not prompts: command
/// output, shell escapes, interrupts. No model call answers them.
const NOT_PROMPTS: [&str; 6] = ["<local-command", "<bash-", "[Request interrupted", "<command-message>", "<command-args>", "<command-name>"];

/// Decide a user record's prompt kind from fixed harness markers.
fn prompt_kind(v: &Value) -> Option<String> {
    let text = prompt_text(v)?;
    if v.get("isCompactSummary").and_then(|b| b.as_bool()) == Some(true) {
        return Some("resume".into());
    }
    let t = text.trim_start();
    if t.starts_with("<command-name>/compact") || t.starts_with("/compact") {
        return Some("compact".into());
    }
    if NOT_PROMPTS.iter().any(|p| t.starts_with(p)) {
        return None;
    }
    // A paste wrapper does not change what the prompt is.
    let body = if t.starts_with("<pasted_content") { t.split_once('\n').map(|x| x.1).unwrap_or("") } else { t };
    let origin = v.pointer("/origin/kind").and_then(|k| k.as_str());
    if body.starts_with("<task-notification>") || origin == Some("task-notification") {
        return Some("task-notification".into());
    }
    Some("human".into())
}

const TEST_TOOLS: [&str; 7] = ["pytest", "jest", "vitest", "mocha", "rspec", "phpunit", "ctest"];
const TEST_RUNNERS: [&str; 16] =
    ["cargo", "go", "npm", "pnpm", "yarn", "bun", "deno", "mix", "dotnet", "swift", "xcodebuild", "make", "mvn", "gradle", "flutter", "dart"];

/// Is this shell command a test run? Word-based, so `cat test.txt` is not.
pub fn is_test_command(cmd: &str) -> bool {
    let cleaned = cmd.replace("&&", " ").replace("||", " ").replace([';', '|', '(', ')'], " ");
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let w = w.rsplit('/').next().unwrap_or(w);
        if TEST_TOOLS.contains(&w) {
            return true;
        }
        if TEST_RUNNERS.contains(&w) {
            let mut next = words.get(i + 1).copied();
            if next == Some("run") {
                next = words.get(i + 2).copied();
            }
            // `test`, `tests`, or a named script such as `test:renderer`.
            if matches!(next, Some("test") | Some("tests")) || next.is_some_and(|n| n.starts_with("test:")) {
                return true;
            }
        }
        if w == "node" && words.contains(&"--test") {
            return true;
        }
    }
    false
}

/// How many tests did this run's output report failing? Reads the summary lines
/// test runners print: TAP/node ("# fail N") and "N failed" / "N failing" (cargo,
/// jest, vitest, pytest, mocha). The largest count found; None when there is none.
pub fn test_output_fail_count(text: &str) -> Option<i64> {
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '(' || c == ')')
        .filter(|w| !w.is_empty())
        .collect();
    let count = |w: &str| w.parse::<i64>().ok();
    let mut found: Option<i64> = None;
    for (i, w) in words.iter().enumerate() {
        let n = if *w == "fail" && i > 0 && words[i - 1] == "#" {
            words.get(i + 1).and_then(|n| count(n)) // "# fail N"
        } else if (*w == "failed" || *w == "failing") && i > 0 {
            count(words[i - 1]) // "N failed" / "N failing"
        } else {
            None
        };
        if let Some(n) = n {
            found = Some(found.map_or(n, |f| f.max(n)));
        }
    }
    found
}

/// Is this shell command reviewing a pull request? `gh pr` / `bb pr` subcommands
/// that read, check out or comment on one (not create or merge), or git fetching
/// or diffing a pull-request ref.
pub fn is_review_command(cmd: &str) -> bool {
    const PR_READS: [&str; 8] = ["view", "diff", "checkout", "review", "comment", "checks", "approve", "status"];
    let cleaned = cmd.replace("&&", " ").replace("||", " ").replace([';', '|', '(', ')'], " ");
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        if (*w == "gh" || *w == "bb") && words.get(i + 1) == Some(&"pr") {
            if words.get(i + 2).is_some_and(|sub| PR_READS.contains(sub)) {
                return true;
            }
        }
        if *w == "git" && words.get(i + 1).is_some_and(|sub| ["fetch", "diff", "log", "checkout"].contains(sub)) {
            if words[i + 2..].iter().any(|a| a.contains("pull/") || a.contains("refs/pr/")) {
                return true;
            }
        }
        // A local review reads a range or a commit: git diff/log A..B, git show <sha>.
        if *w == "git" && words.get(i + 1).is_some_and(|sub| ["diff", "log", "show"].contains(sub)) {
            let args = words[i + 2..].iter().take_while(|a| !a.starts_with('>'));
            let is_sha = |a: &&&str| a.len() >= 7 && a.len() <= 40 && a.chars().all(|c| c.is_ascii_hexdigit());
            let mut args = args.peekable();
            // A pre-push check (origin/main..HEAD, @{u}..HEAD) is git ops, not a review.
            let pre_push = |a: &str| {
                let (left, right) = a.split_once("...").or_else(|| a.split_once("..")).unwrap_or((a, ""));
                (right.is_empty() || right == "HEAD")
                    && (left.starts_with("origin/") || left.starts_with("upstream/") || left.starts_with("@{u"))
            };
            let range = args.clone().any(|a| a.contains("..") && !a.starts_with('-') && !pre_push(a));
            let commit = words.get(i + 1) == Some(&"show") && args.any(|a| is_sha(&a));
            if range || commit {
                return true;
            }
        }
    }
    false
}

/// A notes or docs file (markdown, text): writing one is not a source edit.
fn is_notes_path(p: &str) -> bool {
    let p = p.trim_matches(|c| c == '\'' || c == '"');
    [".md", ".markdown", ".txt", ".rst", ".adoc"].iter().any(|ext| p.to_lowercase().ends_with(ext))
}

/// Drop heredoc bodies: after a line with an unquoted `<<WORD` (or `<<-WORD`,
/// quoted or not), skip lines up to and including the one that is just WORD. The
/// body is data for a program, not shell, so its words are not the command's.
fn strip_heredocs(cmd: &str) -> String {
    let mut out = Vec::new();
    let mut until: Option<String> = None;
    for line in cmd.lines() {
        if let Some(end) = &until {
            if line.trim() == end { until = None; }
            continue;
        }
        out.push(line);
        let toks = shell_tokens(line);
        if let Some(i) = toks.iter().position(|t| t.op && t.text.starts_with("<<") && t.text != "<<<") {
            if let Some(w) = toks.get(i + 1).filter(|t| !t.op) {
                until = Some(w.text.clone());
            }
        }
    }
    out.join("\n")
}

/// One shell token: a word (quotes removed; `''` is an empty word) or, when `op`,
/// an unquoted operator: a separator (`;` `|` `||` `&&`, a newline is `;`), a
/// redirect (`>` `>>` `2>` `&>`, `2>&1` as one token) or `<`, `<<`, `<<-`, `<<<`.
#[derive(Debug, Clone, PartialEq)]
struct Tok { text: String, op: bool }

/// Split one shell line (or command) into tokens, honouring quotes. Not a full
/// shell parser: enough to find a command's arguments and redirects.
fn shell_tokens(cmd: &str) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    fn flush(out: &mut Vec<Tok>, cur: &mut String, started: &mut bool) {
        if *started || !cur.is_empty() { out.push(Tok { text: std::mem::take(cur), op: false }); }
        *started = false;
    }
    let op = |out: &mut Vec<Tok>, t: &str| out.push(Tok { text: t.to_string(), op: true });
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => { quote = Some(c); started = true; }
            None if c == '\n' => { flush(&mut out, &mut cur, &mut started); op(&mut out, ";"); }
            None if c.is_whitespace() => flush(&mut out, &mut cur, &mut started),
            None if c == ';' || c == '(' || c == ')' => { flush(&mut out, &mut cur, &mut started); op(&mut out, ";"); }
            None if c == '|' => {
                flush(&mut out, &mut cur, &mut started);
                if next == Some('|') { op(&mut out, "||"); i += 1; } else { op(&mut out, "|"); }
            }
            None if c == '&' && next == Some('&') => { flush(&mut out, &mut cur, &mut started); op(&mut out, "&&"); i += 1; }
            None if c == '>' || (c == '&' && next == Some('>')) => {
                // An fd number or & right before the > belongs to the operator.
                let mut text = if !cur.is_empty() && cur.chars().all(|d| d.is_ascii_digit()) { std::mem::take(&mut cur) } else { flush(&mut out, &mut cur, &mut started); String::new() };
                started = false;
                if c == '&' { text.push('&'); i += 1; }
                text.push('>');
                if chars.get(i + 1) == Some(&'>') { text.push('>'); i += 1; }
                // >&N duplicates a descriptor: no file.
                if chars.get(i + 1) == Some(&'&') {
                    text.push('&'); i += 1;
                    while chars.get(i + 1).is_some_and(|d| d.is_ascii_digit() || *d == '-') { text.push(chars[i + 1]); i += 1; }
                }
                op(&mut out, &text);
            }
            None if c == '<' => {
                flush(&mut out, &mut cur, &mut started);
                let mut text = String::from("<");
                while chars.get(i + 1) == Some(&'<') { text.push('<'); i += 1; }
                if text == "<<" && chars.get(i + 1) == Some(&'-') { text.push('-'); i += 1; }
                op(&mut out, &text);
            }
            None => { cur.push(c); started = true; }
        }
        i += 1;
    }
    flush(&mut out, &mut cur, &mut started);
    out
}

/// The words of a command (heredoc bodies dropped), operators included as text.
fn shell_words(cmd: &str) -> Vec<String> {
    shell_tokens(&strip_heredocs(cmd)).into_iter().map(|t| t.text).collect()
}

/// Resolve `.` and `..` in a path and drop a trailing `/` (`/repo/sub/..` is `/repo`).
fn normalize_path(p: &str) -> String {
    let absolute = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => { if parts.last().is_some_and(|l| *l != "..") { parts.pop(); } else if !absolute { parts.push(".."); } }
            _ => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if absolute { format!("/{joined}") } else { joined }
}

/// A stable 64-bit FNV-1a hash, as 16 hex digits.
fn fnv1a(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Which test suite a run was, as a hash: the directory it ran in (the record's
/// cwd, moved by any `cd` before the test command) and the shell segment that
/// runs the tests, with redirects, leading `VAR=value` assignments and extra
/// spaces dropped. The same suite run with different plumbing gets one key; a
/// focused run, or the same command in another repo or worktree, gets another.
/// Only the hash is kept, never the command or the directory.
pub fn test_command_key(cmd: &str, cwd: &str) -> Option<String> {
    let mut segments: Vec<Vec<String>> = vec![Vec::new()];
    for w in shell_words(cmd) {
        if is_separator(&w) { segments.push(Vec::new()); } else { segments.last_mut().unwrap().push(w); }
    }
    let at = segments.iter().position(|s| !s.is_empty() && is_test_command(&s.join(" ")))?;
    let mut dir = cwd.to_string();
    for s in &segments[..at] {
        if s.first().map(|w| w.as_str()) == Some("cd") {
            if let Some(target) = s.get(1) {
                dir = if target.starts_with('/') || target.starts_with('~') { target.clone() } else { format!("{dir}/{target}") };
            }
        }
    }
    let seg = segments.swap_remove(at);
    let mut out: Vec<String> = Vec::new();
    let mut skip_target = false;
    for w in seg {
        if skip_target { skip_target = false; continue; }
        let redirect = w.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
        if redirect.starts_with('>') || redirect.starts_with('<') {
            // A bare operator takes the next word as its target.
            skip_target = redirect.trim_start_matches(['>', '<', '&']).is_empty() && !w.contains("&1") && !w.contains("&2");
            continue;
        }
        if out.is_empty() && w.contains('=') && !w.starts_with('-') {
            continue;
        }
        out.push(w);
    }
    let dir = normalize_path(&dir);
    (!out.is_empty()).then(|| fnv1a(&format!("{dir}\0{}", out.join(" "))))
}

fn is_separator(w: &str) -> bool {
    matches!(w, "&&" | "||" | ";" | "|")
}

/// The files a shell command writes: output redirects (not /dev/*), in-place
/// sed/perl targets, tee targets, and cp/mv destinations. `git apply` and `patch`
/// write files they do not name, reported as "<patch>". Heredoc bodies and quoted
/// text are not the command. A script run with `python3 - <<EOF` is not seen.
pub fn write_targets(cmd: &str) -> Vec<String> {
    write_targets_in(cmd, "")
}

/// write_targets, with each relative target resolved against the directory it was
/// written in: `cwd`, moved by any `cd` before it. A leading `$VAR` / `${VAR}` set
/// earlier in the command (`T=$TMPDIR`, `export T=/tmp/x`) is expanded, so writes
/// through such an alias are seen for what they are.
pub fn write_targets_in(cmd: &str, cwd: &str) -> Vec<String> {
    let raw = write_targets_raw(cmd);
    let toks = shell_tokens(&strip_heredocs(cmd));
    let mut dir = cwd.to_string();
    let mut vars: Vec<(String, String)> = Vec::new();
    let mut first = true;
    let mut i = 0;
    let mut dirs_at: Vec<(usize, String, Vec<(String, String)>)> = Vec::new();
    while i < toks.len() {
        let t = &toks[i];
        if t.op { first = !t.text.contains('>') && !t.text.starts_with('<'); i += 1; continue; }
        if first {
            first = false;
            let word = if t.text == "export" { toks.get(i + 1).filter(|n| !n.op).map(|n| n.text.as_str()).unwrap_or("") } else { t.text.as_str() };
            if t.text == "cd" {
                if let Some(target) = toks.get(i + 1).filter(|n| !n.op) {
                    dir = if target.text.starts_with('/') || target.text.starts_with('~') { target.text.clone() } else { format!("{dir}/{}", target.text) };
                }
            } else if let Some((name, value)) = word.split_once('=') {
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    vars.push((name.to_string(), value.to_string()));
                }
            }
        }
        dirs_at.push((i, dir.clone(), vars.clone()));
        i += 1;
    }
    // Resolve each raw target with the dir and vars in force where it appears.
    raw.into_iter().map(|(at, target)| {
        let (dir, vars) = dirs_at.iter().rev().find(|(j, _, _)| *j <= at).map(|(_, d, v)| (d.clone(), v.clone())).unwrap_or((cwd.to_string(), Vec::new()));
        let mut t = target;
        for (name, value) in vars.iter().rev() {
            for form in [format!("${{{name}}}"), format!("${name}")] {
                if let Some(rest) = t.strip_prefix(&form) { t = format!("{value}{rest}"); break; }
            }
        }
        let relative = !(t.starts_with('/') || t.starts_with('$') || t.starts_with('~') || t.starts_with('<'));
        if relative && !dir.is_empty() { normalize_path(&format!("{dir}/{t}")) } else { t }
    }).collect()
}

/// The raw write targets with the token index each was found at.
fn write_targets_raw(cmd: &str) -> Vec<(usize, String)> {
    let toks = shell_tokens(&strip_heredocs(cmd));
    let mut out: Vec<(usize, String)> = Vec::new();
    // The words of one simple command: up to the next separator or redirect.
    let args = |from: usize| -> Vec<&String> { toks[from..].iter().take_while(|t| !t.op).map(|t| &t.text).collect() };
    for (i, t) in toks.iter().enumerate() {
        if t.op {
            let is_redirect_out = t.text.contains('>') && !t.text.contains(">&");
            if is_redirect_out {
                if let Some(target) = toks.get(i + 1).filter(|n| !n.op) {
                    if !target.text.is_empty() && !target.text.starts_with("/dev/") {
                        out.push((i + 1, target.text.clone()));
                    }
                }
            }
            continue;
        }
        // Only a command word (the first of its simple command) starts a command.
        let first = i == 0 || toks[i - 1].op && !toks[i - 1].text.contains('>') && !toks[i - 1].text.starts_with('<');
        if !first {
            continue;
        }
        let rest = args(i + 1);
        match t.text.rsplit('/').next().unwrap_or("") {
            "sed" | "perl" if rest.iter().take(3).any(|a| a.starts_with("-i") || a.starts_with("-pi")) => {
                // After the options (and BSD's empty -i suffix), the first argument
                // is the script unless -e gave it; the rest are the files.
                let mut script_seen = rest.iter().any(|a| a.as_str() == "-e");
                let mut skip_next = false;
                for a in rest {
                    if skip_next { skip_next = false; continue; }
                    if a.starts_with('-') { skip_next = a.as_str() == "-e"; continue; }
                    if a.is_empty() { continue; }
                    if !script_seen { script_seen = true; continue; }
                    out.push((i, a.clone()));
                }
            }
            "tee" => out.extend(rest.into_iter().filter(|a| !a.starts_with('-')).map(|a| (i, a.clone()))),
            "cp" | "mv" => {
                if let Some(dst) = rest.iter().filter(|a| !a.starts_with('-')).nth(1).and(rest.iter().filter(|a| !a.starts_with('-')).last()) {
                    out.push((i, dst.to_string()));
                }
            }
            "git" if rest.first().map(|s| s.as_str()) == Some("apply") => out.push((i, "<patch>".into())),
            "patch" => out.push((i, "<patch>".into())),
            _ => {}
        }
    }
    out
}

/// A scratch location: the temp dirs. Writing there is not a source edit.
fn is_temp_path(p: &str) -> bool {
    ["/tmp/", "/private/tmp/", "/var/folders/", "/private/var/folders/", "$TMPDIR", "${TMPDIR}"].iter().any(|t| p.starts_with(t))
}

/// Does this shell command write a file (an edit made from the shell)?
pub fn writes_files(cmd: &str) -> bool {
    !write_targets(cmd).is_empty()
}

/// Files a shell command puts back: `git checkout [<ref>] -- <paths>`,
/// `git restore <paths>`, and the destination of `cp` / `mv`.
fn restore_targets(cmd: &str) -> Vec<String> {
    let words = shell_words(cmd);
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let rest: Vec<&String> = words[i + 1..].iter().take_while(|a| !is_separator(a)).collect();
        let sub = rest.first().map(|s| s.as_str());
        match (w.as_str(), sub) {
            ("git", Some("checkout")) => {
                if let Some(dd) = rest.iter().position(|a| a.as_str() == "--") {
                    out.extend(rest[dd + 1..].iter().map(|s| s.to_string()));
                }
            }
            ("git", Some("restore")) => out.extend(rest[1..].iter().filter(|a| !a.starts_with('-')).map(|s| s.to_string())),
            ("cp", _) | ("mv", _) => {
                if let Some(dst) = rest.iter().filter(|a| !a.starts_with('-')).last() {
                    out.push(dst.to_string());
                }
            }
            // A file removed again was a scratch file.
            ("rm", _) => out.extend(rest.iter().filter(|a| !a.starts_with('-')).map(|s| s.to_string())),
            _ => {}
        }
    }
    out
}

/// The same file, written two ways (relative and absolute)?
fn same_path(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim_start_matches("./"), b.trim_start_matches("./"));
    !a.is_empty() && !b.is_empty() && (a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}")))
}

/// Does this shell command only read files (cat, sed -n, grep, rg, head, tail,
/// less, wc, nl)? A local review reads code this way rather than with Read.
pub fn is_shell_read(cmd: &str) -> bool {
    const READERS: [&str; 8] = ["cat", "grep", "rg", "head", "tail", "less", "wc", "nl"];
    if writes_files(cmd) {
        return false;
    }
    let words = shell_words(cmd);
    let mut first = true;
    for (i, w) in words.iter().enumerate() {
        if is_separator(w) { first = true; continue; }
        if first {
            first = false;
            let w = w.rsplit('/').next().unwrap_or(w);
            if READERS.contains(&w) { return true; }
            if w == "sed" && words[i + 1..].iter().take_while(|a| !is_separator(a)).any(|a| a.as_str() == "-n") { return true; }
        }
    }
    false
}

/// A tool call's tag, decided from its input in memory: "test" for a test run,
/// "review" for a pull-request command or
/// a read-only git range, "notes" for a notes/docs file written, "write" for any
/// other file written from the shell, "read" for a shell command that only reads,
/// else none. parse_str retags a source edit restored in the same turn "scratch".
#[cfg(test)]
fn tool_tag(name: &str, input: Option<&Value>) -> Option<String> {
    tool_tag_in(name, input, "")
}

/// tool_tag, for a call made with working directory `cwd`.
fn tool_tag_in(name: &str, input: Option<&Value>, cwd: &str) -> Option<String> {
    let input = input?;
    if name == "Bash" {
        let cmd = input.get("command").and_then(|c| c.as_str()).unwrap_or("");
        if is_test_command(cmd) {
            return Some("test".into());
        }
        if is_review_command(cmd) {
            return Some("review".into());
        }
        let targets = write_targets_in(cmd, cwd);
        if !targets.is_empty() {
            // Temp files are scratch; of the rest, notes only when every one is a notes file.
            let real: Vec<&String> = targets.iter().filter(|t| !is_temp_path(t)).collect();
            if real.is_empty() {
                return Some("scratch".into());
            }
            let notes = real.iter().all(|t| is_notes_path(t));
            return Some(if notes { "notes" } else { "write" }.into());
        }
        return is_shell_read(cmd).then(|| "read".into());
    }
    let path = ["file_path", "path", "notebook_path"]
        .iter()
        .find_map(|k| input.get(*k).and_then(|p| p.as_str()))
        .unwrap_or("");
    let edits = ["Edit", "Write", "MultiEdit", "NotebookEdit"].contains(&name);
    let in_temp_cwd = !path.starts_with('/') && is_temp_path(&format!("{cwd}/"));
    if edits && (is_temp_path(path) || in_temp_cwd) {
        return Some("scratch".into());
    }
    (edits && is_notes_path(path)).then(|| "notes".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ParsedFile {
        let content = include_str!("../tests/fixtures/sample.jsonl");
        parse_str("proj-a", content)
    }

    #[test]
    fn parses_message_rows_with_tokens() {
        let p = sample();
        assert_eq!(p.messages.len(), 3);
        let first = &p.messages[0];
        assert_eq!(first.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(first.input_tokens, 100);
        assert_eq!(first.cache_read, 200);
        assert_eq!(first.ephemeral_1h, 40);
        assert_eq!(first.project, "proj-a");
    }

    #[test]
    fn parses_tool_calls_and_flags_mcp() {
        let p = sample();
        assert_eq!(p.tool_calls.len(), 2);
        assert_eq!(p.tool_calls[0].name, "Edit");
        assert!(!p.tool_calls[0].is_mcp);
        assert!(p.tool_calls[1].is_mcp);
    }

    #[test]
    fn links_user_modified_by_tool_use_id() {
        let p = sample();
        let edit = p.tool_calls.iter().find(|t| t.name == "Edit").unwrap();
        assert_eq!(edit.user_modified, Some(true));
    }

    #[test]
    fn captures_session_title() {
        let p = sample();
        assert_eq!(p.titles.len(), 1);
        assert_eq!(p.titles[0], ("s1".to_string(), "Refactor the parser".to_string()));
    }

    fn activity() -> ParsedFile {
        parse_str("proj-a", include_str!("../tests/fixtures/activity.jsonl"))
    }

    #[test]
    fn one_api_call_split_over_several_lines_is_counted_once() {
        let p = activity();
        let calls: Vec<_> = p.messages.iter().filter(|m| m.role.as_deref() == Some("assistant")).collect();
        assert_eq!(calls.len(), 2, "msg_1 spans three lines but is one call");
        assert_eq!(calls[0].message_id.as_deref(), Some("msg_1"));
        assert_eq!(calls[0].cache_read, 5000);
        assert_eq!(p.messages.iter().map(|m| m.output_tokens).sum::<i64>(), 120);
        // Tool calls still come from every line of the call.
        assert_eq!(p.tool_calls.len(), 5);
    }

    #[test]
    fn fast_calls_are_keyed_apart_and_web_searches_kept() {
        let p = activity();
        let m2 = p.messages.iter().find(|m| m.message_id.as_deref() == Some("msg_2")).unwrap();
        assert_eq!(m2.model.as_deref(), Some("claude-opus-5-5:fast"));
        assert_eq!(m2.web_searches, 3);
    }

    #[test]
    fn prompts_are_labelled_by_kind_and_command_output_is_not_a_prompt() {
        let p = activity();
        let kinds: Vec<&str> = p.prompts.iter().map(|x| x.kind.as_str()).collect();
        // The fifth is wrapped in <pasted_content>: still a prompt you typed.
        assert_eq!(kinds, vec!["human", "human", "compact", "resume", "human", "task-notification", "compact"]);
    }

    #[test]
    fn tool_calls_carry_a_tag_and_no_command_or_path() {
        let p = activity();
        let tags: Vec<Option<&str>> = p.tool_calls.iter().map(|t| t.tag.as_deref()).collect();
        assert_eq!(tags, vec![Some("test"), None, None, None, Some("test")]);
        assert_eq!(p.tool_calls[0].tool_use_id.as_deref(), Some("t1"));
    }

    #[test]
    fn keeps_claude_codes_own_last_cost_for_the_session() {
        // The snapshot covers the calls of the process that wrote it: from its
        // startTime up to the last timestamp written before it.
        let cs = activity().cost_states;
        assert_eq!(cs.len(), 1);
        assert_eq!((cs[0].session_id.as_str(), cs[0].total_usd), ("sa", 0.75));
        assert_eq!(cs[0].from_ts, "2026-10-01T10:00:00.000Z");
        assert_eq!(cs[0].to_ts, "2026-10-01T10:06:00.000Z");
    }

    #[test]
    fn shell_commands_that_write_files_are_tagged_write() {
        for c in ["sed -i '' 's/a/b/' src/x.rs", "cat > src/a.ts <<'EOF'", "echo x >> notes.md", "git apply fix.patch", "perl -pi -e 's/a/b/' f", "printf x | tee out.txt"] {
            assert!(writes_files(c), "{c}");
        }
        for c in ["git status", "cargo build 2>/dev/null", "ls > /dev/null", "grep -n foo src 2>&1 | head", "[ $a -gt 1 ]"] {
            assert!(!writes_files(c), "{c}");
        }
        let tag = |cmd: &str| tool_tag("Bash", Some(&serde_json::json!({ "command": cmd })));
        assert_eq!(tag("sed -i '' 's/a/b/' src/x.rs").as_deref(), Some("write"));
    }

    #[test]
    fn user_rows_carry_their_uuid_so_copies_can_be_found() {
        let p = parse_str("p", r#"{"type":"user","sessionId":"s","uuid":"u-1","timestamp":"2026-10-01T10:00:00.000Z","message":{"role":"user","content":"hi"}}"#);
        assert_eq!(p.messages[0].message_id.as_deref(), Some("u-1"));
    }

    #[test]
    fn a_test_runs_failure_count_is_read_from_its_result_and_kept_as_a_number() {
        for (out, want) in [("# tests 9\n# pass 7\n# fail 2", Some(2)), ("# pass 9\n# fail 0", Some(0)),
            ("test result: FAILED. 61 passed; 1 failed", Some(1)), ("test result: ok. 62 passed; 0 failed", Some(0)),
            ("Tests: 3 failed, 10 passed", Some(3)), ("=== 2 failed, 5 passed in 1.2s ===", Some(2)), ("all good", None)] {
            assert_eq!(test_output_fail_count(out), want, "{out}");
        }
        let lines = [
            r##"{"type":"assistant","sessionId":"s","timestamp":"2026-10-01T10:00:00.000Z","message":{"id":"m1","role":"assistant","model":"claude-haiku-4-5","usage":{"input_tokens":1,"output_tokens":1},"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"npm test 2>&1 | tail -5"}},{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"cargo test"}},{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"ls"}}]}}"##,
            r##"{"type":"user","sessionId":"s","timestamp":"2026-10-01T10:00:01.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"# pass 3\n# fail 1"}]},{"type":"tool_result","tool_use_id":"t2","is_error":true,"content":"Exit code 101"},{"type":"tool_result","tool_use_id":"t3","is_error":true,"content":"nope"}]}}"##,
        ].join("\n");
        let p = parse_str("p", &lines);
        let counts: Vec<Option<i64>> = p.tool_calls.iter().map(|t| t.fail_count).collect();
        // t2 failed (is_error) with no count it could read: -1 = "failed, count unknown".
        assert_eq!(counts, vec![Some(1), Some(-1), None], "only test runs carry an outcome");
    }

    #[test]
    fn pr_tools_are_tagged_review_and_notes_files_are_not_source_edits() {
        let tag = |name: &str, input: serde_json::Value| tool_tag(name, Some(&input));
        let bash = |c: &str| tag("Bash", serde_json::json!({ "command": c }));
        for c in ["gh pr diff 123", "gh pr view 12 --comments", "gh pr checkout 7", "bb pr view 3", "bb pr diff 3",
                  "git fetch origin pull/12/head:pr-12", "git diff main...refs/pull/12/head", "gh pr review 12 --comment -b ok"] {
            assert_eq!(bash(c).as_deref(), Some("review"), "{c}");
        }
        // Read-only git ranges and commits are what a local review looks at.
        for c in ["git diff main...feature", "git diff a1b2c3d..e4f5a6b", "git log --oneline 3873aa2c..d677f2cf", "git show 8b2377e6", "git show 8b2377e6 --stat"] {
            assert_eq!(bash(c).as_deref(), Some("review"), "{c}");
        }
        for c in ["git diff", "git fetch origin", "gh issue view 3", "git show --stat", "git log -3"] {
            assert_ne!(bash(c).as_deref(), Some("review"), "{c}");
        }
        assert_eq!(bash("npm test").as_deref(), Some("test"), "a test run stays a test run");
        assert_eq!(bash("cat > review-notes.md <<'EOF'").as_deref(), Some("notes"));
        assert_eq!(bash("sed -i '' 's/a/b/' src/x.rs").as_deref(), Some("write"));
        for p in ["/r/REVIEW.md", "/r/review-notes.txt", "/r/docs/notes.rst"] {
            assert_eq!(tag("Write", serde_json::json!({ "file_path": p })).as_deref(), Some("notes"), "{p}");
        }
        assert_eq!(tag("Edit", serde_json::json!({ "file_path": "/r/src/a.rs" })), None);
    }

    #[test]
    fn shell_reads_are_reads_and_only_the_write_target_decides_notes() {
        let bash = |c: &str| tool_tag("Bash", Some(&serde_json::json!({ "command": c })));
        for c in ["sed -n '1,40p' src/a.rs", "cat src/a.rs", "grep -n foo src/", "rg foo", "head -20 x", "tail -5 x", "less x", "cat src/a.rs | head -5"] {
            assert_eq!(bash(c).as_deref(), Some("read"), "{c}");
        }
        assert_eq!(bash("sed -i '' 's/a/b/' src/x.rs README.md").as_deref(), Some("write"), "one target is source");
        assert_eq!(bash("cat > src/a.rs <<'EOF'\nsee requirements.txt\nEOF").as_deref(), Some("write"), "the target is src/a.rs");
        assert_eq!(bash("sed -i '' 's/a/b/' README.md").as_deref(), Some("notes"));
        assert_eq!(bash("cat > review.md <<'EOF'\nok\nEOF").as_deref(), Some("notes"));
    }

    #[test]
    fn a_source_edit_restored_in_the_same_turn_is_scratch() {
        let a = |id: &str, name: &str, input: serde_json::Value, ts: &str| serde_json::json!({"type":"assistant","sessionId":"s","timestamp":ts,
            "message":{"id":id,"role":"assistant","model":"claude-haiku-4-5","usage":{"input_tokens":1,"output_tokens":1},
            "content":[{"type":"tool_use","id":format!("t-{id}"),"name":name,"input":input}]}}).to_string();
        let prompt = |ts: &str| serde_json::json!({"type":"user","sessionId":"s","timestamp":ts,"message":{"role":"user","content":"go"}}).to_string();
        let lines = [
            prompt("2026-10-01T10:00:00.000Z"),
            a("m1", "Bash", serde_json::json!({"command":"sed -i '' 's/x/y/' src/a.rs"}), "2026-10-01T10:00:01.000Z"),
            a("m2", "Edit", serde_json::json!({"file_path":"/repo/src/b.rs"}), "2026-10-01T10:00:02.000Z"),
            a("m3", "Edit", serde_json::json!({"file_path":"/repo/src/kept.rs"}), "2026-10-01T10:00:03.000Z"),
            a("m4", "Bash", serde_json::json!({"command":"npm test"}), "2026-10-01T10:00:04.000Z"),
            a("m5", "Bash", serde_json::json!({"command":"git checkout -- src/a.rs && cp /tmp/b.bak src/b.rs"}), "2026-10-01T10:00:05.000Z"),
            // A new prompt: a restore after it does not reach back.
            a("m6", "Edit", serde_json::json!({"file_path":"/repo/src/c.rs"}), "2026-10-01T10:00:06.000Z"),
            prompt("2026-10-01T10:01:00.000Z"),
            a("m7", "Bash", serde_json::json!({"command":"git restore src/c.rs"}), "2026-10-01T10:01:01.000Z"),
        ].join("\n");
        let p = parse_str("p", &lines);
        let tags: Vec<Option<&str>> = p.tool_calls.iter().map(|t| t.tag.as_deref()).collect();
        // m5 puts files back: it is scratch too, not a new edit.
        assert_eq!(tags, vec![Some("scratch"), Some("scratch"), None, Some("test"), Some("scratch"), None, None]);
    }

    #[test]
    fn a_test_run_is_keyed_by_its_normalised_command_and_directory_as_a_hash() {
        let key = |c: &str, cwd: &str| test_command_key(c, cwd);
        // The same suite in the same directory, with different plumbing: one key.
        let full = key("node --test test/*.test.cjs", "/repo");
        assert!(full.is_some());
        assert_eq!(key("node  --test   test/*.test.cjs 2>&1 | tail -5", "/repo"), full);
        assert_eq!(key("cd /repo && node --test test/*.test.cjs > $TMPDIR/run.txt 2>&1; grep -E fail $TMPDIR/run.txt", "/elsewhere"), full, "cd decides the directory");
        assert_eq!(key("cd sub/.. && node --test test/*.test.cjs", "/repo"), full, "a relative cd joins the cwd, with . and .. resolved");
        assert_eq!(key("cd ./a/../ && node --test test/*.test.cjs", "/repo/"), full);
        assert_eq!(key("(cd /repo && node --test test/*.test.cjs)", "/elsewhere"), full, "a subshell's cd counts");
        // The same command in another repo or worktree is another suite.
        assert_ne!(key("node --test test/*.test.cjs", "/worktree-2"), full);
        assert_ne!(key("cd /fork && node --test test/*.test.cjs", "/repo"), full);
        // A focused run is a different command.
        assert_ne!(key("node --test test/prompt-gate.test.cjs", "/repo"), full);
        assert_eq!(key("ls -la", "/repo"), None, "not a test run");
        // Only a hash is kept, never the command or the directory.
        let k = full.unwrap();
        assert_eq!(k.len(), 16);
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn write_targets_ignore_heredoc_bodies_and_quoted_text() {
        let t = |c: &str| write_targets(c);
        assert_eq!(t("cat > notes.md <<'EOF'\nif a > b then c\nEOF"), vec!["notes.md"]);
        assert!(t("python3 - <<'EOF'\nprint(1 > 0)\nEOF").is_empty(), "a heredoc body is not the command");
        assert!(t("echo 'x > y' | grep x").is_empty(), "inside quotes");
        assert!(t("awk 'c>=0' f").is_empty());
        assert_eq!(t("sed -i '' 's/a b/c/' src/x.rs\necho done more words"), vec!["src/x.rs"], "a newline ends the list");
        assert_eq!(t("tee out.txt <<EOF\nline words here\nEOF"), vec!["out.txt"]);
        assert_eq!(t("npm run build > build.log 2>&1"), vec!["build.log"]);
        assert_eq!(t("echo hi>out.txt"), vec!["out.txt"], "an attached redirect");
        assert!(t("cmd 2>&1 >/dev/null").is_empty());
        assert_eq!(t("cat > a.md <<'EOF'\nbody\nEOF\necho x > b.rs"), vec!["a.md", "b.rs"], "commands after the heredoc still count");
        assert_eq!(t("cp -r src/a.rs dst/a.rs"), vec!["dst/a.rs"], "cp writes its destination");
    }

    #[test]
    fn writes_to_temp_are_scratch_not_source_edits() {
        let bash = |c: &str| tool_tag("Bash", Some(&serde_json::json!({ "command": c })));
        let tool = |p: &str| tool_tag("Write", Some(&serde_json::json!({ "file_path": p })));
        for c in ["cp src/a.rs /tmp/a.bak", "echo x > $TMPDIR/out.txt", "git diff > \"${TMPDIR}/d.patch\"", "ls > /private/tmp/claude-501/l.txt"] {
            assert_eq!(bash(c).as_deref(), Some("scratch"), "{c}");
        }
        assert_eq!(bash("echo x > /tmp/a.txt && echo y > src/b.rs").as_deref(), Some("write"), "one real target");
        for p in ["/tmp/probe.rs", "/private/tmp/x/y.cjs", "/var/folders/91/T/z.py"] {
            assert_eq!(tool(p).as_deref(), Some("scratch"), "{p}");
        }
    }

    #[test]
    fn temp_targets_after_cd_or_through_a_variable_are_scratch() {
        let bash = |c: &str| tool_tag("Bash", Some(&serde_json::json!({ "command": c })));
        for c in ["cd /tmp/work && echo x > out.rs", "T=$TMPDIR; echo x > $T/out.rs", "export T=/tmp/w && cp a.rs ${T}/b.rs",
                  "(cd /private/tmp/p && cat > probe.py <<'EOF'\nprint(1)\nEOF\n)"] {
            assert_eq!(bash(c).as_deref(), Some("scratch"), "{c}");
        }
        assert_eq!(bash("cd /repo && echo x > out.rs").as_deref(), Some("write"));
        // A record whose own working directory is a temp dir writes scratch.
        assert_eq!(tool_tag_in("Bash", Some(&serde_json::json!({ "command": "echo x > out.rs" })), "/tmp/play").as_deref(), Some("scratch"));
    }

    #[test]
    fn a_file_created_and_removed_in_the_same_turn_is_scratch() {
        let a = |id: &str, cmd: &str, ts: &str| serde_json::json!({"type":"assistant","sessionId":"s","timestamp":ts,
            "message":{"id":id,"role":"assistant","model":"claude-haiku-4-5","usage":{"input_tokens":1,"output_tokens":1},
            "content":[{"type":"tool_use","id":format!("t-{id}"),"name":"Bash","input":{"command":cmd}}]}}).to_string();
        let lines = [
            serde_json::json!({"type":"user","sessionId":"s","timestamp":"2026-10-01T10:00:00.000Z","message":{"role":"user","content":"go"}}).to_string(),
            a("m1", "cat > probe.rs <<'EOF'\nfn main(){}\nEOF", "2026-10-01T10:00:01.000Z"),
            a("m2", "echo x > kept.rs", "2026-10-01T10:00:02.000Z"),
            a("m3", "rm -f probe.rs", "2026-10-01T10:00:03.000Z"),
        ].join("\n");
        let tags: Vec<Option<String>> = parse_str("p", &lines).tool_calls.iter().map(|t| t.tag.clone()).collect();
        assert_eq!(tags, vec![Some("scratch".into()), Some("write".into()), None]);
    }

    #[test]
    fn same_path_matches_on_a_slash_boundary_only() {
        assert!(same_path("/repo/src/a.rs", "src/a.rs"));
        assert!(same_path("./src/a.rs", "src/a.rs"));
        assert!(!same_path("src/data.rs", "a.rs"));
        assert!(!same_path("src/xa.rs", "a.rs"));
    }

    #[test]
    fn a_pre_push_range_check_is_git_ops_not_review() {
        let bash = |c: &str| tool_tag("Bash", Some(&serde_json::json!({ "command": c })));
        for c in ["git log origin/main..HEAD", "git log --oneline @{u}..HEAD", "git diff origin/main...HEAD --stat", "git log upstream/main..HEAD"] {
            assert_ne!(bash(c).as_deref(), Some("review"), "{c}");
        }
        assert_eq!(bash("git log --oneline 3873aa2c..d677f2cf").as_deref(), Some("review"));
        assert_eq!(bash("git diff main...feature").as_deref(), Some("review"));
    }

    #[test]
    fn test_runner_detection() {
        for c in ["npm test", "cargo test --offline", "pytest -q", "go test ./...", "npm run test", "node --test test/a.cjs", "cd x && npx vitest run", "make test",
                  "npm run test:focused -- a", "npm run test:renderer", "flutter test", "dart test"] {
            assert!(is_test_command(c), "{c}");
        }
        for c in ["git status", "cat test.txt", "ls tests/", "echo testing"] {
            assert!(!is_test_command(c), "{c}");
        }
    }

    #[test]
    fn skips_malformed_and_unknown_lines() {
        let content = include_str!("../tests/fixtures/malformed.jsonl");
        let p = parse_str("proj-b", content);
        // Only the first assistant line yields a message row; the bad JSON,
        // the summary record, and the message-less assistant are skipped.
        assert_eq!(p.messages.len(), 1);
        assert_eq!(p.messages[0].model.as_deref(), Some("claude-haiku-4-5"));
    }
}
