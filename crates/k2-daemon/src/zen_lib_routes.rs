//! `/cli/zen/lib/*`: the Zen widget library's first-use downloads and CDN
//! cache (prd-zen-user-widgets-v2 UWB15; Rosson R2, R8 2026-10-08).
//!
//! The sealed widget frame never touches the network (UW14, Boundary rule
//! UWB18). When a widget's `requires.libs` names a library K2 doesn't ship
//! in the app (`source: "download"` in `src/shared/zen-lib.json`: Babylon.js,
//! Plotly) or a pinned CDN file (`{url, integrity}`), THIS daemon fetches it
//! once, checks it against the pin, and caches it under
//! `~/.k2/cache/zen-lib/`. The renderer then reads the checked text and
//! inlines it into the frame (`zen-lib-loader.ts`).
//!
//! ```text
//! POST /cli/zen/lib/fetch  {id, version} | {url, integrity}
//!      → {ok, key, files: [{name, bytes, sha256}], cached}
//! GET  /cli/zen/lib/file?id=&version=&file=   a download row's file
//! GET  /cli/zen/lib/file?integrity=<sri>       a CDN file
//!      → 200 text/javascript (the checked bytes)
//! ```
//! Errors: `{ok: false, error: <code>, message}` with codes `bad_request`
//! (400), `airgapped` (403), `unknown_lib` / `not_cached` (404),
//! `hash_mismatch` (409 for a bad cache file, 502 for a bad download),
//! `too_large` (413), `fetch_failed` (502).
//!
//! **Air-gap:** with `K2_AIRGAP` on (or a `--features airgap` build) the
//! daemon never fetches; a library that is already cached is still served,
//! because that reads only this disk. Bundled libraries never come here.
//!
//! Every cached file is re-hashed on every read: a cache file that no longer
//! matches its pin is deleted and refused.
//!
//! Routing: `zen_routes::handle` calls [`handle`] first (one hook line) so
//! the Zen rules hold: owner token only (`zen_local_only`), Member policy
//! rows, POST rows in `zen_routes::POST_ROUTES` (405 on GET), 64 KB bodies.
//! Feature key [`FEATURE`] in `/boot-status`.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value as J};
use sha2::{Digest, Sha256, Sha384, Sha512};

use k2_core::zen::stdlib::{self, LibSource, ZenLibEntry, ZenLibManifest};

use crate::cli_response::CliResponse;

/// The POST row (B2 lists it in `zen_routes::POST_ROUTES` and the policy table).
pub const POST_ROUTES: &[&str] = &["/cli/zen/lib/fetch"];
/// The GET row.
pub const GET_ROUTES: &[&str] = &["/cli/zen/lib/file"];
/// `/boot-status` feature key for these routes.
pub const FEATURE: &str = "zen-lib-v1";

/// Is `path` one of this module's routes?
pub fn is_route(path: &str) -> bool {
    POST_ROUTES.contains(&path) || GET_ROUTES.contains(&path)
}

/// Downloads run one at a time (two widgets asking for Babylon at once
/// download it once).
static FETCH_LOCK: Mutex<()> = Mutex::new(());

/// What a request needs from the outside world. [`Env::live`] is the
/// daemon's; tests build their own (temp cache, fake network).
pub struct Env<'a> {
    pub manifest: &'a ZenLibManifest,
    /// `~/.k2/cache/zen-lib` in the daemon.
    pub cache_root: PathBuf,
    pub airgapped: bool,
    /// GET `url`, refusing more than `max` bytes.
    pub fetch: &'a dyn Fn(&str, u64) -> Result<Vec<u8>, String>,
}

impl Env<'static> {
    pub fn live() -> Self {
        Env {
            manifest: stdlib::zen_lib_manifest(),
            cache_root: cache_root(),
            airgapped: k2_core::airgap::enabled(),
            fetch: &live_fetch,
        }
    }
}

/// `~/.k2/cache/zen-lib`.
pub fn cache_root() -> PathBuf {
    k2_core::paths::k2_home().join("cache").join("zen-lib")
}

/// One `/cli/zen/lib/*` request, or `None` when `path` isn't one (the
/// caller goes on with its own routes). `owner` is `token_is_owner`.
pub fn handle(path: &str, owner: bool, params: &HashMap<String, String>, body: &[u8]) -> Option<CliResponse> {
    if !is_route(path) {
        return None;
    }
    if !owner {
        return Some(crate::zen_routes::local_only());
    }
    let env = Env::live();
    Some(route(&env, path, params, body))
}

/// [`handle`] after the owner check, against `env`.
pub fn route(env: &Env<'_>, path: &str, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    let r = match path {
        "/cli/zen/lib/fetch" => handle_fetch(env, body).map(|v| CliResponse::ok_json(v.to_string())),
        "/cli/zen/lib/file" => handle_file(env, params),
        _ => Err(LibErr::new("404 Not Found", "unknown_route", format!("no zen lib route {path}"))),
    };
    r.unwrap_or_else(LibErr::into_response)
}

/// A refused request.
#[derive(Debug)]
pub struct LibErr {
    status: &'static str,
    code: &'static str,
    message: String,
}

impl LibErr {
    fn new(status: &'static str, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }
    fn bad(m: impl Into<String>) -> Self {
        Self::new("400 Bad Request", "bad_request", m)
    }
    pub fn code(&self) -> &'static str {
        self.code
    }
    fn into_response(self) -> CliResponse {
        CliResponse {
            status: self.status,
            content_type: "application/json",
            body: json!({ "ok": false, "error": self.code, "message": self.message }).to_string(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchBody {
    id: Option<String>,
    version: Option<String>,
    url: Option<String>,
    integrity: Option<String>,
}

fn handle_fetch(env: &Env<'_>, body: &[u8]) -> Result<J, LibErr> {
    let b: FetchBody = serde_json::from_slice(body)
        .map_err(|e| LibErr::bad(format!("lib/fetch takes {{id, version}} or {{url, integrity}}: {e}")))?;
    match (b.id, b.version, b.url, b.integrity) {
        (Some(id), Some(version), None, None) => fetch_named(env, &id, &version),
        (None, None, Some(url), Some(integrity)) => fetch_cdn(env, &url, &integrity),
        _ => Err(LibErr::bad("lib/fetch takes {id, version} or {url, integrity}")),
    }
}

/// The manifest row for an exact `id@version` that K2 downloads.
fn download_entry<'m>(m: &'m ZenLibManifest, id: &str, version: &str) -> Result<&'m ZenLibEntry, LibErr> {
    let e = m
        .libs
        .iter()
        .find(|e| e.id == id && e.version == version)
        .ok_or_else(|| LibErr::new("404 Not Found", "unknown_lib", format!("K2 has no library {id}@{version}")))?;
    if e.source == LibSource::Bundled {
        return Err(LibErr::bad(format!("{id}@{version} ships inside the K2 app; there is nothing to download")));
    }
    Ok(e)
}

/// `name` must be a flat file name (it becomes a path under the cache).
fn flat(name: &str) -> Result<&str, LibErr> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@'));
    if ok {
        Ok(name)
    } else {
        Err(LibErr::bad(format!("'{name}' isn't a library file name")))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), LibErr> {
    let io = |e: std::io::Error| LibErr::new("500 Internal Server Error", "io", format!("{}: {e}", path.display()));
    let dir = path.parent().ok_or_else(|| LibErr::new("500 Internal Server Error", "io", "no parent"))?;
    std::fs::create_dir_all(dir).map_err(io)?;
    let tmp = dir.join(format!(".{}.part-{}", path.file_name().and_then(|n| n.to_str()).unwrap_or("f"), std::process::id()));
    std::fs::write(&tmp, bytes).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

/// A cached file's bytes when it exists and still matches `check`; a file
/// that doesn't match is deleted. `Ok(None)` = not cached.
fn read_cached(path: &Path, check: &dyn Fn(&[u8]) -> bool) -> Result<Option<Vec<u8>>, LibErr> {
    match std::fs::read(path) {
        Ok(bytes) if check(&bytes) => Ok(Some(bytes)),
        Ok(_) => {
            let _ = std::fs::remove_file(path);
            Err(LibErr::new(
                "409 Conflict",
                "hash_mismatch",
                format!("{} no longer matches its pin; K2 deleted it. Fetch it again.", path.display()),
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(LibErr::new("500 Internal Server Error", "io", format!("{}: {e}", path.display()))),
    }
}

fn refuse_airgap(env: &Env<'_>, what: &str) -> Result<(), LibErr> {
    if env.airgapped {
        return Err(LibErr::new(
            "403 Forbidden",
            "airgapped",
            format!(
                "This computer is air-gapped (K2_AIRGAP), so K2 won't download {what}. Libraries that ship inside the K2 app still work."
            ),
        ));
    }
    Ok(())
}

fn download(env: &Env<'_>, url: &str, what: &str) -> Result<Vec<u8>, LibErr> {
    refuse_airgap(env, what)?;
    let max = env.manifest.max_bytes_per_widget;
    let bytes = (env.fetch)(url, max)
        .map_err(|e| LibErr::new("502 Bad Gateway", "fetch_failed", format!("couldn't download {what} from {url}: {e}")))?;
    if bytes.len() as u64 > max {
        return Err(LibErr::new(
            "413 Payload Too Large",
            "too_large",
            format!("{what} is over the {max}-byte per-widget library limit"),
        ));
    }
    Ok(bytes)
}

fn fetch_named(env: &Env<'_>, id: &str, version: &str) -> Result<J, LibErr> {
    let entry = download_entry(env.manifest, id, version)?;
    let key = entry.key();
    let _one = FETCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all_cached = true;
    let mut files = Vec::new();
    for f in &entry.files {
        let name = flat(&f.name)?;
        let path = env.cache_root.join(&key).join(name);
        let want = f.sha256.clone();
        let cached = match read_cached(&path, &|b| sha256_hex(b) == want) {
            Ok(c) => c,
            // A bad cache file was deleted; download it again below.
            Err(e) if e.code == "hash_mismatch" => None,
            Err(e) => return Err(e),
        };
        if cached.is_none() {
            all_cached = false;
            let url = f
                .url
                .as_deref()
                .ok_or_else(|| LibErr::new("500 Internal Server Error", "bad_manifest", format!("{key}/{name} has no url")))?;
            stdlib::check_cdn(env.manifest, url, "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
                .map_err(|e| LibErr::new("500 Internal Server Error", "bad_manifest", format!("{key}/{name}: {e}")))?;
            let bytes = download(env, url, &format!("{} {}", entry.title, entry.version))?;
            let got = sha256_hex(&bytes);
            if got != f.sha256 {
                return Err(LibErr::new(
                    "502 Bad Gateway",
                    "hash_mismatch",
                    format!("{url} sent bytes with sha256 {got}, not the pinned {}; K2 refused them", f.sha256),
                ));
            }
            write_atomic(&path, &bytes)?;
        }
        files.push(json!({ "name": f.name, "bytes": f.bytes, "sha256": f.sha256 }));
    }
    Ok(json!({ "ok": true, "key": key, "files": files, "cached": all_cached }))
}

/// An SRI value split into its algorithm and digest bytes.
struct Sri {
    algo: &'static str,
    digest: Vec<u8>,
}

impl Sri {
    fn parse(integrity: &str) -> Result<Self, LibErr> {
        let (algo, b64) = integrity
            .split_once('-')
            .ok_or_else(|| LibErr::bad("integrity must be an SRI hash: sha256-…, sha384-… or sha512-…"))?;
        let (algo, len) = match algo {
            "sha256" => ("sha256", 32),
            "sha384" => ("sha384", 48),
            "sha512" => ("sha512", 64),
            _ => return Err(LibErr::bad("integrity must be sha256, sha384 or sha512")),
        };
        let digest = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|_| LibErr::bad("integrity isn't valid base64"))?;
        if digest.len() != len {
            return Err(LibErr::bad(format!("a {algo} integrity is {len} bytes")));
        }
        Ok(Sri { algo, digest })
    }

    fn matches(&self, bytes: &[u8]) -> bool {
        let got: Vec<u8> = match self.algo {
            "sha256" => Sha256::digest(bytes).to_vec(),
            "sha384" => Sha384::digest(bytes).to_vec(),
            _ => Sha512::digest(bytes).to_vec(),
        };
        // Not secret; a plain compare is fine.
        got == self.digest
    }

    /// The cache file for this hash: `cdn/<algo>-<hex>.js`.
    fn path(&self, root: &Path) -> PathBuf {
        root.join("cdn").join(format!("{}-{}.js", self.algo, hex(&self.digest)))
    }
}

fn fetch_cdn(env: &Env<'_>, url: &str, integrity: &str) -> Result<J, LibErr> {
    stdlib::check_cdn(env.manifest, url, integrity).map_err(LibErr::bad)?;
    let sri = Sri::parse(integrity)?;
    let name = url.rsplit('/').next().unwrap_or("lib.js").to_string();
    let path = sri.path(&env.cache_root);
    let _one = FETCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (bytes, cached) = match read_cached(&path, &|b| sri.matches(b)) {
        Ok(Some(b)) => (b, true),
        Ok(None) | Err(LibErr { code: "hash_mismatch", .. }) => {
            let bytes = download(env, url, url)?;
            if !sri.matches(&bytes) {
                return Err(LibErr::new(
                    "502 Bad Gateway",
                    "hash_mismatch",
                    format!("{url} doesn't match its integrity {integrity}; K2 refused it"),
                ));
            }
            write_atomic(&path, &bytes)?;
            (bytes, false)
        }
        Err(e) => return Err(e),
    };
    Ok(json!({
        "ok": true,
        "key": format!("cdn:{integrity}"),
        "files": [{ "name": name, "bytes": bytes.len(), "sha256": sha256_hex(&bytes) }],
        "cached": cached,
    }))
}

fn text_response(bytes: Vec<u8>, what: &str) -> Result<CliResponse, LibErr> {
    let body = String::from_utf8(bytes)
        .map_err(|_| LibErr::new("502 Bad Gateway", "fetch_failed", format!("{what} isn't UTF-8 text")))?;
    Ok(CliResponse { status: "200 OK", content_type: "text/javascript; charset=utf-8", body })
}

fn not_cached(what: &str) -> LibErr {
    LibErr::new("404 Not Found", "not_cached", format!("{what} isn't downloaded yet; POST /cli/zen/lib/fetch first"))
}

fn handle_file(env: &Env<'_>, params: &HashMap<String, String>) -> Result<CliResponse, LibErr> {
    let p = |k: &str| params.get(k).map(|s| s.trim()).filter(|s| !s.is_empty());
    match (p("id"), p("version"), p("file"), p("integrity")) {
        (Some(id), Some(version), Some(file), None) => {
            let entry = download_entry(env.manifest, id, version)?;
            let f = entry
                .files
                .iter()
                .find(|f| f.name == file)
                .ok_or_else(|| LibErr::new("404 Not Found", "unknown_lib", format!("{} has no file {file}", entry.key())))?;
            let path = env.cache_root.join(entry.key()).join(flat(&f.name)?);
            let want = f.sha256.clone();
            let what = format!("{}/{}", entry.key(), f.name);
            let bytes = read_cached(&path, &|b| sha256_hex(b) == want)?.ok_or_else(|| not_cached(&what))?;
            text_response(bytes, &what)
        }
        (None, None, None, Some(integrity)) => {
            let sri = Sri::parse(integrity)?;
            let bytes = read_cached(&sri.path(&env.cache_root), &|b| sri.matches(b))?.ok_or_else(|| not_cached(integrity))?;
            text_response(bytes, integrity)
        }
        _ => Err(LibErr::bad("lib/file takes ?id=&version=&file= or ?integrity=")),
    }
}

/// The daemon's downloader: https only, redirects only to an allowed CDN
/// host over https, at most `max` bytes read.
fn live_fetch(url: &str, max: u64) -> Result<Vec<u8>, String> {
    let hosts: Vec<String> = stdlib::zen_lib_manifest().cdn_hosts.clone();
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            let u = attempt.url();
            let ok = u.scheme() == "https" && u.host_str().is_some_and(|h| hosts.iter().any(|a| a == h));
            if !ok {
                attempt.error("redirect away from the allowed CDNs")
            } else if attempt.previous().len() > 5 {
                attempt.error("too many redirects")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client.get(url).send().map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("http status {}", resp.status()));
    }
    // Read at most max + 1 bytes: one byte over is enough to refuse it.
    let mut bytes = Vec::new();
    resp.take(max + 1).read_to_end(&mut bytes).map_err(|e| format!("read body: {e}"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::zen::stdlib::ZenLibFile;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU32, Ordering};

    const BODY: &[u8] = b"var BIGLIB={v:1};";
    const URL: &str = "https://cdn.jsdelivr.net/npm/biglib@2.0.0/biglib.js";
    const CDN_URL: &str = "https://unpkg.com/tiny@1.2.3/dist/tiny.js";
    const CDN_BODY: &[u8] = b"var TINY=1;";

    fn temp_root() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "k2-zen-lib-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("temp dir");
        d
    }

    fn manifest() -> ZenLibManifest {
        let mut m = stdlib::zen_lib_manifest().clone();
        m.max_bytes_per_widget = 64;
        m.libs = vec![
            ZenLibEntry {
                id: "biglib".into(),
                version: "2.0.0".into(),
                title: "BigLib".into(),
                global: "BIGLIB".into(),
                license: "MIT".into(),
                license_file: "LICENSE.txt".into(),
                copyright: "c".into(),
                source: LibSource::Download,
                files: vec![ZenLibFile {
                    name: "biglib.js".into(),
                    bytes: BODY.len() as u64,
                    sha256: sha256_hex(BODY),
                    url: Some(URL.into()),
                }],
                prebundled: None,
                notes: None,
            },
            ZenLibEntry {
                id: "shipped".into(),
                version: "1.0.0".into(),
                title: "Shipped".into(),
                global: "S".into(),
                license: "MIT".into(),
                license_file: "LICENSE.txt".into(),
                copyright: "c".into(),
                source: LibSource::Bundled,
                files: vec![ZenLibFile { name: "s.js".into(), bytes: 1, sha256: "00".into(), url: None }],
                prebundled: None,
                notes: None,
            },
        ];
        m
    }

    fn sri384(b: &[u8]) -> String {
        format!("sha384-{}", base64::engine::general_purpose::STANDARD.encode(Sha384::digest(b)))
    }

    struct Net {
        calls: RefCell<Vec<String>>,
        answer: RefCell<Result<Vec<u8>, String>>,
    }

    impl Net {
        fn new(answer: Result<Vec<u8>, String>) -> Self {
            Net { calls: RefCell::new(vec![]), answer: RefCell::new(answer) }
        }
    }

    fn call(m: &ZenLibManifest, root: &Path, airgapped: bool, net: &Net, path: &str, params: &[(&str, &str)], body: J) -> (String, J, String) {
        let fetch = |url: &str, _max: u64| {
            net.calls.borrow_mut().push(url.to_string());
            net.answer.borrow().clone()
        };
        let env = Env { manifest: m, cache_root: root.to_path_buf(), airgapped, fetch: &fetch };
        let params: HashMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let body = if body.is_null() { Vec::new() } else { body.to_string().into_bytes() };
        let r = route(&env, path, &params, &body);
        let json = serde_json::from_str(&r.body).unwrap_or(J::Null);
        (r.status.to_string(), json, r.body)
    }

    fn fetch_named_body() -> J {
        json!({"id": "biglib", "version": "2.0.0"})
    }

    const FILE_Q: &[(&str, &str)] = &[("id", "biglib"), ("version", "2.0.0"), ("file", "biglib.js")];

    #[test]
    fn named_download_is_fetched_once_checked_cached_and_served() {
        let (m, root, net) = (manifest(), temp_root(), Net::new(Ok(BODY.to_vec())));
        let (st, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/file", FILE_Q, J::Null);
        assert_eq!((st.as_str(), j["error"].as_str()), ("404 Not Found", Some("not_cached")));

        let (st, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/fetch", &[], fetch_named_body());
        assert_eq!(st, "200 OK", "{j}");
        assert_eq!(j["cached"], json!(false));
        assert_eq!(j["key"], json!("biglib@2.0.0"));
        assert_eq!(j["files"][0]["sha256"], json!(sha256_hex(BODY)));
        assert_eq!(net.calls.borrow().as_slice(), [URL.to_string()]);
        assert!(root.join("biglib@2.0.0/biglib.js").is_file());

        let (st, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/fetch", &[], fetch_named_body());
        assert_eq!((st.as_str(), &j["cached"]), ("200 OK", &json!(true)));
        assert_eq!(net.calls.borrow().len(), 1, "a cached library is not downloaded again");

        let (st, _, body) = call(&m, &root, false, &net, "/cli/zen/lib/file", FILE_Q, J::Null);
        assert_eq!((st.as_str(), body.as_bytes()), ("200 OK", BODY));
    }

    #[test]
    fn air_gap_never_fetches_but_serves_what_is_cached() {
        let (m, root, net) = (manifest(), temp_root(), Net::new(Ok(BODY.to_vec())));
        let (st, j, _) = call(&m, &root, true, &net, "/cli/zen/lib/fetch", &[], fetch_named_body());
        assert_eq!((st.as_str(), j["error"].as_str()), ("403 Forbidden", Some("airgapped")));
        assert!(j["message"].as_str().is_some_and(|s| s.contains("air-gapped")), "{j}");
        assert!(net.calls.borrow().is_empty(), "air-gap must not touch the network");
        let (st, j, _) = call(&m, &root, true, &net, "/cli/zen/lib/fetch", &[], json!({"url": CDN_URL, "integrity": sri384(CDN_BODY)}));
        assert_eq!((st.as_str(), j["error"].as_str()), ("403 Forbidden", Some("airgapped")));
        assert!(net.calls.borrow().is_empty());

        // Downloaded before air-gap went on: still served from this disk.
        call(&m, &root, false, &net, "/cli/zen/lib/fetch", &[], fetch_named_body());
        let (st, j, _) = call(&m, &root, true, &net, "/cli/zen/lib/fetch", &[], fetch_named_body());
        assert_eq!((st.as_str(), &j["cached"]), ("200 OK", &json!(true)));
        let (st, _, body) = call(&m, &root, true, &net, "/cli/zen/lib/file", FILE_Q, J::Null);
        assert_eq!((st.as_str(), body.as_bytes()), ("200 OK", BODY));
    }

    #[test]
    fn wrong_bytes_too_many_bytes_and_network_errors_are_refused_and_not_cached() {
        let m = manifest();
        let root = temp_root();
        let (st, j, _) = call(&m, &root, false, &Net::new(Ok(b"var EVIL=1;".to_vec())), "/cli/zen/lib/fetch", &[], fetch_named_body());
        assert_eq!((st.as_str(), j["error"].as_str()), ("502 Bad Gateway", Some("hash_mismatch")));
        let (st, j, _) = call(&m, &root, false, &Net::new(Ok(vec![b'x'; 65])), "/cli/zen/lib/fetch", &[], fetch_named_body());
        assert_eq!((st.as_str(), j["error"].as_str()), ("413 Payload Too Large", Some("too_large")));
        let (st, j, _) = call(&m, &root, false, &Net::new(Err("dns".into())), "/cli/zen/lib/fetch", &[], fetch_named_body());
        assert_eq!((st.as_str(), j["error"].as_str()), ("502 Bad Gateway", Some("fetch_failed")));
        assert!(!root.join("biglib@2.0.0/biglib.js").exists(), "nothing bad is cached");
    }

    #[test]
    fn a_tampered_cache_file_is_deleted_and_refused_then_fetched_again() {
        let (m, root, net) = (manifest(), temp_root(), Net::new(Ok(BODY.to_vec())));
        call(&m, &root, false, &net, "/cli/zen/lib/fetch", &[], fetch_named_body());
        let cached = root.join("biglib@2.0.0/biglib.js");
        std::fs::write(&cached, b"var EVIL=1;").expect("tamper");
        let (st, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/file", FILE_Q, J::Null);
        assert_eq!((st.as_str(), j["error"].as_str()), ("409 Conflict", Some("hash_mismatch")));
        assert!(!cached.exists(), "the bad file is deleted");
        let (st, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/fetch", &[], fetch_named_body());
        assert_eq!((st.as_str(), &j["cached"]), ("200 OK", &json!(false)));
        assert_eq!(net.calls.borrow().len(), 2);
    }

    #[test]
    fn named_requests_are_checked() {
        let (m, root, net) = (manifest(), temp_root(), Net::new(Ok(BODY.to_vec())));
        let cases = [
            (json!({"id": "nope", "version": "1.0.0"}), "404 Not Found", "unknown_lib"),
            (json!({"id": "biglib", "version": "2.0"}), "404 Not Found", "unknown_lib"),
            (json!({"id": "shipped", "version": "1.0.0"}), "400 Bad Request", "bad_request"),
            (json!({"id": "biglib"}), "400 Bad Request", "bad_request"),
            (json!({"id": "biglib", "version": "2.0.0", "url": URL}), "400 Bad Request", "bad_request"),
            (json!({"id": "biglib", "version": "2.0.0", "extra": 1}), "400 Bad Request", "bad_request"),
        ];
        for (body, status, code) in cases {
            let (st, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/fetch", &[], body.clone());
            assert_eq!((st.as_str(), j["error"].as_str()), (status, Some(code)), "{body}");
        }
        let (st, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/file", &[("id", "biglib"), ("version", "2.0.0"), ("file", "../x")], J::Null);
        assert_eq!((st.as_str(), j["error"].as_str()), ("404 Not Found", Some("unknown_lib")));
        let (st, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/file", &[("id", "biglib")], J::Null);
        assert_eq!((st.as_str(), j["error"].as_str()), ("400 Bad Request", Some("bad_request")));
        assert!(net.calls.borrow().is_empty());
    }

    #[test]
    fn cdn_files_are_checked_by_integrity_and_cached_by_hash() {
        let (m, root, net) = (manifest(), temp_root(), Net::new(Ok(CDN_BODY.to_vec())));
        let sri = sri384(CDN_BODY);
        let (st, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/fetch", &[], json!({"url": CDN_URL, "integrity": sri}));
        assert_eq!(st, "200 OK", "{j}");
        assert_eq!(j["files"][0]["name"], json!("tiny.js"));
        assert_eq!(j["files"][0]["bytes"], json!(CDN_BODY.len()));
        let (st, _, body) = call(&m, &root, false, &net, "/cli/zen/lib/file", &[("integrity", &sri)], J::Null);
        assert_eq!((st.as_str(), body.as_bytes()), ("200 OK", CDN_BODY));
        let (_, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/fetch", &[], json!({"url": CDN_URL, "integrity": sri}));
        assert_eq!(j["cached"], json!(true));
        assert_eq!(net.calls.borrow().len(), 1);

        let other = sri384(b"something else");
        let (st, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/fetch", &[], json!({"url": CDN_URL, "integrity": other}));
        assert_eq!((st.as_str(), j["error"].as_str()), ("502 Bad Gateway", Some("hash_mismatch")));

        for (url, integrity) in [
            ("https://evil.example.test/x@1.2.3/x.js", sri.as_str()),
            ("http://unpkg.com/tiny@1.2.3/dist/tiny.js", sri.as_str()),
            ("https://unpkg.com/tiny@latest/dist/tiny.js", sri.as_str()),
            (CDN_URL, "md5-AAAA"),
            (CDN_URL, "sha384-not*base64AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ] {
            let (st, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/fetch", &[], json!({"url": url, "integrity": integrity}));
            assert_eq!((st.as_str(), j["error"].as_str()), ("400 Bad Request", Some("bad_request")), "{url} {integrity}");
        }
        let (st, j, _) = call(&m, &root, false, &net, "/cli/zen/lib/file", &[("integrity", &other)], J::Null);
        assert_eq!((st.as_str(), j["error"].as_str()), ("404 Not Found", Some("not_cached")));
    }

    #[test]
    fn handle_routes_only_its_paths_and_only_the_owner() {
        let none = HashMap::new();
        assert!(handle("/cli/zen/get", true, &none, b"").is_none());
        assert!(handle("/cli/zen/libx", true, &none, b"").is_none());
        let r = handle("/cli/zen/lib/fetch", false, &none, b"{}").expect("a lib route");
        assert_eq!(r.status, "403 Forbidden");
        assert!(r.body.contains("zen_local_only"));
        let r = handle("/cli/zen/lib/file", false, &none, b"").expect("a lib route");
        assert_eq!(r.status, "403 Forbidden");
    }

    #[test]
    fn the_live_cache_is_under_k2_home_and_the_real_download_rows_are_well_formed() {
        assert!(cache_root().ends_with(".k2/cache/zen-lib"));
        let m = stdlib::zen_lib_manifest();
        let downloads: Vec<_> = m.libs.iter().filter(|l| l.source == LibSource::Download).collect();
        assert!(!downloads.is_empty());
        for l in downloads {
            for f in &l.files {
                flat(&f.name).expect("flat name");
                let url = f.url.as_deref().expect("download rows carry a url");
                stdlib::check_cdn(m, url, "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").expect("allowed CDN, exact version");
                assert!(f.bytes <= m.max_bytes_per_widget, "{}", l.key());
                assert_eq!(f.sha256.len(), 64);
            }
        }
        assert_eq!(FEATURE, "zen-lib-v1");
        assert!(POST_ROUTES.iter().chain(GET_ROUTES).all(|p| p.starts_with("/cli/zen/lib/")));
    }
}
