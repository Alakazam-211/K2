//! Tauri-side HTTP client for the k2so-daemon.
//!
//! The daemon exposes a token-authed HTTP server on `127.0.0.1:<random>` —
//! see `crates/k2so-daemon/src/main.rs`. This module is the Tauri app's
//! counterpart: it discovers the daemon's port + token via the conventional
//! `~/.k2/daemon.port` / `~/.k2/daemon.token` files and wraps
//! `reqwest::blocking` calls so command handlers in `src-tauri` can proxy
//! state-mutating work through the daemon instead of running it in-process.
//!
//! Scope of this commit: **connection management + the two endpoints the
//! daemon serves today (`/ping`, `/status`)**. Additional endpoints (state
//! proxies, scheduler calls, etc.) land in later commits as the daemon
//! grows them.
//!
//! Design choices:
//! - **Blocking reqwest** matches the existing K2SO HTTP story (llm::download
//!   + push adapters both use `reqwest::blocking`). No tokio-runtime
//!   ceremony for the handful of ops we make per command.
//! - **Token loaded lazily** — we re-read the port/token files on every
//!   construction because the daemon can be restarted (launchd KeepAlive)
//!   and its port + token rotate. Holding a stale client is a footgun.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

/// Minimal JSON shape returned by `GET /status` on the daemon.
/// Keep in lockstep with `crates/k2so-daemon/src/main.rs::send_response`
/// body composition.
#[derive(Debug, Clone, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub uptime_secs: u64,
    pub pid: u32,
    pub port: u16,
}

/// Holds the resolved daemon base URL + token, loaded on construction.
/// Cheap to create — two tiny file reads. Create one per command-handler
/// call; don't cache across commands.
pub struct DaemonClient {
    /// Full scheme+authority, NO trailing slash (e.g.
    /// `http://127.0.0.1:58211` or `https://reggie.k2.dev`). All request
    /// URLs are `format!("{base}{path}...")`.
    base: String,
    token: String,
    http: reqwest::blocking::Client,
}

impl DaemonClient {
    /// Resolve a ready-to-use client.
    ///
    /// Reads `~/.k2/daemon.port` + `~/.k2/daemon.token` and targets the
    /// local bundled daemon at `http://127.0.0.1:<port>`. `Err` when either
    /// file is missing or malformed — caller's responsibility to trigger
    /// `launchctl load` and retry.
    pub fn try_connect() -> Result<Self, String> {
        Self::try_connect_in(&k2_home_dir()?)
    }

    /// [`Self::try_connect`] against an explicit `.k2` dir (tests point it
    /// at a temp dir, never the live `~/.k2`).
    pub fn try_connect_in(k2_dir: &std::path::Path) -> Result<Self, String> {
        // Local bundled daemon: discover port + token from disk.
        let port = read_port(&k2_dir.join("daemon.port"))?;
        let token = read_token(&k2_dir.join("daemon.token"))?;
        Self::at(format!("http://127.0.0.1:{port}"), token)
    }

    fn at(base: String, token: String) -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("build http client: {e}"))?;
        Ok(Self { base, token, http })
    }

    /// A client for a fake daemon in tests ([`test_support::FakeDaemon`]).
    #[cfg(test)]
    pub(crate) fn for_test(base: &str, token: &str) -> Self {
        Self::at(base.to_string(), token.to_string()).expect("build test client")
    }


    /// Hit a `/cli/*` route on the daemon, returning the raw response
    /// body (the CLI text/JSON the daemon emits). `params` are
    /// percent-encoded into the query string alongside the auth
    /// token. Any non-2xx status is surfaced as Err with the body.
    pub fn cli_get(&self, path: &str, params: &[(&str, &str)]) -> Result<String, String> {
        self.cli_query(false, path, params)
    }

    /// POST twin of [`Self::cli_get`] for routes that change state: the
    /// daemon answers GET with 405 on them (security patch 0.44.4). Same
    /// query-string params, empty body.
    pub fn cli_post_query(&self, path: &str, params: &[(&str, &str)]) -> Result<String, String> {
        self.cli_query(true, path, params)
    }

    fn cli_query(&self, post: bool, path: &str, params: &[(&str, &str)]) -> Result<String, String> {
        let mut url = format!("{}{}?token={}", self.base, path, self.token);
        for (k, v) in params {
            url.push('&');
            url.push_str(&pct_encode(k));
            url.push('=');
            url.push_str(&pct_encode(v));
        }
        let request = if post {
            self.http.post(&url).body("")
        } else {
            self.http.get(&url)
        };
        let response = request
            .send()
            .map_err(|e| format!("daemon {path}: {e}"))?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(format!("daemon {path} {}: {body}", status.as_u16()));
        }
        Ok(body)
    }

    /// POST a JSON body to a `/cli/*` route. `body` is serialized as
    /// `application/json`; the auth token rides in the query string
    /// (same wire shape `cli_get` uses, so a single daemon-token
    /// rotation invalidates GET and POST identically). Returns the
    /// raw response body on 2xx; any non-2xx is surfaced as `Err`.
    pub fn cli_post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<String, String> {
        let url = format!("{}{}?token={}", self.base, path, self.token);
        let response = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .map_err(|e| format!("daemon {path}: {e}"))?;
        let status = response.status();
        let resp_body = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(format!("daemon {path} {}: {resp_body}", status.as_u16()));
        }
        Ok(resp_body)
    }

    /// Convenience: GET a /cli/* route and decode the JSON body into `T`.
    /// Returns the same error shape as `cli_get` for HTTP failures and
    /// adds a serde decoding error on success-with-bad-body.
    pub fn cli_get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T, String> {
        let body = self.cli_get(path, params)?;
        serde_json::from_str(&body).map_err(|e| format!("decode {path}: {e}: body={body}"))
    }

    /// Convenience: POST a JSON body and decode the JSON response into `T`.
    pub fn cli_post_json_decode<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, String> {
        let resp = self.cli_post_json(path, body)?;
        serde_json::from_str(&resp).map_err(|e| format!("decode {path}: {e}: body={resp}"))
    }

    /// Scheme + authority this client talks to (`http://127.0.0.1:<port>`
    /// for [`DaemonClient::try_connect`]).
    /// GET a thin-client mirror route and return its JSON reply. Mirrors
    /// are the `/cli/*` routes that run the k2-core fn a Tauri command used
    /// to run in this process (0.46.4); see [`mirror_result`] for errors.
    pub fn mirror_get(&self, path: &str, params: &[(&str, &str)]) -> Result<serde_json::Value, String> {
        let mut url = format!("{}{}?token={}", self.base, path, self.token);
        for (k, v) in params {
            url.push('&');
            url.push_str(&pct_encode(k));
            url.push('=');
            url.push_str(&pct_encode(v));
        }
        let response = self.http.get(&url).send().map_err(|e| format!("daemon {path}: {e}"))?;
        let status = response.status().as_u16();
        let body = response.text().unwrap_or_default();
        mirror_result(path, status, &body)
    }

    /// POST a JSON body to a thin-client mirror route and return its JSON
    /// reply (see [`mirror_get`]).
    pub fn mirror_post(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        let url = format!("{}{}?token={}", self.base, path, self.token);
        let response = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .map_err(|e| format!("daemon {path}: {e}"))?;
        let status = response.status().as_u16();
        let resp_body = response.text().unwrap_or_default();
        mirror_result(path, status, &resp_body)
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Hit `GET /status?token=<t>` and decode the JSON body.
    pub fn status(&self) -> Result<DaemonStatus, String> {
        let url = format!("{}/status?token={}", self.base, self.token);
        let response = self
            .http
            .get(url)
            .send()
            .map_err(|e| format!("daemon /status: {e}"))?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(format!("daemon /status {}: {}", status.as_u16(), body));
        }
        serde_json::from_str(&body).map_err(|e| format!("decode /status: {e}: body={body}"))
    }
}

/// Percent-encode a query-string component without pulling a new
/// crate. RFC 3986 unreserved set is letters, digits, `-`, `_`, `.`,
/// `~`. Everything else gets `%HH`-encoded.
/// Run a blocking daemon call (reqwest::blocking, 10 s timeout) on Tauri's
/// blocking pool so an `async` command never holds the main thread while
/// the daemon is slow. A sync Tauri command runs ON the main thread, and
/// reqwest::blocking must not run on the async runtime's own workers
/// either ("Cannot drop a runtime in a context where blocking is not
/// allowed"), so daemon-calling commands are `async` + this.
pub async fn off_main<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("daemon call task failed: {e}"))?
}

/// A mirror route's reply → the command's result. 2xx: the parsed JSON
/// value. Non-2xx with the daemon's `{"error": "<text>"}` body: `<text>`,
/// which is the same string the k2-core fn returned when the command ran it
/// in process, so the renderer sees the message it always saw. Anything
/// else: `daemon <path> <status>: <body>` (transport-level failures).
fn mirror_result(path: &str, status: u16, body: &str) -> Result<serde_json::Value, String> {
    if (200..300).contains(&status) {
        return serde_json::from_str(body).map_err(|e| format!("decode {path}: {e}: body={body}"));
    }
    if status == 400 {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(serde_json::Value::String(msg)) = map.get("error") {
                return Err(msg.clone());
            }
        }
    }
    Err(format!("daemon {path} {status}: {body}"))
}

fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn k2_home_dir() -> Result<PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or_else(|| "home dir unavailable".to_string())?
        .join(".k2"))
}

fn read_port(path: &std::path::Path) -> Result<u16, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    raw.trim()
        .parse::<u16>()
        .map_err(|e| format!("parse port from {}: {e}", path.display()))
}

fn read_token(path: &std::path::Path) -> Result<String, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("empty token file at {}", path.display()));
    }
    Ok(trimmed.to_string())
}

/// A one-shot fake daemon for the forwarder tests: a loopback HTTP listener
/// that answers each request with a canned status + body and records what
/// it received, so a test can check the exact method, path, query and body
/// a Tauri command sends. Never touches `~/.k2` or a real daemon.
#[cfg(test)]
pub(crate) mod test_support {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// One request as the fake daemon saw it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Seen {
        pub method: String,
        /// Path without the query string.
        pub path: String,
        /// Decoded query pairs, in order (the token included).
        pub query: Vec<(String, String)>,
        pub body: String,
    }

    pub struct FakeDaemon {
        pub base: String,
        seen: mpsc::Receiver<Seen>,
    }

    impl FakeDaemon {
        /// Serve `replies.len()` requests, the n-th answered with
        /// `replies[n]` = (status, body), then stop listening.
        pub fn serve(replies: Vec<(u16, &'static str)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake daemon");
            let base = format!("http://{}", listener.local_addr().expect("fake daemon addr"));
            let (tx, seen) = mpsc::channel();
            std::thread::spawn(move || {
                for (status, body) in replies {
                    let (stream, _) = listener.accept().expect("accept");
                    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                    let mut request_line = String::new();
                    reader.read_line(&mut request_line).expect("request line");
                    let mut content_length = 0usize;
                    loop {
                        let mut header = String::new();
                        reader.read_line(&mut header).expect("header");
                        if header == "\r\n" || header.is_empty() {
                            break;
                        }
                        let lower = header.to_ascii_lowercase();
                        if let Some(v) = lower.strip_prefix("content-length:") {
                            content_length = v.trim().parse().expect("content-length");
                        }
                    }
                    let mut req_body = vec![0u8; content_length];
                    reader.read_exact(&mut req_body).expect("request body");
                    let mut parts = request_line.split_whitespace();
                    let method = parts.next().expect("method").to_string();
                    let target = parts.next().expect("target").to_string();
                    let (path, query) = match target.split_once('?') {
                        Some((p, q)) => (p.to_string(), q.to_string()),
                        None => (target.clone(), String::new()),
                    };
                    let query = query
                        .split('&')
                        .filter(|kv| !kv.is_empty())
                        .map(|kv| {
                            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                            (pct_decode(k), pct_decode(v))
                        })
                        .collect();
                    tx.send(Seen {
                        method,
                        path,
                        query,
                        body: String::from_utf8(req_body).expect("utf-8 body"),
                    })
                    .expect("record request");
                    let mut out = stream;
                    write!(
                        out,
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .expect("write reply");
                }
            });
            Self { base, seen }
        }

        /// The next request the fake daemon answered (waits up to 10 s).
        pub fn next(&self) -> Seen {
            self.seen
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("the command never reached the fake daemon")
        }
    }

    fn pct_decode(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).expect("hex");
                out.push(u8::from_str_radix(hex, 16).expect("percent escape"));
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).expect("utf-8 query")
    }

    /// `(k, v)` pairs as owned strings, for comparing with [`Seen::query`].
    pub fn pairs(kv: &[(&str, &str)]) -> Vec<(String, String)> {
        kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The meaty integration test ('boot the daemon, connect, ping,
    // query /status') lives in a manual script rather than here because
    // spawning launchd'd processes from cargo test is fiddly on macOS —
    // the daemon's auth-token file is a per-boot singleton at
    // ~/.k2so/daemon.token, so a test can't pave over a real running
    // daemon without tripping the user's own workflow. Smoke-test
    // workflow is in scripts/perf-load-harness.sh neighborhood.
    //
    // What we CAN test here: the port/token readers and the status
    // deserializer — pure file-I/O + serde.

    #[test]
    fn status_decodes_from_canonical_daemon_body() {
        // Matches the format emitted by k2so-daemon/src/main.rs.
        let json = r#"{"version":"0.33.0-dev","uptime_secs":42,"pid":12345,"port":58211}"#;
        let s: DaemonStatus = serde_json::from_str(json).expect("decode");
        assert_eq!(s.version, "0.33.0-dev");
        assert_eq!(s.uptime_secs, 42);
        assert_eq!(s.pid, 12345);
        assert_eq!(s.port, 58211);
    }

    #[test]
    fn mirror_result_passes_values_and_unwraps_route_errors() {
        assert_eq!(mirror_result("/x", 200, "[1,2]").unwrap(), serde_json::json!([1, 2]));
        assert_eq!(mirror_result("/x", 200, "null").unwrap(), serde_json::Value::Null);
        assert_eq!(mirror_result("/x", 200, "7").unwrap(), serde_json::json!(7));
        // A k2-core Err(e) comes back as 400 {"error": e} → the bare e.
        assert_eq!(mirror_result("/x", 400, r#"{"error":"no such agent"}"#).unwrap_err(), "no such agent");
        // Other failures keep the status and the body.
        assert_eq!(mirror_result("/x", 403, r#"{"error":"nope"}"#).unwrap_err(), r#"daemon /x 403: {"error":"nope"}"#);
        assert_eq!(mirror_result("/x", 400, "plain").unwrap_err(), "daemon /x 400: plain");
        assert!(mirror_result("/x", 200, "not json").unwrap_err().starts_with("decode /x:"));
    }

    #[test]
    fn off_main_runs_the_call_off_the_calling_thread_and_returns_its_result() {
        let caller = std::thread::current().id();
        let (ok, ran_on): (Result<u8, String>, std::thread::ThreadId) = tauri::async_runtime::block_on(async move {
            let ran_on = off_main(|| Ok(std::thread::current().id())).await.expect("ran");
            (off_main(|| Ok(7u8)).await, ran_on)
        });
        assert_eq!(ok, Ok(7));
        assert_ne!(ran_on, caller, "the blocking call must not run on the calling thread");
        let err: Result<u8, String> =
            tauri::async_runtime::block_on(off_main(|| Err("daemon /x 500: boom".to_string())));
        assert_eq!(err, Err("daemon /x 500: boom".to_string()));
    }

    #[test]
    fn read_port_trims_whitespace() {
        let tmp = std::env::temp_dir().join(format!(
            "k2so-daemon-client-port-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&tmp, "  58211\n").expect("write");
        let p = read_port(&tmp).expect("read");
        assert_eq!(p, 58211);
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn read_port_rejects_malformed() {
        let tmp = std::env::temp_dir().join(format!(
            "k2so-daemon-client-port-bad-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&tmp, "not-a-port").expect("write");
        assert!(read_port(&tmp).is_err());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn read_token_rejects_empty_file() {
        let tmp = std::env::temp_dir().join(format!(
            "k2so-daemon-client-token-empty-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&tmp, "   \n").expect("write");
        assert!(read_token(&tmp).is_err());
        std::fs::remove_file(&tmp).ok();
    }
}
