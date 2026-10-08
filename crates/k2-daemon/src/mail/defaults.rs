//! Hosted-mail field fixes (K2 0.45.1, `prd-hostmail-field-fixes-0451-v1`,
//! amendments HF1–HF19): the few Stalwart settings K2 owns, healed by one
//! reconcile so existing boxes fix themselves after the update.
//!
//! Steps, in order, each logged and isolated (a failure in one never
//! blocks the others):
//!
//! 1. **A2 certificate names** — on a Stalwart-ACME tls-alpn box, lock the
//!    mail host's Domain `subjectAlternativeNames` to exactly `[host]`
//!    (an empty list orders five names in ONE order; one missing A record
//!    fails it). If the lock was written by THIS run, queue at most one
//!    `AcmeRenewal` — only when the served cert is missing or self-signed,
//!    the mail host points here, and no Pending/Retry AcmeRenewal exists
//!    for that Domain (HF8). A valid cert is never re-ordered (§10).
//! 2. **A4 spam rules** — Stalwart downloads the *latest* spam-filter rules
//!    at first start. v3.0.2 (live 2026-09-22 11:32 UTC) uses `bit_and`,
//!    which first exists in Stalwart 0.16.23; the 3 DNSBL rules then fail
//!    to load and every `ReloadSettings` is refused. K2 pins the rules URL
//!    to v3.0.1 (only while it is Stalwart's `releases/latest` default)
//!    and rewrites those 3 rules' `tag` to the v3.0.1 body — only when the
//!    INSTALLED Stalwart lacks `bit_and` (HF3). Runs BEFORE A1 so the
//!    reload that follows is not refused (HF6 correction).
//! 3. **A1 outbound** — the `mx` route goes `v4ThenV6` → `v4Only` ONCE
//!    (whole object written back minus `id`/`name`), recorded, never
//!    touched again (HF6). On Stalwart < 0.16.20 every `optional` DANE
//!    strategy goes `disable`, recorded per strategy; on ≥ 0.16.20 K2 puts
//!    back only what it recorded (HF5/HF7). A `disable` a person set is
//!    never touched.
//! 4. **One `ReloadSettings`** when anything above changed Stalwart's
//!    settings. If Stalwart refuses it, one restart through the usual
//!    doors (mail helper, then plain sudo); with no door the settings stay
//!    saved and `pendingReload` says so until a later reload lands.
//!
//! `mail_server.outbound_json` (migration 0139) records ONLY what K2
//! changed, each with `from`/`at` — status reads Stalwart for live state.
//!
//! Runs at boot ([`spawn_startup_reconcile`]), at the end of an enable and
//! after a successful `k2 hostmail upgrade` ([`spawn_after`]). Never
//! through hostmail disable/enable.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mail::cert_names::PointsHere;
use crate::mail::cert_owner::{CertMgmtApi, CertOwner};
use crate::mail::jmap::{sans_locked_to, CertManagement, StalwartClient};

const P: &str = "[mail/defaults]";

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn rfc3339(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| at.to_string())
}

// ── Stalwart versions ───────────────────────────────────────────────────

/// `(major, minor, patch)`.
pub type Ver = (u32, u32, u32);

/// 0.16.20 turns DANE off by itself when its resolver can't validate.
pub const V_DANE_SELF_OFF: Ver = (0, 16, 20);
/// `bit_and` first exists in the expression function table.
pub const V_BIT_AND: Ver = (0, 16, 23);
/// The DNSSEC resolver queries one nameserver at a time (hickory race).
pub const V_DNSSEC_RACE_FIXED: Ver = (0, 16, 23);

/// `0.16.10`, `v0.16.20`, `0.16.25-k2.1` → the numeric triple; anything
/// else is `None` (version-gated steps then skip, loudly).
pub fn parse_version(v: &str) -> Option<Ver> {
    let v = v.trim().trim_start_matches('v');
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c))
}

fn ver_str(v: Ver) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

// ── What K2 changed (mail_server.outbound_json) ─────────────────────────

/// Only what K2 itself changed in Stalwart, each with `from`/`at`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Markers {
    /// The `mx` route decision, made once (HF6). `to: None` = K2 found a
    /// non-default value and left it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mx_route: Option<RouteMark>,
    /// TLS strategies K2 set to `dane: disable` (HF7).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dane: Vec<DaneMark>,
    /// The spam-rules URL pin (HF4), made once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spam_url: Option<UrlMark>,
    /// DNSBL rules K2 rewrote to the v3.0.1 body.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spam_repair: Vec<RepairMark>,
    /// The one ACME order K2 queued after a fresh name lock (HF8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acme_retry: Option<AcmeRetryMark>,
    /// Settings K2 saved that Stalwart has not loaded yet (reload refused
    /// and no restart door). Cleared by the next reload that lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_reload: Option<PendingReload>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteMark {
    pub id: String,
    pub from: String,
    #[serde(default)]
    pub to: Option<String>,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaneMark {
    pub id: String,
    pub name: String,
    pub from: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UrlMark {
    pub from: String,
    pub to: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairMark {
    pub id: String,
    pub name: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcmeRetryMark {
    pub domain_id: String,
    pub task_id: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingReload {
    pub at: i64,
    pub error: String,
}

/// Parse `outbound_json`. NULL/empty = nothing changed yet; garbage is an
/// Err (the reconcile then skips the once-only steps instead of redoing
/// them blind).
pub fn parse_markers(raw: Option<&str>) -> Result<Markers, String> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(Markers::default()),
        Some(s) => serde_json::from_str(s)
            .map_err(|e| format!("mail_server.outbound_json is not readable ({e})")),
    }
}

pub fn load_markers() -> Result<Markers, String> {
    parse_markers(super::supervisor::row_field("outbound_json").as_deref())
}

fn save_markers(m: &Markers) -> Result<(), String> {
    let json = serde_json::to_string(m).map_err(|e| format!("outbound_json serialize: {e}"))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE mail_server SET outbound_json = ?1, updated_at = ?2 WHERE id = 1",
        rusqlite::params![json, now_secs()],
    )
    .map(|_| ())
    .map_err(|e| format!("outbound_json write: {e}"))
}

// ── Registry seam ───────────────────────────────────────────────────────

/// One `AcmeRenewal` task as Stalwart reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeTask {
    pub id: String,
    pub domain_id: String,
    /// `Pending` | `Retry` | `Failed`.
    pub state: String,
    /// `failureReason` (Retry / Failed).
    pub reason: Option<String>,
    /// `failedAt` (Failed) or `createdAt`.
    pub at: Option<String>,
    /// The next attempt (Pending / Retry).
    pub due: Option<String>,
}

impl AcmeTask {
    pub fn is_live(&self) -> bool {
        self.state == "Pending" || self.state == "Retry"
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "state": self.state,
            "reason": self.reason,
            "at": self.at,
            "due": self.due,
        })
    }
}

/// `x:Task/get` reply → its AcmeRenewal tasks. Other task types are
/// skipped (0.16.10 can't filter the query by type, so K2 reads them
/// all); an AcmeRenewal whose status `@type` K2 does not know, or a task
/// with no `@type`, is a loud Err.
pub fn parse_acme_tasks(args: &Value) -> Result<Vec<AcmeTask>, String> {
    let list = args
        .get("list")
        .and_then(|v| v.as_array())
        .ok_or("x:Task/get: reply has no 'list'")?;
    let mut out = Vec::new();
    for e in list {
        let id = e
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("x:Task/get: a task has no id")?;
        let typ = e
            .get("@type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("x:Task/get: task {id} has no '@type'"))?;
        if typ != "AcmeRenewal" {
            continue;
        }
        let domain_id = e
            .get("domainId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("x:Task/get: AcmeRenewal {id} has no domainId"))?;
        let status = e
            .get("status")
            .ok_or_else(|| format!("x:Task/get: AcmeRenewal {id} has no status"))?;
        let st = status.get("@type").and_then(|v| v.as_str()).unwrap_or("");
        let s = |k: &str| status.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let (state, reason, at, due) = match st {
            "Pending" => ("Pending", None, s("createdAt"), s("due")),
            "Retry" => ("Retry", s("failureReason"), s("createdAt"), s("due")),
            "Failed" => ("Failed", s("failureReason"), s("failedAt").or(s("createdAt")), None),
            other => {
                return Err(format!(
                    "x:Task/get: AcmeRenewal {id} has status '@type' '{other}' — K2 knows \
                     Pending/Retry/Failed only"
                ))
            }
        };
        out.push(AcmeTask {
            id: id.to_string(),
            domain_id: domain_id.to_string(),
            state: state.to_string(),
            reason,
            at,
            due,
        });
    }
    Ok(out)
}

/// The registry calls the reconcile makes. Production = [`StalwartClient`].
pub trait DefaultsApi: CertMgmtApi {
    fn mta_routes(&self) -> Result<Vec<Value>, String>;
    fn mta_route_set(&self, id: &str, obj: Value) -> Result<(), String>;
    fn tls_strategies(&self) -> Result<Vec<Value>, String>;
    fn tls_strategy_set(&self, id: &str, obj: Value) -> Result<(), String>;
    fn dnsbl_servers(&self) -> Result<Vec<Value>, String>;
    fn dnsbl_server_set(&self, id: &str, obj: Value) -> Result<(), String>;
    fn spam_rules_url(&self) -> Result<Option<String>, String>;
    fn set_spam_rules_url(&self, url: &str) -> Result<(), String>;
    fn reload_settings(&self) -> Result<(), String>;
    fn acme_tasks(&self) -> Result<Vec<AcmeTask>, String>;
    fn queue_acme_renewal(&self, domain_id: &str) -> Result<String, String>;
}

impl DefaultsApi for StalwartClient {
    fn mta_routes(&self) -> Result<Vec<Value>, String> {
        self.registry_list("MtaRoute")
    }
    fn mta_route_set(&self, id: &str, obj: Value) -> Result<(), String> {
        self.registry_update("MtaRoute", id, obj)
    }
    fn tls_strategies(&self) -> Result<Vec<Value>, String> {
        self.registry_list("MtaTlsStrategy")
    }
    fn tls_strategy_set(&self, id: &str, obj: Value) -> Result<(), String> {
        self.registry_update("MtaTlsStrategy", id, obj)
    }
    fn dnsbl_servers(&self) -> Result<Vec<Value>, String> {
        self.registry_list("SpamDnsblServer")
    }
    fn dnsbl_server_set(&self, id: &str, obj: Value) -> Result<(), String> {
        self.registry_update("SpamDnsblServer", id, obj)
    }
    fn spam_rules_url(&self) -> Result<Option<String>, String> {
        self.spam_filter_rules_url()
    }
    fn set_spam_rules_url(&self, url: &str) -> Result<(), String> {
        self.set_spam_filter_rules_url(url)
    }
    fn reload_settings(&self) -> Result<(), String> {
        self.action_reload_settings()
    }
    fn acme_tasks(&self) -> Result<Vec<AcmeTask>, String> {
        self.acme_renewal_tasks()
    }
    fn queue_acme_renewal(&self, domain_id: &str) -> Result<String, String> {
        StalwartClient::queue_acme_renewal(self, domain_id)
    }
}

/// A registry object written back whole, minus the read-only `id` and
/// `name` (HF6 correction: `name` is read-only, writing it is refused).
fn writable_body(obj: &Value) -> Value {
    let mut body = obj.clone();
    if let Some(m) = body.as_object_mut() {
        m.remove("id");
        m.remove("name");
    }
    body
}

fn str_field<'a>(obj: &'a Value, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(|v| v.as_str())
}

// ── A4: spam rules ──────────────────────────────────────────────────────

/// Stalwart's default: always the newest rules, whatever the server.
pub const RULES_URL_LATEST: &str =
    "https://github.com/stalwartlabs/spam-filter/releases/latest/download/spam-filter-rules.json.gz";
/// The newest rules 0.16.x can parse (v3.0.2 needs 0.16.23's `bit_and`).
/// Moves with the Stalwart pin (HF2: at the `0.16.25-k2.1` move, together
/// with dropping the repair).
pub const RULES_URL_PINNED: &str =
    "https://github.com/stalwartlabs/spam-filter/releases/download/v3.0.1/spam-filter-rules.json.gz";

/// The three DNSBL rules spam-filter v3.0.2 moved to `bit_and` (the only
/// objects that differ between v3.0.1 and v3.0.2, HF1).
pub const REPAIR_NAMES: [&str; 3] = [
    "STWT_SURBL_DOMAIN",
    "STWT_URIBL_DOMAIN",
    "STWT_SURBL_HASHBL_DOMAIN",
];

/// The v3.0.1 `tag` bodies, vendored (byte-checked in tests against the
/// checked-in release excerpt `fixtures/spam-filter-v3.0.1-dnsbl.json`).
/// Equality tests: a domain listed on two lists at once (a combined code)
/// gets no tag — what every pre-2026-09-22 box runs, and a rule that loads.
const V301_TAG_SURBL: &str = r#"{"else":"false","match":{"0":{"if":"octets[3] == 128","then":"'CRACKED_SURBL'"},"1":{"if":"octets[3] == 64","then":"'ABUSE_SURBL'"},"2":{"if":"octets[3] == 32","then":"'CT_SURBL'"},"3":{"if":"octets[3] == 16","then":"'MW_SURBL_MULTI'"},"4":{"if":"octets[3] == 8","then":"'PH_SURBL_MULTI'"},"5":{"if":"octets[3] == 4","then":"'DM_SURBL'"},"6":{"if":"octets[3] == 1","then":"'SURBL_BLOCKED'"}}}"#;
const V301_TAG_URIBL: &str = r#"{"else":"false","match":{"0":{"if":"octets[3] == 1","then":"'URIBL_BLOCKED'"},"1":{"if":"octets[3] == 2","then":"'URIBL_BLACK'"},"2":{"if":"octets[3] == 4","then":"'URIBL_GREY'"},"3":{"if":"octets[3] == 8","then":"'URIBL_RED'"}}}"#;
const V301_TAG_HASHBL: &str = r#"{"else":"false","match":{"0":{"if":"octets[0] != 127","then":"false"},"1":{"if":"octets[3] == 8","then":"'SURBL_HASHBL_PHISH'"},"2":{"if":"octets[3] == 16","then":"'SURBL_HASHBL_MALWARE'"},"3":{"if":"octets[3] == 64","then":"'SURBL_HASHBL_ABUSE'"},"4":{"if":"octets[3] == 128","then":"'SURBL_HASHBL_CRACKED'"},"5":{"if":"octets[2] == 1","then":"'SURBL_HASHBL_EMAIL'"}}}"#;

/// The vendored v3.0.1 `tag` for one of [`REPAIR_NAMES`].
pub fn v301_tag(name: &str) -> Option<Value> {
    let raw = match name {
        "STWT_SURBL_DOMAIN" => V301_TAG_SURBL,
        "STWT_URIBL_DOMAIN" => V301_TAG_URIBL,
        "STWT_SURBL_HASHBL_DOMAIN" => V301_TAG_HASHBL,
        _ => return None,
    };
    Some(serde_json::from_str(raw).expect("vendored v3.0.1 tag is valid JSON"))
}

/// Stalwart 0.16.10–0.16.22 expression functions (`FUNCTIONS` then
/// `ASYNC_FUNCTIONS`, `common/src/expr/functions/mod.rs`; identical at
/// 0.16.10 and 0.16.20). A name outside this list fails to load with
/// `Invalid variable or constant "<name>"`.
pub const STALWART_0_16_FUNCTIONS: &[&str] = &[
    "count", "sort", "dedup", "winnow", "is_intersect", "is_email", "email_part", "is_empty",
    "is_number", "is_ip_addr", "is_ipv4_addr", "is_ipv6_addr", "is_ip_in_cidr",
    "ip_reverse_name", "trim", "trim_end", "trim_start", "len", "to_lowercase",
    "to_uppercase", "is_uppercase", "is_lowercase", "has_digits", "count_spaces",
    "count_uppercase", "count_lowercase", "count_chars", "contains", "contains_ignore_case",
    "eq_ignore_case", "starts_with", "ends_with", "lines", "substring", "strip_prefix",
    "strip_suffix", "split", "rsplit", "split_once", "rsplit_once", "split_n", "split_words",
    "hash", "if_then", "is_local_domain", "is_local_address", "key_get", "key_exists",
    "key_set", "counter_incr", "counter_get", "dns_query", "sql_query",
];

/// The function table of the INSTALLED Stalwart: 0.16.x before 0.16.23
/// is [`STALWART_0_16_FUNCTIONS`]; from 0.16.23 it also has `bit_and`.
pub fn known_functions(installed: Ver) -> Vec<&'static str> {
    let mut v = STALWART_0_16_FUNCTIONS.to_vec();
    if installed >= V_BIT_AND {
        v.push("bit_and");
    }
    v
}

/// Function names called in one Stalwart expression string: an
/// identifier followed (after optional spaces) by `(`, outside quotes.
pub fn called_functions(expr: &str) -> Vec<String> {
    let b = expr.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\'' || c == b'"' {
            // Skip a quoted literal (backslash escapes).
            let q = c;
            i += 1;
            while i < b.len() && b[i] != q {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        let ident_start = c.is_ascii_alphabetic() || c == b'_';
        let prev_ok = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'$' || b[i - 1] == b'.');
        if ident_start && prev_ok {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let mut j = i;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            if j < b.len() && b[j] == b'(' {
                let name = &expr[start..i];
                if !out.iter().any(|n: &String| n == name) {
                    out.push(name.to_string());
                }
            }
            continue;
        }
        i += 1;
    }
    out
}

/// Every expression string inside a registry object: any object carrying
/// `else` + `match` is a Stalwart Expression (`match` is index-keyed or a
/// list of `{if, then}`).
pub fn expression_strings(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_exprs(v, &mut out);
    out
}

fn collect_exprs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            if m.contains_key("else") && m.contains_key("match") {
                if let Some(e) = m.get("else").and_then(|x| x.as_str()) {
                    out.push(e.to_string());
                }
                let arms: Vec<&Value> = match m.get("match") {
                    Some(Value::Object(o)) => o.values().collect(),
                    Some(Value::Array(a)) => a.iter().collect(),
                    _ => Vec::new(),
                };
                for arm in arms {
                    for k in ["if", "then"] {
                        if let Some(s) = arm.get(k).and_then(|x| x.as_str()) {
                            out.push(s.to_string());
                        }
                    }
                }
                return;
            }
            for x in m.values() {
                collect_exprs(x, out);
            }
        }
        Value::Array(a) => {
            for x in a {
                collect_exprs(x, out);
            }
        }
        _ => {}
    }
}

/// Functions an object calls that `known` lacks.
pub fn unknown_functions(obj: &Value, known: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in expression_strings(obj) {
        for f in called_functions(&e) {
            if !known.contains(&f.as_str()) && !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

/// One DNSBL rule K2 rewrites.
#[derive(Debug, Clone, PartialEq)]
pub struct RepairWrite {
    pub id: String,
    pub name: String,
    pub body: Value,
}

/// A4 plan (pure, HF3): rewrite `tag` to the v3.0.1 body for each of the
/// 3 known rules whose stored expressions call `bit_and` — only when the
/// installed Stalwart lacks it. Unknown rules are never written (the
/// doctor names them).
pub fn plan_spam_repair(servers: &[Value], installed: Ver) -> Vec<RepairWrite> {
    if installed >= V_BIT_AND {
        return Vec::new();
    }
    let mut out = Vec::new();
    for s in servers {
        let (Some(id), Some(name)) = (str_field(s, "id"), str_field(s, "name")) else {
            continue;
        };
        let Some(tag) = v301_tag(name) else {
            continue;
        };
        let uses_bit_and = s
            .get("tag")
            .map(|t| unknown_functions(t, STALWART_0_16_FUNCTIONS).iter().any(|f| f == "bit_and"))
            .unwrap_or(false);
        if !uses_bit_and {
            continue;
        }
        let mut body = writable_body(s);
        body["tag"] = tag;
        out.push(RepairWrite { id: id.to_string(), name: name.to_string(), body });
    }
    out
}

/// A4 URL pin (pure, HF4): only while the URL is Stalwart's `latest`
/// default and K2 has not pinned it before.
pub fn plan_spam_url(current: Option<&str>, mark: Option<&UrlMark>) -> Option<&'static str> {
    if mark.is_some() {
        return None;
    }
    (current.map(str::trim) == Some(RULES_URL_LATEST)).then_some(RULES_URL_PINNED)
}

// ── A1: outbound route + DANE ───────────────────────────────────────────

/// The `mx` route decision.
#[derive(Debug, Clone, PartialEq)]
pub enum RoutePlan {
    /// Default `v4ThenV6` → write `v4Only` (whole object, minus id/name).
    Write { id: String, body: Value, from: String },
    /// A non-default value a person (or K2 earlier) set: record, never touch.
    Record { id: String, from: String },
    Skip(String),
}

/// A1 route plan (pure, HF6): decided once per route id.
pub fn plan_mx_route(routes: &[Value], mark: Option<&RouteMark>) -> RoutePlan {
    let mx = routes
        .iter()
        .find(|r| str_field(r, "@type") == Some("Mx") && str_field(r, "name") == Some("mx"));
    let Some(mx) = mx else {
        return RoutePlan::Skip("no Mx route named 'mx'".into());
    };
    let Some(id) = str_field(mx, "id") else {
        return RoutePlan::Skip("the 'mx' route has no id".into());
    };
    if mark.is_some_and(|m| m.id == id) {
        return RoutePlan::Skip("already decided once (outbound_json)".into());
    }
    let from = str_field(mx, "ipLookupStrategy").unwrap_or("").to_string();
    if from == "v4ThenV6" {
        let mut body = writable_body(mx);
        body["ipLookupStrategy"] = Value::String("v4Only".into());
        RoutePlan::Write { id: id.to_string(), body, from }
    } else {
        RoutePlan::Record { id: id.to_string(), from }
    }
}

/// One TLS strategy write.
#[derive(Debug, Clone, PartialEq)]
pub struct DaneWrite {
    pub id: String,
    pub name: String,
    pub to: &'static str,
    pub body: Value,
}

/// A1 DANE plan result.
#[derive(Debug, Clone, PartialEq)]
pub struct DanePlan {
    pub writes: Vec<DaneWrite>,
    /// The DANE markers after the writes land.
    pub marks: Vec<DaneMark>,
}

/// A1 DANE plan (pure, HF5/HF7). Below 0.16.20: every `optional` strategy
/// K2 has not decided before → `disable` (+ marker). From 0.16.20: put
/// back `optional` only where K2 recorded the change and the value is
/// still `disable`; markers are dropped either way. A strategy that is
/// gone drops its marker. A hand-set `disable` (no marker) is never
/// touched.
pub fn plan_dane(strategies: &[Value], marks: &[DaneMark], installed: Ver, now: i64) -> DanePlan {
    let live: Vec<(&str, &str, &str, &Value)> = strategies
        .iter()
        .filter_map(|s| {
            Some((str_field(s, "id")?, str_field(s, "name").unwrap_or(""), str_field(s, "dane").unwrap_or(""), s))
        })
        .collect();
    let mut marks: Vec<DaneMark> = marks
        .iter()
        .filter(|m| live.iter().any(|(id, ..)| *id == m.id))
        .cloned()
        .collect();
    let mut writes = Vec::new();
    if installed < V_DANE_SELF_OFF {
        for (id, name, dane, obj) in &live {
            if *dane != "optional" || marks.iter().any(|m| m.id == *id) {
                continue;
            }
            let mut body = writable_body(obj);
            body["dane"] = Value::String("disable".into());
            writes.push(DaneWrite { id: id.to_string(), name: name.to_string(), to: "disable", body });
            marks.push(DaneMark { id: id.to_string(), name: name.to_string(), from: "optional".into(), at: now });
        }
    } else {
        for m in &marks {
            if let Some((id, name, dane, obj)) = live.iter().find(|(id, ..)| *id == m.id) {
                if *dane == "disable" {
                    let mut body = writable_body(obj);
                    body["dane"] = Value::String("optional".into());
                    writes.push(DaneWrite { id: id.to_string(), name: name.to_string(), to: "optional", body });
                }
            }
        }
        marks.clear();
    }
    DanePlan { writes, marks }
}

// ── A2: certificate names + the one guarded retry ───────────────────────

/// What the A2 step decided.
#[derive(Debug, Clone, PartialEq)]
pub enum CertStep {
    Skipped(String),
    AlreadyLocked,
    /// Manual Domain: Stalwart ACME is off; nothing to lock.
    NotAutomatic,
    /// Lock written now (`previous` = the old list); the retry decision.
    Locked { previous: Vec<String>, retry: RetryDecision },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RetryDecision {
    Queued { domain_id: String, task_id: String },
    NotQueued(String),
}

/// Inputs the A2 step reads lazily (the TLS probe and DNS run only after
/// a fresh lock).
pub struct CertInputs<'a> {
    pub host: &'a str,
    pub owner: CertOwner,
    pub port_plan: Option<&'a str>,
    /// `missing` | `self-signed` | `issued` (status's TLS probe).
    pub served_state: &'a dyn Fn() -> String,
    /// Does the mail host point at this box?
    pub points_here: &'a dyn Fn() -> PointsHere,
}

/// A2 (HF8): lock + at most one guarded `AcmeRenewal`.
pub fn cert_step(api: &dyn DefaultsApi, inp: &CertInputs<'_>) -> Result<CertStep, String> {
    let host = inp.host.trim().trim_end_matches('.').to_ascii_lowercase();
    if inp.owner != CertOwner::StalwartAcme {
        return Ok(CertStep::Skipped(format!(
            "owner {} — K2 locks names only where Stalwart's own ACME issues the mail cert",
            inp.owner.as_str()
        )));
    }
    if inp.port_plan != Some("tls-alpn") {
        return Ok(CertStep::Skipped(format!(
            "port plan {} — Stalwart ACME runs on tls-alpn only",
            inp.port_plan.unwrap_or("unset")
        )));
    }
    let domain_id = api
        .mail_domain_id(&host)?
        .ok_or_else(|| format!("no Stalwart domain carries the mail hostname {host}"))?;
    let (provider, previous) = match api.get(&domain_id)? {
        CertManagement::Manual => return Ok(CertStep::NotAutomatic),
        CertManagement::Other(t) => {
            return Err(format!(
                "domain {domain_id} has certificateManagement '@type' '{t}' — not touching it"
            ))
        }
        CertManagement::Automatic { acme_provider_id, subject_alternative_names } => {
            (acme_provider_id, subject_alternative_names)
        }
    };
    if sans_locked_to(&previous, &host) {
        return Ok(CertStep::AlreadyLocked);
    }
    api.set_locked(&domain_id, &provider, &host)?;
    let retry = retry_decision(api, &domain_id, &host, inp);
    Ok(CertStep::Locked { previous, retry })
}

fn retry_decision(
    api: &dyn DefaultsApi,
    domain_id: &str,
    host: &str,
    inp: &CertInputs<'_>,
) -> RetryDecision {
    let served = (inp.served_state)();
    if served != "missing" && served != "self-signed" {
        return RetryDecision::NotQueued(format!(
            "the served certificate is {served} — never re-ordered; the next scheduled \
             renewal orders {host} only"
        ));
    }
    match (inp.points_here)() {
        PointsHere::Yes => {}
        PointsHere::No(d) => {
            return RetryDecision::NotQueued(format!(
                "{d} — add the A record for {host}, then `k2 hostmail cert renew`"
            ))
        }
        PointsHere::Unknown(d) => {
            return RetryDecision::NotQueued(format!(
                "could not tell whether {host} points here ({d}) — `k2 hostmail cert renew` \
                 once it does"
            ))
        }
    }
    let tasks = match api.acme_tasks() {
        Ok(t) => t,
        Err(e) => {
            return RetryDecision::NotQueued(format!(
                "could not read Stalwart's ACME tasks ({e}) — no order queued"
            ))
        }
    };
    if let Some(t) = tasks.iter().find(|t| t.domain_id == domain_id && t.is_live()) {
        return RetryDecision::NotQueued(format!(
            "Stalwart already has a {} ACME task ({}) for this domain — it picks up the \
             locked names itself",
            t.state, t.id
        ));
    }
    match api.queue_acme_renewal(domain_id) {
        Ok(task_id) => RetryDecision::Queued { domain_id: domain_id.to_string(), task_id },
        Err(e) => RetryDecision::NotQueued(format!("queueing the ACME order failed: {e}")),
    }
}

// ── The reconcile ───────────────────────────────────────────────────────

/// Everything the reconcile reads besides the registry.
pub struct Ctx<'a> {
    pub cert: CertInputs<'a>,
    /// `mail_server.installed_version`.
    pub installed: Option<&'a str>,
    pub now: i64,
}

/// The reconcile's result: log lines + the markers to persist.
#[derive(Debug)]
pub struct Outcome {
    pub lines: Vec<String>,
    pub markers: Markers,
    /// Settings writes that needed a reload (A4 + A1).
    pub settings_written: usize,
}

/// One pass of A2 → A4 → A1 → reload (pure over the seams). `markers` =
/// `None` when outbound_json is unreadable: the once-only steps (URL pin,
/// route, DANE) are then skipped, never redone blind.
pub fn reconcile(
    api: &dyn DefaultsApi,
    ctx: &Ctx<'_>,
    markers: Option<Markers>,
    restart: &mut dyn FnMut() -> Result<(), String>,
) -> Outcome {
    let mut lines = Vec::new();
    let markers_ok = markers.is_some();
    let mut m = markers.unwrap_or_default();
    if !markers_ok {
        lines.push(format!(
            "{P} outbound_json is unreadable — the once-only steps (rules URL, mx route, DANE) \
             are skipped; the spam repair and the cert lock still run"
        ));
    }
    let installed = ctx.installed.and_then(parse_version);
    let mut written = 0usize;

    // 1. A2 — certificate names (+ the one guarded retry).
    let host = ctx.cert.host;
    match cert_step(api, &ctx.cert) {
        Ok(CertStep::Skipped(why)) => lines.push(format!("{P} cert names {host}: skipped — {why}")),
        Ok(CertStep::AlreadyLocked) => {
            lines.push(format!("{P} cert names {host}: Stalwart ACME already orders {host} only"))
        }
        Ok(CertStep::NotAutomatic) => lines.push(format!(
            "{P} cert names {host}: Stalwart ACME is Manual for this domain — nothing to lock"
        )),
        Ok(CertStep::Locked { previous, retry }) => {
            let was = if previous.is_empty() {
                "the empty setup default (mail host + mta-sts, ua-auto-config, autoconfig, \
                 autodiscover in one order)"
                    .to_string()
            } else {
                previous.join(", ")
            };
            lines.push(format!("{P} cert names {host}: locked to {host} only (was {was})"));
            match retry {
                RetryDecision::Queued { domain_id, task_id } => {
                    lines.push(format!(
                        "{P} cert names {host}: queued ONE ACME order (task {task_id}) — no \
                         real certificate is served and {host} points here"
                    ));
                    m.acme_retry = Some(AcmeRetryMark { domain_id, task_id, at: ctx.now });
                }
                RetryDecision::NotQueued(why) => {
                    lines.push(format!("{P} cert names {host}: no ACME order queued — {why}"))
                }
            }
        }
        Err(e) => lines.push(format!("{P} cert names {host}: FAILED (retried next boot): {e}")),
    }

    // 2. A4 — spam rules (before A1: a bit_and rule refuses every reload).
    if markers_ok {
        match api.spam_rules_url() {
            Ok(cur) => {
                if let Some(to) = plan_spam_url(cur.as_deref(), m.spam_url.as_ref()) {
                    match api.set_spam_rules_url(to) {
                        Ok(()) => {
                            written += 1;
                            lines.push(format!(
                                "{P} spam rules URL pinned to v3.0.1 (was releases/latest)"
                            ));
                            m.spam_url = Some(UrlMark {
                                from: cur.unwrap_or_default(),
                                to: to.to_string(),
                                at: ctx.now,
                            });
                        }
                        Err(e) => lines.push(format!("{P} spam rules URL: FAILED: {e}")),
                    }
                }
            }
            Err(e) => lines.push(format!("{P} spam rules URL: could not read it: {e}")),
        }
    }
    match installed {
        None => lines.push(format!(
            "{P} spam rules: skipped — installed Stalwart version unknown ({})",
            ctx.installed.unwrap_or("none")
        )),
        Some(v) => match api.dnsbl_servers() {
            Ok(servers) => {
                for w in plan_spam_repair(&servers, v) {
                    match api.dnsbl_server_set(&w.id, w.body.clone()) {
                        Ok(()) => {
                            written += 1;
                            lines.push(format!(
                                "{P} spam rules: {} rewritten to the v3.0.1 body (Stalwart {} has no bit_and)",
                                w.name,
                                ver_str(v)
                            ));
                            m.spam_repair.retain(|r| r.id != w.id);
                            m.spam_repair.push(RepairMark { id: w.id, name: w.name, at: ctx.now });
                        }
                        Err(e) => lines.push(format!("{P} spam rules: {} FAILED: {e}", w.name)),
                    }
                }
            }
            Err(e) => lines.push(format!("{P} spam rules: could not read the DNSBL rules: {e}")),
        },
    }

    // 3. A1 — mx route (once) + DANE.
    if markers_ok {
        match api.mta_routes() {
            Ok(routes) => match plan_mx_route(&routes, m.mx_route.as_ref()) {
                RoutePlan::Write { id, body, from } => match api.mta_route_set(&id, body) {
                    Ok(()) => {
                        written += 1;
                        lines.push(format!("{P} mx route: {from} → v4Only (K2 sends over IPv4 only)"));
                        m.mx_route = Some(RouteMark { id, from, to: Some("v4Only".into()), at: ctx.now });
                    }
                    Err(e) => lines.push(format!("{P} mx route: FAILED (retried next boot): {e}")),
                },
                RoutePlan::Record { id, from } => {
                    lines.push(format!("{P} mx route: {from} set by hand — left as it is"));
                    m.mx_route = Some(RouteMark { id, from, to: None, at: ctx.now });
                }
                RoutePlan::Skip(_) => {}
            },
            Err(e) => lines.push(format!("{P} mx route: could not read routes: {e}")),
        }
        match installed {
            None => lines.push(format!("{P} DANE: skipped — installed Stalwart version unknown")),
            Some(v) => match api.tls_strategies() {
                Ok(strategies) => {
                    let plan = plan_dane(&strategies, &m.dane, v, ctx.now);
                    let mut failed: Vec<String> = Vec::new();
                    for w in &plan.writes {
                        match api.tls_strategy_set(&w.id, w.body.clone()) {
                            Ok(()) => {
                                written += 1;
                                lines.push(if w.to == "disable" {
                                    format!(
                                        "{P} DANE: '{}' optional → disable (Stalwart {} can't check \
                                         DNSSEC reliably; K2 turns it back on after the upgrade to \
                                         0.16.20)",
                                        w.name,
                                        ver_str(v)
                                    )
                                } else {
                                    format!("{P} DANE: '{}' back to optional (Stalwart {})", w.name, ver_str(v))
                                });
                            }
                            Err(e) => {
                                failed.push(w.id.clone());
                                lines.push(format!("{P} DANE: '{}' FAILED: {e}", w.name));
                            }
                        }
                    }
                    // A failed disable leaves no marker (nothing changed);
                    // a failed restore keeps its marker (retried next time).
                    let mut marks = plan.marks;
                    for w in plan.writes.iter().filter(|w| failed.contains(&w.id)) {
                        if w.to == "disable" {
                            marks.retain(|mk| mk.id != w.id);
                        } else if let Some(old) = m.dane.iter().find(|mk| mk.id == w.id) {
                            marks.push(old.clone());
                        }
                    }
                    m.dane = marks;
                }
                Err(e) => lines.push(format!("{P} DANE: could not read TLS strategies: {e}")),
            },
        }
    }

    // 4. One reload (restart only if Stalwart refuses it).
    if written > 0 || m.pending_reload.is_some() {
        match api.reload_settings() {
            Ok(()) => {
                m.pending_reload = None;
                lines.push(format!("{P} settings reloaded (no restart)"));
            }
            Err(reload_err) => match restart() {
                Ok(()) => {
                    m.pending_reload = None;
                    lines.push(format!(
                        "{P} Stalwart refused the reload ({reload_err}) — restarted it once to \
                         load the settings"
                    ));
                }
                Err(restart_err) => {
                    lines.push(format!(
                        "{P} settings SAVED but not live: reload refused ({reload_err}) and no \
                         restart ({restart_err})"
                    ));
                    m.pending_reload = Some(PendingReload {
                        at: ctx.now,
                        error: format!("reload: {reload_err}; restart: {restart_err}"),
                    });
                }
            },
        }
    }
    if lines.is_empty() {
        lines.push(format!("{P} nothing to change"));
    }
    Outcome { lines, markers: m, settings_written: written }
}

// ── Live wiring (boot, enable, upgrade) ─────────────────────────────────

/// Why the reconcile must not run, or `None` when it should (HF11): these
/// steps are registry-only, so a box whose row says `error` still heals
/// once the admin API answers. `disabled` and `installing` are skipped.
pub fn gate(status: Option<&str>, enable_completed: bool) -> Option<String> {
    let Some(status) = status else {
        return Some("hosted mail is not installed".into());
    };
    if !matches!(status, "running" | "degraded" | "stopped" | "error") {
        return Some(format!("hosted mail status is '{status}'"));
    }
    if !enable_completed {
        return Some("the enable has not completed".into());
    }
    None
}

/// Boot hook (main.rs, next to the cert-owner reconcile). Linux only; one
/// detached thread; panics contained; never blocks boot.
pub fn spawn_startup_reconcile() {
    spawn("boot");
}

/// After a completed enable or a successful `k2 hostmail upgrade`.
pub fn spawn_after(reason: &'static str) {
    spawn(reason);
}

fn spawn(reason: &'static str) {
    if !super::supervisor::mail_supported() || cfg!(test) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("mail-defaults".into())
        .spawn(move || {
            if std::panic::catch_unwind(|| run_live(reason)).is_err() {
                k2_core::log_debug!("{P} FAILED (daemon carries on): panicked ({reason})");
            }
        });
    if let Err(e) = spawned {
        k2_core::log_debug!("{P} FAILED (daemon carries on): thread spawn: {e}");
    }
}

const API_WAIT_STEP_SECS: u64 = 3;
/// ~10 min: covers the IMAP reconcile's Stalwart restart.
const API_WAIT_TRIES: u32 = 200;
/// HF4: how long the end of an enable waits for Stalwart's first-start
/// rules download to fill `SpamDnsblServer`.
const RULES_WAIT_SECS: u64 = 300;

struct EndEnable;
impl Drop for EndEnable {
    fn drop(&mut self) {
        super::supervisor::end_enable();
    }
}

/// Wait for the admin API, then take the enable lock (the cert-owner
/// pattern). `None` = gave up (logged).
fn wait_and_lock(client: &StalwartClient, reason: &str) -> Option<EndEnable> {
    use std::sync::atomic::Ordering;
    let mut up = false;
    let mut last_err = String::from("enable lock held");
    for _ in 0..API_WAIT_TRIES {
        if !super::supervisor::enable_running().load(Ordering::SeqCst) {
            if !up {
                match client.ping() {
                    Ok(()) => up = true,
                    Err(e) => last_err = e,
                }
            }
            if up && super::supervisor::try_begin_enable() {
                return Some(EndEnable);
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(API_WAIT_STEP_SECS));
    }
    k2_core::log_debug!("{P} skipped ({reason}; retried at the next boot): {last_err}");
    None
}

#[cfg_attr(test, allow(dead_code))]
fn run_live(reason: &'static str) {
    use super::supervisor;
    let status = supervisor::current_status();
    if let Some(why) = gate(status.as_deref(), supervisor::enable_completed()) {
        k2_core::log_debug!("{P} skipped ({reason}) because {why}");
        return;
    }
    let Some(host) = supervisor::row_field("hostname").filter(|h| !h.trim().is_empty()) else {
        k2_core::log_debug!("{P} skipped ({reason}) because the mail server has no hostname");
        return;
    };
    let client = match supervisor::mgmt_client_from_row() {
        Ok(c) => c,
        Err(e) => {
            k2_core::log_debug!("{P} skipped ({reason}) because no management client: {e}");
            return;
        }
    };
    if reason == "enable" {
        // HF4: pin the rules URL first (the first-start UpdateRules reads
        // it when it runs), then wait for the rules to land, lock released.
        if let Some(guard) = wait_and_lock(&client, reason) {
            let mut m = match load_markers() {
                Ok(m) => m,
                Err(e) => {
                    k2_core::log_debug!("{P} enable: {e}");
                    return;
                }
            };
            if let Ok(cur) = client.spam_rules_url() {
                if let Some(to) = plan_spam_url(cur.as_deref(), m.spam_url.as_ref()) {
                    match client.set_spam_rules_url(to) {
                        Ok(()) => {
                            m.spam_url = Some(UrlMark { from: cur.unwrap_or_default(), to: to.into(), at: now_secs() });
                            if let Err(e) = save_markers(&m) {
                                k2_core::log_debug!("{P} enable: {e}");
                            }
                            // A refused reload here is fine: the reconcile
                            // below reloads after its own writes.
                            let reload = client.action_reload_settings();
                            k2_core::log_debug!(
                                "{P} enable: spam rules URL pinned to v3.0.1 (reload: {})",
                                reload.err().unwrap_or_else(|| "ok".into())
                            );
                        }
                        Err(e) => k2_core::log_debug!("{P} enable: rules URL pin failed (the reconcile retries): {e}"),
                    }
                }
            }
            drop(guard);
        } else {
            return;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(RULES_WAIT_SECS);
        loop {
            match client.dnsbl_servers() {
                Ok(list) if !list.is_empty() => break,
                _ => {}
            }
            if std::time::Instant::now() >= deadline {
                k2_core::log_debug!(
                    "{P} enable: Stalwart's spam rules did not arrive within 5 min — the rules \
                     repair runs at the next boot"
                );
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    }
    let Some(_guard) = wait_and_lock(&client, reason) else {
        return;
    };
    // Re-read under the lock.
    let installed = supervisor::row_field("installed_version");
    let port_plan = supervisor::row_field("port_plan");
    let owner = super::cert_owner::mail_cert_owner(&host);
    let served = || {
        supervisor::tls_cert_status(Some(&host))["state"]
            .as_str()
            .unwrap_or("missing")
            .to_string()
    };
    let points = || live_points_here(&host);
    let ctx = Ctx {
        cert: CertInputs {
            host: &host,
            owner,
            port_plan: port_plan.as_deref(),
            served_state: &served,
            points_here: &points,
        },
        installed: installed.as_deref(),
        now: now_secs(),
    };
    let markers = match load_markers() {
        Ok(m) => Some(m),
        Err(e) => {
            k2_core::log_debug!("{P} {e}");
            None
        }
    };
    let readable = markers.is_some();
    let mut restart = restart_live;
    let out = reconcile(&client, &ctx, markers, &mut restart);
    if readable {
        if let Err(e) = save_markers(&out.markers) {
            k2_core::log_debug!("{P} FAILED to record what K2 changed: {e}");
        }
    }
    for line in out.lines {
        k2_core::log_debug!("{line} ({reason})");
    }
    invalidate_status_cache();
}

/// Does the mail host point at this box (public IPv4 + interface
/// addresses; IPv6 = this box's global interface addresses, so a stray
/// AAAA counts as "elsewhere" — Let's Encrypt prefers IPv6)?
#[cfg_attr(test, allow(dead_code))]
fn live_points_here(host: &str) -> PointsHere {
    let resolver = match super::dns_verify::SystemResolver::new() {
        Ok(r) => r,
        Err(e) => return PointsHere::Unknown(format!("no resolver: {e}")),
    };
    // Only reached after a fresh lock with no real cert served: one
    // checkip read when the cache is empty/stale (never under air-gap).
    super::cert_names::refresh_public_ipv4_if_stale();
    let addrs = super::cert_names::box_addrs_local(super::cert_names::cached_public_ipv4().as_deref());
    super::cert_names::points_here(&resolver, host, &addrs)
}

/// Restart through the usual doors (mail helper, then plain sudo).
#[cfg_attr(test, allow(dead_code))]
fn restart_live() -> Result<(), String> {
    #[cfg(test)]
    {
        return Err("tests never restart Stalwart".into());
    }
    #[cfg(not(test))]
    {
        use super::imap_listeners::{restart_once, LiveDoors, RestartReport, SYSTEMCTL_PATH};
        let mut doors = LiveDoors { why: "settings reload" };
        match restart_once(&mut doors)? {
            RestartReport::Restarted(_) => Ok(()),
            RestartReport::NotRestarted { helper } => Err(format!(
                "no door can restart Stalwart (mail helper: {}; `sudo -n {SYSTEMCTL_PATH} \
                 restart stalwart` is not allowed) — the settings apply at Stalwart's next \
                 restart; as root: systemctl restart stalwart",
                helper.as_str()
            )),
        }
    }
}

// ── Status (GET /cli/mail/status) ───────────────────────────────────────

/// `status.outbound` (pure): live route + strategies from Stalwart, who
/// set each value (K2's markers), and the last doctor DNSSEC verdict.
pub fn outbound_status(
    routes: &Result<Vec<Value>, String>,
    strategies: &Result<Vec<Value>, String>,
    markers: &Markers,
    dnssec: Option<&str>,
) -> Value {
    let mut out = serde_json::json!({ "dnssec": dnssec });
    match routes {
        Ok(routes) => {
            let mx = routes
                .iter()
                .find(|r| str_field(r, "@type") == Some("Mx") && str_field(r, "name") == Some("mx"));
            let strategy = mx.and_then(|r| str_field(r, "ipLookupStrategy"));
            out["ipStrategy"] = serde_json::json!(strategy);
            let set_by = match (strategy, markers.mx_route.as_ref()) {
                (None, _) => Value::Null,
                (Some(s), Some(mk)) if mk.to.as_deref() == Some(s) => "k2".into(),
                (Some("v4ThenV6"), _) => "default".into(),
                (Some(_), _) => "hand".into(),
            };
            out["ipStrategySetBy"] = set_by;
        }
        Err(e) => out["routeError"] = serde_json::json!(e),
    }
    match strategies {
        Ok(list) => {
            let rows: Vec<Value> = list
                .iter()
                .filter_map(|s| {
                    let id = str_field(s, "id")?;
                    let dane = str_field(s, "dane").unwrap_or("?");
                    let set_by = if markers.dane.iter().any(|m| m.id == id) && dane == "disable" {
                        "k2"
                    } else if dane == "disable" || dane == "require" {
                        "hand"
                    } else {
                        "default"
                    };
                    Some(serde_json::json!({
                        "name": str_field(s, "name"),
                        "dane": dane,
                        "setBy": set_by,
                    }))
                })
                .collect();
            let off: Vec<&Value> = rows.iter().filter(|r| r["dane"] == "disable").collect();
            let summary = if rows.is_empty() {
                "unknown".to_string()
            } else if off.is_empty() {
                let mut v: Vec<&str> = rows.iter().filter_map(|r| r["dane"].as_str()).collect();
                v.sort_unstable();
                v.dedup();
                v.join("/")
            } else if off.iter().all(|r| r["setBy"] == "k2") {
                "off (K2, until 0.16.20)".into()
            } else if off.len() == rows.len() {
                "off (set by hand)".into()
            } else {
                "partly off".into()
            };
            out["dane"] = Value::Array(rows);
            out["daneSummary"] = summary.into();
            out["daneAutoOff"] = (!markers.dane.is_empty()).into();
        }
        Err(e) => out["daneError"] = serde_json::json!(e),
    }
    if let Some(p) = &markers.pending_reload {
        out["pendingReload"] = serde_json::json!({ "at": rfc3339(p.at), "error": p.error });
    }
    out
}

/// Stalwart's real order list for an Automatic Domain (`acme/order.rs`
/// `build_domains`, http/tls-alpn challenges): an empty list means the
/// four technical names under the Domain + the server name when it is in
/// that zone; otherwise each name (a bare label is under the Domain).
pub fn order_names(sans: &[String], domain_name: &str, host: &str) -> Vec<String> {
    let domain = domain_name.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if sans.is_empty() {
        let mut v: Vec<String> = ["mta-sts", "ua-auto-config", "autoconfig", "autodiscover"]
            .iter()
            .map(|h| format!("{h}.{domain}"))
            .collect();
        if host == domain || host.ends_with(&format!(".{domain}")) {
            v.push(host);
        }
        v.sort();
        v.dedup();
        return v;
    }
    sans.iter()
        .map(|h| {
            let h = h.trim().trim_end_matches('.').to_ascii_lowercase();
            if h.contains('.') {
                h
            } else {
                format!("{h}.{domain}")
            }
        })
        .collect()
}

/// The newest AcmeRenewal for one Domain (Failed by `failedAt`, others by
/// `createdAt`; RFC 3339 strings sort chronologically).
pub fn last_task_for<'a>(tasks: &'a [AcmeTask], domain_id: &str) -> Option<&'a AcmeTask> {
    tasks
        .iter()
        .filter(|t| t.domain_id == domain_id)
        .max_by(|a, b| a.at.cmp(&b.at))
}

/// `status.cert.acme` (pure).
pub fn acme_status(
    host: &str,
    domain: &Result<Option<(String, String, CertManagement)>, String>,
    tasks: &Result<Vec<AcmeTask>, String>,
) -> Value {
    match domain {
        Err(e) => serde_json::json!({ "error": e }),
        Ok(None) => serde_json::json!({ "error": format!("no Stalwart domain carries {host}") }),
        Ok(Some((domain_id, domain_name, cm))) => {
            let mut v = match cm {
                CertManagement::Manual => serde_json::json!({ "mode": "Manual", "orderNames": [], "locked": false }),
                CertManagement::Other(t) => serde_json::json!({ "mode": t, "orderNames": [], "locked": false }),
                CertManagement::Automatic { subject_alternative_names, .. } => serde_json::json!({
                    "mode": "Automatic",
                    "orderNames": order_names(subject_alternative_names, domain_name, host),
                    "locked": sans_locked_to(subject_alternative_names, host),
                }),
            };
            match tasks {
                Ok(t) => v["lastTask"] = last_task_for(t, domain_id).map(AcmeTask::to_json).unwrap_or(Value::Null),
                Err(e) => v["lastTaskError"] = serde_json::json!(e),
            }
            v
        }
    }
}

/// `lastError` read-time fallback (A2 fix 4): no supervisor error, the
/// cert is not `issued`, and the last ACME task is Retry/Failed →
/// `acme: <failureReason>`. The health loop can't wipe it.
pub fn last_error_fallback(supervisor_err: Option<&str>, cert_state: &str, acme: &Value) -> Option<String> {
    if supervisor_err.is_some_and(|e| !e.trim().is_empty()) || cert_state == "issued" {
        return None;
    }
    let t = acme.get("lastTask")?;
    let state = t.get("state")?.as_str()?;
    if state != "Retry" && state != "Failed" {
        return None;
    }
    let reason = t.get("reason").and_then(|r| r.as_str()).unwrap_or("no reason given");
    Some(format!("acme: {reason}"))
}

/// HF9: a valid served cert whose SANs are a strict superset of `[host]`
/// while the order is locked to `[host]` → the next renewal drops names.
pub fn next_renewal_note(host: &str, cert_state: &str, served: &[String], acme: &Value) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if cert_state != "issued" || acme.get("locked") != Some(&Value::Bool(true)) {
        return None;
    }
    let extra: Vec<&String> = served.iter().filter(|n| **n != host).collect();
    if !served.contains(&host) || extra.is_empty() {
        return None;
    }
    Some(format!(
        "the served certificate also covers {}; the next renewal covers {host} only",
        extra.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
    ))
}

/// Registry reads for status, cached 60 s (Settings polls status, HF10)
/// and bounded to 2 s (a slow API never stalls status).
#[derive(Clone)]
struct StatusReads {
    routes: Result<Vec<Value>, String>,
    strategies: Result<Vec<Value>, String>,
    domain: Result<Option<(String, String, CertManagement)>, String>,
    tasks: Result<Vec<AcmeTask>, String>,
}

static STATUS_CACHE: std::sync::Mutex<Option<(std::time::Instant, String, StatusReads)>> =
    std::sync::Mutex::new(None);

fn invalidate_status_cache() {
    *STATUS_CACHE.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

#[cfg_attr(test, allow(dead_code))]
fn read_status_live(host: &str) -> StatusReads {
    let fail = |e: String| StatusReads {
        routes: Err(e.clone()),
        strategies: Err(e.clone()),
        domain: Err(e.clone()),
        tasks: Err(e),
    };
    let client = match super::supervisor::mgmt_client_from_row() {
        Ok(c) => c,
        Err(e) => return fail(format!("no management client: {e}")),
    };
    let host_owned = host.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("mail-status-reads".into()).spawn(move || {
        let domain = client.mail_hostname_domain(&host_owned).and_then(|d| match d {
            None => Ok(None),
            Some((id, name)) => client.domain_get_certificate_management(&id).map(|cm| Some((id, name, cm))),
        });
        let _ = tx.send(StatusReads {
            routes: client.mta_routes(),
            strategies: client.tls_strategies(),
            domain,
            tasks: client.acme_renewal_tasks(),
        });
    });
    if spawned.is_err() {
        return fail("could not start the status read".into());
    }
    rx.recv_timeout(std::time::Duration::from_secs(2))
        .unwrap_or_else(|_| fail("the mail server's admin API did not answer within 2 s".into()))
}

fn status_reads(host: &str) -> StatusReads {
    #[cfg(test)]
    {
        let _ = host;
        if let Some(r) = TEST_STATUS_READS.with(|c| c.borrow().clone()) {
            return r;
        }
        let e = String::from("not read in tests");
        return StatusReads {
            routes: Err(e.clone()),
            strategies: Err(e.clone()),
            domain: Err(e.clone()),
            tasks: Err(e),
        };
    }
    #[cfg(not(test))]
    {
        let mut slot = STATUS_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, h, r)) = slot.as_ref() {
            if h == host && at.elapsed() < std::time::Duration::from_secs(60) {
                return r.clone();
            }
        }
        let r = read_status_live(host);
        *slot = Some((std::time::Instant::now(), host.to_string(), r.clone()));
        r
    }
}

#[cfg(test)]
thread_local! {
    static TEST_STATUS_READS: std::cell::RefCell<Option<StatusReads>> =
        const { std::cell::RefCell::new(None) };
}

/// Test seam: what the status registry reads return (`None` = errors).
#[cfg(test)]
pub(crate) fn set_test_status_reads(
    routes: Result<Vec<Value>, String>,
    strategies: Result<Vec<Value>, String>,
    domain: Result<Option<(String, String, CertManagement)>, String>,
    tasks: Result<Vec<AcmeTask>, String>,
) {
    TEST_STATUS_READS.with(|c| *c.borrow_mut() = Some(StatusReads { routes, strategies, domain, tasks }));
}

#[cfg(test)]
pub(crate) fn clear_test_status_reads() {
    TEST_STATUS_READS.with(|c| *c.borrow_mut() = None);
}

/// Last doctor verdict on the box resolver (`dnssec-resolver`), if any.
fn last_dnssec_verdict() -> Option<String> {
    let run = super::doctor::latest_run_json(None).ok().flatten()?;
    let checks = run.get("checks").and_then(|c| c.as_array())?;
    let c = checks.iter().find(|c| c["id"] == DNSSEC_ID)?;
    Some(match c["status"].as_str() {
        Some("pass") => "ok".to_string(),
        Some(s) => s.to_string(),
        None => return None,
    })
}

/// Fill `status.outbound`, `status.cert.acme` (+ HF9 note) and the
/// `lastError` fallback into a status body. Installed boxes only.
pub fn fill_status(body: &mut Value, host: Option<&str>) {
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return;
    };
    let reads = status_reads(host);
    let markers = load_markers().unwrap_or_default();
    body["outbound"] = outbound_status(&reads.routes, &reads.strategies, &markers, last_dnssec_verdict().as_deref());
    let acme = acme_status(host, &reads.domain, &reads.tasks);
    let state = body["cert"]["state"].as_str().unwrap_or("missing").to_string();
    let served: Vec<String> = body["cert"]["names"]
        .as_array()
        .map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if let Some(note) = next_renewal_note(host, &state, &served, &acme) {
        body["cert"]["note"] = note.into();
    }
    if let Some(fallback) = last_error_fallback(body["lastError"].as_str(), &state, &acme) {
        body["lastError"] = fallback.into();
    }
    if let Some(cert) = body.get_mut("cert").and_then(|c| c.as_object_mut()) {
        cert.insert("acme".into(), acme);
    }
}

// ── Doctor checks (pure over the seams) ─────────────────────────────────

use super::doctor::{DoctorCheck, ST_INFO, ST_PASS, ST_UNKNOWN, ST_WARN};

pub const DNSSEC_ID: &str = "dnssec-resolver";

fn check(id: &str, label: &str, status: &'static str, detail: String) -> DoctorCheck {
    DoctorCheck { id: id.into(), label: label.into(), status, detail, gates_direct: false }
}

/// HF19: warn while ANY outbound TLS strategy has `dane: disable` (K2 or a
/// person set it), with the line to turn it back on.
pub fn dane_disabled_check(
    strategies: &Result<Vec<Value>, String>,
    markers: &Markers,
    installed: Option<Ver>,
) -> DoctorCheck {
    const ID: &str = "dane-disabled";
    const LABEL: &str = "DANE on for outbound mail";
    let list = match strategies {
        Ok(l) => l,
        Err(e) => return check(ID, LABEL, ST_UNKNOWN, format!("could not read the TLS strategies: {e}")),
    };
    let off: Vec<(String, bool)> = list
        .iter()
        .filter(|s| str_field(s, "dane") == Some("disable"))
        .map(|s| {
            let id = str_field(s, "id").unwrap_or("");
            (str_field(s, "name").unwrap_or(id).to_string(), markers.dane.iter().any(|m| m.id == id))
        })
        .collect();
    if off.is_empty() {
        return check(ID, LABEL, ST_PASS, "DANE is not disabled on any outbound TLS strategy".into());
    }
    let by_k2: Vec<&str> = off.iter().filter(|(_, k2)| *k2).map(|(n, _)| n.as_str()).collect();
    let by_hand: Vec<&str> = off.iter().filter(|(_, k2)| !*k2).map(|(n, _)| n.as_str()).collect();
    let old = installed.is_none_or(|v| v < V_DANE_SELF_OFF);
    let mut parts = vec![format!(
        "DANE disabled on {} — mail to domains that publish TLSA records is not checked \
         against them",
        off.iter().map(|(n, _)| format!("'{n}'")).collect::<Vec<_>>().join(", ")
    )];
    if !by_k2.is_empty() {
        parts.push(format!(
            "K2 turned it off on {} because Stalwart before 0.16.20 can't check DNSSEC \
             reliably; `k2 hostmail upgrade` (0.16.20) and K2 turns it back on",
            by_k2.join(", ")
        ));
    }
    if !by_hand.is_empty() {
        parts.push(if old {
            format!(
                "set by hand on {}; after `k2 hostmail upgrade` (0.16.20) turn it back on: \
                 `k2 hostmail outbound dane on`",
                by_hand.join(", ")
            )
        } else {
            format!("set by hand on {}; turn it back on: `k2 hostmail outbound dane on`", by_hand.join(", "))
        });
    }
    check(ID, LABEL, ST_WARN, parts.join(". "))
}

/// The `mx` route: v4Only is right (K2's PTR and SPF are IPv4 only).
pub fn outbound_route_check(routes: &Result<Vec<Value>, String>, installed: Option<Ver>) -> DoctorCheck {
    const ID: &str = "outbound-ip-strategy";
    const LABEL: &str = "Outbound mail looks up IPv4 only";
    let routes = match routes {
        Ok(r) => r,
        Err(e) => return check(ID, LABEL, ST_UNKNOWN, format!("could not read the outbound routes: {e}")),
    };
    let strategy = routes
        .iter()
        .find(|r| str_field(r, "@type") == Some("Mx") && str_field(r, "name") == Some("mx"))
        .and_then(|r| str_field(r, "ipLookupStrategy"));
    match strategy {
        Some("v4Only") => check(ID, LABEL, ST_PASS, "the mx route is v4Only".into()),
        None => check(ID, LABEL, ST_UNKNOWN, "no Mx route named 'mx'".into()),
        Some(s) if installed.is_none_or(|v| v < V_DANE_SELF_OFF) => check(
            ID,
            LABEL,
            ST_WARN,
            format!(
                "the mx route is {s}: Stalwart before 0.16.20 drops good IPv4 answers when an \
                 IPv6 (AAAA) lookup errors, so mail to MX hosts without IPv6 can stall. K2 sends \
                 over IPv4 only (PTR and SPF are IPv4)"
            ),
        ),
        Some(s) => check(
            ID,
            LABEL,
            ST_INFO,
            format!("the mx route is {s} (set by hand) — K2's PTR and SPF cover IPv4 only"),
        ),
    }
}

/// Queued mail stuck on a DNSSEC error: name the one-time retry.
pub fn queue_dnssec_check(queue: &Result<Vec<crate::mail::jmap::QueuedMessageInfo>, String>) -> DoctorCheck {
    const ID: &str = "queue-dnssec";
    const LABEL: &str = "No queued mail stuck on DNSSEC";
    let q = match queue {
        Ok(q) => q,
        Err(e) => return check(ID, LABEL, ST_UNKNOWN, format!("could not read the queue: {e}")),
    };
    let stuck: Vec<&str> = q
        .iter()
        .filter(|m| m.recipients.to_string().contains("DNSSEC"))
        .map(|m| m.id.as_str())
        .collect();
    if stuck.is_empty() {
        return check(ID, LABEL, ST_PASS, format!("{} queued message(s), none failing on DNSSEC", q.len()));
    }
    check(
        ID,
        LABEL,
        ST_WARN,
        format!(
            "{} queued message(s) last failed on a DNSSEC lookup ({}). They keep their own retry \
             time; once the outbound fix is live retry each: `k2 hostmail queue retry <id>`",
            stuck.len(),
            stuck.join(", ")
        ),
    )
}

/// A4 doctor check: every spam rule expression must only call functions
/// the INSTALLED Stalwart has.
pub fn spam_rules_load_check(
    objects: &Result<Vec<(String, Value)>, String>,
    installed: Option<Ver>,
) -> DoctorCheck {
    const ID: &str = "spam-rules-load";
    const LABEL: &str = "Spam rules load on this Stalwart";
    let Some(v) = installed else {
        return check(ID, LABEL, ST_UNKNOWN, "installed Stalwart version unknown".into());
    };
    let objs = match objects {
        Ok(o) => o,
        Err(e) => return check(ID, LABEL, ST_UNKNOWN, format!("could not read the spam rules: {e}")),
    };
    let known = known_functions(v);
    let bad: Vec<String> = objs
        .iter()
        .filter_map(|(typ, o)| {
            let f = unknown_functions(o, &known);
            if f.is_empty() {
                return None;
            }
            let name = str_field(o, "name").or(str_field(o, "id")).unwrap_or("?");
            Some(format!("{typ} {name} ({})", f.join(", ")))
        })
        .collect();
    if bad.is_empty() {
        return check(ID, LABEL, ST_PASS, format!("{} rule objects checked against Stalwart {}", objs.len(), ver_str(v)));
    }
    check(
        ID,
        LABEL,
        ST_WARN,
        format!(
            "{} use functions Stalwart {} does not have, so they never load and Stalwart refuses \
             every settings reload: {}. K2 repairs the 3 known DNSBL rules at boot",
            bad.len(),
            ver_str(v),
            bad.join("; ")
        ),
    )
}

/// A2 doctor extras on `acme-cert-names`: the last ACME task error and
/// the HF9 next-renewal note.
pub fn acme_check_extras(c: &mut DoctorCheck, acme: &Value, note: Option<&str>) {
    let mut extra = Vec::new();
    if let Some(t) = acme.get("lastTask").filter(|t| !t.is_null()) {
        if let (Some(state), Some(reason)) = (t["state"].as_str(), t["reason"].as_str()) {
            extra.push(format!(
                "last ACME task {state}{}: {reason}",
                t["at"].as_str().map(|a| format!(" ({a})")).unwrap_or_default()
            ));
        }
    }
    if let Some(n) = note {
        extra.push(n.to_string());
        if c.status == ST_PASS {
            c.status = ST_INFO;
        }
    }
    if !extra.is_empty() {
        c.detail = format!("{}. {}", c.detail, extra.join(". "));
    }
}

/// The production doctor's 0.45.1 additions (`doctor::run`). Registry
/// reads on the loopback admin API; one TCP DNS question per box
/// nameserver. `server_level` = not a per-domain run.
#[cfg_attr(test, allow(dead_code))]
pub fn doctor_checks_live(
    env: &dyn super::doctor::DoctorEnv,
    host: &str,
    public_ip: Option<&str>,
    server_level: bool,
    checks: &mut Vec<DoctorCheck>,
) {
    if let Some(ip) = public_ip {
        super::cert_names::store_public_ipv4(ip, now_secs());
    }
    let installed = super::supervisor::row_field("installed_version");
    let ver = installed.as_deref().and_then(parse_version);
    let client = super::domains::engine_from_db().map(|(c, _)| c);
    // A2: last ACME task + next-renewal note on the existing check.
    if let Some(c) = checks.iter_mut().find(|c| c.id == "acme-cert-names") {
        let acme = match &client {
            Ok(cl) => {
                let domain = cl.mail_hostname_domain(host).and_then(|d| match d {
                    None => Ok(None),
                    Some((id, name)) => cl.domain_get_certificate_management(&id).map(|cm| Some((id, name, cm))),
                });
                acme_status(host, &domain, &cl.acme_renewal_tasks())
            }
            Err(e) => serde_json::json!({ "error": e }),
        };
        let cert = super::supervisor::tls_cert_status(Some(host));
        let served: Vec<String> = cert["names"]
            .as_array()
            .map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let note = next_renewal_note(host, cert["state"].as_str().unwrap_or("missing"), &served, &acme);
        acme_check_extras(c, &acme, note.as_deref());
    }
    if !server_level {
        return;
    }
    let markers = load_markers().unwrap_or_default();
    let read = |f: &dyn Fn(&StalwartClient) -> Result<Vec<Value>, String>| match &client {
        Ok(c) => f(c),
        Err(e) => Err(e.clone()),
    };
    let routes = read(&|c| c.mta_routes());
    let strategies = read(&|c| c.tls_strategies());
    checks.push(outbound_route_check(&routes, ver));
    checks.push(dane_disabled_check(&strategies, &markers, ver));
    let nameservers = env.resolv_nameservers();
    checks.push(dnssec_resolver_check(&nameservers, &|ns, q| env.dns_tcp_exchange(ns, q), ver));
    let queue = match &client {
        Ok(c) => c.queued_message_query().and_then(|ids| c.queued_message_get(&ids)),
        Err(e) => Err(e.clone()),
    };
    checks.push(queue_dnssec_check(&queue));
    let spam: Result<Vec<(String, Value)>, String> = (|| {
        let c = client.as_ref().map_err(Clone::clone)?;
        let mut all = Vec::new();
        for typ in ["SpamRule", "SpamDnsblServer", "SpamTag"] {
            all.extend(c.registry_list(typ)?.into_iter().map(|o| (typ.to_string(), o)));
        }
        Ok(all)
    })();
    checks.push(spam_rules_load_check(&spam, ver));
    let box_ip = super::cert_names::cached_public_ipv4().or_else(|| public_ip.map(str::to_string));
    let addrs = super::cert_names::box_addrs_local(box_ip.as_deref());
    match super::dns_verify::SystemResolver::new() {
        Ok(r) => checks.push(mail_host_address_check(&r, host, box_ip.as_deref(), &addrs)),
        Err(e) => checks.push(check(MAIL_HOST_ID, MAIL_HOST_LABEL, ST_UNKNOWN, format!("no resolver: {e}"))),
    }
}

const MAIL_HOST_ID: &str = "mail-host-address";
const MAIL_HOST_LABEL: &str = "The mail host's A record points here";

/// A3 / Q3: the mail host's A must point at this box — the doctor FAILS
/// it (MX and the certificate need it) while the domain's verified status
/// never flips on it. Never gates direct send.
pub fn mail_host_address_check(
    resolver: &dyn super::dns_verify::DnsResolver,
    host: &str,
    box_ip: Option<&str>,
    addrs: &super::cert_names::BoxAddrs,
) -> DoctorCheck {
    use super::doctor::ST_FAIL;
    let expected = box_ip.and_then(|s| s.parse::<std::net::Ipv4Addr>().ok());
    let g = super::dns_verify::grade_mail_host(resolver, host, expected, addrs);
    let want = expected.map(|ip| ip.to_string()).unwrap_or_else(|| "this server's public IPv4".into());
    let live = g.live.join(", ");
    match g.status {
        "valid" => match g.aaaa_note {
            Some(n) => check(MAIL_HOST_ID, MAIL_HOST_LABEL, ST_WARN, n),
            None => check(MAIL_HOST_ID, MAIL_HOST_LABEL, ST_PASS, format!("{host}: {live}")),
        },
        "missing" => check(
            MAIL_HOST_ID,
            MAIL_HOST_LABEL,
            ST_FAIL,
            format!(
                "{host} has no A record. Add `{host} A {want}` at the DNS host of its zone — MX \
                 points here and the certificate needs it — then `k2 hostmail cert renew`"
            ),
        ),
        "wrong" => check(
            MAIL_HOST_ID,
            MAIL_HOST_LABEL,
            ST_FAIL,
            format!("{host} answers {live}; it should be A {want} (and no AAAA that is not this box)"),
        ),
        _ => check(
            MAIL_HOST_ID,
            MAIL_HOST_LABEL,
            ST_UNKNOWN,
            if live.is_empty() {
                format!("could not look up {host}")
            } else {
                format!("{host} answers {live}, but this box's public IPv4 is unknown — run the doctor again")
            },
        ),
    }
}

// ── DNSSEC resolver probe (doctor `dnssec-resolver`) ────────────────────

/// `. DNSKEY IN` with EDNS0 (1232-byte buffer) and the DO bit — the same
/// question Stalwart's validator must answer first. The root DNSKEY set
/// is ~1.4 KB: truncated over UDP, so it only arrives over TCP.
pub fn dnskey_query(id: u16) -> Vec<u8> {
    let mut m = Vec::with_capacity(28);
    m.extend_from_slice(&id.to_be_bytes());
    m.extend_from_slice(&[0x01, 0x00]); // RD
    m.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 1]); // 1 question, 1 additional
    m.extend_from_slice(&[0, 0, 48, 0, 1]); // root, DNSKEY, IN
    m.extend_from_slice(&[0, 0, 41, 0x04, 0xd0, 0, 0, 0x80, 0, 0, 0]); // OPT, 1232, DO
    m
}

/// What the TCP answer carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnskeyAnswer {
    pub rcode: u8,
    pub truncated: bool,
    pub dnskeys: usize,
    /// An RRSIG whose "type covered" is DNSKEY.
    pub rrsig_over_dnskey: bool,
    pub bytes: usize,
}

fn skip_name(m: &[u8], mut pos: usize) -> Result<usize, String> {
    loop {
        let len = *m.get(pos).ok_or("truncated name")? as usize;
        if len == 0 {
            return Ok(pos + 1);
        }
        if len & 0xc0 == 0xc0 {
            m.get(pos + 1).ok_or("truncated pointer")?;
            return Ok(pos + 2);
        }
        pos += 1 + len;
    }
}

fn be16(m: &[u8], pos: usize) -> Result<u16, String> {
    Ok(u16::from_be_bytes([
        *m.get(pos).ok_or("truncated")?,
        *m.get(pos + 1).ok_or("truncated")?,
    ]))
}

/// Parse a TCP DNS stream (2-byte length + message) answering
/// [`dnskey_query`]`(id)`. Pure; fixtures are captured wire bytes.
pub fn parse_dnskey_tcp(id: u16, stream: &[u8]) -> Result<DnskeyAnswer, String> {
    let len = be16(stream, 0).map_err(|_| "empty answer".to_string())? as usize;
    let m = stream.get(2..2 + len).ok_or_else(|| {
        format!("short TCP answer: {} of {len} bytes", stream.len().saturating_sub(2))
    })?;
    if m.len() < 12 {
        return Err("answer shorter than a DNS header".into());
    }
    if be16(m, 0)? != id {
        return Err("answer id does not match the query".into());
    }
    let flags = be16(m, 2)?;
    if flags & 0x8000 == 0 {
        return Err("not a response".into());
    }
    let (qd, an) = (be16(m, 4)? as usize, be16(m, 6)? as usize);
    let mut pos = 12;
    for _ in 0..qd {
        pos = skip_name(m, pos)? + 4;
    }
    let mut dnskeys = 0;
    let mut rrsig = false;
    for _ in 0..an {
        pos = skip_name(m, pos)?;
        let typ = be16(m, pos)?;
        let rdlen = be16(m, pos + 8)? as usize;
        let rdata = pos + 10;
        if m.len() < rdata + rdlen {
            return Err("truncated record".into());
        }
        match typ {
            48 => dnskeys += 1,
            46 if be16(m, rdata)? == 48 => rrsig = true,
            _ => {}
        }
        pos = rdata + rdlen;
    }
    Ok(DnskeyAnswer {
        rcode: (flags & 0x000f) as u8,
        truncated: flags & 0x0200 != 0,
        dnskeys,
        rrsig_over_dnskey: rrsig,
        bytes: m.len(),
    })
}

/// `nameserver` lines of a resolv.conf (comments stripped).
pub fn resolv_conf_nameservers(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split(['#', ';']).next().unwrap_or("").trim())
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            (it.next() == Some("nameserver")).then(|| it.next().map(str::to_string)).flatten()
        })
        .collect()
}

/// One DNS-over-TCP exchange with a nameserver on port 53 (3 s budget):
/// returns the stream as received (length + message). Production only.
#[cfg_attr(test, allow(dead_code))]
pub fn tcp_dns_exchange(nameserver: &str, query: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::{Read, Write};
    let t = std::time::Duration::from_secs(3);
    let ip: std::net::IpAddr = nameserver
        .split('%')
        .next()
        .unwrap_or(nameserver)
        .parse()
        .map_err(|e| format!("'{nameserver}' is not an IP address: {e}"))?;
    let mut s = std::net::TcpStream::connect_timeout(&std::net::SocketAddr::new(ip, 53), t)
        .map_err(|e| format!("TCP/53 connect: {e}"))?;
    let _ = s.set_read_timeout(Some(t));
    let _ = s.set_write_timeout(Some(t));
    let mut out = (query.len() as u16).to_be_bytes().to_vec();
    out.extend_from_slice(query);
    s.write_all(&out).map_err(|e| format!("TCP/53 write: {e}"))?;
    let mut len = [0u8; 2];
    s.read_exact(&mut len).map_err(|e| format!("TCP/53 read: {e}"))?;
    let mut msg = vec![0u8; u16::from_be_bytes(len) as usize];
    s.read_exact(&mut msg).map_err(|e| format!("TCP/53 read: {e}"))?;
    let mut stream = len.to_vec();
    stream.extend_from_slice(&msg);
    Ok(stream)
}

/// The doctor's `dnssec-resolver` check (HF5): per nameserver in
/// resolv.conf, `. DNSKEY` with DO over TCP must come back with an RRSIG
/// over the key set. Warns when one can't — DANE can't work then — and,
/// on Stalwart before 0.16.23, when 2+ nameservers are listed (a hickory
/// race a single-server probe can't see). Never a public resolver.
pub fn dnssec_resolver_check(
    nameservers: &[String],
    probe: &dyn Fn(&str, &[u8]) -> Result<Vec<u8>, String>,
    installed: Option<Ver>,
) -> DoctorCheck {
    const LABEL: &str = "The box resolver serves DNSSEC over TCP";
    if nameservers.is_empty() {
        return check(DNSSEC_ID, LABEL, ST_UNKNOWN, "no nameserver in /etc/resolv.conf".into());
    }
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    for (i, ns) in nameservers.iter().enumerate() {
        let id = 0x4b30u16.wrapping_add(i as u16);
        match probe(ns, &dnskey_query(id)).and_then(|s| parse_dnskey_tcp(id, &s)) {
            Ok(a) if a.rcode == 0 && a.dnskeys > 0 && a.rrsig_over_dnskey => ok.push(ns.clone()),
            Ok(a) if a.rcode != 0 => bad.push(format!("{ns}: answered rcode {}", a.rcode)),
            Ok(a) if a.dnskeys == 0 => bad.push(format!("{ns}: no root DNSKEY in the answer")),
            Ok(_) => bad.push(format!("{ns}: the root DNSKEY came back without its signature (RRSIG)")),
            Err(e) => bad.push(format!("{ns}: {e}")),
        }
    }
    let count = format!("{} nameserver(s) in /etc/resolv.conf", nameservers.len());
    if !bad.is_empty() {
        return check(
            DNSSEC_ID,
            LABEL,
            ST_WARN,
            format!(
                "DANE can't work here: the resolver can't serve DNSSEC over TCP (check TCP/53 \
                 egress) — {}. {count}",
                bad.join("; ")
            ),
        );
    }
    if nameservers.len() >= 2 && installed.is_none_or(|v| v < V_DNSSEC_RACE_FIXED) {
        return check(
            DNSSEC_ID,
            LABEL,
            ST_WARN,
            format!(
                "{count}, each serves DNSSEC over TCP, but Stalwart before 0.16.23 can falsely \
                 fail DNSSEC when more than one nameserver is listed. If mail stalls on \
                 'DNSSEC … Bogus', DANE off is the workaround (K2 does it below 0.16.20)"
            ),
        );
    }
    check(DNSSEC_ID, LABEL, ST_PASS, format!("{count}: {} serve(s) DNSSEC over TCP", ok.join(", ")))
}

// ── `k2 hostmail outbound` (GET show, POST dane on) ─────────────────────

/// Turn DANE back on (`optional`) on every `disable` strategy, drop K2's
/// markers, reload. Refused below 0.16.20 (mail to IPv6-less MX hosts
/// would stall again).
pub fn dane_on(
    api: &dyn DefaultsApi,
    installed: Option<&str>,
    markers: &mut Markers,
    restart: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Vec<String>, String> {
    let v = installed.and_then(parse_version).ok_or_else(|| {
        format!("installed Stalwart version unknown ({})", installed.unwrap_or("none"))
    })?;
    if v < V_DANE_SELF_OFF {
        return Err(format!(
            "Stalwart {} can't check DNSSEC reliably, so DANE stays off. Run `k2 hostmail \
             upgrade` first (0.16.20 turns DANE off by itself when it can't validate); K2 turns \
             back on what it turned off, then run this for the rest",
            ver_str(v)
        ));
    }
    let mut changed = Vec::new();
    for s in api.tls_strategies()? {
        if str_field(&s, "dane") != Some("disable") {
            continue;
        }
        let id = str_field(&s, "id").ok_or("a TLS strategy has no id")?.to_string();
        let mut body = writable_body(&s);
        body["dane"] = Value::String("optional".into());
        api.tls_strategy_set(&id, body)?;
        changed.push(str_field(&s, "name").unwrap_or(&id).to_string());
    }
    markers.dane.clear();
    if !changed.is_empty() {
        if let Err(e) = api.reload_settings() {
            restart().map_err(|r| {
                format!("DANE is saved as optional but not live: reload refused ({e}) and no restart ({r})")
            })?;
        }
    }
    Ok(changed)
}

fn err_json(status: &'static str, code: &str, hint: String) -> crate::cli_response::CliResponse {
    crate::cli_response::CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

/// GET `/cli/mail/outbound` — the outbound settings K2 manages (fresh
/// reads, not the status cache).
pub fn handle_get(_params: &std::collections::HashMap<String, String>) -> crate::cli_response::CliResponse {
    let host = super::supervisor::row_field("hostname");
    if host.is_none() {
        return err_json("409 Conflict", "not_ready", "the mail server is not installed".into());
    }
    let (client, _) = match super::domains::engine_from_db() {
        Ok(c) => c,
        Err(e) => return err_json("409 Conflict", "not_ready", e),
    };
    let markers = load_markers().unwrap_or_default();
    let out = outbound_status(&client.mta_routes(), &client.tls_strategies(), &markers, last_dnssec_verdict().as_deref());
    let mut body = serde_json::json!({ "ok": true, "installedVersion": super::supervisor::row_field("installed_version") });
    if let (Some(b), Some(o)) = (body.as_object_mut(), out.as_object()) {
        for (k, v) in o {
            b.insert(k.clone(), v.clone());
        }
    }
    crate::cli_response::CliResponse::ok_json(body.to_string())
}

/// POST `/cli/mail/outbound` `{"dane":"on"}` — turn DANE back on.
pub fn handle_post(body: &[u8]) -> crate::cli_response::CliResponse {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return crate::cli_response::CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if parsed["dane"].as_str() != Some("on") {
        return err_json(
            "400 Bad Request",
            "usage",
            "body must be {\"dane\":\"on\"} — the only outbound setting K2 changes on request".into(),
        );
    }
    if let Some(resp) = super::routes_server::upgrade_running_response() {
        return resp;
    }
    let (client, _) = match super::domains::engine_from_db() {
        Ok(c) => c,
        Err(e) => return err_json("409 Conflict", "not_ready", e),
    };
    let mut markers = match load_markers() {
        Ok(m) => m,
        Err(e) => return err_json("500 Internal Server Error", "state", e),
    };
    if !super::supervisor::try_begin_enable() {
        return err_json(
            "409 Conflict",
            "busy",
            "an enable, upgrade or boot reconcile holds the mail server — try again in a minute".into(),
        );
    }
    let installed = super::supervisor::row_field("installed_version");
    let mut restart = restart_live;
    let result = dane_on(&client, installed.as_deref(), &mut markers, &mut restart);
    let saved = save_markers(&markers);
    super::supervisor::end_enable();
    invalidate_status_cache();
    match (result, saved) {
        (Ok(changed), Ok(())) => crate::cli_response::CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "changed": changed,
                "hint": if changed.is_empty() {
                    "DANE was already on everywhere — nothing changed".to_string()
                } else {
                    format!("DANE is optional again on {}", changed.join(", "))
                },
            })
            .to_string(),
        ),
        (Err(e), _) => err_json("409 Conflict", "refused", e),
        (Ok(_), Err(e)) => err_json("500 Internal Server Error", "state", e),
    }
}

#[cfg(test)]
mod tests;
