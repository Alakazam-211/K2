//! Hosted-mail DAV policy (prd-hostmail-calendars-v1 S2: CAL8, CAL28,
//! CAL39). Owner-only `k2 hostmail calendar status|enable|disable
//! [--files on|off]` → `GET|POST /cli/mail/dav`.
//!
//! Stalwart 0.16.10 serves CalDAV, CardDAV and WebDAV file storage to
//! every account by default. The owner decides, host-wide, whether
//! hosted addresses may use CalDAV/CardDAV ("calendars") and WebDAV
//! file storage ("files"):
//!
//! - **Mechanism:** per account, `x:Account/set update {permissions:
//!   {"@type":"Merge","enabledPermissions":{…},"disabledPermissions":
//!   {…}}}`. The account's CURRENT `permissions` is read first and only
//!   the names in [`CALENDAR_PERMISSIONS`] / [`FILES_PERMISSIONS`] move;
//!   every other enabled/disabled entry an operator set stays
//!   ([`merge_permissions`]). Stalwart drops the account's cached
//!   access token itself when `permissions` changes
//!   (`common/src/cache/invalidate.rs`), so no restart and no
//!   `InvalidateCaches` action are needed.
//! - **Never** `hostmail disable/enable` and never a Stalwart restart
//!   (CAL39). Never the service account (`k2-daemon`, Admin role) or
//!   the recovery `admin`: only accounts K2 minted (rows in
//!   `mail_addresses`) are touched, and an Admin-role account is
//!   skipped even if one were listed.
//! - **Storage:** `mail_server.dav_policy_json` (migration 0128).
//!   NULL = never applied = Stalwart's default (everything on). The
//!   row is deleted on `hostmail uninstall`, so the policy resets.
//! - **Default (CAL28):** CalDAV/CardDAV stay on; WebDAV files go OFF
//!   on the owner's FIRST `calendar enable|disable` (not at boot).
//! - **New mints** carry the policy in the create call
//!   ([`mint_permissions`] → `addresses::mint_address`).
//! - **Backfill:** enable/disable applies to every hosted account
//!   right away. If any account failed, `backfilledAt` stays unset and
//!   the boot reconcile ([`spawn_startup_backfill`], the
//!   `imap_listeners.rs` pattern) finishes the job once; never per
//!   request.
//!
//! Permissions apply to USER logins only: the admin API key (and so
//! the agent mail CLI) is unaffected. Names are exact Stalwart 0.16.10
//! `Permission` strings (`registry/src/schema/enums_impl.rs`).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::cli_response::CliResponse;
use crate::mail::domains;
use crate::mail::hosted::err_json;
use crate::mail::jmap::StalwartClient;

/// CalDAV + CardDAV + their JMAP twins + calendar scheduling/alarms.
/// Denied when the owner runs `k2 hostmail calendar disable`.
pub const CALENDAR_PERMISSIONS: &[&str] = &[
    // CalDAV
    "davCalPropFind",
    "davCalPropPatch",
    "davCalGet",
    "davCalMkCol",
    "davCalDelete",
    "davCalPut",
    "davCalCopy",
    "davCalMove",
    "davCalLock",
    "davCalAcl",
    "davCalQuery",
    "davCalMultiGet",
    "davCalFreeBusyQuery",
    // CardDAV
    "davCardPropFind",
    "davCardPropPatch",
    "davCardGet",
    "davCardMkCol",
    "davCardDelete",
    "davCardPut",
    "davCardCopy",
    "davCardMove",
    "davCardLock",
    "davCardAcl",
    "davCardQuery",
    "davCardMultiGet",
    // JMAP for Calendars
    "jmapCalendarGet",
    "jmapCalendarChanges",
    "jmapCalendarCreate",
    "jmapCalendarUpdate",
    "jmapCalendarDestroy",
    "jmapCalendarEventGet",
    "jmapCalendarEventChanges",
    "jmapCalendarEventQuery",
    "jmapCalendarEventQueryChanges",
    "jmapCalendarEventCreate",
    "jmapCalendarEventUpdate",
    "jmapCalendarEventDestroy",
    "jmapCalendarEventCopy",
    "jmapCalendarEventParse",
    "jmapCalendarEventNotificationGet",
    "jmapCalendarEventNotificationChanges",
    "jmapCalendarEventNotificationQuery",
    "jmapCalendarEventNotificationQueryChanges",
    "jmapCalendarEventNotificationCreate",
    "jmapCalendarEventNotificationUpdate",
    "jmapCalendarEventNotificationDestroy",
    "jmapParticipantIdentityGet",
    "jmapParticipantIdentityChanges",
    "jmapParticipantIdentityCreate",
    "jmapParticipantIdentityUpdate",
    "jmapParticipantIdentityDestroy",
    // JMAP for Contacts
    "jmapAddressBookGet",
    "jmapAddressBookChanges",
    "jmapAddressBookCreate",
    "jmapAddressBookUpdate",
    "jmapAddressBookDestroy",
    "jmapContactCardGet",
    "jmapContactCardChanges",
    "jmapContactCardQuery",
    "jmapContactCardQueryChanges",
    "jmapContactCardCreate",
    "jmapContactCardUpdate",
    "jmapContactCardDestroy",
    "jmapContactCardCopy",
    "jmapContactCardParse",
    // Scheduling (iTIP) + email alarms
    "calendarSchedulingSend",
    "calendarSchedulingReceive",
    "calendarAlarmsSend",
];

/// WebDAV file storage + its JMAP twin. Denied by `--files off`.
pub const FILES_PERMISSIONS: &[&str] = &[
    "davFilePropFind",
    "davFilePropPatch",
    "davFileGet",
    "davFileMkCol",
    "davFileDelete",
    "davFilePut",
    "davFileCopy",
    "davFileMove",
    "davFileLock",
    "davFileAcl",
    "jmapFileNodeGet",
    "jmapFileNodeChanges",
    "jmapFileNodeQuery",
    "jmapFileNodeQueryChanges",
    "jmapFileNodeCreate",
    "jmapFileNodeUpdate",
    "jmapFileNodeDestroy",
    "jmapFileNodeCopy",
];

/// The service account `hostmail enable` mints (Admin role). Never touched.
const SERVICE_ACCOUNT_NAME: &str = "k2-daemon";
/// Stalwart's bootstrap/recovery principal. Never touched.
const RECOVERY_ADMIN_NAME: &str = "admin";

const LOG_PREFIX: &str = "[mail-dav]";

/// The paths a foreign :443 proxy must forward for CalDAV/CardDAV.
pub const FOREIGN_PROXY_REASON: &str =
    "something other than K2 or Stalwart owns :443 on this box, so K2 cannot route \
     CalDAV/CardDAV: your proxy must forward /.well-known/caldav, /.well-known/carddav \
     and /dav/ to https://127.0.0.1:8443";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnOff {
    On,
    Off,
}

impl OnOff {
    pub fn as_str(self) -> &'static str {
        match self {
            OnOff::On => "on",
            OnOff::Off => "off",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "on" => Ok(OnOff::On),
            "off" => Ok(OnOff::Off),
            other => Err(format!("expected on|off, got '{other}'")),
        }
    }
}

/// `mail_server.dav_policy_json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DavPolicy {
    pub calendars: OnOff,
    pub files: OnOff,
    pub applied_at: i64,
    /// Set once every hosted account carries the policy. `None` = the
    /// boot reconcile still has accounts to finish.
    pub backfilled_at: Option<i64>,
}

impl DavPolicy {
    /// Names this policy denies.
    pub fn denied(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.calendars == OnOff::Off {
            out.extend_from_slice(CALENDAR_PERMISSIONS);
        }
        if self.files == OnOff::Off {
            out.extend_from_slice(FILES_PERMISSIONS);
        }
        out
    }

    /// Managed names this policy allows (taken back out of an account's
    /// disabled list).
    pub fn allowed(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.calendars == OnOff::On {
            out.extend_from_slice(CALENDAR_PERMISSIONS);
        }
        if self.files == OnOff::On {
            out.extend_from_slice(FILES_PERMISSIONS);
        }
        out
    }
}

pub fn parse_policy_json(raw: &str) -> Result<DavPolicy, String> {
    serde_json::from_str(raw).map_err(|e| format!("mail_server.dav_policy_json: {e}"))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── Pure permission merge ───────────────────────────────────────────────

/// A `Map<Permission>` from the wire: `{"name": true}` (what Stalwart
/// serves) or a plain `["name"]` list. Only `true` entries count.
fn permission_set(v: Option<&serde_json::Value>) -> Result<BTreeMap<String, bool>, String> {
    let mut out = BTreeMap::new();
    match v {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Object(m)) => {
            for (k, val) in m {
                if val.as_bool() == Some(true) {
                    out.insert(k.clone(), true);
                }
            }
        }
        Some(serde_json::Value::Array(a)) => {
            for item in a {
                let s = item
                    .as_str()
                    .ok_or_else(|| format!("permission list entry is not a string: {item}"))?;
                out.insert(s.to_string(), true);
            }
        }
        Some(other) => return Err(format!("permission list is not an object: {other}")),
    }
    Ok(out)
}

fn to_wire(set: &BTreeMap<String, bool>) -> serde_json::Value {
    serde_json::Value::Object(
        set.keys()
            .map(|k| (k.clone(), serde_json::Value::Bool(true)))
            .collect(),
    )
}

/// The `permissions` value to write so `existing` denies `denied` and
/// no longer denies `allowed`, keeping every other entry. `Ok(None)` =
/// already there, write nothing.
///
/// - `Inherit` (or absent): becomes `Merge {enabled: {}, disabled:
///   denied}`; nothing denied → unchanged.
/// - `Merge`: disabled = disabled − allowed + denied; enabled =
///   enabled − denied. Both empty afterwards → `Inherit`.
/// - `Replace` (an operator's explicit list): enabled = enabled −
///   denied + allowed; disabled = disabled − allowed + denied. Stays
///   `Replace`.
/// - Anything else → error; never clobbered.
pub fn merge_permissions(
    existing: Option<&serde_json::Value>,
    denied: &[&str],
    allowed: &[&str],
) -> Result<Option<serde_json::Value>, String> {
    let kind = match existing {
        None | Some(serde_json::Value::Null) => "Inherit".to_string(),
        Some(v) => v
            .get("@type")
            .and_then(|t| t.as_str())
            .ok_or_else(|| format!("permissions has no @type: {v}"))?
            .to_string(),
    };
    let (mut enabled, mut disabled) = match kind.as_str() {
        "Inherit" => (BTreeMap::new(), BTreeMap::new()),
        "Merge" | "Replace" => {
            let v = existing.expect("Merge/Replace came from a value");
            (
                permission_set(v.get("enabledPermissions"))?,
                permission_set(v.get("disabledPermissions"))?,
            )
        }
        other => return Err(format!("unknown permissions type '{other}' — not changed")),
    };
    let before = (enabled.clone(), disabled.clone());
    for name in allowed {
        disabled.remove(*name);
        if kind == "Replace" {
            enabled.insert((*name).to_string(), true);
        }
    }
    for name in denied {
        enabled.remove(*name);
        disabled.insert((*name).to_string(), true);
    }
    if (enabled.clone(), disabled.clone()) == before {
        return Ok(None);
    }
    if kind != "Replace" && enabled.is_empty() && disabled.is_empty() {
        return Ok(Some(serde_json::json!({ "@type": "Inherit" })));
    }
    let out_kind = if kind == "Replace" {
        "Replace"
    } else {
        "Merge"
    };
    Ok(Some(serde_json::json!({
        "@type": out_kind,
        "enabledPermissions": to_wire(&enabled),
        "disabledPermissions": to_wire(&disabled),
    })))
}

/// The `permissions` a NEW account is created with under `policy`.
/// `None` = Stalwart's default (`Inherit`).
pub fn create_permissions(policy: &DavPolicy) -> Option<serde_json::Value> {
    merge_permissions(None, &policy.denied(), &[]).expect("Inherit always merges")
}

// ── Engine seam ─────────────────────────────────────────────────────────

/// What the policy needs from Stalwart. Production = the JMAP client;
/// tests use a recording fake.
pub trait DavAccounts {
    /// Raw `x:Account/get` rows (`id`, `@type`, `name`, `roles`,
    /// `permissions`) for `ids`. Ids Stalwart does not know are absent.
    fn get_accounts(&self, ids: &[String]) -> Result<Vec<serde_json::Value>, String>;
    fn set_permissions(&self, id: &str, permissions: &serde_json::Value) -> Result<(), String>;
}

impl DavAccounts for StalwartClient {
    fn get_accounts(&self, ids: &[String]) -> Result<Vec<serde_json::Value>, String> {
        self.account_get_permissions(ids)
    }
    fn set_permissions(&self, id: &str, permissions: &serde_json::Value) -> Result<(), String> {
        self.account_set_permissions(id, permissions)
    }
}

/// Outcome of one apply pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    pub total: usize,
    pub updated: Vec<String>,
    pub unchanged: usize,
    /// (account id, why) — not ours to touch, or gone from Stalwart.
    pub skipped: Vec<(String, String)>,
    /// (account id, error) — the boot reconcile retries these.
    pub failed: Vec<(String, String)>,
}

impl ApplyReport {
    pub fn complete(&self) -> bool {
        self.failed.is_empty()
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "total": self.total,
            "updated": self.updated.len(),
            "unchanged": self.unchanged,
            "skipped": self.skipped.iter().map(|(id, why)| serde_json::json!({"id": id, "reason": why})).collect::<Vec<_>>(),
            "failed": self.failed.iter().map(|(id, e)| serde_json::json!({"id": id, "error": e})).collect::<Vec<_>>(),
        })
    }
}

/// Why `row` must not be touched, if so: not a User, an Admin role,
/// or the service/recovery principal by name.
fn protected_reason(row: &serde_json::Value) -> Option<String> {
    if let Some(t) = row.get("@type").and_then(|v| v.as_str()) {
        if t != "User" {
            return Some(format!("account type {t}"));
        }
    }
    let role = row
        .get("roles")
        .and_then(|r| r.get("@type"))
        .and_then(|v| v.as_str());
    if role == Some("Admin") {
        return Some("admin role".to_string());
    }
    let name = row.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name.eq_ignore_ascii_case(SERVICE_ACCOUNT_NAME)
        || name.eq_ignore_ascii_case(RECOVERY_ADMIN_NAME)
    {
        return Some(format!("service account '{name}'"));
    }
    None
}

/// Apply `policy` to every account in `ids`. One read for all, then one
/// write per account that needs a change. Never stops at the first
/// failure.
pub fn apply_policy(api: &dyn DavAccounts, ids: &[String], policy: &DavPolicy) -> ApplyReport {
    let mut report = ApplyReport {
        total: ids.len(),
        ..ApplyReport::default()
    };
    if ids.is_empty() {
        return report;
    }
    let rows = match api.get_accounts(ids) {
        Ok(r) => r,
        Err(e) => {
            report.failed = ids.iter().map(|id| (id.clone(), e.clone())).collect();
            return report;
        }
    };
    let by_id: HashMap<&str, &serde_json::Value> = rows
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(|id| (id, r)))
        .collect();
    let denied = policy.denied();
    let allowed = policy.allowed();
    for id in ids {
        let Some(row) = by_id.get(id.as_str()) else {
            report
                .skipped
                .push((id.clone(), "not on the mail server".to_string()));
            continue;
        };
        if let Some(why) = protected_reason(row) {
            report.skipped.push((id.clone(), why));
            continue;
        }
        match merge_permissions(row.get("permissions"), &denied, &allowed) {
            Ok(None) => report.unchanged += 1,
            Ok(Some(next)) => match api.set_permissions(id, &next) {
                Ok(()) => report.updated.push(id.clone()),
                Err(e) => report.failed.push((id.clone(), e)),
            },
            Err(e) => report.failed.push((id.clone(), e)),
        }
    }
    report
}

// ── DB ──────────────────────────────────────────────────────────────────

/// The stored policy. `Ok(None)` = not installed or never applied.
pub fn load_policy() -> Result<Option<DavPolicy>, String> {
    let raw: Option<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT dav_policy_json FROM mail_server WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .ok()
        .flatten()
    };
    match raw.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => parse_policy_json(s).map(Some),
        None => Ok(None),
    }
}

fn store_policy(policy: &DavPolicy) -> Result<(), String> {
    let json = serde_json::to_string(policy).map_err(|e| e.to_string())?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n = conn
        .execute(
            "UPDATE mail_server SET dav_policy_json = ?1, updated_at = ?2 WHERE id = 1",
            rusqlite::params![json, now_secs()],
        )
        .map_err(|e| format!("store dav_policy_json: {e}"))?;
    if n == 0 {
        return Err("hosted mail is not installed".to_string());
    }
    Ok(())
}

/// Stamp `backfilledAt` only if the stored policy is still the one we
/// applied (a newer enable/disable in between owns the row).
fn stamp_backfilled(applied: &DavPolicy, at: i64) -> Result<bool, String> {
    match load_policy()? {
        Some(cur)
            if cur.applied_at == applied.applied_at
                && cur.calendars == applied.calendars
                && cur.files == applied.files =>
        {
            store_policy(&DavPolicy {
                backfilled_at: Some(at),
                ..cur
            })?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Stalwart ids of every hosted address K2 minted (active and retired —
/// retired mailboxes keep their data and their account).
/// A read error is an error — never an empty list that would stamp
/// `backfilledAt` over accounts nobody touched.
pub fn hosted_account_ids() -> Result<Vec<String>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT stalwart_account_id FROM mail_addresses \
             WHERE stalwart_account_id IS NOT NULL AND TRIM(stalwart_account_id) != '' \
             ORDER BY stalwart_account_id",
        )
        .map_err(|e| format!("list hosted accounts: {e}"))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| format!("list hosted accounts: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("list hosted accounts: {e}"))
}

/// The `permissions` a mint creates its account with (`None` = the
/// policy was never applied, or denies nothing).
pub fn mint_permissions() -> Result<Option<serde_json::Value>, String> {
    Ok(load_policy()?.as_ref().and_then(create_permissions))
}

// ── Status ──────────────────────────────────────────────────────────────

/// `{enabled, files, policy}` for status and the GET route.
pub fn policy_json(policy: Option<&DavPolicy>) -> serde_json::Value {
    match policy {
        None => serde_json::json!({
            "enabled": true,
            "files": true,
            "policy": "default",
            "appliedAt": null,
            "backfilledAt": null,
        }),
        Some(p) => serde_json::json!({
            "enabled": p.calendars == OnOff::On,
            "files": p.files == OnOff::On,
            "policy": "applied",
            "appliedAt": p.applied_at,
            "backfilledAt": p.backfilled_at,
        }),
    }
}

/// CAL26: can a calendar client reach CalDAV on this box? Pure.
/// `host` is the mail host the url names; `caddy_mail_site` = the
/// rendered Caddyfile has the mail Host site AND Caddy is running.
pub fn calendar_reach(
    installed: bool,
    port_plan: Option<&str>,
    host: Option<&str>,
    caddy_mail_site: bool,
) -> (bool, Option<String>) {
    if !installed {
        return (false, Some("hosted mail is not installed".to_string()));
    }
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return (
            false,
            Some("the mail server has no hostname on file".to_string()),
        );
    };
    let lower = host.trim_end_matches('.').to_ascii_lowercase();
    if lower == "k2.dev" || lower.ends_with(".k2.dev") {
        return (
            false,
            Some(format!(
                "{host} resolves to the K2 control plane, not this box — calendar \
                 clients need a mail hostname on your own domain (k2 hostmail ptr set \
                 / k2 domain)"
            )),
        );
    }
    if port_plan == Some("tls-alpn") {
        return (true, None);
    }
    if caddy_mail_site {
        return (true, None);
    }
    (false, Some(FOREIGN_PROXY_REASON.to_string()))
}

/// The CalDAV discovery url for `host` (never a `*.k2.dev` name).
pub fn caldav_url(host: Option<&str>) -> Option<String> {
    let host = host.map(str::trim).filter(|h| !h.is_empty())?;
    let lower = host.trim_end_matches('.').to_ascii_lowercase();
    if lower == "k2.dev" || lower.ends_with(".k2.dev") {
        return None;
    }
    Some(format!(
        "https://{}/.well-known/caldav",
        host.trim_end_matches('.')
    ))
}

/// The mail host calendar clients use: the default mail domain's mail
/// host (`mail_hostname_for_apex`, CAL25), else the server hostname.
pub fn calendar_host(server_hostname: Option<&str>) -> Option<String> {
    let apex = k2_core::app_settings::load().mail_default_domain;
    let apex = apex.trim();
    if !apex.is_empty() {
        if let Some(h) = crate::mail::autoconfig::mail_hostname_for_apex(apex) {
            return Some(h);
        }
    }
    server_hostname
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_string)
}

/// `status.calendar` for `GET /cli/mail/status` (and the dav GET).
pub fn status_block(
    installed: bool,
    server_hostname: Option<&str>,
    port_plan: Option<&str>,
) -> serde_json::Value {
    let policy = load_policy();
    let host = calendar_host(server_hostname);
    let caddy = match host.as_deref() {
        Some(h) if installed && port_plan != Some("tls-alpn") => {
            k2_core::skin_door::mail_site_live(h)
        }
        _ => false,
    };
    let (reachable, reason) = calendar_reach(installed, port_plan, host.as_deref(), caddy);
    let mut out = match &policy {
        Ok(p) => policy_json(p.as_ref()),
        Err(e) => serde_json::json!({ "policy": "unreadable", "policyError": e }),
    };
    out["reachable"] = serde_json::json!(reachable);
    out["url"] = serde_json::json!(caldav_url(host.as_deref()));
    out["reason"] = serde_json::json!(reason);
    out
}

// ── Routes ──────────────────────────────────────────────────────────────

fn server_row() -> Option<(String, Option<String>, Option<String>)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT status, hostname, port_plan FROM mail_server WHERE id = 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .ok()
}

fn status_payload() -> serde_json::Value {
    let row = server_row();
    let installed = row.is_some();
    let (hostname, plan) = row.map(|(_, h, p)| (h, p)).unwrap_or((None, None));
    let mut v = status_block(installed, hostname.as_deref(), plan.as_deref());
    v["ok"] = serde_json::json!(true);
    v["installed"] = serde_json::json!(installed);
    v
}

/// `GET /cli/mail/dav` — the policy + reach. Owner/admin (gate in
/// `mail_routes::is_owner_level_mutation`).
pub fn handle_dav_get(_params: &HashMap<String, String>) -> CliResponse {
    CliResponse::ok_json(status_payload().to_string())
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct DavBody {
    action: Option<String>,
    files: Option<String>,
}

/// `POST /cli/mail/dav` `{action: "status"|"enable"|"disable", files?:
/// "on"|"off"}`.
pub fn handle_dav_post(body: &[u8]) -> CliResponse {
    let b: DavBody = if body.iter().all(|c| c.is_ascii_whitespace()) {
        DavBody::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(b) => b,
            Err(e) => {
                return err_json(
                    "400 Bad Request",
                    "usage",
                    format!("invalid JSON body: {e}"),
                )
            }
        }
    };
    let action = b.action.as_deref().map(str::trim).unwrap_or("");
    let calendars = match action {
        "status" => {
            if b.files.is_some() {
                return err_json(
                    "400 Bad Request",
                    "usage",
                    "--files goes with enable|disable, not status".to_string(),
                );
            }
            return CliResponse::ok_json(status_payload().to_string());
        }
        "enable" => OnOff::On,
        "disable" => OnOff::Off,
        "" => {
            return err_json(
                "400 Bad Request",
                "usage",
                "missing 'action' (status|enable|disable)".to_string(),
            )
        }
        other => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("unknown action '{other}' — status|enable|disable"),
            )
        }
    };
    let files_flag = match b.files.as_deref() {
        Some(raw) => match OnOff::parse(raw) {
            Ok(v) => Some(v),
            Err(e) => return err_json("400 Bad Request", "usage", format!("--files: {e}")),
        },
        None => None,
    };
    let previous = match load_policy() {
        Ok(p) => p,
        Err(e) => return err_json("500 Internal Server Error", "error", e),
    };
    // CAL28: files go off on the owner's FIRST enable|disable; later
    // calls keep the stored choice unless --files says otherwise.
    let files =
        files_flag.unwrap_or_else(|| previous.as_ref().map(|p| p.files).unwrap_or(OnOff::Off));

    let engine = match domains::engine_from_db() {
        Ok((c, _)) => c,
        Err(e) => return err_json("503 Service Unavailable", "not_ready", e),
    };
    if !crate::mail::supervisor::try_begin_enable() {
        return err_json(
            "409 Conflict",
            "busy",
            "a hosted-mail enable (or the boot reconcile) is running — try again in a minute"
                .to_string(),
        );
    }
    struct EndEnable;
    impl Drop for EndEnable {
        fn drop(&mut self) {
            crate::mail::supervisor::end_enable();
        }
    }
    let _guard = EndEnable;

    let policy = DavPolicy {
        calendars,
        files,
        applied_at: now_secs(),
        backfilled_at: None,
    };
    // Store first: a mint from here on carries the policy, and a crash
    // mid-apply leaves backfilledAt unset for the boot reconcile.
    if let Err(e) = store_policy(&policy) {
        return err_json("503 Service Unavailable", "not_ready", e);
    }
    let ids = match hosted_account_ids() {
        Ok(ids) => ids,
        Err(e) => {
            return err_json(
                "500 Internal Server Error",
                "error",
                format!("policy saved, but {e} — the daemon retries on its next start"),
            )
        }
    };
    let report = apply_policy(&engine, &ids, &policy);
    if !report.complete() {
        let failed: Vec<String> = report
            .failed
            .iter()
            .map(|(id, e)| format!("{id}: {e}"))
            .collect();
        return CliResponse {
            status: "502 Bad Gateway",
            content_type: "application/json",
            body: serde_json::json!({
                "ok": false,
                "error": {
                    "code": "engine",
                    "hint": format!(
                        "policy saved, but {} of {} account(s) did not take it ({}). The daemon \
                         finishes them on its next start, or run this command again.",
                        report.failed.len(),
                        report.total,
                        failed.join("; ")
                    ),
                },
                "accounts": report.to_json(),
            })
            .to_string(),
        };
    }
    if let Err(e) = stamp_backfilled(&policy, now_secs()) {
        return err_json("500 Internal Server Error", "error", e);
    }
    let mut out = status_payload();
    out["accounts"] = report.to_json();
    CliResponse::ok_json(out.to_string())
}

// ── Boot reconcile (imap_listeners.rs pattern) ──────────────────────────

/// Why the boot backfill must not run, or `None` when it should. Pure.
pub fn backfill_gate(policy: Option<&DavPolicy>) -> Option<String> {
    match policy {
        None => Some("the DAV policy was never applied (Stalwart defaults)".to_string()),
        Some(p) if p.backfilled_at.is_some() => {
            Some("every hosted account already carries the policy".to_string())
        }
        Some(_) => None,
    }
}

/// One backfill pass over `ids`. Returns the log lines and whether the
/// pass finished every account. Pure over the seam.
pub fn backfill_with(
    api: &dyn DavAccounts,
    ids: &[String],
    policy: &DavPolicy,
) -> (Vec<String>, bool) {
    let report = apply_policy(api, ids, policy);
    let mut lines = vec![format!(
        "{LOG_PREFIX} backfill calendars={} files={}: {} account(s), {} updated, {} unchanged, {} skipped, {} failed",
        policy.calendars.as_str(),
        policy.files.as_str(),
        report.total,
        report.updated.len(),
        report.unchanged,
        report.skipped.len(),
        report.failed.len()
    )];
    for (id, e) in &report.failed {
        lines.push(format!("{LOG_PREFIX} account {id} FAILED: {e}"));
    }
    (lines, report.complete())
}

/// Boot hook (main.rs, next to the IMAP listener reconcile). Linux
/// only; one detached thread; panics contained; never blocks boot.
pub fn spawn_startup_backfill() {
    if !super::supervisor::mail_supported() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("mail-dav-backfill".into())
        .spawn(|| {
            let res = std::panic::catch_unwind(run_startup_backfill_live);
            if res.is_err() {
                k2_core::log_debug!("{LOG_PREFIX} FAILED (daemon carries on): panicked");
            }
        });
    if let Err(e) = spawned {
        k2_core::log_debug!("{LOG_PREFIX} FAILED (daemon carries on): thread spawn: {e}");
    }
}

const API_WAIT_STEP_SECS: u64 = 3;
/// ~10 min: covers the IMAP reconcile's Stalwart restart.
const API_WAIT_TRIES: u32 = 200;

fn run_startup_backfill_live() {
    use super::supervisor;
    use std::sync::atomic::Ordering;

    let policy = match load_policy() {
        Ok(p) => p,
        Err(e) => {
            k2_core::log_debug!(
                "{LOG_PREFIX} skipped because the stored policy is unreadable: {e}"
            );
            return;
        }
    };
    if let Some(why) = backfill_gate(policy.as_ref()) {
        k2_core::log_debug!("{LOG_PREFIX} skipped because {why}");
        return;
    }
    let status = supervisor::current_status();
    if let Some(why) =
        super::imap_listeners::startup_gate(status.as_deref(), supervisor::enable_completed())
    {
        k2_core::log_debug!("{LOG_PREFIX} skipped because {why}");
        return;
    }
    let client = match supervisor::mgmt_client_from_row() {
        Ok(c) => c,
        Err(e) => {
            k2_core::log_debug!("{LOG_PREFIX} skipped because no management client: {e}");
            return;
        }
    };
    // Stalwart boots alongside the daemon, and the IMAP listener
    // reconcile (same boot, same enable lock) may hold the lock for a
    // Stalwart restart — wait for the API, then for the lock, instead
    // of skipping to the next boot.
    let mut last_err = String::from("enable lock held");
    let mut up = false;
    let mut locked = false;
    for _ in 0..API_WAIT_TRIES {
        if !supervisor::enable_running().load(Ordering::SeqCst) {
            if !up {
                match client.ping() {
                    Ok(()) => up = true,
                    Err(e) => last_err = e,
                }
            }
            if up && supervisor::try_begin_enable() {
                locked = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(API_WAIT_STEP_SECS));
    }
    if !locked {
        k2_core::log_debug!(
            "{LOG_PREFIX} skipped: within {}s the stalwart admin API {} — retried on the next \
             daemon start ({last_err})",
            API_WAIT_STEP_SECS * API_WAIT_TRIES as u64,
            if up {
                "answered but an enable kept the lock"
            } else {
                "did not answer"
            }
        );
        return;
    }
    struct EndEnable;
    impl Drop for EndEnable {
        fn drop(&mut self) {
            super::supervisor::end_enable();
        }
    }
    let _guard = EndEnable;
    // An owner enable/disable may have run while we waited: re-read
    // under the lock and only finish what is still unfinished.
    let policy = match load_policy() {
        Ok(p) => p,
        Err(e) => {
            k2_core::log_debug!(
                "{LOG_PREFIX} skipped because the stored policy is unreadable: {e}"
            );
            return;
        }
    };
    if let Some(why) = backfill_gate(policy.as_ref()) {
        k2_core::log_debug!("{LOG_PREFIX} skipped because {why}");
        return;
    }
    let policy = policy.expect("gate passed");
    let ids = match hosted_account_ids() {
        Ok(ids) => ids,
        Err(e) => {
            k2_core::log_debug!("{LOG_PREFIX} FAILED (retried on the next start): {e}");
            return;
        }
    };
    let (lines, complete) = backfill_with(&client, &ids, &policy);
    for line in lines {
        k2_core::log_debug!("{line}");
    }
    if complete {
        match stamp_backfilled(&policy, now_secs()) {
            Ok(true) => k2_core::log_debug!("{LOG_PREFIX} backfill complete"),
            Ok(false) => k2_core::log_debug!(
                "{LOG_PREFIX} backfill done, but a newer policy was saved meanwhile — left for it"
            ),
            Err(e) => k2_core::log_debug!("{LOG_PREFIX} could not stamp backfilledAt: {e}"),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Tests — fixture accounts only; no Stalwart, no network.
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn pol(calendars: OnOff, files: OnOff) -> DavPolicy {
        DavPolicy {
            calendars,
            files,
            applied_at: 1_000,
            backfilled_at: None,
        }
    }

    fn names(v: &serde_json::Value, key: &str) -> Vec<String> {
        v[key]
            .as_object()
            .unwrap_or_else(|| panic!("{key} must be an object: {v}"))
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn permission_names_are_exact_and_disjoint() {
        assert_eq!(
            CALENDAR_PERMISSIONS.len(),
            68,
            "every CalDAV/CardDAV/JMAP calendar+contacts name"
        );
        assert_eq!(
            FILES_PERMISSIONS.len(),
            18,
            "every davFile* + jmapFileNode* name"
        );
        for n in CALENDAR_PERMISSIONS {
            assert!(!FILES_PERMISSIONS.contains(n), "{n} in both lists");
            assert!(
                n.starts_with("davCal")
                    || n.starts_with("davCard")
                    || n.starts_with("jmapCalendar")
                    || n.starts_with("jmapParticipantIdentity")
                    || n.starts_with("jmapAddressBook")
                    || n.starts_with("jmapContactCard")
                    || n.starts_with("calendar"),
                "{n}"
            );
        }
        for n in FILES_PERMISSIONS {
            assert!(
                n.starts_with("davFile") || n.starts_with("jmapFileNode"),
                "{n}"
            );
        }
        // Shared discovery stays out of both: denying it would break mail-only JMAP.
        for shared in [
            "davPrincipalList",
            "davSyncCollection",
            "jmapPrincipalGet",
            "sysCalendarGet",
        ] {
            assert!(!CALENDAR_PERMISSIONS.contains(&shared));
            assert!(!FILES_PERMISSIONS.contains(&shared));
        }
    }

    #[test]
    fn inherit_becomes_merge_with_exactly_the_denied_names() {
        let p = pol(OnOff::On, OnOff::Off);
        let out = merge_permissions(
            Some(&serde_json::json!({"@type": "Inherit"})),
            &p.denied(),
            &p.allowed(),
        )
        .expect("merge")
        .expect("a change");
        assert_eq!(out["@type"], "Merge");
        assert_eq!(out["enabledPermissions"], serde_json::json!({}));
        let mut want: Vec<String> = FILES_PERMISSIONS.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(names(&out, "disabledPermissions"), want);
        // Absent = Inherit.
        let absent = merge_permissions(None, &p.denied(), &p.allowed()).expect("merge");
        assert_eq!(absent, Some(out));
        // Nothing denied on an Inherit account → no write.
        let all_on = pol(OnOff::On, OnOff::On);
        assert_eq!(
            merge_permissions(
                Some(&serde_json::json!({"@type": "Inherit"})),
                &all_on.denied(),
                &all_on.allowed()
            )
            .expect("merge"),
            None
        );
    }

    #[test]
    fn merge_keeps_unrelated_operator_entries() {
        let existing = serde_json::json!({
            "@type": "Merge",
            "enabledPermissions": {"impersonate": true, "davCalGet": true},
            "disabledPermissions": {"emailSend": true, "davFileGet": true},
        });
        // calendars off, files on.
        let p = pol(OnOff::Off, OnOff::On);
        let out = merge_permissions(Some(&existing), &p.denied(), &p.allowed())
            .expect("merge")
            .expect("change");
        assert_eq!(out["@type"], "Merge");
        let en = names(&out, "enabledPermissions");
        assert_eq!(
            en,
            vec!["impersonate".to_string()],
            "davCalGet left enabled list"
        );
        let dis = names(&out, "disabledPermissions");
        assert!(
            dis.contains(&"emailSend".to_string()),
            "operator deny kept: {dis:?}"
        );
        assert!(
            !dis.contains(&"davFileGet".to_string()),
            "files on takes it back: {dis:?}"
        );
        for n in CALENDAR_PERMISSIONS {
            assert!(dis.contains(&n.to_string()), "{n} denied");
        }
        // Idempotent: applying again writes nothing.
        assert_eq!(
            merge_permissions(Some(&out), &p.denied(), &p.allowed()).expect("merge"),
            None
        );
    }

    #[test]
    fn enable_everything_on_a_k2_only_merge_returns_to_inherit() {
        let p_off = pol(OnOff::Off, OnOff::Off);
        let first = merge_permissions(None, &p_off.denied(), &p_off.allowed())
            .expect("merge")
            .expect("change");
        let p_on = pol(OnOff::On, OnOff::On);
        let back = merge_permissions(Some(&first), &p_on.denied(), &p_on.allowed())
            .expect("merge")
            .expect("change");
        assert_eq!(back, serde_json::json!({"@type": "Inherit"}));
    }

    #[test]
    fn replace_stays_replace_and_unknown_types_refuse() {
        let existing = serde_json::json!({
            "@type": "Replace",
            "enabledPermissions": {"authenticate": true, "davFileGet": true},
            "disabledPermissions": {},
        });
        let p = pol(OnOff::On, OnOff::Off);
        let out = merge_permissions(Some(&existing), &p.denied(), &p.allowed())
            .expect("merge")
            .expect("change");
        assert_eq!(out["@type"], "Replace");
        let en = names(&out, "enabledPermissions");
        assert!(en.contains(&"authenticate".to_string()));
        assert!(
            en.contains(&"davCalGet".to_string()),
            "calendars on adds to a Replace list"
        );
        assert!(!en.contains(&"davFileGet".to_string()));
        let err = merge_permissions(
            Some(&serde_json::json!({"@type": "Weird"})),
            &p.denied(),
            &p.allowed(),
        )
        .expect_err("unknown type must refuse");
        assert!(err.contains("Weird"), "{err}");
        assert!(merge_permissions(Some(&serde_json::json!({"x": 1})), &[], &[]).is_err());
    }

    #[test]
    fn create_permissions_only_when_something_is_denied() {
        assert_eq!(create_permissions(&pol(OnOff::On, OnOff::On)), None);
        let v = create_permissions(&pol(OnOff::On, OnOff::Off)).expect("files off");
        assert_eq!(v["@type"], "Merge");
        assert_eq!(
            names(&v, "disabledPermissions").len(),
            FILES_PERMISSIONS.len()
        );
        let v = create_permissions(&pol(OnOff::Off, OnOff::Off)).expect("all off");
        assert_eq!(
            names(&v, "disabledPermissions").len(),
            FILES_PERMISSIONS.len() + CALENDAR_PERMISSIONS.len()
        );
    }

    #[test]
    fn policy_json_roundtrips_and_rejects_junk() {
        let p = DavPolicy {
            calendars: OnOff::On,
            files: OnOff::Off,
            applied_at: 7,
            backfilled_at: Some(9),
        };
        let s = serde_json::to_string(&p).expect("ser");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&s).expect("json"),
            serde_json::json!({"calendars":"on","files":"off","appliedAt":7,"backfilledAt":9})
        );
        assert_eq!(parse_policy_json(&s).expect("parse"), p);
        assert!(parse_policy_json(r#"{"calendars":"maybe","files":"off","appliedAt":1}"#).is_err());
        assert!(OnOff::parse("ON").is_ok());
        assert!(OnOff::parse("yes").is_err());
    }

    struct FakeAccounts {
        rows: Vec<serde_json::Value>,
        get_err: Option<String>,
        fail_set: Vec<String>,
        sets: Mutex<Vec<(String, serde_json::Value)>>,
        gets: Mutex<Vec<Vec<String>>>,
    }

    impl FakeAccounts {
        fn new(rows: Vec<serde_json::Value>) -> Self {
            Self {
                rows,
                get_err: None,
                fail_set: Vec::new(),
                sets: Mutex::new(Vec::new()),
                gets: Mutex::new(Vec::new()),
            }
        }
    }

    impl DavAccounts for FakeAccounts {
        fn get_accounts(&self, ids: &[String]) -> Result<Vec<serde_json::Value>, String> {
            self.gets.lock().unwrap().push(ids.to_vec());
            if let Some(e) = &self.get_err {
                return Err(e.clone());
            }
            Ok(self
                .rows
                .iter()
                .filter(|r| ids.iter().any(|id| r["id"] == id.as_str()))
                .cloned()
                .collect())
        }
        fn set_permissions(&self, id: &str, permissions: &serde_json::Value) -> Result<(), String> {
            if self.fail_set.iter().any(|f| f == id) {
                return Err("Account/set update rejected — forbidden".to_string());
            }
            self.sets
                .lock()
                .unwrap()
                .push((id.to_string(), permissions.clone()));
            Ok(())
        }
    }

    fn user(id: &str, name: &str, permissions: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "@type": "User",
            "name": name,
            "roles": {"@type": "User"},
            "permissions": permissions,
        })
    }

    #[test]
    fn apply_touches_users_only_and_never_the_service_or_admin_accounts() {
        let api = FakeAccounts::new(vec![
            user("a1", "alice", serde_json::json!({"@type": "Inherit"})),
            user(
                "a2",
                "bob",
                serde_json::json!({
                    "@type": "Merge",
                    "enabledPermissions": {},
                    "disabledPermissions": {"emailSend": true},
                }),
            ),
            serde_json::json!({"id": "svc", "@type": "User", "name": "k2-daemon", "roles": {"@type": "Admin"}, "permissions": {"@type": "Inherit"}}),
            user("adm", "admin", serde_json::json!({"@type": "Inherit"})),
            serde_json::json!({"id": "g1", "@type": "Group", "name": "team"}),
        ]);
        let ids: Vec<String> = ["a1", "a2", "svc", "adm", "g1", "gone"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let r = apply_policy(&api, &ids, &pol(OnOff::On, OnOff::Off));
        assert_eq!(r.total, 6);
        assert_eq!(r.updated, vec!["a1".to_string(), "a2".to_string()]);
        assert!(r.failed.is_empty(), "{r:?}");
        let skipped: Vec<&str> = r.skipped.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(skipped, vec!["svc", "adm", "g1", "gone"]);
        assert_eq!(
            api.gets.lock().unwrap().len(),
            1,
            "one read for every account"
        );
        let sets = api.sets.lock().unwrap();
        assert_eq!(sets.len(), 2);
        assert!(sets.iter().all(|(id, _)| id != "svc" && id != "adm"));
        let bob = &sets.iter().find(|(id, _)| id == "a2").expect("bob").1;
        assert_eq!(
            bob["disabledPermissions"]["emailSend"], true,
            "operator deny kept"
        );
        assert_eq!(bob["disabledPermissions"]["davFileGet"], true);
        assert!(bob["disabledPermissions"].get("davCalGet").is_none());
        // Wire shape: Merge with object maps.
        let alice = &sets.iter().find(|(id, _)| id == "a1").expect("alice").1;
        assert_eq!(alice["@type"], "Merge");
        assert_eq!(alice["enabledPermissions"], serde_json::json!({}));
    }

    #[test]
    fn apply_reports_every_failure_and_keeps_going() {
        let mut api = FakeAccounts::new(vec![
            user("a1", "alice", serde_json::json!({"@type": "Inherit"})),
            user("a2", "bob", serde_json::json!({"@type": "Inherit"})),
            user("a3", "carol", serde_json::json!({"@type": "Odd"})),
        ]);
        api.fail_set = vec!["a1".into()];
        let ids: Vec<String> = ["a1", "a2", "a3"].iter().map(|s| s.to_string()).collect();
        let r = apply_policy(&api, &ids, &pol(OnOff::Off, OnOff::Off));
        assert_eq!(r.updated, vec!["a2".to_string()]);
        assert_eq!(r.failed.len(), 2, "{r:?}");
        assert!(r.failed[0].1.contains("forbidden"));
        assert!(r.failed[1].1.contains("Odd"));
        assert!(!r.complete());

        let mut down = FakeAccounts::new(vec![]);
        down.get_err = Some("connection refused".into());
        let r = apply_policy(&down, &ids, &pol(OnOff::Off, OnOff::Off));
        assert_eq!(r.failed.len(), 3);
        assert!(down.sets.lock().unwrap().is_empty());

        // Empty id list: no engine call at all.
        let idle = FakeAccounts::new(vec![]);
        let r = apply_policy(&idle, &[], &pol(OnOff::Off, OnOff::Off));
        assert!(r.complete());
        assert!(idle.gets.lock().unwrap().is_empty());
    }

    #[test]
    fn backfill_gate_runs_once_per_applied_policy() {
        assert!(
            backfill_gate(None).is_some(),
            "never applied → nothing to backfill"
        );
        let mut p = pol(OnOff::On, OnOff::Off);
        assert!(backfill_gate(Some(&p)).is_none());
        p.backfilled_at = Some(5);
        assert!(backfill_gate(Some(&p)).is_some());
        let api = FakeAccounts::new(vec![user(
            "a1",
            "alice",
            serde_json::json!({"@type": "Inherit"}),
        )]);
        let (lines, complete) =
            backfill_with(&api, &["a1".to_string()], &pol(OnOff::On, OnOff::Off));
        assert!(complete);
        assert!(lines[0].contains("1 updated"), "{lines:?}");
    }

    #[test]
    fn reach_tls_alpn_or_caddy_else_names_the_proxy_paths() {
        assert_eq!(
            calendar_reach(true, Some("tls-alpn"), Some("mail.example.com"), false),
            (true, None)
        );
        assert_eq!(
            calendar_reach(true, Some("http-01"), Some("mail.example.com"), true),
            (true, None)
        );
        let (ok, why) = calendar_reach(true, Some("http-01"), Some("mail.example.com"), false);
        assert!(!ok);
        let why = why.expect("reason");
        for p in [
            "/.well-known/caldav",
            "/.well-known/carddav",
            "/dav/",
            "https://127.0.0.1:8443",
        ] {
            assert!(why.contains(p), "{why}");
        }
        let (ok, why) = calendar_reach(true, Some("tls-alpn"), Some("mail.box.k2.dev"), false);
        assert!(!ok, "never mail.<box>.k2.dev");
        assert!(why.expect("reason").contains("control plane"));
        assert!(!calendar_reach(false, Some("tls-alpn"), Some("mail.example.com"), false).0);
        assert_eq!(
            caldav_url(Some("mail.example.com")).as_deref(),
            Some("https://mail.example.com/.well-known/caldav")
        );
        assert_eq!(caldav_url(Some("mail.box.k2.dev")), None);
        assert_eq!(caldav_url(None), None);
    }

    #[test]
    fn post_validates_action_and_files_before_touching_anything() {
        let r = handle_dav_post(br#"{}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        let r = handle_dav_post(br#"{"action":"wipe"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        let r = handle_dav_post(br#"{"action":"enable","files":"maybe"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("--files"), "{}", r.body);
        let r = handle_dav_post(br#"{"action":"status","files":"on"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        let r = handle_dav_post(b"not json");
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }

    #[test]
    fn stored_policy_drives_mint_permissions_and_resets_with_the_row() {
        let _g = crate::mail::mail_server_test_lock();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM mail_server WHERE id = 1", [])
                .expect("clear");
        }
        assert_eq!(load_policy().expect("load"), None, "no row = never applied");
        assert_eq!(mint_permissions().expect("mint"), None);
        assert!(
            store_policy(&pol(OnOff::On, OnOff::Off)).is_err(),
            "no row → not installed"
        );
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, hostname, updated_at) \
                 VALUES (1, 'running', '0.16.10', 'mail.example.com', 1)",
                [],
            )
            .expect("seed");
        }
        assert_eq!(
            load_policy().expect("load"),
            None,
            "NULL column = never applied"
        );
        let p = pol(OnOff::Off, OnOff::Off);
        store_policy(&p).expect("store");
        assert_eq!(load_policy().expect("load"), Some(p.clone()));
        let perms = mint_permissions().expect("mint").expect("denies");
        assert_eq!(
            names(&perms, "disabledPermissions").len(),
            CALENDAR_PERMISSIONS.len() + FILES_PERMISSIONS.len()
        );
        // backfilledAt stamps only the policy it applied.
        assert!(stamp_backfilled(&p, 42).expect("stamp"));
        assert_eq!(
            load_policy().expect("load").expect("p").backfilled_at,
            Some(42)
        );
        let newer = DavPolicy {
            applied_at: 2_000,
            ..pol(OnOff::On, OnOff::Off)
        };
        store_policy(&newer).expect("store newer");
        assert!(
            !stamp_backfilled(&p, 43).expect("stamp"),
            "older pass must not stamp a newer policy"
        );
        assert_eq!(load_policy().expect("load").expect("p").backfilled_at, None);
        // GET shape.
        let v: serde_json::Value =
            serde_json::from_str(&handle_dav_get(&HashMap::new()).body).expect("json");
        assert_eq!(v["ok"], true);
        assert_eq!(v["enabled"], true);
        assert_eq!(v["files"], false);
        assert_eq!(v["policy"], "applied");
        assert_eq!(v["installed"], true);
        // Uninstall deletes the row → policy resets.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM mail_server WHERE id = 1", [])
                .expect("clear");
        }
        assert_eq!(load_policy().expect("load"), None);
        let v: serde_json::Value =
            serde_json::from_str(&handle_dav_get(&HashMap::new()).body).expect("json");
        assert_eq!(v["policy"], "default");
        assert_eq!(v["installed"], false);
        assert_eq!(v["reachable"], false);
    }

    #[test]
    fn hosted_account_ids_lists_active_and_retired_minted_accounts_only() {
        let tag = uuid::Uuid::new_v4().simple().to_string();
        let (active, retired) = (format!("dav-a-{tag}"), format!("dav-r-{tag}"));
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            for (id, acc, status) in [
                (format!("row-a-{tag}"), Some(active.clone()), "active"),
                (format!("row-r-{tag}"), Some(retired.clone()), "retired"),
                (format!("row-n-{tag}"), None, "active"),
            ] {
                conn.execute(
                    "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
                     owner_project_id, status, created_at) VALUES (?1, ?2, 'dom-dav', ?3, 'proj-dav', ?4, 1)",
                    rusqlite::params![id, format!("{id}@example.com"), acc, status],
                )
                .expect("seed address");
            }
        }
        let ids = hosted_account_ids().expect("list");
        assert!(ids.contains(&active), "{ids:?}");
        assert!(
            ids.contains(&retired),
            "retired mailboxes keep their account: {ids:?}"
        );
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "DELETE FROM mail_addresses WHERE id LIKE ?1",
                rusqlite::params![format!("row-%-{tag}")],
            )
            .expect("cleanup");
        }
    }

    #[test]
    fn unreadable_policy_fails_mint_loudly() {
        let _g = crate::mail::mail_server_test_lock();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM mail_server WHERE id = 1", [])
                .expect("clear");
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, hostname, dav_policy_json, updated_at) \
                 VALUES (1, 'running', '0.16.10', 'mail.example.com', '{\"calendars\":7}', 1)",
                [],
            )
            .expect("seed");
        }
        let err = mint_permissions().expect_err("junk policy must not mint silently");
        assert!(err.contains("dav_policy_json"), "{err}");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM mail_server WHERE id = 1", [])
                .expect("clear");
        }
    }
}
