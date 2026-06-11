//! Tauri-side HTTP client for the k2so-daemon.
//!
//! The daemon exposes a token-authed HTTP server on `127.0.0.1:<random>` —
//! see `crates/k2so-daemon/src/main.rs`. This module is the Tauri app's
//! counterpart: it discovers the daemon's port + token via the conventional
//! `~/.k2so/daemon.port` / `~/.k2so/daemon.token` files and wraps
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
    /// Reads `~/.k2so/daemon.port` + `~/.k2so/daemon.token` and targets the
    /// local bundled daemon at `http://127.0.0.1:<port>`. `Err` when either
    /// file is missing or malformed — caller's responsibility to trigger
    /// `launchctl load` and retry.
    pub fn try_connect() -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("build http client: {e}"))?;
        // Local bundled daemon: discover port + token from disk.
        let k2so_dir = k2so_dir()?;
        let port = read_port(&k2so_dir.join("daemon.port"))?;
        let token = read_token(&k2so_dir.join("daemon.token"))?;
        let base = format!("http://127.0.0.1:{port}");
        Ok(Self { base, token, http })
    }

    /// Hit `GET /ping` — no auth required. Returns `true` iff the daemon
    /// is reachable and replied with a 2xx. Intended for the Tauri app's
    /// post-launchd-load handshake.
    pub fn ping(&self) -> bool {
        let url = format!("{}/ping", self.base);
        self.http
            .get(url)
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    /// Hit a `/cli/*` route on the daemon, returning the raw response
    /// body (the CLI text/JSON the daemon emits). `params` are
    /// percent-encoded into the query string alongside the auth
    /// token. Any non-2xx status is surfaced as Err with the body.
    pub fn cli_get(&self, path: &str, params: &[(&str, &str)]) -> Result<String, String> {
        let mut url = format!("{}{}?token={}", self.base, path, self.token);
        for (k, v) in params {
            url.push('&');
            url.push_str(&pct_encode(k));
            url.push('=');
            url.push_str(&pct_encode(v));
        }
        let response = self
            .http
            .get(&url)
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

fn k2so_dir() -> Result<PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or_else(|| "home dir unavailable".to_string())?
        .join(".k2"))
}

fn read_port(path: &PathBuf) -> Result<u16, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    raw.trim()
        .parse::<u16>()
        .map_err(|e| format!("parse port from {}: {e}", path.display()))
}

fn read_token(path: &PathBuf) -> Result<String, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("empty token file at {}", path.display()));
    }
    Ok(trimmed.to_string())
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
