//! DNS verification for mail domains — K2 Mail S2 (PRD §6.3).
//!
//! Every REQUIRED record in a domain's table resolves and classifies
//! into `Valid | Missing | Wrong` — Wrong (and Missing-with-context)
//! carries the LIVE values so the UI diffs expected-vs-live instead of
//! a bare "missing" (pre-mortem #4). Comparison rules encode the
//! registrar-UX traps:
//!
//! - TXT chunks are JOINED before comparison (long DKIM values are
//!   split into multiple quoted strings — many DNS UIs re-split at
//!   different boundaries, pre-mortem #4/#16); DKIM comparison strips
//!   ALL whitespace (UIs fold long values), other TXT normalizes
//!   whitespace runs + case.
//! - MX comparison tolerates trailing dots and case on the exchange,
//!   and treats a different preference as still-Valid (mail routes).
//!
//! **Domain state machine:** MX + SPF + ≥1 DKIM Valid → **Verified**
//! (DMARC nags, never blocks). A resolver ERROR (timeout etc.) marks
//! records `unknown` and NEVER regresses a domain — only definitive
//! Missing/Wrong answers can flip Verified→error, which raises the
//! standard daemon event ([`k2_core::agent_hooks::HookEvent::
//! MailDomainStatusChanged`], `regressed: true`).
//!
//! **Poller:** [`spawn`] registers the background loop next to the
//! federation-drain/heartbeat-monitor pollers in `main.rs` — same
//! shape (detached tokio task, blocking sweep via `spawn_blocking`).
//! Cadence: pending/error domains re-check every ~5 min (verification
//! "retries for 48 h, never fail-fasts" — pre-mortem #4; in fact it
//! never stops), Verified domains re-verify DAILY (§6.3).
//!
//! DNS is behind the [`DnsResolver`] trait: production =
//! [`SystemResolver`] (hickory, system config); every test uses canned
//! answers — no real DNS lookups in tests, ever.

use tokio::task::JoinHandle;

use super::domains::{
    self, DnsStatus, RecordRow, CAT_REQUIRED, ST_MISSING, ST_UNKNOWN, ST_VALID, ST_WRONG,
};

// ── Resolver seam ───────────────────────────────────────────────────────

/// Lookup failure classes: `NotFound` = a definitive empty answer
/// (NXDOMAIN / no records of that type) → Missing; `Other` = transport
/// trouble (timeout, SERVFAIL, no resolver) → `unknown`, never a
/// regression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsError {
    NotFound,
    Other(String),
}

/// One live MX answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MxHost {
    pub preference: u16,
    pub exchange: String,
}

/// The lookups mail verification needs: MX + TXT for the S2 record
/// table, A + PTR for the S6 doctor (FCrDNS + DNSBL — one resolver
/// abstraction for the whole family, never two). Object-safe on
/// purpose — handlers take `&dyn DnsResolver` so tests inject canned
/// answers.
pub trait DnsResolver: Send + Sync {
    fn mx(&self, name: &str) -> Result<Vec<MxHost>, DnsError>;
    /// TXT answers with their chunk structure preserved (one inner
    /// `Vec<String>` per record — a >255-char value arrives split).
    fn txt(&self, name: &str) -> Result<Vec<Vec<String>>, DnsError>;
    /// A-record answers (S6: forward-confirm rDNS, DNSBL queries).
    fn a(&self, name: &str) -> Result<Vec<std::net::Ipv4Addr>, DnsError>;
    /// PTR names for `ip` (S6: rDNS/FCrDNS).
    fn ptr(&self, ip: std::net::IpAddr) -> Result<Vec<String>, DnsError>;
}

/// Production resolver: hickory (system configuration — the same
/// resolvers the box itself uses, so "valid here" means "propagated to
/// where this server sits"). Blocking; callers run in spawn_blocking.
pub struct SystemResolver {
    inner: hickory_resolver::Resolver,
}

impl SystemResolver {
    pub fn new() -> Result<Self, String> {
        hickory_resolver::Resolver::from_system_conf()
            .map(|inner| Self { inner })
            .map_err(|e| format!("system DNS resolver unavailable: {e}"))
    }
}

/// Absolute query name (trailing dot) so search-domain suffixing can
/// never rewrite what we verify.
fn fqdn(name: &str) -> String {
    if name.ends_with('.') {
        name.to_string()
    } else {
        format!("{name}.")
    }
}

fn map_resolve_err(e: hickory_resolver::error::ResolveError) -> DnsError {
    match e.kind() {
        hickory_resolver::error::ResolveErrorKind::NoRecordsFound { .. } => DnsError::NotFound,
        _ => DnsError::Other(e.to_string()),
    }
}

impl DnsResolver for SystemResolver {
    fn mx(&self, name: &str) -> Result<Vec<MxHost>, DnsError> {
        self.inner
            .mx_lookup(fqdn(name))
            .map(|l| {
                l.iter()
                    .map(|mx| MxHost {
                        preference: mx.preference(),
                        exchange: mx.exchange().to_utf8(),
                    })
                    .collect()
            })
            .map_err(map_resolve_err)
    }

    fn txt(&self, name: &str) -> Result<Vec<Vec<String>>, DnsError> {
        self.inner
            .txt_lookup(fqdn(name))
            .map(|l| {
                l.iter()
                    .map(|txt| {
                        txt.txt_data()
                            .iter()
                            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
                            .collect()
                    })
                    .collect()
            })
            .map_err(map_resolve_err)
    }

    fn a(&self, name: &str) -> Result<Vec<std::net::Ipv4Addr>, DnsError> {
        self.inner
            .ipv4_lookup(fqdn(name))
            .map(|l| l.iter().map(|a| a.0).collect())
            .map_err(map_resolve_err)
    }

    fn ptr(&self, ip: std::net::IpAddr) -> Result<Vec<String>, DnsError> {
        self.inner
            .reverse_lookup(ip)
            .map(|l| {
                l.iter()
                    .map(|ptr| ptr.0.to_utf8().trim_end_matches('.').to_string())
                    .collect()
            })
            .map_err(map_resolve_err)
    }
}

// ── Comparison rules (pre-mortem #4/#16) ────────────────────────────────

/// RFC-1035 TXT semantics: a record's character-strings concatenate
/// DIRECTLY (no separator) into the logical value.
pub fn join_chunks(chunks: &[String]) -> String {
    chunks.concat()
}

/// General TXT normalization: collapse whitespace runs, trim,
/// lowercase (SPF mechanisms + DMARC tags are case-insensitive).
pub fn norm_txt(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase()
}

/// DKIM normalization: strip ALL whitespace (registrar UIs fold long
/// values and re-split at arbitrary boundaries) but KEEP case — `p=`
/// is base64.
pub fn norm_dkim(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Hostname equality tolerant of the trailing-dot spelling + case.
pub fn host_eq(a: &str, b: &str) -> bool {
    a.trim_end_matches('.').eq_ignore_ascii_case(b.trim_end_matches('.'))
}

/// Parse an MX expected value (`"10 mail.acme.dev."`) into
/// (preference, exchange). A bare exchange with no preference parses
/// too (defensive).
fn parse_mx_expected(expected: &str) -> (Option<u16>, String) {
    let mut parts = expected.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some(p), Some(host)) => (p.parse().ok(), host.to_string()),
        (Some(host), None) => (None, host.to_string()),
        _ => (None, String::new()),
    }
}

// ── Per-record verification ─────────────────────────────────────────────

/// What one verification pass concluded (drives the domain flip).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CheckSummary {
    pub mx_valid: bool,
    pub spf_valid: bool,
    /// ≥1 DKIM selector Valid (§6.3 — Stalwart publishes two).
    pub dkim_valid: bool,
    pub dmarc_valid: bool,
    /// Any required lookup errored (transient) — blocks regression.
    pub any_unknown: bool,
}

/// Which TXT records at a name are candidates for a row.
#[derive(Clone, Copy)]
enum TxtKind {
    Spf,
    Dkim,
    Dmarc,
}

fn set_state(row: &mut RecordRow, status: &str, live: Option<Vec<String>>, now: i64) {
    row.status = status.to_string();
    // `unknown` keeps the previous live snapshot (the last definitive
    // answer is more useful than erasing it on a timeout).
    if status != ST_UNKNOWN {
        row.live = live;
    }
    row.checked_at = Some(now);
}

fn check_mx(resolver: &dyn DnsResolver, domain: &str, row: &mut RecordRow, now: i64) -> Result<bool, ()> {
    let (_, expected_host) = parse_mx_expected(&row.expected);
    match resolver.mx(domain) {
        Ok(hosts) if hosts.is_empty() => {
            set_state(row, ST_MISSING, None, now);
            Ok(false)
        }
        Ok(hosts) => {
            let live: Vec<String> = hosts
                .iter()
                .map(|h| format!("{} {}", h.preference, h.exchange.trim_end_matches('.')))
                .collect();
            // The exchange is what routes mail — a different preference
            // still delivers, so it stays Valid (pre-mortem #4: MX
            // comparison tolerates trailing dots; case-insensitive).
            if hosts.iter().any(|h| host_eq(&h.exchange, &expected_host)) {
                set_state(row, ST_VALID, Some(live), now);
                Ok(true)
            } else {
                set_state(row, ST_WRONG, Some(live), now);
                Ok(false)
            }
        }
        Err(DnsError::NotFound) => {
            set_state(row, ST_MISSING, None, now);
            Ok(false)
        }
        Err(DnsError::Other(_)) => {
            set_state(row, ST_UNKNOWN, None, now);
            Err(())
        }
    }
}

fn check_txt(
    resolver: &dyn DnsResolver,
    kind: TxtKind,
    row: &mut RecordRow,
    now: i64,
) -> Result<bool, ()> {
    match resolver.txt(&row.name) {
        Ok(records) => {
            let joined: Vec<String> = records.iter().map(|r| join_chunks(r)).collect();
            // Candidate filter: SPF shares the apex with unrelated TXT
            // records (site verifications etc.) — only v=spf1 records
            // are the diff set. DKIM/DMARC live at dedicated names, so
            // everything found there is relevant.
            let candidates: Vec<&String> = match kind {
                TxtKind::Spf => joined
                    .iter()
                    .filter(|s| s.trim_start().to_ascii_lowercase().starts_with("v=spf1"))
                    .collect(),
                TxtKind::Dkim | TxtKind::Dmarc => joined.iter().collect(),
            };
            let matches = |live: &str| match kind {
                TxtKind::Dkim => norm_dkim(live) == norm_dkim(&row.expected),
                _ => norm_txt(live) == norm_txt(&row.expected),
            };
            if let Some(hit) = candidates.iter().find(|c| matches(c)) {
                set_state(row, ST_VALID, Some(vec![(*hit).clone()]), now);
                return Ok(true);
            }
            if !candidates.is_empty() {
                let live = candidates.into_iter().cloned().collect();
                set_state(row, ST_WRONG, Some(live), now);
                return Ok(false);
            }
            // No candidate — Missing, but when OTHER records exist at
            // the name, surface them as context (pre-mortem #4: show
            // what IS there; catches "pasted the name into the value").
            let context = if joined.is_empty() { None } else { Some(joined) };
            set_state(row, ST_MISSING, context, now);
            Ok(false)
        }
        Err(DnsError::NotFound) => {
            set_state(row, ST_MISSING, None, now);
            Ok(false)
        }
        Err(DnsError::Other(_)) => {
            set_state(row, ST_UNKNOWN, None, now);
            Err(())
        }
    }
}

/// Verify every REQUIRED row in place (instruction + advanced rows are
/// never resolved — PTR is unverifiable from DNS, advanced rows are
/// not gated, §6.5). Returns the summary that drives the domain flip.
pub fn verify_rows(
    resolver: &dyn DnsResolver,
    domain: &str,
    rows: &mut [RecordRow],
    now: i64,
) -> CheckSummary {
    let mut summary = CheckSummary::default();
    for row in rows.iter_mut() {
        if row.category != CAT_REQUIRED {
            continue;
        }
        let outcome = if row.id == "mx" {
            check_mx(resolver, domain, row, now)
        } else if row.id == "spf" {
            check_txt(resolver, TxtKind::Spf, row, now)
        } else if row.id.starts_with("dkim:") {
            check_txt(resolver, TxtKind::Dkim, row, now)
        } else if row.id == "dmarc" {
            check_txt(resolver, TxtKind::Dmarc, row, now)
        } else {
            continue;
        };
        match outcome {
            Ok(valid) => {
                if valid {
                    match row.id.as_str() {
                        "mx" => summary.mx_valid = true,
                        "spf" => summary.spf_valid = true,
                        "dmarc" => summary.dmarc_valid = true,
                        _ => summary.dkim_valid = true, // dkim:*
                    }
                }
            }
            Err(()) => summary.any_unknown = true,
        }
    }
    summary
}

/// The domain status transition (pure — the regression rules live
/// here): Verified needs MX + SPF + ≥1 DKIM (§6.3, DMARC never
/// blocks); a pass with ANY unknown lookup can never regress; a
/// definitive non-pass flips Verified→error (`regressed = true` — the
/// event that must notify).
pub fn next_status(old: &str, s: &CheckSummary) -> (String, bool) {
    let required_ok = s.mx_valid && s.spf_valid && s.dkim_valid;
    if required_ok {
        return ("verified".to_string(), false);
    }
    if s.any_unknown {
        return (old.to_string(), false);
    }
    if old == "verified" {
        return ("error".to_string(), true);
    }
    (old.to_string(), false)
}

// ── Domain-level check (route `domain/check` + the poller) ─────────────

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Force-verify one domain NOW: resolve every required record,
/// persist the updated table + status, emit the transition event, and
/// return the full `show`-shaped JSON. DNS runs with NO db lock held.
pub fn check_domain_now(
    resolver: &dyn DnsResolver,
    raw_domain: &str,
) -> Result<serde_json::Value, domains::OpError> {
    let domain =
        k2_core::mail_domain::normalize_mail_domain(raw_domain).map_err(domains::OpError::Usage)?;

    // Snapshot under the lock…
    let (row, mut rows, zone_file) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(row) = domains::load_domain(&conn, &domain) else {
            return Err(domains::OpError::NotFound(format!(
                "domain '{domain}' is not hosted here"
            )));
        };
        let rows = domains::effective_rows(&conn, &row);
        let zone_file = domains::dns_status_of(&row).zone_file;
        (row, rows, zone_file)
    };

    // …resolve with the lock RELEASED (network under a shared DB mutex
    // would stall every route)…
    let now = now_secs();
    let summary = verify_rows(resolver, &domain, &mut rows, now);
    let (new_status, regressed) = next_status(&row.status, &summary);
    let verified_at = if new_status == "verified" {
        row.verified_at.or(Some(now))
    } else {
        row.verified_at
    };

    // …persist + re-read under the lock.
    let (json, changed) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let status = DnsStatus { records: rows, zone_file };
        domains::save_check(&conn, &row.id, &status, &new_status, verified_at, now)
            .map_err(domains::OpError::Engine)?;
        let fresh = domains::load_domain(&conn, &domain).ok_or_else(|| {
            domains::OpError::Engine("domain row vanished mid-check".to_string())
        })?;
        (domains::domain_json(&conn, &fresh), new_status != row.status)
    };

    if changed {
        k2_core::agent_hooks::emit(
            k2_core::agent_hooks::HookEvent::MailDomainStatusChanged,
            serde_json::json!({
                "domain": domain,
                "status": new_status,
                "regressed": regressed,
            }),
        );
        k2_core::log_debug!(
            "[mail/dns] domain {domain} → {new_status}{}",
            if regressed { " (REGRESSION)" } else { "" }
        );
    }
    Ok(json)
}

// ── Background poller ───────────────────────────────────────────────────

/// Sweep tick. Cheap when nothing is due (one indexed SELECT).
const DEFAULT_TICK_SECS: u64 = 60;
/// Unverified (pending/error) domains re-check on this cadence —
/// propagation-friendly: keeps retrying indefinitely, never fail-fasts
/// (pre-mortem #4).
const UNVERIFIED_RECHECK_SECS: i64 = 300;
/// Verified domains re-verify daily (§6.3).
const VERIFIED_RECHECK_SECS: i64 = 86_400;

fn tick_interval() -> std::time::Duration {
    let secs = std::env::var("K2_MAIL_DNS_TICK_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_TICK_SECS);
    std::time::Duration::from_secs(secs)
}

/// Pure due-schedule: never-checked domains are always due; then
/// per-status cadence.
pub fn is_due(status: &str, last_checked_at: Option<i64>, now: i64) -> bool {
    let Some(last) = last_checked_at else { return true };
    let interval = if status == "verified" {
        VERIFIED_RECHECK_SECS
    } else {
        UNVERIFIED_RECHECK_SECS
    };
    now - last >= interval
}

/// One blocking sweep: check every due domain. The resolver is built
/// once per sweep (system conf can change under us — cheap to reload).
fn sweep_once() {
    let due: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let now = now_secs();
        domains::load_all_domains(&conn)
            .into_iter()
            .filter(|d| is_due(&d.status, d.last_checked_at, now))
            .map(|d| d.domain)
            .collect()
    };
    if due.is_empty() {
        return;
    }
    let resolver = match SystemResolver::new() {
        Ok(r) => r,
        Err(e) => {
            k2_core::log_debug!("[mail/dns] sweep skipped — {e}");
            return;
        }
    };
    for domain in due {
        if let Err(e) = check_domain_now(&resolver, &domain) {
            k2_core::log_debug!("[mail/dns] check {domain} failed: {e:?}");
        }
    }
}

/// Register the verification poller (main.rs, next to the other
/// background loops — federation drain / heartbeat monitor pattern).
pub fn spawn() -> JoinHandle<()> {
    tokio::spawn(async move {
        let tick = tick_interval();
        k2_core::log_debug!("[mail/dns] poller started — tick={}s", tick.as_secs());
        loop {
            // Blocking DNS + SQLite — keep it off the async reactor.
            let _ = tokio::task::spawn_blocking(sweep_once).await;
            tokio::time::sleep(tick).await;
        }
    })
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — canned resolver ONLY (no real DNS, house rule)
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::domains::tests::{fixture_rows, FakeEngine, ZONE_FIXTURE};
    use crate::mail::domains::{add_domain, load_domain};
    use std::collections::HashMap;

    /// Canned resolver: per-name answers; anything unlisted is
    /// NotFound; names in `broken` answer a transport error.
    #[derive(Default)]
    struct FakeResolver {
        mx: HashMap<String, Vec<MxHost>>,
        txt: HashMap<String, Vec<Vec<String>>>,
        broken: Vec<String>,
    }

    impl DnsResolver for FakeResolver {
        fn mx(&self, name: &str) -> Result<Vec<MxHost>, DnsError> {
            if self.broken.iter().any(|b| b == name) {
                return Err(DnsError::Other("timeout".to_string()));
            }
            self.mx.get(name).cloned().ok_or(DnsError::NotFound)
        }
        fn txt(&self, name: &str) -> Result<Vec<Vec<String>>, DnsError> {
            if self.broken.iter().any(|b| b == name) {
                return Err(DnsError::Other("timeout".to_string()));
            }
            self.txt.get(name).cloned().ok_or(DnsError::NotFound)
        }
        fn a(&self, _name: &str) -> Result<Vec<std::net::Ipv4Addr>, DnsError> {
            // S2 verification never issues A lookups — the doctor's
            // own fakes cover them.
            Err(DnsError::NotFound)
        }
        fn ptr(&self, _ip: std::net::IpAddr) -> Result<Vec<String>, DnsError> {
            Err(DnsError::NotFound)
        }
    }

    /// A resolver serving the fixture domain fully + correctly, with
    /// the RSA DKIM re-split at a DIFFERENT boundary than the zone
    /// file (the registrar-re-chunking trap, pre-mortem #4).
    fn all_valid_resolver(domain: &str) -> FakeResolver {
        let rows = fixture_rows();
        let expect = |id: &str| {
            rows.iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("fixture row {id}"))
                .expected
                .clone()
        };
        let mut r = FakeResolver::default();
        r.mx.insert(
            domain.to_string(),
            vec![MxHost { preference: 10, exchange: "MAIL.ACME.DEV.".to_string() }],
        );
        r.txt.insert(domain.to_string(), vec![vec![expect("spf")]]);
        r.txt.insert(
            format!("202601e._domainkey.{domain}"),
            vec![vec![expect("dkim:202601e")]],
        );
        // Re-chunk the long RSA value at byte 100 — a different split
        // than Stalwart's. JOIN-then-compare must still match.
        let rsa = expect("dkim:202601r");
        let (a, b) = rsa.split_at(100);
        r.txt.insert(
            format!("202601r._domainkey.{domain}"),
            vec![vec![a.to_string(), b.to_string()]],
        );
        r.txt.insert(
            format!("_dmarc.{domain}"),
            vec![vec!["v=DMARC1; p=none; rua=mailto:postmaster@acme.dev".to_string()]],
        );
        r
    }

    // ── Comparison rules ──

    #[test]
    fn txt_normalization_joins_chunks_and_folds_whitespace() {
        assert_eq!(join_chunks(&["v=spf1 ".to_string(), "mx -all".to_string()]), "v=spf1 mx -all");
        assert_eq!(norm_txt("  V=SPF1   mx\t-ALL "), "v=spf1 mx -all");
        // DKIM: whitespace stripped, CASE KEPT (p= is base64).
        assert_eq!(norm_dkim("v=DKIM1; k=rsa;\n p=AbC dEf"), "v=DKIM1;k=rsa;p=AbCdEf");
        assert_ne!(norm_dkim("p=abc"), norm_dkim("p=ABC"), "base64 case matters");
    }

    #[test]
    fn mx_host_equality_tolerates_trailing_dots_and_case() {
        assert!(host_eq("mail.acme.dev.", "MAIL.acme.DEV"));
        assert!(host_eq("mail.acme.dev", "mail.acme.dev."));
        assert!(!host_eq("mail.acme.dev", "mail2.acme.dev"));
        assert_eq!(parse_mx_expected("10 mail.acme.dev."), (Some(10), "mail.acme.dev.".to_string()));
        assert_eq!(parse_mx_expected("mail.acme.dev"), (None, "mail.acme.dev".to_string()));
    }

    // ── verify_rows classification ──

    #[test]
    fn all_correct_records_verify_including_resplit_dkim() {
        let mut rows = fixture_rows();
        let resolver = all_valid_resolver("acme.dev");
        let s = verify_rows(&resolver, "acme.dev", &mut rows, 1000);
        assert!(s.mx_valid && s.spf_valid && s.dkim_valid && s.dmarc_valid, "{s:?}");
        assert!(!s.any_unknown);
        for id in ["mx", "spf", "dkim:202601e", "dkim:202601r", "dmarc"] {
            let row = rows.iter().find(|r| r.id == id).unwrap();
            assert_eq!(row.status, ST_VALID, "{id}");
            assert_eq!(row.checked_at, Some(1000));
            assert!(row.live.is_some(), "{id} carries the live value");
        }
        // Instruction + advanced rows were never touched.
        assert!(rows.iter().any(|r| r.id == "ptr" && r.checked_at.is_none()));
        assert!(rows
            .iter()
            .filter(|r| r.category == crate::mail::domains::CAT_ADVANCED)
            .all(|r| r.checked_at.is_none()));
    }

    #[test]
    fn wrong_records_carry_the_expected_vs_live_diff() {
        let mut rows = fixture_rows();
        let mut resolver = all_valid_resolver("acme.dev");
        // MX points at the WRONG host (the auto-appended-zone classic:
        // mail.acme.dev.acme.dev), SPF has a stray include.
        resolver.mx.insert(
            "acme.dev".to_string(),
            vec![MxHost { preference: 10, exchange: "mail.acme.dev.acme.dev.".to_string() }],
        );
        resolver
            .txt
            .insert("acme.dev".to_string(), vec![vec!["v=spf1 include:old.example -all".to_string()]]);
        let s = verify_rows(&resolver, "acme.dev", &mut rows, 1000);
        assert!(!s.mx_valid && !s.spf_valid, "{s:?}");
        assert!(s.dkim_valid, "DKIM untouched and still valid");

        let mx = rows.iter().find(|r| r.id == "mx").unwrap();
        assert_eq!(mx.status, ST_WRONG);
        assert_eq!(
            mx.live.as_deref(),
            Some(&["10 mail.acme.dev.acme.dev".to_string()][..]),
            "live shows what IS there — never a bare mismatch"
        );
        let spf = rows.iter().find(|r| r.id == "spf").unwrap();
        assert_eq!(spf.status, ST_WRONG);
        assert_eq!(spf.live.as_deref(), Some(&["v=spf1 include:old.example -all".to_string()][..]));
    }

    #[test]
    fn missing_spf_with_unrelated_apex_txt_shows_context_not_wrong() {
        let mut rows = fixture_rows();
        let mut resolver = all_valid_resolver("acme.dev");
        // Apex TXT exists but none of it is SPF (site verifications) —
        // that's Missing (not Wrong), with the found records as
        // context (pre-mortem #4).
        resolver.txt.insert(
            "acme.dev".to_string(),
            vec![vec!["google-site-verification=abc123".to_string()]],
        );
        let s = verify_rows(&resolver, "acme.dev", &mut rows, 1000);
        assert!(!s.spf_valid);
        let spf = rows.iter().find(|r| r.id == "spf").unwrap();
        assert_eq!(spf.status, ST_MISSING);
        assert_eq!(
            spf.live.as_deref(),
            Some(&["google-site-verification=abc123".to_string()][..])
        );
        // Truly-absent name → Missing with no live noise.
        let mut resolver = all_valid_resolver("acme.dev");
        resolver.txt.remove("_dmarc.acme.dev");
        let _ = verify_rows(&resolver, "acme.dev", &mut rows, 1000);
        let dmarc = rows.iter().find(|r| r.id == "dmarc").unwrap();
        assert_eq!(dmarc.status, ST_MISSING);
        assert_eq!(dmarc.live, None);
    }

    #[test]
    fn tolerant_matching_mx_preference_and_txt_whitespace() {
        let mut rows = fixture_rows();
        let mut resolver = all_valid_resolver("acme.dev");
        // Preference drifted (20 vs 10) — exchange right → still Valid.
        resolver.mx.insert(
            "acme.dev".to_string(),
            vec![MxHost { preference: 20, exchange: "mail.acme.dev".to_string() }],
        );
        // SPF with folded whitespace + case drift → still Valid.
        resolver
            .txt
            .insert("acme.dev".to_string(), vec![vec!["V=spf1   MX  -all".to_string()]]);
        let s = verify_rows(&resolver, "acme.dev", &mut rows, 1000);
        assert!(s.mx_valid, "exchange match wins over preference drift");
        assert!(s.spf_valid, "whitespace/case-normalized TXT compare");
    }

    #[test]
    fn resolver_trouble_marks_unknown_and_sets_any_unknown() {
        let mut rows = fixture_rows();
        let mut resolver = all_valid_resolver("acme.dev");
        resolver.broken.push("acme.dev".to_string()); // MX + SPF lookups
        let s = verify_rows(&resolver, "acme.dev", &mut rows, 1000);
        assert!(s.any_unknown);
        assert!(s.dkim_valid, "unbroken names still verify");
        let mx = rows.iter().find(|r| r.id == "mx").unwrap();
        assert_eq!(mx.status, ST_UNKNOWN);
    }

    // ── Domain state machine ──

    #[test]
    fn next_status_transitions_and_regression_rules() {
        let ok = CheckSummary {
            mx_valid: true,
            spf_valid: true,
            dkim_valid: true,
            dmarc_valid: false, // DMARC never blocks (§6.3)
            any_unknown: false,
        };
        assert_eq!(next_status("pending", &ok), ("verified".to_string(), false));
        assert_eq!(next_status("error", &ok), ("verified".to_string(), false));
        assert_eq!(next_status("verified", &ok), ("verified".to_string(), false));

        let broken = CheckSummary { mx_valid: true, spf_valid: false, dkim_valid: true, ..ok.clone() };
        assert_eq!(next_status("pending", &broken), ("pending".to_string(), false));
        assert_eq!(
            next_status("verified", &broken),
            ("error".to_string(), true),
            "Verified→broken is THE regression"
        );
        assert_eq!(next_status("error", &broken), ("error".to_string(), false));

        // Transient resolver trouble NEVER regresses.
        let unknown = CheckSummary { any_unknown: true, ..broken };
        assert_eq!(next_status("verified", &unknown), ("verified".to_string(), false));
    }

    #[test]
    fn due_schedule_pending_5min_verified_daily() {
        assert!(is_due("pending", None, 0), "never-checked is always due");
        assert!(!is_due("pending", Some(1000), 1000 + 299));
        assert!(is_due("pending", Some(1000), 1000 + 300));
        assert!(is_due("error", Some(1000), 1000 + 300), "regressed re-checks like pending");
        assert!(!is_due("verified", Some(1000), 1000 + 3600), "verified is NOT hourly");
        assert!(is_due("verified", Some(1000), 1000 + 86_400), "verified re-verifies daily");
    }

    // ── End-to-end against the shared test DB (canned resolver) ──

    #[test]
    fn check_domain_now_persists_verified_then_regression() {
        let domain = "e2e-dns.example";
        let zone = ZONE_FIXTURE.replace("acme.dev", domain);
        // Clean slate, then seed via the real add path (fake engine).
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_domains WHERE domain = ?1", rusqlite::params![domain]);
        }
        let engine = FakeEngine { zone, ..FakeEngine::ok() };
        add_domain(&engine, domain).expect("seed add");

        // Pass 1: everything correct → Verified, verified_at stamped.
        let mut resolver = all_valid_resolver(domain);
        // all_valid_resolver keys some names off the literal fixture
        // domain — rebuild them for this domain.
        let rekey: Vec<(String, Vec<Vec<String>>)> = resolver
            .txt
            .iter()
            .map(|(k, v)| (k.replace("acme.dev", domain), v.clone()))
            .collect();
        resolver.txt = rekey.into_iter().collect();
        // The MX answer must match the REPLACED zone's exchange
        // (mail.<domain>), not the fixture's mail.acme.dev.
        resolver.mx.insert(
            domain.to_string(),
            vec![MxHost { preference: 10, exchange: format!("mail.{domain}.") }],
        );
        // The fixture DKIM/SPF expected values are domain-independent
        // strings; the DMARC rua mentions acme.dev but norm-compare is
        // against the row's own expected (also acme.dev-based since
        // the zone was string-replaced) — fix the DMARC answer to the
        // replaced row's value.
        let expected_dmarc: String = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let row = load_domain(&conn, domain).unwrap();
            crate::mail::domains::effective_rows(&conn, &row)
                .iter()
                .find(|r| r.id == "dmarc")
                .unwrap()
                .expected
                .clone()
        };
        resolver
            .txt
            .insert(format!("_dmarc.{domain}"), vec![vec![expected_dmarc]]);

        let v = check_domain_now(&resolver, domain).expect("check ok");
        assert_eq!(v["ok"], true);
        assert_eq!(v["status"], "verified");
        assert_eq!(v["dmarcNag"], false);
        let verified_at = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let row = load_domain(&conn, domain).unwrap();
            assert_eq!(row.status, "verified");
            assert!(row.last_checked_at.is_some());
            row.verified_at.expect("verified_at stamped")
        };

        // Pass 2: the SPF record breaks at the registrar → regression
        // to `error`, per-record Wrong diff persisted, verified_at
        // KEPT as history.
        resolver.txt.insert(
            domain.to_string(),
            vec![vec!["v=spf1 include:someoneelse.example ~all".to_string()]],
        );
        let v = check_domain_now(&resolver, domain).expect("recheck ok");
        assert_eq!(v["status"], "error");
        let rec = v["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "spf")
            .unwrap();
        assert_eq!(rec["status"], "wrong");
        assert_eq!(rec["live"][0], "v=spf1 include:someoneelse.example ~all");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let row = load_domain(&conn, domain).unwrap();
            assert_eq!(row.status, "error");
            assert_eq!(row.verified_at, Some(verified_at), "history kept");
            let _ = conn.execute("DELETE FROM mail_domains WHERE domain = ?1", rusqlite::params![domain]);
        }

        // Unknown domain → NotFound.
        match check_domain_now(&resolver, "nope.example") {
            Err(domains::OpError::NotFound(_)) => {}
            other => panic!("must be NotFound, got {other:?}"),
        }
    }
}
