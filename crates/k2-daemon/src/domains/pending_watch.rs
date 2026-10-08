//! The box keeps its custom-domain zone status fresh by itself
//! (prd-dns-pending-and-cutover-safety-v1 P1/P2, A5; DN12, DN14).
//!
//! Before this, a `pending_ns` row only turned `active` when someone ran
//! `k2 domain refresh`. k2.dev's sweep flips a zone within minutes of the
//! registrar delegation, but the box kept its cached `pending_ns` and
//! refused every `k2 dns` write locally (quillify.info, 2026-10-07).
//!
//! - **P1, the loop** ([`spawn`]): one detached thread for the daemon's
//!   life. While any bound row is `pending_ns` it makes ONE read-only
//!   `GET /api/dns/zones` per tick (no zone create, no audit row, no rate
//!   budget on k2.dev) and applies the answer to every bound row
//!   ([`apply_zone_statuses`]). The schedule ([`next_check_delay`]) follows
//!   the youngest pending row's age: 1 min for the first hour, 5 min until
//!   48 h, 30 min until 14 days, 6 h until 60 days, then daily. Pending
//!   zones may wait forever on k2.dev (its sweep is log-only), so a very
//!   old one costs four calls a day at most. Errors double the wait (cap
//!   1 h, never shorter than the age step). With nothing pending the
//!   thread parks; attach and refresh wake it ([`wake`]). Under air-gap or
//!   with no tunnel token it makes no call.
//! - **P2, before refusing a write** ([`recheck_now`]): a record write on a
//!   row that still reads `pending_ns` asks k2.dev once (reusing the zones
//!   list a by-domain lookup already fetched), stores the answer, then
//!   decides.
//!
//! A flip writes the row, logs it, emits `domains_changed`, and wakes the
//! cert renewer so DNS-01 names skipped as pending get issued.
//!
//! Poll state (last check, failures, zones missing from k2.dev's list) is
//! in memory only (DN14): no migration.

use std::collections::BTreeSet;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use k2_core::domains::{
    get_binding_by_zone_id, list_bindings, set_zone_status, DomainBinding, ZONE_STATUS_ACTIVE,
    ZONE_STATUS_PENDING_NS, ZONE_STATUS_SUSPENDED, ZONE_STATUS_WINDDOWN,
};
use k2_core::log_debug;

use crate::dns::proxy::{proxy_request, DnsHttpResponse};

/// First check after boot (off the boot path).
pub const FIRST_CHECK_AFTER_SECS: u64 = 60;
/// `(binding age below, wait)` steps. Older than the last step waits
/// [`OLD_PENDING_DELAY_SECS`].
pub const SCHEDULE: &[(i64, u64)] = &[
    (3_600, 60),
    (48 * 3_600, 5 * 60),
    (14 * 86_400, 30 * 60),
    (60 * 86_400, 6 * 3_600),
];
/// Wait for a zone pending longer than 60 days.
pub const OLD_PENDING_DELAY_SECS: u64 = 86_400;
/// Error backoff doubles the wait up to this (never below the age step).
pub const ERROR_BACKOFF_CAP_SECS: u64 = 3_600;
/// ± this fraction of jitter on every wait, so a fleet doesn't tick together.
pub const JITTER_FRACTION: f64 = 0.1;
/// Under air-gap / with no tunnel token the loop re-reads local state at
/// most this often (no network), so turning air-gap off or pairing is
/// noticed without a restart.
pub const QUIET_RECHECK_SECS: u64 = 10 * 60;

const ZONES_PATH: &str = "/api/dns/zones";
const AGENT: &str = "k2-dns-pending-watch";

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── Pure parts ──────────────────────────────────────────────────────────

/// One zone from k2.dev's agent `GET /api/dns/zones` (raw status).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteZone {
    pub id: String,
    pub domain: String,
    /// Raw k2.dev status: `pending|active|delegated|winddown|suspended|error`.
    pub status: String,
}

/// Parse the zones list body. Fails on anything but a JSON object with a
/// `zones` array (an HTML catch-all page is an error, never "no zones").
pub fn parse_zones_list(body: &str) -> Result<Vec<RemoteZone>, String> {
    let v: serde_json::Value = serde_json::from_str(body.trim())
        .map_err(|e| format!("GET {ZONES_PATH}: reply is not JSON ({e})"))?;
    if !v.get("zones").is_some_and(|z| z.is_array()) {
        return Err(format!("GET {ZONES_PATH}: reply has no `zones` array"));
    }
    Ok(zones_from_value(&v))
}

/// The zones in an already-parsed list body (`zones` absent → empty).
pub fn zones_from_value(v: &serde_json::Value) -> Vec<RemoteZone> {
    let Some(arr) = v.get("zones").and_then(|z| z.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|z| {
            let id = z.get("id")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            let text = |k: &str| {
                z.get(k)
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches('.')
                    .to_ascii_lowercase()
            };
            Some(RemoteZone {
                id: id.to_string(),
                domain: text("domain"),
                status: text("status"),
            })
        })
        .collect()
}

/// The zones list out of an HTTP reply (200 only).
pub fn zones_from_response(resp: &DnsHttpResponse) -> Result<Vec<RemoteZone>, String> {
    if resp.status != 200 {
        return Err(format!("GET {ZONES_PATH} answered HTTP {}", resp.status));
    }
    parse_zones_list(&resp.body)
}

/// k2.dev's raw zone status → the box's `(status, dns_write)`.
///
/// `pending|error` → `pending_ns` (no writes until P3/P4); `active|
/// delegated` → `active`, writable; `winddown|suspended` → that word,
/// read-only. Unknown words → `None` (the row is left alone).
pub fn map_remote_status(raw: &str) -> Option<(&'static str, bool)> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "pending" | "pending_ns" | "error" => Some((ZONE_STATUS_PENDING_NS, false)),
        "active" | "delegated" => Some((ZONE_STATUS_ACTIVE, true)),
        "winddown" => Some((ZONE_STATUS_WINDDOWN, false)),
        "suspended" => Some((ZONE_STATUS_SUSPENDED, false)),
        _ => None,
    }
}

/// One row whose stored status differs from k2.dev's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusChange {
    pub apex: String,
    pub zone_id: String,
    pub from: Option<String>,
    pub to: String,
    pub dns_write: bool,
}

/// What a zones list says about the bound rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub changes: Vec<StatusChange>,
    /// Apexes whose zone id is not in k2.dev's list (row kept, shown).
    pub missing: Vec<String>,
    /// `(apex, raw status)` k2.dev sent a word this box doesn't know.
    pub unknown: Vec<(String, String)>,
}

/// Compare every bound row with k2.dev's list. BYO rows (no zone id) are
/// never touched. A zone missing from the list, or with an unknown status,
/// leaves its row alone.
pub fn apply_zone_statuses(rows: &[DomainBinding], zones: &[RemoteZone]) -> Applied {
    let mut out = Applied::default();
    for row in rows {
        let Some(zone_id) = row.zone_id.as_deref().map(str::trim).filter(|z| !z.is_empty()) else {
            continue;
        };
        let Some(zone) = zones.iter().find(|z| z.id == zone_id) else {
            out.missing.push(row.apex.clone());
            continue;
        };
        let Some((to, dns_write)) = map_remote_status(&zone.status) else {
            out.unknown.push((row.apex.clone(), zone.status.clone()));
            continue;
        };
        if row.status.as_deref() != Some(to) || row.dns_write != dns_write {
            out.changes.push(StatusChange {
                apex: row.apex.clone(),
                zone_id: zone_id.to_string(),
                from: row.status.clone(),
                to: to.to_string(),
                dns_write,
            });
        }
    }
    out
}

/// Age of the youngest bound `pending_ns` row, `None` when nothing is
/// pending (the loop parks).
pub fn youngest_pending_age(rows: &[DomainBinding], now: i64) -> Option<i64> {
    rows.iter()
        .filter(|b| b.is_pending_ns() && b.zone_id.as_deref().is_some_and(|z| !z.trim().is_empty()))
        .map(|b| (now - b.created_at).max(0))
        .min()
}

/// The age step for a pending binding this old.
pub fn base_delay_secs(age_secs: i64) -> u64 {
    let age = age_secs.max(0);
    SCHEDULE
        .iter()
        .find(|(below, _)| age < *below)
        .map(|(_, d)| *d)
        .unwrap_or(OLD_PENDING_DELAY_SECS)
}

/// Wait before the next check: the age step, doubled per consecutive
/// failure up to [`ERROR_BACKOFF_CAP_SECS`] (never below the age step).
pub fn next_check_delay(age_secs: i64, failures: u32) -> Duration {
    let base = base_delay_secs(age_secs);
    if failures == 0 {
        return Duration::from_secs(base);
    }
    let backoff = base
        .saturating_mul(1u64 << failures.min(20))
        .min(ERROR_BACKOFF_CAP_SECS);
    Duration::from_secs(base.max(backoff))
}

/// `d` ± [`JITTER_FRACTION`]; `unit` in `[0, 1)` (0.5 = no change).
pub fn jittered(d: Duration, unit: f64) -> Duration {
    let u = if unit.is_finite() { unit.clamp(0.0, 1.0) } else { 0.5 };
    let f = 1.0 + JITTER_FRACTION * (2.0 * u - 1.0);
    Duration::from_secs_f64(d.as_secs_f64() * f)
}

fn random_unit() -> f64 {
    let mut b = [0u8; 4];
    if getrandom::getrandom(&mut b).is_err() {
        return 0.5;
    }
    (u32::from_le_bytes(b) as f64) / (u32::MAX as f64 + 1.0)
}

// ── In-memory state (DN14) ──────────────────────────────────────────────

#[derive(Debug, Default)]
struct WatchState {
    /// Last time a zones list was read and applied (loop, P2, refresh).
    last_check: Option<i64>,
    /// Last loop attempt of any kind (schedules the next one).
    last_attempt: Option<i64>,
    /// Consecutive failed loop fetches.
    failures: u32,
    last_error: Option<String>,
    /// Apexes whose zone id was not in k2.dev's last list.
    missing: BTreeSet<String>,
}

fn state() -> &'static Mutex<WatchState> {
    static S: OnceLock<Mutex<WatchState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(WatchState::default()))
}

fn with_state<R>(f: impl FnOnce(&mut WatchState) -> R) -> R {
    let mut g = state().lock().unwrap_or_else(|p| p.into_inner());
    f(&mut g)
}

/// True when the apex's zone was missing from k2.dev's last zones list
/// (DN12d: Settings and `k2 domain list` show it).
pub fn zone_missing(apex: &str) -> bool {
    with_state(|s| s.missing.contains(apex))
}

/// A successful bind proved the zone exists: stop showing it as missing.
pub fn clear_missing(apex: &str) {
    with_state(|s| {
        s.missing.remove(apex);
    });
}

/// Unix seconds of the last zones-list check, if any since boot.
pub fn last_check() -> Option<i64> {
    with_state(|s| s.last_check)
}

/// Last loop error since the last good check (Settings / `k2 domain list`).
pub fn last_error() -> Option<String> {
    with_state(|s| s.last_error.clone())
}

// ── Store + announce ────────────────────────────────────────────────────

/// What one check stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckReport {
    pub changes: Vec<StatusChange>,
    pub missing: Vec<String>,
}

fn read_rows() -> Result<Vec<DomainBinding>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    list_bindings(&conn).map_err(|e| format!("read domain_bindings: {e}"))
}

/// Apply a fresh zones list to every bound row, store what changed, and
/// announce it. Never holds the DB lock across a network call (the list
/// is already in hand).
pub fn store_zones(zones: &[RemoteZone], now: i64) -> Result<CheckReport, String> {
    let rows = read_rows()?;
    let applied = apply_zone_statuses(&rows, zones);
    let mut stored = Vec::new();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for c in &applied.changes {
            match set_zone_status(&conn, &c.apex, &c.zone_id, &c.to, c.dns_write) {
                Ok(true) => stored.push(c.clone()),
                Ok(false) => {}
                Err(e) => return Err(format!("store {} status: {e}", c.apex)),
            }
        }
    }
    for (apex, raw) in &applied.unknown {
        log_debug!("[domains/pending-watch] {apex}: k2.dev status '{raw}' is not known here — row left as is");
    }
    let new_missing: BTreeSet<String> = applied.missing.iter().cloned().collect();
    let (appeared, found_again) = with_state(|s| {
        s.last_check = Some(now);
        s.failures = 0;
        s.last_error = None;
        let appeared: Vec<String> = new_missing.difference(&s.missing).cloned().collect();
        let found_again: Vec<String> = s.missing.difference(&new_missing).cloned().collect();
        s.missing = new_missing;
        (appeared, found_again)
    });
    announce(&stored, &appeared, &found_again);
    Ok(CheckReport {
        changes: stored,
        missing: applied.missing,
    })
}

fn announce(changes: &[StatusChange], appeared: &[String], found_again: &[String]) {
    for c in changes {
        log_debug!(
            "[domains/pending-watch] {} (zone {}): {} → {} (dns_write={})",
            c.apex,
            c.zone_id,
            c.from.as_deref().unwrap_or("none"),
            c.to,
            c.dns_write
        );
        crate::session_events::emit_domains_changed("status_changed", Some(&c.apex));
    }
    if changes.iter().any(|c| c.to == ZONE_STATUS_ACTIVE) {
        // DNS-01 names skipped while pending can be issued now.
        crate::domains::renew::wake();
    }
    for apex in appeared {
        log_debug!("[domains/pending-watch] {apex}: its zone is not in k2.dev's list for this server — row kept");
        crate::session_events::emit_domains_changed("zone_missing", Some(apex));
    }
    for apex in found_again {
        crate::session_events::emit_domains_changed("status_changed", Some(apex));
    }
}

/// Read the zones list from k2.dev (one read-only GET).
pub fn fetch_zones_live() -> Result<Vec<RemoteZone>, String> {
    let resp = proxy_request("GET", ZONES_PATH, Some(AGENT), None)?;
    zones_from_response(&resp)
}

/// P2: re-check once with k2.dev and store the answer. `prefetched` is a
/// zones list the caller already holds (zero extra calls); otherwise one
/// `GET /api/dns/zones`.
pub fn recheck_now(prefetched: Option<&[RemoteZone]>) -> Result<CheckReport, String> {
    match prefetched {
        Some(z) => store_zones(z, now_unix()),
        None => {
            let zones = fetch_zones_live()?;
            store_zones(&zones, now_unix())
        }
    }
}

/// `k2 dns verify`: store the status k2.dev's verify answered on the row
/// bound to `zone_id`. `Ok(None)` = nothing changed (or unknown word).
pub fn store_verify_status(zone_id: &str, raw_status: &str) -> Result<Option<StatusChange>, String> {
    let Some((to, dns_write)) = map_remote_status(raw_status) else {
        return Ok(None);
    };
    let change = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(row) = get_binding_by_zone_id(&conn, zone_id).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let c = StatusChange {
            apex: row.apex.clone(),
            zone_id: zone_id.to_string(),
            from: row.status.clone(),
            to: to.to_string(),
            dns_write,
        };
        match set_zone_status(&conn, &row.apex, zone_id, to, dns_write) {
            Ok(true) => Some(c),
            Ok(false) => None,
            Err(e) => return Err(e.to_string()),
        }
    };
    if let Some(c) = &change {
        announce(std::slice::from_ref(c), &[], &[]);
    }
    Ok(change)
}

// ── The loop ────────────────────────────────────────────────────────────

/// Everything one tick touches besides `domain_bindings`.
pub trait WatchDeps {
    fn now(&self) -> i64;
    fn airgap(&self) -> bool;
    fn has_token(&self) -> bool;
    fn fetch(&mut self) -> Result<DnsHttpResponse, String>;
}

/// What one tick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tick {
    /// No bound row is `pending_ns`: no call.
    NothingPending,
    /// Air-gap is on: no call.
    AirGap,
    /// No tunnel token (unpaired): no call.
    NoTunnel,
    Checked(CheckReport),
    Failed(String),
}

/// One pass: at most one `GET /api/dns/zones`, only while something is
/// pending, never under air-gap or with no tunnel token.
pub fn tick(deps: &mut dyn WatchDeps) -> Tick {
    let now = deps.now();
    let rows = match read_rows() {
        Ok(r) => r,
        Err(e) => return Tick::Failed(e),
    };
    if youngest_pending_age(&rows, now).is_none() {
        return Tick::NothingPending;
    }
    with_state(|s| s.last_attempt = Some(now));
    if deps.airgap() {
        return Tick::AirGap;
    }
    if !deps.has_token() {
        return Tick::NoTunnel;
    }
    let zones = match deps.fetch().and_then(|r| zones_from_response(&r)) {
        Ok(z) => z,
        Err(e) => {
            with_state(|s| {
                s.failures = s.failures.saturating_add(1);
                s.last_error = Some(e.clone());
            });
            return Tick::Failed(e);
        }
    };
    match store_zones(&zones, now) {
        Ok(r) => Tick::Checked(r),
        Err(e) => {
            with_state(|s| s.last_error = Some(e.clone()));
            Tick::Failed(e)
        }
    }
}

struct LiveWatch;

impl WatchDeps for LiveWatch {
    fn now(&self) -> i64 {
        now_unix()
    }
    fn airgap(&self) -> bool {
        k2_core::airgap::enabled()
    }
    fn has_token(&self) -> bool {
        crate::dns::proxy::tunnel_bearer_token().is_ok()
    }
    fn fetch(&mut self) -> Result<DnsHttpResponse, String> {
        proxy_request("GET", ZONES_PATH, Some(AGENT), None)
    }
}

fn wake_signal() -> &'static (Mutex<bool>, Condvar) {
    static W: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();
    W.get_or_init(|| (Mutex::new(false), Condvar::new()))
}

/// Make the loop re-read the bindings now (attach, refresh): a new pending
/// row starts the 1-minute step instead of waiting out a parked loop.
pub fn wake() {
    let (m, cv) = wake_signal();
    *m.lock().unwrap_or_else(|p| p.into_inner()) = true;
    cv.notify_all();
}

/// Sleep up to `d` (forever with `None`); return early on [`wake`].
fn wait_for_wake(d: Option<Duration>) {
    let (m, cv) = wake_signal();
    let mut flag = m.lock().unwrap_or_else(|p| p.into_inner());
    let deadline = d.map(|d| std::time::Instant::now() + d);
    while !*flag {
        match deadline {
            None => {
                flag = cv.wait(flag).unwrap_or_else(|p| p.into_inner());
            }
            Some(dl) => {
                let now = std::time::Instant::now();
                if now >= dl {
                    return;
                }
                flag = cv
                    .wait_timeout(flag, dl - now)
                    .map(|(g, _)| g)
                    .unwrap_or_else(|p| p.into_inner().0);
            }
        }
    }
    *flag = false;
}

/// Start the pending-zone re-check loop (one detached thread for the life
/// of the daemon; a panic is contained per tick). No-op in tests.
pub fn spawn() {
    if cfg!(test) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("k2-dns-pending-watch".into())
        .spawn(|| {
            wait_for_wake(Some(Duration::from_secs(FIRST_CHECK_AFTER_SECS)));
            run_loop();
        });
}

fn run_loop() -> ! {
    let mut jitter = random_unit();
    // Log a quiet state once, not every tick.
    let mut quiet: Option<&'static str> = None;
    loop {
        let now = now_unix();
        let rows = match read_rows() {
            Ok(r) => r,
            Err(e) => {
                log_debug!("[domains/pending-watch] {e} — retrying in {QUIET_RECHECK_SECS}s");
                wait_for_wake(Some(Duration::from_secs(QUIET_RECHECK_SECS)));
                continue;
            }
        };
        let Some(age) = youngest_pending_age(&rows, now) else {
            if quiet != Some("parked") {
                log_debug!("[domains/pending-watch] no pending domains — parked until attach/refresh");
                quiet = Some("parked");
            }
            wait_for_wake(None);
            continue;
        };
        let (last_attempt, failures) = with_state(|s| (s.last_attempt, s.failures));
        let mut delay = jittered(next_check_delay(age, failures), jitter);
        if matches!(quiet, Some("airgap") | Some("no-tunnel")) {
            delay = delay.min(Duration::from_secs(QUIET_RECHECK_SECS));
        }
        if let Some(t) = last_attempt {
            let due = t.saturating_add(delay.as_secs() as i64);
            if now < due {
                wait_for_wake(Some(Duration::from_secs((due - now) as u64)));
                continue;
            }
        }
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tick(&mut LiveWatch)));
        jitter = random_unit();
        match r {
            Err(_) => {
                log_debug!("[domains/pending-watch] tick panicked — retrying next tick");
                with_state(|s| {
                    s.last_attempt = Some(now);
                    s.failures = s.failures.saturating_add(1);
                });
                quiet = None;
            }
            Ok(Tick::AirGap) => {
                if quiet != Some("airgap") {
                    log_debug!("[domains/pending-watch] air-gap is on — not checking k2.dev");
                    quiet = Some("airgap");
                }
            }
            Ok(Tick::NoTunnel) => {
                if quiet != Some("no-tunnel") {
                    log_debug!("[domains/pending-watch] no K2 Connect tunnel token — not checking k2.dev");
                    quiet = Some("no-tunnel");
                }
            }
            Ok(Tick::Failed(e)) => {
                log_debug!("[domains/pending-watch] check failed: {e}");
                quiet = None;
            }
            Ok(Tick::Checked(_)) | Ok(Tick::NothingPending) => {
                quiet = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::domains::{get_binding, remove_binding, upsert_binding, upsert_binding_zone, BoundZone};

    const MIN: i64 = 60;
    const HOUR: i64 = 3_600;
    const DAY: i64 = 86_400;

    fn row(apex: &str, zone: Option<&str>, status: Option<&str>, dns_write: bool, created_at: i64) -> DomainBinding {
        DomainBinding {
            apex: apex.into(),
            zone_id: zone.map(str::to_string),
            dns_write,
            created_at,
            status: status.map(str::to_string),
            nameservers: vec![],
            auto_created: false,
        }
    }

    fn zone(id: &str, status: &str) -> RemoteZone {
        RemoteZone {
            id: id.into(),
            domain: format!("{id}.example"),
            status: status.into(),
        }
    }

    #[test]
    fn map_remote_status_table() {
        let cases: &[(&str, Option<(&str, bool)>)] = &[
            ("pending", Some(("pending_ns", false))),
            ("pending_ns", Some(("pending_ns", false))),
            ("error", Some(("pending_ns", false))),
            ("active", Some(("active", true))),
            ("ACTIVE", Some(("active", true))),
            ("delegated", Some(("active", true))),
            ("winddown", Some(("winddown", false))),
            ("suspended", Some(("suspended", false))),
            ("", None),
            ("deleted", None),
        ];
        for (raw, want) in cases {
            assert_eq!(map_remote_status(raw), *want, "{raw:?}");
        }
    }

    #[test]
    fn apply_zone_statuses_table() {
        let rows = vec![
            row("pend-to-active.example", Some("z1"), Some("pending_ns"), false, 0),
            row("pend-stays.example", Some("z2"), Some("pending_ns"), false, 0),
            row("error-is-pending.example", Some("z3"), Some("pending_ns"), false, 0),
            row("delegated.example", Some("z4"), Some("pending_ns"), false, 0),
            row("winddown.example", Some("z5"), Some("active"), true, 0),
            row("suspended.example", Some("z6"), Some("active"), true, 0),
            row("byo.example", None, None, false, 0),
            row("missing.example", Some("z-gone"), Some("pending_ns"), false, 0),
            row("odd.example", Some("z7"), Some("pending_ns"), false, 0),
            row("legacy-active.example", Some("z8"), None, true, 0),
        ];
        let zones = vec![
            zone("z1", "active"),
            zone("z2", "pending"),
            zone("z3", "error"),
            zone("z4", "delegated"),
            zone("z5", "winddown"),
            zone("z6", "suspended"),
            zone("z7", "quarantined"),
            zone("z8", "active"),
            // A zone for a domain this box doesn't hold: ignored.
            zone("z-other", "active"),
        ];
        let a = apply_zone_statuses(&rows, &zones);
        let got: Vec<(&str, &str, bool)> = a
            .changes
            .iter()
            .map(|c| (c.apex.as_str(), c.to.as_str(), c.dns_write))
            .collect();
        assert_eq!(
            got,
            vec![
                ("pend-to-active.example", "active", true),
                ("delegated.example", "active", true),
                ("winddown.example", "winddown", false),
                ("suspended.example", "suspended", false),
                ("legacy-active.example", "active", true),
            ]
        );
        assert_eq!(a.changes[0].from.as_deref(), Some("pending_ns"));
        assert_eq!(a.changes[0].zone_id, "z1");
        assert_eq!(a.missing, vec!["missing.example".to_string()]);
        assert_eq!(a.unknown, vec![("odd.example".to_string(), "quarantined".to_string())]);
        // BYO never appears anywhere.
        assert!(!a.changes.iter().any(|c| c.apex == "byo.example"));
        assert!(!a.missing.contains(&"byo.example".to_string()));
    }

    #[test]
    fn parse_zones_list_shapes() {
        let z = parse_zones_list(
            r#"{"zones":[{"id":"a","domain":"Example.COM.","status":"Pending"},{"id":"","domain":"x"},{"domain":"no-id"}],"capability":{}}"#,
        )
        .expect("parse");
        assert_eq!(z, vec![RemoteZone { id: "a".into(), domain: "example.com".into(), status: "pending".into() }]);
        assert_eq!(parse_zones_list(r#"{"zones":[]}"#).expect("empty"), vec![]);
        assert!(parse_zones_list("<html>catch-all</html>").is_err());
        assert!(parse_zones_list(r#"{"error":"nope"}"#).is_err());
        assert!(zones_from_response(&DnsHttpResponse { status: 401, body: r#"{"zones":[]}"#.into() }).is_err());
    }

    #[test]
    fn next_check_delay_schedule() {
        let s = |age, f| next_check_delay(age, f).as_secs();
        // Age steps.
        assert_eq!(s(0, 0), 60);
        assert_eq!(s(59 * MIN, 0), 60);
        assert_eq!(s(HOUR, 0), 5 * 60);
        assert_eq!(s(47 * HOUR, 0), 5 * 60);
        assert_eq!(s(48 * HOUR, 0), 30 * 60);
        assert_eq!(s(13 * DAY, 0), 30 * 60);
        assert_eq!(s(14 * DAY, 0), 6 * 3_600);
        assert_eq!(s(59 * DAY, 0), 6 * 3_600);
        assert_eq!(s(60 * DAY, 0), 86_400);
        assert_eq!(s(3 * 365 * DAY, 0), 86_400, "years-old pending zone: daily");
        assert_eq!(s(-5, 0), 60, "clock skew reads as new");
        // Errors double, cap 1 h.
        assert_eq!(s(0, 1), 120);
        assert_eq!(s(0, 2), 240);
        assert_eq!(s(0, 5), 1_920);
        assert_eq!(s(0, 6), 3_600);
        assert_eq!(s(0, 40), 3_600, "no overflow");
        assert_eq!(s(2 * DAY, 1), 3_600);
        // Errors never make an old zone check MORE often.
        assert_eq!(s(20 * DAY, 3), 6 * 3_600);
        assert_eq!(s(90 * DAY, 9), 86_400);
    }

    #[test]
    fn jitter_stays_within_ten_percent() {
        let d = Duration::from_secs(1_000);
        assert_eq!(jittered(d, 0.5).as_secs(), 1_000);
        assert_eq!(jittered(d, 0.0).as_secs(), 900);
        assert_eq!(jittered(d, 1.0).as_secs(), 1_100);
        assert_eq!(jittered(d, f64::NAN).as_secs(), 1_000);
        for _ in 0..100 {
            let j = jittered(d, random_unit()).as_secs();
            assert!((900..=1_100).contains(&j), "{j}");
        }
    }

    #[test]
    fn youngest_pending_age_ignores_byo_and_active() {
        let now = 10 * DAY;
        let rows = vec![
            row("a.example", Some("z1"), Some("pending_ns"), false, now - 3 * DAY),
            row("b.example", Some("z2"), Some("pending_ns"), false, now - 2 * HOUR),
            row("c.example", Some("z3"), Some("active"), true, now - MIN),
            row("d.example", None, None, false, now - MIN),
        ];
        assert_eq!(youngest_pending_age(&rows, now), Some(2 * HOUR));
        assert_eq!(youngest_pending_age(&rows[2..], now), None, "nothing pending: park");
        assert_eq!(youngest_pending_age(&[], now), None);
    }

    // ── tick against the shared test DB ────────────────────────────────

    struct FakeDeps {
        now: i64,
        airgap: bool,
        token: bool,
        reply: Result<DnsHttpResponse, String>,
        fetches: usize,
    }

    impl FakeDeps {
        fn new(reply: Result<DnsHttpResponse, String>) -> Self {
            Self { now: now_unix(), airgap: false, token: true, reply, fetches: 0 }
        }
    }

    impl WatchDeps for FakeDeps {
        fn now(&self) -> i64 {
            self.now
        }
        fn airgap(&self) -> bool {
            self.airgap
        }
        fn has_token(&self) -> bool {
            self.token
        }
        fn fetch(&mut self) -> Result<DnsHttpResponse, String> {
            self.fetches += 1;
            self.reply.clone()
        }
    }

    fn ok(body: &str) -> Result<DnsHttpResponse, String> {
        Ok(DnsHttpResponse { status: 200, body: body.into() })
    }

    fn seed_pending(apex: &str, zone_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        upsert_binding_zone(
            &conn,
            apex,
            &BoundZone {
                zone_id: Some(zone_id.into()),
                status: Some(ZONE_STATUS_PENDING_NS.into()),
                nameservers: vec!["ns1.k2.dev".into(), "ns2.k2.dev".into()],
                dns_write: false,
                auto_created: true,
            },
        )
        .expect("seed pending row");
    }

    fn stored(apex: &str) -> DomainBinding {
        let db = k2_core::db::shared();
        let conn = db.lock();
        get_binding(&conn, apex).expect("db").unwrap_or_else(|| panic!("{apex} row missing"))
    }

    fn drop_row(apex: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        remove_binding(&conn, apex).expect("cleanup");
    }

    /// Drain the bus and return the `domains_changed` frames for `apex`.
    fn domains_frames(
        rx: &mut tokio::sync::broadcast::Receiver<crate::session_events::SessionEvent>,
        apex: &str,
    ) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let crate::session_events::SessionEvent::DomainsChanged { reason, apex: Some(a) } = ev {
                if a == apex {
                    out.push(reason);
                }
            }
        }
        out
    }

    #[test]
    fn tick_flips_pending_to_active_and_emits_domains_changed() {
        let _ = k2_core::db::init_for_tests();
        let _lock = crate::domains::bind::pending_rows_test_lock();
        let apex = "pw-flip.example";
        seed_pending(apex, "zone-pw-flip");
        let mut rx = crate::session_events::subscribe();
        let mut deps = FakeDeps::new(ok(
            r#"{"zones":[{"id":"zone-pw-flip","domain":"pw-flip.example","status":"active"}],"capability":{"allowed":true}}"#,
        ));
        let t = tick(&mut deps);
        assert_eq!(deps.fetches, 1, "one GET per tick");
        let report = match t {
            Tick::Checked(r) => r,
            other => panic!("expected Checked, got {other:?}"),
        };
        let mine: Vec<&StatusChange> = report.changes.iter().filter(|c| c.apex == apex).collect();
        assert_eq!(mine.len(), 1, "{report:?}");
        assert_eq!(mine[0].to, "active");
        let b = stored(apex);
        assert_eq!(b.status.as_deref(), Some("active"));
        assert!(b.dns_write);
        assert_eq!(b.nameservers, vec!["ns1.k2.dev", "ns2.k2.dev"], "nameservers kept");
        assert!(b.auto_created, "auto_created kept");
        assert_eq!(domains_frames(&mut rx, apex), vec!["status_changed".to_string()]);
        assert!(last_check().is_some());
        assert!(!zone_missing(apex));

        // Same answer again: nothing stored, nothing announced.
        let mut rx = crate::session_events::subscribe();
        let mut deps = FakeDeps::new(ok(
            r#"{"zones":[{"id":"zone-pw-flip","domain":"pw-flip.example","status":"active"}]}"#,
        ));
        // No pending row left (for this test's rows) → may be NothingPending
        // if no other pending rows exist; either way this apex is unchanged.
        let _ = tick(&mut deps);
        assert!(domains_frames(&mut rx, apex).is_empty());
        drop_row(apex);
    }

    #[test]
    fn tick_skips_under_airgap_and_without_token() {
        let _ = k2_core::db::init_for_tests();
        let _lock = crate::domains::bind::pending_rows_test_lock();
        let apex = "pw-quiet.example";
        seed_pending(apex, "zone-pw-quiet");
        let active = r#"{"zones":[{"id":"zone-pw-quiet","status":"active"}]}"#;

        let mut deps = FakeDeps::new(ok(active));
        deps.airgap = true;
        assert_eq!(tick(&mut deps), Tick::AirGap);
        assert_eq!(deps.fetches, 0, "never calls k2.dev under air-gap");
        assert!(stored(apex).is_pending_ns());

        let mut deps = FakeDeps::new(ok(active));
        deps.token = false;
        assert_eq!(tick(&mut deps), Tick::NoTunnel);
        assert_eq!(deps.fetches, 0, "never calls k2.dev unpaired");
        assert!(stored(apex).is_pending_ns());
        drop_row(apex);
    }

    #[test]
    fn tick_failure_keeps_row_and_counts_failures() {
        let _ = k2_core::db::init_for_tests();
        let _lock = crate::domains::bind::pending_rows_test_lock();
        let apex = "pw-fail.example";
        seed_pending(apex, "zone-pw-fail");
        let cases: Vec<Result<DnsHttpResponse, String>> = vec![
            Err("DNS API GET /api/dns/zones: connection refused".into()),
            Ok(DnsHttpResponse { status: 502, body: "<html>bad gateway</html>".into() }),
            ok("<html>catch-all</html>"),
        ];
        for (i, reply) in cases.into_iter().enumerate() {
            let before = with_state(|s| s.failures);
            let mut deps = FakeDeps::new(reply);
            let t = tick(&mut deps);
            assert!(matches!(t, Tick::Failed(_)), "case {i}: {t:?}");
            assert_eq!(with_state(|s| s.failures), before + 1, "case {i}");
            assert!(last_error().is_some(), "case {i}");
            assert!(stored(apex).is_pending_ns(), "case {i}: row untouched");
        }
        // A good check resets the failure count.
        let mut deps = FakeDeps::new(ok(r#"{"zones":[{"id":"zone-pw-fail","status":"pending"}]}"#));
        assert!(matches!(tick(&mut deps), Tick::Checked(_)));
        assert_eq!(with_state(|s| s.failures), 0);
        assert!(last_error().is_none());
        assert!(stored(apex).is_pending_ns(), "still pending on k2.dev");
        drop_row(apex);
    }

    #[test]
    fn missing_zone_is_kept_and_shown_then_cleared() {
        let _ = k2_core::db::init_for_tests();
        let _lock = crate::domains::bind::pending_rows_test_lock();
        let apex = "pw-missing.example";
        seed_pending(apex, "zone-pw-missing");
        let mut rx = crate::session_events::subscribe();
        let mut deps = FakeDeps::new(ok(r#"{"zones":[]}"#));
        let r = match tick(&mut deps) {
            Tick::Checked(r) => r,
            other => panic!("expected Checked, got {other:?}"),
        };
        assert!(r.missing.contains(&apex.to_string()), "{r:?}");
        assert!(stored(apex).is_pending_ns(), "row kept");
        assert!(zone_missing(apex));
        assert_eq!(domains_frames(&mut rx, apex), vec!["zone_missing".to_string()]);

        // Second identical check: no new frame.
        let mut deps = FakeDeps::new(ok(r#"{"zones":[]}"#));
        let _ = tick(&mut deps);
        assert!(domains_frames(&mut rx, apex).is_empty(), "announced once");

        // Back in the list: no longer missing.
        let mut deps = FakeDeps::new(ok(r#"{"zones":[{"id":"zone-pw-missing","status":"pending"}]}"#));
        let _ = tick(&mut deps);
        assert!(!zone_missing(apex));
        assert_eq!(domains_frames(&mut rx, apex), vec!["status_changed".to_string()]);
        drop_row(apex);
    }

    #[test]
    fn byo_rows_are_never_touched_by_a_check() {
        let _ = k2_core::db::init_for_tests();
        let _lock = crate::domains::bind::pending_rows_test_lock();
        let byo = "pw-byo.example";
        let pend = "pw-byo-pending.example";
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            upsert_binding(&conn, byo, None, false).expect("seed byo");
        }
        seed_pending(pend, "zone-pw-byo-pending");
        let mut deps = FakeDeps::new(ok(r#"{"zones":[{"id":"zone-pw-byo-pending","status":"active"}]}"#));
        assert!(matches!(tick(&mut deps), Tick::Checked(_)));
        let b = stored(byo);
        assert_eq!(b.status, None);
        assert!(!b.dns_write);
        assert!(!zone_missing(byo), "BYO rows have no zone to miss");
        drop_row(byo);
        drop_row(pend);
    }

    #[test]
    fn verify_status_is_stored_on_the_row() {
        let _ = k2_core::db::init_for_tests();
        let _lock = crate::domains::bind::pending_rows_test_lock();
        let apex = "pw-verify.example";
        seed_pending(apex, "zone-pw-verify");
        assert_eq!(store_verify_status("zone-pw-verify", "pending").expect("store"), None);
        assert!(stored(apex).is_pending_ns());
        let c = store_verify_status("zone-pw-verify", "active").expect("store").expect("changed");
        assert_eq!(c.apex, apex);
        assert_eq!(c.to, "active");
        assert!(stored(apex).dns_write);
        assert_eq!(store_verify_status("zone-pw-verify", "who-knows").expect("store"), None);
        assert_eq!(store_verify_status("zone-not-bound", "active").expect("store"), None);
        drop_row(apex);
    }
}
