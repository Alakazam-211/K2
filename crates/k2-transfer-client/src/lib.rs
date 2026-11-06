//! Resumable uploads and downloads against a K2 transfer door.
//!
//! K2 (the agent server) gives an App's people a small HTTP door for big
//! files, made for the times a phone app can't keep its own connection
//! alive: `/_k2/transfers/…` on the App's own host. The App's server starts
//! the transfer as the person (`files.uploadCreate`, `files.downloadToken`,
//! `thread.attachmentToken`) and hands the app a URL path and a short-lived
//! token. This crate is the app's side, for a Tauri app's Rust code (or any
//! tokio program):
//!
//! - [`Client::upload_file`] sends a file in ranges (16 MiB at most each),
//!   asks the door first what it already holds, so a second call after a
//!   crash, a kill or a lost network sends only what is missing; then it
//!   completes with the file's sha256.
//! - [`Client::download_to`] saves a file, resuming into `<dest>.part` with
//!   `Range` and `If-Range`, so a changed file restarts instead of mixing
//!   versions.
//! - Every request retries transient failures (network, 5xx, 429) with
//!   exponential backoff and jitter; a refusal (a 4xx) returns at once with
//!   K2's error code ([`Error::code`]), for example `transfer_token_expired`
//!   (ask the App's server for a new token) or `transfer_expired` (start
//!   over).
//!
//! On a phone the OS does the transfer while the app sleeps (iOS
//! `URLSession` background tasks, Android WorkManager); this crate is for
//! the foreground, the desktop, and for working out what is left
//! ([`missing_parts`]) when the app wakes up. The wire protocol and the OS
//! recipes: `k2 study app-transfers`.
//!
//! TLS is rustls through reqwest with reqwest's own default provider.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// The most one range carries (the door refuses more).
pub const MAX_PART: u64 = 16 * 1024 * 1024;

/// Where a transfer lives: the full URL of its door path and its token.
#[derive(Clone)]
pub struct Endpoint {
    /// `https://<host>/_k2/transfers/uploads/<id>` (or `…/downloads/<id>`).
    pub url: String,
    /// `k2up_…` or `k2dn_…`. Never logged by this crate.
    pub token: String,
}

impl Endpoint {
    /// `base` is the App's own origin (`https://notes.box.k2.dev`), `path`
    /// the `url` K2 answered (`/_k2/transfers/uploads/<id>`).
    pub fn new(base: &str, path: &str, token: &str) -> Self {
        Endpoint { url: format!("{}{}", base.trim_end_matches('/'), path), token: token.to_string() }
    }
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint").field("url", &self.url).field("token", &"<redacted>").finish()
    }
}

/// What the door holds of an upload (`GET`/`HEAD`; every `PUT` answers it).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadStatus {
    pub upload_id: String,
    pub size: u64,
    /// `[first, last]` inclusive, sorted and merged.
    pub received: Vec<[u64; 2]>,
    /// `[first, last]` inclusive.
    pub missing: Vec<[u64; 2]>,
    pub received_bytes: u64,
    pub complete: bool,
    pub done: bool,
    /// Milliseconds since the epoch.
    pub expires_at: u64,
    pub part_max: u64,
    #[serde(default)]
    pub rel_path: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

/// A finished upload.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadDone {
    pub upload_id: String,
    pub done: bool,
    /// Where the file landed, room-relative.
    pub rel_path: String,
    pub size: u64,
    pub sha256: String,
}

/// A finished download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadDone {
    pub path: PathBuf,
    pub size: u64,
    pub etag: Option<String>,
    /// Bytes this call fetched (less than `size` when it resumed).
    pub fetched: u64,
}

/// Progress for a callback: bytes the door holds (upload) or the file
/// holds (download), of `total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// Why a transfer step failed.
#[derive(Debug)]
pub enum Error {
    /// K2 refused: its HTTP status, error code and hint.
    Refused { status: u16, code: String, hint: String, retry_after_secs: Option<u64> },
    /// The network or TLS.
    Transport(reqwest::Error),
    /// A local file.
    Io(std::io::Error),
    /// The answer wasn't what the protocol says.
    Protocol(String),
}

impl Error {
    /// K2's error code, when K2 refused (`transfer_token_expired`, …).
    pub fn code(&self) -> Option<&str> {
        match self {
            Error::Refused { code, .. } => Some(code),
            _ => None,
        }
    }

    /// Worth trying the same request again: the network, a 5xx, 408 or 429.
    pub fn retryable(&self) -> bool {
        match self {
            Error::Transport(_) => true,
            Error::Refused { status, .. } => *status >= 500 || *status == 408 || *status == 429,
            Error::Io(_) | Error::Protocol(_) => false,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Refused { status, code, hint, .. } => write!(f, "K2 refused ({status} {code}): {hint}"),
            Error::Transport(e) => write!(f, "network: {e}"),
            Error::Io(e) => write!(f, "file: {e}"),
            Error::Protocol(m) => write!(f, "protocol: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// How to retry a transient failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    /// Tries per request, the first included.
    pub attempts: u32,
    /// The first wait; it doubles each time, up to `max`.
    pub base: Duration,
    pub max: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Retry { attempts: 6, base: Duration::from_millis(500), max: Duration::from_secs(30) }
    }
}

/// The wait before try number `attempt` (1-based, after a failure):
/// exponential, capped, with "equal jitter" (half fixed, half from `seed`),
/// at least what the server's `Retry-After` asked.
pub fn backoff_delay(retry: &Retry, attempt: u32, seed: u64, retry_after_secs: Option<u64>) -> Duration {
    let exp = retry.base.saturating_mul(1u32.checked_shl(attempt.saturating_sub(1).min(20)).unwrap_or(u32::MAX));
    let capped = exp.min(retry.max);
    let half = capped / 2;
    // splitmix64 of the seed, for a spread without a random crate.
    let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    let jitter = if half.as_millis() == 0 { Duration::ZERO } else { Duration::from_millis(z % (half.as_millis() as u64 + 1)) };
    let d = half + jitter;
    match retry_after_secs {
        Some(s) => d.max(Duration::from_secs(s)),
        None => d,
    }
}

/// The `(start, len)` pieces that cover `missing` (inclusive gaps, as the
/// door answers them), each at most `part` bytes.
pub fn missing_parts(missing: &[[u64; 2]], part: u64) -> Vec<(u64, u64)> {
    let part = part.clamp(1, MAX_PART);
    let mut out = Vec::new();
    for &[first, last] in missing {
        if last < first {
            continue;
        }
        let mut at = first;
        let end = last + 1;
        while at < end {
            let len = (end - at).min(part);
            out.push((at, len));
            at += len;
        }
    }
    out
}

/// A file's sha256, hex.
pub async fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = tokio::fs::File::open(path).await?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What an upload does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadOptions {
    /// Bytes per range (at most [`MAX_PART`]; the door's `partMax` also caps it).
    pub part_size: u64,
    /// Send each range's own sha256 (`K2-Content-Sha256`), so a corrupted
    /// range is refused before it is written.
    pub range_checksums: bool,
}

impl Default for UploadOptions {
    fn default() -> Self {
        UploadOptions { part_size: 8 * 1024 * 1024, range_checksums: true }
    }
}

/// The transfer client. Cheap to clone (reqwest's client is shared).
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    retry: Retry,
}

impl Client {
    /// A client with reqwest's defaults (rustls, its default provider).
    pub fn new() -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .build()
            .map_err(Error::Transport)?;
        Ok(Client { http, retry: Retry::default() })
    }

    /// Use your own reqwest client (proxy, timeouts, user agent).
    pub fn with_http(http: reqwest::Client) -> Self {
        Client { http, retry: Retry::default() }
    }

    /// Change how transient failures are retried.
    pub fn retry(mut self, retry: Retry) -> Self {
        self.retry = retry;
        self
    }

    async fn send_once(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response, Error> {
        let resp = req.send().await.map_err(Error::Transport)?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let retry_after_secs = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let text = resp.text().await.unwrap_or_default();
        let (code, hint) = match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => {
                let err = &v["error"];
                let code = err["code"].as_str().or_else(|| err.as_str()).unwrap_or("failed").to_string();
                let hint = err["hint"].as_str().or_else(|| v["message"].as_str()).unwrap_or("").to_string();
                (code, hint)
            }
            Err(_) => ("failed".to_string(), text.chars().take(200).collect()),
        };
        Err(Error::Refused { status, code, hint, retry_after_secs })
    }

    /// Send `make()`'s request, retrying transient failures.
    async fn send<F>(&self, make: F) -> Result<reqwest::Response, Error>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.send_once(make()).await {
                Ok(r) => return Ok(r),
                Err(e) if e.retryable() && attempt < self.retry.attempts.max(1) => {
                    let after = match &e {
                        Error::Refused { retry_after_secs, .. } => *retry_after_secs,
                        _ => None,
                    };
                    let seed = (attempt as u64) ^ (std::process::id() as u64) << 16;
                    tokio::time::sleep(backoff_delay(&self.retry, attempt, seed, after)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// What the door holds of this upload.
    pub async fn upload_status(&self, ep: &Endpoint) -> Result<UploadStatus, Error> {
        let r = self.send(|| self.http.get(&ep.url).bearer_auth(&ep.token)).await?;
        r.json().await.map_err(Error::Transport)
    }

    /// Send one range (`start`, these bytes) of a `total`-byte upload.
    /// Idempotent: sending it twice is fine.
    pub async fn put_range(&self, ep: &Endpoint, start: u64, bytes: Vec<u8>, total: u64, checksum: bool) -> Result<UploadStatus, Error> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_PART {
            return Err(Error::Protocol(format!("a range is 1..={MAX_PART} bytes")));
        }
        let last = start + bytes.len() as u64 - 1;
        let sum = checksum.then(|| hex(&Sha256::digest(&bytes)));
        let body = bytes;
        let r = self
            .send(|| {
                let mut req = self
                    .http
                    .put(&ep.url)
                    .bearer_auth(&ep.token)
                    .header(reqwest::header::CONTENT_RANGE, format!("bytes {start}-{last}/{total}"))
                    .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                    .body(body.clone());
                if let Some(s) = &sum {
                    req = req.header("K2-Content-Sha256", s.as_str());
                }
                req
            })
            .await?;
        r.json().await.map_err(Error::Transport)
    }

    /// Finish the upload: K2 checks every byte and the sha256, then
    /// publishes. Safe to call again.
    pub async fn complete(&self, ep: &Endpoint, sha256: Option<&str>) -> Result<UploadDone, Error> {
        let url = format!("{}/complete", ep.url);
        let body = match sha256 {
            Some(s) => serde_json::json!({ "sha256": s }).to_string(),
            None => "{}".to_string(),
        };
        let r = self
            .send(|| {
                self.http
                    .post(&url)
                    .bearer_auth(&ep.token)
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(body.clone())
            })
            .await?;
        r.json().await.map_err(Error::Transport)
    }

    /// Stop the upload; what K2 held is deleted.
    pub async fn abort(&self, ep: &Endpoint) -> Result<(), Error> {
        self.send(|| self.http.delete(&ep.url).bearer_auth(&ep.token)).await.map(|_| ())
    }

    /// Upload `path` (whose size the upload was made with): ask what the
    /// door holds, send only the missing ranges, then complete with the
    /// file's sha256. Call it again after any failure; it resumes.
    pub async fn upload_file<P>(&self, ep: &Endpoint, path: &Path, opts: UploadOptions, mut progress: P) -> Result<UploadDone, Error>
    where
        P: FnMut(Progress),
    {
        let sha = sha256_file(path).await?;
        let mut st = self.upload_status(ep).await?;
        let len = tokio::fs::metadata(path).await?.len();
        if len != st.size {
            return Err(Error::Protocol(format!("the file has {len} bytes, the upload was made for {}", st.size)));
        }
        if !st.done {
            progress(Progress { done: st.received_bytes, total: st.size });
            let part = opts.part_size.min(st.part_max.max(1));
            let mut f = tokio::fs::File::open(path).await?;
            for (start, n) in missing_parts(&st.missing, part) {
                f.seek(std::io::SeekFrom::Start(start)).await?;
                let mut buf = vec![0u8; n as usize];
                f.read_exact(&mut buf).await?;
                st = self.put_range(ep, start, buf, st.size, opts.range_checksums).await?;
                progress(Progress { done: st.received_bytes, total: st.size });
            }
        }
        self.complete(ep, Some(&sha)).await
    }

    /// Save the file behind a download token to `dest`, resuming a
    /// `<dest>.part` an earlier call left (its version kept in
    /// `<dest>.part.etag`). A file that changed since restarts from 0.
    pub async fn download_to<P>(&self, ep: &Endpoint, dest: &Path, mut progress: P) -> Result<DownloadDone, Error>
    where
        P: FnMut(Progress),
    {
        let part = with_suffix(dest, ".part");
        let etag_file = with_suffix(dest, ".part.etag");
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.download_once(ep, dest, &part, &etag_file, &mut progress).await {
                Ok(d) => return Ok(d),
                Err(e) if e.retryable() && attempt < self.retry.attempts.max(1) => {
                    let after = match &e {
                        Error::Refused { retry_after_secs, .. } => *retry_after_secs,
                        _ => None,
                    };
                    tokio::time::sleep(backoff_delay(&self.retry, attempt, attempt as u64, after)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn download_once<P>(&self, ep: &Endpoint, dest: &Path, part: &Path, etag_file: &Path, progress: &mut P) -> Result<DownloadDone, Error>
    where
        P: FnMut(Progress),
    {
        let have = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
        let etag = tokio::fs::read_to_string(etag_file).await.ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let mut req = self.http.get(&ep.url).bearer_auth(&ep.token);
        if let (true, Some(tag)) = (have > 0, etag.as_deref()) {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-")).header(reqwest::header::IF_RANGE, tag);
        }
        let resp = match self.send_once(req).await {
            Ok(r) => r,
            // Asked past the end: the part doesn't fit the file now. Drop it
            // and let the retry loop start over (a 503-shaped retry).
            Err(Error::Refused { status: 416, .. }) if have > 0 => {
                let _ = tokio::fs::remove_file(part).await;
                let _ = tokio::fs::remove_file(etag_file).await;
                return Err(Error::Refused { status: 503, code: "restart".into(), hint: "the saved part was stale; starting over".into(), retry_after_secs: Some(0) });
            }
            Err(e) => return Err(e),
        };
        let status = resp.status().as_u16();
        let new_etag = resp.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).map(str::to_string);
        let (mut file, mut at, total) = if status == 206 {
            let total = content_range_total(resp.headers()).ok_or_else(|| Error::Protocol("206 without Content-Range".into()))?;
            let f = tokio::fs::OpenOptions::new().append(true).open(part).await?;
            (f, have, total)
        } else {
            let total = resp.content_length().unwrap_or(0);
            let f = tokio::fs::File::create(part).await?;
            (f, 0, total)
        };
        match &new_etag {
            Some(t) => tokio::fs::write(etag_file, t).await?,
            None => {
                let _ = tokio::fs::remove_file(etag_file).await;
            }
        }
        progress(Progress { done: at, total });
        let fetched_from = at;
        let mut resp = resp;
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    file.write_all(&chunk).await?;
                    at += chunk.len() as u64;
                    progress(Progress { done: at, total });
                }
                Ok(None) => break,
                Err(e) => {
                    // Everything received is on disk before the next try
                    // reads the part's length.
                    file.flush().await?;
                    file.sync_all().await?;
                    return Err(Error::Transport(e));
                }
            }
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        if total > 0 && at != total {
            return Err(Error::Protocol(format!("the download stopped at {at} of {total} bytes")));
        }
        tokio::fs::rename(part, dest).await?;
        let _ = tokio::fs::remove_file(etag_file).await;
        Ok(DownloadDone { path: dest.to_path_buf(), size: at, etag: new_etag, fetched: at - fetched_from })
    }
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// The `<size>` of `Content-Range: bytes a-b/<size>`.
fn content_range_total(h: &reqwest::header::HeaderMap) -> Option<u64> {
    let v = h.get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    v.rsplit_once('/')?.1.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_cover_the_gaps_exactly() {
        assert_eq!(missing_parts(&[[0, 9]], 4), vec![(0, 4), (4, 4), (8, 2)]);
        assert_eq!(missing_parts(&[[0, 0], [10, 19]], 100), vec![(0, 1), (10, 10)]);
        assert_eq!(missing_parts(&[], 4), vec![]);
        assert_eq!(missing_parts(&[[5, 4]], 4), vec![], "a reversed gap is nothing");
        let big = missing_parts(&[[0, MAX_PART * 2]], u64::MAX);
        assert_eq!(big.len(), 3, "never more than MAX_PART per range");
        assert!(big.iter().all(|(_, n)| *n <= MAX_PART));
    }

    #[test]
    fn backoff_grows_caps_and_honours_retry_after() {
        let r = Retry { attempts: 5, base: Duration::from_millis(100), max: Duration::from_millis(1000) };
        for seed in 0..50 {
            let d1 = backoff_delay(&r, 1, seed, None);
            assert!(d1 >= Duration::from_millis(50) && d1 <= Duration::from_millis(100), "{d1:?}");
            let d3 = backoff_delay(&r, 3, seed, None);
            assert!(d3 >= Duration::from_millis(200) && d3 <= Duration::from_millis(400), "{d3:?}");
            let d9 = backoff_delay(&r, 9, seed, None);
            assert!(d9 >= Duration::from_millis(500) && d9 <= Duration::from_millis(1000), "capped: {d9:?}");
        }
        assert_eq!(backoff_delay(&r, 1, 7, Some(3)), Duration::from_secs(3));
        assert!(backoff_delay(&r, 40, 1, None) <= r.max, "no overflow on a long run");
    }

    #[test]
    fn errors_say_what_to_retry() {
        let refused = |status: u16| Error::Refused { status, code: "x".into(), hint: String::new(), retry_after_secs: None };
        assert!(refused(503).retryable());
        assert!(refused(429).retryable());
        assert!(!refused(401).retryable());
        assert!(!refused(410).retryable());
        assert_eq!(refused(401).code(), Some("x"));
        assert!(!Error::Protocol("p".into()).retryable());
    }

    #[test]
    fn an_endpoint_never_prints_its_token() {
        let ep = Endpoint::new("https://a.example/", "/_k2/transfers/uploads/abc", "k2up_secret");
        assert_eq!(ep.url, "https://a.example/_k2/transfers/uploads/abc");
        assert!(!format!("{ep:?}").contains("secret"));
    }
}
