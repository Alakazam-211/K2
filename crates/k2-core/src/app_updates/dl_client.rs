//! The daemon's client for the download host (TC-12, contract
//! `dl-k2-dev-upload-api-contract-v1`, k2dl 1.3.0).
//!
//! - `PUT /api/v1/apps/{id}/versions/{version}/files/{filename}`: one file,
//!   streamed from disk, with `X-Content-SHA256`. `201` new, `200` the same
//!   bytes were already there (an idempotent retry). The answer's URL is
//!   immutable and content-addressed.
//! - `PUT /api/v1/apps/{id}/channels/{channel}/latest.json`: the static
//!   fallback manifest (after every file it names is up).
//! - `GET /api/v1/apps/{id}/stats?from&to`: downloads per version, file
//!   and day, plus `latest.json` fetches.
//! - `GET /healthz`.
//!
//! Auth is the App's own `k2dl_` token (`Authorization: Bearer`). The
//! token is never logged and never put in a URL. A network error, a `5xx`,
//! `408`, `429` or `503 busy` is retried with backoff (honouring
//! `Retry-After`, capped); every other refusal comes back with the host's
//! code. A reply whose sha256 differs from ours is an error.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value as J};

/// Why a call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlError {
    /// The host didn't answer usefully after the retries (network, 5xx,
    /// 408, 429, 503): try later. The string is a one-line reason.
    Unavailable(String),
    /// The host refused: its HTTP status, its `error` code and message.
    Refused { status: u16, code: String, message: String },
    /// The host answered something K2 can't use (bad JSON, a URL that
    /// isn't content-addressed for this file, a sha256 that differs).
    Bad(String),
}

impl DlError {
    /// One line for a refusal hint or a release's error column. Never the token.
    pub fn line(&self) -> String {
        match self {
            DlError::Unavailable(w) => format!("the download host is unavailable: {w}"),
            DlError::Refused { status, code, message } => format!("the download host refused ({status} {code}): {message}"),
            DlError::Bad(w) => format!("the download host's answer was unusable: {w}"),
        }
    }
}

/// One uploaded file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uploaded {
    pub url: String,
    /// `true` = a new file (201), `false` = the same bytes were there (200).
    pub created: bool,
}

/// The client. `backoff` is the first retry's wait (doubled each time).
pub struct Client {
    base: String,
    token: String,
    http: reqwest::blocking::Client,
    pub attempts: u32,
    pub backoff: Duration,
    pub max_wait: Duration,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("base", &self.base).field("token", &"<redacted>").finish()
    }
}

fn is_retryable(status: u16) -> bool {
    status >= 500 || status == 408 || status == 429
}

impl Client {
    pub fn new(base: &str, token: &str) -> Result<Self, DlError> {
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(None)
            .user_agent(concat!("k2-daemon/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| DlError::Bad(format!("http client: {e}")))?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            http,
            attempts: 4,
            backoff: Duration::from_secs(2),
            max_wait: Duration::from_secs(60),
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Run `send` until it answers something final, retrying the retryable.
    fn with_retries(&self, mut send: impl FnMut() -> reqwest::Result<reqwest::blocking::Response>) -> Result<(u16, String), DlError> {
        let mut wait = self.backoff;
        let mut last = String::new();
        for attempt in 0..self.attempts.max(1) {
            if attempt > 0 {
                std::thread::sleep(wait.min(self.max_wait));
                wait = wait.saturating_mul(2);
            }
            match send() {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let retry_after =
                        resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok()).map(Duration::from_secs);
                    let body = resp.text().unwrap_or_default();
                    if is_retryable(status) {
                        last = format!("HTTP {status}");
                        if let Some(ra) = retry_after {
                            wait = ra.min(self.max_wait);
                        }
                        continue;
                    }
                    return Ok((status, body));
                }
                Err(e) => {
                    // The error's text can carry the URL, never the token.
                    last = if e.is_timeout() { "timed out".into() } else if e.is_connect() { "can't connect".into() } else { "request failed".into() };
                }
            }
        }
        Err(DlError::Unavailable(format!("{last} after {} attempts", self.attempts.max(1))))
    }

    fn refused(status: u16, body: &str) -> DlError {
        let v: J = serde_json::from_str(body).unwrap_or(J::Null);
        DlError::Refused {
            status,
            code: v.get("error").and_then(J::as_str).unwrap_or("http_error").to_string(),
            message: v.get("message").and_then(J::as_str).unwrap_or("").chars().take(300).collect(),
        }
    }

    /// Upload `path` (whose sha256 and size the caller already checked) as
    /// `filename` of `version` for the App `id`.
    pub fn upload(&self, id: &str, version: &str, filename: &str, path: &Path, sha256: &str, size: u64, content_type: &str) -> Result<Uploaded, DlError> {
        let url = format!("{}/api/v1/apps/{id}/versions/{version}/files/{filename}", self.base);
        // The host's own deadline: 10 min + 1 s per MiB.
        let deadline = Duration::from_secs(600 + size / (1024 * 1024) + 60);
        let (status, body) = self.with_retries(|| {
            let f = std::fs::File::open(path).map_err(|e| e.to_string());
            let body = match f {
                Ok(f) => reqwest::blocking::Body::sized(f, size),
                // A vanished staging file: send an empty body; the host's
                // length check refuses it (never a silent success).
                Err(_) => reqwest::blocking::Body::from(Vec::new()),
            };
            self.http
                .put(&url)
                .bearer_auth(&self.token)
                .header("Content-Type", content_type)
                .header("X-Content-SHA256", sha256)
                .timeout(deadline)
                .body(body)
                .send()
        })?;
        if status != 200 && status != 201 {
            return Err(Self::refused(status, &body));
        }
        let v: J = serde_json::from_str(&body).map_err(|e| DlError::Bad(format!("upload answer isn't JSON: {e}")))?;
        let got_sha = v.get("sha256").and_then(J::as_str).unwrap_or("");
        if got_sha != sha256 {
            return Err(DlError::Bad(format!("the host stored sha256 {got_sha}, K2 sent {sha256}")));
        }
        let got_url = v.get("url").and_then(J::as_str).unwrap_or("").to_string();
        if !url_is_for(&got_url, id, version, sha256) {
            return Err(DlError::Bad(format!("the host's URL isn't this file's content-addressed URL: {got_url}")));
        }
        Ok(Uploaded { url: got_url, created: status == 201 })
    }

    /// Publish the channel's static `latest.json` (the fallback).
    pub fn put_manifest(&self, id: &str, channel: &str, manifest: &J) -> Result<String, DlError> {
        let url = format!("{}/api/v1/apps/{id}/channels/{channel}/latest.json", self.base);
        let body = manifest.to_string();
        let (status, text) = self.with_retries(|| {
            self.http.put(&url).bearer_auth(&self.token).header("Content-Type", "application/json").timeout(Duration::from_secs(60)).body(body.clone()).send()
        })?;
        if status != 200 {
            return Err(Self::refused(status, &text));
        }
        let v: J = serde_json::from_str(&text).map_err(|e| DlError::Bad(format!("manifest answer isn't JSON: {e}")))?;
        Ok(v.get("url").and_then(J::as_str).unwrap_or("").to_string())
    }

    /// The App's download numbers, `from`..=`to` (`YYYY-MM-DD`, UTC).
    pub fn stats(&self, id: &str, from: &str, to: &str) -> Result<J, DlError> {
        let url = format!("{}/api/v1/apps/{id}/stats?from={from}&to={to}", self.base);
        let (status, text) = self.with_retries(|| self.http.get(&url).bearer_auth(&self.token).timeout(Duration::from_secs(30)).send())?;
        if status != 200 {
            return Err(Self::refused(status, &text));
        }
        serde_json::from_str(&text).map_err(|e| DlError::Bad(format!("stats aren't JSON: {e}")))
    }

    /// `GET /healthz` (no token needed; sent anyway, it's the same host).
    pub fn health(&self) -> Result<J, DlError> {
        let url = format!("{}/healthz", self.base);
        let (status, text) = self.with_retries(|| self.http.get(&url).timeout(Duration::from_secs(10)).send())?;
        if status != 200 {
            return Err(Self::refused(status, &text));
        }
        serde_json::from_str(&text).map_err(|e| DlError::Bad(format!("health isn't JSON: {e}")))
    }
}

/// `url` is `<any base>/<id>/<version>/<sha256>/<file>` for this file.
pub fn url_is_for(url: &str, id: &str, version: &str, sha256: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else { return false };
    let mut parts = rest.splitn(2, '/');
    let _host = parts.next();
    let path = parts.next().unwrap_or("");
    let segs: Vec<&str> = path.split('/').collect();
    segs.len() == 4 && segs[0] == id && segs[1] == version && segs[2] == sha256 && !segs[3].is_empty()
}

/// Downloads per version from a stats answer: `{version: {downloads, bytes}}`
/// and the total.
pub fn downloads_by_version(stats: &J) -> (std::collections::BTreeMap<String, (i64, i64)>, i64) {
    let mut out = std::collections::BTreeMap::new();
    let mut total = 0i64;
    for v in stats.get("versions").and_then(J::as_array).into_iter().flatten() {
        let ver = v.get("version").and_then(J::as_str).unwrap_or("").to_string();
        let e = out.entry(ver).or_insert((0i64, 0i64));
        for f in v.get("files").and_then(J::as_array).into_iter().flatten() {
            for d in f.get("days").and_then(J::as_array).into_iter().flatten() {
                let n = d.get("downloads").and_then(J::as_i64).unwrap_or(0);
                e.0 += n;
                e.1 += d.get("bytes").and_then(J::as_i64).unwrap_or(0);
                total += n;
            }
        }
    }
    (out, total)
}

/// A stand-in stats answer with nothing in it (tests, docs).
pub fn empty_stats(id: &str, from: &str, to: &str) -> J {
    json!({ "app": id, "from": from, "to": to, "versions": [], "manifest_checks": [] })
}
