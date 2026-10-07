//! Renewal of certificates K2's own ACME issuer made
//! ([`crate::domains::acme`]): the renewal window, the per-name lock a
//! manual issue and the background renewer share, persisted per-name
//! backoff, and the daemon's background scan.
//!
//! Scope: names in the domain inventory (`k2 domain`) whose PEM in the
//! box store (`~/.k2/certs/<name>/`) is a Let's Encrypt leaf and was not
//! uploaded by hand, plus the CAL44 extra mail names K2 issued before
//! (present in `extra-names.json` and still extra names of a hosted
//! domain; renewed through [`crate::mail::cert_names`], whose run
//! re-checks DNS). Never a walk of the certs directory. Never Stalwart's own ACME (tls-alpn boxes renew
//! themselves), never Caddy's certificates (Caddy renews its own), never
//! the Connect `*.k2.dev` certificate (`tunnel_tls_listener`).
//!
//! The scan runs in the daemon (headless, no client needed): ~5 minutes
//! after boot, then hourly. Each scan only reads SQLite + the box store;
//! it talks to the CA only for a name that is due (inside
//! [`RENEW_WINDOW_SECS`] of expiry, or a renewed certificate that never
//! reached Stalwart) and whose backoff has elapsed. Skipped entirely
//! under air-gap.
//!
//! State: `~/.k2/certs/renewal.json` (0600). No migration.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex;

use k2_core::domains::{DomainBinding, ROLE_MAIL};
use k2_core::log_debug;

use crate::domains::status::LeafInfo;

/// A K2-issued certificate with less than this left is re-ordered (Let's
/// Encrypt's own advice: renew at 1/3 of the 90-day lifetime).
pub const RENEW_WINDOW_SECS: i64 = 30 * 86_400;
/// Doctor warns when a K2-issued certificate has less than this left.
pub const WARN_WITHIN_SECS: i64 = 14 * 86_400;
/// First scan after boot (off the boot path).
pub const FIRST_SCAN_AFTER_SECS: u64 = 5 * 60;
/// Scan cadence. Cheap (local reads); it is what makes the backoff steps
/// below real. A name is renewed at the first scan after it enters the
/// window.
pub const SCAN_EVERY_SECS: u64 = 60 * 60;
/// Retry after the 1st, 2nd, 3rd-and-later consecutive failure.
pub const BACKOFF_SECS: [i64; 3] = [3_600, 6 * 3_600, 24 * 3_600];
/// At most this many names are attempted per scan (soonest expiry
/// first); the rest wait for the next scan.
pub const MAX_ATTEMPTS_PER_SCAN: usize = 5;

/// Inside the renewal window (or already expired)?
pub fn needs_renewal(not_after: i64, now: i64) -> bool {
    not_after - now < RENEW_WINDOW_SECS
}

/// Wait before the next attempt after `failures` consecutive failures.
pub fn backoff_secs(failures: u32) -> i64 {
    let i = (failures.max(1) as usize - 1).min(BACKOFF_SECS.len() - 1);
    BACKOFF_SECS[i]
}

fn norm(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

// ── Per-name lock ───────────────────────────────────────────────────────

fn in_flight() -> &'static Mutex<BTreeSet<String>> {
    static IN_FLIGHT: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    &IN_FLIGHT
}

/// Held while one certificate run (manual issue, background renewal)
/// works on a name. A second run for the same name does not wait: it
/// gets `None` and reports busy.
#[derive(Debug)]
pub struct NameLock {
    name: String,
}

impl NameLock {
    pub fn try_acquire(name: &str) -> Option<Self> {
        let name = norm(name);
        let mut set = in_flight().lock().unwrap_or_else(|p| p.into_inner());
        if set.insert(name.clone()) {
            Some(Self { name })
        } else {
            None
        }
    }
}

impl Drop for NameLock {
    fn drop(&mut self) {
        let mut set = in_flight().lock().unwrap_or_else(|p| p.into_inner());
        set.remove(&self.name);
    }
}

/// Why an issue for an attached name did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueError {
    /// Another run holds the name's lock. Not an attempt.
    Busy(String),
    /// The name no longer qualifies (not attached, k2.dev). Not an attempt.
    NotQualified(String),
    /// The ACME order (challenge choice, CA, store write) failed.
    Order(String),
    /// A certificate is in the box store but did not reach Stalwart.
    Plant(String),
}

impl std::fmt::Display for IssueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IssueError::Busy(name) => write!(
                f,
                "a certificate run for {name} is already in progress — try again in a few minutes"
            ),
            IssueError::NotQualified(e) | IssueError::Order(e) | IssueError::Plant(e) => {
                f.write_str(e)
            }
        }
    }
}

/// A completed issue: `ordered` = a new certificate came from the CA
/// (false = the box store's certificate was still good and was re-planted).
#[derive(Debug, Clone)]
pub struct Issued {
    pub pem: crate::domains::store::InstalledPem,
    pub ordered: bool,
}

// ── State (~/.k2/certs/renewal.json) ────────────────────────────────────

/// What K2 last did for one name's certificate.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RenewRecord {
    /// `k2` (issued by K2) | `uploaded` (`k2 cert upload` — never renewed).
    pub source: Option<String>,
    pub last_attempt: Option<i64>,
    /// `background` | `manual`.
    pub last_trigger: Option<String>,
    /// `renewed` | `planted` | `failed`.
    pub last_result: Option<String>,
    pub last_error: Option<String>,
    pub last_success: Option<i64>,
    /// Consecutive failed attempts (reset on success).
    pub failures: u32,
    /// The background renewer waits until then (backoff).
    pub next_attempt: Option<i64>,
    /// Why the last scan skipped the name (not an attempt).
    pub skipped: Option<String>,
    /// A certificate is in the box store but never reached Stalwart.
    pub plant_pending: bool,
    /// Mail role: the Stalwart Certificate K2 last planted for this name.
    pub stalwart_cert_id: Option<String>,
    pub planted_not_after: Option<i64>,
    /// Earlier Certificates K2 planted for this name, removed once the
    /// new one is loaded.
    pub stale_cert_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RenewFile {
    pub names: BTreeMap<String, RenewRecord>,
    pub last_scan: Option<i64>,
    pub last_scan_note: Option<String>,
}

pub fn state_path() -> PathBuf {
    crate::domains::store::certs_root().join("renewal.json")
}

fn state_lock() -> &'static Mutex<()> {
    static LOCK: Mutex<()> = Mutex::new(());
    &LOCK
}

pub fn load() -> RenewFile {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(file: &RenewFile) -> Result<(), String> {
    let path = state_path();
    let dir = crate::domains::store::certs_root();
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let body = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod {}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename {}: {e}", path.display())
    })
}

/// Load → change → save under one lock (manual issues, the scan and the
/// mail plant all write this file).
pub fn update<R>(f: impl FnOnce(&mut RenewFile) -> R) -> Result<R, String> {
    let _g = state_lock().lock().unwrap_or_else(|p| p.into_inner());
    let mut file = load();
    let out = f(&mut file);
    save(&file)?;
    Ok(out)
}

fn update_logged(what: &str, f: impl FnOnce(&mut RenewFile)) {
    if let Err(e) = update(f) {
        log_debug!("[domains/renew] could not record {what}: {e}");
    }
}

/// Record one issue attempt (manual or background) on `rec`.
pub fn record_attempt(
    rec: &mut RenewRecord,
    trigger: &str,
    now: i64,
    result: &Result<bool, IssueError>,
) {
    rec.last_attempt = Some(now);
    rec.last_trigger = Some(trigger.to_string());
    rec.skipped = None;
    match result {
        Ok(ordered) => {
            rec.source = Some("k2".into());
            rec.last_result = Some(if *ordered { "renewed" } else { "planted" }.into());
            rec.last_error = None;
            rec.last_success = Some(now);
            rec.failures = 0;
            rec.next_attempt = None;
            rec.plant_pending = false;
        }
        Err(e) => {
            rec.last_result = Some("failed".into());
            rec.last_error = Some(e.to_string());
            rec.failures = rec.failures.saturating_add(1);
            rec.next_attempt = Some(now + backoff_secs(rec.failures));
            if matches!(e, IssueError::Plant(_)) {
                rec.plant_pending = true;
            }
        }
    }
}

fn ordered(result: &Result<Issued, IssueError>) -> Result<bool, IssueError> {
    result.as_ref().map(|i| i.ordered).map_err(Clone::clone)
}

/// `true` when `result` was an attempt (busy / no-longer-qualifies are not).
fn is_attempt<T>(result: &Result<T, IssueError>) -> bool {
    !matches!(
        result,
        Err(IssueError::Busy(_)) | Err(IssueError::NotQualified(_))
    )
}

/// Manual `k2 cert issue|renew` / `k2 hostmail cert renew`: record the
/// attempt (a success also clears the background backoff).
pub fn record_manual(name: &str, now: i64, result: &Result<Issued, IssueError>) {
    if !is_attempt(result) {
        return;
    }
    let name = norm(name);
    update_logged("manual issue", |f| {
        record_attempt(f.names.entry(name).or_default(), "manual", now, &ordered(result))
    });
}

/// Map one CAL44 per-name outcome to an attempt result (`None` = not an
/// attempt: DNS moved / unknown, zone unsupported, busy).
fn extra_outcome(
    out: &crate::mail::cert_names::NameOutcome,
    reload: &crate::mail::cert_names::ReloadOutcome,
) -> Option<Result<bool, IssueError>> {
    use crate::mail::cert_names as cn;
    match out.result {
        cn::R_ISSUED | cn::R_PLANTED if reload.method == "failed" => Some(Err(IssueError::Plant(
            reload.error.clone().unwrap_or_else(|| "TLS reload failed".into()),
        ))),
        cn::R_ISSUED => Some(Ok(true)),
        cn::R_PLANTED | cn::R_CURRENT => Some(Ok(false)),
        cn::R_FAILED | cn::R_BLOCKED => Some(Err(IssueError::Order(
            out.detail.clone().unwrap_or_else(|| out.result.to_string()),
        ))),
        _ => None,
    }
}

/// Record a CAL44 run (manual POST or background) per name.
pub fn record_extra_run(report: &crate::mail::cert_names::RunReport, trigger: &str) {
    let now = chrono::Utc::now().timestamp();
    let rows: Vec<(String, Result<bool, IssueError>)> = report
        .names
        .iter()
        .filter_map(|o| extra_outcome(o, &report.reload).map(|r| (norm(&o.name), r)))
        .collect();
    if rows.is_empty() {
        return;
    }
    update_logged("extra-name run", |f| {
        for (name, r) in &rows {
            record_attempt(f.names.entry(name.clone()).or_default(), trigger, now, r);
        }
    });
}

/// `k2 cert upload`: a hand-supplied certificate is never renewed by K2.
pub fn mark_uploaded(name: &str) {
    let name = norm(name);
    update_logged("upload", |f| {
        let rec = f.names.entry(name).or_default();
        rec.source = Some("uploaded".into());
        rec.failures = 0;
        rec.next_attempt = None;
        rec.plant_pending = false;
        rec.last_error = None;
    });
}

/// Is the certificate in the box store one K2 renews?
pub fn k2_issued(leaf: Option<&LeafInfo>, rec: Option<&RenewRecord>) -> bool {
    if rec.and_then(|r| r.source.as_deref()) == Some("uploaded") {
        return false;
    }
    leaf.is_some_and(|l| l.lets_encrypt)
}

// ── The scan ────────────────────────────────────────────────────────────

/// One attached name (`domain_names`) with its apex binding.
#[derive(Debug, Clone)]
pub struct AttachedRow {
    pub hostname: String,
    pub role: String,
    pub binding: Option<DomainBinding>,
}

/// Everything the scan does besides `renewal.json`. Production =
/// [`LiveScan`]; tests inject a recording fake.
pub trait ScanDeps {
    fn now(&self) -> i64;
    fn airgap(&self) -> bool;
    fn attached(&self) -> Vec<AttachedRow>;
    fn leaf(&self, name: &str) -> Option<LeafInfo>;
    /// `Some(why)` while the mail server must not be touched (enable).
    fn mail_busy(&self) -> Option<String>;
    /// The same issuer + per-role install as `k2 cert renew`.
    fn renew_attached(&mut self, hostname: &str) -> Result<Issued, IssueError>;
    /// CAL44 extra names K2 issued before ([`crate::mail::cert_names::renewable_extra_names`]).
    fn extras(&self) -> Vec<(String, Option<DomainBinding>)>;
    /// A fresh per-name certificate that never got planted / loaded.
    fn extra_needs_plant(&self, name: &str) -> bool;
    /// One CAL44 run restricted to `names` (DNS gate inside).
    fn renew_extras(
        &mut self,
        names: &BTreeSet<String>,
    ) -> Result<crate::mail::cert_names::RunReport, crate::mail::cert_names::LiveRunError>;
}

/// Re-check that a name still qualifies before anything goes to the CA.
pub fn qualifies(row: &AttachedRow) -> Result<(), String> {
    let Some(b) = row.binding.as_ref() else {
        return Err("the apex is no longer attached".into());
    };
    let host = norm(&row.hostname);
    if host == "k2.dev" || host.ends_with(".k2.dev") {
        return Err("Connect *.k2.dev names stay on cert.k2.dev".into());
    }
    if !k2_core::domains::hostname_under_apex(&host, &b.apex) {
        return Err(format!("not under the attached apex {}", b.apex));
    }
    if b.dns_write && b.is_pending_ns() {
        return Err(format!(
            "zone {} is pending_ns — a DNS-01 record is not visible to the CA yet",
            b.apex
        ));
    }
    Ok(())
}

/// What the scan did with one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanItem {
    pub name: String,
    /// `renewed` | `planted` | `failed` | `busy` | `skipped` | `backoff` | `deferred`.
    pub result: &'static str,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanReport {
    pub airgap: bool,
    pub items: Vec<ScanItem>,
}

/// One background pass. Every effect besides `renewal.json` goes through
/// `deps`; one name's failure never stops the others.
pub fn scan(deps: &mut dyn ScanDeps) -> ScanReport {
    let now = deps.now();
    if deps.airgap() {
        update_logged("scan", |f| {
            f.last_scan = Some(now);
            f.last_scan_note = Some("air-gap is on — no renewal attempted".into());
        });
        return ScanReport { airgap: true, items: Vec::new() };
    }
    let file = load();
    let mut items: Vec<ScanItem> = Vec::new();
    // (notAfter, name, is_extra)
    let mut due: Vec<(i64, String, bool)> = Vec::new();
    let mut skips: Vec<(String, String)> = Vec::new();
    let rows = deps.attached();
    let attached_names: BTreeSet<String> = rows.iter().map(|r| norm(&r.hostname)).collect();
    for row in rows {
        let name = norm(&row.hostname);
        let leaf = deps.leaf(&name);
        let rec = file.names.get(&name);
        if !k2_issued(leaf.as_ref(), rec) {
            continue;
        }
        let Some(leaf) = leaf else { continue };
        let pending = rec.is_some_and(|r| r.plant_pending);
        if !needs_renewal(leaf.not_after, now) && !pending {
            continue;
        }
        if let Some(t) = rec.and_then(|r| r.next_attempt).filter(|t| *t > now) {
            items.push(ScanItem {
                name,
                result: "backoff",
                detail: Some(format!("next attempt at {t}")),
            });
            continue;
        }
        let why = qualifies(&row).err().or_else(|| {
            if row.role == ROLE_MAIL {
                deps.mail_busy()
            } else {
                None
            }
        });
        if let Some(why) = why {
            skips.push((name.clone(), why.clone()));
            items.push(ScanItem { name, result: "skipped", detail: Some(why) });
            continue;
        }
        due.push((leaf.not_after, name, false));
    }
    // An extra name that is also attached is renewed on the attached path.
    for (raw, binding) in deps.extras() {
        let name = norm(&raw);
        if attached_names.contains(&name) {
            continue;
        }
        let leaf = deps.leaf(&name);
        let rec = file.names.get(&name);
        if !k2_issued(leaf.as_ref(), rec) {
            continue;
        }
        let Some(leaf) = leaf else { continue };
        if !needs_renewal(leaf.not_after, now) && !deps.extra_needs_plant(&name) {
            continue;
        }
        if let Some(t) = rec.and_then(|r| r.next_attempt).filter(|t| *t > now) {
            items.push(ScanItem { name, result: "backoff", detail: Some(format!("next attempt at {t}")) });
            continue;
        }
        let why = match binding.as_ref() {
            None => Some("the domain's zone is no longer attached".to_string()),
            Some(b) => crate::domains::acme::check_extra_name_issuable(&name, b).err(),
        }
        .or_else(|| deps.mail_busy());
        if let Some(why) = why {
            skips.push((name.clone(), why.clone()));
            items.push(ScanItem { name, result: "skipped", detail: Some(why) });
            continue;
        }
        due.push((leaf.not_after, name, true));
    }
    due.sort();
    let mut extra_due: BTreeSet<String> = BTreeSet::new();
    for (i, (_, name, is_extra)) in due.into_iter().enumerate() {
        if i >= MAX_ATTEMPTS_PER_SCAN {
            items.push(ScanItem {
                name,
                result: "deferred",
                detail: Some("attempt cap for this scan reached — next scan".into()),
            });
            continue;
        }
        if is_extra {
            extra_due.insert(name);
            continue;
        }
        let result = deps.renew_attached(&name);
        let (label, detail) = match &result {
            Ok(i) if i.ordered => ("renewed", None),
            Ok(_) => ("planted", None),
            Err(IssueError::Busy(_)) => ("busy", Some(result.as_ref().unwrap_err().to_string())),
            Err(IssueError::NotQualified(e)) => {
                skips.push((name.clone(), e.clone()));
                ("skipped", Some(e.clone()))
            }
            Err(e) => ("failed", Some(e.to_string())),
        };
        if is_attempt(&result) {
            log_debug!(
                "[domains/renew] {name}: {label}{}",
                detail.as_deref().map(|d| format!(" — {d}")).unwrap_or_default()
            );
            let n = name.clone();
            update_logged("renewal", |f| {
                record_attempt(f.names.entry(n).or_default(), "background", now, &ordered(&result))
            });
        }
        items.push(ScanItem { name, result: label, detail });
    }
    if !extra_due.is_empty() {
        items.extend(scan_extras(deps, &extra_due, now, &mut skips));
    }
    let note = summary(&items);
    update_logged("scan", |f| {
        for (name, why) in skips {
            f.names.entry(name).or_default().skipped = Some(why);
        }
        f.last_scan = Some(now);
        f.last_scan_note = Some(note);
    });
    ScanReport { airgap: false, items }
}

/// The CAL44 part of a scan: one run over the due extra names.
fn scan_extras(
    deps: &mut dyn ScanDeps,
    due: &BTreeSet<String>,
    now: i64,
    skips: &mut Vec<(String, String)>,
) -> Vec<ScanItem> {
    use crate::mail::cert_names::{self as cn, LiveRunError};
    let mut items = Vec::new();
    let report = match deps.renew_extras(due) {
        Ok(r) => r,
        Err(LiveRunError::Busy) => {
            return due
                .iter()
                .map(|n| ScanItem {
                    name: n.clone(),
                    result: "busy",
                    detail: Some("a per-name certificate run is in progress".into()),
                })
                .collect();
        }
        Err(LiveRunError::NotReady(e)) | Err(LiveRunError::Dns(e)) => {
            let err: Result<bool, IssueError> = Err(IssueError::Order(e.clone()));
            update_logged("extra-name renewal", |f| {
                for n in due {
                    record_attempt(f.names.entry(n.clone()).or_default(), "background", now, &err);
                }
            });
            return due
                .iter()
                .map(|n| ScanItem { name: n.clone(), result: "failed", detail: Some(e.clone()) })
                .collect();
        }
    };
    let mut rows: Vec<(String, Result<bool, IssueError>)> = Vec::new();
    for out in &report.names {
        let name = norm(&out.name);
        let (label, detail) = match extra_outcome(out, &report.reload) {
            Some(Ok(true)) => ("renewed", None),
            Some(Ok(false)) => ("planted", None),
            Some(Err(e)) => ("failed", Some(e.to_string())),
            None if out.result == cn::R_BUSY => ("busy", out.detail.clone()),
            None => {
                let why = out.detail.clone().unwrap_or_else(|| out.result.to_string());
                skips.push((name.clone(), why.clone()));
                ("skipped", Some(why))
            }
        };
        if let Some(r) = extra_outcome(out, &report.reload) {
            log_debug!("[domains/renew] {name}: {label}");
            rows.push((name.clone(), r));
        }
        items.push(ScanItem { name, result: label, detail });
    }
    update_logged("extra-name renewal", |f| {
        for (name, r) in &rows {
            record_attempt(f.names.entry(name.clone()).or_default(), "background", now, r);
        }
    });
    items
}

fn summary(items: &[ScanItem]) -> String {
    if items.is_empty() {
        return "nothing due".into();
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for i in items {
        *counts.entry(i.result).or_default() += 1;
    }
    counts
        .iter()
        .map(|(k, v)| format!("{v} {k}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Production [`ScanDeps`].
pub struct LiveScan;

impl ScanDeps for LiveScan {
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn airgap(&self) -> bool {
        k2_core::airgap::enabled()
    }

    fn attached(&self) -> Vec<AttachedRow> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let names = k2_core::domains::list_names(&conn).unwrap_or_default();
        names
            .into_iter()
            .map(|n| AttachedRow {
                binding: k2_core::domains::get_binding(&conn, &n.apex).ok().flatten(),
                hostname: n.hostname,
                role: n.role,
            })
            .collect()
    }

    fn leaf(&self, name: &str) -> Option<LeafInfo> {
        leaf_from_store(name)
    }

    fn mail_busy(&self) -> Option<String> {
        use std::sync::atomic::Ordering;
        if crate::mail::supervisor::enable_running().load(Ordering::SeqCst) {
            return Some("a mail server enable is running".into());
        }
        if crate::mail::supervisor::upgrade_running().load(Ordering::SeqCst) {
            return Some("a mail server upgrade is running".into());
        }
        None
    }

    fn renew_attached(&mut self, hostname: &str) -> Result<Issued, IssueError> {
        crate::domains::acme::issue_attached_core(hostname)
    }

    fn extras(&self) -> Vec<(String, Option<DomainBinding>)> {
        if !crate::mail::supervisor::mail_supported()
            || crate::mail::supervisor::current_status().is_none()
        {
            return Vec::new();
        }
        crate::mail::cert_names::renewable_extra_names()
    }

    fn extra_needs_plant(&self, name: &str) -> bool {
        crate::mail::cert_names::needs_plant(name)
    }

    fn renew_extras(
        &mut self,
        names: &BTreeSet<String>,
    ) -> Result<crate::mail::cert_names::RunReport, crate::mail::cert_names::LiveRunError> {
        let (report, state_err) = crate::mail::cert_names::live_run(Some(names), None)?;
        if let Some(e) = state_err {
            log_debug!("[domains/renew] extra-names.json: {e}");
        }
        Ok(report)
    }
}

pub fn leaf_from_store(name: &str) -> Option<LeafInfo> {
    let pem = crate::domains::store::load(name)?;
    crate::domains::status::pem_leaf_info(&pem.chain_pem)
}

/// Start the background renewer (one detached thread for the life of the
/// daemon; a panic is contained per scan). No-op in tests.
pub fn spawn() {
    if cfg!(test) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("k2-cert-renew".into())
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(FIRST_SCAN_AFTER_SECS));
            loop {
                let r = std::panic::catch_unwind(|| scan(&mut LiveScan));
                if r.is_err() {
                    log_debug!("[domains/renew] scan panicked — retrying next scan");
                }
                std::thread::sleep(std::time::Duration::from_secs(SCAN_EVERY_SECS));
            }
        });
}

// ── Status + doctor ─────────────────────────────────────────────────────

/// Renewal state word for one name. Pure.
pub fn state_word(leaf: Option<&LeafInfo>, rec: Option<&RenewRecord>, now: i64) -> &'static str {
    if rec.and_then(|r| r.source.as_deref()) == Some("uploaded") {
        return "uploaded";
    }
    let Some(leaf) = leaf.filter(|_| k2_issued(leaf, rec)) else {
        return "not-k2";
    };
    if leaf.not_after <= now {
        return "expired";
    }
    if rec.is_some_and(|r| r.failures > 0) {
        return "retrying";
    }
    if rec.is_some_and(|r| r.plant_pending) {
        return "plant-pending";
    }
    if needs_renewal(leaf.not_after, now) {
        return "due";
    }
    "ok"
}

/// `renewal` object for one name. Pure.
pub fn view_json(leaf: Option<&LeafInfo>, rec: Option<&RenewRecord>, now: i64) -> serde_json::Value {
    let issued = k2_issued(leaf, rec);
    let exp = leaf.map(|l| l.not_after);
    serde_json::json!({
        "k2Issued": issued,
        "state": state_word(leaf, rec, now),
        "expiresAt": exp,
        "renewFrom": if issued { exp.map(|e| e - RENEW_WINDOW_SECS) } else { None },
        "lastAttempt": rec.and_then(|r| r.last_attempt),
        "lastTrigger": rec.and_then(|r| r.last_trigger.clone()),
        "lastResult": rec.and_then(|r| r.last_result.clone()),
        "lastError": rec.and_then(|r| r.last_error.clone()),
        "lastSuccess": rec.and_then(|r| r.last_success),
        "failures": rec.map(|r| r.failures).unwrap_or(0),
        "nextAttempt": rec.and_then(|r| r.next_attempt),
        "skipped": rec.and_then(|r| r.skipped.clone()),
    })
}

/// `renewal` for one name from the box store + `renewal.json`.
pub fn name_json(name: &str) -> serde_json::Value {
    let name = norm(name);
    let file = load();
    let leaf = leaf_from_store(&name);
    view_json(leaf.as_ref(), file.names.get(&name), chrono::Utc::now().timestamp())
}

/// `certRenewal` for `k2 hostmail status`.
pub fn mail_status_json(mail_host: Option<&str>) -> serde_json::Value {
    let file = load();
    let now = chrono::Utc::now().timestamp();
    let host = mail_host.map(norm).filter(|h| !h.is_empty());
    let mail = host.as_ref().map(|h| {
        let leaf = leaf_from_store(h);
        let mut v = view_json(leaf.as_ref(), file.names.get(h), now);
        v["name"] = serde_json::json!(h);
        v
    });
    let extra: Vec<serde_json::Value> = crate::mail::cert_names::renewable_extra_names()
        .into_iter()
        .map(|(n, _)| {
            let n = norm(&n);
            let leaf = leaf_from_store(&n);
            let mut v = view_json(leaf.as_ref(), file.names.get(&n), now);
            v["name"] = serde_json::json!(n);
            v
        })
        .collect();
    serde_json::json!({
        "mailHost": mail,
        "extraNames": extra,
        "lastScan": file.last_scan,
        "lastScanNote": file.last_scan_note,
        "windowDays": RENEW_WINDOW_SECS / 86_400,
        "scanEverySecs": SCAN_EVERY_SECS,
        "airgap": k2_core::airgap::enabled(),
    })
}

pub const DOCTOR_ID: &str = "k2-cert-renewal";
const DOCTOR_LABEL: &str = "K2-issued certificates renew on time";

/// One K2-issued name for the doctor.
#[derive(Debug, Clone, Default)]
pub struct DoctorEntry {
    pub name: String,
    pub leaf: Option<LeafInfo>,
    pub rec: Option<RenewRecord>,
    /// SHA-256 of the leaf K2 planted into Stalwart (mail host only).
    pub planted_sha256: Option<String>,
    /// What Stalwart presents for the name (`None` = not probed).
    pub served: Option<crate::domains::status::Served>,
    /// The port `served` was read on.
    pub port: u16,
}

/// The remedy when the planted certificate is not the one being served.
pub const NOT_LOADED_REMEDY: &str = "the hot reload did not land — restart Stalwart: \
     `sudo systemctl restart stalwart` (on boxes without the mail helper that is the way; \
     never `k2 hostmail disable/enable`)";

/// Warn (never gates direct send) when a K2-issued certificate is within
/// [`WARN_WITHIN_SECS`] of expiry, its last renewal attempt failed (at
/// once — the current certificate keeps serving meanwhile), or the
/// certificate K2 planted is not the one Stalwart presents (planted but
/// never loaded).
pub fn doctor_check(entries: &[DoctorEntry], now: i64) -> crate::mail::doctor::DoctorCheck {
    use crate::mail::doctor::{DoctorCheck, ST_INFO, ST_PASS, ST_WARN};
    let mk = |status: &'static str, detail: String| DoctorCheck {
        id: DOCTOR_ID.into(),
        label: DOCTOR_LABEL.into(),
        status,
        detail,
        gates_direct: false,
    };
    let mut ok = Vec::new();
    let mut problems = Vec::new();
    for e in entries {
        if !k2_issued(e.leaf.as_ref(), e.rec.as_ref()) {
            continue;
        }
        let Some(leaf) = e.leaf.as_ref() else { continue };
        let left = leaf.not_after - now;
        let failed = e.rec.as_ref().filter(|r| r.failures > 0);
        if let (Some(planted), Some(crate::domains::status::Served::Leaf(served))) =
            (e.planted_sha256.as_deref(), e.served.as_ref())
        {
            if planted != served {
                problems.push(format!(
                    "{}: the certificate K2 planted (sha256 {}…) is not what Stalwart serves \
                     on :{} (sha256 {}…) — if `k2 hostmail cert renew` succeeded but tls-cert \
                     still shows the old leaf, {NOT_LOADED_REMEDY}",
                    e.name,
                    &planted[..planted.len().min(16)],
                    e.port,
                    &served[..served.len().min(16)],
                ));
                continue;
            }
        }
        if left < WARN_WITHIN_SECS || failed.is_some() {
            let when = if left <= 0 {
                "has EXPIRED".to_string()
            } else {
                format!("expires in {} days", left / 86_400)
            };
            let why = match failed {
                Some(r) => format!(
                    "; last renewal failed ({} in a row; the current certificate keeps \
                     serving): {}{}",
                    r.failures,
                    r.last_error.as_deref().unwrap_or("no detail"),
                    r.next_attempt
                        .map(|t| format!("; the daemon retries at {t}"))
                        .unwrap_or_default()
                ),
                None => "; the daemon has not renewed it yet".into(),
            };
            problems.push(format!("{} {when}{why}", e.name));
        } else {
            ok.push(e.name.clone());
        }
    }
    if !problems.is_empty() {
        return mk(
            ST_WARN,
            format!(
                "{}. Retry now: k2 cert renew <hostname> (or k2 hostmail cert renew)",
                problems.join(". ")
            ),
        );
    }
    if ok.is_empty() {
        return mk(ST_INFO, "no certificate on this box was issued by K2".into());
    }
    mk(
        ST_PASS,
        format!(
            "{} current; the daemon renews them {} days before expiry",
            ok.join(", "),
            RENEW_WINDOW_SECS / 86_400
        ),
    )
}

/// The doctor check over every attached K2-issued name (`only_apex` =
/// names under one hosted domain). `mail` = (mail host, Stalwart's TLS
/// port): that one name is probed for the leaf Stalwart presents.
pub fn doctor_check_live(
    only_apex: Option<&str>,
    mail: Option<(&str, u16)>,
) -> crate::mail::doctor::DoctorCheck {
    let file = load();
    let mut names: BTreeSet<String> = LiveScan
        .attached()
        .into_iter()
        .map(|r| norm(&r.hostname))
        .collect();
    names.extend(
        crate::mail::cert_names::renewable_extra_names()
            .into_iter()
            .map(|(n, _)| norm(&n)),
    );
    let entries: Vec<DoctorEntry> = names
        .into_iter()
        .filter(|n| only_apex.is_none_or(|a| k2_core::domains::hostname_under_apex(n, a)))
        .map(|name| {
            let rec = file.names.get(&name).cloned();
            let probe = mail
                .filter(|(host, _)| norm(host) == name)
                .filter(|_| rec.as_ref().is_some_and(|r| r.stalwart_cert_id.is_some()));
            let (planted_sha256, served, port) = match probe {
                Some((host, port)) => (
                    crate::domains::store::load(&name)
                        .and_then(|p| crate::domains::status::pem_leaf_sha256(&p.chain_pem)),
                    Some(crate::domains::status::served_leaf(host, port)),
                    port,
                ),
                None => (None, None, 0),
            };
            DoctorEntry { leaf: leaf_from_store(&name), rec, name, planted_sha256, served, port }
        })
        .collect();
    doctor_check(&entries, chrono::Utc::now().timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 86_400;

    fn le(not_after: i64) -> LeafInfo {
        LeafInfo { not_after, issuer: "CN=R11, O=Let's Encrypt".into(), lets_encrypt: true }
    }

    fn binding(apex: &str, dns_write: bool, status: Option<&str>) -> DomainBinding {
        DomainBinding {
            apex: apex.into(),
            zone_id: dns_write.then(|| "zone-1".to_string()),
            dns_write,
            created_at: 0,
            status: status.map(String::from),
            nameservers: Vec::new(),
            auto_created: false,
        }
    }

    fn row(host: &str, role: &str, b: Option<DomainBinding>) -> AttachedRow {
        AttachedRow { hostname: host.into(), role: role.into(), binding: b }
    }

    struct Fake {
        now: i64,
        airgap: bool,
        rows: Vec<AttachedRow>,
        leaves: HashMap<String, LeafInfo>,
        fail: HashMap<String, IssueError>,
        mail_busy: Option<String>,
        calls: RefCell<Vec<String>>,
        extras: Vec<(String, Option<DomainBinding>)>,
        needs_plant: BTreeSet<String>,
        /// Per extra name: the cert_names result the fake run reports.
        extra_results: HashMap<String, &'static str>,
        extra_reload: &'static str,
        extra_busy: bool,
    }

    impl Fake {
        fn new(rows: Vec<AttachedRow>) -> Self {
            Self {
                now: NOW,
                airgap: false,
                rows,
                leaves: HashMap::new(),
                fail: HashMap::new(),
                mail_busy: None,
                calls: RefCell::new(Vec::new()),
                extras: Vec::new(),
                needs_plant: BTreeSet::new(),
                extra_results: HashMap::new(),
                extra_reload: "action",
                extra_busy: false,
            }
        }
        fn leaf(mut self, name: &str, l: LeafInfo) -> Self {
            self.leaves.insert(name.into(), l);
            self
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl ScanDeps for Fake {
        fn now(&self) -> i64 {
            self.now
        }
        fn airgap(&self) -> bool {
            self.airgap
        }
        fn attached(&self) -> Vec<AttachedRow> {
            self.rows.clone()
        }
        fn leaf(&self, name: &str) -> Option<LeafInfo> {
            self.leaves.get(name).cloned()
        }
        fn mail_busy(&self) -> Option<String> {
            self.mail_busy.clone()
        }
        fn renew_attached(&mut self, hostname: &str) -> Result<Issued, IssueError> {
            self.calls.borrow_mut().push(format!("renew {hostname}"));
            if let Some(e) = self.fail.get(hostname) {
                return Err(e.clone());
            }
            self.leaves.insert(hostname.into(), le(self.now + 90 * DAY));
            Ok(Issued {
                pem: crate::domains::store::InstalledPem {
                    hostname: hostname.into(),
                    chain_pem: String::new(),
                    key_pem: String::new(),
                },
                ordered: true,
            })
        }
        fn extras(&self) -> Vec<(String, Option<DomainBinding>)> {
            self.extras.clone()
        }
        fn extra_needs_plant(&self, name: &str) -> bool {
            self.needs_plant.contains(name)
        }
        fn renew_extras(
            &mut self,
            names: &BTreeSet<String>,
        ) -> Result<crate::mail::cert_names::RunReport, crate::mail::cert_names::LiveRunError> {
            use crate::mail::cert_names as cn;
            self.calls
                .borrow_mut()
                .push(format!("extras {}", names.iter().cloned().collect::<Vec<_>>().join(",")));
            if self.extra_busy {
                return Err(cn::LiveRunError::Busy);
            }
            let outs = names
                .iter()
                .map(|n| {
                    let result = self.extra_results.get(n).copied().unwrap_or(cn::R_ISSUED);
                    cn::NameOutcome {
                        name: n.clone(),
                        apex: "example.com".into(),
                        points_here: cn::PointsHere::Yes,
                        result,
                        detail: (result != cn::R_ISSUED).then(|| format!("{result} detail")),
                        cert_id: None,
                        expires_at: None,
                    }
                })
                .collect();
            let reload = cn::ReloadOutcome {
                method: self.extra_reload,
                error: (self.extra_reload == "failed").then(|| "reload and restart failed".to_string()),
            };
            Ok(cn::RunReport { names: outs, reload })
        }
    }

    #[test]
    fn window_math() {
        assert!(!needs_renewal(NOW + 31 * DAY, NOW), "31 days left: reuse");
        assert!(needs_renewal(NOW + 29 * DAY, NOW), "29 days left: renew");
        assert!(needs_renewal(NOW - 1, NOW), "expired: renew");
        assert_eq!(backoff_secs(1), 3_600);
        assert_eq!(backoff_secs(2), 6 * 3_600);
        assert_eq!(backoff_secs(3), 24 * 3_600);
        assert_eq!(backoff_secs(9), 24 * 3_600, "capped");
    }

    #[test]
    fn name_lock_is_exclusive_per_name() {
        let a = NameLock::try_acquire("lock-a.example.com").expect("first");
        assert!(NameLock::try_acquire("LOCK-A.example.com.").is_none(), "same name, any spelling");
        let b = NameLock::try_acquire("lock-b.example.com");
        assert!(b.is_some(), "other names are independent");
        drop(a);
        assert!(NameLock::try_acquire("lock-a.example.com").is_some(), "released on drop");
    }

    #[test]
    fn scan_picks_only_in_window_k2_issued_names() {
        let _home = crate::test_support::TempHome::new();
        let b = || Some(binding("example.com", false, None));
        let mut fake = Fake::new(vec![
            row("fresh.example.com", "other", b()),
            row("due.example.com", "other", b()),
            row("expired.example.com", "mail", b()),
            row("byhand.example.com", "other", b()),
            row("caddy.example.com", "publish", b()),
            row("rcgen.example.com", "other", b()),
        ])
        .leaf("fresh.example.com", le(NOW + 31 * DAY))
        .leaf("due.example.com", le(NOW + 29 * DAY))
        .leaf("expired.example.com", le(NOW - DAY))
        .leaf("byhand.example.com", le(NOW + DAY))
        .leaf(
            "rcgen.example.com",
            LeafInfo { not_after: NOW + DAY, issuer: "CN=rcgen self signed".into(), lets_encrypt: false },
        );
        // caddy.example.com: nothing in the box store (Caddy keeps its own).
        mark_uploaded("byhand.example.com");
        let r = scan(&mut fake);
        assert_eq!(fake.calls(), vec!["renew expired.example.com", "renew due.example.com"], "soonest expiry first");
        assert!(r.items.iter().all(|i| i.result == "renewed"), "{:?}", r.items);
        let f = load();
        assert_eq!(f.names["due.example.com"].last_trigger.as_deref(), Some("background"));
        assert_eq!(f.names["due.example.com"].last_result.as_deref(), Some("renewed"));
        assert!(!f.names.contains_key("fresh.example.com"));
        assert_eq!(f.names["byhand.example.com"].last_attempt, None, "uploaded is never renewed");
        assert!(f.last_scan == Some(NOW));
    }

    /// The scan drives off the domain inventory only: a box-store dir for
    /// a name no longer in `domain_names` (left over from a scratch run)
    /// is never renewed, planted, recorded or deleted.
    #[test]
    fn orphan_cert_store_dir_is_ignored_attached_name_is_renewed() {
        let _home = crate::test_support::TempHome::new();
        // A real PEM dir in the box store for the orphan, inside the window.
        let orphan = "scratch-le1.example.com";
        crate::domains::store::install(
            orphan,
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
            "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n",
        )
        .expect("orphan dir");
        let mut fake = Fake::new(vec![row("app.example.com", "other", Some(binding("example.com", false, None)))])
            .leaf("app.example.com", le(NOW + 10 * DAY))
            .leaf(orphan, le(NOW + DAY));
        let r = scan(&mut fake);
        assert_eq!(fake.calls(), vec!["renew app.example.com"]);
        assert!(r.items.iter().all(|i| i.name != orphan), "{:?}", r.items);
        assert!(!load().names.contains_key(orphan), "no record for an orphan");
        assert!(crate::domains::store::host_dir(orphan).is_dir(), "never deleted");
        // The doctor and status only list attached names, so the orphan
        // never warns.
        let entries = vec![DoctorEntry { name: "app.example.com".into(), leaf: Some(le(NOW + 89 * DAY)), ..Default::default() }];
        assert_eq!(doctor_check(&entries, NOW).status, "pass");
        // Production enumeration reads SQLite, never the store dirs.
        let _ = k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::upsert_binding(&conn, "orphan-live.test", None, false).unwrap();
            k2_core::domains::upsert_name(&conn, "app.orphan-live.test", "orphan-live.test", "other").unwrap();
        }
        crate::domains::store::install(
            "scratch-le2.orphan-live.test",
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
            "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n",
        )
        .expect("orphan dir");
        let live: Vec<String> = LiveScan.attached().into_iter().map(|r| r.hostname).collect();
        assert!(live.contains(&"app.orphan-live.test".to_string()), "{live:?}");
        assert!(!live.iter().any(|h| h.starts_with("scratch-le")), "{live:?}");
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = k2_core::domains::remove_binding(&conn, "orphan-live.test");
    }

    #[test]
    fn scan_skips_everything_under_airgap() {
        let _home = crate::test_support::TempHome::new();
        let mut fake = Fake::new(vec![row("due.example.com", "other", Some(binding("example.com", false, None)))])
            .leaf("due.example.com", le(NOW + DAY));
        fake.airgap = true;
        let r = scan(&mut fake);
        assert!(r.airgap);
        assert!(fake.calls().is_empty(), "no CA traffic under air-gap");
        assert!(load().last_scan_note.unwrap().contains("air-gap"));
    }

    #[test]
    fn scan_skips_names_that_no_longer_qualify() {
        let _home = crate::test_support::TempHome::new();
        let mut fake = Fake::new(vec![
            row("gone.example.com", "other", None),
            row("pending.example.net", "other", Some(binding("example.net", true, Some(k2_core::domains::ZONE_STATUS_PENDING_NS)))),
            row("mail.example.org", "mail", Some(binding("example.org", true, Some("active")))),
        ])
        .leaf("gone.example.com", le(NOW + DAY))
        .leaf("pending.example.net", le(NOW + DAY))
        .leaf("mail.example.org", le(NOW + DAY));
        fake.mail_busy = Some("a mail server enable is running".into());
        let r = scan(&mut fake);
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
        assert!(r.items.iter().all(|i| i.result == "skipped"), "{:?}", r.items);
        let f = load();
        assert!(f.names["gone.example.com"].skipped.as_deref().unwrap().contains("no longer attached"));
        assert!(f.names["pending.example.net"].skipped.as_deref().unwrap().contains("pending_ns"));
        assert!(f.names["mail.example.org"].skipped.as_deref().unwrap().contains("enable"));
        assert_eq!(f.names["gone.example.com"].failures, 0, "a skip is not a failure");
    }

    #[test]
    fn backoff_persists_across_a_restart_and_isolates_names() {
        let _home = crate::test_support::TempHome::new();
        let b = || Some(binding("example.com", false, None));
        let rows = vec![row("bad.example.com", "other", b()), row("good.example.com", "other", b())];
        let mut fake = Fake::new(rows.clone())
            .leaf("bad.example.com", le(NOW + DAY))
            .leaf("good.example.com", le(NOW + 2 * DAY));
        fake.fail.insert("bad.example.com".into(), IssueError::Order("acme new order: rateLimited".into()));
        let r = scan(&mut fake);
        let res: Vec<(&str, &str)> = r.items.iter().map(|i| (i.name.as_str(), i.result)).collect();
        assert_eq!(res, vec![("bad.example.com", "failed"), ("good.example.com", "renewed")], "one failure never stops the others");
        let rec = load().names["bad.example.com"].clone();
        assert_eq!(rec.failures, 1);
        assert_eq!(rec.next_attempt, Some(NOW + 3_600));
        assert!(rec.last_error.unwrap().contains("rateLimited"));

        // "Restart": a brand-new deps object 30 minutes later reads the
        // same file and does not hammer the CA.
        let mut fake2 = Fake::new(rows.clone()).leaf("bad.example.com", le(NOW + DAY));
        fake2.now = NOW + 1_800;
        fake2.fail.insert("bad.example.com".into(), IssueError::Order("still failing".into()));
        let r = scan(&mut fake2);
        assert!(fake2.calls().is_empty(), "{:?}", fake2.calls());
        assert_eq!(r.items[0].result, "backoff");

        // After the hour: retried, second failure → 6 h.
        fake2.now = NOW + 3_601;
        scan(&mut fake2);
        assert_eq!(fake2.calls(), vec!["renew bad.example.com"]);
        let rec = load().names["bad.example.com"].clone();
        assert_eq!(rec.failures, 2);
        assert_eq!(rec.next_attempt, Some(NOW + 3_601 + 6 * 3_600));

        // Success clears the backoff.
        fake2.fail.clear();
        fake2.now = NOW + 3_601 + 6 * 3_600;
        scan(&mut fake2);
        let rec = load().names["bad.example.com"].clone();
        assert_eq!((rec.failures, rec.next_attempt), (0, None));
        assert_eq!(rec.last_result.as_deref(), Some("renewed"));
    }

    #[test]
    fn plant_failure_keeps_the_name_pending_without_a_new_window() {
        let _home = crate::test_support::TempHome::new();
        let rows = vec![row("mail.example.com", "mail", Some(binding("example.com", true, Some("active"))))];
        let mut fake = Fake::new(rows.clone()).leaf("mail.example.com", le(NOW + DAY));
        fake.fail.insert(
            "mail.example.com".into(),
            IssueError::Plant("ReloadTlsCertificates failed and the restart failed".into()),
        );
        scan(&mut fake);
        assert!(load().names["mail.example.com"].plant_pending);
        // The new certificate is fresh on disk, but the plant is retried
        // after the backoff (no new CA order: the issuer reuses the store).
        let mut fake2 = Fake::new(rows).leaf("mail.example.com", le(NOW + 89 * DAY));
        fake2.now = NOW + 3_601;
        scan(&mut fake2);
        assert_eq!(fake2.calls(), vec!["renew mail.example.com"]);
        assert!(!load().names["mail.example.com"].plant_pending);
    }

    #[test]
    fn busy_name_is_not_a_failure_and_lock_blocks_the_scan() {
        let _home = crate::test_support::TempHome::new();
        let mut fake = Fake::new(vec![row("busy.example.com", "other", Some(binding("example.com", false, None)))])
            .leaf("busy.example.com", le(NOW + DAY));
        fake.fail.insert("busy.example.com".into(), IssueError::Busy("busy.example.com".into()));
        let r = scan(&mut fake);
        assert_eq!(r.items[0].result, "busy");
        let f = load();
        assert!(f.names.get("busy.example.com").is_none_or(|r| r.failures == 0 && r.last_attempt.is_none()));
    }

    #[test]
    fn attempt_cap_defers_the_rest() {
        let _home = crate::test_support::TempHome::new();
        let mut rows = Vec::new();
        let mut fake = Fake::new(Vec::new());
        for i in 0..(MAX_ATTEMPTS_PER_SCAN + 2) {
            let h = format!("n{i}.example.com");
            rows.push(row(&h, "other", Some(binding("example.com", false, None))));
            fake = fake.leaf(&h, le(NOW + DAY + i as i64));
        }
        fake.rows = rows;
        let r = scan(&mut fake);
        assert_eq!(fake.calls().len(), MAX_ATTEMPTS_PER_SCAN);
        assert_eq!(r.items.iter().filter(|i| i.result == "deferred").count(), 2);
    }

    #[test]
    fn state_file_is_0600_and_round_trips() {
        let _home = crate::test_support::TempHome::new();
        update(|f| {
            f.names.insert("a.example.com".into(), RenewRecord { failures: 2, ..Default::default() });
        })
        .expect("save");
        assert_eq!(load().names["a.example.com"].failures, 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(state_path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    fn k2_zone(apex: &str) -> Option<DomainBinding> {
        Some(binding(apex, true, Some("active")))
    }

    /// CAL44 names come only from the extra-names state (via `extras`):
    /// in-window ones go to ONE cert_names run, fresh ones stay out, a
    /// never-loaded one is finished, BYO zones are skipped, failures back
    /// off per name, and a DNS skip is not a failure.
    #[test]
    fn scan_renews_due_extra_names_in_one_run() {
        use crate::mail::cert_names as cn;
        let _home = crate::test_support::TempHome::new();
        let mut fake = Fake::new(vec![row("mail.example.com", "mail", k2_zone("example.com"))])
            .leaf("mail.example.com", le(NOW + 80 * DAY))
            .leaf("autoconfig.example.com", le(NOW + 20 * DAY))
            .leaf("autodiscover.example.com", le(NOW + 25 * DAY))
            .leaf("mta-sts.example.com", le(NOW + 80 * DAY))
            .leaf("ua-auto-config.example.com", le(NOW + 85 * DAY))
            .leaf("autoconfig.example.net", le(NOW + DAY))
            .leaf("autoconfig.example.org", le(NOW + 2 * DAY));
        fake.extras = vec![
            ("autoconfig.example.com".into(), k2_zone("example.com")),
            ("autodiscover.example.com".into(), k2_zone("example.com")),
            ("mta-sts.example.com".into(), k2_zone("example.com")),
            ("ua-auto-config.example.com".into(), k2_zone("example.com")),
            ("autoconfig.example.net".into(), Some(binding("example.net", false, None))),
            ("autoconfig.example.org".into(), k2_zone("example.org")),
        ];
        fake.needs_plant.insert("ua-auto-config.example.com".into());
        fake.extra_results.insert("autodiscover.example.com".into(), cn::R_FAILED);
        fake.extra_results.insert("autoconfig.example.org".into(), cn::R_SKIPPED_DNS);
        let r = scan(&mut fake);
        assert_eq!(
            fake.calls(),
            vec!["extras autoconfig.example.com,autoconfig.example.org,autodiscover.example.com,ua-auto-config.example.com"],
            "one run; fresh mta-sts and the BYO zone stay out; the attached mail host is fresh"
        );
        let by: HashMap<&str, &str> = r.items.iter().map(|i| (i.name.as_str(), i.result)).collect();
        assert_eq!(by["autoconfig.example.com"], "renewed");
        assert_eq!(by["autodiscover.example.com"], "failed");
        assert_eq!(by["autoconfig.example.org"], "skipped", "DNS moved: not renewed, not a failure");
        assert_eq!(by["autoconfig.example.net"], "skipped", "BYO zone");
        let f = load();
        assert_eq!(f.names["autodiscover.example.com"].failures, 1);
        assert_eq!(f.names["autodiscover.example.com"].next_attempt, Some(NOW + 3_600));
        assert_eq!(f.names["autoconfig.example.com"].last_result.as_deref(), Some("renewed"));
        assert!(f.names.get("autoconfig.example.org").is_none_or(|r| r.failures == 0));
        assert!(f.names["autoconfig.example.net"].skipped.as_deref().unwrap().contains("not K2-hosted"));

        // Restart 10 minutes later: the failed name waits out its backoff.
        let mut fake2 = Fake::new(Vec::new()).leaf("autodiscover.example.com", le(NOW + 25 * DAY));
        fake2.now = NOW + 600;
        fake2.extras = vec![("autodiscover.example.com".into(), k2_zone("example.com"))];
        let r = scan(&mut fake2);
        assert!(fake2.calls().is_empty(), "{:?}", fake2.calls());
        assert_eq!(r.items[0].result, "backoff");
    }

    #[test]
    fn extra_reload_failure_is_a_plant_failure_and_busy_run_is_not() {
        let _home = crate::test_support::TempHome::new();
        let mut fake = Fake::new(Vec::new()).leaf("autoconfig.example.com", le(NOW + DAY));
        fake.extras = vec![("autoconfig.example.com".into(), k2_zone("example.com"))];
        fake.extra_reload = "failed";
        scan(&mut fake);
        let rec = load().names["autoconfig.example.com"].clone();
        assert!(rec.plant_pending && rec.failures == 1, "{rec:?}");

        let _ = std::fs::remove_file(state_path());
        let mut fake = Fake::new(Vec::new()).leaf("autoconfig.example.com", le(NOW + DAY));
        fake.extras = vec![("autoconfig.example.com".into(), k2_zone("example.com"))];
        fake.extra_busy = true;
        let r = scan(&mut fake);
        assert_eq!(r.items[0].result, "busy");
        assert!(load().names.get("autoconfig.example.com").is_none_or(|r| r.failures == 0));
    }

    #[test]
    fn status_words_and_doctor() {
        let ok = le(NOW + 60 * DAY);
        let soon = le(NOW + 10 * DAY);
        let failing = RenewRecord {
            failures: 1,
            last_error: Some("acme new order: rateLimited".into()),
            next_attempt: Some(NOW + 3_600),
            ..Default::default()
        };
        let uploaded = RenewRecord { source: Some("uploaded".into()), ..Default::default() };
        assert_eq!(state_word(Some(&ok), None, NOW), "ok");
        assert_eq!(state_word(Some(&le(NOW + 20 * DAY)), None, NOW), "due");
        assert_eq!(state_word(Some(&ok), Some(&failing), NOW), "retrying");
        assert_eq!(state_word(Some(&le(NOW - 1)), None, NOW), "expired");
        assert_eq!(state_word(Some(&ok), Some(&uploaded), NOW), "uploaded");
        assert_eq!(state_word(None, None, NOW), "not-k2");
        let v = view_json(Some(&soon), Some(&failing), NOW);
        assert_eq!(v["k2Issued"], true);
        assert_eq!(v["renewFrom"], serde_json::json!(NOW + 10 * DAY - RENEW_WINDOW_SECS));
        assert_eq!(v["nextAttempt"], serde_json::json!(NOW + 3_600));

        let entry = |name: &str, leaf: LeafInfo, rec: Option<RenewRecord>| DoctorEntry {
            name: name.into(),
            leaf: Some(leaf),
            rec,
            ..Default::default()
        };
        let c = doctor_check(&[entry("a.example.com", ok.clone(), None)], NOW);
        assert_eq!((c.status, c.gates_direct), ("pass", false));
        let c = doctor_check(&[entry("a.example.com", soon.clone(), None)], NOW);
        assert_eq!(c.status, "warn");
        assert!(c.detail.contains("expires in 10 days"), "{}", c.detail);
        let c = doctor_check(&[entry("a.example.com", ok.clone(), Some(failing.clone()))], NOW);
        assert_eq!(c.status, "warn", "a failed attempt warns even with time left");
        assert!(c.detail.contains("rateLimited"), "{}", c.detail);
        assert!(c.detail.contains("the current certificate keeps serving"), "{}", c.detail);
        assert!(!c.gates_direct);
        let c = doctor_check(&[entry("a.example.com", soon, Some(uploaded))], NOW);
        assert_eq!(c.status, "info", "uploaded certs are not K2's to renew");
        assert_eq!(c.id, DOCTOR_ID);
    }

    /// (g) Planted but never loaded: the mail cert K2 planted is not the
    /// leaf Stalwart presents → warn with the restart remedy (never the
    /// helper by hand, never disable/enable). Equal fingerprints pass;
    /// an unprobed / unanswered port is left to tls-cert.
    #[test]
    fn planted_but_not_served_mail_cert_warns_with_the_restart_remedy() {
        use crate::domains::status::Served;
        let mail = |served: Option<Served>| DoctorEntry {
            name: "mail.example.com".into(),
            leaf: Some(le(NOW + 80 * DAY)),
            rec: Some(RenewRecord { stalwart_cert_id: Some("c1".into()), ..Default::default() }),
            planted_sha256: Some("aaaa1111bbbb2222cccc".into()),
            served,
            port: 443,
        };
        let c = doctor_check(&[mail(Some(Served::Leaf("ffff0000eeee".into())))], NOW);
        assert_eq!(c.status, "warn", "{}", c.detail);
        assert!(c.detail.contains("mail.example.com"), "{}", c.detail);
        assert!(c.detail.contains("is not what Stalwart serves on :443"), "{}", c.detail);
        assert!(c.detail.contains("hot reload did not land"), "{}", c.detail);
        assert!(c.detail.contains("`sudo systemctl restart stalwart`"), "{}", c.detail);
        assert!(c.detail.contains("never `k2 hostmail disable/enable`"), "{}", c.detail);
        assert!(!c.gates_direct);
        let c = doctor_check(&[mail(Some(Served::Leaf("aaaa1111bbbb2222cccc".into())))], NOW);
        assert_eq!(c.status, "pass", "{}", c.detail);
        for unprobed in [None, Some(Served::Nothing)] {
            let c = doctor_check(&[mail(unprobed)], NOW);
            assert_eq!(c.status, "pass", "{}", c.detail);
        }
    }

    /// The live doctor probes the mail host only, and compares Stalwart's
    /// served leaf with the PEM K2 planted (box store + renewal.json).
    #[test]
    fn live_doctor_compares_the_served_mail_leaf_with_the_planted_one() {
        use crate::domains::status::{pem_leaf_sha256, set_test_served, Served};
        let _home = crate::test_support::TempHome::new();
        let _ = k2_core::db::init_for_tests();
        let host = "mail.served-live.test";
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::upsert_binding(&conn, "served-live.test", None, false).unwrap();
            k2_core::domains::upsert_name(&conn, host, "served-live.test", ROLE_MAIL).unwrap();
        }
        let pem = crate::domains::acme::test_le_pem(host);
        crate::domains::store::install(host, &pem.chain_pem, &pem.key_pem).expect("install");
        update(|f| f.names.entry(host.into()).or_default().stalwart_cert_id = Some("c1".into()))
            .expect("renewal.json");
        let planted = pem_leaf_sha256(&pem.chain_pem).expect("sha");

        set_test_served(host, Some(Served::Leaf("0".repeat(64))));
        let c = doctor_check_live(Some("served-live.test"), Some((host, 443)));
        assert_eq!(c.status, "warn", "{}", c.detail);
        assert!(c.detail.contains("hot reload did not land"), "{}", c.detail);

        set_test_served(host, Some(Served::Leaf(planted)));
        let c = doctor_check_live(Some("served-live.test"), Some((host, 443)));
        assert_eq!(c.status, "pass", "{}", c.detail);
        set_test_served(host, None);

        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = k2_core::domains::remove_binding(&conn, "served-live.test");
    }

    /// (d) A failed renewal never leaves the box without its current
    /// certificate: the scan records lastError and backoff, the doctor
    /// warns at once (not only at 14 days), and nothing is planted.
    #[test]
    fn failed_renewal_keeps_the_current_cert_and_warns_at_once() {
        let _home = crate::test_support::TempHome::new();
        let rows = vec![row("mail.example.com", "mail", Some(binding("example.com", true, Some("active"))))];
        // 25 days left: inside the renewal window, outside the 14-day warning.
        let current = le(NOW + 25 * DAY);
        let mut fake = Fake::new(rows).leaf("mail.example.com", current.clone());
        fake.fail.insert("mail.example.com".into(), IssueError::Order("dns-01: tunnel token rejected".into()));
        scan(&mut fake);
        assert_eq!(fake.leaves["mail.example.com"], current, "the serving certificate is untouched");
        let rec = load().names["mail.example.com"].clone();
        assert_eq!(rec.last_result.as_deref(), Some("failed"));
        assert!(rec.last_error.as_deref().unwrap_or_default().contains("tunnel token rejected"));
        assert!(!rec.plant_pending, "an order failure plants nothing");
        let c = doctor_check(
            &[DoctorEntry { name: "mail.example.com".into(), leaf: Some(current), rec: Some(rec), ..Default::default() }],
            NOW,
        );
        assert_eq!(c.status, "warn", "warns immediately: {}", c.detail);
        assert!(c.detail.contains("tunnel token rejected"), "{}", c.detail);
    }
}
