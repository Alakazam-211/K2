//! `/cli/sew/approvals*` — the chat approval queue (S30 K18).
//!
//! **Who may approve** is a per-box admin policy (Rosson 2026-10-15),
//! `approvals.by`:
//! - `people` (default): a person only — the owner token or an Owner/Admin
//!   Connect login, each with a fresh vault confirm bound to the item
//!   (`chat-approve`, `secretId` = the `out_…` id) or to the batch
//!   (`chat-approve-batch`, `secretId` = [`core::batch_hash`]). Same shape
//!   as node-admin approvals (`compute_admin.rs` `approve`). The confirm is
//!   single-use and expires in 60 s.
//! - `people+agents`: also an agent (or K2 shell tab) whose workspace has
//!   Manage Sew (`chat_manage`) on. Recorded as `decided_kind = agent` and
//!   shown as an agent's approval — never person proof (KM13 rev 148).
//!
//! Nobody approves their own post: an agent's own items are refused (it
//! cancels them with `k2 chat outbox cancel`). A post an assistant makes
//! on a person's behalf is still the agent's post (Rosson 2026-10-27,
//! ticket 411eda53), so that person may approve it here like anyone else;
//! their own after-the-fact review of what was said for them is a separate
//! record (`/cli/sew/on-behalf/review`). Deny needs a note;
//! an agent with Manage Sew may deny other agents' items whatever the
//! policy (denying never widens anything). Widening the policy is a person
//! step (owner/admin); an agent with Manage Sew may only tighten it back to
//! `people`.
//!
//! These are not the KM13 widget-library approvals; the queues stay apart.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use k2_core::connect_users::Role;
use k2_core::sew_chat::{self as core, store, ApprovalsBy};

use super::routes::{
    backend, credential, emit_hook, expire_now, internal, now, outbox_json, parse_body, sew_err, usage, ChatErr, R,
};
use super::sew::{self, ActAs, NewMessage};
use crate::compute_routes::Who;

/// Who is deciding.
#[derive(Debug, Clone)]
enum Decider {
    /// The owner token or an Owner/Admin login. `stamp` is the vault
    /// confirm stamp (`owner` / `connect:<user>`).
    Person { label: String, stamp: String },
    /// A passport whose workspace has Manage Sew on.
    Agent { project_id: String, label: String },
}

impl Decider {
    fn label(&self) -> &str {
        match self {
            Decider::Person { label, .. } | Decider::Agent { label, .. } => label,
        }
    }
}

fn decider(who: &Who) -> R<Decider> {
    credential(who)?;
    match who {
        Who::Owner => Ok(Decider::Person {
            label: "owner".into(),
            stamp: "owner".into(),
        }),
        Who::Login { username, role } if *role >= Role::Admin => Ok(Decider::Person {
            label: format!("login:{username}"),
            stamp: format!("connect:{username}"),
        }),
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => {
            if !k2_core::workspace::settings::agents_can_manage_chat(project_id) {
                return Err(ChatErr::new(
                    "chat_manage_off",
                    403,
                    "the chat approval queue needs the Manage Chat / Voice Hosting switch on this workspace — your owner turns it on \
                     (k2 agent access set <workspace> chat_manage on)",
                ));
            }
            Ok(Decider::Agent { project_id: project_id.clone(), label: format!("agent:{}", crate::compute_routes::ws_handle(project_id)) })
        }
        _ => Err(owner_only()),
    }
}

fn owner_only() -> ChatErr {
    ChatErr::new(
        "owner_only",
        403,
        "only the owner or an admin decides this — Settings, or k2 hostchat approvals from a terminal outside K2",
    )
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<Value> {
    match path {
        "/cli/sew/approvals" => list(who, params),
        "/cli/sew/approvals/approve" => approve(who, body),
        "/cli/sew/approvals/approve-batch" => approve_batch(who, body),
        "/cli/sew/approvals/deny" => deny(who, body),
        "/cli/sew/approvals/policy" => policy(who),
        "/cli/sew/approvals/policy/set" => policy_set(who, body),
        _ => Err(ChatErr::new("not_found", 404, "no such approvals route")),
    }
}

fn policy_now() -> R<ApprovalsBy> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::approvals_by(&conn).map_err(internal)
}

/// Can `d` approve `r` under `by`? `Err` carries the refusal.
fn may_approve(d: &Decider, by: ApprovalsBy, r: &store::OutboxRow) -> R<()> {
    match d {
        // An on-behalf post is the agent's own, not the person's (Rosson
        // 2026-10-27): any person may approve it, including the one it
        // speaks for.
        Decider::Person { .. } => Ok(()),
        Decider::Agent { project_id, .. } => {
            if by == ApprovalsBy::People {
                return Err(ChatErr::new(
                    "approvals_people_only",
                    403,
                    "on this box only a person approves queued chat posts (approvals.by = people)",
                ));
            }
            if &r.project_id == project_id {
                return Err(own_item());
            }
            Ok(())
        }
    }
}

fn own_item() -> ChatErr {
    ChatErr::new("own_item", 403, "nobody approves or denies their own post — cancel it with k2 chat outbox cancel <out_…>")
}

fn approval_json(r: &store::OutboxRow, d: &Decider, by: ApprovalsBy) -> Value {
    let mut v = outbox_json(r);
    v["agent"] = r.agent_name.clone().into();
    v["queueReason"] = r.queue_reason.clone().into();
    v["preview"] = r.body.as_deref().map(core::preview).into();
    v["canApprove"] = may_approve(d, by, r).is_ok().into();
    v
}

fn list(who: &Who, params: &HashMap<String, String>) -> R<Value> {
    let d = decider(who)?;
    let by = policy_now()?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    expire_now(&conn)?;
    let mut rows = store::queued(&conn).map_err(internal)?;
    if let Some(ws) = params.get("workspace").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let id = crate::compute_routes::ws_by_token(ws).map(|w| w.id).ok_or_else(|| ChatErr::new("not_found", 404, format!("no workspace '{ws}'")))?;
        rows.retain(|r| r.project_id == id);
    }
    Ok(json!({
        "ok": true,
        "approvalsBy": by.as_str(),
        "decider": d.label(),
        "count": rows.len(),
        "items": rows.iter().map(|r| approval_json(r, &d, by)).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ApproveBody {
    id: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    confirm_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BatchBody {
    ids: Vec<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    confirm_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DenyBody {
    id: String,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PolicyBody {
    #[serde(default)]
    approvals_by: Option<String>,
    /// The Space whose access governs DMs outside every Space (`none` or
    /// empty clears it).
    #[serde(default)]
    default_space: Option<String>,
}

fn vault() -> R<std::sync::Arc<k2_vault::LocalVault>> {
    #[cfg(test)]
    if let Some(v) = test_seams::vault() {
        return Ok(v);
    }
    crate::vault::get().map_err(|e| ChatErr::new("vault_closed", 503, format!("the vault isn't open ({}): {}", e.code, e.hint)))
}

/// Take a person's confirm: single use, bound to `action` + `secret_id`.
/// Returns the confirm method.
pub(crate) fn take_confirm(stamp: &str, token: Option<&str>, action: &str, secret_id: &str) -> R<String> {
    let need = || {
        ChatErr::new(
            "confirm_required",
            403,
            "this needs a fresh vault confirm from a person (Touch ID, your login or the vault password) — \
             k2 hostchat asks for it from a terminal outside K2",
        )
        .with("action", action)
        .with("secretId", secret_id)
    };
    let token = token.map(str::trim).filter(|t| !t.is_empty()).ok_or_else(need)?;
    let proof = vault()?
        .confirm
        .take(token, action, Some(secret_id), stamp)
        .map_err(|_| need().with("why", "a confirm works once, for this item, within 60 seconds"))?;
    match proof {
        k2_vault::HumanProof::Confirmed { method } => Ok(method),
        k2_vault::HumanProof::None => Err(need()),
    }
}

fn load_queued(id: &str) -> R<store::OutboxRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    expire_now(&conn)?;
    let r = store::outbox(&conn, id).map_err(internal)?.ok_or_else(|| ChatErr::new("not_found", 404, format!("no queued post '{id}'")))?;
    if r.status != "queued" {
        return Err(not_queued(&r));
    }
    Ok(r)
}

fn not_queued(r: &store::OutboxRow) -> ChatErr {
    if r.decided_by.as_deref() == Some("system:expired") {
        ChatErr::new("expired", 410, format!("{} expired (24 h) — the agent can post it again", r.id))
    } else {
        ChatErr::new("not_queued", 409, format!("{} is already {}", r.id, r.status))
    }
}

fn decide_err(id: &str, e: store::DecideErr) -> ChatErr {
    match e {
        store::DecideErr::NotFound => ChatErr::new("not_found", 404, format!("no queued post '{id}'")),
        store::DecideErr::NotQueued(s) => ChatErr::new("not_queued", 409, format!("{id} is already {s}")),
        store::DecideErr::Db(e) => internal(e),
    }
}

fn decided_hook(r: &store::OutboxRow) {
    emit_hook(
        k2_core::agent_hooks::HookEvent::SewChatApprovalDecided,
        json!({"outboxId": r.id, "status": r.status, "decidedKind": r.decided_kind}),
    );
}

fn check_note(note: Option<&str>) -> R<Option<String>> {
    match note.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if n.chars().count() > 500 => Err(usage("a note is at most 500 characters")),
        n => Ok(n.map(str::to_string)),
    }
}

/// Approve one item that `may_approve` already cleared: decide atomically,
/// then post it to Sew AS the agent with the row's stored ids.
fn approve_one(d: &Decider, method: Option<&str>, id: &str, note: Option<&str>) -> R<Value> {
    let sew = backend()?;
    let kind = match d {
        Decider::Person { .. } => store::DecidedKind::Person,
        Decider::Agent { .. } => store::DecidedKind::Agent,
    };
    let dec = store::Decision { status: store::OutboxStatus::Approved, by: d.label(), kind, method, note };
    let (row, body) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::take_for_approval(&conn, id, &dec, now()).map_err(|e| decide_err(id, e))?
    };
    let action = store::action_of(&row).map_err(internal)?;
    // Sew-Actor (G7): who approved, and how. An agent's is labelled as one.
    let actor = match (d, method) {
        (Decider::Person { label, .. }, Some(m)) => format!("person:{label} via {m}"),
        (Decider::Person { label, .. }, None) => format!("person:{label}"),
        (Decider::Agent { label, .. }, _) => label.clone(),
    };
    let a = ActAs {
        workspace_uuid: row.project_id.to_ascii_lowercase(),
        k2_session: row.k2_session.clone(),
        on_behalf_of: row.on_behalf_of.clone(),
        actor: Some(actor),
    };
    let idem = format!("k2c-{}", row.client_key);
    let res = match action {
        core::Action::Send | core::Action::Reply | core::Action::Dm => sew
            .create(
                &a,
                &NewMessage {
                    channel_id: row.channel_id.clone(),
                    body,
                    reply_to: row.target_id.clone().filter(|_| action == core::Action::Reply),
                    broadcast: row.broadcast,
                    idempotency_key: idem,
                },
            )
            .map(|m| m.id),
        core::Action::Edit => sew.edit(&a, row.target_id.as_deref().unwrap_or(""), &body, &idem).map(|m| m.id),
        core::Action::Delete => return Err(internal("a delete is never queued")),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let out = match res {
        Ok(mid) => store::set_delivery(&conn, &row.id, true, Some(&mid), None, None, now()).map_err(internal)?,
        Err(e) => {
            let r = store::set_delivery(&conn, &row.id, false, None, Some(&e.code), Some(i64::from(e.status)), now()).map_err(internal)?;
            drop(conn);
            decided_hook(&r);
            return Err(sew_err(e).with("id", r.id.clone()).with("status", "approved").with("delivery", "failed"));
        }
    };
    drop(conn);
    decided_hook(&out);
    Ok(json!({
        "ok": true,
        "id": out.id,
        "status": out.status,
        "delivery": out.delivery,
        "messageId": out.message_id.as_deref().map(sew::wire_msg_id),
        "decidedBy": out.decided_by,
        "decidedKind": out.decided_kind,
        "confirmMethod": out.confirm_method,
    }))
}

fn approve(who: &Who, body: &[u8]) -> R<Value> {
    let d = decider(who)?;
    let b: ApproveBody = parse_body(body)?;
    let note = check_note(b.note.as_deref())?;
    let id = b.id.trim().to_string();
    backend()?; // never burn a confirm when Sew is off
    let r = load_queued(&id)?;
    may_approve(&d, policy_now()?, &r)?;
    let method = match &d {
        Decider::Person { stamp, .. } => Some(take_confirm(stamp, b.confirm_token.as_deref(), core::CONFIRM_APPROVE, &id)?),
        Decider::Agent { .. } => {
            if b.confirm_token.is_some() {
                return Err(usage("an agent's approval takes no confirm token (it is recorded as an agent's)"));
            }
            None
        }
    };
    approve_one(&d, method.as_deref(), &id, note.as_deref())
}

fn approve_batch(who: &Who, body: &[u8]) -> R<Value> {
    let d = decider(who)?;
    let b: BatchBody = parse_body(body)?;
    let note = check_note(b.note.as_deref())?;
    let mut ids: Vec<String> = b.ids.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() || ids.len() > 50 {
        return Err(usage("approve 1–50 queued posts at once"));
    }
    backend()?;
    // Check everything first; any stale item changes nothing.
    let by = policy_now()?;
    for id in &ids {
        let r = load_queued(id).map_err(|e| e.with("batchChanged", true))?;
        may_approve(&d, by, &r)?;
    }
    let method = match &d {
        Decider::Person { stamp, .. } => {
            Some(take_confirm(stamp, b.confirm_token.as_deref(), core::CONFIRM_APPROVE_BATCH, &core::batch_hash(&ids))?)
        }
        Decider::Agent { .. } => {
            if b.confirm_token.is_some() {
                return Err(usage("an agent's approval takes no confirm token (it is recorded as an agent's)"));
            }
            None
        }
    };
    let mut results = Vec::new();
    for id in &ids {
        results.push(match approve_one(&d, method.as_deref(), id, note.as_deref()) {
            Ok(v) => v,
            Err(e) => json!({"ok": false, "id": id, "error": {"code": e.code, "hint": e.hint}}),
        });
    }
    Ok(json!({ "ok": true, "results": results }))
}

fn deny(who: &Who, body: &[u8]) -> R<Value> {
    let d = decider(who)?;
    let b: DenyBody = parse_body(body)?;
    let note = check_note(b.note.as_deref())?.ok_or_else(|| usage("deny needs a note (--note \"why\"); the agent sees it"))?;
    let id = b.id.trim().to_string();
    let r = load_queued(&id)?;
    if let Decider::Agent { project_id, .. } = &d {
        if &r.project_id == project_id {
            return Err(own_item());
        }
    }
    let kind = match d {
        Decider::Person { .. } => store::DecidedKind::Person,
        Decider::Agent { .. } => store::DecidedKind::Agent,
    };
    let dec = store::Decision { status: store::OutboxStatus::Denied, by: d.label(), kind, method: None, note: Some(&note) };
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::decide(&conn, &id, &dec, now()).map_err(|e| decide_err(&id, e))?
    };
    decided_hook(&row);
    Ok(json!({ "ok": true, "id": row.id, "status": row.status, "note": row.note, "decidedBy": row.decided_by, "decidedKind": row.decided_kind }))
}

fn policy(who: &Who) -> R<Value> {
    credential(who)?;
    let by = policy_now()?;
    let default_space = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::default_space(&conn).map_err(internal)?
    };
    let can_change = matches!(who, Who::Owner) || matches!(who, Who::Login { role, .. } if *role >= Role::Admin);
    Ok(json!({
        "ok": true,
        "approvalsBy": by.as_str(),
        "defaultSpace": default_space,
        "canChange": can_change,
        "allowed": ["people", "people+agents"],
    }))
}

/// Who is changing the per-box chat policy. `tighten` = the change only
/// narrows what agents may do (an agent with Manage Sew may make those).
fn policy_actor(who: &Who, tighten: bool, why: &str) -> R<(String, &'static str)> {
    match who {
        Who::Owner => Ok(("owner".to_string(), "owner_token")),
        Who::Login { username, role } if *role >= Role::Admin => Ok((format!("login:{username}"), "login")),
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. }
            if tighten && k2_core::workspace::settings::agents_can_manage_chat(project_id) =>
        {
            Ok((format!("agent:{}", crate::compute_routes::ws_handle(project_id)), "agent"))
        }
        Who::Agent { .. } | Who::Shell { .. } => Err(owner_only().with("why", why.to_string())),
        _ => Err(owner_only()),
    }
}

fn policy_audit(who: &Who, label: &str, kind: &str, op: &str, detail: String) -> R<()> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::insert_audit(
        &conn,
        &store::AuditRow {
            id: core::new_id("aud_"),
            project_id: who.passport().map(|(p, _)| p.to_string()),
            actor: label.to_string(),
            actor_kind: kind.into(),
            op: op.into(),
            detail: Some(detail),
            outcome: "ok".into(),
            created_at: now(),
            ..Default::default()
        },
    )
    .map_err(internal)
}

/// A Space slug for the default-Space setting: 2–64 of `a-z 0-9 -`.
fn check_space_slug(s: &str) -> R<()> {
    let ok = (2..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(usage(format!("defaultSpace is a Space slug (2–64 of a-z, 0-9, -) or none (got '{s}')")))
    }
}

fn policy_set(who: &Who, body: &[u8]) -> R<Value> {
    credential(who)?;
    let b: PolicyBody = parse_body(body)?;
    if b.approvals_by.is_none() && b.default_space.is_none() {
        return Err(usage("set approvalsBy and/or defaultSpace"));
    }
    let by = b.approvals_by.as_deref().map(ApprovalsBy::parse).transpose().map_err(usage)?;
    // The default Space governs DMs outside every Space: naming one widens
    // agents' DM reach (a person's call); clearing it only narrows.
    let space = match b.default_space.as_deref().map(|s| s.trim().to_ascii_lowercase()) {
        Some(s) if s.is_empty() || s == "none" => Some(None),
        Some(s) => {
            check_space_slug(&s)?;
            Some(Some(s))
        }
        None => None,
    };
    // Check every part before changing any.
    let by_actor = match by {
        Some(by) => Some((
            by,
            policy_actor(who, by == ApprovalsBy::People, "an agent never widens who may approve (it may set people with Manage Chat / Voice Hosting on)")?,
        )),
        None => None,
    };
    let space_actor = match &space {
        Some(sp) => Some((
            sp.clone(),
            policy_actor(who, sp.is_none(), "which Space governs DMs widens what agents may do there: a person sets it (an agent with Manage Chat / Voice Hosting may only clear it)")?,
        )),
        None => None,
    };
    if let Some((by, (label, kind))) = &by_actor {
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::set_approvals_by(&conn, *by, label, now()).map_err(internal)?;
        }
        policy_audit(who, label, kind, "approvals.by", by.as_str().into())?;
    }
    if let Some((sp, (label, kind))) = &space_actor {
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::set_default_space(&conn, sp.as_deref(), label, now()).map_err(internal)?;
        }
        policy_audit(who, label, kind, "default_space", sp.clone().unwrap_or_else(|| "none".into()))?;
    }
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"sewChat": "policy"}));
    let (now_by, now_space) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::approvals_by(&conn).map_err(internal)?, store::default_space(&conn).map_err(internal)?)
    };
    Ok(json!({ "ok": true, "approvalsBy": now_by.as_str(), "defaultSpace": now_space }))
}

#[cfg(test)]
pub(crate) mod test_seams {
    use std::cell::RefCell;
    use std::sync::Arc;

    thread_local! {
        static VAULT: RefCell<Option<Arc<k2_vault::LocalVault>>> = const { RefCell::new(None) };
    }

    pub(crate) fn set_vault(v: Option<Arc<k2_vault::LocalVault>>) {
        VAULT.with(|c| *c.borrow_mut() = v);
    }

    pub(super) fn vault() -> Option<Arc<k2_vault::LocalVault>> {
        VAULT.with(|c| c.borrow().clone())
    }
}
