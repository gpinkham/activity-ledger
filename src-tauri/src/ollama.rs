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
use std::time::Duration;

/// The labels the model may answer with. Anything else is rejected.
pub const MODEL_LABELS: [&str; 10] = [
    "coding", "debugging", "code review", "triage", "research", "review / explore",
    "shell / ops", "chat / planning", "other work", "overhead",
];
pub const PROMPT_VERSION: &str = "label-prompt-v2";
/// The largest response accepted. A label answer is a few hundred bytes; a bigger
/// (or claimed bigger) body is refused before it is read into memory.
pub const MAX_RESPONSE_BYTES: usize = 1 << 20;
pub const SYSTEM_PROMPT: &str = "You label one stretch of AI coding-agent work. Answer with JSON only: {\"label\": \"<one of the labels>\"}. Labels: coding, debugging, code review, triage, research, review / explore, shell / ops, chat / planning, other work, overhead.";

#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// Path prefix from the URL ("" for a bare host).
    pub base: String,
    /// The user allowed a non-loopback Ollama. When false, post_json also refuses
    /// a resolved address that is not loopback.
    pub remote_ok: bool,
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
    Ok(Endpoint { host, port, base: path.trim_end_matches('/').to_string(), remote_ok: allow_remote })
}

/// POST a JSON body and return the response body. Errors on a connect failure,
/// a timeout (connect, write or read), or a non-2xx status.
pub fn post_json(ep: &Endpoint, path: &str, body: &str, timeout: Duration) -> Result<String, String> {
    let host = ep.host.trim_start_matches('[').trim_end_matches(']');
    let addr = (host, ep.port).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    if !ep.remote_ok && !addr.ip().is_loopback() {
        return Err(format!("{} resolved to {}, which is not this machine", ep.host, addr.ip()));
    }
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let req = format!(
        "POST {}{} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        ep.base, path, ep.host, ep.port, body.len(), body
    );
    stream.write_all(req.as_bytes()).map_err(|e| format!("write: {e}"))?;
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status).map_err(|e| format!("read: {e}"))?;
    let code: u16 = status.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| format!("bad status line: {}", status.trim()))?;
    let mut length: Option<usize> = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| format!("read: {e}"))?;
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
            reader.read_line(&mut size).map_err(|e| format!("read: {e}"))?;
            let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16).map_err(|_| "bad chunk size".to_string())?;
            if n == 0 { break; }
            if out.len().saturating_add(n) > MAX_RESPONSE_BYTES { return Err(too_large()); }
            let mut buf = vec![0u8; n];
            reader.read_exact(&mut buf).map_err(|e| format!("read: {e}"))?;
            out.extend_from_slice(&buf);
            let mut crlf = String::new();
            reader.read_line(&mut crlf).ok();
        }
    } else if let Some(n) = length {
        if n > MAX_RESPONSE_BYTES { return Err(too_large()); }
        out.resize(n, 0);
        reader.read_exact(&mut out).map_err(|e| format!("read: {e}"))?;
    } else {
        reader.by_ref().take(MAX_RESPONSE_BYTES as u64 + 1).read_to_end(&mut out).map_err(|e| format!("read: {e}"))?;
        if out.len() > MAX_RESPONSE_BYTES { return Err(too_large()); }
    }
    if !(200..300).contains(&code) {
        return Err(format!("Ollama answered {code}"));
    }
    String::from_utf8(out).map_err(|_| "response is not UTF-8".to_string())
}

/// Ask the model for one label. Ok only for a label in MODEL_LABELS.
pub fn chat_label(ep: &Endpoint, model: &str, user: &str, num_ctx: i64, timeout: Duration) -> Result<String, String> {
    let body = json!({
        "model": model,
        "stream": false,
        "format": "json",
        "options": { "num_ctx": num_ctx, "num_predict": 64, "temperature": 0 },
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": user },
        ],
    })
    .to_string();
    let resp = post_json(ep, "/api/chat", &body, timeout)?;
    let v: Value = serde_json::from_str(&resp).map_err(|_| "response is not JSON".to_string())?;
    let content = v.pointer("/message/content").and_then(|c| c.as_str()).ok_or("no message in the response")?;
    let answer: Value = serde_json::from_str(content.trim()).map_err(|_| "the model did not answer JSON".to_string())?;
    let label = answer.get("label").and_then(|l| l.as_str()).map(|l| l.trim().to_lowercase()).ok_or("no label in the answer")?;
    if MODEL_LABELS.contains(&label.as_str()) { Ok(label) } else { Err(format!("not a known label: {label}")) }
}

#[cfg(test)]
mod tests {
    use super::*;
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
