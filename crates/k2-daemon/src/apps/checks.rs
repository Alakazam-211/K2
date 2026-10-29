//! `host.checks`: an App's own probes (prd-tauri-core-0467-v1 TC-16).
//!
//! A manifest lists up to [`MAX_CHECKS`] of
//! `{name, path, expect, as?, header?: {name, contains}, bodyContains?}`.
//! The daemon runs them against the live instance ([`TIMEOUT`] each, in
//! parallel) for `k2 app status`, and once after every switch to live
//! ([`after_switch`]): those results are recorded on the deployment (the
//! deploy audit's `checks` event) and a failed check wakes the App's agent
//! once for that deployment (the alerts rail).
//!
//! - **Checks never gate a deploy**: the health gate does.
//! - **No App output reaches an agent raw** (vs-live TA-20): a result says
//!   the status it got and whether the header and the body matched, never
//!   a header value or a body.
//! - `as: testGuest` probes signed in as the App's test guest
//!   (`deploy::health::probe_headers`, the health gate's own hook).
//!
//! The manifest's typed `host.checks` (Lane A, `deploy::manifest`) and this
//! parse take the same shape; this module reads the Seed's stored manifest
//! JSON, so an App deployed by an older daemon simply has no checks.

use std::io::Read;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::deploy::health::{self, ProbeAs};

/// At most this many checks per App.
pub const MAX_CHECKS: usize = 10;
/// Each check's time limit.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// `bodyContains` looks at most this far into the body.
pub const BODY_WINDOW: usize = 64 * 1024;
/// `bodyContains` is at most this long.
pub const BODY_CONTAINS_MAX: usize = 200;

/// One manifest check.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Check {
    pub name: String,
    pub path: String,
    pub expect: u16,
    #[serde(rename = "as", default)]
    pub as_: Option<String>,
    #[serde(default)]
    pub header: Option<HeaderCheck>,
    #[serde(default)]
    pub body_contains: Option<String>,
}

/// A response header that must hold `contains` (case-insensitive name).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderCheck {
    pub name: String,
    pub contains: String,
}

impl Check {
    fn probe_as(&self) -> ProbeAs {
        match self.as_.as_deref() {
            Some("testGuest") => ProbeAs::TestGuest,
            _ => ProbeAs::Anonymous,
        }
    }
}

/// The checks in a manifest (`host.checks`; none when absent).
pub fn parse(manifest: &J) -> Result<Vec<Check>, String> {
    let Some(raw) = manifest.pointer("/host/checks") else { return Ok(Vec::new()) };
    let checks: Vec<Check> = serde_json::from_value(raw.clone()).map_err(|e| format!("host.checks: {e}"))?;
    if checks.len() > MAX_CHECKS {
        return Err(format!("host.checks: at most {MAX_CHECKS}, not {}", checks.len()));
    }
    let mut names = std::collections::BTreeSet::new();
    for c in &checks {
        let name_ok = !c.name.is_empty() && c.name.len() <= 40 && c.name.chars().all(|ch| ch.is_ascii_alphanumeric() || "-_. ".contains(ch));
        if !name_ok {
            return Err(format!("host.checks: name {:?} is 1–40 letters, digits, space, '-', '_' or '.'", c.name));
        }
        if !names.insert(c.name.as_str()) {
            return Err(format!("host.checks: two checks are named {:?}", c.name));
        }
        if !c.path.starts_with('/') || c.path.len() > 512 || c.path.chars().any(|ch| ch.is_control() || ch == ' ') {
            return Err(format!("host.checks {}: path is an absolute path (starts with /, no spaces)", c.name));
        }
        if !(100..=599).contains(&c.expect) {
            return Err(format!("host.checks {}: expect is one HTTP status (100–599)", c.name));
        }
        if let Some(a) = c.as_.as_deref() {
            if a != "anonymous" && a != "testGuest" {
                return Err(format!("host.checks {}: as is anonymous or testGuest, not {a:?}", c.name));
            }
        }
        if let Some(h) = &c.header {
            if h.name.is_empty() || !h.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') || h.contains.is_empty() || h.contains.len() > 200 {
                return Err(format!("host.checks {}: header is {{name, contains}} (a header name; contains 1–200 characters)", c.name));
            }
        }
        if let Some(b) = &c.body_contains {
            if b.is_empty() || b.chars().count() > BODY_CONTAINS_MAX {
                return Err(format!("host.checks {}: bodyContains is 1–{BODY_CONTAINS_MAX} characters", c.name));
            }
        }
    }
    Ok(checks)
}

/// One check's result. Never a header value or a body (TA-20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub name: String,
    pub path: String,
    pub expect: u16,
    pub as_test_guest: bool,
    /// The status it got (`None`: nothing answered).
    pub status: Option<u16>,
    /// `Some(matched)` when the check has a `header`.
    pub header: Option<bool>,
    /// `Some(matched)` when the check has `bodyContains`.
    pub body: Option<bool>,
    pub ok: bool,
    /// Why it failed: `status`, `header`, `body`, `no_answer`,
    /// `test_guest_unavailable`.
    pub why: Option<&'static str>,
    pub ms: u64,
}

impl Outcome {
    pub fn to_json(&self) -> J {
        json!({
            "name": self.name,
            "path": self.path,
            "expect": self.expect,
            "as": if self.as_test_guest { "testGuest" } else { "anonymous" },
            "status": self.status,
            "header": self.header,
            "body": self.body,
            "ok": self.ok,
            "why": self.why,
            "ms": self.ms,
        })
    }
}

/// What one GET answered (the fetcher's part, so tests need no network).
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// At most [`BODY_WINDOW`] bytes.
    pub body: Vec<u8>,
}

/// Judge one answer against one check.
pub fn judge(c: &Check, a: &Answer) -> (bool, Option<bool>, Option<bool>, Option<&'static str>) {
    let header = c.header.as_ref().map(|h| {
        a.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case(&h.name) && v.contains(&h.contains))
    });
    let body = c.body_contains.as_ref().map(|needle| {
        let window = &a.body[..a.body.len().min(BODY_WINDOW)];
        String::from_utf8_lossy(window).contains(needle.as_str())
    });
    let why = if a.status != c.expect {
        Some("status")
    } else if header == Some(false) {
        Some("header")
    } else if body == Some(false) {
        Some("body")
    } else {
        None
    };
    (why.is_none(), header, body, why)
}

/// GET `base + path` with `headers`, no redirects, [`TIMEOUT`].
fn fetch(base: &str, path: &str, headers: &[(String, String)]) -> Result<Answer, String> {
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| format!("client: {e}"))?;
    let mut req = client.get(format!("{base}{path}")).header("User-Agent", "k2-check/1");
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let r = req.send().map_err(|e| e.to_string())?;
    let status = r.status().as_u16();
    let headers = r.headers().iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).to_string())).collect();
    let mut body = Vec::new();
    r.take(BODY_WINDOW as u64).read_to_end(&mut body).map_err(|e| format!("body: {e}"))?;
    Ok(Answer { status, headers, body })
}

/// Run `checks` against `base` (`http://127.0.0.1:<port>`, or the App's
/// public `https://<host>` for `--public`), in parallel.
pub fn run(app_id: &str, base: &str, checks: &[Check]) -> Vec<Outcome> {
    std::thread::scope(|s| {
        let handles: Vec<_> = checks.iter().map(|c| s.spawn(move || run_one(app_id, base, c))).collect();
        handles
            .into_iter()
            .zip(checks)
            .map(|(h, c)| {
                h.join().unwrap_or_else(|_| Outcome {
                    name: c.name.clone(),
                    path: c.path.clone(),
                    expect: c.expect,
                    as_test_guest: c.probe_as() == ProbeAs::TestGuest,
                    status: None,
                    header: None,
                    body: None,
                    ok: false,
                    why: Some("no_answer"),
                    ms: 0,
                })
            })
            .collect()
    })
}

fn run_one(app_id: &str, base: &str, c: &Check) -> Outcome {
    let t0 = Instant::now();
    let as_tg = c.probe_as() == ProbeAs::TestGuest;
    let mut out = Outcome {
        name: c.name.clone(),
        path: c.path.clone(),
        expect: c.expect,
        as_test_guest: as_tg,
        status: None,
        header: None,
        body: None,
        ok: false,
        why: None,
        ms: 0,
    };
    let headers = match health::probe_headers(app_id, c.probe_as()) {
        Ok(h) => h,
        Err(_) => {
            out.why = Some("test_guest_unavailable");
            return out;
        }
    };
    match fetch(base, &c.path, &headers) {
        Ok(a) => {
            let (ok, header, body, why) = judge(c, &a);
            out.status = Some(a.status);
            out.header = header;
            out.body = body;
            out.ok = ok;
            out.why = why;
        }
        Err(_) => out.why = Some("no_answer"),
    }
    out.ms = t0.elapsed().as_millis() as u64;
    out
}

/// The checks of the App's live Seed (none: no live Seed, or no checks).
pub fn of_live_seed(app: &k2_core::deploy::store::AppRow) -> Result<Vec<Check>, String> {
    match app.live_seed.as_deref() {
        Some(hash) => of_seed(hash),
        None => Ok(Vec::new()),
    }
}

/// The checks in one Seed's manifest.
pub fn of_seed(hash: &str) -> Result<Vec<Check>, String> {
    let seed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::store::get_seed(&conn, hash).map_err(|e| e.to_string())?
    };
    match seed {
        Some(s) => parse(&s.manifest),
        None => Ok(Vec::new()),
    }
}

/// The deploy audit event a post-switch run records.
pub const AUDIT_EVENT: &str = "checks";

/// After a switch to live: run the live Seed's checks once on `port`,
/// record them on the deployment and, when one failed, tell the App's
/// agent (once: this runs once per switch). On its own thread.
pub fn after_switch(app: &k2_core::deploy::store::AppRow, seed: &str, deployment: &str, port: u16) {
    let checks = match of_seed(seed) {
        Ok(c) if !c.is_empty() => c,
        Ok(_) => return,
        Err(e) => {
            k2_core::log_debug!("[apps] {} checks: {e}", app.app_id);
            return;
        }
    };
    let (app_id, ws, dep) = (app.app_id.clone(), app.owner_workspace.clone(), deployment.to_string());
    let spawned = std::thread::Builder::new().name("app-checks".into()).spawn(move || {
        let results = run(&app_id, &format!("http://127.0.0.1:{port}"), &checks);
        record(&app_id, &dep, &results);
        let failed: Vec<&Outcome> = results.iter().filter(|r| !r.ok).collect();
        if !failed.is_empty() {
            super::alerts::notify(&ws, &wake_text(&app_id, &dep, &failed));
        }
    });
    if let Err(e) = spawned {
        k2_core::log_debug!("[apps] {} checks not run: {e}", app.app_id);
    }
}

/// Record a run on the deployment (the deploy audit).
pub fn record(app_id: &str, deployment: &str, results: &[Outcome]) {
    let rows: Vec<J> = results.iter().map(Outcome::to_json).collect();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let payload = json!({ "deployment": deployment, "results": rows });
    if let Err(e) = k2_core::deploy::audit::append(&conn, k2_core::deploy::audit::SUBJECT_APP, app_id, AUDIT_EVENT, "system", None, &payload) {
        k2_core::log_debug!("[apps] {app_id} checks audit: {e}");
    }
}

/// The last recorded run for `deployment` (`k2 app status --no-checks`).
pub fn recorded(app_id: &str, deployment: &str) -> Option<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows = k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, app_id, 50).ok()?;
    rows.into_iter()
        .find(|r| r.event == AUDIT_EVENT && r.payload.get("deployment").and_then(J::as_str) == Some(deployment))
        .map(|r| json!({ "at": r.at, "results": r.payload.get("results").cloned().unwrap_or(J::Null) }))
}

/// The wake text for failed checks (names, statuses and what didn't match;
/// never App output).
pub fn wake_text(app_id: &str, deployment: &str, failed: &[&Outcome]) -> String {
    let mut s = format!("[k2-apps] {app_id}: {} of its host.checks failed after deployment {deployment} went live:\n", failed.len());
    for f in failed {
        let got = f.status.map(|c| c.to_string()).unwrap_or_else(|| "no answer".into());
        s.push_str(&format!("  {} GET {}: expected {}, got {got}", f.name, f.path, f.expect));
        match f.why {
            Some("header") => s.push_str(" (the header didn't match)"),
            Some("body") => s.push_str(" (the body didn't match)"),
            Some("test_guest_unavailable") => s.push_str(" (no test guest to sign in as)"),
            _ => {}
        }
        s.push('\n');
    }
    s.push_str(&format!("The App stays live (checks never gate a deploy). Look: k2 app status {app_id}"));
    s
}

#[cfg(test)]
#[path = "checks_tests.rs"]
mod tests;
