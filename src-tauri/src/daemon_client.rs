//! Tauri-side HTTP client for the k2-daemon.
//!
//! The daemon exposes a token-authed HTTP server on `127.0.0.1:<random>` —
//! see `crates/k2-daemon/src/main.rs`. This module is the Tauri app's
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
//! - **Blocking reqwest** matches the existing K2SO HTTP story (the push
//!   adapters use `reqwest::blocking`). No tokio-runtime
//!   ceremony for the handful of ops we make per command.
//! - **Token loaded lazily** — we re-read the port/token files on every
//!   construction because the daemon can be restarted (launchd KeepAlive)
//!   and its port + token rotate. Holding a stale client is a footgun.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

/// Minimal JSON shape returned by `GET /status` on the daemon.
/// Keep in lockstep with `crates/k2-daemon/src/main.rs::send_response`
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
            // K2 never sends a request to its own server through a proxy:
            // http_proxy, HTTPS_PROXY, ALL_PROXY and the rest are ignored.
            .no_proxy()
            .build()
            .map_err(|e| format!("build http client: {e}"))?;
        Ok(Self { base, token, http })
    }

    /// A client for a fake daemon in tests ([`test_support::FakeDaemon`]).
    #[cfg(test)]
    pub(crate) fn for_test(base: &str, token: &str) -> Self {
        Self::at(base.to_string(), token.to_string()).expect("build test client")
    }


    /// `{base}{path}` with `params` percent-encoded into the query string.
    /// The token never goes in the address: see [`Self::request`].
    fn url(&self, path: &str, params: &[(&str, &str)]) -> String {
        let mut url = format!("{}{}", self.base, path);
        for (i, (k, v)) in params.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(&pct_encode(k));
            url.push('=');
            url.push_str(&pct_encode(v));
        }
        url
    }

    /// Every request to the daemon starts here: the owner token goes in an
    /// `Authorization: Bearer` header, never the address.
    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::blocking::RequestBuilder {
        self.http.request(method, url).bearer_auth(&self.token)
    }

    /// Hit a `/cli/*` route on the daemon, returning the raw response
    /// body (the CLI text/JSON the daemon emits). `params` are
    /// percent-encoded into the query string; the token rides a header.
    /// Any non-2xx status is surfaced as Err with the body.
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
        let url = self.url(path, params);
        let request = if post {
            self.request(reqwest::Method::POST, &url).body("")
        } else {
            self.request(reqwest::Method::GET, &url)
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
    /// `application/json`; the token rides the same header `cli_get`
    /// sends. Returns the raw response body on 2xx; any non-2xx is
    /// surfaced as `Err`.
    pub fn cli_post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<String, String> {
        let url = self.url(path, &[]);
        let response = self
            .request(reqwest::Method::POST, &url)
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
        let url = self.url(path, params);
        let response = self.request(reqwest::Method::GET, &url).send().map_err(|e| format!("daemon {path}: {e}"))?;
        let status = response.status().as_u16();
        let body = response.text().unwrap_or_default();
        mirror_result(path, status, &body)
    }

    /// POST a JSON body to a thin-client mirror route and return its JSON
    /// reply (see [`mirror_get`]).
    pub fn mirror_post(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        let url = self.url(path, &[]);
        let response = self
            .request(reqwest::Method::POST, &url)
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

    /// Hit `GET /status` and decode the JSON body.
    pub fn status(&self) -> Result<DaemonStatus, String> {
        let url = self.url("/status", &[]);
        let response = self
            .request(reqwest::Method::GET, &url)
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
    k2_client::paths::k2_home_opt().ok_or_else(|| "home dir unavailable".to_string())
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
        /// Decoded query pairs, in order.
        pub query: Vec<(String, String)>,
        /// The `Authorization` header's value, if one came.
        pub authorization: Option<String>,
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
                    let mut authorization = None;
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
                        if lower.starts_with("authorization:") {
                            authorization = Some(header["authorization:".len()..].trim().to_string());
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
                        authorization,
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
    // ~/.k2/daemon.token, so a test can't pave over a real running
    // daemon without tripping the user's own workflow. Smoke-test
    // workflow is in scripts/perf-load-harness.sh neighborhood.
    //
    // What we CAN test here: the port/token readers and the status
    // deserializer — pure file-I/O + serde.

    #[test]
    fn status_decodes_from_canonical_daemon_body() {
        // Matches the format emitted by k2-daemon/src/main.rs.
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

    #[test]
    fn a_request_carries_the_token_as_a_bearer_header_never_in_the_address() {
        let client = DaemonClient::for_test("http://127.0.0.1:9", "tok");
        let req = client
            .request(reqwest::Method::GET, &client.url("/cli/x", &[("a", "b c")]))
            .build()
            .expect("build request");
        assert_eq!(req.url().as_str(), "http://127.0.0.1:9/cli/x?a=b%20c");
        assert!(!req.url().as_str().contains("token"), "token in the address: {}", req.url());
        let auth = req.headers().get(reqwest::header::AUTHORIZATION).expect("an Authorization header");
        assert_eq!(auth.to_str().expect("ascii"), "Bearer tok");
        assert_eq!(client.url("/status", &[]), "http://127.0.0.1:9/status");
    }

    #[test]
    fn every_route_helper_sends_the_bearer_and_no_token_param() {
        use test_support::{pairs, FakeDaemon};
        let fake = FakeDaemon::serve(vec![(200, "{}"), (200, "{}"), (200, "{}"), (200, "{}"), (200, "{}")]);
        let client = DaemonClient::for_test(&fake.base, "tok");
        let base = fake.base.clone();
        tauri::async_runtime::block_on(off_main(move || {
            client.cli_get("/cli/a", &[("k", "v")])?;
            client.cli_post_query("/cli/b", &[("k", "v")])?;
            client.cli_post_json("/cli/c", &serde_json::json!({}))?;
            client.mirror_get("/cli/d", &[])?;
            client.mirror_post("/cli/e", &serde_json::json!({}))?;
            Ok(())
        }))
        .unwrap_or_else(|e| panic!("calls to {base}: {e}"));
        for (method, path, query) in [
            ("GET", "/cli/a", pairs(&[("k", "v")])),
            ("POST", "/cli/b", pairs(&[("k", "v")])),
            ("POST", "/cli/c", pairs(&[])),
            ("GET", "/cli/d", pairs(&[])),
            ("POST", "/cli/e", pairs(&[])),
        ] {
            let seen = fake.next();
            assert_eq!((seen.method.as_str(), seen.path.as_str()), (method, path));
            assert_eq!(seen.query, query, "{path}: the token must not be a query param");
            assert_eq!(seen.authorization.as_deref(), Some("Bearer tok"), "{path}");
        }
    }

    /// Every proxy variable reqwest reads.
    const PROXY_VARS: [&str; 6] = ["http_proxy", "HTTP_PROXY", "https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"];
    const PROXY_CHILD: &str = "DAEMON_CLIENT_TEST_PROXY_CHILD";
    const PROXY_CHILD_DAEMON: &str = "DAEMON_CLIENT_TEST_DAEMON_BASE";

    /// A listening stand-in proxy: counts every connection it accepts and
    /// answers each with 502, so a client that went through it fails.
    struct ProxyStub {
        url: String,
        hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl ProxyStub {
        fn start() -> Self {
            use std::io::{BufRead, BufReader, Write};
            use std::sync::atomic::Ordering;
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind proxy stub");
            let url = format!("http://{}", listener.local_addr().expect("proxy stub addr"));
            let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counter = hits.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let stream = stream.expect("proxy stub accept");
                    counter.fetch_add(1, Ordering::SeqCst);
                    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("read timeout");
                    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                    let mut line = String::new();
                    while reader.read_line(&mut line).map(|n| n > 0).unwrap_or(false) && line != "\r\n" {
                        line.clear();
                    }
                    let mut out = stream;
                    let _ = out.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                }
            });
            Self { url, hits }
        }

        fn hits(&self) -> usize {
            self.hits.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// Run this test's child: this test binary again, with every proxy
    /// variable pointing at `proxy`. The variables are set on the child
    /// only, never in this process, so no other test sees them.
    fn run_proxy_child(mode: &str, proxy: &ProxyStub, daemon_base: &str) -> std::process::Output {
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
        child
            .args(["--exact", "daemon_client::tests::proxy_settings_never_reach_the_daemon_client"])
            .args(["--nocapture", "--test-threads=1"])
            .env(PROXY_CHILD, mode)
            .env(PROXY_CHILD_DAEMON, daemon_base)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .env_remove("REQUEST_METHOD")
            .stdin(std::process::Stdio::null());
        for var in PROXY_VARS {
            child.env(var, &proxy.url);
        }
        child.output().expect("the child ran")
    }

    fn assert_child_passed(mode: &str, out: &std::process::Output) {
        assert!(
            out.status.success(),
            "the {mode} child failed ({}):\n{}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// K2 never sends a request to its own server through a proxy. The
    /// child process has every proxy variable set to a listening stub;
    /// a plain reqwest client there goes through the stub (the control,
    /// so the zero below means something), and the daemon client goes
    /// straight to the daemon with the stub seeing nothing.
    #[test]
    fn proxy_settings_never_reach_the_daemon_client() {
        if let Ok(mode) = std::env::var(PROXY_CHILD) {
            let base = std::env::var(PROXY_CHILD_DAEMON).expect("the daemon stub's base");
            match mode.as_str() {
                "control" => {
                    let plain = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(10))
                        .build()
                        .expect("plain client");
                    let res = plain.get(format!("{base}/cli/control")).send().expect("the proxy stub answered");
                    assert_eq!(res.status().as_u16(), 502, "a plain client must go through the proxy stub");
                }
                "daemon" => {
                    let client = DaemonClient::for_test(&base, "tok");
                    let body = client.cli_get("/cli/through", &[]).expect("the daemon stub answered");
                    assert_eq!(body, "{}");
                }
                other => panic!("unknown child mode {other}"),
            }
            return;
        }

        // Control: the proxy variables do reach a plain client in the child.
        // Nothing listens at its daemon address: only the proxy stub answers.
        let proxy = ProxyStub::start();
        let out = run_proxy_child("control", &proxy, "http://127.0.0.1:9");
        assert_child_passed("control", &out);
        assert_eq!(proxy.hits(), 1, "the control request must reach the proxy stub");

        // The daemon client ignores them: the stub sees nothing.
        let proxy = ProxyStub::start();
        let fake = test_support::FakeDaemon::serve(vec![(200, "{}")]);
        let out = run_proxy_child("daemon", &proxy, &fake.base);
        assert_eq!(proxy.hits(), 0, "a proxy variable reached the daemon client: the proxy stub got its request");
        assert_child_passed("daemon", &out);
        let seen = fake.next();
        assert_eq!((seen.method.as_str(), seen.path.as_str()), ("GET", "/cli/through"));
        assert_eq!(seen.authorization.as_deref(), Some("Bearer tok"));
    }
}
