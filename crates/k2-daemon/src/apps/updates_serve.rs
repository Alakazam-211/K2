//! K2's update answers on an App's own hosts (prd-tauri-core-0467-v1 TC-9,
//! D5; vs-live TA-9 … TA-11), App up or down:
//!
//! ```text
//! GET|HEAD /_k2/updates/<channel>/latest.json                       Tauri's static form
//! GET|HEAD /_k2/updates/<channel>/<target>/<arch>/<current_version>   Tauri's dynamic form: 200 or 204
//! ```
//!
//! - Every `url` is the download host's immutable URL: the box never serves
//!   an archive byte. Answers are a few KB, `Cache-Control: no-cache`.
//! - Served from an in-memory snapshot of the records ([`refresh`] after
//!   every change), never a k2.db read per request.
//! - **Check-ins** (Rosson 10-29): each dynamic check counts once for its
//!   day, channel, platform and version. An app that sends an anonymous
//!   install id (`?install=<id>` on its endpoint URL, 8–64 of
//!   `[A-Za-z0-9-]`) is also counted as one install per day; only a
//!   SHA-256 of (App, day, id) is kept, for two days. K2 never sees or keeps
//!   an address here (the router's answer hook has none). Counts reach
//!   k2.db at most every 30 s ([`flush`]).
//! - Only Apps K2 fronts (`host.clientIp` on) reach these answers: a raw
//!   splice never consults K2's answers (TA-10).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use k2_core::app_updates::{self as au, feed, store};
use serde_json::{json, Value as J};

pub const PREFIX: &str = "/_k2/updates/";
/// Pending count keys kept in memory between flushes (beyond: dropped).
const MAX_PENDING: usize = 5000;
const FLUSH_EVERY_SECS: i64 = 30;

fn snapshot() -> &'static RwLock<HashMap<String, Arc<feed::Feed>>> {
    static S: OnceLock<RwLock<HashMap<String, Arc<feed::Feed>>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Load every App's feed once (and fail pushes a restart interrupted).
pub(crate) fn ensure_loaded() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let apps = {
            let h = k2_core::db::shared();
            let conn = h.lock();
            match store::fail_interrupted(&conn, k2_core::deploy::now()) {
                Ok(failed) => {
                    for (app, version, _) in failed {
                        k2_core::log_debug!("[app-updates] {app} {version}: the daemon stopped mid-upload; the release is failed, no channel moved");
                    }
                }
                Err(e) => k2_core::log_debug!("[app-updates] WARN boot check: {e}"),
            }
            store::apps_with_feed(&conn).unwrap_or_default()
        };
        super::updates_routes::clean_all_staging_of_failed();
        for a in apps {
            refresh(&a);
        }
    });
}

/// Re-read one App's feed into the snapshot.
pub fn refresh(app_id: &str) {
    refresh_from(&k2_core::db::shared(), app_id);
}

/// [`refresh`] from a given DB handle (the upload thread carries the
/// request's handle).
pub fn refresh_from(db: &Arc<parking_lot::ReentrantMutex<rusqlite::Connection>>, app_id: &str) {
    let f = {
        let conn = db.lock();
        feed::load(&conn, app_id)
    };
    let mut s = snapshot().write().unwrap_or_else(|e| e.into_inner());
    match f {
        Ok(f) if !f.channels.is_empty() => {
            s.insert(app_id.to_string(), Arc::new(f));
        }
        Ok(_) => {
            s.remove(app_id);
        }
        Err(e) => k2_core::log_debug!("[app-updates] {app_id}: WARN reading the feed: {e}"),
    }
}

/// The App went away.
pub fn forget(app_id: &str) {
    snapshot().write().unwrap_or_else(|e| e.into_inner()).remove(app_id);
}

pub fn get(app_id: &str) -> Option<Arc<feed::Feed>> {
    ensure_loaded();
    snapshot().read().unwrap_or_else(|e| e.into_inner()).get(app_id).cloned()
}

/// A [`crate::client_ip::LocalAnswer`] for `app_id` while it has a live
/// channel (`None`: K2 answers nothing under `/_k2/updates/` for it).
pub fn local_for(app_id: &str) -> Option<crate::client_ip::LocalAnswer> {
    get(app_id)?;
    let id = app_id.to_string();
    Some(Arc::new(move |method: &str, path: &str, _host: Option<&str>| answer(&id, method, path)))
}

// ── responses ────────────────────────────────────────────────────────

fn resp(status: &str, body: Option<&J>, head_only: bool) -> Vec<u8> {
    let common = "Cache-Control: no-cache\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n";
    match body {
        None => format!("HTTP/1.1 {status}\r\n{common}\r\n").into_bytes(),
        Some(b) => {
            let text = b.to_string();
            let mut out = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{common}\r\n", text.len()).into_bytes();
            if !head_only {
                out.extend_from_slice(text.as_bytes());
            }
            out
        }
    }
}

fn not_found(head_only: bool, what: &str) -> Vec<u8> {
    resp("404 Not Found", Some(&json!({ "error": "not_found", "message": what })), head_only)
}

/// The whole response for `method path_and_query` on `app_id`, or `None`
/// (not an update path: the App answers).
pub fn answer(app_id: &str, method: &str, path_and_query: &str) -> Option<Vec<u8>> {
    let (path, query) = path_and_query.split_once('?').unwrap_or((path_and_query, ""));
    let rest = path.strip_prefix(PREFIX)?;
    let head_only = method.eq_ignore_ascii_case("HEAD");
    if !head_only && !method.eq_ignore_ascii_case("GET") {
        let b = json!({ "error": "method_not_allowed", "message": "GET or HEAD" });
        let mut r = resp("405 Method Not Allowed", Some(&b), false);
        // Add Allow before the blank line.
        let text = String::from_utf8_lossy(&r).replacen("Connection: close\r\n", "Allow: GET, HEAD\r\nConnection: close\r\n", 1);
        r = text.into_bytes();
        return Some(r);
    }
    let Some(f) = get(app_id) else { return Some(not_found(head_only, "this App has no update channel")) };
    let segs: Vec<&str> = rest.split('/').collect();
    match segs.as_slice() {
        [ch, "latest.json"] => match f.channels.get(*ch) {
            Some(c) => {
                count(app_id, ch, "-", "-", None);
                Some(resp("200 OK", Some(&feed::static_manifest(c.current())), head_only))
            }
            None => Some(not_found(head_only, "no such channel")),
        },
        [ch, target, arch, current] => match f.channels.get(*ch) {
            Some(c) => match feed::dynamic(c, target, arch, current) {
                feed::Dynamic::Update { body, .. } => {
                    count(app_id, ch, &format!("{target}-{arch}"), current, install_of(query));
                    Some(resp("200 OK", Some(&body), head_only))
                }
                feed::Dynamic::NoUpdate => {
                    count(app_id, ch, &format!("{target}-{arch}"), current, install_of(query));
                    Some(resp("204 No Content", None, head_only))
                }
                feed::Dynamic::Bad(why) => Some(resp("400 Bad Request", Some(&json!({ "error": "bad_request", "message": why })), head_only)),
            },
            None => Some(not_found(head_only, "no such channel")),
        },
        _ => Some(not_found(head_only, "use /_k2/updates/<channel>/latest.json or /_k2/updates/<channel>/<target>/<arch>/<current_version>")),
    }
}

/// The anonymous install id from the query (`install=`), if well formed.
fn install_of(query: &str) -> Option<String> {
    query.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == "install").map(|(_, v)| v.to_string()).filter(|v| {
        (8..=64).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

// ── check-ins ────────────────────────────────────────────────────────

type CountKey = (String, String, String, String, String); // app, day, channel, platform, version

#[derive(Default)]
struct Pending {
    checks: HashMap<CountKey, i64>,
    installs: Vec<(CountKey, String)>,
    last_flush: i64,
    flushing: bool,
}

fn pending() -> &'static Mutex<Pending> {
    static P: OnceLock<Mutex<Pending>> = OnceLock::new();
    P.get_or_init(Default::default)
}

fn count(app: &str, ch: &str, platform: &str, version: &str, install: Option<String>) {
    // The version is the caller's: keep only well-formed ones.
    let version = if version == "-" || au::parse_version(version.trim_start_matches('v')).is_ok() {
        version.trim_start_matches('v').to_string()
    } else {
        return;
    };
    let now = k2_core::deploy::now();
    let day = au::day_of(now);
    let key: CountKey = (app.into(), day.clone(), ch.into(), platform.into(), version);
    let spawn = {
        let mut p = pending().lock().unwrap_or_else(|e| e.into_inner());
        if p.checks.len() >= MAX_PENDING && !p.checks.contains_key(&key) {
            return;
        }
        *p.checks.entry(key.clone()).or_insert(0) += 1;
        if let Some(id) = install {
            if p.installs.len() < MAX_PENDING {
                p.installs.push((key, install_hash(app, &day, &id)));
            }
        }
        let due = now - p.last_flush >= FLUSH_EVERY_SECS && !p.flushing;
        if due {
            p.flushing = true;
        }
        due
    };
    if spawn {
        std::thread::spawn(|| {
            flush();
        });
    }
}

fn install_hash(app: &str, day: &str, id: &str) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(format!("{app}\n{day}\n{id}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Write pending counts to k2.db (and prune install hashes older than
/// yesterday). Routes call it before they read counts.
pub fn flush() {
    let (checks, installs) = {
        let mut p = pending().lock().unwrap_or_else(|e| e.into_inner());
        p.last_flush = k2_core::deploy::now();
        (std::mem::take(&mut p.checks), std::mem::take(&mut p.installs))
    };
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let mut new_installs: HashMap<CountKey, i64> = HashMap::new();
        for (key, hash) in installs {
            match store::install_seen_first(&conn, &key.0, &key.1, &hash) {
                Ok(true) => *new_installs.entry(key).or_insert(0) += 1,
                Ok(false) => {}
                Err(e) => k2_core::log_debug!("[app-updates] WARN install count: {e}"),
            }
        }
        for (k, n) in &checks {
            let i = new_installs.remove(k).unwrap_or(0);
            if let Err(e) = store::add_checkins(&conn, &k.0, &k.1, &k.2, &k.3, &k.4, *n, i) {
                k2_core::log_debug!("[app-updates] WARN check-in count: {e}");
            }
        }
        let yesterday = au::day_of(k2_core::deploy::now() - 86_400);
        if let Err(e) = store::prune_install_days(&conn, &yesterday) {
            k2_core::log_debug!("[app-updates] WARN pruning install hashes: {e}");
        }
    }
    pending().lock().unwrap_or_else(|e| e.into_inner()).flushing = false;
}

#[cfg(test)]
pub(crate) fn reset_for_test() {
    snapshot().write().unwrap_or_else(|e| e.into_inner()).clear();
    let mut p = pending().lock().unwrap_or_else(|e| e.into_inner());
    *p = Pending::default();
    // Never start a background flush in a test: the test flushes itself.
    p.last_flush = i64::MAX / 2;
}
