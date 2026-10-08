//! CAL44 — one non-default certificate per extra mail-family name
//! (prd-hostmail-calendars-v1 §12, vs-live §8 CAL44–CAL46; replaces
//! CAL36 / IT1).
//!
//! The extra names are exactly `autoconfig`, `autodiscover`, `mta-sts`
//! and `ua-auto-config` under each hosted apex — never the apex, never
//! `www`, never anything else. Stalwart already serves those endpoints
//! on any Host; only a certificate for each name is missing (strict TLS
//! fails because they get the mail host's certificate).
//!
//! Rules this module keeps:
//! - **One certificate per name.** Each name gets its own order from
//!   K2's issuer ([`crate::domains::acme::issue_extra_name`], DNS-01 on a
//!   K2-hosted zone). Never a SAN on the mail certificate, never
//!   Stalwart's ACME (one failing name fails a whole Stalwart order —
//!   the noir incident behind C24).
//! - **DNS-gated, every run.** A name is only issued when its A records
//!   are all this box (the public IPv4, else the mail host's A set) and
//!   any AAAA it has are the mail host's. Checked again on every
//!   issue/renew run.
//! - **Non-default install.** `x:Certificate/set` create only
//!   ([`StalwartClient::certificate_add`]); `defaultCertificateId` stays
//!   on the mail host's certificate, so clients that send no SNI name
//!   still get the mail certificate. Refused outright when Stalwart has
//!   no live default certificate (it would then serve an arbitrary one).
//! - **Hot reload first.** `x:Action/set ReloadTlsCertificates` (S0.10:
//!   re-reads every Certificate into the SNI map, 0.16.10 and 0.16.20
//!   alike), then — only if that fails — the existing
//!   `restart_stalwart_to_reload_tls` path. No third restart path, never
//!   hostmail disable/enable.
//! - **Isolation.** A failure on one name is recorded on that name and
//!   the run moves on. The mail certificate is never read, replaced or
//!   re-pointed here.
//! - **DNS moved → not renewed.** A name that stops pointing at the box
//!   is skipped; the certificate already in Stalwart is left to expire
//!   (Stalwart deletes expired certificates itself). Nothing is
//!   destroyed on a DNS answer, so a transient or wrong lookup can never
//!   take a working name's certificate away.
//!
//! Renewal: the daemon's background renewer ([`crate::domains::renew`])
//! re-runs this same run for names already in the state file whose
//! certificate is inside [`RENEW_BEFORE_SECS`] of expiry (or never got
//! loaded), with per-name backoff, under the same per-name lock as
//! `k2 hostmail cert names renew`. A renewed certificate supersedes the
//! name's previous one, which is destroyed once the new one is loaded.
//!
//! State: `~/.k2/certs/extra-names.json` (0600) next to the PEM store.

use std::collections::{BTreeMap, HashMap};
use std::net::{Ipv4Addr, Ipv6Addr};

use k2_core::domains::DomainBinding;

use crate::cli_response::CliResponse;
use crate::mail::dns_verify::{DnsError, DnsResolver};
use crate::mail::jmap::DefaultCertificate;

/// The only extra names (IT1). Never the apex, never `www`.
pub const EXTRA_LABELS: [&str; 4] = ["autoconfig", "autodiscover", "mta-sts", "ua-auto-config"];

/// A per-name certificate inside this window of expiry is re-issued.
pub const RENEW_BEFORE_SECS: i64 = crate::domains::renew::RENEW_WINDOW_SECS;

/// The four extra names under `apex`. Empty for a `*.k2.dev` apex
/// (Connect names stay on cert.k2.dev). A name equal to the mail host
/// is dropped (the mail certificate already covers it, and it must stay
/// the default).
pub fn extra_names_for_apex(apex: &str, mail_host: Option<&str>) -> Vec<String> {
    let apex = norm(apex);
    if apex.is_empty() || !apex.contains('.') || apex == "k2.dev" || apex.ends_with(".k2.dev") {
        return Vec::new();
    }
    let mail = mail_host.map(norm);
    EXTRA_LABELS
        .iter()
        .map(|l| format!("{l}.{apex}"))
        .filter(|n| Some(n) != mail.as_ref())
        .collect()
}

fn norm(s: &str) -> String {
    s.trim().trim_end_matches('.').to_ascii_lowercase()
}

// ── DNS gate ────────────────────────────────────────────────────────────

/// What "this box" is for the DNS gate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoxAddrs {
    pub v4: Vec<Ipv4Addr>,
    pub v6: Vec<Ipv6Addr>,
    /// `public-ip` (discovered, like the doctor's PTR check) or
    /// `mail-host` (the mail host's A records, CAL44's fallback).
    pub source: &'static str,
}

impl BoxAddrs {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "v4": self.v4.iter().map(|ip| ip.to_string()).collect::<Vec<_>>(),
            "v6": self.v6.iter().map(|ip| ip.to_string()).collect::<Vec<_>>(),
            "source": self.source,
        })
    }
}

/// The box's addresses: the discovered public IPv4 when there is one,
/// else the mail host's A records; IPv6 = the mail host's AAAA records
/// (K2 has no IPv6 discovery).
pub fn box_addrs(public_ip: Option<&str>, resolver: &dyn DnsResolver, mail_host: &str) -> BoxAddrs {
    let v6 = resolver.aaaa(mail_host).unwrap_or_default();
    if let Some(ip) = public_ip.and_then(|s| s.trim().parse::<Ipv4Addr>().ok()) {
        return BoxAddrs { v4: vec![ip], v6, source: "public-ip" };
    }
    BoxAddrs {
        v4: resolver.a(mail_host).unwrap_or_default(),
        v6,
        source: "mail-host",
    }
}

/// Does a name resolve to this box?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointsHere {
    Yes,
    /// A definitive answer that it does not (missing A, or an A/AAAA
    /// that is another host).
    No(String),
    /// The lookup itself failed — never treated as "moved".
    Unknown(String),
}

impl PointsHere {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            PointsHere::Yes => Some(true),
            PointsHere::No(_) => Some(false),
            PointsHere::Unknown(_) => None,
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            PointsHere::Yes => None,
            PointsHere::No(d) | PointsHere::Unknown(d) => Some(d.as_str()),
        }
    }
}

/// The CAL44 gate. Every A answer must be one of `addrs.v4` (and there
/// must be at least one); every AAAA answer must be one of `addrs.v6`
/// (a foreign AAAA sends IPv6 clients to another host).
pub fn points_here(resolver: &dyn DnsResolver, name: &str, addrs: &BoxAddrs) -> PointsHere {
    if addrs.v4.is_empty() {
        return PointsHere::Unknown(
            "this box's public address is unknown (no public IP discovered and the mail \
             host has no A record)"
                .into(),
        );
    }
    let a = match resolver.a(name) {
        Ok(a) => a,
        Err(DnsError::NotFound) => Vec::new(),
        Err(DnsError::Other(e)) => return PointsHere::Unknown(format!("A lookup for {name}: {e}")),
    };
    if a.is_empty() {
        return PointsHere::No(format!("{name} has no A record"));
    }
    if let Some(foreign) = a.iter().find(|ip| !addrs.v4.contains(ip)) {
        return PointsHere::No(format!(
            "{name} A {foreign} is not this box ({})",
            join_ips(&addrs.v4)
        ));
    }
    match resolver.aaaa(name) {
        Ok(v6) => {
            if let Some(foreign) = v6.iter().find(|ip| !addrs.v6.contains(ip)) {
                return PointsHere::No(format!(
                    "{name} AAAA {foreign} is not this box — IPv6 clients would reach \
                     another host"
                ));
            }
        }
        Err(DnsError::NotFound) => {}
        Err(DnsError::Other(e)) => {
            return PointsHere::Unknown(format!("AAAA lookup for {name}: {e}"))
        }
    }
    PointsHere::Yes
}

fn join_ips(ips: &[Ipv4Addr]) -> String {
    ips.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
}

// ── State (~/.k2/certs/extra-names.json) ────────────────────────────────

/// What K2 last did for one extra name.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NameRecord {
    /// The Stalwart Certificate id of the last planted certificate.
    pub stalwart_cert_id: Option<String>,
    /// That certificate's notAfter (matches the PEM on disk while it is
    /// the one in Stalwart).
    pub planted_not_after: Option<i64>,
    pub planted_at: Option<i64>,
    /// Stalwart reloaded (action or restart) after that plant.
    pub loaded: bool,
    pub points_here: Option<bool>,
    pub dns_detail: Option<String>,
    pub checked_at: Option<i64>,
    pub last_result: Option<String>,
    pub last_error: Option<String>,
    /// Earlier Certificates planted for this name, destroyed once the
    /// current one is loaded (renewal supersedes; an id already gone is Ok).
    pub stale_cert_ids: Vec<String>,
}

pub type State = BTreeMap<String, NameRecord>;

pub fn state_path() -> std::path::PathBuf {
    crate::domains::store::certs_root().join("extra-names.json")
}

pub fn load_state() -> State {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_state(state: &State) -> Result<(), String> {
    let dir = crate::domains::store::certs_root();
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let body = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    crate::domains::store::write_tmp_rename(&state_path(), body.as_bytes(), 0o600)
}

// ── The run (pure over the seams) ───────────────────────────────────────

/// A certificate in the box store for one name.
#[derive(Debug, Clone)]
pub struct Inventory {
    pub chain_pem: String,
    pub key_pem: String,
    pub not_after: Option<i64>,
    /// A live Let's Encrypt leaf for exactly this name (not Fake CA, not
    /// rcgen, not expired).
    pub reusable: bool,
}

/// Everything the run does that is not DNS. Production = [`LiveDeps`];
/// tests inject recording fakes.
pub trait Deps {
    fn now(&self) -> i64;
    fn inventory(&self, name: &str) -> Option<Inventory>;
    /// Order a certificate for exactly `name` (and write it to the box
    /// store). Never plants.
    fn issue(&mut self, name: &str, binding: &DomainBinding) -> Result<Inventory, String>;
    fn default_certificate(&mut self) -> Result<DefaultCertificate, String>;
    /// `x:Certificate/set` create WITHOUT `defaultCertificateId`.
    fn add_certificate(&mut self, chain_pem: &str, key_pem: &str) -> Result<String, String>;
    /// `x:Action/set ReloadTlsCertificates`.
    fn reload_tls(&mut self) -> Result<(), String>;
    /// The existing restart path (`restart_stalwart_to_reload_tls`).
    fn restart_stalwart(&mut self) -> Result<(), String>;
    /// `x:Certificate/set` destroy of a superseded per-name certificate.
    fn remove_certificate(&mut self, id: &str) -> Result<(), String>;
}

/// One hosted apex and its extra names.
#[derive(Debug, Clone)]
pub struct ApexPlan {
    pub apex: String,
    /// `domain_bindings` row for the apex (`None` = not attached).
    pub binding: Option<DomainBinding>,
    pub names: Vec<String>,
    pub addrs: BoxAddrs,
}

pub const R_CURRENT: &str = "current";
pub const R_ISSUED: &str = "issued";
pub const R_PLANTED: &str = "planted";
pub const R_SKIPPED_DNS: &str = "skipped-dns";
pub const R_SKIPPED_DNS_UNKNOWN: &str = "skipped-dns-unknown";
pub const R_UNSUPPORTED: &str = "unsupported";
pub const R_BLOCKED: &str = "blocked";
pub const R_FAILED: &str = "failed";
/// Another certificate run holds this name (renewer or manual).
pub const R_BUSY: &str = "busy";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameOutcome {
    pub name: String,
    pub apex: String,
    pub points_here: PointsHere,
    pub result: &'static str,
    pub detail: Option<String>,
    pub cert_id: Option<String>,
    pub expires_at: Option<i64>,
}

impl NameOutcome {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "apex": self.apex,
            "pointsHere": self.points_here.as_bool(),
            "dns": self.points_here.detail(),
            "result": self.result,
            "detail": self.detail,
            "stalwartCertId": self.cert_id,
            "expiresAt": self.expires_at,
        })
    }
}

/// How the planted certificates were loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadOutcome {
    /// `none` (nothing planted) | `action` | `restart` | `failed`.
    pub method: &'static str,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RunReport {
    pub names: Vec<NameOutcome>,
    pub reload: ReloadOutcome,
}

impl RunReport {
    pub fn failed(&self) -> usize {
        self.names.iter().filter(|n| n.result == R_FAILED).count()
            + usize::from(self.reload.method == "failed")
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "ok": true,
            "names": self.names.iter().map(NameOutcome::to_json).collect::<Vec<_>>(),
            "reload": { "method": self.reload.method, "error": self.reload.error },
            "failed": self.failed(),
        })
    }
}

/// Issue / renew every extra name that points here (or only `only`).
/// Per-name failures are recorded and never stop the run; nothing here
/// touches the mail certificate or `defaultCertificateId`.
pub fn run(
    plans: &[ApexPlan],
    resolver: &dyn DnsResolver,
    deps: &mut dyn Deps,
    state: &mut State,
    only: Option<&str>,
) -> RunReport {
    let only = only.map(norm);
    let now = deps.now();
    let mut outcomes: Vec<NameOutcome> = Vec::new();
    // Indexes into `outcomes` whose plant waits for the reload.
    let mut pending: Vec<usize> = Vec::new();
    // Lazily checked once, only when something would be planted.
    let mut default_guard: Option<Result<(), String>> = None;
    // Per-name locks (shared with the attached-name issuer and the
    // background renewer), held until the reload is done.
    let mut locks: Vec<crate::domains::renew::NameLock> = Vec::new();

    for plan in plans {
        for name in &plan.names {
            if only.as_deref().is_some_and(|o| o != name) {
                continue;
            }
            let Some(lock) = crate::domains::renew::NameLock::try_acquire(name) else {
                outcomes.push(NameOutcome {
                    name: name.clone(),
                    apex: plan.apex.clone(),
                    points_here: PointsHere::Unknown("not checked".into()),
                    result: R_BUSY,
                    detail: Some("another certificate run for this name is in progress".into()),
                    cert_id: None,
                    expires_at: None,
                });
                continue;
            };
            locks.push(lock);
            let rec = state.entry(name.clone()).or_default();
            let ph = points_here(resolver, name, &plan.addrs);
            rec.points_here = ph.as_bool();
            rec.dns_detail = ph.detail().map(String::from);
            rec.checked_at = Some(now);
            let mut out = NameOutcome {
                name: name.clone(),
                apex: plan.apex.clone(),
                points_here: ph.clone(),
                result: R_FAILED,
                detail: None,
                cert_id: rec.stalwart_cert_id.clone(),
                expires_at: rec.planted_not_after,
            };
            match &ph {
                PointsHere::No(d) => {
                    out.result = R_SKIPPED_DNS;
                    out.detail = Some(if rec.stalwart_cert_id.is_some() {
                        format!(
                            "{d}; not renewed — the certificate already in Stalwart is \
                             left to expire"
                        )
                    } else {
                        d.clone()
                    });
                    finish(rec, &out);
                    outcomes.push(out);
                    continue;
                }
                PointsHere::Unknown(d) => {
                    out.result = R_SKIPPED_DNS_UNKNOWN;
                    out.detail = Some(format!("{d}; nothing changed"));
                    finish(rec, &out);
                    outcomes.push(out);
                    continue;
                }
                PointsHere::Yes => {}
            }
            let binding = match plan.binding.as_ref() {
                None => Err(format!(
                    "zone {} is not attached here — K2 issues per-name certificates through \
                     DNS-01 on a K2-hosted zone only (k2 domain attach)",
                    plan.apex
                )),
                Some(b) => crate::domains::acme::check_extra_name_issuable(name, b).map(|_| b),
            };
            let binding = match binding {
                Ok(b) => b,
                Err(e) => {
                    out.result = R_UNSUPPORTED;
                    out.detail = Some(e);
                    finish(rec, &out);
                    outcomes.push(out);
                    continue;
                }
            };
            let inv = deps.inventory(name);
            let fresh = inv.as_ref().filter(|i| {
                i.reusable && i.not_after.is_some_and(|na| na - now > RENEW_BEFORE_SECS)
            });
            let planted_this = fresh.is_some_and(|i| {
                rec.stalwart_cert_id.is_some() && rec.planted_not_after == i.not_after
            });
            if planted_this && rec.loaded {
                out.result = R_CURRENT;
                out.expires_at = rec.planted_not_after;
                finish(rec, &out);
                outcomes.push(out);
                continue;
            }
            let guard = default_guard
                .get_or_insert_with(|| default_certificate_guard(deps.default_certificate()))
                .clone();
            if let Err(e) = guard {
                out.result = R_BLOCKED;
                out.detail = Some(e);
                finish(rec, &out);
                outcomes.push(out);
                continue;
            }
            if planted_this {
                // In Stalwart already; only the reload is missing.
                out.result = R_PLANTED;
                out.detail = Some("already in Stalwart; waiting for the TLS reload".into());
                out.expires_at = rec.planted_not_after;
                pending.push(outcomes.len());
                finish(rec, &out);
                outcomes.push(out);
                continue;
            }
            let (pem, result) = match fresh.cloned() {
                Some(i) => (i, R_PLANTED),
                None => match deps.issue(name, binding) {
                    Ok(i) => (i, R_ISSUED),
                    Err(e) => {
                        out.result = R_FAILED;
                        out.detail = Some(format!("issue: {e}"));
                        finish(rec, &out);
                        outcomes.push(out);
                        continue;
                    }
                },
            };
            match deps.add_certificate(&pem.chain_pem, &pem.key_pem) {
                Ok(id) => {
                    if let Some(prev) = rec.stalwart_cert_id.replace(id.clone()) {
                        if prev != id && !rec.stale_cert_ids.contains(&prev) {
                            rec.stale_cert_ids.push(prev);
                        }
                    }
                    rec.planted_not_after = pem.not_after;
                    rec.planted_at = Some(now);
                    rec.loaded = false;
                    out.result = result;
                    out.cert_id = Some(id);
                    out.expires_at = pem.not_after;
                    pending.push(outcomes.len());
                }
                Err(e) => {
                    out.result = R_FAILED;
                    out.detail = Some(format!(
                        "certificate is in the box store but was not added to Stalwart: {e}"
                    ));
                }
            }
            finish(rec, &out);
            outcomes.push(out);
        }
    }

    let reload = if pending.is_empty() {
        ReloadOutcome { method: "none", error: None }
    } else {
        match deps.reload_tls() {
            Ok(()) => ReloadOutcome { method: "action", error: None },
            Err(action_err) => match deps.restart_stalwart() {
                Ok(()) => ReloadOutcome {
                    method: "restart",
                    error: Some(format!("ReloadTlsCertificates failed ({action_err}); restarted Stalwart instead")),
                },
                Err(restart_err) => ReloadOutcome {
                    method: "failed",
                    error: Some(format!(
                        "ReloadTlsCertificates failed ({action_err}) and the restart failed \
                         ({restart_err}) — the new certificates load on Stalwart's next restart"
                    )),
                },
            },
        }
    };
    let loaded = reload.method == "action" || reload.method == "restart";
    for idx in pending {
        let out = &mut outcomes[idx];
        if let Some(rec) = state.get_mut(&out.name) {
            rec.loaded = loaded;
            if !loaded {
                rec.last_error = reload.error.clone();
            } else {
                // The new certificate is live: drop the ones it superseded.
                let current = rec.stalwart_cert_id.clone();
                let mut keep = Vec::new();
                for old in std::mem::take(&mut rec.stale_cert_ids) {
                    if Some(&old) == current.as_ref() {
                        continue;
                    }
                    if deps.remove_certificate(&old).is_err() {
                        keep.push(old);
                    }
                }
                rec.stale_cert_ids = keep;
            }
        }
        if !loaded {
            out.detail = reload.error.clone();
        }
    }
    drop(locks);
    RunReport { names: outcomes, reload }
}

fn finish(rec: &mut NameRecord, out: &NameOutcome) {
    rec.last_result = Some(out.result.to_string());
    rec.last_error = match out.result {
        R_FAILED | R_BLOCKED => out.detail.clone(),
        _ => None,
    };
}

/// CAL44 guard: never add a second certificate while Stalwart has no
/// live default (with no `*` entry it hands SNI-less clients an
/// arbitrary certificate — possibly the new one).
fn default_certificate_guard(state: Result<DefaultCertificate, String>) -> Result<(), String> {
    match state {
        Ok(DefaultCertificate::Present(_)) => Ok(()),
        Ok(DefaultCertificate::Unset) => Err(
            "Stalwart has no default certificate — adding a per-name certificate could \
             change what clients without SNI get. Fix the mail certificate first \
             (k2 hostmail cert renew)"
                .into(),
        ),
        Ok(DefaultCertificate::Missing(id)) => Err(format!(
            "Stalwart's default certificate ({id}) no longer exists (expired?) — fix the \
             mail certificate first (k2 hostmail cert renew)"
        )),
        Err(e) => Err(format!("could not read Stalwart's default certificate: {e}")),
    }
}

// ── Cached view (status) ────────────────────────────────────────────────

/// Cert state for one name from the state file + box store (no network).
/// `missing` | `failed` | `expired` | `not-planted` | `not-loaded` |
/// `renew-due` | `issued`.
pub fn cert_state(
    rec: Option<&NameRecord>,
    inv: Option<&Inventory>,
    now: i64,
) -> (&'static str, Option<i64>) {
    let Some(inv) = inv else {
        if rec.and_then(|r| r.last_error.as_ref()).is_some() {
            return ("failed", None);
        }
        return ("missing", None);
    };
    let exp = inv.not_after;
    if exp.is_some_and(|e| e <= now) {
        return ("expired", exp);
    }
    let Some(rec) = rec else {
        return ("not-planted", exp);
    };
    if rec.stalwart_cert_id.is_none() || rec.planted_not_after != exp {
        return ("not-planted", exp);
    }
    if !rec.loaded {
        return ("not-loaded", exp);
    }
    if exp.is_some_and(|e| e - now < RENEW_BEFORE_SECS) {
        return ("renew-due", exp);
    }
    ("issued", exp)
}

/// Every hosted apex with its mail host and extra names.
fn hosted_apexes() -> Vec<(String, Option<String>, Vec<String>)> {
    let domains: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        crate::mail::domains::load_all_domains(&conn)
            .into_iter()
            .map(|d| d.domain)
            .collect()
    };
    domains
        .into_iter()
        .map(|apex| {
            let mail = crate::mail::autoconfig::mail_hostname_for_apex(&apex);
            let names = extra_names_for_apex(&apex, mail.as_deref());
            (apex, mail, names)
        })
        .collect()
}

/// `inventory` from the box store (`~/.k2/certs/<name>/`).
pub fn inventory_from_store(name: &str) -> Option<Inventory> {
    let pem = crate::domains::store::load(name)?;
    Some(inventory_from_pem(name, pem.chain_pem, pem.key_pem))
}

fn inventory_from_pem(name: &str, chain_pem: String, key_pem: String) -> Inventory {
    Inventory {
        not_after: crate::domains::status::pem_leaf_not_after(&chain_pem),
        reusable: crate::domains::status::is_reusable_lets_encrypt(name, &chain_pem),
        chain_pem,
        key_pem,
    }
}

/// `extraNames` for `k2 hostmail status`: the last run's DNS answer +
/// the cert state from the box store. Never touches the network.
pub fn status_json(installed: bool) -> serde_json::Value {
    if !installed {
        return serde_json::json!([]);
    }
    let state = load_state();
    let now = chrono::Utc::now().timestamp();
    let mut out = Vec::new();
    for (_, _, names) in hosted_apexes() {
        for name in names {
            let rec = state.get(&name);
            let inv = inventory_from_store(&name);
            let (st, exp) = cert_state(rec, inv.as_ref(), now);
            out.push(serde_json::json!({
                "name": name,
                "pointsHere": rec.and_then(|r| r.points_here),
                "cert": { "state": st, "expiresAt": exp },
            }));
        }
    }
    serde_json::Value::Array(out)
}

// ── Routes: GET|POST /cli/mail/cert/names ───────────────────────────────

fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint } })
            .to_string(),
    }
}

fn mail_hostname() -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT hostname FROM mail_server WHERE id = 1", [], |r| {
        r.get::<_, Option<String>>(0)
    })
    .ok()
    .flatten()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

fn binding_for(apex: &str) -> Option<DomainBinding> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::domains::get_binding(&conn, apex).ok().flatten()
}

fn issuer_label(binding: Option<&DomainBinding>, name: &str) -> (&'static str, Option<String>) {
    match binding {
        None => (
            "unsupported",
            Some("zone not attached here (K2 issues per-name certificates on K2-hosted zones only)".into()),
        ),
        Some(b) => match crate::domains::acme::check_extra_name_issuable(name, b) {
            Ok(()) => ("k2-dns-01", None),
            Err(e) => ("unsupported", Some(e)),
        },
    }
}

/// GET `/cli/mail/cert/names` — per-name state. On Linux the DNS answer
/// is live (system resolver + discovered public IP); elsewhere it is the
/// last run's (the Mac example page stays network-silent).
pub fn handle_get(_params: &HashMap<String, String>) -> CliResponse {
    let Some(mail_host) = mail_hostname() else {
        return err_json(
            "409 Conflict",
            "not_ready",
            "the mail server is not installed — enable it first".into(),
        );
    };
    // Tests never touch the network, even on a Linux runner.
    let live = crate::mail::supervisor::mail_supported() && !cfg!(test);
    let resolver = if live {
        crate::mail::dns_verify::SystemResolver::new().ok()
    } else {
        None
    };
    let public_ip = if resolver.is_some() {
        use crate::mail::preflight::PreflightEnv;
        crate::mail::preflight::RealPreflightEnv.public_ip()
    } else {
        None
    };
    let state = load_state();
    let now = chrono::Utc::now().timestamp();
    let mut names_out = Vec::new();
    let mut box_json = serde_json::Value::Null;
    for (apex, apex_mail, names) in hosted_apexes() {
        let binding = binding_for(&apex);
        let host = apex_mail.clone().unwrap_or_else(|| mail_host.clone());
        let addrs = resolver
            .as_ref()
            .map(|r| box_addrs(public_ip.as_deref(), r, &host));
        if let (Some(a), true) = (addrs.as_ref(), box_json.is_null()) {
            box_json = a.to_json();
        }
        for name in names {
            let rec = state.get(&name);
            let inv = inventory_from_store(&name);
            let (st, exp) = cert_state(rec, inv.as_ref(), now);
            let (points, dns) = match (resolver.as_ref(), addrs.as_ref()) {
                (Some(r), Some(a)) => {
                    let ph = points_here(r, &name, a);
                    (ph.as_bool(), ph.detail().map(String::from))
                }
                _ => (
                    rec.and_then(|r| r.points_here),
                    rec.and_then(|r| r.dns_detail.clone()),
                ),
            };
            let (issuer, reason) = issuer_label(binding.as_ref(), &name);
            names_out.push(serde_json::json!({
                "name": name,
                "apex": apex,
                "pointsHere": points,
                "dns": dns,
                "issuer": issuer,
                "reason": reason,
                "cert": {
                    "state": st,
                    "expiresAt": exp,
                    "stalwartCertId": rec.and_then(|r| r.stalwart_cert_id.clone()),
                    "plantedAt": rec.and_then(|r| r.planted_at),
                    "lastResult": rec.and_then(|r| r.last_result.clone()),
                    "lastError": rec.and_then(|r| r.last_error.clone()),
                },
            }));
        }
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "live": live,
            "mailHost": mail_host,
            "box": box_json,
            "names": names_out,
        })
        .to_string(),
    )
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct PostBody {
    action: Option<String>,
    name: Option<String>,
}

/// POST `/cli/mail/cert/names` `{"action":"issue"|"renew", "name"?}` —
/// order / renew per-name certificates (owner/admin, the same gate as
/// `cert/renew`). `issue` and `renew` are the same run: a name gets a
/// new order only when it has no certificate or is inside
/// [`RENEW_BEFORE_SECS`] of expiry. Never touches the mail certificate.
pub fn handle_post(body: &[u8]) -> CliResponse {
    let trimmed = std::str::from_utf8(body).map(str::trim).unwrap_or("");
    let parsed: PostBody = if trimmed.is_empty() {
        PostBody::default()
    } else {
        match serde_json::from_str(trimmed) {
            Ok(b) => b,
            Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
        }
    };
    match parsed.action.as_deref() {
        Some("issue") | Some("renew") => {}
        other => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!(
                    "action must be 'issue' or 'renew' (got {})",
                    other.map(|s| format!("'{s}'")).unwrap_or_else(|| "nothing".into())
                ),
            )
        }
    }
    if mail_hostname().is_none() {
        return err_json(
            "409 Conflict",
            "not_ready",
            "the mail server is not installed — enable it before issuing per-name certificates"
                .into(),
        );
    }
    let apexes = hosted_apexes();
    let only = match parsed.name.as_deref().map(norm).filter(|s| !s.is_empty()) {
        None => None,
        Some(n) => {
            if !apexes.iter().any(|(_, _, names)| names.contains(&n)) {
                return err_json(
                    "400 Bad Request",
                    "not_extra_name",
                    format!(
                        "'{n}' is not an extra mail-family name — only autoconfig, \
                         autodiscover, mta-sts and ua-auto-config under a hosted domain \
                         (never the apex, never www)"
                    ),
                );
            }
            Some(n)
        }
    };
    if !crate::mail::supervisor::mail_supported() {
        return err_json(
            "409 Conflict",
            "unsupported",
            "the email server only works on Linux deployments; this daemon is not Linux".into(),
        );
    }
    if let Some(resp) = crate::mail::routes_server::upgrade_running_response() {
        return resp;
    }
    match live_run(None, only.as_deref()) {
        Ok((report, state_err)) => {
            crate::domains::renew::record_extra_run(&report, "manual");
            let mut body = report.to_json();
            if let Some(e) = state_err {
                body["stateError"] = serde_json::json!(e);
            }
            CliResponse::ok_json(body.to_string())
        }
        Err(LiveRunError::Busy) => err_json(
            "409 Conflict",
            "busy",
            "a per-name certificate run (the background renewer?) is in progress — try again \
             in a few minutes"
                .into(),
        ),
        Err(LiveRunError::NotReady(e)) => err_json("409 Conflict", "not_ready", e),
        Err(LiveRunError::Dns(e)) => err_json("502 Bad Gateway", "dns", e),
    }
}

/// Why [`live_run`] did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveRunError {
    /// Another run holds the state file (manual POST vs the renewer).
    Busy,
    NotReady(String),
    Dns(String),
}

fn run_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

/// One run against the live box: the POST route (`only` = one name) and
/// the background renewer (`names` = exactly these names). Plans come
/// from the hosted domains (never a certs-dir walk); the DNS gate runs
/// inside [`run`]. Serialised on the state file: a second concurrent run
/// gets [`LiveRunError::Busy`] instead of waiting. The `Option<String>`
/// is a state-file save error (the run itself happened).
pub fn live_run(
    names: Option<&std::collections::BTreeSet<String>>,
    only: Option<&str>,
) -> Result<(RunReport, Option<String>), LiveRunError> {
    let _g = run_lock().try_lock().map_err(|_| LiveRunError::Busy)?;
    let mail_host = mail_hostname().ok_or_else(|| {
        LiveRunError::NotReady("the mail server is not installed".into())
    })?;
    let client = crate::mail::domains::engine_from_db().map(|(c, _)| c).map_err(|e| {
        LiveRunError::NotReady(format!(
            "cannot plant certificates while the mail server is down: {e}"
        ))
    })?;
    let resolver = crate::mail::dns_verify::SystemResolver::new().map_err(LiveRunError::Dns)?;
    let public_ip = {
        use crate::mail::preflight::PreflightEnv;
        crate::mail::preflight::RealPreflightEnv.public_ip()
    };
    let plans: Vec<ApexPlan> = hosted_apexes()
        .into_iter()
        .map(|(apex, apex_mail, all)| {
            let host = apex_mail.unwrap_or_else(|| mail_host.clone());
            ApexPlan {
                binding: binding_for(&apex),
                addrs: box_addrs(public_ip.as_deref(), &resolver, &host),
                apex,
                names: all
                    .into_iter()
                    .filter(|n| names.is_none_or(|set| set.contains(n)))
                    .collect(),
            }
        })
        .filter(|p| !p.names.is_empty())
        .collect();
    let mut state = load_state();
    let mut deps = LiveDeps { client };
    let report = run(&plans, &resolver, &mut deps, &mut state, only);
    Ok((report, save_state(&state).err()))
}

/// Renewer input: the extra names K2 has issued before (present in the
/// state file) that are still extra names of a hosted domain, with the
/// apex binding. Never a certs-dir walk; a name whose domain is no longer
/// hosted is left alone.
pub fn renewable_extra_names() -> Vec<(String, Option<DomainBinding>)> {
    let state = load_state();
    let mut out = Vec::new();
    for (apex, _, names) in hosted_apexes() {
        let binding = binding_for(&apex);
        for n in names {
            if state.contains_key(&n) {
                out.push((n, binding.clone()));
            }
        }
    }
    out
}

/// A fresh certificate in the box store that never got planted / loaded
/// (the renewer finishes it without a new order).
pub fn needs_plant(name: &str) -> bool {
    let state = load_state();
    let Some(rec) = state.get(name) else {
        return false;
    };
    let Some(inv) = inventory_from_store(name) else {
        return false;
    };
    inv.reusable
        && (rec.stalwart_cert_id.is_none() || rec.planted_not_after != inv.not_after || !rec.loaded)
}

/// Production [`Deps`].
pub struct LiveDeps {
    pub client: crate::mail::jmap::StalwartClient,
}

impl Deps for LiveDeps {
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn inventory(&self, name: &str) -> Option<Inventory> {
        inventory_from_store(name)
    }

    fn issue(&mut self, name: &str, binding: &DomainBinding) -> Result<Inventory, String> {
        let pem = crate::domains::acme::issue_extra_name(name, binding)?;
        Ok(inventory_from_pem(name, pem.chain_pem, pem.key_pem))
    }

    fn default_certificate(&mut self) -> Result<DefaultCertificate, String> {
        self.client.default_certificate_state()
    }

    fn add_certificate(&mut self, chain_pem: &str, key_pem: &str) -> Result<String, String> {
        self.client.certificate_add(chain_pem, key_pem)
    }

    fn reload_tls(&mut self) -> Result<(), String> {
        self.client.action_reload_tls_certificates()
    }

    fn restart_stalwart(&mut self) -> Result<(), String> {
        crate::mail::supervisor::restart_stalwart_to_reload_tls()
    }

    fn remove_certificate(&mut self, id: &str) -> Result<(), String> {
        self.client.certificate_destroy(id)
    }
}

/// Doctor input: one hosted apex.
#[derive(Debug, Clone)]
pub struct DoctorApex {
    pub apex: String,
    pub names: Vec<String>,
    /// The zone is attached with DNS write (K2 can issue these).
    pub k2_dns: bool,
    pub addrs: BoxAddrs,
}

/// Build the doctor's apex list (`only` = one hosted domain, else all).
pub fn doctor_apexes(
    only: Option<&str>,
    public_ip: Option<&str>,
    resolver: &dyn DnsResolver,
    fallback_mail_host: &str,
) -> Vec<DoctorApex> {
    hosted_apexes()
        .into_iter()
        .filter(|(apex, _, _)| only.is_none_or(|o| o == apex))
        .map(|(apex, mail, names)| {
            let host = mail.unwrap_or_else(|| fallback_mail_host.to_string());
            let k2_dns = binding_for(&apex)
                .is_some_and(|b| b.dns_write && !b.is_pending_ns());
            DoctorApex {
                addrs: box_addrs(public_ip, resolver, &host),
                apex,
                names,
                k2_dns,
            }
        })
        .collect()
}

/// The doctor's local expiry read (box store PEM).
pub fn local_expiry(name: &str) -> Option<i64> {
    inventory_from_store(name).and_then(|i| i.not_after)
}

/// The doctor's "which leaf did K2 plant for this name": the SHA-256 of
/// the box store's leaf, only when K2 planted it into Stalwart
/// (`extra-names.json` has its Certificate id).
pub fn local_sha256(name: &str) -> Option<String> {
    let state = load_state();
    state.get(&norm(name))?.stalwart_cert_id.as_ref()?;
    let pem = crate::domains::store::load(name)?;
    crate::domains::status::pem_leaf_sha256(&pem.chain_pem)
}

// ── 0.45.1: this box's own addresses (A2 retry gate, A3 A row) ─────────

/// A public IPv4: not loopback, private, link-local, CGNAT (100.64/10),
/// unspecified or broadcast.
pub fn is_public_v4(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || (o[0] == 100 && (o[1] & 0xc0) == 64))
}

/// A global unicast IPv6 (2000::/3).
pub fn is_global_v6(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xe000) == 0x2000
}

/// This box's public interface addresses (HF12/HF14): IPv4 bound to a
/// local interface (an OVH failover IP counts) and global IPv6. Tests
/// inject them; never the network.
pub fn local_interface_addrs() -> (Vec<Ipv4Addr>, Vec<Ipv6Addr>) {
    #[cfg(test)]
    {
        return TEST_LOCAL_ADDRS.with(|c| c.borrow().clone()).unwrap_or_default();
    }
    #[cfg(not(test))]
    {
        let mut v4 = Vec::new();
        let mut v6 = Vec::new();
        // SAFETY: getifaddrs fills a linked list we only read, then free.
        unsafe {
            let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
            if libc::getifaddrs(&mut head) != 0 {
                return (v4, v6);
            }
            let mut cur = head;
            while !cur.is_null() {
                let addr = (*cur).ifa_addr;
                if !addr.is_null() {
                    match (*addr).sa_family as i32 {
                        libc::AF_INET => {
                            let sin = &*(addr as *const libc::sockaddr_in);
                            let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                            if is_public_v4(&ip) && !v4.contains(&ip) {
                                v4.push(ip);
                            }
                        }
                        libc::AF_INET6 => {
                            let sin6 = &*(addr as *const libc::sockaddr_in6);
                            let ip = Ipv6Addr::from(sin6.sin6_addr.s6_addr);
                            if is_global_v6(&ip) && !v6.contains(&ip) {
                                v6.push(ip);
                            }
                        }
                        _ => {}
                    }
                }
                cur = (*cur).ifa_next;
            }
            libc::freeifaddrs(head);
        }
        (v4, v6)
    }
}

#[cfg(test)]
thread_local! {
    static TEST_LOCAL_ADDRS: std::cell::RefCell<Option<(Vec<Ipv4Addr>, Vec<Ipv6Addr>)>> =
        const { std::cell::RefCell::new(None) };
}

/// Test seam: this box's interface addresses (default: none).
#[cfg(test)]
pub(crate) fn set_test_local_addrs(addrs: Option<(Vec<Ipv4Addr>, Vec<Ipv6Addr>)>) {
    TEST_LOCAL_ADDRS.with(|c| *c.borrow_mut() = addrs);
}

/// "This box" from its own view: the public (egress) IPv4 plus every
/// public interface IPv4; IPv6 = the global interface addresses only. A
/// mail-host AAAA that is not one of them counts as elsewhere (unlike
/// [`box_addrs`], which takes v6 from the mail host's own AAAA, HF12).
pub fn box_addrs_local(public_ip: Option<&str>) -> BoxAddrs {
    let (local_v4, v6) = local_interface_addrs();
    let mut v4: Vec<Ipv4Addr> = public_ip
        .and_then(|s| s.trim().parse::<Ipv4Addr>().ok())
        .into_iter()
        .collect();
    for ip in local_v4 {
        if !v4.contains(&ip) {
            v4.push(ip);
        }
    }
    BoxAddrs { v4, v6, source: "public-ip+interfaces" }
}

/// The cached public IPv4 (`mail_server.public_ipv4`, migration 0139).
pub fn cached_public_ipv4() -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT public_ipv4 FROM mail_server WHERE id = 1", [], |r| {
        r.get::<_, Option<String>>(0)
    })
    .ok()
    .flatten()
    .filter(|s| s.parse::<Ipv4Addr>().is_ok())
}

/// Record the public IPv4 (+ when). Ignores a non-IPv4 value.
pub fn store_public_ipv4(ip: &str, at: i64) {
    if ip.trim().parse::<Ipv4Addr>().is_err() {
        return;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "UPDATE mail_server SET public_ipv4 = ?1, public_ipv4_at = ?2 WHERE id = 1",
        rusqlite::params![ip.trim(), at],
    );
}

/// Read the public IPv4 now (checkip / ipify) and cache it. Never under
/// `K2_AIRGAP` (HF14); tests never touch the network (seam).
pub fn refresh_public_ipv4() -> Option<String> {
    #[cfg(test)]
    let airgap = TEST_AIRGAP.with(|c| *c.borrow());
    #[cfg(not(test))]
    let airgap = k2_core::airgap::enabled();
    if airgap {
        return None;
    }
    #[cfg(test)]
    let ip = TEST_PUBLIC_IP.with(|c| c.borrow().clone());
    #[cfg(not(test))]
    let ip = {
        use crate::mail::preflight::{PreflightEnv, RealPreflightEnv};
        RealPreflightEnv.public_ip()
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Some(ip) = &ip {
        store_public_ipv4(ip, now);
    }
    ip
}

/// [`refresh_public_ipv4`] when the cache is empty or older than an hour
/// (`domain add` / `domain check` — user-triggered, never the poller).
pub fn refresh_public_ipv4_if_stale() {
    let at: Option<i64> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT public_ipv4_at FROM mail_server WHERE id = 1 AND public_ipv4 IS NOT NULL",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if at.is_none_or(|t| now - t > 3600) {
        let _ = refresh_public_ipv4();
    }
}

#[cfg(test)]
thread_local! {
    static TEST_PUBLIC_IP: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// Per-thread air-gap pin (never the process env: parallel tests).
    static TEST_AIRGAP: std::cell::RefCell<bool> = const { std::cell::RefCell::new(false) };
}

#[cfg(test)]
pub(crate) fn set_test_airgap(on: bool) {
    TEST_AIRGAP.with(|c| *c.borrow_mut() = on);
}

/// Test seam: what "checkip" answers (default: nothing).
#[cfg(test)]
pub(crate) fn set_test_public_ip(ip: Option<&str>) {
    TEST_PUBLIC_IP.with(|c| *c.borrow_mut() = ip.map(str::to_string));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const BOX: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 10);
    const OTHER: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 99);
    const NOW: i64 = 1_800_000_000;

    /// `run` takes process-wide per-name locks; tests that run it share
    /// names, so they take turns.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        SERIAL.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Canned DNS: A / AAAA per name; anything unlisted = NotFound.
    #[derive(Default)]
    struct Dns {
        a: HashMap<String, Result<Vec<Ipv4Addr>, DnsError>>,
        aaaa: HashMap<String, Result<Vec<Ipv6Addr>, DnsError>>,
    }

    impl Dns {
        fn a(mut self, name: &str, ips: &[Ipv4Addr]) -> Self {
            self.a.insert(name.into(), Ok(ips.to_vec()));
            self
        }
        fn a_err(mut self, name: &str) -> Self {
            self.a.insert(name.into(), Err(DnsError::Other("SERVFAIL".into())));
            self
        }
        fn aaaa(mut self, name: &str, ips: &[Ipv6Addr]) -> Self {
            self.aaaa.insert(name.into(), Ok(ips.to_vec()));
            self
        }
    }

    impl DnsResolver for Dns {
        fn mx(&self, _: &str) -> Result<Vec<crate::mail::dns_verify::MxHost>, DnsError> {
            Err(DnsError::NotFound)
        }
        fn txt(&self, _: &str) -> Result<Vec<Vec<String>>, DnsError> {
            Err(DnsError::NotFound)
        }
        fn a(&self, name: &str) -> Result<Vec<Ipv4Addr>, DnsError> {
            self.a.get(name).cloned().unwrap_or(Err(DnsError::NotFound))
        }
        fn ptr(&self, _: std::net::IpAddr) -> Result<Vec<String>, DnsError> {
            Err(DnsError::NotFound)
        }
        fn srv(&self, _: &str) -> Result<Vec<crate::mail::dns_verify::SrvAnswer>, DnsError> {
            Err(DnsError::NotFound)
        }
        fn aaaa(&self, name: &str) -> Result<Vec<Ipv6Addr>, DnsError> {
            self.aaaa.get(name).cloned().unwrap_or(Err(DnsError::NotFound))
        }
    }

    /// Recording fake for every non-DNS effect.
    struct Fake {
        calls: RefCell<Vec<String>>,
        inventory: HashMap<String, Inventory>,
        issue_fail: Vec<String>,
        default: Result<DefaultCertificate, String>,
        add_fail: Vec<String>,
        reload: Result<(), String>,
        restart: Result<(), String>,
        next_id: usize,
    }

    impl Default for Fake {
        fn default() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                inventory: HashMap::new(),
                issue_fail: Vec::new(),
                default: Ok(DefaultCertificate::Present("cert-mail".into())),
                add_fail: Vec::new(),
                reload: Ok(()),
                restart: Ok(()),
                next_id: 0,
            }
        }
    }

    fn pem_for(name: &str, not_after: i64, reusable: bool) -> Inventory {
        Inventory {
            chain_pem: format!("CHAIN:{name}"),
            key_pem: format!("KEY:{name}"),
            not_after: Some(not_after),
            reusable,
        }
    }

    impl Deps for Fake {
        fn now(&self) -> i64 {
            NOW
        }
        fn inventory(&self, name: &str) -> Option<Inventory> {
            self.inventory.get(name).cloned()
        }
        fn issue(&mut self, name: &str, binding: &DomainBinding) -> Result<Inventory, String> {
            self.calls.borrow_mut().push(format!("issue {name} @{}", binding.apex));
            if self.issue_fail.iter().any(|n| n == name) {
                return Err(format!("acme order for {name} failed"));
            }
            let inv = pem_for(name, NOW + 90 * 86_400, true);
            self.inventory.insert(name.into(), inv.clone());
            Ok(inv)
        }
        fn default_certificate(&mut self) -> Result<DefaultCertificate, String> {
            self.calls.borrow_mut().push("default".into());
            self.default.clone()
        }
        fn add_certificate(&mut self, chain_pem: &str, _key_pem: &str) -> Result<String, String> {
            let name = chain_pem.trim_start_matches("CHAIN:").to_string();
            self.calls.borrow_mut().push(format!("add {name}"));
            if self.add_fail.contains(&name) {
                return Err("x:Certificate/set create rejected — invalidProperties".into());
            }
            self.next_id += 1;
            Ok(format!("cert-{}", self.next_id))
        }
        fn reload_tls(&mut self) -> Result<(), String> {
            self.calls.borrow_mut().push("reload".into());
            self.reload.clone()
        }
        fn restart_stalwart(&mut self) -> Result<(), String> {
            self.calls.borrow_mut().push("restart".into());
            self.restart.clone()
        }
        fn remove_certificate(&mut self, id: &str) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("remove {id}"));
            Ok(())
        }
    }

    fn k2_binding(apex: &str) -> DomainBinding {
        DomainBinding {
            apex: apex.into(),
            zone_id: Some("zone-1".into()),
            dns_write: true,
            created_at: 0,
            status: Some("active".into()),
            nameservers: Vec::new(),
            auto_created: false,
        }
    }

    fn plan(apex: &str, binding: Option<DomainBinding>) -> ApexPlan {
        ApexPlan {
            apex: apex.into(),
            binding,
            names: extra_names_for_apex(apex, Some(&format!("mail.{apex}"))),
            addrs: BoxAddrs { v4: vec![BOX], v6: Vec::new(), source: "public-ip" },
        }
    }

    fn all_here(apex: &str) -> Dns {
        let mut d = Dns::default();
        for l in EXTRA_LABELS {
            d = d.a(&format!("{l}.{apex}"), &[BOX]);
        }
        d
    }

    fn calls(f: &Fake) -> Vec<String> {
        f.calls.borrow().clone()
    }

    #[test]
    fn names_are_the_four_labels_never_apex_or_www() {
        let names = extra_names_for_apex("Example.COM.", Some("mail.example.com"));
        assert_eq!(
            names,
            vec![
                "autoconfig.example.com",
                "autodiscover.example.com",
                "mta-sts.example.com",
                "ua-auto-config.example.com",
            ]
        );
        assert!(!names.iter().any(|n| n == "example.com" || n.starts_with("www.")));
        // A mail host that IS one of the labels is the mail cert's, never a second cert.
        let names = extra_names_for_apex("example.com", Some("autoconfig.example.com"));
        assert_eq!(names.len(), 3);
        assert!(!names.contains(&"autoconfig.example.com".to_string()));
        assert!(extra_names_for_apex("rosson.k2.dev", None).is_empty(), "Connect names stay on cert.k2.dev");
        assert!(extra_names_for_apex("", None).is_empty());
        assert!(extra_names_for_apex("localhost", None).is_empty());
    }

    #[test]
    fn dns_gate_a_aaaa_and_unknown() {
        let addrs = BoxAddrs {
            v4: vec![BOX],
            v6: vec!["2001:db8::10".parse().unwrap()],
            source: "public-ip",
        };
        let dns = Dns::default()
            .a("autoconfig.example.com", &[BOX])
            .a("autodiscover.example.com", &[OTHER])
            .a("mta-sts.example.com", &[BOX, OTHER])
            .a_err("ua-auto-config.example.com")
            .a("v6ok.example.com", &[BOX])
            .aaaa("v6ok.example.com", &["2001:db8::10".parse().unwrap()])
            .a("v6bad.example.com", &[BOX])
            .aaaa("v6bad.example.com", &["2001:db8::99".parse().unwrap()]);
        assert_eq!(points_here(&dns, "autoconfig.example.com", &addrs), PointsHere::Yes);
        let no = points_here(&dns, "autodiscover.example.com", &addrs);
        assert!(matches!(&no, PointsHere::No(d) if d.contains("192.0.2.99")), "{no:?}");
        assert!(
            matches!(points_here(&dns, "mta-sts.example.com", &addrs), PointsHere::No(_)),
            "every A must be this box"
        );
        assert!(matches!(
            points_here(&dns, "ua-auto-config.example.com", &addrs),
            PointsHere::Unknown(_)
        ));
        let none = points_here(&dns, "nothing.example.com", &addrs);
        assert!(matches!(&none, PointsHere::No(d) if d.contains("no A record")), "{none:?}");
        assert_eq!(points_here(&dns, "v6ok.example.com", &addrs), PointsHere::Yes);
        let v6 = points_here(&dns, "v6bad.example.com", &addrs);
        assert!(matches!(&v6, PointsHere::No(d) if d.contains("AAAA")), "{v6:?}");
        // Unknown box address → Unknown, never "moved".
        let empty = BoxAddrs::default();
        assert!(matches!(
            points_here(&dns, "autoconfig.example.com", &empty),
            PointsHere::Unknown(_)
        ));
    }

    #[test]
    fn box_addrs_prefers_public_ip_then_mail_host() {
        let dns = Dns::default()
            .a("mail.example.com", &[OTHER])
            .aaaa("mail.example.com", &["2001:db8::1".parse().unwrap()]);
        let a = box_addrs(Some("192.0.2.10"), &dns, "mail.example.com");
        assert_eq!(a.v4, vec![BOX]);
        assert_eq!(a.source, "public-ip");
        assert_eq!(a.v6.len(), 1);
        let b = box_addrs(None, &dns, "mail.example.com");
        assert_eq!(b.v4, vec![OTHER]);
        assert_eq!(b.source, "mail-host");
    }

    #[test]
    fn issues_only_names_that_point_here_one_cert_each_then_reloads_once() {
        let _s = serial();
        let dns = Dns::default()
            .a("autoconfig.example.com", &[BOX])
            .a("autodiscover.example.com", &[BOX])
            .a("mta-sts.example.com", &[OTHER]); // ua-auto-config: no A
        let mut fake = Fake::default();
        let mut state = State::new();
        let r = run(&[plan("example.com", Some(k2_binding("example.com")))], &dns, &mut fake, &mut state, None);
        let res: Vec<(&str, &str)> = r.names.iter().map(|n| (n.name.as_str(), n.result)).collect();
        assert_eq!(
            res,
            vec![
                ("autoconfig.example.com", R_ISSUED),
                ("autodiscover.example.com", R_ISSUED),
                ("mta-sts.example.com", R_SKIPPED_DNS),
                ("ua-auto-config.example.com", R_SKIPPED_DNS),
            ]
        );
        assert_eq!(
            calls(&fake),
            vec![
                "default",
                "issue autoconfig.example.com @example.com",
                "add autoconfig.example.com",
                "issue autodiscover.example.com @example.com",
                "add autodiscover.example.com",
                "reload",
            ],
            "one order + one non-default add per name, the default checked once, one reload"
        );
        assert_eq!(r.reload, ReloadOutcome { method: "action", error: None });
        let rec = &state["autoconfig.example.com"];
        assert_eq!(rec.stalwart_cert_id.as_deref(), Some("cert-1"));
        assert!(rec.loaded);
        assert_eq!(rec.points_here, Some(true));
        assert_eq!(state["mta-sts.example.com"].points_here, Some(false));
        assert_eq!(r.failed(), 0);
    }

    #[test]
    fn one_name_failing_never_stops_the_others() {
        let _s = serial();
        let mut fake = Fake {
            issue_fail: vec!["autodiscover.example.com".into()],
            add_fail: vec!["mta-sts.example.com".into()],
            ..Fake::default()
        };
        let mut state = State::new();
        let r = run(&[plan("example.com", Some(k2_binding("example.com")))], &all_here("example.com"), &mut fake, &mut state, None);
        let res: Vec<&str> = r.names.iter().map(|n| n.result).collect();
        assert_eq!(res, vec![R_ISSUED, R_FAILED, R_FAILED, R_ISSUED]);
        assert!(r.names[1].detail.as_deref().unwrap().contains("acme order"));
        assert!(r.names[2].detail.as_deref().unwrap().contains("not added to Stalwart"));
        assert_eq!(r.reload.method, "action", "the good names still load");
        assert!(state["autoconfig.example.com"].loaded);
        assert!(state["ua-auto-config.example.com"].loaded);
        assert!(state["autodiscover.example.com"].last_error.is_some());
        assert!(state["mta-sts.example.com"].stalwart_cert_id.is_none());
        assert_eq!(r.failed(), 2);
        assert!(
            !calls(&fake).iter().any(|c| c.contains("mail.example.com")),
            "the mail cert is never touched: {:?}",
            calls(&fake)
        );
    }

    #[test]
    fn reload_action_first_then_restart_fallback_then_failed() {
        let _s = serial();
        // Action fails → restart (the existing path) → loaded.
        let mut fake = Fake { reload: Err("validationFailed".into()), ..Fake::default() };
        let mut state = State::new();
        let r = run(&[plan("example.com", Some(k2_binding("example.com")))], &all_here("example.com"), &mut fake, &mut state, Some("autoconfig.example.com"));
        let c = calls(&fake);
        let reload_at = c.iter().position(|x| x == "reload").expect("reload");
        let restart_at = c.iter().position(|x| x == "restart").expect("restart");
        assert!(reload_at < restart_at, "action before restart: {c:?}");
        assert_eq!(r.reload.method, "restart");
        assert!(state["autoconfig.example.com"].loaded);

        // Action OK → never a restart.
        let mut fake = Fake::default();
        let mut state = State::new();
        run(&[plan("example.com", Some(k2_binding("example.com")))], &all_here("example.com"), &mut fake, &mut state, Some("autoconfig.example.com"));
        assert!(!calls(&fake).contains(&"restart".to_string()));

        // Both fail → recorded, not loaded, certs still planted.
        let mut fake = Fake {
            reload: Err("validationFailed".into()),
            restart: Err("mail helper missing".into()),
            ..Fake::default()
        };
        let mut state = State::new();
        let r = run(&[plan("example.com", Some(k2_binding("example.com")))], &all_here("example.com"), &mut fake, &mut state, Some("autoconfig.example.com"));
        assert_eq!(r.reload.method, "failed");
        assert!(r.reload.error.as_deref().unwrap().contains("next restart"));
        let rec = &state["autoconfig.example.com"];
        assert!(!rec.loaded);
        assert!(rec.stalwart_cert_id.is_some());
        assert_eq!(cert_state(Some(rec), fake.inventory.get("autoconfig.example.com"), NOW).0, "not-loaded");

        // Nothing planted → no reload at all.
        let mut fake = Fake::default();
        let mut state = State::new();
        let r = run(&[plan("example.com", Some(k2_binding("example.com")))], &Dns::default(), &mut fake, &mut state, None);
        assert_eq!(r.reload.method, "none");
        assert!(calls(&fake).is_empty(), "{:?}", calls(&fake));
    }

    #[test]
    fn renewal_skips_a_name_whose_dns_moved_and_leaves_its_cert() {
        let _s = serial();
        let mut fake = Fake::default();
        let mut state = State::new();
        let p = [plan("example.com", Some(k2_binding("example.com")))];
        run(&p, &all_here("example.com"), &mut fake, &mut state, None);
        // Later: autoconfig now points at another host, and every cert is
        // inside the renewal window.
        for inv in fake.inventory.values_mut() {
            inv.not_after = Some(NOW + 5 * 86_400);
        }
        for rec in state.values_mut() {
            rec.planted_not_after = Some(NOW + 5 * 86_400);
        }
        fake.calls.borrow_mut().clear();
        let moved = all_here("example.com").a("autoconfig.example.com", &[OTHER]);
        let r = run(&p, &moved, &mut fake, &mut state, None);
        assert_eq!(r.names[0].result, R_SKIPPED_DNS);
        assert!(r.names[0].detail.as_deref().unwrap().contains("left to expire"));
        let c = calls(&fake);
        assert!(!c.iter().any(|x| x.contains("autoconfig")), "no order, no destroy: {c:?}");
        assert!(c.contains(&"issue autodiscover.example.com @example.com".to_string()), "{c:?}");
        let rec = &state["autoconfig.example.com"];
        assert_eq!(rec.points_here, Some(false));
        assert_eq!(rec.stalwart_cert_id.as_deref(), Some("cert-1"), "record kept");

        // A DNS lookup failure changes nothing either.
        fake.calls.borrow_mut().clear();
        let flaky = Dns::default().a_err("autoconfig.example.com");
        let r = run(&p, &flaky, &mut fake, &mut state, Some("autoconfig.example.com"));
        assert_eq!(r.names[0].result, R_SKIPPED_DNS_UNKNOWN);
        assert!(calls(&fake).is_empty());
    }

    #[test]
    fn current_cert_is_left_alone_and_a_planted_unloaded_one_only_reloads() {
        let _s = serial();
        let mut fake = Fake::default();
        let mut state = State::new();
        let p = [plan("example.com", Some(k2_binding("example.com")))];
        run(&p, &all_here("example.com"), &mut fake, &mut state, Some("autoconfig.example.com"));
        fake.calls.borrow_mut().clear();
        let r = run(&p, &all_here("example.com"), &mut fake, &mut state, Some("autoconfig.example.com"));
        assert_eq!(r.names[0].result, R_CURRENT);
        assert!(calls(&fake).is_empty(), "{:?}", calls(&fake));
        // Planted but the reload never happened → reload only, no new add.
        state.get_mut("autoconfig.example.com").unwrap().loaded = false;
        let r = run(&p, &all_here("example.com"), &mut fake, &mut state, Some("autoconfig.example.com"));
        assert_eq!(r.names[0].result, R_PLANTED);
        assert_eq!(calls(&fake), vec!["default", "reload"]);
        // On disk (fresh LE) but not in Stalwart → planted without a new order.
        let mut fake = Fake::default();
        fake.inventory.insert("autodiscover.example.com".into(), pem_for("autodiscover.example.com", NOW + 80 * 86_400, true));
        let mut state = State::new();
        let r = run(&p, &all_here("example.com"), &mut fake, &mut state, Some("autodiscover.example.com"));
        assert_eq!(r.names[0].result, R_PLANTED);
        assert_eq!(calls(&fake), vec!["default", "add autodiscover.example.com", "reload"]);
        // A non-LE (fake CA) PEM on disk is re-issued, not planted.
        let mut fake = Fake::default();
        fake.inventory.insert("autodiscover.example.com".into(), pem_for("autodiscover.example.com", NOW + 80 * 86_400, false));
        let r = run(&p, &all_here("example.com"), &mut fake, &mut State::new(), Some("autodiscover.example.com"));
        assert_eq!(r.names[0].result, R_ISSUED);
    }

    #[test]
    fn no_live_default_certificate_blocks_every_plant() {
        let _s = serial();
        for default in [
            Ok(DefaultCertificate::Unset),
            Ok(DefaultCertificate::Missing("cert-old".into())),
            Err("JMAP down".to_string()),
        ] {
            let mut fake = Fake { default, ..Fake::default() };
            let mut state = State::new();
            let r = run(&[plan("example.com", Some(k2_binding("example.com")))], &all_here("example.com"), &mut fake, &mut state, None);
            assert!(r.names.iter().all(|n| n.result == R_BLOCKED), "{:?}", r.names);
            assert_eq!(calls(&fake), vec!["default"], "checked once, nothing ordered or added");
            assert_eq!(r.reload.method, "none");
        }
    }

    #[test]
    fn zone_not_k2_hosted_is_unsupported_never_issued() {
        let _s = serial();
        let byo = DomainBinding { dns_write: false, zone_id: None, ..k2_binding("example.com") };
        for b in [None, Some(byo)] {
            let mut fake = Fake::default();
            let r = run(&[plan("example.com", b)], &all_here("example.com"), &mut fake, &mut State::new(), None);
            assert!(r.names.iter().all(|n| n.result == R_UNSUPPORTED), "{:?}", r.names);
            assert!(calls(&fake).is_empty(), "{:?}", calls(&fake));
        }
    }

    #[test]
    fn only_filter_and_multiple_apexes() {
        let _s = serial();
        let mut fake = Fake::default();
        let dns = all_here("example.com").a("autoconfig.example.net", &[BOX]);
        let plans = [
            plan("example.com", Some(k2_binding("example.com"))),
            plan("example.net", Some(k2_binding("example.net"))),
        ];
        let r = run(&plans, &dns, &mut fake, &mut State::new(), Some("AUTOCONFIG.example.net."));
        assert_eq!(r.names.len(), 1);
        assert_eq!(r.names[0].name, "autoconfig.example.net");
        assert_eq!(r.names[0].result, R_ISSUED);
    }

    /// A name another run holds (the background renewer / an attached
    /// issue) is reported busy and untouched; the others go on.
    #[test]
    fn locked_name_is_busy_and_untouched() {
        let _s = serial();
        let held = crate::domains::renew::NameLock::try_acquire("autodiscover.example.com").expect("lock");
        let mut fake = Fake::default();
        let mut state = State::new();
        let r = run(&[plan("example.com", Some(k2_binding("example.com")))], &all_here("example.com"), &mut fake, &mut state, None);
        let res: Vec<&str> = r.names.iter().map(|n| n.result).collect();
        assert_eq!(res, vec![R_ISSUED, R_BUSY, R_ISSUED, R_ISSUED]);
        assert!(!calls(&fake).iter().any(|c| c.contains("autodiscover")), "{:?}", calls(&fake));
        assert!(!state.contains_key("autodiscover.example.com"));
        drop(held);
        // The run released its own locks.
        assert!(crate::domains::renew::NameLock::try_acquire("autoconfig.example.com").is_some());
    }

    /// Renewal supersedes: the new certificate is added (non-default),
    /// reloaded, and only then the name's previous certificate is removed.
    #[test]
    fn renewal_removes_the_superseded_cert_after_the_reload() {
        let _s = serial();
        let mut fake = Fake::default();
        let mut state = State::new();
        let p = [plan("example.com", Some(k2_binding("example.com")))];
        run(&p, &all_here("example.com"), &mut fake, &mut state, Some("autoconfig.example.com"));
        assert_eq!(state["autoconfig.example.com"].stalwart_cert_id.as_deref(), Some("cert-1"));
        // 20 days left → inside the window → re-issued.
        fake.inventory.get_mut("autoconfig.example.com").unwrap().not_after = Some(NOW + 20 * 86_400);
        state.get_mut("autoconfig.example.com").unwrap().planted_not_after = Some(NOW + 20 * 86_400);
        fake.calls.borrow_mut().clear();
        let r = run(&p, &all_here("example.com"), &mut fake, &mut state, Some("autoconfig.example.com"));
        assert_eq!(r.names[0].result, R_ISSUED);
        assert_eq!(
            calls(&fake),
            vec!["default", "issue autoconfig.example.com @example.com", "add autoconfig.example.com", "reload", "remove cert-1"],
            "remove only after the reload"
        );
        let rec = &state["autoconfig.example.com"];
        assert_eq!(rec.stalwart_cert_id.as_deref(), Some("cert-2"));
        assert!(rec.stale_cert_ids.is_empty());
        // A failed load keeps the old one (both stay; Stalwart serves the
        // newest per name once it loads).
        let mut fake = Fake { reload: Err("x".into()), restart: Err("y".into()), ..fake };
        fake.inventory.get_mut("autoconfig.example.com").unwrap().not_after = Some(NOW + 10 * 86_400);
        state.get_mut("autoconfig.example.com").unwrap().planted_not_after = Some(NOW + 10 * 86_400);
        fake.calls.borrow_mut().clear();
        run(&p, &all_here("example.com"), &mut fake, &mut state, Some("autoconfig.example.com"));
        assert!(!calls(&fake).iter().any(|c| c.starts_with("remove")), "{:?}", calls(&fake));
        assert_eq!(state["autoconfig.example.com"].stale_cert_ids, vec!["cert-2".to_string()]);
    }

    #[test]
    fn cert_state_table() {
        let inv = pem_for("a.example.com", NOW + 60 * 86_400, true);
        let rec = NameRecord {
            stalwart_cert_id: Some("c".into()),
            planted_not_after: inv.not_after,
            loaded: true,
            ..NameRecord::default()
        };
        assert_eq!(cert_state(None, None, NOW).0, "missing");
        let failed = NameRecord { last_error: Some("x".into()), ..NameRecord::default() };
        assert_eq!(cert_state(Some(&failed), None, NOW).0, "failed");
        assert_eq!(cert_state(Some(&rec), Some(&inv), NOW), ("issued", inv.not_after));
        assert_eq!(cert_state(None, Some(&inv), NOW).0, "not-planted");
        let unloaded = NameRecord { loaded: false, ..rec.clone() };
        assert_eq!(cert_state(Some(&unloaded), Some(&inv), NOW).0, "not-loaded");
        let soon = pem_for("a.example.com", NOW + 86_400, true);
        let rec_soon = NameRecord { planted_not_after: soon.not_after, ..rec.clone() };
        assert_eq!(cert_state(Some(&rec_soon), Some(&soon), NOW).0, "renew-due");
        let old = pem_for("a.example.com", NOW - 1, true);
        assert_eq!(cert_state(Some(&rec), Some(&old), NOW).0, "expired");
    }

    #[test]
    fn state_file_round_trips_0600() {
        let _home = crate::test_support::TempHome::new();
        let mut s = State::new();
        s.insert(
            "autoconfig.example.com".into(),
            NameRecord { stalwart_cert_id: Some("cert-1".into()), loaded: true, ..NameRecord::default() },
        );
        save_state(&s).expect("save");
        assert_eq!(load_state(), s);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(state_path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    /// 0.45.1: this box's own addresses — public IPv4 + public interface
    /// IPv4s, IPv6 only from global interfaces (HF12/HF14).
    #[test]
    fn box_addrs_local_uses_interfaces_not_the_mail_hosts_aaaa() {
        assert!(is_public_v4(&"203.0.113.7".parse().unwrap()));
        for p in ["10.0.0.1", "192.168.1.20", "172.16.0.1", "127.0.0.1", "169.254.1.1", "100.64.0.1", "0.0.0.0"] {
            assert!(!is_public_v4(&p.parse().unwrap()), "{p}");
        }
        assert!(is_global_v6(&"2001:db8::1".parse().unwrap()));
        assert!(!is_global_v6(&"fe80::1".parse().unwrap()));
        assert!(!is_global_v6(&"::1".parse().unwrap()));
        set_test_local_addrs(Some((vec!["198.51.100.9".parse().unwrap()], vec!["2001:db8::5".parse().unwrap()])));
        let b = box_addrs_local(Some("203.0.113.7"));
        assert_eq!(b.v4, vec!["203.0.113.7".parse::<Ipv4Addr>().unwrap(), "198.51.100.9".parse().unwrap()]);
        assert_eq!(b.v6, vec!["2001:db8::5".parse::<Ipv6Addr>().unwrap()]);
        set_test_local_addrs(None);
        let b = box_addrs_local(None);
        assert!(b.v4.is_empty() && b.v6.is_empty());
    }

    /// The IP cache: written by a refresh, never under air-gap, never
    /// a non-IPv4 value; `refresh_if_stale` skips a fresh value.
    #[test]
    fn public_ipv4_cache_refresh_respects_air_gap() {
        let _g = crate::mail::mail_server_test_lock();
        let _clean = crate::mail::MailServerRowCleanup;
        let _ = k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, updated_at) VALUES (1, 'running', '0.16.20', 1)",
                [],
            )
            .expect("seed");
        }
        set_test_public_ip(Some("203.0.113.7"));
        set_test_airgap(true);
        assert_eq!(refresh_public_ipv4(), None, "air-gap: no lookup");
        assert_eq!(cached_public_ipv4(), None);
        set_test_airgap(false);
        assert_eq!(refresh_public_ipv4().as_deref(), Some("203.0.113.7"));
        assert_eq!(cached_public_ipv4().as_deref(), Some("203.0.113.7"));
        // Fresh: not re-read even when checkip would now say otherwise.
        set_test_public_ip(Some("203.0.113.8"));
        refresh_public_ipv4_if_stale();
        assert_eq!(cached_public_ipv4().as_deref(), Some("203.0.113.7"));
        store_public_ipv4("not-an-ip", 5);
        assert_eq!(cached_public_ipv4().as_deref(), Some("203.0.113.7"));
        set_test_public_ip(None);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
        }
    }

    #[test]
    fn post_body_validation() {
        let r = handle_post(br#"{"action":"wipe"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("issue"), "{}", r.body);
        let r = handle_post(b"{}");
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        let r = handle_post(br#"{"action":"issue","force":true}"#);
        assert_eq!(r.status, "400 Bad Request", "unknown keys refused: {}", r.body);
        let r = handle_post(b"not json");
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }
}
