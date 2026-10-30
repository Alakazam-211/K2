//! Sew release auto-fetch (0.46.7).
//!
//! Enable, upgrade, voice install and rollback used to need a local
//! release tarball (`--tarball`). Now, with none given, the daemon takes
//! the pinned file for this box's CPU from K2's own cache, else downloads
//! it from Sew's public releases:
//!
//! `https://github.com/Alakazam-211/sew-releases/releases/download/v<ver>/<file>`
//!
//! Order: an explicit `--tarball` (the caller's), then the cache
//! (`~/.k2/cache/sew/<ver>/`, 0700, this user's), then GitHub. There is no
//! K2 mirror for Sew releases (`dl.k2.dev` serves App updates only), so
//! none is tried.
//!
//! **Trust.** The only check that counts is the sha256 pinned in K2's code
//! (`k2_root_proto::sew`): the downloaded `.sha256` / `SHA256SUMS` files are
//! never read. The bytes are hashed while they stream; a mismatch deletes
//! the file and refuses before anything is extracted or run. The root
//! helper still checks the binary's sha256 when it installs, as before.
//!
//! **Network.** https only, to `github.com` and the hosts GitHub redirects
//! release assets to; at most 5 redirects; no token or cookie is ever sent;
//! connect timeout 15 s, a whole-download timeout, a size cap per file
//! (checked against Content-Length and again while streaming). Requests
//! run on their own thread (reqwest blocking never sits on a tokio
//! runtime).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use k2_root_proto::sew::{SewAppKit, SewArtifact, MAX_TARBALL};

/// Sew's public release downloads.
pub const RELEASES_URL: &str = "https://github.com/Alakazam-211/sew-releases/releases/download";
/// The hosts a download may touch: the release URL's own and the ones
/// GitHub redirects release assets to.
pub const GITHUB_HOSTS: [&str; 3] = ["github.com", "release-assets.githubusercontent.com", "objects.githubusercontent.com"];
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A whole download (Sew 0.5.0 is 24.7 MB; a slow line still finishes).
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// A preflight reachability check (HEAD).
const PROBE_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_REDIRECTS: usize = 5;
const CHUNK: usize = 256 * 1024;

/// Where release files are fetched from. Production is GitHub over https;
/// tests use a loopback server; [`Origin::offline`] reaches nothing.
#[derive(Debug, Clone)]
pub struct Origin {
    base: String,
    hosts: Vec<String>,
    https_only: bool,
}

impl Origin {
    pub fn github() -> Self {
        Origin { base: RELEASES_URL.to_string(), hosts: GITHUB_HOSTS.iter().map(|h| h.to_string()).collect(), https_only: true }
    }

    /// No network source (tests and fakes default to it).
    pub fn offline() -> Self {
        Origin { base: String::new(), hosts: vec![], https_only: true }
    }

    /// A plain-http server on 127.0.0.1 (tests only).
    #[cfg(test)]
    pub fn loopback(port: u16) -> Self {
        Origin { base: format!("http://127.0.0.1:{port}"), hosts: vec!["127.0.0.1".into()], https_only: false }
    }

    pub fn is_offline(&self) -> bool {
        self.base.is_empty()
    }

    /// May a request (or a redirect) go to `url`?
    pub fn allows(&self, url: &reqwest::Url) -> Result<(), String> {
        let shown = without_query(url);
        match url.scheme() {
            "https" => {}
            "http" if !self.https_only => {}
            s => return Err(format!("refused {shown}: {s} is not https")),
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(format!("refused {shown}: a URL with credentials"));
        }
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        if !self.hosts.iter().any(|h| *h == host) {
            return Err(format!("refused a redirect to {host}: not GitHub's release download"));
        }
        Ok(())
    }
}

/// Where the daemon looks for, and keeps, release files.
#[derive(Debug, Clone)]
pub struct Source {
    pub origin: Origin,
    /// `~/.k2/cache/sew` in production; `None` = no cache.
    pub cache_root: Option<PathBuf>,
}

impl Source {
    pub fn live() -> Self {
        Source { origin: Origin::github(), cache_root: Some(k2_core::paths::k2_home().join("cache").join("sew")) }
    }

    pub fn offline() -> Self {
        Source { origin: Origin::offline(), cache_root: None }
    }
}

/// One pinned file of a Sew release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseFile {
    /// The sew-releases tag, without the `v` (the core version).
    pub tag: String,
    pub name: String,
    /// The sha256 pinned in K2's code: the only one that counts.
    pub sha256: String,
    pub cap: u64,
    /// "Sew 0.5.0", "sew-media 0.5.0", "Sew's App kit 0.5.0".
    pub label: String,
}

impl ReleaseFile {
    pub fn core(version: &str, a: &SewArtifact) -> Self {
        ReleaseFile {
            tag: version.into(),
            name: format!("sew-{version}-{}.tar.gz", a.triple),
            sha256: a.tarball_sha256.into(),
            cap: MAX_TARBALL as u64,
            label: format!("Sew {version}"),
        }
    }

    /// Sew release `core_version`'s voice engine (its own version).
    pub fn media(core_version: &str, media_version: &str, a: &SewArtifact) -> Self {
        ReleaseFile {
            tag: core_version.into(),
            name: format!("sew-media-{media_version}-{}.tar.gz", a.triple),
            sha256: a.tarball_sha256.into(),
            cap: MAX_TARBALL as u64,
            label: format!("sew-media {media_version}"),
        }
    }

    /// Sew release `core_version`'s App kit.
    pub fn app_kit(core_version: &str, kit: &SewAppKit) -> Self {
        ReleaseFile {
            tag: core_version.into(),
            name: k2_core::app_sdk::sew_kit::file_name(kit.version),
            sha256: kit.sha256.into(),
            cap: k2_core::app_sdk::sew_kit::MAX_KIT as u64,
            label: format!("Sew's App kit {}", kit.version),
        }
    }

    /// The public URL (what a person fetches by hand).
    pub fn url(&self) -> String {
        format!("{RELEASES_URL}/v{}/{}", self.tag, self.name)
    }

    fn url_at(&self, o: &Origin) -> String {
        format!("{}/v{}/{}", o.base, self.tag, self.name)
    }

    /// What a box without internet does instead: `cmd` is the command that
    /// takes the file (`k2 hostchat enable --tarball`).
    pub fn by_hand(&self, cmd: &str) -> String {
        format!(
            "On a box without internet: download {} (sha256 {}) on another machine, copy it here, then run {cmd} /path/to/{}",
            self.url(),
            self.sha256,
            self.name
        )
    }
}

/// `24.7 MB`.
pub fn size_words(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// No answer, an HTTP error status, a timeout: try again, or by hand.
    Unreachable(String),
    /// The bytes are not the pinned file (deleted).
    Mismatch(String),
    /// Refused by policy: too big, a redirect off GitHub, not https.
    Refused(String),
    /// K2's cache directory is unusable.
    Cache(String),
}

impl FetchError {
    pub fn unreachable(&self) -> bool {
        matches!(self, FetchError::Unreachable(_))
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Unreachable(m) | FetchError::Mismatch(m) | FetchError::Refused(m) | FetchError::Cache(m) => f.write_str(m),
        }
    }
}

fn without_query(u: &reqwest::Url) -> String {
    let mut u = u.clone();
    u.set_query(None);
    u.set_fragment(None);
    u.to_string()
}

/// reqwest's error and its causes, without the (signed) URL.
fn why(e: reqwest::Error) -> String {
    let timeout = e.is_timeout();
    let e = e.without_url();
    let mut parts = vec![e.to_string()];
    let mut src = std::error::Error::source(&e);
    while let Some(s) = src {
        let t = s.to_string();
        if !parts.iter().any(|p| p.contains(&t)) {
            parts.push(t);
        }
        src = s.source();
    }
    let mut out = parts.join(": ");
    if timeout && !out.contains("timed out") {
        out.push_str(" (timed out)");
    }
    out
}

/// A failed request: a redirect the policy refused is `Refused`, the rest
/// `Unreachable`.
fn send_error(e: reqwest::Error) -> FetchError {
    if e.is_redirect() {
        FetchError::Refused(why(e))
    } else {
        FetchError::Unreachable(why(e))
    }
}

// ── cache ─────────────────────────────────────────────────────────────────

/// `<root>/<tag>`.
pub fn cache_dir(root: &Path, f: &ReleaseFile) -> PathBuf {
    root.join(&f.tag)
}

/// A directory only this user can use: created 0700, refused when it is a
/// symlink or another user's, tightened to 0700 when looser.
fn private_dir(d: &Path) -> Result<(), FetchError> {
    let err = |e: String| FetchError::Cache(format!("K2's download cache {}: {e}", d.display()));
    if let Some(p) = d.parent() {
        fs::create_dir_all(p).map_err(|e| err(e.to_string()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        match fs::DirBuilder::new().mode(0o700).create(d) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(err(e.to_string())),
        }
        let m = fs::symlink_metadata(d).map_err(|e| err(e.to_string()))?;
        if !m.is_dir() {
            return Err(err("not a directory (a symlink or a file is in the way)".into()));
        }
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        if m.uid() != me {
            return Err(err(format!("owned by uid {}, not this user ({me})", m.uid())));
        }
        if m.permissions().mode() & 0o777 != 0o700 {
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).map_err(|e| err(e.to_string()))?;
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(d).map_err(|e| err(e.to_string()))?;
    }
    Ok(())
}

fn sha256_file(p: &Path, cap: u64) -> std::io::Result<Option<String>> {
    let mut file = fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut n = 0u64;
    loop {
        let r = file.read(&mut buf)?;
        if r == 0 {
            break;
        }
        n += r as u64;
        if n > cap {
            return Ok(None);
        }
        h.update(&buf[..r]);
    }
    Ok(Some(hex(&h.finalize())))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The pinned file in K2's cache, verified against the pin. A file there
/// that isn't (truncated, tampered, a symlink) is deleted: `Ok(None)`, and
/// the caller downloads it again.
pub fn cached(src: &Source, f: &ReleaseFile) -> Option<PathBuf> {
    let root = src.cache_root.as_ref()?;
    let p = cache_dir(root, f).join(&f.name);
    let m = fs::symlink_metadata(&p).ok()?;
    let good = m.is_file() && m.len() <= f.cap && matches!(sha256_file(&p, f.cap), Ok(Some(h)) if h.eq_ignore_ascii_case(&f.sha256));
    if good {
        return Some(p);
    }
    k2_core::log_debug!("[sew] the cached {} is not the pinned file; removing it", p.display());
    let _ = fs::remove_file(&p);
    None
}

// ── network ───────────────────────────────────────────────────────────────

fn client(o: &Origin, timeout: Duration) -> Result<reqwest::blocking::Client, FetchError> {
    let policy_origin = o.clone();
    reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(timeout)
        .https_only(o.https_only)
        .referer(false)
        .redirect(reqwest::redirect::Policy::custom(move |a| {
            if a.previous().len() >= MAX_REDIRECTS {
                return a.error(format!("more than {MAX_REDIRECTS} redirects"));
            }
            match policy_origin.allows(a.url()) {
                Ok(()) => a.follow(),
                Err(e) => a.error(e),
            }
        }))
        .user_agent(concat!("k2-daemon/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| FetchError::Unreachable(why(e)))
}

/// Run `f` on its own thread (no ambient tokio runtime under reqwest's
/// blocking client).
fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, FetchError> {
    std::thread::Builder::new()
        .name("k2-sew-fetch".into())
        .spawn(f)
        .map_err(|e| FetchError::Unreachable(format!("spawn: {e}")))?
        .join()
        .map_err(|_| FetchError::Unreachable("the download thread panicked".into()))
}

fn start_url(o: &Origin, f: &ReleaseFile) -> Result<reqwest::Url, FetchError> {
    if o.is_offline() {
        return Err(FetchError::Unreachable("this box has no download source for Sew releases".into()));
    }
    let url = reqwest::Url::parse(&f.url_at(o)).map_err(|e| FetchError::Refused(format!("{}: {e}", f.url_at(o))))?;
    o.allows(&url).map_err(FetchError::Refused)?;
    Ok(url)
}

fn content_length(h: &reqwest::header::HeaderMap) -> Option<u64> {
    h.get(reqwest::header::CONTENT_LENGTH)?.to_str().ok()?.trim().parse().ok()
}

/// Is the pinned file reachable? Its size when the server says
/// (`HEAD`, short timeout; nothing is written).
pub fn probe(src: &Source, f: &ReleaseFile) -> Result<Option<u64>, FetchError> {
    let url = start_url(&src.origin, f)?;
    let (o, shown, cap) = (src.origin.clone(), f.url(), f.cap);
    on_thread(move || {
        let c = client(&o, PROBE_TIMEOUT)?;
        let r = c.head(url).send().map_err(send_error)?;
        o.allows(r.url()).map_err(FetchError::Refused)?;
        if !r.status().is_success() {
            return Err(FetchError::Unreachable(format!("{shown} answered HTTP {}", r.status().as_u16())));
        }
        let n = content_length(r.headers());
        if n.is_some_and(|n| n > cap) {
            return Err(FetchError::Refused(format!("{shown} is larger than K2 accepts ({} MB)", cap / 1_000_000)));
        }
        Ok(n)
    })?
}

/// Download the pinned file into K2's cache, hashing as it streams;
/// `progress(bytes, total)` on the caller's thread. The file only gets its
/// real name once its sha256 equals the pin; anything else deletes it.
pub fn download(src: &Source, f: &ReleaseFile, progress: &dyn Fn(u64, Option<u64>)) -> Result<PathBuf, FetchError> {
    let url = start_url(&src.origin, f)?;
    let root = src.cache_root.clone().ok_or_else(|| FetchError::Cache("this box has no download cache".into()))?;
    let dir = cache_dir(&root, f);
    private_dir(&root)?;
    private_dir(&dir)?;
    let (fin, part) = (dir.join(&f.name), dir.join(format!("{}.part", f.name)));
    let (o, ff, part2) = (src.origin.clone(), f.clone(), part.clone());
    let (tx, rx) = std::sync::mpsc::channel::<(u64, Option<u64>)>();
    let worker = std::thread::Builder::new()
        .name("k2-sew-fetch".into())
        .spawn(move || {
            let r = stream(&o, &ff, url, &part2, &tx);
            if r.is_err() {
                let _ = fs::remove_file(&part2);
            }
            r
        })
        .map_err(|e| FetchError::Unreachable(format!("spawn: {e}")))?;
    for (n, total) in rx {
        progress(n, total);
    }
    let hash = worker.join().map_err(|_| {
        let _ = fs::remove_file(&part);
        FetchError::Unreachable("the download thread panicked".into())
    })??;
    if !hash.eq_ignore_ascii_case(&f.sha256) {
        let _ = fs::remove_file(&part);
        return Err(FetchError::Mismatch(format!(
            "the download of {} ({}) has sha256 {hash}, but K2's pin for it is {}: the file was deleted and nothing was installed",
            f.label,
            f.url(),
            f.sha256
        )));
    }
    fs::rename(&part, &fin).map_err(|e| {
        let _ = fs::remove_file(&part);
        FetchError::Cache(format!("K2's download cache {}: {e}", dir.display()))
    })?;
    Ok(fin)
}

/// GET into `part` (0600); the sha256 of what arrived.
fn stream(o: &Origin, f: &ReleaseFile, url: reqwest::Url, part: &Path, tx: &std::sync::mpsc::Sender<(u64, Option<u64>)>) -> Result<String, FetchError> {
    let shown = f.url();
    let c = client(o, DOWNLOAD_TIMEOUT)?;
    let mut r = c.get(url).send().map_err(send_error)?;
    o.allows(r.url()).map_err(FetchError::Refused)?;
    if !r.status().is_success() {
        return Err(FetchError::Unreachable(format!("{shown} answered HTTP {}", r.status().as_u16())));
    }
    let total = content_length(r.headers());
    let too_big = || FetchError::Refused(format!("{shown} is larger than K2 accepts for {} ({} MB)", f.label, f.cap / 1_000_000));
    if total.is_some_and(|t| t > f.cap) {
        return Err(too_big());
    }
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let cache = |e: std::io::Error| FetchError::Cache(format!("K2's download cache {}: {e}", part.display()));
    let mut out = opts.open(part).map_err(cache)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut n = 0u64;
    loop {
        let got = r.read(&mut buf).map_err(|e| FetchError::Unreachable(format!("{shown}: the download broke off: {e}")))?;
        if got == 0 {
            break;
        }
        n += got as u64;
        if n > f.cap {
            return Err(too_big());
        }
        h.update(&buf[..got]);
        out.write_all(&buf[..got]).map_err(cache)?;
        let _ = tx.send((n, total));
    }
    if total.is_some_and(|t| t != n) {
        return Err(FetchError::Unreachable(format!("{shown}: the download stopped at {n} of {} bytes", total.unwrap_or(0))));
    }
    out.sync_all().map_err(cache)?;
    Ok(hex(&h.finalize()))
}

/// How a pinned file was obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obtained {
    pub path: PathBuf,
    /// `cache` or `download`.
    pub how: &'static str,
}

/// The cache, else a download.
pub fn obtain(src: &Source, f: &ReleaseFile, progress: &dyn Fn(u64, Option<u64>)) -> Result<Obtained, FetchError> {
    if let Some(path) = cached(src, f) {
        return Ok(Obtained { path, how: "cache" });
    }
    download(src, f, progress).map(|path| Obtained { path, how: "download" })
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;

/// A tiny HTTP/1.1 server on 127.0.0.1 for the fetch tests (GET and HEAD,
/// one connection at a time; `Connection: close`).
#[cfg(test)]
pub mod test_server {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    pub enum Reply {
        Bytes(Vec<u8>),
        /// 302 to this absolute URL.
        Redirect(String),
        /// Claims this Content-Length, sends the bytes.
        Lying(u64, Vec<u8>),
    }

    pub struct Server {
        pub port: u16,
        routes: Arc<Mutex<HashMap<String, Reply>>>,
        /// Every request line + its Authorization/Cookie headers (if any).
        pub seen: Arc<Mutex<Vec<String>>>,
    }

    impl Server {
        pub fn start() -> Server {
            let l = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            let port = l.local_addr().unwrap().port();
            let routes: Arc<Mutex<HashMap<String, Reply>>> = Arc::default();
            let seen: Arc<Mutex<Vec<String>>> = Arc::default();
            let (r2, s2) = (routes.clone(), seen.clone());
            std::thread::spawn(move || {
                for conn in l.incoming() {
                    let Ok(mut c) = conn else { continue };
                    let mut rd = BufReader::new(c.try_clone().unwrap());
                    let mut line = String::new();
                    if rd.read_line(&mut line).is_err() {
                        continue;
                    }
                    let mut creds = vec![];
                    loop {
                        let mut h = String::new();
                        if rd.read_line(&mut h).is_err() || h.trim().is_empty() {
                            break;
                        }
                        let lower = h.to_ascii_lowercase();
                        if lower.starts_with("authorization:") || lower.starts_with("cookie:") {
                            creds.push(h.trim().to_string());
                        }
                    }
                    let mut it = line.split_whitespace();
                    let (method, path) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string());
                    s2.lock().unwrap().push(format!("{method} {path}{}", if creds.is_empty() { String::new() } else { format!(" {}", creds.join(" ")) }));
                    let head = method == "HEAD";
                    let routes = r2.lock().unwrap();
                    let _ = match routes.get(&path) {
                        Some(Reply::Bytes(b)) => {
                            let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len());
                            if head { Ok(()) } else { c.write_all(b) }
                        }
                        Some(Reply::Lying(n, b)) => {
                            let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Length: {n}\r\nConnection: close\r\n\r\n");
                            if head { Ok(()) } else { c.write_all(b) }
                        }
                        Some(Reply::Redirect(to)) => {
                            write!(c, "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        }
                        None => write!(c, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                    };
                }
            });
            Server { port, routes, seen }
        }

        pub fn serve(&self, path: &str, r: Reply) {
            self.routes.lock().unwrap().insert(path.to_string(), r);
        }

        pub fn origin(&self) -> super::Origin {
            super::Origin::loopback(self.port)
        }
    }

    /// A fresh directory under the system temp dir, removed on drop.
    pub struct Scratch(pub std::path::PathBuf);

    impl Scratch {
        pub fn new(tag: &str) -> Scratch {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("k2-sew-fetch-{tag}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("scratch dir");
            Scratch(p)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
