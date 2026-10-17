//! `pending`, `approve`, `batch-approve`, `secret`, `pin` and `unpin`
//! handlers (contract §4.2, §4.3). Owner: B2.
//!
//! `crate::sources_routes::handle` has already classified the caller and
//! refused app passes and invalid tokens; both transports refuse a GET to a
//! POST-only path with 405. Every route here is for a person, or for an
//! agent (or K2 shell tab) whose workspace has "Manage Gardens and data
//! sources" (`gardens_manage`, 0181, [`crate::gardens_manage`]); any other
//! passport gets 403 `person_only` (CLI exit 3).
//!
//! An agent with the switch acts with no confirm: it approves (its own
//! proposals and others' on this server), binds a secret it may already
//! use (its own workspace's secret, or a server secret only when it also
//! has vault_manage; `secret_not_usable` otherwise), pins and unpins. It
//! never sets viewers / agentAccess / keep (`field_person_only`), and the
//! bind is still narrowed to the source's one origin inside the secret's
//! sites. Every such act writes a source_audit row (with the session) and
//! an Access Audit line.
//!
//! Approvals bind to what the person reviewed: the call carries the
//! `specHash` they saw, and a changed spec is 409 `spec_changed`. A
//! secretless source on a public host is one click (Q2). Binding a secret
//! needs a vault confirm: `source-secret` with `secretId` = the source id,
//! or one `source-batch` for a batch with `secretId` = the batch hash
//! (contract §4.3) [GV43].

use std::collections::HashMap;

use k2_core::sources::spec::{AgentAccess, Keep, SourceStatus, Viewers};
use k2_core::sources::trigger_store as ts;
use k2_core::sources::triggers::TriggerStatus;
use k2_core::sources::{store, SourceRow};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::compute_routes::Who;

use super::SourceErr;
use util::*;

/// Helpers every B2 handler shares (caller stamps, body parsing, source
/// lookup, the `viewers` floor).
pub(crate) mod util {
    use super::*;
    use k2_core::connect_users::Role;

    pub(crate) fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    /// `(actor, actor_kind)` for `source_audit` (`agent:<projectId>`, …).
    pub(crate) fn actor(who: &Who) -> (String, &'static str) {
        match who {
            Who::Owner => ("owner".into(), "owner"),
            Who::Login { username, .. } => (format!("login:{username}"), "login"),
            Who::Agent { project_id, .. } => (format!("agent:{project_id}"), "agent"),
            Who::Shell { project_id, .. } => (format!("shell:{project_id}"), "shell"),
            Who::Wrong | Who::Invalid => ("-".into(), "daemon"),
        }
    }

    /// The `made_by` stamp (`shell` follows the agent rules, so a shell tab
    /// makes things as its workspace's agent).
    pub(crate) fn maker(who: &Who) -> (String, Option<String>) {
        match who {
            Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => {
                (format!("agent:{project_id}"), Some(project_id.clone()))
            }
            other => (actor(other).0, None),
        }
    }

    pub(crate) fn is_person(who: &Who) -> bool {
        matches!(who, Who::Owner | Who::Login { .. })
    }

    /// The caller's own project (agent and shell passports).
    pub(crate) fn own_project(who: &Who) -> Option<&str> {
        match who {
            Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => Some(project_id),
            _ => None,
        }
    }

    pub(crate) fn person_only() -> SourceErr {
        SourceErr::new(
            "person_only",
            403,
            "This needs a person (run it outside K2, or use Settings → Gardens → External Data), or turn on \"Manage Gardens and data sources\" for this agent (Settings → Workspaces → Agent).",
        )
    }

    pub(crate) fn need_person(who: &Who) -> Result<(), SourceErr> {
        if is_person(who) {
            Ok(())
        } else {
            Err(person_only())
        }
    }

    /// An agent or K2 shell passport whose workspace has "Manage Gardens
    /// and data sources" (0181).
    pub(crate) fn is_manager(who: &Who) -> bool {
        crate::gardens_manage::who_may(who)
    }

    /// A person, or a passport with "Manage Gardens and data sources".
    pub(crate) fn need_person_or_manager(who: &Who) -> Result<(), SourceErr> {
        if is_person(who) || is_manager(who) {
            Ok(())
        } else {
            Err(person_only())
        }
    }

    pub(crate) fn bad_request(hint: impl Into<String>) -> SourceErr {
        SourceErr::new("bad_request", 400, hint)
    }

    pub(crate) fn db_err(e: impl std::fmt::Display) -> SourceErr {
        SourceErr::new("db_error", 500, format!("the sources store failed: {e}"))
    }

    /// A JSON body (unknown fields refused by the type).
    pub(crate) fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, SourceErr> {
        let b = if body.iter().all(u8::is_ascii_whitespace) { b"{}".as_slice() } else { body };
        serde_json::from_slice(b).map_err(|e| bad_request(format!("bad JSON body: {e}")))
    }

    pub(crate) fn param<'a>(params: &'a HashMap<String, String>, key: &str) -> Result<&'a str, SourceErr> {
        params.get(key).map(String::as_str).filter(|s| !s.is_empty()).ok_or_else(|| bad_request(format!("{key} is required")))
    }

    /// The live source called `name`; a tombstoned name is 410 `removed`.
    pub(crate) fn source_by_name(conn: &Connection, name: &str) -> Result<SourceRow, SourceErr> {
        if let Some(r) = store::get_by_name(conn, name).map_err(db_err)? {
            return Ok(r);
        }
        let removed: Option<i64> = conn
            .query_row(
                "SELECT MAX(removed_at) FROM sources WHERE name = ?1 AND status = 'removed'",
                params![name],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        match removed {
            Some(at) => Err(SourceErr::new("removed", 410, format!("{name} was removed")).with("removedAt", at)),
            None => Err(SourceErr::new("source_not_found", 404, format!("no source called {name}"))),
        }
    }

    /// The `viewers` floor for reads: a Member login can't read an `admin`
    /// source. The owner token and passports skip it (passports are masked
    /// per `agentAccess` instead).
    pub(crate) fn check_viewers(who: &Who, row: &SourceRow) -> Result<(), SourceErr> {
        match (who, row.viewers) {
            (Who::Login { role: Role::Member, .. }, Viewers::Admin) => Err(SourceErr::new(
                "cap_not_granted",
                403,
                format!("{} is for admins on this server", row.name),
            )
            .with("role", "admin")),
            _ => Ok(()),
        }
    }

    pub(crate) fn can_read(who: &Who, row: &SourceRow) -> bool {
        check_viewers(who, row).is_ok()
    }

    /// Bump the change counter and announce it.
    pub(crate) fn changed(conn: &Connection) {
        if let Ok(rev) = store::bump_rev(conn) {
            crate::sources::notify_changed(rev);
        }
    }

    pub(crate) fn audit(
        conn: &Connection,
        who: &Who,
        action: &str,
        source_id: Option<&str>,
        trigger_id: Option<&str>,
        detail: Value,
    ) {
        let (a, k) = actor(who);
        let detail = with_session(who, detail);
        if let Err(e) = ts::audit(conn, now(), &a, k, action, source_id, trigger_id, &detail) {
            k2_core::log_debug!("[sources] audit {action}: {e}");
        }
    }

    /// A passport's audit detail names its session (0181).
    pub(crate) fn with_session(who: &Who, mut detail: Value) -> Value {
        if let (Some(s), Value::Object(m)) = (crate::gardens_manage::session_of(who), &mut detail) {
            m.insert("session".into(), Value::String(s.to_string()));
        }
        detail
    }

    /// How a proposed source can be approved.
    pub(crate) fn approval_kind(row: &SourceRow) -> &'static str {
        if row.lan {
            "owner_only"
        } else if row.secret_name.is_some() || row.url_secret_name.is_some() {
            "needs_confirm"
        } else {
            "one_click"
        }
    }
}

pub fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> Result<Value, SourceErr> {
    need_person_or_manager(who)?;
    match path {
        "/cli/sources/pending" => pending(who, params),
        "/cli/sources/approve" => approve(who, body),
        "/cli/sources/batch-approve" => batch_approve(who, body),
        "/cli/sources/secret" => secret(who, body),
        "/cli/sources/pin" => pin(who, body, true),
        "/cli/sources/unpin" => pin(who, body, false),
        _ => Err(SourceErr::new("not_found", 404, "no such sources route")),
    }
}

// ── pending ──────────────────────────────────────────────────────────

fn pending(who: &Who, params: &HashMap<String, String>) -> Result<Value, SourceErr> {
    let names: Option<Vec<&str>> =
        params.get("names").filter(|s| !s.is_empty()).map(|s| s.split(',').map(str::trim).filter(|n| !n.is_empty()).collect());
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows = store::list_live(&conn).map_err(db_err)?;
    let wanted = |name: &str| names.as_ref().is_none_or(|n| n.contains(&name));
    let mut sources = Vec::new();
    for r in rows.iter().filter(|r| r.status == SourceStatus::Proposed && wanted(&r.name)) {
        sources.push(json!({
            "id": r.id,
            "name": r.name,
            "label": r.label,
            "madeBy": r.made_by,
            "maker": maker_view(&r.made_by),
            "origins": r.origins,
            "everySecs": r.every_secs,
            "targets": target_count(r),
            "secret": r.secret_name,
            "urlSecret": r.url_secret_name,
            "lan": r.lan,
            "specHash": r.spec_hash,
            "viewers": r.viewers,
            "agentAccess": r.agent_access,
            "keep": r.keep,
            "approval": approval_kind(r),
        }));
    }
    let by_id: HashMap<&str, &SourceRow> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut triggers = Vec::new();
    for t in ts::list_by_status(&conn, TriggerStatus::Proposed).map_err(db_err)? {
        let Some(src) = by_id.get(t.source_id.as_str()) else { continue };
        if !wanted(&src.name) {
            continue;
        }
        let ws = crate::compute_routes::ws_by_id(&t.target_project_id);
        triggers.push(json!({
            "id": t.id,
            "source": src.name,
            "on": t.on,
            "agent": {"projectId": t.target_project_id, "handle": ws.as_ref().map_or_else(|| t.target_project_id.clone(), |w| w.handle.clone()), "missing": ws.is_none()},
            "note": t.note,
            "madeBy": t.made_by,
        }));
    }
    let _ = who;
    Ok(json!({"sources": sources, "triggers": triggers}))
}

/// `{"kind": "agent", "handle": …}` for an agent maker, so Settings can say
/// "proposed by k2-dev-web".
fn maker_view(made_by: &str) -> Value {
    match k2_core::sources::Maker::parse(made_by) {
        Some(k2_core::sources::Maker::Agent { project_id }) => {
            let ws = crate::compute_routes::ws_by_id(&project_id);
            json!({"kind": "agent", "projectId": project_id, "handle": ws.map(|w| w.handle)})
        }
        Some(k2_core::sources::Maker::Login(u)) => json!({"kind": "login", "user": u}),
        _ => json!({"kind": "owner"}),
    }
}

fn target_count(r: &SourceRow) -> usize {
    serde_json::from_str::<Value>(&r.spec_json)
        .ok()
        .and_then(|v| v.get("fetch").and_then(|f| f.get("targets")).and_then(Value::as_array).map(Vec::len))
        .unwrap_or(1)
}

// ── approve ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ApproveBody {
    name: String,
    spec_hash: String,
    #[serde(default)]
    secret: Option<String>,
    #[serde(default)]
    confirm_token: Option<String>,
    #[serde(default)]
    viewers: Option<Viewers>,
    #[serde(default)]
    agent_access: Option<AgentAccess>,
    #[serde(default)]
    keep: Option<Keep>,
    #[serde(default)]
    use_anyway: bool,
}

/// The vault confirm stamp (`connect:<user>` for a login, as
/// `vault_routes` mints it).
fn confirm_stamp(who: &Who) -> Option<String> {
    match who {
        Who::Owner => Some("owner".into()),
        Who::Login { username, .. } => Some(format!("connect:{username}")),
        _ => None,
    }
}

fn vault() -> Result<std::sync::Arc<k2_vault::LocalVault>, SourceErr> {
    #[cfg(test)]
    if let Some(v) = test_seams::vault() {
        return Ok(v);
    }
    crate::vault::get().map_err(|e| SourceErr::new("vault_unavailable", 503, format!("the vault isn't open ({})", e.code)))
}

/// 403 `confirm_required` without a token (checked before the root-key
/// warning, so a warning never burns a token).
fn need_token(token: Option<&str>) -> Result<&str, SourceErr> {
    token.filter(|t| !t.is_empty()).ok_or_else(|| {
        SourceErr::new("confirm_required", 403, "binding a secret needs a vault confirm (Touch ID, your login or vault password)")
    })
}

/// Take a confirm token for `action` bound to `secret_id`.
fn take_confirm(who: &Who, action: &str, secret_id: &str, token: Option<&str>) -> Result<k2_vault::HumanProof, SourceErr> {
    let token = need_token(token)?;
    let stamp = confirm_stamp(who).ok_or_else(person_only)?;
    let v = vault()?;
    v.confirm
        .take(token, action, Some(secret_id), &stamp)
        .map_err(|_| SourceErr::new("confirm_failed", 403, "confirm again: a confirm works once, for this item, within 60 seconds"))
}

/// The bind origin for a source: its one origin (a secret-bound source has
/// exactly one).
fn bind_origin(row: &SourceRow) -> Result<String, SourceErr> {
    match row.origins.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(SourceErr::new(
            "bad_spec",
            400,
            "a source that uses a secret must fetch from one origin",
        )
        .with("specCode", "mixed_origins_with_secret")),
    }
}

/// The secret a person picked (or the one the proposal named).
fn secret_for(row: &SourceRow, picked: Option<&str>) -> Option<String> {
    picked.map(str::to_string).or_else(|| row.url_secret_name.clone()).or_else(|| row.secret_name.clone())
}

/// The vault scope an agent with "Manage Gardens and data sources" binds
/// `secret` from: its own workspace's secret first (`Some(ws)`), else a
/// server secret (`None`) only when it also has vault_manage. Anything
/// else is refused before any grant is made: 403 `secret_not_usable` (a
/// server secret without vault_manage), 404 `secret_not_found`. A person's
/// bind is always the server scope (`None`, today's rule).
fn bind_scope(who: &Who, secret: &str) -> Result<Option<String>, SourceErr> {
    let Some(ws) = own_project(who) else { return Ok(None) };
    if sec::exists(Some(ws), secret)? {
        return Ok(Some(ws.to_string()));
    }
    if sec::exists(None, secret)? {
        if sec::vault_manage(ws) {
            return Ok(None);
        }
        return Err(SourceErr::new(
            "secret_not_usable",
            403,
            format!("{secret} is a server-wide secret: binding it needs \"Allow managing server-wide secrets\" for this agent too; or keep it in your own workspace"),
        )
        .with("secret", secret));
    }
    Err(SourceErr::new("secret_not_found", 404, format!("no secret named {secret} in this workspace or on this server")).with("secret", secret))
}

/// Bind `secret` (scope `ws`) to `row` for its one origin: a person with
/// the confirm proof, an agent with the switch with none.
fn bind_for(who: &Who, row: &SourceRow, ws: Option<&str>, secret: &str, proof: Option<&k2_vault::HumanProof>) -> Result<(), SourceErr> {
    let origin = bind_origin(row)?;
    match proof {
        Some(p) => sec::bind(&row.id, ws, secret, &origin, super::secrets::BindAuth::Person(p)),
        None if is_manager(who) => sec::bind(&row.id, ws, secret, &origin, super::secrets::BindAuth::Agent),
        None => Err(person_only()),
    }
}

/// 403 `field_person_only` when an agent sets a person's choice on approve.
fn no_agent_choices(who: &Who, viewers: Option<Viewers>, access: Option<AgentAccess>, keep: Option<Keep>) -> Result<(), SourceErr> {
    if is_person(who) {
        return Ok(());
    }
    for (f, set) in [("viewers", viewers.is_some()), ("agentAccess", access.is_some()), ("keep", keep.is_some())] {
        if set {
            return Err(SourceErr::new("field_person_only", 403, format!("only a person sets {f}")).with("field", f));
        }
    }
    Ok(())
}

/// Root-key check: 409 `root_key` unless `useAnyway`.
fn check_root_key(who: &Who, row: &SourceRow, secret: &str, use_anyway: bool) -> Result<(), SourceErr> {
    check_root_key_in(who, row, None, secret, use_anyway)
}

fn check_root_key_in(who: &Who, row: &SourceRow, ws: Option<&str>, secret: &str, use_anyway: bool) -> Result<(), SourceErr> {
    if let Some(hint) = sec::root_key_hint_for(ws, secret)? {
        if !use_anyway {
            return Err(SourceErr::new("root_key", 409, hint.hint).with("rootKey", json!({"code": hint.code, "hint": hint.hint})));
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        audit(&conn, who, "root_key_use_anyway", Some(&row.id), None, json!({"secret": secret, "code": hint.code}));
    }
    Ok(())
}

/// Activate a proposed source (one transaction): status active, the
/// approval stamps, the person's choices for viewers/agentAccess/keep.
#[allow(clippy::too_many_arguments)]
fn activate(
    conn: &Connection,
    who: &Who,
    row: &SourceRow,
    b_viewers: Option<Viewers>,
    b_access: Option<AgentAccess>,
    b_keep: Option<Keep>,
    secret: Option<&str>,
    secret_ws: Option<&str>,
    bound: bool,
    now: i64,
) -> Result<(), SourceErr> {
    let (by, _) = actor(who);
    let enum_code = |v: Value| v.as_str().map(str::to_string).unwrap_or_default();
    let viewers = enum_code(json!(b_viewers.unwrap_or(row.viewers)));
    let access = enum_code(json!(b_access.unwrap_or(row.agent_access)));
    let keep = enum_code(json!(b_keep.unwrap_or(row.keep)));
    let (secret_col, url_secret_col) = match (secret, &row.url_secret_name) {
        (Some(_), Some(u)) => (row.secret_name.clone(), Some(u.clone())),
        (Some(s), None) => (Some(s.to_string()), None),
        (None, _) => (row.secret_name.clone(), row.url_secret_name.clone()),
    };
    let rev = store::bump_rev(conn).map_err(db_err)?;
    conn.execute(
        "UPDATE sources SET status = 'active', pause_reason = NULL, approved_by = ?2, approved_at = ?3, \
         approved_spec_hash = spec_hash, viewers = ?4, agent_access = ?5, keep = ?6, secret_name = ?7, \
         url_secret_name = ?8, secret_bound = ?9, rev = ?10, updated_at = ?3, secret_workspace = ?11 WHERE id = ?1",
        params![row.id, by, now, viewers, access, keep, secret_col, url_secret_col, i64::from(bound), rev, if bound { secret_ws } else { None }],
    )
    .map_err(db_err)?;
    Ok(())
}

fn approve(who: &Who, body: &[u8]) -> Result<Value, SourceErr> {
    let b: ApproveBody = parse(body)?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        source_by_name(&conn, &b.name)?
    };
    if row.status != SourceStatus::Proposed {
        return Err(SourceErr::new("not_proposed", 409, format!("{} is {}, not waiting for approval", row.name, status_code(row.status))));
    }
    if row.lan && !who.is_owner() {
        return Err(SourceErr::new("lan_owner_only", 403, "sources on a LAN address or plain http are approved by the owner"));
    }
    if b.spec_hash != row.spec_hash {
        return Err(SourceErr::new("spec_changed", 409, "the source changed since you reviewed it; review it again"));
    }
    no_agent_choices(who, b.viewers, b.agent_access, b.keep)?;
    let secret = secret_for(&row, b.secret.as_deref());
    let mut bound = false;
    let mut scope: Option<String> = None;
    if let Some(s) = &secret {
        if is_person(who) {
            need_token(b.confirm_token.as_deref())?;
            check_root_key(who, &row, s, b.use_anyway)?;
            let proof = take_confirm(who, k2_core::sources::CONFIRM_ACTION_SECRET, &row.id, b.confirm_token.as_deref())?;
            bind_for(who, &row, None, s, Some(&proof))?;
        } else {
            // An agent with "Manage Gardens and data sources": no confirm,
            // only a secret it may already use.
            scope = bind_scope(who, s)?;
            bind_origin(&row)?;
            check_root_key_in(who, &row, scope.as_deref(), s, b.use_anyway)?;
            bind_for(who, &row, scope.as_deref(), s, None)?;
        }
        bound = true;
    }
    let now = now();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        activate(&tx, who, &row, b.viewers, b.agent_access, b.keep, secret.as_deref(), scope.as_deref(), bound, now)?;
        audit(&tx, who, "approve", Some(&row.id), None, json!({"name": row.name, "specHash": row.spec_hash}));
        if bound {
            audit(&tx, who, "bind", Some(&row.id), None, json!({"secret": secret, "scope": scope_code(scope.as_deref())}));
        }
        tx.commit().map_err(db_err)?;
    }
    crate::gardens_manage::audit_who(who, "sources-approve", &row.name, None);
    if bound {
        crate::gardens_manage::audit_who(who, "sources-bind", &row.name, secret.as_deref());
    }
    let next_at = super::engine().poll_now(&row.id).ok();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        changed(&conn);
    }
    Ok(json!({"name": row.name, "status": "active", "secretBound": bound, "nextAt": next_at}))
}

/// `server` or `workspace` for an audit row (never the value).
fn scope_code(ws: Option<&str>) -> &'static str {
    if ws.is_some() {
        "workspace"
    } else {
        "server"
    }
}

fn status_code(s: SourceStatus) -> &'static str {
    match s {
        SourceStatus::Proposed => "proposed",
        SourceStatus::Active => "active",
        SourceStatus::Paused => "paused",
        SourceStatus::Removed => "removed",
    }
}

// ── batch ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BatchItem {
    name: String,
    spec_hash: String,
    #[serde(default)]
    secret: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BatchBody {
    #[serde(default)]
    items: Vec<BatchItem>,
    #[serde(default)]
    triggers: Vec<String>,
    #[serde(default)]
    confirm_token: Option<String>,
    #[serde(default)]
    use_anyway: bool,
}

/// `sha256_hex(join("\n", sort(["<sourceId>:<secretName>" …])))`
/// (contract §4.3): the `secretId` a `source-batch` confirm is bound to.
pub fn batch_hash(pairs: &[(String, String)]) -> String {
    use sha2::{Digest, Sha256};
    let mut lines: Vec<String> = pairs.iter().map(|(id, s)| format!("{id}:{s}")).collect();
    lines.sort();
    let d = Sha256::digest(lines.join("\n").as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn batch_approve(who: &Who, body: &[u8]) -> Result<Value, SourceErr> {
    let b: BatchBody = parse(body)?;
    if b.items.is_empty() && b.triggers.is_empty() {
        return Err(bad_request("nothing to approve"));
    }
    // Check everything first; any mismatch changes nothing.
    let (rows, trigger_rows) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut rows = Vec::new();
        for it in &b.items {
            let r = source_by_name(&conn, &it.name)?;
            if r.status != SourceStatus::Proposed {
                return Err(SourceErr::new("batch_changed", 409, format!("{} is no longer waiting for approval", r.name)));
            }
            if r.spec_hash != it.spec_hash {
                return Err(SourceErr::new("batch_changed", 409, format!("{} changed since you reviewed it", r.name)));
            }
            if r.lan && !who.is_owner() {
                return Err(SourceErr::new("lan_owner_only", 403, format!("{} is on a LAN address; the owner approves it", r.name)));
            }
            let secret = secret_for(&r, it.secret.as_deref());
            rows.push((r, secret));
        }
        let mut trows = Vec::new();
        for id in &b.triggers {
            let t = ts::get(&conn, id).map_err(db_err)?.ok_or_else(|| SourceErr::new("trigger_not_found", 404, format!("no trigger {id}")))?;
            if t.status != TriggerStatus::Proposed {
                return Err(SourceErr::new("batch_changed", 409, format!("trigger {id} is no longer waiting for approval")));
            }
            trows.push(t);
        }
        (rows, trows)
    };
    let pairs: Vec<(String, String)> = rows.iter().filter_map(|(r, s)| s.as_ref().map(|s| (r.id.clone(), s.clone()))).collect();
    let person = is_person(who);
    if person && !pairs.is_empty() {
        need_token(b.confirm_token.as_deref()).map_err(|e| e.with("batchHash", batch_hash(&pairs)))?;
    }
    // Every secret's scope is settled before anything is bound: one secret
    // an agent can't use refuses the whole batch, and nothing changes.
    let mut scopes: Vec<Option<String>> = Vec::with_capacity(rows.len());
    for (r, s) in &rows {
        let scope = match s {
            Some(s) if !person => bind_scope(who, s)?,
            _ => None,
        };
        if let Some(s) = s {
            bind_origin(r)?;
            check_root_key_in(who, r, scope.as_deref(), s, b.use_anyway)?;
        }
        scopes.push(scope);
    }
    let proof = if pairs.is_empty() || !person {
        None
    } else {
        let token = b.confirm_token.as_deref().filter(|t| !t.is_empty()).ok_or_else(|| {
            SourceErr::new("confirm_required", 403, "approving sources that use secrets needs one vault confirm").with("batchHash", batch_hash(&pairs))
        })?;
        let stamp = confirm_stamp(who).ok_or_else(person_only)?;
        let v = vault()?;
        Some(
            v.confirm
                .take(token, k2_core::sources::CONFIRM_ACTION_BATCH, Some(&batch_hash(&pairs)), &stamp)
                .map_err(|_| SourceErr::new("batch_changed", 409, "the confirm doesn't match this list; confirm the list again"))?,
        )
    };
    // Bind secrets (all or none), then commit every status change in one
    // transaction.
    let mut bound_ids: Vec<String> = Vec::new();
    if proof.is_some() || !person {
        for ((r, s), scope) in rows.iter().zip(&scopes) {
            if let Some(s) = s {
                if let Err(e) = bind_for(who, r, scope.as_deref(), s, proof.as_ref()) {
                    for id in &bound_ids {
                        let _ = sec::unbind(id);
                    }
                    return Err(e);
                }
                bound_ids.push(r.id.clone());
            }
        }
    }
    let now = now();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        for ((r, s), scope) in rows.iter().zip(&scopes) {
            activate(&tx, who, r, None, None, None, s.as_deref(), scope.as_deref(), s.is_some(), now)?;
            if let Some(s) = s {
                audit(&tx, who, "bind", Some(&r.id), None, json!({"secret": s, "scope": scope_code(scope.as_deref())}));
            }
        }
        let (by, _) = actor(who);
        for t in &trigger_rows {
            ts::set_status(&tx, &t.id, TriggerStatus::Active, Some(&by), now).map_err(db_err)?;
        }
        audit(
            &tx,
            who,
            "batch_approve",
            None,
            None,
            json!({"sources": rows.iter().map(|(r, _)| r.name.clone()).collect::<Vec<_>>(), "triggers": b.triggers, "bound": bound_ids.len()}),
        );
        tx.commit().map_err(db_err)?;
        changed(&conn);
    }
    for (r, s) in &rows {
        crate::gardens_manage::audit_who(who, "sources-approve", &r.name, None);
        if s.is_some() {
            crate::gardens_manage::audit_who(who, "sources-bind", &r.name, s.as_deref());
        }
        let _ = super::engine().poll_now(&r.id);
    }
    for t in &trigger_rows {
        crate::gardens_manage::audit_who(who, "sources-trigger-approve", &t.id, None);
    }
    Ok(json!({
        "approved": rows.iter().map(|(r, _)| r.name.clone()).collect::<Vec<_>>(),
        "triggersApproved": trigger_rows.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
    }))
}

// ── secret ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SecretBody {
    name: String,
    secret: Option<String>,
    #[serde(default)]
    confirm_token: Option<String>,
    #[serde(default)]
    use_anyway: bool,
}

fn secret(who: &Who, body: &[u8]) -> Result<Value, SourceErr> {
    let b: SecretBody = parse(body)?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        source_by_name(&conn, &b.name)?
    };
    if row.lan && !who.is_owner() {
        return Err(SourceErr::new("lan_owner_only", 403, "sources on a LAN address or plain http are the owner's"));
    }
    let now = now();
    match b.secret.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => {
            if row.secret_bound {
                sec::unbind(&row.id)?;
            }
            let db = k2_core::db::shared();
            let conn = db.lock();
            let rev = store::bump_rev(&conn).map_err(db_err)?;
            conn.execute(
                "UPDATE sources SET secret_name = NULL, secret_bound = 0, secret_workspace = NULL, rev = ?2, updated_at = ?3 WHERE id = ?1",
                params![row.id, rev, now],
            )
            .map_err(db_err)?;
            audit(&conn, who, "unbind", Some(&row.id), None, json!({"secret": row.secret_name}));
            changed(&conn);
            drop(conn);
            crate::gardens_manage::audit_who(who, "sources-unbind", &row.name, row.secret_name.as_deref());
            Ok(json!({"name": row.name, "secret": Value::Null, "secretBound": false}))
        }
        Some(s) => {
            if row.url_secret_name.is_some() {
                return Err(bad_request("this source's URL is itself a vault secret; approve it to bind that secret"));
            }
            let (scope, proof) = if is_person(who) {
                need_token(b.confirm_token.as_deref())?;
                check_root_key(who, &row, s, b.use_anyway)?;
                (None, Some(take_confirm(who, k2_core::sources::CONFIRM_ACTION_SECRET, &row.id, b.confirm_token.as_deref())?))
            } else {
                let scope = bind_scope(who, s)?;
                check_root_key_in(who, &row, scope.as_deref(), s, b.use_anyway)?;
                (scope, None)
            };
            bind_origin(&row)?;
            if row.secret_bound {
                sec::unbind(&row.id)?;
            }
            bind_for(who, &row, scope.as_deref(), s, proof.as_ref())?;
            let db = k2_core::db::shared();
            let conn = db.lock();
            let rev = store::bump_rev(&conn).map_err(db_err)?;
            conn.execute(
                "UPDATE sources SET secret_name = ?2, secret_bound = 1, secret_workspace = ?5, rev = ?3, updated_at = ?4 WHERE id = ?1",
                params![row.id, s, rev, now, scope],
            )
            .map_err(db_err)?;
            audit(&conn, who, "bind", Some(&row.id), None, json!({"secret": s, "scope": scope_code(scope.as_deref())}));
            changed(&conn);
            drop(conn);
            crate::gardens_manage::audit_who(who, "sources-bind", &row.name, Some(s));
            let _ = super::engine().poll_now(&row.id);
            Ok(json!({"name": row.name, "secret": s, "secretBound": true}))
        }
    }
}

// ── pin ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NameBody {
    name: String,
}

fn pin(who: &Who, body: &[u8], on: bool) -> Result<Value, SourceErr> {
    let b: NameBody = parse(body)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = source_by_name(&conn, &b.name)?;
    if row.pinned != on {
        let (by, _) = actor(who);
        let now = now();
        let rev = store::bump_rev(&conn).map_err(db_err)?;
        if on {
            conn.execute(
                "UPDATE sources SET pinned = 1, pinned_by = ?2, pinned_at = ?3, rev = ?4, updated_at = ?3 WHERE id = ?1",
                params![row.id, by, now, rev],
            )
        } else {
            conn.execute(
                "UPDATE sources SET pinned = 0, pinned_by = NULL, pinned_at = NULL, rev = ?3, updated_at = ?2 WHERE id = ?1",
                params![row.id, now, rev],
            )
        }
        .map_err(db_err)?;
        audit(&conn, who, if on { "pin" } else { "unpin" }, Some(&row.id), None, json!({"name": row.name}));
        changed(&conn);
        drop(conn);
        crate::gardens_manage::audit_who(who, if on { "sources-pin" } else { "sources-unpin" }, &row.name, None);
    }
    Ok(json!({"name": row.name, "pinned": on}))
}

/// B1's secret calls (`sources::secrets`), behind a seam so the approval
/// rules are testable without a real vault grant.
mod sec {
    use super::super::secrets::{self, RootKeyHint};
    use super::SourceErr;

    pub(super) fn bind(
        source_id: &str,
        ws: Option<&str>,
        secret: &str,
        origin: &str,
        auth: secrets::BindAuth<'_>,
    ) -> Result<(), SourceErr> {
        #[cfg(test)]
        if let Some(f) = super::test_seams::secrets() {
            if let secrets::BindAuth::Person(p) = &auth {
                if !matches!(p, k2_vault::HumanProof::Confirmed { .. }) {
                    return Err(SourceErr::new("confirm_required", 403, "binding a secret needs a person's confirm"));
                }
            }
            return (f.bind)(source_id, secret, origin);
        }
        secrets::bind_in(source_id, ws, secret, origin, auth)
    }

    /// Does a secret called `secret` exist in scope `ws` (`None`: server)?
    pub(super) fn exists(ws: Option<&str>, secret: &str) -> Result<bool, SourceErr> {
        #[cfg(test)]
        if let Some(f) = super::test_seams::secrets() {
            return Ok((f.exists)(ws, secret));
        }
        secrets::secret_exists(ws, secret)
    }

    /// The workspace's vault_manage switch (0149).
    pub(super) fn vault_manage(ws: &str) -> bool {
        crate::vault::actor::vault_manage(ws)
    }

    pub(super) fn unbind(source_id: &str) -> Result<(), SourceErr> {
        #[cfg(test)]
        if let Some(f) = super::test_seams::secrets() {
            return (f.unbind)(source_id);
        }
        secrets::unbind(source_id)
    }

    pub(super) fn root_key_hint_for(ws: Option<&str>, secret: &str) -> Result<Option<RootKeyHint>, SourceErr> {
        #[cfg(test)]
        if let Some(f) = super::test_seams::secrets() {
            return Ok((f.root_key)(secret));
        }
        secrets::root_key_hint_in(ws, secret)
    }
}

/// Test seams: a fake vault for confirm tokens, fake secret binding.
#[cfg(test)]
pub mod test_seams {
    use std::cell::RefCell;
    use std::sync::Arc;

    use super::super::secrets::RootKeyHint;
    use super::SourceErr;

    #[derive(Clone, Copy)]
    pub struct FakeSecrets {
        pub bind: fn(&str, &str, &str) -> Result<(), SourceErr>,
        pub unbind: fn(&str) -> Result<(), SourceErr>,
        pub root_key: fn(&str) -> Option<RootKeyHint>,
        /// Does a secret exist in this scope (`None`: server)?
        pub exists: fn(Option<&str>, &str) -> bool,
    }

    thread_local! {
        static VAULT: RefCell<Option<Arc<k2_vault::LocalVault>>> = const { RefCell::new(None) };
        static SECRETS: RefCell<Option<FakeSecrets>> = const { RefCell::new(None) };
    }

    pub fn set_vault(v: Option<Arc<k2_vault::LocalVault>>) {
        VAULT.with(|c| *c.borrow_mut() = v);
    }

    pub(super) fn vault() -> Option<Arc<k2_vault::LocalVault>> {
        VAULT.with(|c| c.borrow().clone())
    }

    pub fn set_secrets(f: Option<FakeSecrets>) {
        SECRETS.with(|c| *c.borrow_mut() = f);
    }

    pub(super) fn secrets() -> Option<FakeSecrets> {
        SECRETS.with(|c| *c.borrow())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use k2_core::connect_users::Role;

    pub(crate) fn admin() -> Who {
        Who::Login { username: "ada".into(), role: Role::Admin }
    }

    pub(crate) fn member() -> Who {
        Who::Login { username: "mo".into(), role: Role::Member }
    }

    pub(crate) fn agent(p: &str) -> Who {
        Who::Agent { project_id: p.into(), session_id: "sess".into() }
    }

    pub(crate) fn hash() -> String {
        format!("{:064}", 7)
    }

    /// Insert a source row the way B1's propose would.
    pub(crate) fn insert_source(name: &str, status: &str, secret: Option<&str>, lan: bool, viewers: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let h = k2_core::db::shared();
        let c = h.lock();
        c.execute(
            "INSERT INTO sources (id, name, label, spec_json, spec_hash, origins_json, lan, secret_name, every_secs, \
             viewers, agent_access, keep, status, made_by, made_by_project, rev, created_at, updated_at) \
             VALUES (?1, ?2, ?3, '{\"fetch\":{\"targets\":[{\"key\":\"a\",\"url\":\"https://a.k2.dev/boot-status\"},{\"key\":\"b\",\"url\":\"https://b.k2.dev/boot-status\"}]}}', \
             ?4, '[\"https://api.example.com:443\"]', ?5, ?6, 60, ?7, 'values', 'values', ?8, 'agent:p1', 'p1', 1, 0, 0)",
            params![id, name, format!("{name} label"), hash(), i64::from(lan), secret, viewers, status],
        )
        .unwrap();
        id
    }

    fn row(name: &str) -> SourceRow {
        let h = k2_core::db::shared();
        let c = h.lock();
        store::get_by_name(&c, name).unwrap().unwrap()
    }

    fn call(path: &str, who: &Who, body: Value) -> Result<Value, SourceErr> {
        handle(path, who, body.to_string().as_bytes(), &HashMap::new())
    }

    fn audit_actions() -> Vec<String> {
        let h = k2_core::db::shared();
        let c = h.lock();
        let mut s = c.prepare("SELECT action FROM source_audit ORDER BY id").unwrap();
        let rows: Vec<String> = s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        rows
    }

    fn fake_secrets() {
        test_seams::set_secrets(Some(test_seams::FakeSecrets {
            bind: |_, _, _| Ok(()),
            unbind: |_| Ok(()),
            root_key: |s| {
                (s == "STRIPE_SK")
                    .then_some(super::super::secrets::RootKeyHint { code: "stripe_secret", hint: "use a restricted key" })
            },
            // Workspace p1 keeps WS_KEY; the server keeps VERCEL_RO,
            // STRIPE_RK and STRIPE_SK.
            exists: |ws, s| match ws {
                Some("p1") => s == "WS_KEY",
                Some(_) => false,
                None => matches!(s, "VERCEL_RO" | "STRIPE_RK" | "STRIPE_SK"),
            },
        }));
    }

    fn mint(action: &str, id: &str, stamp: &str) -> String {
        let v = crate::compute_admin::test_seams::vault_handle();
        v.confirm.mint(action, Some(id), stamp, "vault-password")
    }

    pub(crate) struct Env {
        _db: k2_core::db::TestDbGuard,
    }
    impl Drop for Env {
        fn drop(&mut self) {
            test_seams::set_vault(None);
            test_seams::set_secrets(None);
        }
    }
    pub(crate) fn env() -> Env {
        let db = k2_core::db::scoped_for_test();
        test_seams::set_vault(Some(crate::compute_admin::test_seams::vault_handle()));
        fake_secrets();
        Env { _db: db }
    }

    /// The renderer's `batchHash` pins the same vector.
    #[test]
    fn batch_hash_sorts_lines_and_matches_the_renderer() {
        let h = batch_hash(&[("b".into(), "Y".into()), ("a".into(), "X".into())]);
        assert_eq!(h, "6976188b7ee15f6eef1614e1c09a81e74d9915020a7b0801b2b276e368652074");
    }

    #[test]
    fn agents_and_shells_are_person_only_on_every_approval_route() {
        let _e = env();
        insert_source("fleet", "proposed", None, false, "member");
        let shell = Who::Shell { project_id: "p1".into(), session_id: "s".into() };
        for path in [
            "/cli/sources/pending",
            "/cli/sources/approve",
            "/cli/sources/batch-approve",
            "/cli/sources/secret",
            "/cli/sources/pin",
            "/cli/sources/unpin",
        ] {
            for who in [agent("p1"), shell.clone()] {
                let e = call(path, &who, json!({"name": "fleet", "specHash": hash()})).unwrap_err();
                assert_eq!((e.code, e.status), ("person_only", 403), "{path}");
            }
        }
    }

    #[test]
    fn one_click_approve_for_a_secretless_public_source() {
        let _e = env();
        insert_source("fleet", "proposed", None, false, "member");
        let p = call("/cli/sources/pending", &admin(), json!({})).unwrap();
        assert_eq!(p["sources"][0]["approval"], "one_click");
        assert_eq!(p["sources"][0]["targets"], 2);
        let stale = call("/cli/sources/approve", &admin(), json!({"name": "fleet", "specHash": "0".repeat(64)})).unwrap_err();
        assert_eq!((stale.code, stale.status), ("spec_changed", 409));
        let r = call("/cli/sources/approve", &admin(), json!({"name": "fleet", "specHash": hash()})).unwrap();
        assert_eq!((r["status"].as_str(), r["secretBound"].as_bool()), (Some("active"), Some(false)));
        let after = row("fleet");
        assert_eq!(after.status, SourceStatus::Active);
        assert_eq!(after.approved_by.as_deref(), Some("login:ada"));
        assert_eq!(after.approved_spec_hash.as_deref(), Some(hash().as_str()));
        assert!(audit_actions().contains(&"approve".to_string()));
        let again = call("/cli/sources/approve", &admin(), json!({"name": "fleet", "specHash": hash()})).unwrap_err();
        assert_eq!(again.code, "not_proposed");
        let unknown =
            call("/cli/sources/approve", &admin(), json!({"name": "fleet", "specHash": hash(), "bogus": 1})).unwrap_err();
        assert_eq!(unknown.code, "bad_request", "unknown fields are refused");
    }

    #[test]
    fn lan_sources_are_approved_by_the_owner_only() {
        let _e = env();
        insert_source("nas", "proposed", None, true, "member");
        let p = call("/cli/sources/pending", &admin(), json!({})).unwrap();
        assert_eq!(p["sources"][0]["approval"], "owner_only");
        let e = call("/cli/sources/approve", &admin(), json!({"name": "nas", "specHash": hash()})).unwrap_err();
        assert_eq!((e.code, e.status), ("lan_owner_only", 403));
        let owner_login = Who::Login { username: "own".into(), role: Role::Owner };
        call("/cli/sources/approve", &owner_login, json!({"name": "nas", "specHash": hash()})).unwrap();
    }

    #[test]
    fn a_secret_needs_a_confirm_bound_to_the_source_id() {
        let _e = env();
        let id = insert_source("deploys", "proposed", Some("VERCEL_RO"), false, "admin");
        let p = call("/cli/sources/pending", &admin(), json!({})).unwrap();
        assert_eq!(p["sources"][0]["approval"], "needs_confirm");
        assert_eq!(p["sources"][0]["id"], id.as_str());
        let e = call("/cli/sources/approve", &admin(), json!({"name": "deploys", "specHash": hash()})).unwrap_err();
        assert_eq!((e.code, e.status), ("confirm_required", 403));
        // A token for another source, another action or another person is refused.
        for (action, sid, stamp) in
            [("source-secret", "some-other-id", "connect:ada"), ("reveal", id.as_str(), "connect:ada"), ("source-secret", id.as_str(), "owner")]
        {
            let t = mint(action, sid, stamp);
            let e = call("/cli/sources/approve", &admin(), json!({"name": "deploys", "specHash": hash(), "confirmToken": t}))
                .unwrap_err();
            assert_eq!(e.code, "confirm_failed", "{action} {sid} {stamp}");
        }
        let good = mint("source-secret", &id, "connect:ada");
        let r = call("/cli/sources/approve", &admin(), json!({"name": "deploys", "specHash": hash(), "confirmToken": good}))
            .unwrap();
        assert_eq!(r["secretBound"], true);
        let after = row("deploys");
        assert!(after.secret_bound);
        assert_eq!(after.secret_name.as_deref(), Some("VERCEL_RO"));
        let acts = audit_actions();
        assert!(acts.contains(&"approve".to_string()) && acts.contains(&"bind".to_string()), "{acts:?}");
    }

    #[test]
    fn root_keys_warn_then_use_anyway() {
        let _e = env();
        let id = insert_source("money", "proposed", Some("STRIPE_SK"), false, "admin");
        let t = mint("source-secret", &id, "owner");
        let e = call("/cli/sources/approve", &Who::Owner, json!({"name": "money", "specHash": hash(), "confirmToken": t}))
            .unwrap_err();
        assert_eq!((e.code, e.status), ("root_key", 409));
        assert_eq!(e.extra["rootKey"]["code"], "stripe_secret");
        // The warning didn't burn the token.
        let r = call(
            "/cli/sources/approve",
            &Who::Owner,
            json!({"name": "money", "specHash": hash(), "confirmToken": t, "useAnyway": true, "keep": "states"}),
        )
        .unwrap();
        assert_eq!(r["status"], "active");
        assert_eq!(row("money").keep, Keep::States);
        assert!(audit_actions().contains(&"root_key_use_anyway".to_string()));
    }

    #[test]
    fn batch_confirm_is_bound_to_the_exact_list() {
        let _e = env();
        let a = insert_source("deploys", "proposed", Some("VERCEL_RO"), false, "admin");
        let b = insert_source("money", "proposed", Some("STRIPE_RK"), false, "admin");
        insert_source("fleet", "proposed", None, false, "member");
        let trig = {
            let h = k2_core::db::shared();
            let c = h.lock();
            let src = store::get_by_name(&c, "fleet").unwrap().unwrap();
            ts::insert(
                &c,
                &ts::NewTrigger {
                    source_id: &src.id,
                    on: &k2_core::sources::triggers::TriggerOn::ItemWentCrit,
                    target_project_id: "p1",
                    note: None,
                    cooldown_secs: 1800,
                    daily_cap: 12,
                    status: TriggerStatus::Proposed,
                    made_by: "agent:p1",
                    made_by_project: Some("p1"),
                    approved_by: None,
                    now: 0,
                },
            )
            .unwrap()
        };
        let items = json!([
            {"name": "deploys", "specHash": hash()},
            {"name": "money", "specHash": hash()},
            {"name": "fleet", "specHash": hash()},
        ]);
        let e = call("/cli/sources/batch-approve", &admin(), json!({"items": items, "triggers": [trig.id]})).unwrap_err();
        assert_eq!(e.code, "confirm_required");
        let want = batch_hash(&[(a.clone(), "VERCEL_RO".into()), (b.clone(), "STRIPE_RK".into())]);
        assert_eq!(e.extra["batchHash"], want.as_str());
        // A confirm for a different list (one item) is refused; nothing changes.
        let narrow = mint("source-batch", &batch_hash(&[(a.clone(), "VERCEL_RO".into())]), "connect:ada");
        let e = call("/cli/sources/batch-approve", &admin(), json!({"items": items, "triggers": [trig.id], "confirmToken": narrow}))
            .unwrap_err();
        assert_eq!((e.code, e.status), ("batch_changed", 409));
        assert_eq!(row("fleet").status, SourceStatus::Proposed);
        let token = mint("source-batch", &want, "connect:ada");
        let r = call("/cli/sources/batch-approve", &admin(), json!({"items": items, "triggers": [trig.id], "confirmToken": token}))
            .unwrap();
        assert_eq!(r["approved"], json!(["deploys", "money", "fleet"]));
        assert_eq!(r["triggersApproved"], json!([trig.id]));
        for n in ["deploys", "money", "fleet"] {
            assert_eq!(row(n).status, SourceStatus::Active, "{n}");
        }
        assert!(row("money").secret_bound && !row("fleet").secret_bound);
        // A stale hash in the list refuses the whole batch.
        insert_source("late", "proposed", None, false, "member");
        let e = call("/cli/sources/batch-approve", &admin(), json!({"items": [{"name": "late", "specHash": "1".repeat(64)}]}))
            .unwrap_err();
        assert_eq!(e.code, "batch_changed");
    }

    #[test]
    fn secret_route_binds_with_confirm_and_unbinds_without() {
        let _e = env();
        let id = insert_source("deploys", "active", None, false, "admin");
        let e = call("/cli/sources/secret", &admin(), json!({"name": "deploys", "secret": "VERCEL_RO"})).unwrap_err();
        assert_eq!(e.code, "confirm_required");
        let t = mint("source-secret", &id, "connect:ada");
        let r = call("/cli/sources/secret", &admin(), json!({"name": "deploys", "secret": "VERCEL_RO", "confirmToken": t}))
            .unwrap();
        assert_eq!(r, json!({"name": "deploys", "secret": "VERCEL_RO", "secretBound": true}));
        let r = call("/cli/sources/secret", &admin(), json!({"name": "deploys", "secret": null})).unwrap();
        assert_eq!(r["secretBound"], false);
        assert!(!row("deploys").secret_bound);
    }

    #[test]
    fn pin_and_unpin_stamp_and_bump_the_rev() {
        let _e = env();
        insert_source("fleet", "active", None, false, "member");
        let before = {
            let h = k2_core::db::shared();
            let c = h.lock();
            store::current_rev(&c).unwrap()
        };
        assert_eq!(call("/cli/sources/pin", &admin(), json!({"name": "fleet"})).unwrap(), json!({"name": "fleet", "pinned": true}));
        let r = row("fleet");
        assert!(r.pinned && r.pinned_by.as_deref() == Some("login:ada"));
        assert!(r.rev > before);
        call("/cli/sources/unpin", &admin(), json!({"name": "fleet"})).unwrap();
        assert!(!row("fleet").pinned);
        let gone = call("/cli/sources/pin", &admin(), json!({"name": "nope"})).unwrap_err();
        assert_eq!((gone.code, gone.status), ("source_not_found", 404));
    }

    #[test]
    fn removed_names_answer_410() {
        let _e = env();
        let id = insert_source("old", "active", None, false, "member");
        {
            let h = k2_core::db::shared();
            let c = h.lock();
            c.execute("UPDATE sources SET status = 'removed', removed_at = 99 WHERE id = ?1", params![id]).unwrap();
        }
        let e = call("/cli/sources/pin", &admin(), json!({"name": "old"})).unwrap_err();
        assert_eq!((e.code, e.status), ("removed", 410));
        assert_eq!(e.extra["removedAt"], 99);
    }

    #[test]
    fn member_logins_cant_read_admin_sources() {
        let _e = env();
        insert_source("money", "active", None, false, "admin");
        let r = row("money");
        let e = check_viewers(&member(), &r).unwrap_err();
        assert_eq!((e.code, e.status), ("cap_not_granted", 403));
        assert_eq!(e.extra["role"], "admin");
        assert!(check_viewers(&admin(), &r).is_ok());
        assert!(check_viewers(&agent("p1"), &r).is_ok());
        assert!(check_viewers(&Who::Owner, &r).is_ok());
    }
}
