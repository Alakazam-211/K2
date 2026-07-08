//! Deliverability doctor — S6 (PRD §9, modeled on mail-in-a-box's
//! `status_checks.py`): MiaB-style ✓/✖/? checks with prose +
//! current-vs-expected values, and a **direct-send readiness grade**
//! (`pass|warn|fail`) that GATES enabling direct mode
//! ([`direct_send_gate`], consumed by [`super::config::set_send_mode`]).
//!
//! CHECKS (server-level run, `domain = None`):
//! - `server-state` — the supervisor's view of the sidecar.
//! - `public-ip` — box IP discoverable (NAT/CGNAT coaching on fail).
//! - `ptr` / `fcrdns` — reverse DNS == mail hostname, and the PTR name
//!   resolves BACK to the box IP (forward-confirmed rDNS). PTR is a
//!   BLOCKER for direct mode only (pre-mortem #5) — the detail carries
//!   the "set it at your VPS provider, not in your DNS zone" reality.
//! - `smtp-banner` — the SMTP greeting hostname matches the mail
//!   hostname (filters score mismatches, pre-mortem #16). Soft.
//! - `outbound-25` — TCP reachability of an external MX; failing adds
//!   the PROVIDER coaching (GCP: never unblocked; DigitalOcean:
//!   effectively never; Hetzner: ticket + ~1 month; Linode:
//!   friendliest) and points at relay mode (pre-mortem #6).
//! - `dnsbl:<zone>` — Spamhaus ZEN + Barracuda + SpamCop (blocking),
//!   UCEPROTECT informational-only ("major providers ignore this");
//!   Spamhaus PBL return codes get the self-service exclusion link;
//!   public-resolver-blocked answers read as `unknown`, never `fail`.
//! - `open-relay` — a REAL relay self-test against the local Stalwart
//!   (unauthenticated MAIL/RCPT to a foreign domain must 5xx; never a
//!   DATA). Cannot-run is a FAIL, not a skip (pre-mortem #3).
//! - `starttls-25` / `starttls-587` — STARTTLS advertised.
//! - `tls-cert` — plan A: a default-verifying HTTPS handshake against
//!   the mail hostname (never `danger_accept_invalid_certs`); plans
//!   B/C report info (the cert lives behind the owner's proxy).
//! - `disk` — headroom for mail storage (pre-mortem #12).
//!
//! Plus, for a DOMAIN run (`domain = Some`): `mx`/`spf`/`dkim`/`dmarc`
//! posture re-using [`super::dns_verify`]'s Valid/Missing/Wrong
//! classification (ONE resolver abstraction for the family), and
//! `spf-lookups` (the 10-DNS-lookup SPF limit, pre-mortem #16).
//!
//! **No real network in tests** — every probe is behind the
//! [`DoctorEnv`] trait (+ the shared `DnsResolver`); production impls
//! are thin ([`RealDoctorEnv`]) and the SMTP reply parsing they share
//! is pure and fixture-tested. The end-to-end seed-mailbox probe
//! (PRD §9 "send a probe message") is NOT built here: the K2-operated
//! seed mailbox does not exist yet (PRD §18.3) — and the doctor NEVER
//! sends mail until it does (pre-mortem #1).
//!
//! Runs persist to `mail_doctor_runs`; [`latest_run_json`] serves the
//! GET route + the Settings card, which never trigger probes.

use std::io::BufRead;

use crate::mail::dns_verify::{self, DnsError, DnsResolver};
use crate::mail::domains;

// ── Check vocabulary ────────────────────────────────────────────────────

pub const ST_PASS: &str = "pass";
pub const ST_WARN: &str = "warn";
pub const ST_FAIL: &str = "fail";
pub const ST_INFO: &str = "info";
/// Probe/lookup trouble — never graded as a definitive fail.
pub const ST_UNKNOWN: &str = "unknown";

#[derive(Debug, Clone)]
pub struct DoctorCheck {
    pub id: String,
    pub label: String,
    pub status: &'static str,
    pub detail: String,
    /// Counts toward the DIRECT-SEND readiness grade.
    pub gates_direct: bool,
}

impl DoctorCheck {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "label": self.label,
            "status": self.status,
            "detail": self.detail,
            "gatesDirect": self.gates_direct,
        })
    }
}

#[derive(Debug)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
    pub grade: &'static str,
    pub direct_blockers: Vec<String>,
    pub coaching: Vec<String>,
    pub hostname: String,
    pub ip: Option<String>,
    pub domain: Option<String>,
}

impl DoctorReport {
    pub fn results_json(&self) -> serde_json::Value {
        serde_json::json!({
            "hostname": self.hostname,
            "ip": self.ip,
            "domain": self.domain,
            "grade": self.grade,
            "checks": self.checks.iter().map(DoctorCheck::to_json).collect::<Vec<_>>(),
            "directBlockers": self.direct_blockers,
            "coaching": self.coaching,
        })
    }
}

/// The direct-send readiness grade over the GATING checks only:
/// any fail → `fail` (+ the blocker ids); any unknown/warn → `warn`;
/// else `pass`. Non-gating checks inform, they never grade.
pub fn grade_of(checks: &[DoctorCheck]) -> (&'static str, Vec<String>) {
    let mut blockers = Vec::new();
    let mut soft = false;
    for c in checks.iter().filter(|c| c.gates_direct) {
        match c.status {
            ST_FAIL => blockers.push(c.id.clone()),
            ST_UNKNOWN | ST_WARN => soft = true,
            _ => {}
        }
    }
    if !blockers.is_empty() {
        (ST_FAIL, blockers)
    } else if soft {
        (ST_WARN, blockers)
    } else {
        (ST_PASS, blockers)
    }
}

// ── Probe seam (injectable — no real network in tests, ever) ────────────

/// EHLO conversation summary.
#[derive(Debug, Clone, Default)]
pub struct EhloInfo {
    pub starttls: bool,
    /// The hostname the 220 greeting announced.
    pub banner_host: Option<String>,
}

/// Outcome of the open-relay self-test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayVerdict {
    /// The server refused the foreign RCPT (the correct answer).
    Refused(u16),
    /// The server ACCEPTED a foreign relay recipient — open relay.
    AcceptedRelay,
}

/// Every non-DNS observation the doctor makes, behind one trait.
/// Production = [`RealDoctorEnv`]; tests inject recording fakes.
pub trait DoctorEnv {
    fn public_ip(&self) -> Option<String>;
    /// Outbound-:25 probe to an external MX (TCP handshake only —
    /// never an SMTP dialogue off-box, pre-mortem #1).
    fn outbound_25(&self) -> Result<(), String>;
    /// EHLO against `host:port` (production call sites pass the
    /// LOOPBACK only).
    fn smtp_ehlo(&self, host: &str, port: u16) -> Result<EhloInfo, String>;
    /// Unauthenticated relay attempt against `host:port` (loopback
    /// only): MAIL FROM a foreign address, RCPT TO a foreign domain,
    /// assert refusal. Never issues DATA.
    fn open_relay_test(&self, host: &str, port: u16) -> Result<RelayVerdict, String>;
    /// Default-VERIFYING TLS handshake against `https://hostname/`
    /// (plan A cert check). Never disables verification.
    fn https_cert(&self, hostname: &str) -> Result<(), String>;
    fn disk_free_bytes(&self) -> Option<u64>;
}

// ── The check table (pure over the seams) ───────────────────────────────

/// What the runner needs from the `mail_server` row.
pub struct ServerCtx {
    pub hostname: String,
    pub port_plan: Option<String>,
    pub status: String,
}

/// A domain's context for the per-domain checks: the CURRENT effective
/// record rows (send-mode SPF adjustment applied at read time).
pub struct DomainCtx {
    pub domain: String,
    pub rows: Vec<domains::RecordRow>,
}

/// DNSBL zones: (zone, informational-only). Informational zones never
/// gate or grade (PRD §9: "major providers ignore this — don't
/// panic").
pub const DNSBLS: [(&str, bool); 4] = [
    ("zen.spamhaus.org", false),
    ("b.barracudacentral.org", false),
    ("bl.spamcop.net", false),
    ("dnsbl-1.uceprotect.net", true),
];

const MIN_DISK_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The §9 provider-coaching text for a blocked outbound :25.
pub fn outbound_25_coaching(reason: &str) -> String {
    format!(
        "outbound port 25 is blocked ({reason}). Provider realities: GCP NEVER unblocks \
         it; DigitalOcean effectively never; Hetzner unblocks via support ticket after \
         the first paid invoice (~1 month); Linode is friendliest (set rDNS + records \
         first, then ticket). Relay mode works everywhere — switch with \
         `k2 mail config --domain <domain> --send-mode relay`"
    )
}

/// §9.1 postmaster hygiene — always in the coaching card.
const POSTMASTER_COACHING: &str =
    "Recommended: register Google Postmaster Tools (postmaster.google.com) and \
     Microsoft SNDS/JMRP — the sender-reputation dashboards for the two biggest \
     inbox providers.";

/// Run the full check table. PURE over the injected seams — every
/// branch unit-tests with canned answers; nothing here touches the DB.
pub fn run_checks(
    resolver: &dyn DnsResolver,
    env: &dyn DoctorEnv,
    ctx: &ServerCtx,
    domain: Option<&mut DomainCtx>,
    now: i64,
) -> DoctorReport {
    let mut checks: Vec<DoctorCheck> = Vec::new();
    let mut coaching: Vec<String> = Vec::new();

    // Server state (context row — grading happens via the probes).
    checks.push(DoctorCheck {
        id: "server-state".into(),
        label: "Mail server state".into(),
        status: if ctx.status == "running" { ST_INFO } else { ST_WARN },
        detail: if ctx.status == "running" {
            format!("running ({})", ctx.hostname)
        } else {
            format!("{} — local SMTP checks below will likely fail until it runs", ctx.status)
        },
        gates_direct: false,
    });

    // Public IP.
    let ip = env.public_ip();
    checks.push(match &ip {
        Some(ip) => DoctorCheck {
            id: "public-ip".into(),
            label: "Public IP discovered".into(),
            status: ST_PASS,
            detail: ip.clone(),
            gates_direct: true,
        },
        None => DoctorCheck {
            id: "public-ip".into(),
            label: "Public IP discovered".into(),
            status: ST_FAIL,
            detail: "no public IP discoverable — NAT/CGNAT likely; direct send (and \
                     inbound mail) cannot work from this box"
                .into(),
            gates_direct: true,
        },
    });

    // PTR + FCrDNS.
    checks.extend(ptr_checks(resolver, &ctx.hostname, ip.as_deref()));

    // SMTP banner vs hostname (loopback EHLO — also feeds STARTTLS).
    let ehlo25 = env.smtp_ehlo("127.0.0.1", 25);
    checks.push(match &ehlo25 {
        Ok(info) => match info.banner_host.as_deref() {
            Some(host) if dns_verify::host_eq(host, &ctx.hostname) => DoctorCheck {
                id: "smtp-banner".into(),
                label: "SMTP banner matches hostname".into(),
                status: ST_PASS,
                detail: host.to_string(),
                gates_direct: false,
            },
            Some(host) => DoctorCheck {
                id: "smtp-banner".into(),
                label: "SMTP banner matches hostname".into(),
                status: ST_WARN,
                detail: format!(
                    "banner announces '{host}' but the mail hostname is '{}' — spam \
                     filters score the mismatch",
                    ctx.hostname
                ),
                gates_direct: false,
            },
            None => DoctorCheck {
                id: "smtp-banner".into(),
                label: "SMTP banner matches hostname".into(),
                status: ST_UNKNOWN,
                detail: "no hostname in the SMTP greeting".into(),
                gates_direct: false,
            },
        },
        Err(e) => DoctorCheck {
            id: "smtp-banner".into(),
            label: "SMTP banner matches hostname".into(),
            status: ST_UNKNOWN,
            detail: format!("could not talk to the local SMTP listener: {e}"),
            gates_direct: false,
        },
    });

    // Outbound :25.
    match env.outbound_25() {
        Ok(()) => checks.push(DoctorCheck {
            id: "outbound-25".into(),
            label: "Outbound port 25 reachable".into(),
            status: ST_PASS,
            detail: String::new(),
            gates_direct: true,
        }),
        Err(reason) => {
            let coach = outbound_25_coaching(&reason);
            coaching.push(coach.clone());
            checks.push(DoctorCheck {
                id: "outbound-25".into(),
                label: "Outbound port 25 reachable".into(),
                status: ST_FAIL,
                detail: coach,
                gates_direct: true,
            });
        }
    }

    // DNSBLs.
    for (zone, informational) in DNSBLS {
        checks.push(dnsbl_check(resolver, zone, informational, ip.as_deref(), &mut coaching));
    }

    // Open-relay self-test (fail-closed: can't-run = fail).
    checks.push(match env.open_relay_test("127.0.0.1", 25) {
        Ok(RelayVerdict::Refused(code)) => DoctorCheck {
            id: "open-relay".into(),
            label: "Not an open relay".into(),
            status: ST_PASS,
            detail: format!("foreign relay recipient refused ({code})"),
            gates_direct: true,
        },
        Ok(RelayVerdict::AcceptedRelay) => DoctorCheck {
            id: "open-relay".into(),
            label: "Not an open relay".into(),
            status: ST_FAIL,
            detail: "the server ACCEPTED an unauthenticated foreign recipient — this is \
                     an OPEN RELAY; spammers find these within hours. Stop the server \
                     and fix the configuration NOW"
                .into(),
            gates_direct: true,
        },
        Err(e) => DoctorCheck {
            id: "open-relay".into(),
            label: "Not an open relay".into(),
            status: ST_FAIL,
            detail: format!(
                "the relay self-test could not run ({e}) — treating as failed \
                 (fail-closed): a relay posture we cannot verify is not safe to send from"
            ),
            gates_direct: true,
        },
    });

    // STARTTLS on 25 (reusing the EHLO above) + 587.
    checks.push(starttls_check("starttls-25", 25, &ehlo25));
    let ehlo587 = env.smtp_ehlo("127.0.0.1", 587);
    checks.push(starttls_check("starttls-587", 587, &ehlo587));

    // TLS cert (plan A only — B/C certs live behind the owner's proxy).
    checks.push(match ctx.port_plan.as_deref() {
        Some("tls-alpn") => match env.https_cert(&ctx.hostname) {
            Ok(()) => DoctorCheck {
                id: "tls-cert".into(),
                label: "TLS certificate valid".into(),
                status: ST_PASS,
                detail: format!("verified handshake against https://{}/", ctx.hostname),
                gates_direct: false,
            },
            Err(e) => DoctorCheck {
                id: "tls-cert".into(),
                label: "TLS certificate valid".into(),
                status: ST_WARN,
                detail: format!(
                    "could not complete a verified TLS handshake against \
                     https://{}/: {e} — check ACME issuance/renewal",
                    ctx.hostname
                ),
                gates_direct: false,
            },
        },
        plan => DoctorCheck {
            id: "tls-cert".into(),
            label: "TLS certificate valid".into(),
            status: ST_INFO,
            detail: format!(
                "port plan {} — the public certificate is served behind your existing \
                 proxy; Stalwart re-verifies it at each ACME renewal",
                plan.unwrap_or("unset")
            ),
            gates_direct: false,
        },
    });

    // Disk headroom (pre-mortem #12).
    checks.push(match env.disk_free_bytes() {
        Some(free) if free >= MIN_DISK_BYTES => DoctorCheck {
            id: "disk".into(),
            label: "Disk headroom for mail storage".into(),
            status: ST_PASS,
            detail: format!("{:.1} GB free", free as f64 / 1e9),
            gates_direct: false,
        },
        Some(free) => DoctorCheck {
            id: "disk".into(),
            label: "Disk headroom for mail storage".into(),
            status: ST_WARN,
            detail: format!(
                "only {:.1} GB free — mail storage grows; a full disk takes the whole \
                 box down (retention defaults help, but free space now)",
                free as f64 / 1e9
            ),
            gates_direct: false,
        },
        None => DoctorCheck {
            id: "disk".into(),
            label: "Disk headroom for mail storage".into(),
            status: ST_WARN,
            detail: "could not determine free disk space".into(),
            gates_direct: false,
        },
    });

    // Per-domain posture.
    let domain_name = domain.as_ref().map(|d| d.domain.clone());
    if let Some(d) = domain {
        checks.extend(domain_checks(resolver, d, now));
    }

    coaching.push(POSTMASTER_COACHING.to_string());
    let (grade, direct_blockers) = grade_of(&checks);
    DoctorReport {
        checks,
        grade,
        direct_blockers,
        coaching,
        hostname: ctx.hostname.clone(),
        ip,
        domain: domain_name,
    }
}

fn starttls_check(
    id: &str,
    port: u16,
    ehlo: &Result<EhloInfo, String>,
) -> DoctorCheck {
    match ehlo {
        Ok(info) if info.starttls => DoctorCheck {
            id: id.into(),
            label: format!("STARTTLS offered on :{port}"),
            status: ST_PASS,
            detail: String::new(),
            gates_direct: false,
        },
        Ok(_) => DoctorCheck {
            id: id.into(),
            label: format!("STARTTLS offered on :{port}"),
            status: ST_WARN,
            detail: "EHLO did not advertise STARTTLS — peers will speak plaintext or \
                     refuse to deliver"
                .into(),
            gates_direct: false,
        },
        Err(e) => DoctorCheck {
            id: id.into(),
            label: format!("STARTTLS offered on :{port}"),
            status: ST_UNKNOWN,
            detail: format!("could not probe: {e}"),
            gates_direct: false,
        },
    }
}

/// PTR + forward-confirmation for the box IP. PTR/FCrDNS gate DIRECT
/// mode only (pre-mortem #5) — and the fix lives at the VPS provider,
/// which the prose says outright.
fn ptr_checks(
    resolver: &dyn DnsResolver,
    hostname: &str,
    ip: Option<&str>,
) -> Vec<DoctorCheck> {
    let provider_hint = format!(
        "set the reverse DNS of the box IP to '{hostname}' at your VPS PROVIDER \
         (Hetzner/DO/Linode panel) — it is not a record in your DNS zone. Required \
         for direct-send mode; relay mode works without it"
    );
    let Some(ip_str) = ip else {
        return vec![
            DoctorCheck {
                id: "ptr".into(),
                label: "Reverse DNS (PTR) matches hostname".into(),
                status: ST_UNKNOWN,
                detail: "no public IP to look up".into(),
                gates_direct: true,
            },
            DoctorCheck {
                id: "fcrdns".into(),
                label: "Forward-confirmed reverse DNS".into(),
                status: ST_UNKNOWN,
                detail: "no public IP to confirm".into(),
                gates_direct: true,
            },
        ];
    };
    let Ok(parsed) = ip_str.parse::<std::net::IpAddr>() else {
        return vec![DoctorCheck {
            id: "ptr".into(),
            label: "Reverse DNS (PTR) matches hostname".into(),
            status: ST_UNKNOWN,
            detail: format!("unparseable public IP '{ip_str}'"),
            gates_direct: true,
        }];
    };
    let mut out = Vec::new();
    match resolver.ptr(parsed) {
        Ok(names) if names.iter().any(|n| dns_verify::host_eq(n, hostname)) => {
            out.push(DoctorCheck {
                id: "ptr".into(),
                label: "Reverse DNS (PTR) matches hostname".into(),
                status: ST_PASS,
                detail: format!("{ip_str} → {hostname}"),
                gates_direct: true,
            });
            // Forward-confirm: the PTR name resolves back to the IP.
            match resolver.a(hostname) {
                Ok(addrs) if addrs.iter().any(|a| a.to_string() == ip_str) => {
                    out.push(DoctorCheck {
                        id: "fcrdns".into(),
                        label: "Forward-confirmed reverse DNS".into(),
                        status: ST_PASS,
                        detail: format!("{hostname} → {ip_str}"),
                        gates_direct: true,
                    })
                }
                Ok(addrs) => out.push(DoctorCheck {
                    id: "fcrdns".into(),
                    label: "Forward-confirmed reverse DNS".into(),
                    status: ST_FAIL,
                    detail: format!(
                        "'{hostname}' resolves to {addrs:?}, not {ip_str} — fix its A \
                         record so the round trip confirms"
                    ),
                    gates_direct: true,
                }),
                Err(DnsError::NotFound) => out.push(DoctorCheck {
                    id: "fcrdns".into(),
                    label: "Forward-confirmed reverse DNS".into(),
                    status: ST_FAIL,
                    detail: format!(
                        "'{hostname}' has no A record — add `A {hostname} → {ip_str}`"
                    ),
                    gates_direct: true,
                }),
                Err(DnsError::Other(e)) => out.push(DoctorCheck {
                    id: "fcrdns".into(),
                    label: "Forward-confirmed reverse DNS".into(),
                    status: ST_UNKNOWN,
                    detail: format!("A lookup failed: {e}"),
                    gates_direct: true,
                }),
            }
        }
        Ok(names) if !names.is_empty() => out.push(DoctorCheck {
            id: "ptr".into(),
            label: "Reverse DNS (PTR) matches hostname".into(),
            status: ST_FAIL,
            detail: format!("{ip_str} → {} (expected '{hostname}'): {provider_hint}", names.join(", ")),
            gates_direct: true,
        }),
        Ok(_) | Err(DnsError::NotFound) => out.push(DoctorCheck {
            id: "ptr".into(),
            label: "Reverse DNS (PTR) matches hostname".into(),
            status: ST_FAIL,
            detail: format!("no PTR record for {ip_str}: {provider_hint}"),
            gates_direct: true,
        }),
        Err(DnsError::Other(e)) => out.push(DoctorCheck {
            id: "ptr".into(),
            label: "Reverse DNS (PTR) matches hostname".into(),
            status: ST_UNKNOWN,
            detail: format!("PTR lookup failed: {e}"),
            gates_direct: true,
        }),
    }
    // A failed/unknown PTR (without the pass arm above) leaves FCrDNS
    // unconfirmable.
    if out.len() == 1 {
        let ptr_status = out[0].status;
        out.push(DoctorCheck {
            id: "fcrdns".into(),
            label: "Forward-confirmed reverse DNS".into(),
            status: if ptr_status == ST_UNKNOWN { ST_UNKNOWN } else { ST_FAIL },
            detail: "unconfirmable until the PTR record is right".into(),
            gates_direct: true,
        });
    }
    out
}

/// Reversed dotted-quad for DNSBL queries (`1.2.3.4` → `4.3.2.1`).
pub fn reverse_ipv4(ip: &str) -> Option<String> {
    let addr: std::net::Ipv4Addr = ip.parse().ok()?;
    let o = addr.octets();
    Some(format!("{}.{}.{}.{}", o[3], o[2], o[1], o[0]))
}

/// One DNSBL check. Listed = the query name resolves; the RETURN CODE
/// carries meaning: `127.255.*` = "public resolver blocked" (Spamhaus
/// answers open resolvers that way — unknown, not listed);
/// `127.0.0.10/11` = Spamhaus PBL (policy listing, self-service
/// exclusion — coached, still a direct blocker until excluded).
fn dnsbl_check(
    resolver: &dyn DnsResolver,
    zone: &str,
    informational: bool,
    ip: Option<&str>,
    coaching: &mut Vec<String>,
) -> DoctorCheck {
    let id = format!("dnsbl:{zone}");
    let label = format!("Not listed on {zone}");
    let gates = !informational;
    let Some(rev) = ip.and_then(reverse_ipv4) else {
        return DoctorCheck {
            id,
            label,
            status: ST_UNKNOWN,
            detail: "no public IPv4 to query".into(),
            gates_direct: gates,
        };
    };
    match resolver.a(&format!("{rev}.{zone}")) {
        Err(DnsError::NotFound) => DoctorCheck {
            id,
            label,
            status: ST_PASS,
            detail: String::new(),
            gates_direct: gates,
        },
        Err(DnsError::Other(e)) => DoctorCheck {
            id,
            label,
            status: ST_UNKNOWN,
            detail: format!("lookup failed: {e}"),
            gates_direct: gates,
        },
        Ok(codes) => {
            let strs: Vec<String> = codes.iter().map(|c| c.to_string()).collect();
            if codes.iter().any(|c| c.octets()[0] == 127 && c.octets()[1] == 255) {
                return DoctorCheck {
                    id,
                    label,
                    status: ST_UNKNOWN,
                    detail: "the blocklist refused this resolver (public/open resolvers \
                             are blocked) — re-run from the box's own resolver"
                        .into(),
                    gates_direct: gates,
                };
            }
            if informational {
                return DoctorCheck {
                    id,
                    label,
                    status: ST_INFO,
                    detail: format!(
                        "listed ({}) — informational only: major providers ignore this \
                         list; don't panic and don't pay for removal",
                        strs.join(", ")
                    ),
                    gates_direct: false,
                };
            }
            let pbl = zone.contains("spamhaus")
                && codes
                    .iter()
                    .any(|c| matches!(c.octets(), [127, 0, 0, 10] | [127, 0, 0, 11]));
            let detail = if pbl {
                let coach = format!(
                    "the IP sits in the Spamhaus PBL (a POLICY listing for dynamic/cloud \
                     ranges, not a spam accusation) — request the self-service exclusion \
                     at https://check.spamhaus.org/ ({})",
                    strs.join(", ")
                );
                coaching.push(coach.clone());
                coach
            } else {
                format!("LISTED ({}) — check the zone's lookup page for delisting", strs.join(", "))
            };
            DoctorCheck { id, label, status: ST_FAIL, detail, gates_direct: gates }
        }
    }
}

/// Count the SPF terms that cost a DNS lookup (RFC 7208 §4.6.4 limit
/// of 10): include, a, mx, ptr, exists, redirect.
pub fn spf_lookup_terms(spf: &str) -> usize {
    spf.split_whitespace()
        .skip(1) // v=spf1
        .filter(|term| {
            let t = term.trim_start_matches(['+', '-', '~', '?']).to_ascii_lowercase();
            t == "a"
                || t == "mx"
                || t == "ptr"
                || t.starts_with("a:")
                || t.starts_with("mx:")
                || t.starts_with("ptr:")
                || t.starts_with("include:")
                || t.starts_with("exists:")
                || t.starts_with("redirect=")
        })
        .count()
}

/// Domain-posture checks re-using the S2 verifier (Valid/Missing/Wrong
/// against the CURRENT effective rows — relay SPF adjustments
/// included). SPF + DKIM gate direct mode; MX and DMARC inform.
fn domain_checks(
    resolver: &dyn DnsResolver,
    d: &mut DomainCtx,
    now: i64,
) -> Vec<DoctorCheck> {
    let summary = dns_verify::verify_rows(resolver, &d.domain, &mut d.rows, now);
    let show_hint = format!("`k2 mail domain show {}` has the expected-vs-live diff", d.domain);
    let mut out = Vec::new();
    let mut push = |id: &str, label: String, ok: bool, gates: bool, detail: String| {
        out.push(DoctorCheck {
            id: id.into(),
            label,
            status: if ok {
                ST_PASS
            } else if summary.any_unknown {
                ST_UNKNOWN
            } else {
                ST_FAIL
            },
            detail: if ok { String::new() } else { detail },
            gates_direct: gates,
        });
    };
    push(
        "mx",
        format!("MX routes {} here", d.domain),
        summary.mx_valid,
        false,
        format!("inbound mail cannot arrive — {show_hint}"),
    );
    push(
        "spf",
        "SPF record valid".into(),
        summary.spf_valid,
        true,
        format!("receivers will distrust this domain's mail — {show_hint}"),
    );
    push(
        "dkim",
        "DKIM keys published".into(),
        summary.dkim_valid,
        true,
        format!("signatures cannot verify — {show_hint}"),
    );
    // DMARC: nagged, never failed (§6.3 discipline carries over).
    out.push(DoctorCheck {
        id: "dmarc".into(),
        label: "DMARC policy published".into(),
        status: if summary.dmarc_valid { ST_PASS } else { ST_WARN },
        detail: if summary.dmarc_valid {
            String::new()
        } else {
            format!(
                "strongly recommended (start at p=none — observation before \
                 enforcement) — {show_hint}"
            )
        },
        gates_direct: false,
    });
    // SPF 10-lookup limit, over the live value when Valid (what the
    // world sees), else the expected value.
    let spf_value = d
        .rows
        .iter()
        .find(|r| r.id == "spf")
        .map(|r| {
            r.live
                .as_ref()
                .and_then(|l| l.first().cloned())
                .unwrap_or_else(|| r.expected.clone())
        })
        .unwrap_or_default();
    let terms = spf_lookup_terms(&spf_value);
    out.push(DoctorCheck {
        id: "spf-lookups".into(),
        label: "SPF stays under the 10-DNS-lookup limit".into(),
        status: if terms <= 10 { ST_PASS } else { ST_FAIL },
        detail: if terms <= 10 {
            format!("{terms} lookup term(s)")
        } else {
            format!(
                "{terms} lookup terms — receivers hard-fail SPF past 10 (RFC 7208); \
                 flatten the record"
            )
        },
        gates_direct: terms > 10,
    });
    out
}

// ── SMTP reply plumbing (pure parsers + the thin real env) ─────────────

/// Read one (possibly multiline) SMTP reply: `250-...` continuation
/// lines until the `250 ` finisher. Returns (code, lines).
pub fn read_smtp_reply<R: BufRead>(r: &mut R) -> Result<(u16, Vec<String>), String> {
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line).map_err(|e| format!("smtp read: {e}"))?;
        if n == 0 {
            return Err("smtp connection closed mid-reply".to_string());
        }
        let line = line.trim_end().to_string();
        if line.len() < 3 || !line.as_bytes()[..3].iter().all(u8::is_ascii_digit) {
            return Err(format!("malformed smtp reply line: '{line}'"));
        }
        let code: u16 = line[..3].parse().map_err(|e| format!("smtp code: {e}"))?;
        let done = line.as_bytes().get(3) != Some(&b'-');
        lines.push(line);
        if done {
            return Ok((code, lines));
        }
    }
}

/// The hostname a `220 mail.acme.dev ESMTP …` greeting announces.
pub fn parse_banner_host(greeting_lines: &[String]) -> Option<String> {
    let first = greeting_lines.first()?;
    first
        .get(4..)?
        .split_whitespace()
        .next()
        .map(str::to_string)
}

/// Does an EHLO reply advertise STARTTLS?
pub fn ehlo_advertises_starttls(lines: &[String]) -> bool {
    lines.iter().any(|l| {
        l.get(4..)
            .map(|rest| rest.trim().eq_ignore_ascii_case("STARTTLS"))
            .unwrap_or(false)
    })
}

/// Production probes. LOOPBACK-only SMTP dialogues (the doctor's call
/// sites pass 127.0.0.1); the single off-box touch is the outbound-:25
/// TCP handshake (no dialogue) and the verified HTTPS handshake —
/// never a mail send (pre-mortem #1).
pub struct RealDoctorEnv;

const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn smtp_connect(host: &str, port: u16) -> Result<std::net::TcpStream, String> {
    use std::net::ToSocketAddrs;
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?
        .next()
        .ok_or_else(|| format!("resolve {host}:{port}: no address"))?;
    let stream = std::net::TcpStream::connect_timeout(&addr, PROBE_TIMEOUT)
        .map_err(|e| format!("connect {host}:{port}: {e}"))?;
    let _ = stream.set_read_timeout(Some(PROBE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(PROBE_TIMEOUT));
    Ok(stream)
}

fn smtp_send(stream: &mut std::net::TcpStream, cmd: &str) -> Result<(), String> {
    use std::io::Write;
    stream
        .write_all(format!("{cmd}\r\n").as_bytes())
        .map_err(|e| format!("smtp write: {e}"))
}

impl DoctorEnv for RealDoctorEnv {
    fn public_ip(&self) -> Option<String> {
        use crate::mail::preflight::{PreflightEnv, RealPreflightEnv};
        RealPreflightEnv.public_ip()
    }

    fn outbound_25(&self) -> Result<(), String> {
        use crate::mail::preflight::{PreflightEnv, RealPreflightEnv};
        RealPreflightEnv.outbound_25()
    }

    fn smtp_ehlo(&self, host: &str, port: u16) -> Result<EhloInfo, String> {
        let mut stream = smtp_connect(host, port)?;
        let mut reader = std::io::BufReader::new(
            stream.try_clone().map_err(|e| format!("smtp clone: {e}"))?,
        );
        let (code, greeting) = read_smtp_reply(&mut reader)?;
        if code != 220 {
            return Err(format!("unexpected greeting code {code}"));
        }
        smtp_send(&mut stream, "EHLO k2-doctor.invalid")?;
        let (code, lines) = read_smtp_reply(&mut reader)?;
        if code != 250 {
            return Err(format!("EHLO refused ({code})"));
        }
        let info = EhloInfo {
            starttls: ehlo_advertises_starttls(&lines),
            banner_host: parse_banner_host(&greeting),
        };
        let _ = smtp_send(&mut stream, "QUIT");
        Ok(info)
    }

    fn open_relay_test(&self, host: &str, port: u16) -> Result<RelayVerdict, String> {
        let mut stream = smtp_connect(host, port)?;
        let mut reader = std::io::BufReader::new(
            stream.try_clone().map_err(|e| format!("smtp clone: {e}"))?,
        );
        let (code, _) = read_smtp_reply(&mut reader)?;
        if code != 220 {
            return Err(format!("unexpected greeting code {code}"));
        }
        smtp_send(&mut stream, "EHLO k2-doctor.invalid")?;
        let (code, _) = read_smtp_reply(&mut reader)?;
        if code != 250 {
            return Err(format!("EHLO refused ({code})"));
        }
        // Foreign sender + foreign recipient, unauthenticated. NEVER a
        // DATA — the dialogue ends at the RCPT verdict.
        smtp_send(&mut stream, "MAIL FROM:<relay-probe@k2-doctor.invalid>")?;
        let (mail_code, _) = read_smtp_reply(&mut reader)?;
        if mail_code >= 400 {
            // Refusing the foreign MAIL FROM outright is a (strict)
            // refusal to relay.
            let _ = smtp_send(&mut stream, "QUIT");
            return Ok(RelayVerdict::Refused(mail_code));
        }
        smtp_send(&mut stream, "RCPT TO:<relay-probe@example.com>")?;
        let (rcpt_code, _) = read_smtp_reply(&mut reader)?;
        let _ = smtp_send(&mut stream, "RSET");
        let _ = smtp_send(&mut stream, "QUIT");
        if (200..300).contains(&(rcpt_code as u32)) {
            Ok(RelayVerdict::AcceptedRelay)
        } else {
            Ok(RelayVerdict::Refused(rcpt_code))
        }
    }

    fn https_cert(&self, hostname: &str) -> Result<(), String> {
        // Default-verifying TLS stack (rustls roots) — a completed
        // handshake means a trusted, unexpired, name-matching cert.
        // Any HTTP status is fine; only transport/TLS failures matter.
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        client
            .get(format!("https://{hostname}/"))
            .send()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn disk_free_bytes(&self) -> Option<u64> {
        use crate::mail::preflight::{PreflightEnv, RealPreflightEnv};
        RealPreflightEnv.disk_free_bytes()
    }
}

// ── Persistence + the production entry points ──────────────────────────

#[derive(Debug)]
pub enum DocError {
    Usage(String),
    NotFound(String),
    NotReady(String),
    Engine(String),
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Persist one report as a `mail_doctor_runs` row; returns the wire
/// JSON (`{id, grade, ranAt}` + the results).
pub fn persist_run(
    domain_id: Option<&str>,
    report: &DoctorReport,
) -> Result<serde_json::Value, String> {
    let id = format!("mdr_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let ran_at = now_secs();
    let results = report.results_json();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_doctor_runs (id, domain_id, results_json, grade, ran_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, domain_id, results.to_string(), report.grade, ran_at],
        )
        .map_err(|e| format!("doctor run insert: {e}"))?;
    }
    let mut out = results;
    out["id"] = serde_json::json!(id);
    out["ranAt"] = serde_json::json!(ran_at);
    out["ok"] = serde_json::json!(true);
    Ok(out)
}

/// PRODUCTION runner behind `POST /cli/mail/doctor` (and, later, the
/// nightly loop): real resolver + real probes — never reached from
/// tests (the route gates non-Linux; every test drives [`run_checks`]
/// with fakes).
pub fn run(raw_domain: Option<&str>) -> Result<serde_json::Value, DocError> {
    // Server context (the singleton row): the doctor needs an
    // installed server with a hostname to reason about.
    let ctx: Option<(String, Option<String>, Option<String>)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT status, hostname, port_plan FROM mail_server WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
    };
    let Some((status, hostname, port_plan)) = ctx else {
        return Err(DocError::NotReady(
            "the email server is not installed — enable it in Settings → Email first"
                .to_string(),
        ));
    };
    let hostname = hostname.filter(|h| !h.trim().is_empty()).ok_or_else(|| {
        DocError::NotReady(
            "the mail server has no hostname on file — re-run Enable in Settings → Email"
                .to_string(),
        )
    })?;
    let ctx = ServerCtx { hostname, port_plan, status };

    // Domain context (optional).
    let mut domain_ctx: Option<(String, DomainCtx)> = None;
    if let Some(raw) = raw_domain {
        let domain =
            k2_core::mail_domain::normalize_mail_domain(raw).map_err(DocError::Usage)?;
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(row) = domains::load_domain(&conn, &domain) else {
            return Err(DocError::NotFound(format!(
                "domain '{domain}' is not hosted here"
            )));
        };
        let rows = domains::effective_rows(&conn, &row);
        domain_ctx = Some((row.id.clone(), DomainCtx { domain, rows }));
    }

    let resolver = dns_verify::SystemResolver::new().map_err(DocError::Engine)?;
    let env = RealDoctorEnv;
    let now = now_secs();
    let (domain_id, mut dctx) = match domain_ctx {
        Some((id, d)) => (Some(id), Some(d)),
        None => (None, None),
    };
    let report = run_checks(&resolver, &env, &ctx, dctx.as_mut(), now);
    persist_run(domain_id.as_deref(), &report).map_err(DocError::Engine)
}

/// The most recent stored run for `domain` (`None` = server-level).
/// `Ok(None)` = no run on file. Reads ONLY — the Settings card and the
/// GET route never trigger probes.
pub fn latest_run_json(raw_domain: Option<&str>) -> Result<Option<serde_json::Value>, DocError> {
    let domain_id: Option<String> = match raw_domain {
        None => None,
        Some(raw) => {
            let domain =
                k2_core::mail_domain::normalize_mail_domain(raw).map_err(DocError::Usage)?;
            let db = k2_core::db::shared();
            let conn = db.lock();
            let Some(row) = domains::load_domain(&conn, &domain) else {
                return Err(DocError::NotFound(format!(
                    "domain '{domain}' is not hosted here"
                )));
            };
            Some(row.id)
        }
    };
    let row: Option<(String, String, String, i64)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let sql_null = "SELECT id, results_json, grade, ran_at FROM mail_doctor_runs \
                        WHERE domain_id IS NULL ORDER BY ran_at DESC, rowid DESC LIMIT 1";
        let sql_dom = "SELECT id, results_json, grade, ran_at FROM mail_doctor_runs \
                       WHERE domain_id = ?1 ORDER BY ran_at DESC, rowid DESC LIMIT 1";
        match &domain_id {
            None => conn
                .query_row(sql_null, [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .ok(),
            Some(id) => conn
                .query_row(sql_dom, rusqlite::params![id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .ok(),
        }
    };
    Ok(row.map(|(id, results, grade, ran_at)| {
        let mut v: serde_json::Value =
            serde_json::from_str(&results).unwrap_or_else(|_| serde_json::json!({}));
        v["id"] = serde_json::json!(id);
        v["grade"] = serde_json::json!(grade);
        v["ranAt"] = serde_json::json!(ran_at);
        v
    }))
}

/// The gate `config::set_send_mode` consults before allowing
/// `direct` (PRD §8.3/§9): requires a stored SERVER-LEVEL run whose
/// grade is not `fail`. The refusal TEACHES — failing checks,
/// provider realities, and the relay escape hatch.
pub fn direct_send_gate() -> Result<serde_json::Value, String> {
    let run = latest_run_json(None).map_err(|e| match e {
        DocError::Usage(h) | DocError::NotFound(h) | DocError::NotReady(h)
        | DocError::Engine(h) => h,
    })?;
    let Some(run) = run else {
        return Err(
            "direct mode is locked until the deliverability doctor grades this box — \
             run `k2 mail doctor` first (direct unlocks on a non-failing grade)"
                .to_string(),
        );
    };
    let grade = run["grade"].as_str().unwrap_or("fail");
    if grade == "fail" {
        let blockers: Vec<String> = run["directBlockers"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        return Err(format!(
            "direct mode is locked — the last doctor run failed on: {}. Provider \
             realities if outbound port 25 is the blocker: GCP never unblocks it, \
             DigitalOcean effectively never, Hetzner takes a support ticket + ~1 month. \
             Relay mode works everywhere: `k2 mail config --domain <domain> --send-mode \
             relay`. Re-run `k2 mail doctor` after fixing.",
            if blockers.is_empty() { "(unrecorded checks)".to_string() } else { blockers.join(", ") }
        ));
    }
    Ok(serde_json::json!({ "grade": grade, "ranAt": run["ranAt"] }))
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — canned resolver + recording env (no network)
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::Ipv4Addr;

    // ── Fakes ──

    #[derive(Default)]
    struct FakeDns {
        a: HashMap<String, Vec<Ipv4Addr>>,
        ptr: HashMap<String, Vec<String>>,
        txt: HashMap<String, Vec<Vec<String>>>,
        mx: HashMap<String, Vec<dns_verify::MxHost>>,
        broken: Vec<String>,
    }

    impl DnsResolver for FakeDns {
        fn mx(&self, name: &str) -> Result<Vec<dns_verify::MxHost>, DnsError> {
            if self.broken.iter().any(|b| b == name) {
                return Err(DnsError::Other("timeout".into()));
            }
            self.mx.get(name).cloned().ok_or(DnsError::NotFound)
        }
        fn txt(&self, name: &str) -> Result<Vec<Vec<String>>, DnsError> {
            if self.broken.iter().any(|b| b == name) {
                return Err(DnsError::Other("timeout".into()));
            }
            self.txt.get(name).cloned().ok_or(DnsError::NotFound)
        }
        fn a(&self, name: &str) -> Result<Vec<Ipv4Addr>, DnsError> {
            if self.broken.iter().any(|b| b == name) {
                return Err(DnsError::Other("timeout".into()));
            }
            self.a.get(name).cloned().ok_or(DnsError::NotFound)
        }
        fn ptr(&self, ip: std::net::IpAddr) -> Result<Vec<String>, DnsError> {
            let key = ip.to_string();
            if self.broken.iter().any(|b| b == &key) {
                return Err(DnsError::Other("timeout".into()));
            }
            self.ptr.get(&key).cloned().ok_or(DnsError::NotFound)
        }
    }

    struct FakeEnv {
        ip: Option<&'static str>,
        outbound_25: Result<(), &'static str>,
        ehlo: Result<EhloInfo, &'static str>,
        relay: Result<RelayVerdict, &'static str>,
        cert: Result<(), &'static str>,
        disk: Option<u64>,
    }

    impl Default for FakeEnv {
        fn default() -> Self {
            Self {
                ip: Some("203.0.113.7"),
                outbound_25: Ok(()),
                ehlo: Ok(EhloInfo {
                    starttls: true,
                    banner_host: Some("mail.acme.dev".into()),
                }),
                relay: Ok(RelayVerdict::Refused(554)),
                cert: Ok(()),
                disk: Some(40 * 1024 * 1024 * 1024),
            }
        }
    }

    impl DoctorEnv for FakeEnv {
        fn public_ip(&self) -> Option<String> {
            self.ip.map(str::to_string)
        }
        fn outbound_25(&self) -> Result<(), String> {
            self.outbound_25.map_err(str::to_string)
        }
        fn smtp_ehlo(&self, host: &str, _port: u16) -> Result<EhloInfo, String> {
            assert_eq!(host, "127.0.0.1", "SMTP probes are loopback-only");
            self.ehlo.clone().map_err(str::to_string)
        }
        fn open_relay_test(&self, host: &str, _port: u16) -> Result<RelayVerdict, String> {
            assert_eq!(host, "127.0.0.1", "the relay self-test is loopback-only");
            self.relay.clone().map_err(str::to_string)
        }
        fn https_cert(&self, _hostname: &str) -> Result<(), String> {
            self.cert.map_err(str::to_string)
        }
        fn disk_free_bytes(&self) -> Option<u64> {
            self.disk
        }
    }

    fn ctx() -> ServerCtx {
        ServerCtx {
            hostname: "mail.acme.dev".into(),
            port_plan: Some("tls-alpn".into()),
            status: "running".into(),
        }
    }

    /// A DNS fake where the box's posture is fully healthy.
    fn healthy_dns() -> FakeDns {
        let mut dns = FakeDns::default();
        dns.ptr.insert("203.0.113.7".into(), vec!["mail.acme.dev".into()]);
        dns.a.insert("mail.acme.dev".into(), vec!["203.0.113.7".parse().unwrap()]);
        // DNSBL zones answer NotFound by default (absent from `a`).
        dns
    }

    fn check<'a>(r: &'a DoctorReport, id: &str) -> &'a DoctorCheck {
        r.checks.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("check {id}"))
    }

    // ── The healthy box ──

    #[test]
    fn healthy_box_grades_pass_with_every_check_present() {
        let r = run_checks(&healthy_dns(), &FakeEnv::default(), &ctx(), None, 1000);
        assert_eq!(r.grade, ST_PASS, "{:?}", r.checks);
        assert!(r.direct_blockers.is_empty());
        for id in [
            "server-state",
            "public-ip",
            "ptr",
            "fcrdns",
            "smtp-banner",
            "outbound-25",
            "dnsbl:zen.spamhaus.org",
            "dnsbl:b.barracudacentral.org",
            "dnsbl:bl.spamcop.net",
            "dnsbl:dnsbl-1.uceprotect.net",
            "open-relay",
            "starttls-25",
            "starttls-587",
            "tls-cert",
            "disk",
        ] {
            assert!(r.checks.iter().any(|c| c.id == id), "missing check {id}");
        }
        assert_eq!(check(&r, "ptr").status, ST_PASS);
        assert_eq!(check(&r, "fcrdns").status, ST_PASS);
        assert_eq!(check(&r, "open-relay").status, ST_PASS);
        assert_eq!(check(&r, "tls-cert").status, ST_PASS);
        // §9.1 postmaster hygiene always rides the coaching card.
        assert!(r.coaching.iter().any(|c| c.contains("Postmaster Tools")), "{:?}", r.coaching);
        // JSON shape.
        let v = r.results_json();
        assert_eq!(v["grade"], "pass");
        assert_eq!(v["hostname"], "mail.acme.dev");
        assert_eq!(v["ip"], "203.0.113.7");
        assert!(v["domain"].is_null());
    }

    // ── Blocked outbound 25 → fail + the provider coaching ──

    #[test]
    fn blocked_outbound_25_fails_the_grade_with_provider_coaching() {
        let env = FakeEnv { outbound_25: Err("connection to aspmx.l.google.com:25 timed out"), ..FakeEnv::default() };
        let r = run_checks(&healthy_dns(), &env, &ctx(), None, 1000);
        assert_eq!(r.grade, ST_FAIL);
        assert_eq!(r.direct_blockers, vec!["outbound-25".to_string()]);
        let c = check(&r, "outbound-25");
        assert_eq!(c.status, ST_FAIL);
        for needle in ["GCP", "DigitalOcean", "Hetzner", "Linode", "--send-mode relay"] {
            assert!(c.detail.contains(needle), "coaching must mention {needle}: {}", c.detail);
        }
        assert!(r.coaching.iter().any(|c| c.contains("GCP")));
    }

    // ── PTR / FCrDNS ──

    #[test]
    fn ptr_mismatch_and_missing_block_direct_with_provider_prose() {
        // Wrong PTR name.
        let mut dns = healthy_dns();
        dns.ptr.insert("203.0.113.7".into(), vec!["static.203-0-113-7.provider.example".into()]);
        let r = run_checks(&dns, &FakeEnv::default(), &ctx(), None, 1000);
        assert_eq!(r.grade, ST_FAIL);
        let c = check(&r, "ptr");
        assert_eq!(c.status, ST_FAIL);
        assert!(c.detail.contains("VPS PROVIDER"), "{}", c.detail);
        assert!(c.detail.contains("relay mode works without it"), "{}", c.detail);
        assert_eq!(check(&r, "fcrdns").status, ST_FAIL, "unconfirmable when PTR is wrong");

        // Missing PTR entirely.
        let mut dns = healthy_dns();
        dns.ptr.clear();
        let r = run_checks(&dns, &FakeEnv::default(), &ctx(), None, 1000);
        assert!(r.direct_blockers.contains(&"ptr".to_string()));

        // PTR right but the A record points elsewhere → FCrDNS fail.
        let mut dns = healthy_dns();
        dns.a.insert("mail.acme.dev".into(), vec!["198.51.100.9".parse().unwrap()]);
        let r = run_checks(&dns, &FakeEnv::default(), &ctx(), None, 1000);
        assert_eq!(check(&r, "ptr").status, ST_PASS);
        assert_eq!(check(&r, "fcrdns").status, ST_FAIL);
        assert!(check(&r, "fcrdns").detail.contains("198.51.100.9"), "live value shown");

        // Resolver trouble → unknown, grade warn (never a hard fail).
        let mut dns = healthy_dns();
        dns.broken.push("203.0.113.7".into());
        let r = run_checks(&dns, &FakeEnv::default(), &ctx(), None, 1000);
        assert_eq!(check(&r, "ptr").status, ST_UNKNOWN);
        assert_eq!(r.grade, ST_WARN);
    }

    // ── DNSBLs ──

    #[test]
    fn dnsbl_listings_fail_pbl_coaches_and_uceprotect_stays_informational() {
        assert_eq!(reverse_ipv4("203.0.113.7").as_deref(), Some("7.113.0.203"));
        assert_eq!(reverse_ipv4("not-an-ip"), None);

        // Barracuda listing → fail; UCEPROTECT listing → info, never
        // grading; zen PBL code → fail + self-service coaching.
        let mut dns = healthy_dns();
        dns.a.insert(
            "7.113.0.203.b.barracudacentral.org".into(),
            vec!["127.0.0.2".parse().unwrap()],
        );
        dns.a.insert(
            "7.113.0.203.dnsbl-1.uceprotect.net".into(),
            vec!["127.0.0.2".parse().unwrap()],
        );
        dns.a.insert(
            "7.113.0.203.zen.spamhaus.org".into(),
            vec!["127.0.0.10".parse().unwrap()],
        );
        let r = run_checks(&dns, &FakeEnv::default(), &ctx(), None, 1000);
        assert_eq!(r.grade, ST_FAIL);
        assert!(r.direct_blockers.contains(&"dnsbl:b.barracudacentral.org".to_string()));
        assert!(r.direct_blockers.contains(&"dnsbl:zen.spamhaus.org".to_string()));
        assert!(
            !r.direct_blockers.contains(&"dnsbl:dnsbl-1.uceprotect.net".to_string()),
            "informational lists never block"
        );
        let uce = check(&r, "dnsbl:dnsbl-1.uceprotect.net");
        assert_eq!(uce.status, ST_INFO);
        assert!(uce.detail.contains("don't panic"), "{}", uce.detail);
        let zen = check(&r, "dnsbl:zen.spamhaus.org");
        assert!(zen.detail.contains("PBL"), "{}", zen.detail);
        assert!(zen.detail.contains("check.spamhaus.org"), "{}", zen.detail);
        assert!(r.coaching.iter().any(|c| c.contains("PBL")));

        // Spamhaus answering an open resolver (127.255.x) → unknown.
        let mut dns = healthy_dns();
        dns.a.insert(
            "7.113.0.203.zen.spamhaus.org".into(),
            vec!["127.255.255.254".parse().unwrap()],
        );
        let r = run_checks(&dns, &FakeEnv::default(), &ctx(), None, 1000);
        let zen = check(&r, "dnsbl:zen.spamhaus.org");
        assert_eq!(zen.status, ST_UNKNOWN);
        assert!(zen.detail.contains("resolver"), "{}", zen.detail);
        assert_eq!(r.grade, ST_WARN);
    }

    // ── Open relay (pre-mortem #3: can't-run = fail) ──

    #[test]
    fn open_relay_accepted_or_unrunnable_both_fail_the_grade() {
        let env = FakeEnv { relay: Ok(RelayVerdict::AcceptedRelay), ..FakeEnv::default() };
        let r = run_checks(&healthy_dns(), &env, &ctx(), None, 1000);
        assert_eq!(r.grade, ST_FAIL);
        assert!(check(&r, "open-relay").detail.contains("OPEN RELAY"));

        let env = FakeEnv { relay: Err("connect 127.0.0.1:25: refused"), ..FakeEnv::default() };
        let r = run_checks(&healthy_dns(), &env, &ctx(), None, 1000);
        assert_eq!(r.grade, ST_FAIL, "fail-closed when the self-test cannot run");
        assert!(check(&r, "open-relay").detail.contains("fail-closed"));
    }

    // ── Soft checks ──

    #[test]
    fn soft_checks_warn_without_blocking_direct() {
        let env = FakeEnv {
            ehlo: Ok(EhloInfo { starttls: false, banner_host: Some("wrong.example".into()) }),
            cert: Err("certificate has expired"),
            disk: Some(500 * 1024 * 1024),
            ..FakeEnv::default()
        };
        let r = run_checks(&healthy_dns(), &env, &ctx(), None, 1000);
        assert_eq!(r.grade, ST_PASS, "soft checks never gate direct: {:?}", r.direct_blockers);
        assert_eq!(check(&r, "smtp-banner").status, ST_WARN);
        assert_eq!(check(&r, "starttls-25").status, ST_WARN);
        assert_eq!(check(&r, "tls-cert").status, ST_WARN);
        assert_eq!(check(&r, "disk").status, ST_WARN);

        // Plans B/C report the cert as info (owner's proxy owns it).
        let ctx_c = ServerCtx { port_plan: Some("http-01".into()), ..ctx() };
        let r = run_checks(&healthy_dns(), &FakeEnv::default(), &ctx_c, None, 1000);
        assert_eq!(check(&r, "tls-cert").status, ST_INFO);
    }

    // ── Domain checks ──

    fn domain_ctx() -> (FakeDns, DomainCtx) {
        // Build the effective rows through the real S2 fixture path.
        let rows = crate::mail::domains::tests::fixture_rows();
        let mut dns = healthy_dns();
        let expect = |id: &str| {
            rows.iter().find(|r| r.id == id).unwrap().expected.clone()
        };
        dns.mx.insert(
            "acme.dev".into(),
            vec![dns_verify::MxHost { preference: 10, exchange: "mail.acme.dev.".into() }],
        );
        dns.txt.insert("acme.dev".into(), vec![vec![expect("spf")]]);
        dns.txt.insert(
            "202601e._domainkey.acme.dev".into(),
            vec![vec![expect("dkim:202601e")]],
        );
        dns.txt.insert(
            "202601r._domainkey.acme.dev".into(),
            vec![vec![expect("dkim:202601r")]],
        );
        dns.txt.insert(
            "_dmarc.acme.dev".into(),
            vec![vec![expect("dmarc")]],
        );
        (dns, DomainCtx { domain: "acme.dev".into(), rows })
    }

    #[test]
    fn domain_run_adds_posture_checks_and_gates_on_spf_dkim() {
        let (dns, mut d) = domain_ctx();
        let r = run_checks(&dns, &FakeEnv::default(), &ctx(), Some(&mut d), 1000);
        assert_eq!(r.grade, ST_PASS, "{:?}", r.checks);
        for id in ["mx", "spf", "dkim", "dmarc", "spf-lookups"] {
            assert!(r.checks.iter().any(|c| c.id == id), "missing {id}");
        }
        assert_eq!(r.domain.as_deref(), Some("acme.dev"));
        assert_eq!(r.results_json()["domain"], "acme.dev");

        // Break SPF at the registrar: spf gates direct, mx informs.
        let (mut dns, mut d) = domain_ctx();
        dns.txt.insert("acme.dev".into(), vec![vec!["v=spf1 include:other.example -all".into()]]);
        dns.mx.remove("acme.dev");
        let r = run_checks(&dns, &FakeEnv::default(), &ctx(), Some(&mut d), 1000);
        assert_eq!(r.grade, ST_FAIL);
        assert!(r.direct_blockers.contains(&"spf".to_string()));
        assert!(!r.direct_blockers.contains(&"mx".to_string()), "MX informs, never gates direct");
        assert_eq!(check(&r, "mx").status, ST_FAIL);
        assert!(check(&r, "spf").detail.contains("k2 mail domain show"), "points at the diff");

        // Missing DMARC nags (warn), never fails.
        let (mut dns, mut d) = domain_ctx();
        dns.txt.remove("_dmarc.acme.dev");
        let r = run_checks(&dns, &FakeEnv::default(), &ctx(), Some(&mut d), 1000);
        assert_eq!(check(&r, "dmarc").status, ST_WARN);
        assert_eq!(r.grade, ST_PASS);
    }

    #[test]
    fn spf_lookup_counter_flags_past_ten() {
        assert_eq!(spf_lookup_terms("v=spf1 mx -all"), 1);
        assert_eq!(spf_lookup_terms("v=spf1 ip4:1.2.3.0/24 -all"), 0, "ip4 costs nothing");
        assert_eq!(
            spf_lookup_terms("v=spf1 a mx ptr include:a.example exists:%{i}.b.example redirect=c.example ~all"),
            6
        );
        let fat = format!(
            "v=spf1 {} -all",
            (0..11).map(|i| format!("include:p{i}.example")).collect::<Vec<_>>().join(" ")
        );
        assert_eq!(spf_lookup_terms(&fat), 11);

        let (mut dns, mut d) = domain_ctx();
        dns.txt.insert("acme.dev".into(), vec![vec![fat.clone()]]);
        // The live (Wrong) value is what the world sees — the counter
        // reads it.
        let r = run_checks(&dns, &FakeEnv::default(), &ctx(), Some(&mut d), 1000);
        let c = check(&r, "spf-lookups");
        assert_eq!(c.status, ST_FAIL);
        assert!(c.detail.contains("flatten"), "{}", c.detail);
        assert!(r.direct_blockers.contains(&"spf-lookups".to_string()));
    }

    // ── SMTP reply parsing (the pure part of the real env) ──

    #[test]
    fn smtp_reply_parser_handles_multiline_and_garbage() {
        let mut cur = std::io::Cursor::new(
            b"250-mail.acme.dev\r\n250-PIPELINING\r\n250-STARTTLS\r\n250 HELP\r\n".to_vec(),
        );
        let (code, lines) = read_smtp_reply(&mut cur).expect("parse");
        assert_eq!(code, 250);
        assert_eq!(lines.len(), 4);
        assert!(ehlo_advertises_starttls(&lines));
        assert!(!ehlo_advertises_starttls(&lines[..2].to_vec()));

        let mut cur = std::io::Cursor::new(b"220 mail.acme.dev ESMTP Stalwart\r\n".to_vec());
        let (code, lines) = read_smtp_reply(&mut cur).expect("parse");
        assert_eq!(code, 220);
        assert_eq!(parse_banner_host(&lines).as_deref(), Some("mail.acme.dev"));

        let mut cur = std::io::Cursor::new(b"not smtp at all\r\n".to_vec());
        assert!(read_smtp_reply(&mut cur).is_err(), "garbage fails loudly");
        let mut cur = std::io::Cursor::new(b"250-never finishes\r\n".to_vec());
        assert!(read_smtp_reply(&mut cur).is_err(), "EOF mid-reply fails loudly");
    }

    // ── Persistence + latest + the direct gate ──

    fn clear_runs() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM mail_doctor_runs WHERE domain_id IS NULL", []);
        let _ = conn.execute(
            "DELETE FROM mail_doctor_runs WHERE domain_id = 'dom-doctor-test'",
            [],
        );
    }

    #[test]
    fn runs_persist_and_latest_reads_by_scope() {
        let _g = crate::mail::mail_server_test_lock();
        clear_runs();

        // Server-level run persists and reads back.
        let report = run_checks(&healthy_dns(), &FakeEnv::default(), &ctx(), None, 1000);
        let v = persist_run(None, &report).expect("persist");
        assert_eq!(v["ok"], true);
        assert!(v["id"].as_str().unwrap().starts_with("mdr_"));
        let latest = latest_run_json(None).expect("read").expect("run on file");
        assert_eq!(latest["id"], v["id"]);
        assert_eq!(latest["grade"], "pass");
        assert!(latest["checks"].as_array().unwrap().len() >= 15);

        // A failing run supersedes (newest wins), scoped runs don't
        // bleed into the server-level read.
        let env = FakeEnv { outbound_25: Err("blocked"), ..FakeEnv::default() };
        let report = run_checks(&healthy_dns(), &env, &ctx(), None, 1000);
        let _ = persist_run(None, &report).expect("persist 2");
        let (dns, mut d) = domain_ctx();
        let dom_report = run_checks(&dns, &FakeEnv::default(), &ctx(), Some(&mut d), 1000);
        let _ = persist_run(Some("dom-doctor-test"), &dom_report).expect("persist domain");
        let latest = latest_run_json(None).expect("read").expect("run");
        assert_eq!(latest["grade"], "fail", "newest server-level run wins");
        assert!(latest["directBlockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == "outbound-25"));

        // Unknown domain → NotFound; garbage domain → Usage.
        assert!(matches!(latest_run_json(Some("ghost-doc.example")), Err(DocError::NotFound(_))));
        assert!(matches!(latest_run_json(Some("not a domain!")), Err(DocError::Usage(_))));
        clear_runs();
    }

    #[test]
    fn direct_gate_requires_a_non_failing_server_run() {
        let _g = crate::mail::mail_server_test_lock();
        clear_runs();

        // No run on file → locked, names the doctor verb.
        let err = direct_send_gate().expect_err("locked");
        assert!(err.contains("k2 mail doctor"), "{err}");

        // Failing run → locked with blockers + provider realities.
        let env = FakeEnv { outbound_25: Err("blocked"), ..FakeEnv::default() };
        let report = run_checks(&healthy_dns(), &env, &ctx(), None, 1000);
        let _ = persist_run(None, &report).expect("persist");
        let err = direct_send_gate().expect_err("locked");
        assert!(err.contains("outbound-25"), "{err}");
        assert!(err.contains("GCP") && err.contains("Hetzner"), "{err}");

        // Warn passes the gate (only fail locks).
        let mut dns = healthy_dns();
        dns.broken.push("203.0.113.7".into()); // PTR unknown → warn
        let report = run_checks(&dns, &FakeEnv::default(), &ctx(), None, 1000);
        assert_eq!(report.grade, ST_WARN);
        let _ = persist_run(None, &report).expect("persist");
        let v = direct_send_gate().expect("warn unlocks");
        assert_eq!(v["grade"], "warn");
        clear_runs();
    }

    #[test]
    fn production_run_refuses_without_an_installed_server() {
        let _g = crate::mail::mail_server_test_lock();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
        }
        match run(None) {
            Err(DocError::NotReady(hint)) => {
                assert!(hint.contains("Settings → Email"), "{hint}")
            }
            other => panic!("expected NotReady, got {other:?}"),
        }
    }
}
