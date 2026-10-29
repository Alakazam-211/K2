//! K2's own static file server (prd-tauri-core-0467-v1 TC-2).
//!
//! One HTTP/1.1 connection loop over a folder, shared by static Apps
//! (`apps::static_engine`) and written with no App-only assumptions so
//! `k2 publish` can use it later (the sibling PRD). The rules live in
//! `k2_core::deploy::static_site`; this module does the IO:
//!
//! - **The jail.** A request path is decoded once and cleaned
//!   (`static_site::request_rel`: no `..`, no encoded separator, no NUL);
//!   the file is then canonicalized and must sit under the canonical root,
//!   so a symlink out of the folder is a 404. Dot-segments other than
//!   `.well-known` are never served.
//! - **MIME** by extension ([`content_type`], moved here from the `--skin`
//!   helper, which keeps its own path list).
//! - **Caching.** Weak ETags from the Seed hash and the path (the Seed is
//!   content-addressed, so the pair names the bytes); `If-None-Match` gives
//!   304; `Cache-Control` per `static_site::Site::cache_control`.
//! - **Pre-compressed siblings** (`<file>.br`, then `<file>.gz`) when
//!   `Accept-Encoding` allows them (`Vary: Accept-Encoding`).
//! - `GET` and `HEAD` only (else 405 with `Allow: GET, HEAD`), SPA
//!   fallback, the 404 file, keep-alive.
//! - Every answer carries `X-Content-Type-Options: nosniff` and the
//!   caller's header lines (the App's frame policy and its allowed
//!   `host.headers`).
//! - Files stream from disk: nothing reads a whole file into memory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_core::deploy::static_site::{self, Site};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest request head read.
const MAX_HEAD: usize = 16 * 1024;
/// A keep-alive connection waits this long for its next request.
const IDLE: Duration = Duration::from_secs(60);
/// A started request head must arrive within this.
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);

/// What one folder serves.
#[derive(Debug, Clone)]
pub struct Served {
    /// The canonical folder (after links).
    pub root: PathBuf,
    pub site: Site,
    /// Names the bytes for ETags (a Seed hash, or a short prefix of one).
    pub etag_base: String,
    /// Header lines on every answer (`Name: value\r\n` each): the frame
    /// policy and the allowed headers.
    pub extra_lines: String,
}

impl Served {
    /// A folder served with `site`'s rules. `root` is canonicalized here;
    /// `frame_lines` are the caller's framing header lines.
    pub fn new(root: &Path, site: Site, etag_base: &str, frame_lines: &str) -> std::io::Result<Self> {
        let root = dunce::canonicalize(root)?;
        let mut extra = frame_lines.to_string();
        for (k, v) in &site.headers {
            extra.push_str(&format!("{k}: {v}\r\n"));
        }
        let base: String = etag_base.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect();
        Ok(Self { root, site, etag_base: base, extra_lines: extra })
    }
}

/// Content-Type by (lower-case) extension. Browsers refuse `.wasm` for
/// streaming compile and `.mjs` as a module script unless the type is
/// exact, so unknown types stay `application/octet-stream`.
pub fn content_type(ext: Option<&str>) -> &'static str {
    match ext {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs" | "cjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        _ => "application/octet-stream",
    }
}

/// [`content_type`] for a path.
pub fn content_type_of(p: &Path) -> &'static str {
    let ext = p.extension().and_then(|s| s.to_str()).map(str::to_ascii_lowercase);
    content_type(ext.as_deref())
}

/// One parsed request head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Req {
    pub method: String,
    /// The path before any `?`.
    pub path: String,
    /// `?…` with the `?`, or empty.
    pub query: String,
    pub keep_alive: bool,
    /// The request says it carries a body (the connection closes after).
    pub has_body: bool,
    pub if_none_match: Option<String>,
    pub accept_encoding: Option<String>,
}

/// Parse a head (`None` = not HTTP/1.x).
pub fn parse_head(head: &[u8]) -> Option<Req> {
    let mut hs = [httparse::EMPTY_HEADER; 64];
    let mut r = httparse::Request::new(&mut hs);
    match r.parse(head) {
        Ok(httparse::Status::Complete(_)) => {}
        _ => return None,
    }
    let method = r.method?.to_string();
    let target = r.path?;
    let version = r.version?;
    let get = |name: &str| r.headers.iter().find(|h| h.name.eq_ignore_ascii_case(name)).and_then(|h| std::str::from_utf8(h.value).ok()).map(str::trim);
    let conn = get("connection").map(str::to_ascii_lowercase);
    let keep_alive = match version {
        1 => !conn.as_deref().is_some_and(|c| c.split(',').any(|t| t.trim() == "close")),
        _ => conn.as_deref().is_some_and(|c| c.split(',').any(|t| t.trim() == "keep-alive")),
    };
    let has_body = get("transfer-encoding").is_some() || get("content-length").is_some_and(|l| l != "0");
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), format!("?{q}")),
        None => (target.to_string(), String::new()),
    };
    Some(Req {
        method,
        path,
        query,
        keep_alive,
        has_body,
        if_none_match: get("if-none-match").map(str::to_string),
        accept_encoding: get("accept-encoding").map(str::to_string),
    })
}

/// Does `Accept-Encoding` allow `coding` (present, and not `q=0`)?
pub fn accepts(accept: Option<&str>, coding: &str) -> bool {
    let Some(a) = accept else { return false };
    a.split(',').any(|item| {
        let mut parts = item.split(';');
        let name = parts.next().unwrap_or("").trim();
        if !name.eq_ignore_ascii_case(coding) {
            return false;
        }
        !parts.any(|p| {
            let p = p.trim();
            p.strip_prefix("q=").and_then(|q| q.trim().parse::<f32>().ok()).is_some_and(|q| q <= 0.0)
        })
    })
}

/// What a request resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Send this file (`rel` under the root, for the cache rule and ETag).
    File { path: PathBuf, rel: String, status: u16, encoding: Option<&'static str>, len: u64 },
    /// A folder asked for without its `/`.
    Redirect(String),
    /// K2's plain 404.
    NotFound,
}

/// A file under the root (after links), or `None`.
async fn jailed_file(root: &Path, rel: &str) -> Option<(PathBuf, u64)> {
    let canon = tokio::fs::canonicalize(root.join(rel)).await.ok()?;
    if !canon.starts_with(root) {
        return None;
    }
    let md = tokio::fs::metadata(&canon).await.ok()?;
    md.is_file().then_some((canon, md.len()))
}

async fn jailed_dir(root: &Path, rel: &str) -> bool {
    match tokio::fs::canonicalize(root.join(rel)).await {
        Ok(c) => c.starts_with(root) && tokio::fs::metadata(&c).await.is_ok_and(|m| m.is_dir()),
        Err(_) => false,
    }
}

/// A path segment K2 never serves: dot-files and dot-folders, except
/// `.well-known`.
fn hidden(rel: &str) -> bool {
    rel.split('/').any(|s| s.starts_with('.') && s != ".well-known")
}

/// Resolve a request path against `s` (Accept-Encoding for siblings).
pub async fn resolve(s: &Served, path: &str, query: &str, accept: Option<&str>) -> Target {
    let Some(rel) = static_site::request_rel(path) else { return Target::NotFound };
    if hidden(&rel) {
        return Target::NotFound;
    }
    let want = if rel.is_empty() || rel.ends_with('/') { format!("{rel}index.html") } else { rel.clone() };
    if let Some((p, len)) = jailed_file(&s.root, &want).await {
        return file_target(s, p, want, 200, len, accept).await;
    }
    if !rel.is_empty() && !rel.ends_with('/') && jailed_dir(&s.root, &rel).await && jailed_file(&s.root, &format!("{rel}/index.html")).await.is_some() {
        // One leading `/` only: `//host/x` must never become a
        // protocol-relative redirect to another site.
        return Target::Redirect(format!("/{}/{query}", path.trim_start_matches('/').trim_end_matches('/')));
    }
    if s.site.spa && !static_site::has_extension(&rel) {
        if let Some((p, len)) = jailed_file(&s.root, "index.html").await {
            return file_target(s, p, "index.html".into(), 200, len, accept).await;
        }
    }
    if let Some(nf) = s.site.not_found.as_deref() {
        if let Some((p, len)) = jailed_file(&s.root, nf).await {
            return file_target(s, p, nf.to_string(), 404, len, accept).await;
        }
    }
    Target::NotFound
}

async fn file_target(s: &Served, path: PathBuf, rel: String, status: u16, len: u64, accept: Option<&str>) -> Target {
    for (coding, ext) in [("br", "br"), ("gzip", "gz")] {
        if accepts(accept, coding) {
            if let Some((p, l)) = jailed_file(&s.root, &format!("{rel}.{ext}")).await {
                return Target::File { path: p, rel, status, encoding: Some(coding), len: l };
            }
        }
    }
    Target::File { path, rel, status, encoding: None, len }
}

/// The weak ETag for `rel` (and its coding) under `base`.
pub fn etag(base: &str, rel: &str, encoding: Option<&str>) -> String {
    // FNV-1a over the path and coding: the Seed hash already names the
    // bytes; this only tells the files of one Seed apart.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in rel.bytes().chain(encoding.unwrap_or("").bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("W/\"{base}-{h:016x}\"")
}

/// Does `If-None-Match` name `tag` (weak comparison)?
pub fn not_modified(if_none_match: Option<&str>, tag: &str) -> bool {
    let Some(inm) = if_none_match else { return false };
    let bare = |t: &str| t.trim().trim_start_matches("W/").to_string();
    let want = bare(tag);
    inm.split(',').any(|t| t.trim() == "*" || bare(t) == want)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        301 => "Moved Permanently",
        304 => "Not Modified",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        _ => "",
    }
}

fn head_text(status: u16, lines: &str, extra: &str, keep_alive: bool) -> String {
    format!(
        "HTTP/1.1 {status} {}\r\n{lines}X-Content-Type-Options: nosniff\r\n{extra}Connection: {}\r\n\r\n",
        reason(status),
        if keep_alive { "keep-alive" } else { "close" }
    )
}

const NOT_FOUND_BODY: &str = "Not found.\r\n";

/// Answer one request. Returns whether the connection may stay open.
pub async fn answer<S>(stream: &mut S, s: &Served, req: &Req) -> std::io::Result<bool>
where
    S: AsyncWrite + Unpin,
{
    let keep = req.keep_alive && !req.has_body;
    let head_only = req.method == "HEAD";
    if req.method != "GET" && !head_only {
        let body = "Method not allowed: this is a static site (GET and HEAD only).\r\n";
        let h = head_text(
            405,
            &format!("Allow: GET, HEAD\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-cache\r\n", body.len()),
            &s.extra_lines,
            false,
        );
        stream.write_all(h.as_bytes()).await?;
        stream.write_all(body.as_bytes()).await?;
        stream.flush().await?;
        return Ok(false);
    }
    match resolve(s, &req.path, &req.query, req.accept_encoding.as_deref()).await {
        Target::Redirect(loc) => {
            let h = head_text(301, &format!("Location: {loc}\r\nContent-Length: 0\r\nCache-Control: no-cache\r\n"), &s.extra_lines, keep);
            stream.write_all(h.as_bytes()).await?;
        }
        Target::NotFound => {
            let h = head_text(
                404,
                &format!("Content-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-cache\r\n", NOT_FOUND_BODY.len()),
                &s.extra_lines,
                keep,
            );
            stream.write_all(h.as_bytes()).await?;
            if !head_only {
                stream.write_all(NOT_FOUND_BODY.as_bytes()).await?;
            }
        }
        Target::File { path, rel, status, encoding, len } => {
            let tag = etag(&s.etag_base, &rel, encoding);
            let cache = s.site.cache_control(&rel);
            // The type is the file's own, not its `.br` / `.gz` sibling's.
            let ctype = content_type_of(Path::new(&rel));
            let enc = encoding.map(|e| format!("Content-Encoding: {e}\r\n")).unwrap_or_default();
            if status == 200 && not_modified(req.if_none_match.as_deref(), &tag) {
                let h = head_text(304, &format!("ETag: {tag}\r\nCache-Control: {cache}\r\nVary: Accept-Encoding\r\n"), &s.extra_lines, keep);
                stream.write_all(h.as_bytes()).await?;
            } else {
                let file = match tokio::fs::File::open(&path).await {
                    Ok(f) => f,
                    Err(_) => {
                        let h = head_text(404, "Content-Length: 0\r\nCache-Control: no-cache\r\n", &s.extra_lines, false);
                        stream.write_all(h.as_bytes()).await?;
                        stream.flush().await?;
                        return Ok(false);
                    }
                };
                let h = head_text(
                    status,
                    &format!(
                        "Content-Type: {ctype}\r\nContent-Length: {len}\r\n{enc}ETag: {tag}\r\nCache-Control: {cache}\r\nVary: Accept-Encoding\r\n"
                    ),
                    &s.extra_lines,
                    keep,
                );
                stream.write_all(h.as_bytes()).await?;
                if !head_only {
                    let sent = tokio::io::copy(&mut file.take(len), stream).await?;
                    if sent != len {
                        // The file shrank under us: the length promised can't
                        // be kept, so the connection ends.
                        stream.flush().await?;
                        return Ok(false);
                    }
                }
            }
        }
    }
    stream.flush().await?;
    Ok(keep)
}

/// Serve requests on one connection until it closes, idles out, or `cut`
/// fires (the instance stopped).
pub async fn serve_conn<S>(stream: &mut S, s: &Served, mut cut: tokio::sync::watch::Receiver<bool>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    loop {
        let end = tokio::select! {
            r = read_head(stream, &mut buf) => r,
            _ = cut.changed() => return,
        };
        let Some(end) = end else { return };
        let req = parse_head(&buf[..end]);
        buf.drain(..end);
        let Some(req) = req else {
            let h = head_text(400, "Content-Length: 0\r\n", &s.extra_lines, false);
            let _ = stream.write_all(h.as_bytes()).await;
            let _ = stream.shutdown().await;
            return;
        };
        let keep = tokio::select! {
            r = answer(stream, s, &req) => r.unwrap_or(false),
            _ = cut.changed() => false,
        };
        if !keep {
            let _ = stream.shutdown().await;
            return;
        }
    }
}

/// Read until a whole head is in `buf`; `Some(head length)`. `None`: the
/// peer closed, went idle, sent too much, or the head timed out (a head
/// that grew past [`MAX_HEAD`] gets a 431 first).
async fn read_head<S>(stream: &mut S, buf: &mut Vec<u8>) -> Option<usize>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let find = |b: &[u8]| b.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    if let Some(e) = find(buf) {
        return Some(e);
    }
    let mut chunk = [0u8; 4096];
    // Idle until the first byte of a head; then the head has HEAD_TIMEOUT.
    let mut deadline = (!buf.is_empty()).then(|| tokio::time::Instant::now() + HEAD_TIMEOUT);
    loop {
        let wait = match deadline {
            Some(d) => d.saturating_duration_since(tokio::time::Instant::now()),
            None => IDLE,
        };
        let n = match tokio::time::timeout(wait, stream.read(&mut chunk)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return None,
            Ok(Ok(n)) => n,
        };
        deadline.get_or_insert_with(|| tokio::time::Instant::now() + HEAD_TIMEOUT);
        buf.extend_from_slice(&chunk[..n]);
        if let Some(e) = find(buf) {
            return Some(e);
        }
        if buf.len() > MAX_HEAD {
            let h = head_text(431, "Content-Length: 0\r\n", "", false);
            let _ = stream.write_all(h.as_bytes()).await;
            return None;
        }
    }
}

#[cfg(test)]
#[path = "static_files_tests.rs"]
mod tests;
