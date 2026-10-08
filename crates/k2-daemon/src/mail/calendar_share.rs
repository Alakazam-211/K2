//! Calendars S4 — groups and calendar sharing on hosted mail
//! (prd-hostmail-calendars-v1 §4 S4, IT10; review CAL22, CAL33, CAL34).
//!
//! | route                                    | what                         |
//! |------------------------------------------|------------------------------|
//! | GET  /cli/mail/group                     | list groups / show one       |
//! | POST /cli/mail/group                     | create                       |
//! | GET  /cli/mail/group/members             | show one group               |
//! | POST /cli/mail/group/members             | add / remove members         |
//! | POST /cli/mail/group/delete              | delete (drops its shares)    |
//! | POST /cli/mail/calendar/manage           | create / rename a calendar   |
//! | POST /cli/mail/calendar/share            | share a calendar             |
//! | POST /cli/mail/calendar/unshare          | stop sharing                 |
//! | GET  /cli/mail/calendar/shares           | who a mailbox shares with    |
//! | GET|POST /cli/mail/calendar/sharing      | the server's `maxShares`     |
//!
//! **Gate (Rosson, 0.45.0 rule):** every route is on the mail-manage
//! surface (`mail_routes::is_mail_manage_surface`, Admin floor): owner and
//! admin always, an agent whose workspace has "manage hosted mail" too —
//! as capable as the owner, EXCEPT a change that widens the CALLING
//! agent's own access, which is 403 `owner_only`
//! ([`agent_creds::refuse_self_widening`]):
//! - `share --with X` where X is an inbox the caller's workspace can read
//!   (`k2 mail access` grant or primary), or a group holding one — unless
//!   the new rights are a subset of what X already has (narrowing);
//! - `group add <group> <addr>` where addr is such an inbox AND the group
//!   is the grantee of any calendar share.
//!
//! Unshare, member removal, group delete and `maxShares` never widen the
//! caller and are always allowed.
//!
//! **Stalwart facts (source, v0.16.20 = v0.16.25 for these files):**
//! - Calendar rights: `mayReadFreeBusy`, `mayReadItems`, `mayWriteAll`,
//!   `mayWriteOwn`, `mayUpdatePrivate`, `mayRSVP`, `mayShare`, `mayDelete`
//!   (`jmap-proto/src/object/calendar.rs:57-66`). There is no `mayAdmin`;
//!   re-sharing is `mayShare`, which K2 never grants. `mayWriteAll` maps
//!   to Modify+AddItems+ModifyItems+RemoveItems, `mayDelete` to
//!   Delete+RemoveItems (`:400-412`) — so in a person's own calendar app,
//!   `--write` can already remove events; `--editor` adds deleting the
//!   calendar itself. K2's agent view applies the stricter IT10 rule
//!   (event delete needs `mayDelete`).
//! - `shareWith` keys are account ids (users and groups alike — a group
//!   is an `x:Account` `@type:"Group"`); every grantee must be an
//!   existing Account and the total must be ≤ `maxShares`
//!   (`jmap/src/api/acl.rs:242-264`, checked on every `shareWith` change).
//!   A rights value of `null`/`{}` removes the grantee (`acl_patch`).
//! - `Calendar/*` needs only `urn:ietf:params:jmap:calendars`
//!   (`jmap-proto/src/request/method.rs`); K2's envelope also carries
//!   `…:principals` (S0.8 answered from source).
//! - Group JSON `{"@type":"Group","name","domainId"}`; membership is the
//!   USER's `memberGroupIds: {"<groupId>": true}`
//!   (`registry/src/schema/structs.rs` `UserAccount`).
//! - Inbound mail is `Permission::EmailReceive` = `emailReceive`, checked
//!   on the recipient account's token at delivery
//!   (`email/src/message/delivery.rs:170`).
//! - `x:Sharing` singleton `{maxShares (10), allowDirectoryQueries
//!   (false)}`.
//! - Calendar names are per-user preferences; before 0.16.19 a name set
//!   through the admin key lands on the ADMIN (`calendar/set.rs`
//!   `preferences_mut(access_token)` at v0.16.10 vs
//!   `personal_id(account_id)` from 0.16.19), so create/rename refuse on
//!   older Stalwart with "run `k2 hostmail upgrade` first".
//!
//! Groups live only in Stalwart (no K2 table).

use std::collections::{BTreeSet, HashMap};

use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::mail::agent_creds::{self, MailCaller};
use crate::mail::jmap::{self, StalwartClient};

// ── Errors ──────────────────────────────────────────────────────────────

fn err(status: &'static str, code: &str, hint: impl AsRef<str>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint.as_ref() } }).to_string(),
    }
}
fn usage(hint: impl AsRef<str>) -> CliResponse {
    err("400 Bad Request", "usage", hint)
}
fn not_found(hint: impl AsRef<str>) -> CliResponse {
    err("404 Not Found", "not_found", hint)
}
fn engine_err(hint: impl AsRef<str>) -> CliResponse {
    err("502 Bad Gateway", "engine", hint)
}
fn ok_json(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

// ── Rights ──────────────────────────────────────────────────────────────

/// The share levels K2 offers. Never `mayShare` (re-sharing).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShareLevel {
    FreeBusy,
    Read,
    Write,
    Editor,
}

impl ShareLevel {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "freebusy" | "free-busy" => Some(Self::FreeBusy),
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "editor" => Some(Self::Editor),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FreeBusy => "freebusy",
            Self::Read => "read",
            Self::Write => "write",
            Self::Editor => "editor",
        }
    }
    /// The `shareWith/<id>` value: `--freebusy` → mayReadFreeBusy;
    /// `--read` + mayReadItems; `--write` + mayWriteAll + mayRSVP (no
    /// mayDelete, no mayShare); `--editor` = write + mayDelete.
    pub fn rights(self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("mayReadFreeBusy".into(), json!(true));
        if self >= Self::Read {
            m.insert("mayReadItems".into(), json!(true));
        }
        if self >= Self::Write {
            m.insert("mayWriteAll".into(), json!(true));
            m.insert("mayRSVP".into(), json!(true));
        }
        if self >= Self::Editor {
            m.insert("mayDelete".into(), json!(true));
        }
        Value::Object(m)
    }
}

/// Right names whose value is `true`.
pub fn true_rights(v: Option<&Value>) -> BTreeSet<String> {
    v.and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(_, b)| b.as_bool() == Some(true))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The K2 level a stored rights object matches (`custom` otherwise).
pub fn level_of(v: &Value) -> &'static str {
    let set = true_rights(Some(v));
    for l in [ShareLevel::Editor, ShareLevel::Write, ShareLevel::Read, ShareLevel::FreeBusy] {
        if true_rights(Some(&l.rights())) == set {
            return l.as_str();
        }
    }
    "custom"
}

/// What a share lets its grantee do, as the agent view reads it
/// (commit 2 uses this): read = mayReadItems, freeBusy = mayReadFreeBusy
/// or mayReadItems, write = mayWriteAll, delete = mayDelete (IT10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShareRights {
    pub free_busy: bool,
    pub read: bool,
    pub write: bool,
    pub delete: bool,
}

impl ShareRights {
    pub fn from_json(v: &Value) -> Self {
        let t = true_rights(Some(v));
        let read = t.contains("mayReadItems");
        Self {
            free_busy: read || t.contains("mayReadFreeBusy"),
            read,
            write: t.contains("mayWriteAll"),
            delete: t.contains("mayDelete"),
        }
    }
    pub fn union(self, o: Self) -> Self {
        Self {
            free_busy: self.free_busy || o.free_busy,
            read: self.read || o.read,
            write: self.write || o.write,
            delete: self.delete || o.delete,
        }
    }
    pub fn any(self) -> bool {
        self.free_busy || self.read || self.write || self.delete
    }
}

// ── Directory (Stalwart accounts) ───────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    User,
    Group,
    Other,
}

/// One `x:Account` row.
#[derive(Debug, Clone)]
pub struct Account {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub domain_id: String,
    /// `emailAddress` (computed by Stalwart), lowercased.
    pub address: Option<String>,
    /// Users: `memberGroupIds`.
    pub member_of: Vec<String>,
    /// Groups: false when `emailReceive` is in `disabledPermissions`.
    pub receives_mail: bool,
    pub aliases: Vec<jmap::EmailAlias>,
}

pub struct Directory {
    pub accounts: Vec<Account>,
}

fn receives_mail(perms: Option<&Value>) -> bool {
    !perms
        .and_then(|p| p.get("disabledPermissions"))
        .and_then(|d| d.get("emailReceive"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

impl Directory {
    pub fn from_rows(rows: &[Value]) -> Self {
        let accounts = rows
            .iter()
            .filter_map(|r| {
                let id = r.get("id").and_then(Value::as_str)?.to_string();
                let kind = match r.get("@type").and_then(Value::as_str) {
                    Some("User") => Kind::User,
                    Some("Group") => Kind::Group,
                    _ => Kind::Other,
                };
                Some(Account {
                    id,
                    kind,
                    name: r.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    domain_id: r.get("domainId").and_then(Value::as_str).unwrap_or("").to_string(),
                    address: r
                        .get("emailAddress")
                        .and_then(Value::as_str)
                        .map(|s| s.trim().to_ascii_lowercase())
                        .filter(|s| !s.is_empty()),
                    member_of: jmap::set_object_keys(r.get("memberGroupIds")),
                    receives_mail: receives_mail(r.get("permissions")),
                    aliases: jmap::parse_email_aliases(r.get("aliases")),
                })
            })
            .collect();
        Self { accounts }
    }

    pub fn by_id(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn groups(&self) -> impl Iterator<Item = &Account> {
        self.accounts.iter().filter(|a| a.kind == Kind::Group)
    }

    /// Group ids `account_id` belongs to.
    pub fn groups_of(&self, account_id: &str) -> Vec<String> {
        self.by_id(account_id).map(|a| a.member_of.clone()).unwrap_or_default()
    }

    /// Accounts whose `memberGroupIds` include `group_id`.
    pub fn members_of(&self, group_id: &str) -> Vec<&Account> {
        self.accounts
            .iter()
            .filter(|a| a.member_of.iter().any(|g| g == group_id))
            .collect()
    }

    /// A group by full address, or by bare name when exactly one group
    /// has it.
    pub fn find_group(&self, raw: &str) -> Result<&Account, CliResponse> {
        let raw = raw.trim().to_ascii_lowercase();
        if raw.is_empty() {
            return Err(usage("missing group — its address (team@example.com) or name"));
        }
        if raw.contains('@') {
            return self
                .groups()
                .find(|g| g.address.as_deref() == Some(raw.as_str()))
                .ok_or_else(|| not_found(format!("no group '{raw}' on this server")));
        }
        let named: Vec<&Account> =
            self.groups().filter(|g| g.name.eq_ignore_ascii_case(&raw)).collect();
        match named.len() {
            1 => Ok(named[0]),
            0 => Err(not_found(format!("no group '{raw}' on this server"))),
            _ => Err(usage(format!(
                "several groups are named '{raw}' — use the full address: {}",
                named.iter().filter_map(|g| g.address.clone()).collect::<Vec<_>>().join(", ")
            ))),
        }
    }
}

// ── The engine seam (fixture-faked in tests) ────────────────────────────

pub trait ShareApi {
    /// Every `x:Account` row (users and groups).
    fn accounts(&self) -> Result<Vec<Value>, String>;
    fn group_create(
        &self,
        name: &str,
        domain_id: &str,
        description: &str,
        permissions: Option<&Value>,
    ) -> Result<String, String>;
    fn account_update(&self, id: &str, patch: Value) -> Result<(), String>;
    fn account_destroy(&self, id: &str) -> Result<(), String>;
    /// Every mailing list address on the server.
    fn list_addresses(&self) -> Result<Vec<String>, String>;
    fn catchall(&self, domain_id: &str) -> Result<Option<String>, String>;
    /// `Calendar/get` with `shareWith` on one account.
    fn calendars(&self, account: &str) -> Result<Vec<Value>, String>;
    fn calendar_set(&self, account: &str, args: Value) -> Result<Value, String>;
    fn sharing_get(&self) -> Result<Value, String>;
    fn sharing_set(&self, patch: Value) -> Result<(), String>;
    fn reload_settings(&self) -> Result<(), String>;
}

impl ShareApi for StalwartClient {
    fn accounts(&self) -> Result<Vec<Value>, String> {
        self.account_rows()
    }
    fn group_create(
        &self,
        name: &str,
        domain_id: &str,
        description: &str,
        permissions: Option<&Value>,
    ) -> Result<String, String> {
        StalwartClient::group_create(self, name, domain_id, description, permissions)
    }
    fn account_update(&self, id: &str, patch: Value) -> Result<(), String> {
        StalwartClient::account_update(self, id, patch)
    }
    fn account_destroy(&self, id: &str) -> Result<(), String> {
        StalwartClient::account_destroy(self, id)
    }
    fn list_addresses(&self) -> Result<Vec<String>, String> {
        let ids = self.mailing_list_query(None)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self
            .mailing_list_get(&ids)?
            .into_iter()
            .filter_map(|l| l.email_address.map(|e| e.trim().to_ascii_lowercase()))
            .collect())
    }
    fn catchall(&self, domain_id: &str) -> Result<Option<String>, String> {
        self.domain_get_catchall(domain_id)
    }
    fn calendars(&self, account: &str) -> Result<Vec<Value>, String> {
        self.calendar_list_shares(account)
    }
    fn calendar_set(&self, account: &str, args: Value) -> Result<Value, String> {
        StalwartClient::calendar_set(self, account, args)
    }
    fn sharing_get(&self) -> Result<Value, String> {
        StalwartClient::sharing_get(self)
    }
    fn sharing_set(&self, patch: Value) -> Result<(), String> {
        StalwartClient::sharing_set(self, patch)
    }
    fn reload_settings(&self) -> Result<(), String> {
        self.action_reload_settings()
    }
}

// ── Request context ─────────────────────────────────────────────────────

/// An active hosted address with its Stalwart account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedAddr {
    pub address: String,
    pub account_id: String,
}

pub struct Ctx<'a> {
    pub api: &'a dyn ShareApi,
    pub caller: MailCaller,
    /// Accounts the CALLER's workspace can read (`k2 mail access` grant
    /// or primary). Empty for the owner.
    pub own_accounts: Vec<String>,
    /// Active hosted addresses with an account.
    pub hosted: Vec<HostedAddr>,
    /// Every `mail_addresses` address, any status (retired rows reserve
    /// their address).
    pub reserved: Vec<String>,
    /// (Stalwart domain id, domain name) of K2's hosted domains.
    pub domains: Vec<(String, String)>,
    pub installed_version: Option<String>,
}

/// Active hosted addresses with a Stalwart account (K2 DB).
pub fn load_hosted() -> Result<Vec<HostedAddr>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT address, stalwart_account_id FROM mail_addresses \
             WHERE status = 'active' AND stalwart_account_id IS NOT NULL \
               AND TRIM(stalwart_account_id) != '' \
             ORDER BY created_at, address",
        )
        .map_err(|e| format!("list hosted addresses: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(HostedAddr { address: r.get::<_, String>(0)?, account_id: r.get::<_, String>(1)? })
        })
        .map_err(|e| format!("list hosted addresses: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("list hosted addresses: {e}"))
}

fn load_reserved() -> Result<Vec<String>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT address FROM mail_addresses")
        .map_err(|e| format!("list addresses: {e}"))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| format!("list addresses: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("list addresses: {e}"))
}

fn load_domains() -> Vec<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    crate::mail::domains::load_all_domains(&conn)
        .into_iter()
        .filter_map(|d| {
            d.stalwart_domain_id
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(|id| (id, d.domain))
        })
        .collect()
}

/// The accounts `workspace_uuid` can read (hosted, primary or grant).
pub fn caller_accounts(workspace_uuid: &str) -> Vec<String> {
    crate::mail::access::readable_hosted(workspace_uuid)
        .into_iter()
        .map(|(_addr, account)| account)
        .collect()
}

impl<'a> Ctx<'a> {
    /// The live context. An agent without mail-manage is refused here
    /// too (the dispatcher already gates it; this is the belt).
    pub fn live(api: &'a dyn ShareApi) -> Result<Self, CliResponse> {
        let caller = agent_creds::mail_caller();
        if let MailCaller::Agent { .. } = caller {
            return Err(agent_creds::refuse_needs_mail_manage(
                "manage groups and calendar sharing",
            ));
        }
        let own_accounts = match &caller {
            MailCaller::ItAgent { workspace_uuid } => caller_accounts(workspace_uuid),
            _ => Vec::new(),
        };
        Ok(Self {
            api,
            caller,
            own_accounts,
            hosted: load_hosted().map_err(engine_err)?,
            reserved: load_reserved().map_err(engine_err)?,
            domains: load_domains(),
            installed_version: crate::mail::supervisor::row_field("installed_version"),
        })
    }

    fn hosted_by_address(&self, raw: &str) -> Option<&HostedAddr> {
        let raw = raw.trim().to_ascii_lowercase();
        self.hosted.iter().find(|h| h.address.eq_ignore_ascii_case(&raw))
    }

    fn hosted_by_account(&self, account: &str) -> Option<&HostedAddr> {
        self.hosted.iter().find(|h| h.account_id == account)
    }

    /// The owner mailbox of a calendar verb: an active hosted address.
    fn owner(&self, raw: &str) -> Result<HostedAddr, CliResponse> {
        if raw.trim().is_empty() {
            return Err(usage("missing owner address — the hosted mailbox that owns the calendar"));
        }
        self.hosted_by_address(raw)
            .cloned()
            .ok_or_else(|| not_found(format!("no hosted address '{}' on this server", raw.trim())))
    }

    fn is_it_agent(&self) -> bool {
        matches!(self.caller, MailCaller::ItAgent { .. })
    }

    fn directory(&self) -> Result<Directory, CliResponse> {
        self.api.accounts().map(|rows| Directory::from_rows(&rows)).map_err(engine_err)
    }

    fn label_of(&self, dir: &Directory, id: &str) -> (String, &'static str) {
        if let Some(h) = self.hosted_by_account(id) {
            return (h.address.clone(), "address");
        }
        match dir.by_id(id) {
            Some(a) if a.kind == Kind::Group => {
                (a.address.clone().unwrap_or_else(|| a.name.clone()), "group")
            }
            Some(a) => (a.address.clone().unwrap_or_else(|| a.name.clone()), "account"),
            None => (id.to_string(), "unknown"),
        }
    }
}

/// A `--with` target.
#[derive(Debug, Clone)]
struct Grantee {
    id: String,
    label: String,
    is_group: bool,
}

fn resolve_grantee(ctx: &Ctx<'_>, dir: &Directory, raw: &str) -> Result<Grantee, CliResponse> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(usage("missing --with — a hosted address or a group on this server"));
    }
    if let Some(h) = ctx.hosted_by_address(raw) {
        return Ok(Grantee { id: h.account_id.clone(), label: h.address.clone(), is_group: false });
    }
    match dir.find_group(raw) {
        Ok(g) => Ok(Grantee {
            id: g.id.clone(),
            label: g.address.clone().unwrap_or_else(|| g.name.clone()),
            is_group: true,
        }),
        Err(r) if r.status.starts_with("400") => Err(r),
        Err(_) => Err(not_found(format!(
            "'{raw}' is not a hosted address or a group on this server — share with \
             'k2 hostmail create' addresses or 'k2 hostmail group' groups only"
        ))),
    }
}

/// True when `grantee` is (or contains) an inbox the caller can read.
fn grantee_is_callers(ctx: &Ctx<'_>, dir: &Directory, g: &Grantee) -> bool {
    if ctx.own_accounts.is_empty() {
        return false;
    }
    if !g.is_group {
        return ctx.own_accounts.iter().any(|a| a == &g.id);
    }
    dir.members_of(&g.id).iter().any(|m| ctx.own_accounts.iter().any(|a| a == &m.id))
}

// ── Calendars of an owner ───────────────────────────────────────────────

fn cal_id(c: &Value) -> &str {
    c.get("id").and_then(Value::as_str).unwrap_or("")
}
fn cal_name(c: &Value) -> &str {
    c.get("name").and_then(Value::as_str).unwrap_or("")
}

/// `--calendar <id|name>` on the owner's calendars; absent → the default
/// one (`isDefault`, else the only, else the first by `sortOrder`).
fn pick_calendar<'v>(cals: &'v [Value], raw: Option<&str>, owner: &str) -> Result<&'v Value, CliResponse> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(r) => {
            if let Some(c) = cals.iter().find(|c| cal_id(c) == r) {
                return Ok(c);
            }
            let named: Vec<&Value> =
                cals.iter().filter(|c| cal_name(c).trim().eq_ignore_ascii_case(r)).collect();
            if named.len() == 1 {
                return Ok(named[0]);
            }
            Err(usage(format!(
                "no single calendar '{r}' on {owner} — available: {}",
                if cals.is_empty() {
                    "(none)".to_string()
                } else {
                    cals.iter()
                        .map(|c| format!("{} ({})", cal_name(c), cal_id(c)))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            )))
        }
        None => {
            let id = crate::mail::calendar::default_calendar(cals).ok_or_else(|| {
                err(
                    "409 Conflict",
                    "no_calendar",
                    format!(
                        "{owner} has no calendar yet — create one with 'k2 hostmail calendar \
                         create {owner} <name>'"
                    ),
                )
            })?;
            cals.iter()
                .find(|c| cal_id(c) == id)
                .ok_or_else(|| engine_err("default calendar vanished from the list"))
        }
    }
}

fn set_failure(reply: &Value, key: &str, id: &str) -> Option<String> {
    let e = reply.get(key)?.get(id)?;
    let t = e.get("type").and_then(Value::as_str).unwrap_or("unknown");
    let d = e.get("description").and_then(Value::as_str).unwrap_or("");
    Some(if d.is_empty() { t.to_string() } else { format!("{t}: {d}") })
}

/// Every hosted account's calendars that name `grantee_id` in shareWith.
fn shares_to(ctx: &Ctx<'_>, grantee_id: &str) -> Result<Vec<(HostedAddr, String, String)>, CliResponse> {
    let mut out = Vec::new();
    for h in &ctx.hosted {
        if h.account_id == grantee_id {
            continue;
        }
        let cals = ctx.api.calendars(&h.account_id).map_err(engine_err)?;
        for c in &cals {
            if c.get("shareWith").and_then(|s| s.get(grantee_id)).is_some_and(|v| !v.is_null()) {
                out.push((h.clone(), cal_id(c).to_string(), cal_name(c).to_string()));
            }
        }
    }
    Ok(out)
}

// ── Version gate (0.16.10 per-user property bug) ────────────────────────

/// Calendar create/rename write a per-user name; Stalwart before 0.16.19
/// stores it on the admin. `installed` is `mail_server.installed_version`
/// (a fork build may carry a `-k2.N` suffix: only the leading x.y.z is
/// compared). Unknown → refused.
pub fn per_user_props_ok(installed: Option<&str>) -> Result<(), CliResponse> {
    let refuse = |v: &str| {
        err(
            "409 Conflict",
            "upgrade_required",
            format!(
                "this server runs Stalwart {v}; before 0.16.19 a calendar name set by K2 lands \
                 on the admin account, not the mailbox — run `k2 hostmail upgrade` first"
            ),
        )
    };
    let Some(v) = installed.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(refuse("(unknown version)"));
    };
    let core = v.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<Option<u64>> = core.split('.').map(|p| p.parse::<u64>().ok()).collect();
    match parts.as_slice() {
        [Some(a), Some(b), Some(c)] if (*a, *b, *c) >= (0, 16, 19) => Ok(()),
        _ => Err(refuse(v)),
    }
}

// ── Groups ──────────────────────────────────────────────────────────────

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GroupBody {
    pub name: Option<String>,
    pub domain: Option<String>,
    /// Keep inbound mail to the group address on (default off).
    pub mail: Option<bool>,
    pub group: Option<String>,
    pub add: Option<Vec<String>>,
    pub remove: Option<Vec<String>>,
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, CliResponse> {
    serde_json::from_slice(body).map_err(|e| usage(format!("invalid JSON body: {e}")))
}

fn group_json(ctx: &Ctx<'_>, dir: &Directory, g: &Account) -> Value {
    let members: Vec<String> = dir
        .members_of(&g.id)
        .iter()
        .map(|m| ctx.label_of(dir, &m.id).0)
        .collect();
    json!({
        "id": g.id,
        "address": g.address.clone().unwrap_or_else(|| g.name.clone()),
        "name": g.name,
        "members": members,
        "receivesMail": g.receives_mail,
    })
}

/// GET /cli/mail/group (`group` param → one) and /cli/mail/group/members.
pub fn group_get_with(ctx: &Ctx<'_>, group: Option<&str>) -> CliResponse {
    let dir = match ctx.directory() {
        Ok(d) => d,
        Err(r) => return r,
    };
    match group.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => match dir.find_group(raw) {
            Ok(g) => ok_json(json!({ "ok": true, "group": group_json(ctx, &dir, g) })),
            Err(r) => r,
        },
        None => ok_json(json!({
            "ok": true,
            "groups": dir.groups().map(|g| group_json(ctx, &dir, g)).collect::<Vec<_>>(),
        })),
    }
}

/// The `{name, domain}` of a create → (local, domain name, domain id).
fn group_target(ctx: &Ctx<'_>, b: &GroupBody) -> Result<(String, String, String), CliResponse> {
    let raw = b.name.as_deref().map(str::trim).unwrap_or("");
    if raw.is_empty() {
        return Err(usage("missing group name — e.g. 'team' or team@example.com"));
    }
    let (local, from_addr) = match raw.split_once('@') {
        Some((l, d)) => (l.to_string(), Some(d.to_ascii_lowercase())),
        None => (raw.to_string(), None),
    };
    let flag = b.domain.as_deref().map(|d| d.trim().to_ascii_lowercase()).filter(|d| !d.is_empty());
    let domain = match (from_addr, flag) {
        (Some(a), Some(f)) if a != f => {
            return Err(usage(format!("'{raw}' is on {a}, but --domain says {f}")))
        }
        (Some(a), _) => a,
        (None, Some(f)) => f,
        (None, None) => match ctx.domains.len() {
            1 => ctx.domains[0].1.clone(),
            0 => return Err(not_found("no hosted mail domain on this server — 'k2 hostmail domain add' first")),
            _ => {
                return Err(usage(format!(
                    "this server hosts several domains — pass --domain: {}",
                    ctx.domains.iter().map(|d| d.1.clone()).collect::<Vec<_>>().join(", ")
                )))
            }
        },
    };
    let address = crate::mail::addresses::normalize_address(&format!("{local}@{domain}"))
        .map_err(crate::mail::hosted::addr_err)?;
    let (local, domain) = address.split_once('@').map(|(l, d)| (l.to_string(), d.to_string()))
        .ok_or_else(|| usage("invalid group address"))?;
    let domain_id = ctx
        .domains
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(&domain))
        .map(|(id, _)| id.clone())
        .ok_or_else(|| not_found(format!("no hosted mail domain '{domain}' on this server")))?;
    Ok((local, domain, domain_id))
}

/// CAL33: a group claims `name@domain`, which must be free of mailboxes
/// (any status), other accounts/groups, account aliases, mailing lists
/// and the domain's catch-all destination.
fn group_collision(
    ctx: &Ctx<'_>,
    dir: &Directory,
    local: &str,
    domain_id: &str,
    address: &str,
) -> Result<(), CliResponse> {
    let exists = |what: &str| err("409 Conflict", "exists", format!("'{address}' is already {what}"));
    if ctx.reserved.iter().any(|a| a.eq_ignore_ascii_case(address)) {
        return Err(exists("a mailbox (or reserved by a retired one)"));
    }
    for a in &dir.accounts {
        let same_name = a.name.eq_ignore_ascii_case(local) && a.domain_id == domain_id;
        if same_name || a.address.as_deref() == Some(address) {
            return Err(exists(if a.kind == Kind::Group { "a group" } else { "an account" }));
        }
        if a.aliases.iter().any(|al| {
            al.enabled && al.name.eq_ignore_ascii_case(local) && al.domain_id == domain_id
        }) {
            return Err(exists("an alias of another mailbox"));
        }
    }
    let lists = ctx.api.list_addresses().map_err(engine_err)?;
    if lists.iter().any(|l| l.eq_ignore_ascii_case(address)) {
        return Err(exists("a mailing list"));
    }
    match ctx.api.catchall(domain_id) {
        Ok(Some(c)) if c.eq_ignore_ascii_case(address) => {
            return Err(exists("this domain's catch-all destination"))
        }
        Ok(_) => {}
        Err(e) => return Err(engine_err(e)),
    }
    Ok(())
}

/// The permissions a new group gets: inbound mail off unless `--mail`.
pub fn group_permissions(mail: bool) -> Option<Value> {
    if mail {
        None
    } else {
        Some(json!({
            "@type": "Merge",
            "enabledPermissions": {},
            "disabledPermissions": { "emailReceive": true },
        }))
    }
}

/// POST /cli/mail/group `{name, domain?, mail?}`.
pub fn group_create_with(ctx: &Ctx<'_>, b: &GroupBody) -> CliResponse {
    if b.group.is_some() || b.add.is_some() || b.remove.is_some() {
        return usage("create takes name, domain and mail — add members with 'k2 hostmail group add'");
    }
    let (local, domain, domain_id) = match group_target(ctx, b) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let address = format!("{local}@{domain}");
    let dir = match ctx.directory() {
        Ok(d) => d,
        Err(r) => return r,
    };
    if let Err(r) = group_collision(ctx, &dir, &local, &domain_id, &address) {
        return r;
    }
    let mail = b.mail.unwrap_or(false);
    let perms = group_permissions(mail);
    match ctx.api.group_create(&local, &domain_id, "k2 group (shared calendars)", perms.as_ref()) {
        Ok(id) => ok_json(json!({
            "ok": true,
            "id": id,
            "address": address,
            "receivesMail": mail,
            "members": [],
        })),
        Err(e) => engine_err(e),
    }
}

fn member_set_value(ids: &[String]) -> Value {
    let mut m = serde_json::Map::new();
    for i in ids {
        m.insert(i.clone(), json!(true));
    }
    Value::Object(m)
}

/// POST /cli/mail/group/members `{group, add?:[], remove?:[]}`.
pub fn group_members_with(ctx: &Ctx<'_>, b: &GroupBody) -> CliResponse {
    if b.name.is_some() || b.domain.is_some() || b.mail.is_some() {
        return usage("members takes group, add and remove");
    }
    let add = b.add.clone().unwrap_or_default();
    let remove = b.remove.clone().unwrap_or_default();
    if add.is_empty() && remove.is_empty() {
        return usage("give add[] and/or remove[] member addresses");
    }
    let dir = match ctx.directory() {
        Ok(d) => d,
        Err(r) => return r,
    };
    let group = match dir.find_group(b.group.as_deref().unwrap_or("")) {
        Ok(g) => g.clone(),
        Err(r) => return r,
    };
    let resolve = |raw: &String| -> Result<HostedAddr, CliResponse> {
        ctx.hosted_by_address(raw).cloned().ok_or_else(|| {
            usage(format!(
                "member '{}' must be a hosted address on this server ('k2 hostmail create')",
                raw.trim()
            ))
        })
    };
    let mut adds = Vec::new();
    for a in &add {
        match resolve(a) {
            Ok(h) => adds.push(h),
            Err(r) => return r,
        }
    }
    let mut removes = Vec::new();
    for a in &remove {
        match resolve(a) {
            Ok(h) => removes.push(h),
            Err(r) => return r,
        }
    }
    // Self-widening: an IT agent may not put an inbox it can read into
    // a group that is the grantee of a calendar share.
    if ctx.is_it_agent() {
        let own: Vec<&HostedAddr> = adds
            .iter()
            .filter(|h| ctx.own_accounts.iter().any(|a| a == &h.account_id))
            .filter(|h| !dir.groups_of(&h.account_id).contains(&group.id))
            .collect();
        if !own.is_empty() {
            let shares = match shares_to(ctx, &group.id) {
                Ok(s) => s,
                Err(r) => return r,
            };
            if !shares.is_empty() {
                return agent_creds::refuse_self_widening(&format!(
                    "adding {} (an inbox this agent's workspace can read) to group {}, which \
                     has {} calendar share(s),",
                    own.iter().map(|h| h.address.clone()).collect::<Vec<_>>().join(", "),
                    group.address.clone().unwrap_or_else(|| group.name.clone()),
                    shares.len()
                ));
            }
        }
    }
    let mut changed = 0usize;
    let mut touched: Vec<&HostedAddr> = Vec::new();
    for h in adds.iter().chain(removes.iter()) {
        if !touched.iter().any(|t| t.account_id == h.account_id) {
            touched.push(h);
        }
    }
    for h in touched {
        let current = dir.groups_of(&h.account_id);
        let mut next = current.clone();
        if adds.iter().any(|a| a.account_id == h.account_id) && !next.contains(&group.id) {
            next.push(group.id.clone());
        }
        if removes.iter().any(|a| a.account_id == h.account_id) {
            next.retain(|g| g != &group.id);
        }
        if next == current {
            continue;
        }
        if let Err(e) = ctx
            .api
            .account_update(&h.account_id, json!({ "memberGroupIds": member_set_value(&next) }))
        {
            return engine_err(format!("updating {}: {e}", h.address));
        }
        changed += 1;
    }
    // Members after the change (re-derived, not re-read).
    let mut members: Vec<String> = dir
        .members_of(&group.id)
        .iter()
        .map(|m| ctx.label_of(&dir, &m.id).0)
        .filter(|a| !removes.iter().any(|r| r.address.eq_ignore_ascii_case(a)))
        .collect();
    for a in &adds {
        if !members.iter().any(|m| m.eq_ignore_ascii_case(&a.address)) {
            members.push(a.address.clone());
        }
    }
    ok_json(json!({
        "ok": true,
        "group": group.address.clone().unwrap_or_else(|| group.name.clone()),
        "members": members,
        "changed": changed,
    }))
}

/// POST /cli/mail/group/delete `{group}`: drop the group from every
/// hosted calendar's shareWith (a dangling grantee would make every later
/// share change on that calendar fail Stalwart's account check), take it
/// off its members, then destroy it.
pub fn group_delete_with(ctx: &Ctx<'_>, b: &GroupBody) -> CliResponse {
    let dir = match ctx.directory() {
        Ok(d) => d,
        Err(r) => return r,
    };
    let group = match dir.find_group(b.group.as_deref().unwrap_or("")) {
        Ok(g) => g.clone(),
        Err(r) => return r,
    };
    let shares = match shares_to(ctx, &group.id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    for (owner, cal, _name) in &shares {
        let path = format!("shareWith/{}", group.id);
        let reply = match ctx.api.calendar_set(
            &owner.account_id,
            json!({ "update": { cal.clone(): { path: Value::Null } } }),
        ) {
            Ok(r) => r,
            Err(e) => return engine_err(e),
        };
        if let Some(m) = set_failure(&reply, "notUpdated", cal) {
            return engine_err(format!("removing the group's share on {}: {m}", owner.address));
        }
    }
    let members = dir.members_of(&group.id);
    for m in &members {
        let next: Vec<String> = m.member_of.iter().filter(|g| *g != &group.id).cloned().collect();
        if let Err(e) = ctx.api.account_update(&m.id, json!({ "memberGroupIds": member_set_value(&next) })) {
            return engine_err(format!("taking {} out of the group: {e}", ctx.label_of(&dir, &m.id).0));
        }
    }
    if let Err(e) = ctx.api.account_destroy(&group.id) {
        return engine_err(e);
    }
    ok_json(json!({
        "ok": true,
        "group": group.address.clone().unwrap_or_else(|| group.name.clone()),
        "deleted": true,
        "sharesRemoved": shares.len(),
        "membersRemoved": members.len(),
    }))
}

// ── Calendars: create / rename ──────────────────────────────────────────

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CalBody {
    pub action: Option<String>,
    pub owner: Option<String>,
    pub calendar: Option<String>,
    pub name: Option<String>,
    pub color: Option<String>,
    pub with: Option<String>,
    pub level: Option<String>,
    #[serde(rename = "maxShares")]
    pub max_shares: Option<u64>,
}

fn check_name(name: &str) -> Result<String, CliResponse> {
    let n = name.trim();
    if n.is_empty() {
        return Err(usage("missing calendar name"));
    }
    if n.len() > 255 {
        return Err(usage("calendar name is too long (max 255 bytes)"));
    }
    Ok(n.to_string())
}

fn check_color(c: Option<&str>) -> Result<Option<String>, CliResponse> {
    match c.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(c) if c.len() <= 64 && c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '#') => {
            Ok(Some(c.to_string()))
        }
        Some(c) => Err(usage(format!("'{c}' is not a color — e.g. #3366ff or blue"))),
    }
}

/// POST /cli/mail/calendar/manage `{action: create|rename, owner, name,
/// calendar? (rename), color?}`.
pub fn manage_with(ctx: &Ctx<'_>, b: &CalBody) -> CliResponse {
    if b.with.is_some() || b.level.is_some() || b.max_shares.is_some() {
        return usage("manage takes action, owner, name, calendar and color");
    }
    let action = b.action.as_deref().map(str::trim).unwrap_or("");
    if action != "create" && action != "rename" {
        return usage("action must be create or rename");
    }
    let owner = match ctx.owner(b.owner.as_deref().unwrap_or("")) {
        Ok(o) => o,
        Err(r) => return r,
    };
    let name = match check_name(b.name.as_deref().unwrap_or("")) {
        Ok(n) => n,
        Err(r) => return r,
    };
    let color = match check_color(b.color.as_deref()) {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(r) = per_user_props_ok(ctx.installed_version.as_deref()) {
        return r;
    }
    let cals = match ctx.api.calendars(&owner.account_id) {
        Ok(c) => c,
        Err(e) => return engine_err(e),
    };
    if action == "create" {
        if b.calendar.is_some() {
            return usage("create takes a name, not --calendar");
        }
        if cals.iter().any(|c| cal_name(c).trim().eq_ignore_ascii_case(&name)) {
            return err("409 Conflict", "exists", format!("{} already has a calendar named '{name}'", owner.address));
        }
        let mut cal = json!({ "name": name });
        if let Some(c) = &color {
            cal["color"] = json!(c);
        }
        let reply = match ctx.api.calendar_set(&owner.account_id, json!({ "create": { "k2new": cal } })) {
            Ok(r) => r,
            Err(e) => return engine_err(e),
        };
        if let Some(m) = set_failure(&reply, "notCreated", "k2new") {
            return engine_err(format!("Calendar/set create: {m}"));
        }
        let Some(id) = reply.pointer("/created/k2new/id").and_then(Value::as_str) else {
            return engine_err("Calendar/set create: no created id in reply");
        };
        return ok_json(json!({
            "ok": true, "owner": owner.address, "calendar": { "id": id, "name": name },
            "created": true,
        }));
    }
    let cal = match pick_calendar(&cals, Some(b.calendar.as_deref().unwrap_or("")), &owner.address) {
        Ok(c) if b.calendar.as_deref().is_some_and(|s| !s.trim().is_empty()) => c.clone(),
        Ok(_) => return usage("rename needs the calendar (id or current name)"),
        Err(r) => return r,
    };
    let id = cal_id(&cal).to_string();
    let mut patch = json!({ "name": name });
    if let Some(c) = &color {
        patch["color"] = json!(c);
    }
    let reply = match ctx.api.calendar_set(&owner.account_id, json!({ "update": { id.clone(): patch } })) {
        Ok(r) => r,
        Err(e) => return engine_err(e),
    };
    if let Some(m) = set_failure(&reply, "notUpdated", &id) {
        return engine_err(format!("Calendar/set update: {m}"));
    }
    ok_json(json!({
        "ok": true, "owner": owner.address,
        "calendar": { "id": id, "name": name, "previousName": cal_name(&cal) },
        "renamed": true,
    }))
}

// ── Sharing ─────────────────────────────────────────────────────────────

fn max_shares(ctx: &Ctx<'_>) -> Option<u64> {
    ctx.api
        .sharing_get()
        .ok()
        .and_then(|s| s.get("maxShares").and_then(Value::as_u64))
}

fn max_shares_refusal(max: u64, cal: &str, owner: &str) -> CliResponse {
    err(
        "409 Conflict",
        "max_shares",
        format!(
            "calendar '{cal}' of {owner} already has {max} share(s), the server's limit — \
             unshare someone, share with a group instead, or raise it with \
             'k2 hostmail calendar sharing --max-shares N'"
        ),
    )
}

/// POST /cli/mail/calendar/share `{owner, calendar?, with, level}`.
pub fn share_with(ctx: &Ctx<'_>, b: &CalBody) -> CliResponse {
    if b.name.is_some() || b.color.is_some() || b.action.is_some() || b.max_shares.is_some() {
        return usage("share takes owner, calendar, with and level");
    }
    let Some(level) = b.level.as_deref().and_then(ShareLevel::parse) else {
        return usage("level must be freebusy, read, write or editor");
    };
    let owner = match ctx.owner(b.owner.as_deref().unwrap_or("")) {
        Ok(o) => o,
        Err(r) => return r,
    };
    let dir = match ctx.directory() {
        Ok(d) => d,
        Err(r) => return r,
    };
    let grantee = match resolve_grantee(ctx, &dir, b.with.as_deref().unwrap_or("")) {
        Ok(g) => g,
        Err(r) => return r,
    };
    if grantee.id == owner.account_id {
        return usage(format!("{} owns this calendar — share it with someone else", owner.address));
    }
    let cals = match ctx.api.calendars(&owner.account_id) {
        Ok(c) => c,
        Err(e) => return engine_err(e),
    };
    let cal = match pick_calendar(&cals, b.calendar.as_deref(), &owner.address) {
        Ok(c) => c.clone(),
        Err(r) => return r,
    };
    let id = cal_id(&cal).to_string();
    let share_map = cal.get("shareWith").and_then(Value::as_object).cloned().unwrap_or_default();
    let existing = share_map.get(&grantee.id).filter(|v| !v.is_null()).cloned();
    let new_rights = level.rights();
    // Self-widening (owner-only): the caller's own inbox, or a group
    // holding it, may only be narrowed.
    if ctx.is_it_agent() && grantee_is_callers(ctx, &dir, &grantee) {
        let have = true_rights(existing.as_ref());
        let want = true_rights(Some(&new_rights));
        if !want.is_subset(&have) {
            return agent_creds::refuse_self_widening(&format!(
                "sharing calendar '{}' of {} with {} ({}) — an inbox this agent's workspace \
                 can read{} —",
                cal_name(&cal),
                owner.address,
                grantee.label,
                level.as_str(),
                if grantee.is_group { " is in that group" } else { "" }
            ));
        }
    }
    let max = max_shares(ctx);
    if existing.is_none() {
        let count = share_map.values().filter(|v| !v.is_null()).count() as u64;
        if let Some(m) = max {
            if count >= m {
                return max_shares_refusal(m, cal_name(&cal), &owner.address);
            }
        }
    }
    let path = format!("shareWith/{}", grantee.id);
    let reply = match ctx
        .api
        .calendar_set(&owner.account_id, json!({ "update": { id.clone(): { path: new_rights.clone() } } }))
    {
        Ok(r) => r,
        Err(e) => return engine_err(e),
    };
    if let Some(m) = set_failure(&reply, "notUpdated", &id) {
        if m.contains("Maximum number of shares") {
            return max_shares_refusal(max.unwrap_or(0), cal_name(&cal), &owner.address);
        }
        if m.contains("is invalid") {
            return not_found(format!("the mail server doesn't know {} ({m})", grantee.label));
        }
        return engine_err(format!("Calendar/set share: {m}"));
    }
    ok_json(json!({
        "ok": true,
        "owner": owner.address,
        "calendar": { "id": id, "name": cal_name(&cal) },
        "with": grantee.label,
        "withType": if grantee.is_group { "group" } else { "address" },
        "level": level.as_str(),
        "rights": new_rights,
        "previousLevel": existing.as_ref().map(level_of),
        "maxShares": max,
    }))
}

/// POST /cli/mail/calendar/unshare `{owner, calendar?, with}`. Always
/// allowed (never widens anyone). Not shared → ok, `removed:false`.
pub fn unshare_with(ctx: &Ctx<'_>, b: &CalBody) -> CliResponse {
    if b.name.is_some() || b.color.is_some() || b.action.is_some() || b.level.is_some()
        || b.max_shares.is_some()
    {
        return usage("unshare takes owner, calendar and with");
    }
    let owner = match ctx.owner(b.owner.as_deref().unwrap_or("")) {
        Ok(o) => o,
        Err(r) => return r,
    };
    let dir = match ctx.directory() {
        Ok(d) => d,
        Err(r) => return r,
    };
    let grantee = match resolve_grantee(ctx, &dir, b.with.as_deref().unwrap_or("")) {
        Ok(g) => g,
        Err(r) => return r,
    };
    let cals = match ctx.api.calendars(&owner.account_id) {
        Ok(c) => c,
        Err(e) => return engine_err(e),
    };
    let cal = match pick_calendar(&cals, b.calendar.as_deref(), &owner.address) {
        Ok(c) => c.clone(),
        Err(r) => return r,
    };
    let id = cal_id(&cal).to_string();
    let shared = cal
        .get("shareWith")
        .and_then(|s| s.get(&grantee.id))
        .is_some_and(|v| !v.is_null());
    if shared {
        let path = format!("shareWith/{}", grantee.id);
        let reply = match ctx
            .api
            .calendar_set(&owner.account_id, json!({ "update": { id.clone(): { path: Value::Null } } }))
        {
            Ok(r) => r,
            Err(e) => return engine_err(e),
        };
        if let Some(m) = set_failure(&reply, "notUpdated", &id) {
            return engine_err(format!("Calendar/set unshare: {m}"));
        }
    }
    ok_json(json!({
        "ok": true,
        "owner": owner.address,
        "calendar": { "id": id, "name": cal_name(&cal) },
        "with": grantee.label,
        "removed": shared,
    }))
}

/// GET /cli/mail/calendar/shares `owner=`.
pub fn shares_with(ctx: &Ctx<'_>, owner_raw: &str) -> CliResponse {
    let owner = match ctx.owner(owner_raw) {
        Ok(o) => o,
        Err(r) => return r,
    };
    let dir = match ctx.directory() {
        Ok(d) => d,
        Err(r) => return r,
    };
    let cals = match ctx.api.calendars(&owner.account_id) {
        Ok(c) => c,
        Err(e) => return engine_err(e),
    };
    let out: Vec<Value> = cals
        .iter()
        .map(|c| {
            let shares: Vec<Value> = c
                .get("shareWith")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(gid, rights)| {
                            let (label, kind) = ctx.label_of(&dir, gid);
                            json!({
                                "with": label,
                                "withType": kind,
                                "level": level_of(rights),
                                "rights": true_rights(Some(rights)).into_iter().collect::<Vec<_>>(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "id": cal_id(c),
                "name": cal_name(c),
                "isDefault": c.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
                "shares": shares,
            })
        })
        .collect();
    ok_json(json!({
        "ok": true,
        "owner": owner.address,
        "calendars": out,
        "maxShares": max_shares(ctx),
    }))
}

/// Upper bound K2 accepts for `maxShares`.
const MAX_SHARES_CAP: u64 = 1000;

/// GET|POST /cli/mail/calendar/sharing (`{maxShares}` on POST).
pub fn sharing_with(ctx: &Ctx<'_>, set: Option<u64>) -> CliResponse {
    if let Some(n) = set {
        if n == 0 || n > MAX_SHARES_CAP {
            return usage(format!("--max-shares must be 1..{MAX_SHARES_CAP}"));
        }
        if let Err(e) = ctx.api.sharing_set(json!({ "maxShares": n })) {
            return engine_err(e);
        }
    }
    let reload = match set {
        Some(_) => match ctx.api.reload_settings() {
            Ok(()) => Value::Bool(true),
            Err(e) => json!(format!("settings reload failed ({e}) — takes effect on the next Stalwart restart")),
        },
        None => Value::Null,
    };
    match ctx.api.sharing_get() {
        Ok(s) => ok_json(json!({
            "ok": true,
            "maxShares": s.get("maxShares").and_then(Value::as_u64),
            "allowDirectoryQueries": s.get("allowDirectoryQueries").and_then(Value::as_bool),
            "reloaded": reload,
        })),
        Err(e) => engine_err(e),
    }
}

// ── HTTP entry points ───────────────────────────────────────────────────

fn engine() -> Result<StalwartClient, CliResponse> {
    crate::mail::domains::engine_from_db()
        .map(|(c, _)| c)
        .map_err(|h| err("503 Service Unavailable", "not_ready", h))
}

fn run(f: impl FnOnce(&Ctx<'_>) -> CliResponse) -> CliResponse {
    let api = match engine() {
        Ok(a) => a,
        Err(r) => return r,
    };
    match Ctx::live(&api) {
        Ok(ctx) => f(&ctx),
        Err(r) => r,
    }
}

pub fn handle_group_get(params: &HashMap<String, String>) -> CliResponse {
    let g = crate::cli::opt_param(params, "group");
    run(|ctx| group_get_with(ctx, g.as_deref()))
}

pub fn handle_group_members_get(params: &HashMap<String, String>) -> CliResponse {
    let Some(g) = crate::cli::opt_param(params, "group") else {
        return usage("missing 'group'");
    };
    run(|ctx| group_get_with(ctx, Some(&g)))
}

pub fn handle_group_create(body: &[u8]) -> CliResponse {
    match parse::<GroupBody>(body) {
        Ok(b) => run(|ctx| group_create_with(ctx, &b)),
        Err(r) => r,
    }
}

pub fn handle_group_members(body: &[u8]) -> CliResponse {
    match parse::<GroupBody>(body) {
        Ok(b) => run(|ctx| group_members_with(ctx, &b)),
        Err(r) => r,
    }
}

pub fn handle_group_delete(body: &[u8]) -> CliResponse {
    match parse::<GroupBody>(body) {
        Ok(b) => run(|ctx| group_delete_with(ctx, &b)),
        Err(r) => r,
    }
}

pub fn handle_manage(body: &[u8]) -> CliResponse {
    match parse::<CalBody>(body) {
        Ok(b) => run(|ctx| manage_with(ctx, &b)),
        Err(r) => r,
    }
}

pub fn handle_share(body: &[u8]) -> CliResponse {
    match parse::<CalBody>(body) {
        Ok(b) => run(|ctx| share_with(ctx, &b)),
        Err(r) => r,
    }
}

pub fn handle_unshare(body: &[u8]) -> CliResponse {
    match parse::<CalBody>(body) {
        Ok(b) => run(|ctx| unshare_with(ctx, &b)),
        Err(r) => r,
    }
}

pub fn handle_shares(params: &HashMap<String, String>) -> CliResponse {
    let owner = crate::cli::opt_param(params, "owner").unwrap_or_default();
    run(|ctx| shares_with(ctx, &owner))
}

pub fn handle_sharing_get(_params: &HashMap<String, String>) -> CliResponse {
    run(|ctx| sharing_with(ctx, None))
}

pub fn handle_sharing_post(body: &[u8]) -> CliResponse {
    match parse::<CalBody>(body) {
        Ok(b) => {
            if b.max_shares.is_none() {
                return usage("give maxShares");
            }
            run(|ctx| sharing_with(ctx, b.max_shares))
        }
        Err(r) => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // ── Fixture JMAP (no network, no DB) ────────────────────────────────

    struct Fake {
        accounts: Vec<Value>,
        /// account → its calendars (with shareWith).
        cals: HashMap<String, Vec<Value>>,
        lists: Vec<String>,
        catchall: Option<String>,
        sharing: Value,
        /// (method, target, args)
        calls: RefCell<Vec<(String, String, Value)>>,
    }

    impl Default for Fake {
        fn default() -> Self {
            Self {
                accounts: Vec::new(),
                cals: HashMap::new(),
                lists: Vec::new(),
                catchall: None,
                sharing: json!({ "maxShares": 10, "allowDirectoryQueries": false }),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl Fake {
        fn record(&self, m: &str, t: &str, a: Value) {
            self.calls.borrow_mut().push((m.to_string(), t.to_string(), a));
        }
        fn calls_of(&self, m: &str) -> Vec<(String, Value)> {
            self.calls
                .borrow()
                .iter()
                .filter(|(n, _, _)| n == m)
                .map(|(_, t, a)| (t.clone(), a.clone()))
                .collect()
        }
        fn writes(&self) -> Vec<String> {
            self.calls
                .borrow()
                .iter()
                .filter(|(n, _, _)| {
                    matches!(
                        n.as_str(),
                        "group_create"
                            | "account_update"
                            | "account_destroy"
                            | "calendar_set"
                            | "sharing_set"
                    )
                })
                .map(|(n, _, _)| n.clone())
                .collect()
        }
    }

    impl ShareApi for Fake {
        fn accounts(&self) -> Result<Vec<Value>, String> {
            Ok(self.accounts.clone())
        }
        fn group_create(
            &self,
            name: &str,
            domain_id: &str,
            d: &str,
            p: Option<&Value>,
        ) -> Result<String, String> {
            self.record(
                "group_create",
                name,
                json!({ "name": name, "domainId": domain_id, "description": d, "permissions": p }),
            );
            Ok("g-new".to_string())
        }
        fn account_update(&self, id: &str, patch: Value) -> Result<(), String> {
            self.record("account_update", id, patch);
            Ok(())
        }
        fn account_destroy(&self, id: &str) -> Result<(), String> {
            self.record("account_destroy", id, Value::Null);
            Ok(())
        }
        fn list_addresses(&self) -> Result<Vec<String>, String> {
            Ok(self.lists.clone())
        }
        fn catchall(&self, _d: &str) -> Result<Option<String>, String> {
            Ok(self.catchall.clone())
        }
        fn calendars(&self, account: &str) -> Result<Vec<Value>, String> {
            self.record("calendars", account, Value::Null);
            Ok(self.cals.get(account).cloned().unwrap_or_default())
        }
        fn calendar_set(&self, account: &str, args: Value) -> Result<Value, String> {
            self.record("calendar_set", account, args.clone());
            if args.get("create").is_some() {
                return Ok(json!({ "created": { "k2new": { "id": "cal-new" } } }));
            }
            let ids: Vec<String> = args
                .get("update")
                .and_then(Value::as_object)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            let mut updated = serde_json::Map::new();
            for i in ids {
                updated.insert(i, Value::Null);
            }
            Ok(json!({ "updated": updated }))
        }
        fn sharing_get(&self) -> Result<Value, String> {
            Ok(self.sharing.clone())
        }
        fn sharing_set(&self, patch: Value) -> Result<(), String> {
            self.record("sharing_set", "singleton", patch);
            Ok(())
        }
        fn reload_settings(&self) -> Result<(), String> {
            self.record("reload_settings", "", Value::Null);
            Ok(())
        }
    }

    // Directory: owner (acc-o), alice (acc-a), bob (acc-b), the agent's
    // own inbox me (acc-me), group team (g-team: alice + me, mail off),
    // group ops (g-ops: bob). owner's calendars: Main (default, not
    // shared) and Team (shared read to ops).
    const D: &str = "d-1";

    fn user(id: &str, local: &str, groups: &[&str]) -> Value {
        let mut m = serde_json::Map::new();
        for g in groups {
            m.insert(g.to_string(), json!(true));
        }
        json!({ "id": id, "@type": "User", "name": local, "domainId": D,
                "emailAddress": format!("{local}@example.com"), "memberGroupIds": m,
                "aliases": {} })
    }

    fn group(id: &str, local: &str, mail_off: bool) -> Value {
        let perms = if mail_off {
            json!({ "@type": "Merge", "enabledPermissions": {},
                    "disabledPermissions": { "emailReceive": true } })
        } else {
            json!({ "@type": "Inherit" })
        };
        json!({ "id": id, "@type": "Group", "name": local, "domainId": D,
                "emailAddress": format!("{local}@example.com"), "permissions": perms })
    }

    fn fake() -> Fake {
        let mut f = Fake::default();
        f.accounts = vec![
            user("acc-o", "owner", &[]),
            user("acc-a", "alice", &["g-team"]),
            user("acc-b", "bob", &["g-ops"]),
            user("acc-me", "me", &["g-team"]),
            group("g-team", "team", true),
            group("g-ops", "ops", false),
            json!({ "id": "acc-admin", "@type": "User", "name": "admin", "domainId": D,
                    "emailAddress": "admin@example.com", "memberGroupIds": {},
                    "aliases": { "0": { "name": "postmaster", "domainId": D, "enabled": true } } }),
        ];
        f.cals.insert(
            "acc-o".into(),
            vec![
                json!({ "id": "c-main", "name": "Main", "isDefault": true, "shareWith": null }),
                json!({ "id": "c-team", "name": "Team", "isDefault": false,
                        "shareWith": { "g-ops": { "mayReadFreeBusy": true, "mayReadItems": true } } }),
            ],
        );
        f
    }

    fn hosted() -> Vec<HostedAddr> {
        [("owner", "acc-o"), ("alice", "acc-a"), ("bob", "acc-b"), ("me", "acc-me")]
            .iter()
            .map(|(l, a)| HostedAddr {
                address: format!("{l}@example.com"),
                account_id: a.to_string(),
            })
            .collect()
    }

    fn ctx(api: &Fake, caller: MailCaller) -> Ctx<'_> {
        let own = match &caller {
            MailCaller::ItAgent { .. } => vec!["acc-me".to_string()],
            _ => Vec::new(),
        };
        Ctx {
            api,
            caller,
            own_accounts: own,
            hosted: hosted(),
            reserved: vec!["retired@example.com".to_string()],
            domains: vec![(D.to_string(), "example.com".to_string())],
            installed_version: Some("0.16.20".to_string()),
        }
    }

    fn it_agent() -> MailCaller {
        MailCaller::ItAgent { workspace_uuid: "ws-it".to_string() }
    }

    fn body(r: &CliResponse) -> Value {
        serde_json::from_str(&r.body).expect("json body")
    }
    fn code(r: &CliResponse) -> String {
        body(r)["error"]["code"].as_str().unwrap_or("").to_string()
    }
    fn gb(v: Value) -> GroupBody {
        serde_json::from_value(v).expect("group body")
    }
    fn cb(v: Value) -> CalBody {
        serde_json::from_value(v).expect("cal body")
    }

    // ── Groups ──────────────────────────────────────────────────────────

    #[test]
    fn group_create_sends_group_json_with_inbound_mail_off_by_default() {
        let f = fake();
        let c = ctx(&f, MailCaller::Owner);
        let r = group_create_with(&c, &gb(json!({ "name": "sales" })));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(body(&r)["address"], "sales@example.com");
        assert_eq!(body(&r)["receivesMail"], false);
        let calls = f.calls_of("group_create");
        assert_eq!(calls.len(), 1);
        let a = &calls[0].1;
        assert_eq!(a["name"], "sales");
        assert_eq!(a["domainId"], D);
        assert_eq!(
            a["permissions"],
            json!({ "@type": "Merge", "enabledPermissions": {},
                    "disabledPermissions": { "emailReceive": true } })
        );

        let f = fake();
        let c = ctx(&f, MailCaller::Owner);
        let r = group_create_with(&c, &gb(json!({ "name": "sales@example.com", "mail": true })));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            f.calls_of("group_create")[0].1["permissions"],
            Value::Null,
            "--mail keeps the account's default permissions"
        );
        assert_eq!(body(&r)["receivesMail"], true);
    }

    #[test]
    fn group_create_refuses_every_address_collision_before_any_write() {
        type Setup = Box<dyn Fn(&mut Fake)>;
        let cases: Vec<(&str, Setup)> = vec![
            ("retired", Box::new(|_| {})),    // a mail_addresses row (any status)
            ("alice", Box::new(|_| {})),      // an account
            ("team", Box::new(|_| {})),       // a group
            ("postmaster", Box::new(|_| {})), // an alias
            ("news", Box::new(|f| f.lists = vec!["news@example.com".into()])),
            ("catch", Box::new(|f| f.catchall = Some("catch@example.com".into()))),
        ];
        for (name, setup) in cases {
            let mut f = fake();
            setup(&mut f);
            let c = ctx(&f, MailCaller::Owner);
            let r = group_create_with(&c, &gb(json!({ "name": name })));
            assert_eq!(r.status, "409 Conflict", "{name}: {}", r.body);
            assert_eq!(code(&r), "exists", "{name}");
            assert!(f.writes().is_empty(), "{name}: no write on a collision");
        }
    }

    #[test]
    fn group_create_domain_rules() {
        let f = fake();
        let mut c = ctx(&f, MailCaller::Owner);
        c.domains.push(("d-2".into(), "other.example".into()));
        let r = group_create_with(&c, &gb(json!({ "name": "sales" })));
        assert_eq!(r.status, "400 Bad Request", "several domains need --domain: {}", r.body);
        let r = group_create_with(&c, &gb(json!({ "name": "sales", "domain": "nope.example" })));
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        let r = group_create_with(
            &c,
            &gb(json!({ "name": "sales@example.com", "domain": "other.example" })),
        );
        assert_eq!(r.status, "400 Bad Request", "address vs --domain: {}", r.body);
        let r = group_create_with(&c, &gb(json!({ "name": "sales", "domain": "other.example" })));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(f.calls_of("group_create")[0].1["domainId"], "d-2");
    }

    #[test]
    fn member_add_and_remove_write_the_users_member_group_ids() {
        let f = fake();
        let c = ctx(&f, MailCaller::Owner);
        // bob is in ops; adding him to team keeps ops.
        let r = group_members_with(&c, &gb(json!({ "group": "team", "add": ["bob@example.com"] })));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let up = f.calls_of("account_update");
        assert_eq!(up.len(), 1);
        assert_eq!(up[0].0, "acc-b");
        assert_eq!(up[0].1, json!({ "memberGroupIds": { "g-ops": true, "g-team": true } }));
        let members = body(&r)["members"].as_array().cloned().unwrap_or_default();
        assert!(members.contains(&json!("bob@example.com")), "{members:?}");

        let f = fake();
        let c = ctx(&f, MailCaller::Owner);
        let r = group_members_with(
            &c,
            &gb(json!({ "group": "team@example.com", "remove": ["alice@example.com"] })),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            f.calls_of("account_update"),
            vec![("acc-a".to_string(), json!({ "memberGroupIds": {} }))]
        );
        let members = body(&r)["members"].as_array().cloned().unwrap_or_default();
        assert!(!members.contains(&json!("alice@example.com")), "{members:?}");

        // Unknown member → usage, unknown group → 404; no write either way.
        let f = fake();
        let c = ctx(&f, MailCaller::Owner);
        let r = group_members_with(
            &c,
            &gb(json!({ "group": "team", "add": ["stranger@example.org"] })),
        );
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        let r = group_members_with(&c, &gb(json!({ "group": "nope", "add": ["bob@example.com"] })));
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        assert!(f.writes().is_empty());
    }

    #[test]
    fn group_list_shows_members_and_mail_state() {
        let f = fake();
        let c = ctx(&f, MailCaller::Owner);
        let v = body(&group_get_with(&c, None));
        let groups = v["groups"].as_array().cloned().unwrap_or_default();
        assert_eq!(groups.len(), 2, "{v}");
        let team = groups.iter().find(|g| g["name"] == "team").expect("team");
        assert_eq!(team["receivesMail"], false);
        let mut m: Vec<String> = team["members"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        m.sort();
        assert_eq!(m, vec!["alice@example.com", "me@example.com"]);
        let ops = groups.iter().find(|g| g["name"] == "ops").expect("ops");
        assert_eq!(ops["receivesMail"], true);
        let one = body(&group_get_with(&c, Some("ops@example.com")));
        assert_eq!(one["group"]["id"], "g-ops");
    }

    #[test]
    fn group_delete_drops_its_shares_then_members_then_the_group() {
        let f = fake();
        let c = ctx(&f, MailCaller::Owner);
        let r = group_delete_with(&c, &gb(json!({ "group": "ops" })));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(body(&r)["sharesRemoved"], 1);
        assert_eq!(body(&r)["membersRemoved"], 1);
        assert_eq!(
            f.calls_of("calendar_set"),
            vec![(
                "acc-o".to_string(),
                json!({ "update": { "c-team": { "shareWith/g-ops": null } } })
            )]
        );
        assert_eq!(
            f.calls_of("account_update"),
            vec![("acc-b".to_string(), json!({ "memberGroupIds": {} }))]
        );
        assert_eq!(f.writes(), vec!["calendar_set", "account_update", "account_destroy"]);
        assert_eq!(f.calls_of("account_destroy")[0].0, "g-ops");
    }

    // ── Share rights ────────────────────────────────────────────────────

    #[test]
    fn share_levels_map_to_stalwart_rights_and_never_may_share() {
        let names =
            |l: ShareLevel| true_rights(Some(&l.rights())).into_iter().collect::<Vec<_>>();
        assert_eq!(names(ShareLevel::FreeBusy), vec!["mayReadFreeBusy"]);
        assert_eq!(names(ShareLevel::Read), vec!["mayReadFreeBusy", "mayReadItems"]);
        assert_eq!(
            names(ShareLevel::Write),
            vec!["mayRSVP", "mayReadFreeBusy", "mayReadItems", "mayWriteAll"]
        );
        assert_eq!(
            names(ShareLevel::Editor),
            vec!["mayDelete", "mayRSVP", "mayReadFreeBusy", "mayReadItems", "mayWriteAll"]
        );
        for l in [ShareLevel::FreeBusy, ShareLevel::Read, ShareLevel::Write, ShareLevel::Editor] {
            let r = l.rights();
            assert!(r.get("mayShare").is_none() && r.get("mayAdmin").is_none(), "{l:?}");
            assert_eq!(level_of(&r), l.as_str());
            assert_eq!(ShareLevel::parse(l.as_str()), Some(l));
        }
        assert_eq!(level_of(&json!({ "mayWriteOwn": true })), "custom");
        // Stalwart reads back every right name with a boolean.
        let stored = json!({ "mayReadFreeBusy": true, "mayReadItems": true, "mayWriteAll": false,
                             "mayWriteOwn": false, "mayUpdatePrivate": false, "mayRSVP": false,
                             "mayShare": false, "mayDelete": false });
        assert_eq!(level_of(&stored), "read");
        assert_eq!(
            ShareRights::from_json(&stored),
            ShareRights { free_busy: true, read: true, write: false, delete: false }
        );
        assert_eq!(
            ShareRights::from_json(&ShareLevel::FreeBusy.rights()),
            ShareRights { free_busy: true, read: false, write: false, delete: false }
        );
        assert_eq!(
            ShareRights::from_json(&ShareLevel::Editor.rights()),
            ShareRights { free_busy: true, read: true, write: true, delete: true }
        );
    }

    #[test]
    fn share_patches_share_with_on_the_owner_account_per_flag() {
        for (flag, want) in [
            ("freebusy", ShareLevel::FreeBusy.rights()),
            ("read", ShareLevel::Read.rights()),
            ("write", ShareLevel::Write.rights()),
            ("editor", ShareLevel::Editor.rights()),
        ] {
            let f = fake();
            let c = ctx(&f, MailCaller::Owner);
            let r = share_with(
                &c,
                &cb(json!({ "owner": "owner@example.com", "calendar": "Team",
                            "with": "team", "level": flag })),
            );
            assert_eq!(r.status, "200 OK", "{flag}: {}", r.body);
            let sets = f.calls_of("calendar_set");
            assert_eq!(sets.len(), 1, "{flag}");
            assert_eq!(sets[0].0, "acc-o", "the OWNER's account");
            assert_eq!(
                sets[0].1,
                json!({ "update": { "c-team": { "shareWith/g-team": want } } }),
                "{flag}"
            );
            assert_eq!(body(&r)["withType"], "group");
            assert_eq!(body(&r)["maxShares"], 10, "maxShares reported");
        }
        // Default calendar when --calendar is absent; a user grantee.
        let f = fake();
        let c = ctx(&f, MailCaller::Owner);
        let r = share_with(
            &c,
            &cb(json!({ "owner": "owner@example.com", "with": "alice@example.com", "level": "read" })),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            f.calls_of("calendar_set")[0].1,
            json!({ "update": { "c-main": { "shareWith/acc-a": ShareLevel::Read.rights() } } })
        );
    }

    #[test]
    fn share_refuses_unknown_grantees_owners_and_levels_without_writing() {
        let f = fake();
        let c = ctx(&f, MailCaller::Owner);
        let bad = [
            (json!({ "owner": "owner@example.com", "with": "stranger@example.org", "level": "read" }), "404"),
            // An account K2 didn't mint is not a hosted address.
            (json!({ "owner": "owner@example.com", "with": "admin@example.com", "level": "read" }), "404"),
            (json!({ "owner": "nobody@example.com", "with": "alice@example.com", "level": "read" }), "404"),
            (json!({ "owner": "owner@example.com", "with": "owner@example.com", "level": "read" }), "400"),
            (json!({ "owner": "owner@example.com", "with": "alice@example.com", "level": "admin" }), "400"),
            (json!({ "owner": "owner@example.com", "with": "alice@example.com", "level": "read",
                     "calendar": "Nope" }), "400"),
        ];
        for (b, want) in bad {
            let r = share_with(&c, &cb(b.clone()));
            assert!(r.status.starts_with(want), "{b}: {} {}", r.status, r.body);
        }
        assert!(f.writes().is_empty(), "no write on a refused share");
    }

    #[test]
    fn share_respects_max_shares_but_resharing_an_existing_grantee_is_fine() {
        let mut f = fake();
        f.sharing = json!({ "maxShares": 1, "allowDirectoryQueries": false });
        let c = ctx(&f, MailCaller::Owner);
        // c-team already has one grantee (ops) = the limit.
        let r = share_with(
            &c,
            &cb(json!({ "owner": "owner@example.com", "calendar": "Team",
                        "with": "alice@example.com", "level": "read" })),
        );
        assert_eq!(r.status, "409 Conflict", "{}", r.body);
        assert_eq!(code(&r), "max_shares");
        assert!(r.body.contains("--max-shares"), "{}", r.body);
        assert!(f.writes().is_empty());
        let r = share_with(
            &c,
            &cb(json!({ "owner": "owner@example.com", "calendar": "Team", "with": "ops",
                        "level": "write" })),
        );
        assert_eq!(r.status, "200 OK", "re-share at the limit: {}", r.body);
        assert_eq!(body(&r)["previousLevel"], "read");
    }

    #[test]
    fn unshare_and_shares() {
        let f = fake();
        let c = ctx(&f, MailCaller::Owner);
        let r = unshare_with(
            &c,
            &cb(json!({ "owner": "owner@example.com", "calendar": "c-team", "with": "ops@example.com" })),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(body(&r)["removed"], true);
        assert_eq!(
            f.calls_of("calendar_set")[0].1,
            json!({ "update": { "c-team": { "shareWith/g-ops": null } } })
        );
        let r = unshare_with(
            &c,
            &cb(json!({ "owner": "owner@example.com", "with": "alice@example.com" })),
        );
        assert_eq!(body(&r)["removed"], false, "not shared → nothing to do");
        assert_eq!(f.calls_of("calendar_set").len(), 1);

        let v = body(&shares_with(&c, "owner@example.com"));
        assert_eq!(v["maxShares"], 10);
        let team = v["calendars"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["id"] == "c-team").cloned())
            .expect("c-team");
        assert_eq!(team["shares"][0]["with"], "ops@example.com");
        assert_eq!(team["shares"][0]["withType"], "group");
        assert_eq!(team["shares"][0]["level"], "read");
    }

    #[test]
    fn sharing_sets_max_shares_and_reloads() {
        let f = fake();
        let c = ctx(&f, MailCaller::Owner);
        let r = sharing_with(&c, Some(25));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(f.calls_of("sharing_set")[0].1, json!({ "maxShares": 25 }));
        assert_eq!(f.calls_of("reload_settings").len(), 1);
        assert_eq!(sharing_with(&c, Some(0)).status, "400 Bad Request");
        assert_eq!(sharing_with(&c, Some(5000)).status, "400 Bad Request");
        let r = sharing_with(&c, None);
        assert_eq!(body(&r)["maxShares"], 10);
        assert_eq!(f.calls_of("sharing_set").len(), 1, "a read never writes");
    }

    // ── create / rename and the version gate ────────────────────────────

    #[test]
    fn per_user_props_need_stalwart_0_16_19() {
        for ok in ["0.16.19", "0.16.20", "0.16.25-k2.1", "0.17.0", "1.0.0"] {
            assert!(per_user_props_ok(Some(ok)).is_ok(), "{ok}");
        }
        for bad in [Some("0.16.10"), Some("0.16.18"), Some("0.15.9"), Some("junk"), Some(""), None] {
            let r = per_user_props_ok(bad)
                .err()
                .unwrap_or_else(|| panic!("{bad:?} must refuse"));
            assert_eq!(r.status, "409 Conflict");
            assert!(
                r.body.contains("upgrade_required") && r.body.contains("k2 hostmail upgrade"),
                "{}",
                r.body
            );
        }
    }

    #[test]
    fn create_and_rename_refuse_on_0_16_10_before_any_call() {
        for action in ["create", "rename"] {
            let f = fake();
            let mut c = ctx(&f, MailCaller::Owner);
            c.installed_version = Some("0.16.10".into());
            let mut b = json!({ "action": action, "owner": "owner@example.com", "name": "Team 2" });
            if action == "rename" {
                b["calendar"] = json!("Team");
            }
            let r = manage_with(&c, &cb(b));
            assert_eq!(r.status, "409 Conflict", "{action}: {}", r.body);
            assert_eq!(code(&r), "upgrade_required");
            assert!(f.calls.borrow().is_empty(), "{action}: no engine call on 0.16.10");
        }
    }

    #[test]
    fn create_and_rename_on_0_16_20() {
        let f = fake();
        let c = ctx(&f, MailCaller::Owner);
        let r = manage_with(
            &c,
            &cb(json!({ "action": "create", "owner": "owner@example.com", "name": "Team 2",
                        "color": "#3366ff" })),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            f.calls_of("calendar_set")[0],
            (
                "acc-o".to_string(),
                json!({ "create": { "k2new": { "name": "Team 2", "color": "#3366ff" } } })
            )
        );
        assert_eq!(body(&r)["calendar"]["id"], "cal-new");
        let r = manage_with(
            &c,
            &cb(json!({ "action": "create", "owner": "owner@example.com", "name": "team" })),
        );
        assert_eq!(r.status, "409 Conflict", "duplicate name: {}", r.body);
        let r = manage_with(
            &c,
            &cb(json!({ "action": "rename", "owner": "owner@example.com", "calendar": "Team",
                        "name": "Crew" })),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            f.calls_of("calendar_set")[1].1,
            json!({ "update": { "c-team": { "name": "Crew" } } })
        );
        assert_eq!(body(&r)["calendar"]["previousName"], "Team");
        let r = manage_with(
            &c,
            &cb(json!({ "action": "rename", "owner": "owner@example.com", "name": "Crew" })),
        );
        assert_eq!(r.status, "400 Bad Request", "rename needs the calendar: {}", r.body);
    }

    // ── The self-widening rule (mail-manage agents) ─────────────────────

    #[test]
    fn it_agent_may_share_with_others_but_not_with_its_own_inbox_or_group() {
        let f = fake();
        let c = ctx(&f, it_agent());
        let r = share_with(
            &c,
            &cb(json!({ "owner": "owner@example.com", "with": "bob@example.com", "level": "editor" })),
        );
        assert_eq!(r.status, "200 OK", "others: {}", r.body);

        // Its own inbox (direct) and a group holding it: owner_only.
        for with in ["me@example.com", "team"] {
            let f = fake();
            let c = ctx(&f, it_agent());
            let r = share_with(
                &c,
                &cb(json!({ "owner": "owner@example.com", "with": with, "level": "read" })),
            );
            assert_eq!(r.status, "403 Forbidden", "{with}: {}", r.body);
            assert_eq!(code(&r), "owner_only", "{with}");
            assert!(r.body.contains("can't widen its own access"), "{}", r.body);
            assert!(f.writes().is_empty(), "{with}: no write");
            // The owner may.
            let f = fake();
            let c = ctx(&f, MailCaller::Owner);
            let r = share_with(
                &c,
                &cb(json!({ "owner": "owner@example.com", "with": with, "level": "read" })),
            );
            assert_eq!(r.status, "200 OK", "owner {with}: {}", r.body);
        }
    }

    #[test]
    fn it_agent_may_narrow_or_remove_a_share_on_its_own_inbox() {
        let mut f = fake();
        if let Some(cals) = f.cals.get_mut("acc-o") {
            cals[0]["shareWith"] = json!({ "acc-me": ShareLevel::Editor.rights() });
        }
        let c = ctx(&f, it_agent());
        let r = share_with(
            &c,
            &cb(json!({ "owner": "owner@example.com", "with": "me@example.com", "level": "read" })),
        );
        assert_eq!(r.status, "200 OK", "narrowing: {}", r.body);
        let r = share_with(
            &c,
            &cb(json!({ "owner": "owner@example.com", "with": "me@example.com", "level": "editor" })),
        );
        assert_eq!(r.status, "200 OK", "the same rights are not wider: {}", r.body);
        let r = unshare_with(
            &c,
            &cb(json!({ "owner": "owner@example.com", "with": "me@example.com" })),
        );
        assert_eq!(r.status, "200 OK", "unshare: {}", r.body);
        assert_eq!(body(&r)["removed"], true);

        // read → write widens: refused.
        let mut f = fake();
        if let Some(cals) = f.cals.get_mut("acc-o") {
            cals[0]["shareWith"] = json!({ "acc-me": ShareLevel::Read.rights() });
        }
        let c = ctx(&f, it_agent());
        let r = share_with(
            &c,
            &cb(json!({ "owner": "owner@example.com", "with": "me@example.com", "level": "write" })),
        );
        assert_eq!(code(&r), "owner_only", "{}", r.body);
    }

    #[test]
    fn it_agent_may_not_join_its_inbox_to_a_group_with_shares() {
        // ops is the grantee of a share on c-team → adding me widens.
        let f = fake();
        let c = ctx(&f, it_agent());
        let r = group_members_with(&c, &gb(json!({ "group": "ops", "add": ["me@example.com"] })));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert_eq!(code(&r), "owner_only");
        assert!(f.calls_of("account_update").is_empty());
        // The owner may.
        let f = fake();
        let r = group_members_with(
            &ctx(&f, MailCaller::Owner),
            &gb(json!({ "group": "ops", "add": ["me@example.com"] })),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        // A group with no shares: fine.
        let mut f = fake();
        if let Some(cals) = f.cals.get_mut("acc-o") {
            cals[1]["shareWith"] = Value::Null;
        }
        let c = ctx(&f, it_agent());
        let r = group_members_with(&c, &gb(json!({ "group": "ops", "add": ["me@example.com"] })));
        assert_eq!(r.status, "200 OK", "no shares: {}", r.body);
        // Someone else into a shared group: fine.
        let f = fake();
        let c = ctx(&f, it_agent());
        let r = group_members_with(
            &c,
            &gb(json!({ "group": "ops", "add": ["alice@example.com"] })),
        );
        assert_eq!(r.status, "200 OK", "someone else: {}", r.body);
        // Taking its own inbox out narrows: fine.
        let f = fake();
        let c = ctx(&f, it_agent());
        let r = group_members_with(
            &c,
            &gb(json!({ "group": "team", "remove": ["me@example.com"] })),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        // Group create/delete, calendar create and maxShares never widen.
        let f = fake();
        let c = ctx(&f, it_agent());
        assert_eq!(group_create_with(&c, &gb(json!({ "name": "crew" }))).status, "200 OK");
        assert_eq!(group_delete_with(&c, &gb(json!({ "group": "team" }))).status, "200 OK");
        assert_eq!(
            manage_with(
                &c,
                &cb(json!({ "action": "create", "owner": "me@example.com", "name": "Mine" }))
            )
            .status,
            "200 OK"
        );
        assert_eq!(sharing_with(&c, Some(50)).status, "200 OK");
    }

    #[test]
    fn bodies_reject_unknown_fields() {
        assert!(parse::<CalBody>(br#"{"owner":"a@example.com","accountId":"x"}"#).is_err());
        assert!(parse::<GroupBody>(br#"{"name":"t","accountId":"x"}"#).is_err());
    }
}
