//! The optional labelling pass's only network call: a local Ollama.
//!
//! Day-job data must not leave the machine, so the URL is checked before any
//! request: loopback only (127.0.0.0/8, ::1, localhost) unless the user ticked
//! "Allow a remote Ollama", and plain http:// only. Never `claude -p`, never a
//! cloud API. The client is a minimal HTTP/1.1 POST over a std TcpStream, so
//! there is no new dependency and no TLS.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The labels the model may answer with. Anything else is rejected.
pub const MODEL_LABELS: [&str; 10] = [
    "coding", "debugging", "code review", "triage", "research", "review / explore",
    "shell / ops", "chat / planning", "other work", "overhead",
];
pub const PROMPT_VERSION: &str = "label-prompt-v2";
/// The largest response accepted. A label answer is a few hundred bytes; a bigger
/// (or claimed bigger) body is refused before it is read into memory.
pub const MAX_RESPONSE_BYTES: usize = 1 << 20;
/// How long to wait for the model to load. The first call after Ollama starts,
/// or after it unloads an idle model, reads the weights from disk and sets up
/// the context; on a CPU-only Mac that can take minutes.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// How long to wait for one label once the model is loaded. Ollama sends nothing
/// until the answer is ready (no streaming), so this is the whole answer time.
pub const LABEL_TIMEOUT: Duration = Duration::from_secs(300);
/// The error when the user pressed Stop.
pub const STOPPED: &str = "stopped";
/// How often a waiting request looks at the Stop flag.
const STOP_POLL: Duration = Duration::from_millis(200);
/// Connecting is quick or it is not going to happen.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How much of an error body to show.
const ERROR_EXCERPT_CHARS: usize = 200;
pub const SYSTEM_PROMPT: &str = "You label one stretch of AI coding-agent work. Answer with JSON only: {\"label\": \"<one of the labels>\"}. Labels: coding, debugging, code review, triage, research, review / explore, shell / ops, chat / planning, other work, overhead.";

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// Path prefix from the URL ("" for a bare host).
    pub base: String,
    /// The user allowed a non-loopback Ollama. When false, post_json also refuses
    /// a resolved address that is not loopback.
    pub remote_ok: bool,
    /// Set by Stop. A request that is waiting ends within STOP_POLL.
    pub stop: Arc<AtomicBool>,
}

/// `localhost`, or a literal loopback IP (127.0.0.0/8, ::1). A name that merely
/// looks like one (127.attacker.example) resolves by DNS and is not loopback.
fn is_loopback_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost") || h.parse::<std::net::IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// Check the configured URL. Ok only for http://, and only for a loopback host
/// unless `allow_remote` is set. The error says why, for the settings card.
pub fn check_url(url: &str, allow_remote: bool) -> Result<Endpoint, String> {
    let url = url.trim();
    if url.starts_with("https://") {
        return Err("https is not supported: the client speaks plain http to a local Ollama".into());
    }
    let rest = url.strip_prefix("http://").ok_or_else(|| format!("not an http:// URL: {url}"))?;
    let (hostport, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, String::new()));
    let (host, port) = if let Some(end) = hostport.strip_prefix('[').and_then(|r| r.find(']')) {
        let host = &hostport[..end + 2];
        let port = hostport[end + 2..].strip_prefix(':').map(|p| p.parse::<u16>()).transpose().map_err(|_| "bad port".to_string())?;
        (host.to_string(), port.unwrap_or(80))
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse::<u16>().map_err(|_| "bad port".to_string())?),
            None => (hostport.to_string(), 80),
        }
    };
    if host.is_empty() {
        return Err("no host in the Ollama URL".into());
    }
    if !is_loopback_host(&host) && !allow_remote {
        return Err(format!("{host} is not this machine: tick \"Allow a remote Ollama\" to send segment excerpts there"));
    }
    Ok(Endpoint { host, port, base: path.trim_end_matches('/').to_string(), remote_ok: allow_remote, stop: Arc::default() })
}

/// An io error as the settings card should show it: a refused connection and a
/// timeout each say what they mean, since both used to read as "not reachable".
fn io_error(stage: &str, e: &std::io::Error, ep: &Endpoint, timeout: Duration) -> String {
    use std::io::ErrorKind::*;
    if e.get_ref().is_some_and(|inner| inner.is::<Stopped>()) {
        return STOPPED.into();
    }
    match e.kind() {
        ConnectionRefused => format!("nothing is listening at {}:{} (connection refused). Is Ollama running?", ep.host, ep.port),
        // A read timeout is WouldBlock on macOS and TimedOut on Linux.
        WouldBlock | TimedOut => format!("no answer from Ollama within {} s (timed out while {stage})", timeout.as_secs_f32().round() as u64),
        _ => format!("{stage}: {e}"),
    }
}

#[derive(Debug)]
struct Stopped;
impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str(STOPPED) }
}
impl std::error::Error for Stopped {}

/// The response side of a request: every read shares one deadline, so a server
/// that sends a byte now and then cannot stretch the wait, and each read waits
/// at most STOP_POLL before looking at the Stop flag again.
struct Answer<'a> {
    stream: TcpStream,
    deadline: Instant,
    stop: &'a AtomicBool,
}

impl Read for Answer<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return Err(std::io::Error::other(Stopped));
            }
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            self.stream.set_read_timeout(Some(left.min(STOP_POLL)))?;
            match self.stream.read(buf) {
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                r => return r,
            }
        }
    }
}

/// A non-2xx answer: the status, and Ollama's own error text (its body is
/// {"error": "..."}), or the start of whatever else the body holds.
fn http_error(code: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let said = serde_json::from_str::<Value>(&text).ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| text.trim().to_string());
    let excerpt: String = said.chars().take(ERROR_EXCERPT_CHARS).collect();
    let more = if said.chars().count() > ERROR_EXCERPT_CHARS { "..." } else { "" };
    if excerpt.is_empty() { format!("Ollama answered {code}") } else { format!("Ollama answered {code}: {excerpt}{more}") }
}

/// POST a JSON body and return the response body. Errors on a connect failure,
/// a timeout (`timeout` is the whole wait for the answer, however it arrives),
/// Stop, or a non-2xx status, each with its own message.
pub fn post_json(ep: &Endpoint, path: &str, body: &str, timeout: Duration) -> Result<String, String> {
    let host = ep.host.trim_start_matches('[').trim_end_matches(']');
    let addr = (host, ep.port).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    if !ep.remote_ok && !addr.ip().is_loopback() {
        return Err(format!("{} resolved to {}, which is not this machine", ep.host, addr.ip()));
    }
    if ep.stop.load(Ordering::SeqCst) {
        return Err(STOPPED.into());
    }
    let connect = timeout.min(CONNECT_TIMEOUT);
    let mut stream = TcpStream::connect_timeout(&addr, connect).map_err(|e| io_error("connecting", &e, ep, connect))?;
    let deadline = Instant::now() + timeout;
    stream.set_write_timeout(Some(timeout)).ok();
    let req = format!(
        "POST {}{} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        ep.base, path, ep.host, ep.port, body.len(), body
    );
    stream.write_all(req.as_bytes()).map_err(|e| io_error("sending", &e, ep, timeout))?;
    let mut reader = BufReader::new(Answer { stream, deadline, stop: &ep.stop });
    let mut status = String::new();
    reader.read_line(&mut status).map_err(|e| io_error("waiting for the answer", &e, ep, timeout))?;
    let code: u16 = status.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| format!("bad status line: {}", status.trim()))?;
    let mut length: Option<usize> = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| io_error("waiting for the answer", &e, ep, timeout))?;
        let line = line.trim_end();
        if line.is_empty() { break; }
        let (k, v) = line.split_once(':').unwrap_or((line, ""));
        match k.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = Some(v.trim().parse::<u64>().ok().map(|n| usize::try_from(n).unwrap_or(usize::MAX)).unwrap_or(usize::MAX)),
            "transfer-encoding" => chunked = v.to_ascii_lowercase().contains("chunked"),
            _ => {}
        }
    }
    let too_large = || format!("response too large (over {MAX_RESPONSE_BYTES} bytes)");
    let mut out = Vec::new();
    if chunked {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).map_err(|e| io_error("waiting for the answer", &e, ep, timeout))?;
            let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16).map_err(|_| "bad chunk size".to_string())?;
            if n == 0 { break; }
            if out.len().saturating_add(n) > MAX_RESPONSE_BYTES { return Err(too_large()); }
            let mut buf = vec![0u8; n];
            reader.read_exact(&mut buf).map_err(|e| io_error("waiting for the answer", &e, ep, timeout))?;
            out.extend_from_slice(&buf);
            let mut crlf = String::new();
            reader.read_line(&mut crlf).ok();
        }
    } else if let Some(n) = length {
        if n > MAX_RESPONSE_BYTES { return Err(too_large()); }
        out.resize(n, 0);
        reader.read_exact(&mut out).map_err(|e| io_error("waiting for the answer", &e, ep, timeout))?;
    } else {
        reader.by_ref().take(MAX_RESPONSE_BYTES as u64 + 1).read_to_end(&mut out).map_err(|e| io_error("waiting for the answer", &e, ep, timeout))?;
        if out.len() > MAX_RESPONSE_BYTES { return Err(too_large()); }
    }
    if !(200..300).contains(&code) {
        return Err(http_error(code, &out));
    }
    String::from_utf8(out).map_err(|_| "response is not UTF-8".to_string())
}

/// How much of a bad answer to quote in the error.
const ANSWER_EXCERPT_CHARS: usize = 120;

/// Ask the model for one label. Ok only for a label in MODEL_LABELS.
///
/// Thinking is turned off (`think: false`): a thinking model such as qwen3
/// otherwise spends the few answer tokens on its reasoning and returns no
/// answer. The answer is constrained by a JSON schema (Ollama's structured
/// outputs) to one of the labels, which small models need. The answer is still
/// read leniently, since older Ollama versions may ignore either setting.
pub fn chat_label(ep: &Endpoint, model: &str, user: &str, num_ctx: i64, timeout: Duration) -> Result<String, String> {
    let body = json!({
        "model": model,
        "stream": false,
        "think": false,
        "format": {
            "type": "object",
            "properties": { "label": { "type": "string", "enum": MODEL_LABELS } },
            "required": ["label"],
        },
        "options": { "num_ctx": num_ctx, "num_predict": 64, "temperature": 0 },
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": user },
        ],
    })
    .to_string();
    let resp = post_json(ep, "/api/chat", &body, timeout)?;
    let v: Value = serde_json::from_str(&resp).map_err(|_| "response is not JSON".to_string())?;
    let message = v.get("message").ok_or("no message in the response")?;
    let content = message.get("content").and_then(|c| c.as_str()).unwrap_or("");
    let thought = message.get("thinking").and_then(|t| t.as_str()).is_some_and(|t| !t.trim().is_empty());
    let label = parse_label(content, thought)?;
    if MODEL_LABELS.contains(&label.as_str()) { Ok(label) } else { Err(format!("not a known label: {label}")) }
}

/// The label in a model's answer. Accepts the answer as asked ({"label": ...}),
/// and also wrapped in a <think> block, a code fence or prose: the first JSON
/// object with a "label" string wins. The error quotes the start of the answer.
fn parse_label(content: &str, thought: bool) -> Result<String, String> {
    let mut text = content.to_string();
    while let Some(start) = text.find("<think>") {
        match text[start..].find("</think>") {
            Some(end) => text.replace_range(start..start + end + "</think>".len(), ""),
            None => text.truncate(start),
        }
    }
    let text = text.trim();
    if text.is_empty() {
        return Err(if thought || content.contains("<think>") {
            "the model was still thinking and gave no answer (is it a thinking model on an Ollama too old to turn thinking off?)".into()
        } else {
            "the model gave an empty answer".into()
        });
    }
    for (i, _) in text.match_indices('{') {
        let mut objects = serde_json::Deserializer::from_str(&text[i..]).into_iter::<Value>();
        if let Some(Ok(v)) = objects.next() {
            if let Some(label) = v.get("label").and_then(|l| l.as_str()) {
                return Ok(label.trim().to_lowercase());
            }
        }
    }
    let excerpt: String = text.chars().take(ANSWER_EXCERPT_CHARS).collect();
    let more = if text.chars().count() > ANSWER_EXCERPT_CHARS { "..." } else { "" };
    Err(format!("the model did not answer JSON with a label: \"{excerpt}{more}\""))
}

/// Load the model before the first label, with its own long wait. Ollama loads
/// a model when it gets a chat with no messages; the context size must match
/// the label requests' or Ollama loads it again for them.
pub fn load_model(ep: &Endpoint, model: &str, num_ctx: i64, timeout: Duration) -> Result<(), String> {
    let body = json!({ "model": model, "stream": false, "messages": [], "options": { "num_ctx": num_ctx } }).to_string();
    post_json(ep, "/api/chat", &body, timeout).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::net::TcpListener;

    /// A fake Ollama that answers one request with `response` (raw HTTP), or never
    /// answers when `response` is None. Returns its URL.
    fn fake(response: Option<String>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let _ = s.read(&mut buf);
                match response {
                    Some(r) => { let _ = s.write_all(r.as_bytes()); }
                    None => std::thread::sleep(Duration::from_secs(3)),
                }
            }
        });
        format!("http://127.0.0.1:{port}")
    }
    fn reply(label: &str) -> String {
        let body = json!({"message": {"role": "assistant", "content": format!("{{\"label\": \"{label}\"}}")}}).to_string();
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", body.len(), body)
    }
    const T: Duration = Duration::from_millis(1500);

    /// A fake Ollama that answers one request per step, in order: it waits the
    /// step's delay (a model loading), then writes the raw response. Every
    /// request it reads is sent back on the channel.
    fn fake_seq(steps: Vec<(Duration, String)>) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (delay, response) in steps {
                let Ok((mut s, _)) = listener.accept() else { return };
                let mut buf = [0u8; 16384];
                let n = s.read(&mut buf).unwrap_or(0);
                let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
                std::thread::sleep(delay);
                let _ = s.write_all(response.as_bytes());
            }
        });
        (format!("http://127.0.0.1:{port}"), rx)
    }
    fn http(code: &str, body: &str) -> String {
        format!("HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", body.len(), body)
    }

    fn answer(message: Value) -> String {
        http("200 OK", &json!({"model": "m", "message": message, "done": true}).to_string())
    }

    #[test]
    fn the_request_turns_thinking_off_and_constrains_the_answer_to_a_label() {
        let (url, rx) = fake_seq(vec![(Duration::ZERO, reply("coding"))]);
        chat_label(&check_url(&url, false).unwrap(), "qwen3:8b", "s", 4096, T).unwrap();
        let req = rx.recv().unwrap();
        let body: Value = serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["think"], false, "a thinking model would spend its answer tokens thinking");
        let schema = &body["format"];
        assert_eq!(schema["type"], "object", "a JSON schema (structured output), not just \"json\": {schema}");
        assert_eq!(schema["required"], json!(["label"]));
        assert_eq!(schema["properties"]["label"]["enum"], json!(MODEL_LABELS));
    }

    #[test]
    fn reads_the_label_from_each_shape_a_model_answers_in() {
        for content in [
            r#"{"label": "coding"}"#,                                                     // llama2
            "<think>\nThe segment edits files, so\n</think>\n\n{\"label\": \"coding\"}",   // qwen3, thinking inline
            "<think>\n\n</think>\n\n{\"label\":\"coding\"}",                               // qwen3, empty think block
            "```json\n{\"label\": \"coding\"}\n```",                                       // fenced
            "Here is the label:\n{\"label\": \"coding\"}\nIt edits files.",                // prose around it
            r#"{"label": " Coding "}"#,                                                   // case and spaces
            r#"{"reason": "{not json", "x": 1} {"label": "coding"}"#,                     // an earlier object without a label
        ] {
            let (url, _rx) = fake_seq(vec![(Duration::ZERO, answer(json!({"role": "assistant", "content": content})))]);
            assert_eq!(chat_label(&check_url(&url, false).unwrap(), "m", "s", 4096, T), Ok("coding".into()), "{content}");
        }
        // Newer Ollama returns a thinking model's reasoning in its own field.
        let (url, _rx) = fake_seq(vec![(Duration::ZERO, answer(json!({"content": "{\"label\":\"coding\"}", "thinking": "edits files"})))]);
        assert_eq!(chat_label(&check_url(&url, false).unwrap(), "m", "s", 4096, T).unwrap(), "coding");
    }

    #[test]
    fn a_model_that_only_thought_is_told_apart_from_one_that_answered_badly() {
        let (url, _rx) = fake_seq(vec![(Duration::ZERO, answer(json!({"content": "", "thinking": "Okay, let me look at the tools used"})))]);
        let err = chat_label(&check_url(&url, false).unwrap(), "qwen3:8b", "s", 4096, T).unwrap_err();
        assert!(err.contains("thinking") && err.contains("no answer"), "{err}");
        let (url, _rx) = fake_seq(vec![(Duration::ZERO, answer(json!({"content": "<think>\nOkay, the user wants"})))]);
        let err = chat_label(&check_url(&url, false).unwrap(), "m", "s", 4096, T).unwrap_err();
        assert!(err.contains("thinking") && err.contains("no answer"), "an unclosed think block: {err}");
    }

    #[test]
    fn a_non_json_answer_quotes_what_the_model_said() {
        let said = format!("I would say this is coding. {}", "More words. ".repeat(40));
        let (url, _rx) = fake_seq(vec![(Duration::ZERO, answer(json!({"content": said})))]);
        let err = chat_label(&check_url(&url, false).unwrap(), "gemma4:e2b", "s", 4096, T).unwrap_err();
        assert!(err.contains("did not answer JSON") && err.contains("I would say this is coding"), "{err}");
        assert!(err.len() < 250, "an excerpt, not the whole answer: {}", err.len());
        let (url, _rx) = fake_seq(vec![(Duration::ZERO, answer(json!({"content": "{\"label\": \"code_review\"}"})))]);
        let err = chat_label(&check_url(&url, false).unwrap(), "m", "s", 4096, T).unwrap_err();
        assert!(err.contains("not a known label: code_review"), "{err}");
    }

    #[test]
    fn a_server_that_trickles_bytes_cannot_run_past_the_timeout() {
        // Sends a header byte every 50 ms, forever: no single read waits long.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nX-Slow: ");
                for _ in 0..100 {
                    if s.write_all(b"a").is_err() { return; }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        });
        let ep = check_url(&format!("http://127.0.0.1:{port}"), false).unwrap();
        let started = std::time::Instant::now();
        let err = post_json(&ep, "/api/chat", "{}", Duration::from_millis(400)).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        assert!(started.elapsed() < Duration::from_millis(1500), "the whole answer has one deadline: {:?}", started.elapsed());
    }

    #[test]
    fn stop_ends_a_request_that_is_still_waiting() {
        let (url, _rx) = fake_seq(vec![(Duration::from_secs(5), reply("coding"))]);
        let ep = check_url(&url, false).unwrap();
        let stop = ep.stop.clone();
        std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(300)); stop.store(true, Ordering::SeqCst); });
        let started = std::time::Instant::now();
        let err = load_model(&ep, "m", 4096, Duration::from_secs(10)).unwrap_err();
        assert_eq!(err, STOPPED);
        assert!(started.elapsed() < Duration::from_millis(1500), "Stop does not wait for the load: {:?}", started.elapsed());
    }

    #[test]
    fn a_slow_answer_is_reported_as_a_timeout_not_as_unreachable() {
        let (url, _rx) = fake_seq(vec![(Duration::from_millis(900), reply("coding"))]);
        let ep = check_url(&url, false).unwrap();
        let err = chat_label(&ep, "m", "s", 4096, Duration::from_millis(200)).unwrap_err();
        assert!(err.contains("timed out") && err.contains("no answer"), "{err}");
    }

    #[test]
    fn the_model_is_loaded_first_with_its_own_long_wait_then_labels_work() {
        // The load answers after longer than a label request may wait.
        let load = http("200 OK", r#"{"model":"m","done":true,"done_reason":"load"}"#);
        let (url, rx) = fake_seq(vec![(Duration::from_millis(700), load), (Duration::ZERO, reply("coding"))]);
        let ep = check_url(&url, false).unwrap();
        load_model(&ep, "m", 4096, Duration::from_secs(5)).unwrap();
        let req = rx.recv().unwrap();
        let body: Value = serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["model"], "m");
        assert_eq!(body["messages"], json!([]), "an empty chat only loads the model");
        assert_eq!(body["options"]["num_ctx"], 4096, "loaded with the context the labels use, so no reload");
        assert_eq!(chat_label(&ep, "m", "s", 4096, Duration::from_millis(500)).unwrap(), "coding");
    }

    #[test]
    fn an_http_error_names_the_status_and_what_ollama_said() {
        let missing = http("404 Not Found", r#"{"error":"model \"qwen3:32b\" not found, try pulling it first"}"#);
        let (url, _rx) = fake_seq(vec![(Duration::ZERO, missing)]);
        let err = load_model(&check_url(&url, false).unwrap(), "qwen3:32b", 4096, T).unwrap_err();
        assert!(err.contains("404") && err.contains("not found, try pulling it first"), "{err}");
        // A body that is not Ollama's JSON is quoted, cut short.
        let (url, _rx) = fake_seq(vec![(Duration::ZERO, http("500 Internal Server Error", &"boom ".repeat(200)))]);
        let err = chat_label(&check_url(&url, false).unwrap(), "m", "s", 4096, T).unwrap_err();
        assert!(err.contains("500") && err.contains("boom"), "{err}");
        assert!(err.len() < 400, "the body is an excerpt: {}", err.len());
    }

    #[test]
    fn nothing_listening_is_reported_as_connection_refused() {
        let port = { let l = TcpListener::bind("127.0.0.1:0").unwrap(); l.local_addr().unwrap().port() };
        let err = chat_label(&check_url(&format!("http://127.0.0.1:{port}"), false).unwrap(), "m", "s", 4096, T).unwrap_err();
        assert!(err.contains("connection refused") && err.contains("Is Ollama running"), "{err}");
    }

    #[test]
    fn only_loopback_unless_remote_is_allowed_and_only_http() {
        for ok in ["http://127.0.0.1:11434", "http://localhost:11434", "http://[::1]:11434", "http://127.0.0.5:8080/ollama"] {
            assert!(check_url(ok, false).is_ok(), "{ok}");
        }
        let err = check_url("http://10.0.0.5:11434", false).unwrap_err();
        assert!(err.contains("Allow a remote Ollama"), "{err}");
        assert!(check_url("http://ollama.example.com:11434", false).is_err());
        assert!(check_url("http://10.0.0.5:11434", true).is_ok(), "allowed when ticked");
        // A NAME that looks like a loopback address is not one: it resolves by DNS.
        for name in ["http://127.attacker.example.com:11434", "http://127.0.0.1.nip.io:11434"] {
            assert!(check_url(name, false).is_err(), "{name}");
        }
        assert!(!check_url("http://127.0.0.1:11434", false).unwrap().remote_ok);
        assert!(check_url("https://127.0.0.1:11434", true).is_err(), "no TLS");
        assert!(check_url("ftp://x", true).is_err());
        assert_eq!(check_url("http://127.0.0.1:8080/ollama/", false).unwrap().base, "/ollama");
    }

    #[test]
    fn asks_for_a_label_and_validates_it() {
        let ep = check_url(&fake(Some(reply("code review"))), false).unwrap();
        assert_eq!(chat_label(&ep, "m", "summary", 4096, T).unwrap(), "code review");
        let ep = check_url(&fake(Some(reply("making coffee"))), false).unwrap();
        assert!(chat_label(&ep, "m", "summary", 4096, T).is_err(), "a label outside the list");
    }

    #[test]
    fn a_response_larger_than_the_cap_is_refused_without_allocating_it() {
        let ep = check_url(&fake(Some("HTTP/1.1 200 OK\r\nContent-Length: 999999999999\r\n\r\n{}".into())), false).unwrap();
        assert!(post_json(&ep, "/api/chat", "{}", T).unwrap_err().contains("too large"));
        let big = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n", MAX_RESPONSE_BYTES + 1);
        let ep = check_url(&fake(Some(big)), false).unwrap();
        assert!(post_json(&ep, "/api/chat", "{}", T).unwrap_err().contains("too large"));
        let body = "x".repeat(MAX_RESPONSE_BYTES + 10);
        let ep = check_url(&fake(Some(format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}"))), false).unwrap();
        assert!(post_json(&ep, "/api/chat", "{}", T).unwrap_err().contains("too large"), "no length given");
    }

    #[test]
    fn reads_a_chunked_response() {
        let body = json!({"message": {"content": "{\"label\":\"coding\"}"}}).to_string();
        let (a, b) = body.split_at(10);
        let raw = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n", a.len(), b.len());
        let ep = check_url(&fake(Some(raw)), false).unwrap();
        assert_eq!(chat_label(&ep, "m", "s", 4096, T).unwrap(), "coding");
    }

    #[test]
    fn a_down_or_silent_or_failing_server_is_an_error_not_a_hang() {
        // Nothing listening.
        let port = { let l = TcpListener::bind("127.0.0.1:0").unwrap(); l.local_addr().unwrap().port() };
        let ep = check_url(&format!("http://127.0.0.1:{port}"), false).unwrap();
        assert!(chat_label(&ep, "m", "s", 4096, T).is_err());
        // Accepts, never answers: the read timeout ends it.
        let started = std::time::Instant::now();
        let ep = check_url(&fake(None), false).unwrap();
        assert!(chat_label(&ep, "m", "s", 4096, Duration::from_millis(300)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        // A 500.
        let ep = check_url(&fake(Some("HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".into())), false).unwrap();
        assert!(chat_label(&ep, "m", "s", 4096, T).unwrap_err().contains("500"));
    }
}
