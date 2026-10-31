//! Hosted mail forwards: the routes (prd-hostmail-external-forwarding-v1
//! §3.3, §4.1; vs-live FW-A7–FW-A10, FW-A12; Rosson's ruling 2026-10-30).
//!
//! ```text
//! GET  /cli/mail/forward?address=          show one mailbox's forward      owner, Admin, mail-manage agent
//! POST /cli/mail/forward {address,to,keep?} set (same box or outside)       owner, Admin, mail-manage agent
//! POST /cli/mail/forward/unset {address}    remove                          owner, Admin, mail-manage agent
//! POST /cli/mail/forward/pause {address}    pause (keep-only)               owner, Admin, mail-manage agent
//! POST /cli/mail/forward/resume {address}   resume a paused forward         owner, Admin; agents: own pauses only
//! GET  /cli/mail/forwards[?state=]          every forward on the box        owner, Admin
//! POST /cli/mail/address/person-link {address, person|null}
//!                                           which box login owns a mailbox  owner, Admin
//! GET|POST /cli/mail/self/forward[...]      the same verbs for the mailbox's own person (a box login linked
//!                                           to it, or the owner); never an agent
//! ```
//!
//! **The row is the truth** (`k2_core::mail_forward`); every change writes
//! the Sieve right away through `forward_sync` (and the hourly reconcile
//! repairs anything that drifted).
//!
//! **Same box vs outside.** A destination that is an active mailbox here
//! is a same-box forward (active at once, as before). Any other address
//! on one of this box's domains is refused. Anything else is an outside
//! forward: it starts `pending_verify` and K2 mails the destination a
//! confirm link (`forward_verify`); nothing is forwarded until its owner
//! presses Confirm. Outside forwards keep a local copy by default.
//!
//! **Who** (Rosson 2026-10-30): the owner, an Admin, the mailbox's person,
//! and any agent with mail-manage on ("Allow agents to manage hosted mail
//! on this host") can set, change and remove forwards, outside ones
//! included, with no per-forward owner OK. What still applies: the
//! destination's confirm link, the caps, the auto-pause, and the loop,
//! spam and DMARC stops. Resuming after an automatic pause (the
//! destination refusing, the box brake) is a person's call (owner, Admin,
//! the mailbox's person); a stopped destination (it pressed Stop) can't be
//! resumed at all, only set again (a new confirm).

use std::collections::HashMap;

use k2_core::mail_forward::{self as store, Forward};
use k2_core::db::schema::MailAddress;
use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;
use crate::email_notify::{self, fail, respond, Fail, R};
use crate::mail::forward_sync::{self, ChangeError};
use crate::mail::forward_verify::{self, Started};
use crate::mail::hostmail_auth::authorize_mailbox;

pub const FORWARD_PATH: &str = "/cli/mail/forward";
pub const UNSET_PATH: &str = "/cli/mail/forward/unset";
pub const PAUSE_PATH: &str = "/cli/mail/forward/pause";
pub const RESUME_PATH: &str = "/cli/mail/forward/resume";
pub const LIST_PATH: &str = "/cli/mail/forwards";
pub const PERSON_LINK_PATH: &str = "/cli/mail/address/person-link";
pub const SELF_PATH: &str = "/cli/mail/self/forward";
pub const SELF_UNSET_PATH: &str = "/cli/mail/self/forward/unset";
pub const SELF_PAUSE_PATH: &str = "/cli/mail/self/forward/pause";
pub const SELF_RESUME_PATH: &str = "/cli/mail/self/forward/resume";

/// The person routes (the dispatcher resolves the person from the token).
pub fn is_self_route(p: &str) -> bool {
    matches!(p, SELF_PATH | SELF_UNSET_PATH | SELF_PAUSE_PATH | SELF_RESUME_PATH)
}

/// How many audit rows `show` returns.
const EVENTS_SHOWN: usize = 50;

// ── the caller ──────────────────────────────────────────────────────

/// Who is asking (already through the route floors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The owner token or an Admin login.
    Owner,
    /// A mail-manage agent (the dispatcher refused the rest).
    Agent { workspace_uuid: String },
    /// A box login (or the owner, `owner`) acting on a mailbox linked to it.
    Person { key: String },
}

impl Caller {
    /// The request's caller on the `/cli/mail/*` admin routes.
    pub fn of_request() -> Caller {
        match crate::caller_workspace::request_principal() {
            Some(p) => Caller::Agent { workspace_uuid: p.workspace_uuid },
            None => Caller::Owner,
        }
    }

    /// The audit actor: `owner` | `agent:<workspace>` | `person:<key>`.
    pub fn actor(&self) -> String {
        match self {
            Caller::Owner => "owner".to_string(),
            Caller::Agent { workspace_uuid } => format!("agent:{}", agent_name(workspace_uuid)),
            Caller::Person { key } => format!("person:{key}"),
        }
    }

    /// Who asked, for the confirm mail (`owner` | a person key |
    /// `agent:<name>`).
    fn asker(&self) -> String {
        match self {
            Caller::Owner => k2_core::notify_prefs::OWNER.to_string(),
            Caller::Agent { workspace_uuid } => format!("agent:{}", agent_name(workspace_uuid)),
            Caller::Person { key } => key.clone(),
        }
    }

    /// May this caller resume a forward paused for `reason`?
    pub fn may_resume(&self, reason: &str) -> Result<(), Fail> {
        if reason == store::P_REVOKED {
            return Err(fail(
                409,
                "destination_stopped",
                "the destination pressed Stop in K2's email, so this forward can't be resumed; set it again to send a new confirm link",
            ));
        }
        if store::auto_lifts(reason) {
            return Err(fail(
                409,
                "held",
                format!("K2 paused this forward because {}; it resumes by itself when that changes", reason_words(reason)),
            ));
        }
        match self {
            Caller::Agent { .. } if reason != store::P_MANUAL => Err(fail(
                403,
                "owner_only",
                format!(
                    "K2 paused this forward because {}; the owner, an Admin or the mailbox's person resumes it (k2 hostmail forward resume)",
                    reason_words(reason)
                ),
            )),
            Caller::Person { .. } if reason == store::P_BOX_BRAKE => Err(fail(
                403,
                "owner_only",
                "every outside forward on this server is paused because too many were refused; the owner or an Admin resumes them",
            )),
            _ => Ok(()),
        }
    }
}

/// The agent's name (its workspace folder), never a path.
fn agent_name(workspace_uuid: &str) -> String {
    crate::workspace_msg::resolve_workspace(workspace_uuid)
        .and_then(|p| std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| workspace_uuid.chars().take(8).collect())
}

/// A pause reason in words.
pub fn reason_words(reason: &str) -> &'static str {
    match reason {
        store::P_DEST_GONE => "the destination says the address doesn't exist",
        store::P_REFUSED => "the destination refused too many forwards as spam or by policy",
        store::P_REVOKED => "the destination asked K2 to stop",
        store::P_DAILY_CAP => "it reached its daily limit",
        store::P_BOX_BRAKE => "too many forwards from this server were refused",
        store::P_RECEIVE_ONLY => "the mailbox's domain is receive-only",
        store::P_AIR_GAP => "air-gap is on",
        store::P_MANUAL => "someone paused it",
        _ => "of a rule",
    }
}

// ── helpers ─────────────────────────────────────────────────────────

fn body_json(b: &[u8]) -> R<J> {
    match serde_json::from_slice::<J>(b) {
        Ok(v @ J::Object(_)) => Ok(v),
        Ok(_) => Err(fail(400, "usage", "the body is a JSON object")),
        Err(e) => Err(fail(400, "usage", format!("invalid JSON body: {e}"))),
    }
}

fn s<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(J::as_str).map(str::trim).filter(|x| !x.is_empty())
}

fn keep_field(v: &J) -> R<Option<bool>> {
    match v.get("keep") {
        None | Some(J::Null) => Ok(None),
        Some(J::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(fail(400, "usage", "'keep' is true or false")),
    }
}

/// `CliResponse` refusals from `hostmail_auth` → [`Fail`] (codes kept).
fn from_cli(r: CliResponse) -> Fail {
    let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
    let code = v.pointer("/error/code").and_then(J::as_str).unwrap_or("failed");
    let hint = v.pointer("/error/hint").and_then(J::as_str).unwrap_or("refused").to_string();
    let status: u16 = r.status.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(500);
    let code: &'static str = match code {
        "usage" => "usage",
        "not_found" => "not_found",
        "forbidden" => "forbidden",
        "not_ready" => "not_ready",
        "engine" => "engine",
        "owner_only" => "owner_only",
        _ => "failed",
    };
    fail(status, code, hint)
}

fn change_fail(e: ChangeError) -> Fail {
    match e {
        ChangeError::Row(store::TransitionError::NotFound) => fail(404, "not_found", "no such forward"),
        ChangeError::Row(store::TransitionError::Illegal(from, to)) => {
            fail(409, "bad_state", format!("the forward is {from}; it can't become {to}"))
        }
        ChangeError::Row(store::TransitionError::Db(e)) => email_notify::db_fail(e),
        ChangeError::Sieve(_, e) => fail(502, "engine", format!("saved, but the mail server wasn't updated yet ({e}); K2 retries within the hour")),
    }
}

/// The change happened; a deferred Sieve write is reported, not failed.
fn changed(r: Result<Forward, ChangeError>) -> R<(Forward, Option<String>)> {
    match r {
        Ok(f) => Ok((f, None)),
        Err(ChangeError::Sieve(f, e)) => Ok((*f, Some(format!("saved; the mail server will be updated within the hour ({})", email_notify::send::mask_addresses(&e))))),
        Err(e) => Err(change_fail(e)),
    }
}

fn now() -> i64 {
    email_notify::now()
}

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> Result<T, String>) -> R<T> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    f(&conn).map_err(email_notify::db_fail)
}

/// Is `domain` one of this box's mail domains (or its mail host)?
fn hosted_domain(domain: &str) -> bool {
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if forward_sync::mail_host().is_some_and(|h| h == d) {
        return true;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM mail_domains WHERE lower(domain) = ?1", rusqlite::params![d], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)
        .unwrap_or(true) // fail closed: an unreadable store refuses outside forwards
}

/// The kind of destination `dest` is for `mailbox` (FW-A10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dest {
    Internal(String),
    External(String),
}

/// Classify and check a destination.
pub fn classify_dest(mailbox: &str, raw: &str) -> R<Dest> {
    let dest = email_notify::compose::strict_address(raw)
        .map_err(|_| fail(400, "usage", format!("'{}' isn't a valid email address", raw.chars().take(120).collect::<String>())))?;
    if dest.eq_ignore_ascii_case(mailbox) {
        return Err(fail(400, "usage", "a mailbox can't forward to itself"));
    }
    if crate::mail::hostmail_auth::require_active_mint(&dest).is_ok() {
        return Ok(Dest::Internal(dest));
    }
    let domain = dest.rsplit_once('@').map(|(_, d)| d).unwrap_or_default();
    if hosted_domain(domain) {
        return Err(fail(
            400,
            "usage",
            format!("'{dest}' is on one of this server's own domains but is not a minted mailbox on this host; forward to a mailbox here (or create it first)"),
        ));
    }
    if email_notify::fence::reserved(&dest) {
        return Err(fail(409, "sender_address", "that is one of this server's notification senders; give an address a person reads"));
    }
    Ok(Dest::External(dest))
}

// ── views ───────────────────────────────────────────────────────────

/// One forward as the CLI and Settings see it.
pub fn forward_json(f: &Forward, now: i64) -> J {
    let (verify, counts) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let verify = f
            .verified_address_id
            .as_deref()
            .and_then(|id| k2_core::email_verify::get(&conn, id).ok().flatten())
            .map(|v| {
                let expired = v.state == k2_core::email_verify::ST_PENDING && v.expires_at <= now;
                json!({
                    "state": if expired { k2_core::email_verify::ST_EXPIRED } else { v.state.as_str() },
                    "expiresAt": (v.state == k2_core::email_verify::ST_PENDING).then_some(v.expires_at),
                    "verifiedAt": v.verified_at,
                })
            });
        let counts = store::counts(&conn, &f.id, now - store::WINDOW_SECS).unwrap_or_default();
        (verify, counts)
    };
    json!({
        "id": f.id,
        "address": f.address,
        "to": f.dest,
        "kind": f.dest_kind,
        "keep": f.keep,
        "state": f.state,
        "pauseReason": f.pause_reason,
        "pauseWords": f.pause_reason.as_deref().map(reason_words),
        "setBy": f.set_by,
        "createdAt": f.created_at,
        "updatedAt": f.updated_at,
        "activatedAt": f.activated_at,
        "pausedAt": f.paused_at,
        "verify": verify,
        "last24h": {
            "forwarded": counts.queued,
            "delivered": counts.delivered,
            "refused": counts.refused(),
            "refusedNoSuchUser": counts.refused_user,
            "refusedPolicy": counts.refused_policy,
        },
    })
}

fn show(row: &MailAddress) -> R<J> {
    let now = now();
    let (live, events) = db(|c| Ok((store::live_for_address(c, &row.id)?, store::events_for_address(c, &row.id, EVENTS_SHOWN)?)))?;
    let fwd = live.as_ref().map(|f| forward_json(f, now));
    Ok(json!({
        "ok": true,
        "address": row.address,
        // The pre-0.46.7 shape, kept for older CLIs: only an active
        // forward is a forward.
        "to": live.as_ref().filter(|f| f.is_active()).map(|f| f.dest.clone()),
        "keep": live.as_ref().is_some_and(|f| f.is_active() && f.keep),
        "forward": fwd,
        "events": events,
    }))
}

// ── the verbs (shared by the admin and person routes) ───────────────

fn set(row: &MailAddress, caller: &Caller, v: &J) -> R<J> {
    let to = s(v, "to").ok_or_else(|| fail(400, "usage", "missing 'to': the destination address"))?;
    let keep = keep_field(v)?;
    let now = now();
    match classify_dest(&row.address, to)? {
        Dest::Internal(dest) => {
            let keep = keep.unwrap_or(false);
            let actor = caller.actor();
            let r = forward_sync::change_and_sync(&row.id, |c, now| {
                store::create(
                    c,
                    &store::NewForward {
                        address_id: &row.id,
                        address: &row.address,
                        dest: &dest,
                        dest_kind: store::KIND_INTERNAL,
                        keep,
                        set_by: &actor,
                        state: store::ST_ACTIVE,
                        verified_address_id: None,
                    },
                    now,
                )
                .map_err(store::TransitionError::Db)
            });
            let (f, note) = changed(r)?;
            Ok(json!({ "ok": true, "address": row.address, "to": f.dest, "keep": f.keep, "kind": f.dest_kind, "state": f.state, "note": note }))
        }
        Dest::External(dest) => set_external(row, caller, &dest, keep.unwrap_or(true), now),
    }
}

fn set_external(row: &MailAddress, caller: &Caller, dest: &str, keep: bool, now: i64) -> R<J> {
    if email_notify::airgap_blocks() {
        return Err(fail(409, "air_gap", "air-gap is on: nothing leaves this server, so it can't forward outside"));
    }
    if !forward_sync::outside_forward_supported() {
        return Err(fail(409, "upgrade_required", forward_sync::UPGRADE_HINT));
    }
    let mb = forward_sync::mailbox(&row.id).map_err(email_notify::db_fail)?.ok_or_else(|| fail(404, "not_found", "no such mailbox"))?;
    if mb.send_mode.as_deref() == Some("receive-only") {
        return Err(fail(
            409,
            "receive_only",
            format!("{}'s domain is receive-only, so its mail can't be forwarded outside (Settings → Email → Sending)", row.address),
        ));
    }
    let actor = caller.actor();
    // Same destination already live: only the keep setting can change.
    if let Some(f) = db(|c| store::live_for_address(c, &row.id))?.filter(|f| f.is_external() && f.dest == dest && f.state != store::ST_PENDING_VERIFY && f.pause_reason.as_deref() != Some(store::P_REVOKED)) {
        let r = forward_sync::change_and_sync(&row.id, |c, now| {
            store::set_keep(c, &f.id, keep, &actor, now).map_err(store::TransitionError::Db)?;
            store::get(c, &f.id).map_err(store::TransitionError::Db)?.ok_or(store::TransitionError::NotFound)
        });
        let (f, note) = changed(r)?;
        return Ok(json!({ "ok": true, "address": row.address, "to": f.dest, "keep": f.keep, "kind": f.dest_kind, "state": f.state, "note": note }));
    }
    let started = forward_verify::start(&row.address, row.stalwart_account_id.as_deref(), dest, &caller.asker(), now)?;
    let (state, sent_via) = match &started {
        Started::AlreadyVerified(_) => (store::ST_ACTIVE, None),
        Started::Sent { via, .. } => (store::ST_PENDING_VERIFY, Some(*via)),
    };
    let verified_id = started.verified_row().id.clone();
    let r = forward_sync::change_and_sync(&row.id, |c, now| {
        let f = store::create(
            c,
            &store::NewForward {
                address_id: &row.id,
                address: &row.address,
                dest,
                dest_kind: store::KIND_EXTERNAL,
                keep,
                set_by: &actor,
                state,
                verified_address_id: Some(&verified_id),
            },
            now,
        )
        .map_err(store::TransitionError::Db)?;
        if let Some(via) = sent_via {
            store::record_event(c, &f.id, store::E_VERIFY_SENT, &actor, &json!({ "via": via }), now).map_err(store::TransitionError::Db)?;
        }
        Ok(f)
    });
    let (f, note) = changed(r)?;
    let pending = f.state == store::ST_PENDING_VERIFY;
    if !pending {
        forward_verify::notice_activated(&f);
    }
    Ok(json!({
        "ok": true,
        "accepted": pending,
        "address": row.address,
        "to": f.dest,
        "keep": f.keep,
        "kind": f.dest_kind,
        "state": f.state,
        "sentVia": sent_via,
        "message": if pending {
            format!("We emailed {} a confirm link. Forwarding starts when they click it (the link works for 24 hours).", f.dest)
        } else {
            format!("{} already confirmed it wants this mail; forwarding is on.", f.dest)
        },
        "note": note,
    }))
}

fn live_or_404(row: &MailAddress) -> R<Forward> {
    db(|c| store::live_for_address(c, &row.id))?.ok_or_else(|| fail(404, "not_found", format!("{} has no forward", row.address)))
}

fn unset(row: &MailAddress, caller: &Caller) -> R<J> {
    let f = match db(|c| store::live_for_address(c, &row.id))? {
        Some(f) => f,
        // Nothing to remove: still make sure no forward block is left.
        None => {
            let note = forward_sync::sync_mailbox(&row.id, forward_sync::Why::Change).err().map(|e| email_notify::send::mask_addresses(&e));
            return Ok(json!({ "ok": true, "address": row.address, "to": J::Null, "removed": false, "note": note }));
        }
    };
    let actor = caller.actor();
    let (f, note) = changed(forward_sync::transition_and_sync(&f.id, store::ST_REMOVED, &store::Change { actor: &actor, ..Default::default() }))?;
    Ok(json!({ "ok": true, "address": row.address, "to": J::Null, "removed": true, "id": f.id, "note": note }))
}

fn pause(row: &MailAddress, caller: &Caller) -> R<J> {
    let f = live_or_404(row)?;
    if f.state == store::ST_PAUSED {
        return Ok(json!({ "ok": true, "address": row.address, "state": f.state, "pauseReason": f.pause_reason }));
    }
    let (f, note) = changed(forward_sync::pause(&f.id, store::P_MANUAL, &caller.actor(), None))?;
    Ok(json!({ "ok": true, "address": row.address, "state": f.state, "pauseReason": f.pause_reason, "note": note }))
}

fn resume(row: &MailAddress, caller: &Caller) -> R<J> {
    let f = live_or_404(row)?;
    if f.state != store::ST_PAUSED {
        return Err(fail(409, "bad_state", format!("the forward is {}, not paused", f.state)));
    }
    caller.may_resume(f.pause_reason.as_deref().unwrap_or(""))?;
    let (f, note) = changed(forward_sync::resume(&f.id, &caller.actor()))?;
    Ok(json!({ "ok": true, "address": row.address, "state": f.state, "pauseReason": f.pause_reason, "note": note }))
}

// ── the admin routes (/cli/mail/forward*) ───────────────────────────

fn mailbox_from(v: &J) -> R<MailAddress> {
    let a = s(v, "address").ok_or_else(|| fail(400, "usage", "missing 'address': which hosted mailbox?"))?;
    authorize_mailbox(a).map_err(from_cli)
}

/// GET `/cli/mail/forward?address=`.
pub fn handle_forward_get(params: &HashMap<String, String>) -> CliResponse {
    let address = crate::cli::str_param(params, "address");
    respond(mailbox_from(&json!({ "address": address })).and_then(|row| show(&row)))
}

/// POST `/cli/mail/forward` `{address, to, keep?}`.
pub fn handle_forward_set(body: &[u8]) -> CliResponse {
    respond(body_json(body).and_then(|v| {
        if s(&v, "to").is_none() {
            return Err(fail(400, "usage", "missing 'to': the destination (a mailbox here, or an outside address)"));
        }
        let row = mailbox_from(&v)?;
        set(&row, &Caller::of_request(), &v)
    }))
}

/// POST `/cli/mail/forward/unset` `{address}`.
pub fn handle_forward_unset(body: &[u8]) -> CliResponse {
    respond(body_json(body).and_then(|v| unset(&mailbox_from(&v)?, &Caller::of_request())))
}

/// POST `/cli/mail/forward/pause` `{address}`.
pub fn handle_forward_pause(body: &[u8]) -> CliResponse {
    respond(body_json(body).and_then(|v| pause(&mailbox_from(&v)?, &Caller::of_request())))
}

/// POST `/cli/mail/forward/resume` `{address}`.
pub fn handle_forward_resume(body: &[u8]) -> CliResponse {
    respond(body_json(body).and_then(|v| resume(&mailbox_from(&v)?, &Caller::of_request())))
}

/// GET `/cli/mail/forwards[?state=paused|active|pending_verify][&kind=external]`
/// — owner and Admins only.
pub fn handle_forwards_list(params: &HashMap<String, String>) -> CliResponse {
    if crate::caller_workspace::request_principal().is_some() {
        return email_notify::fail_response(&fail(403, "owner_only", "the list of every forward on this server is for the owner and Admins"));
    }
    let state = params.get("state").map(|s| s.trim()).filter(|s| !s.is_empty());
    if let Some(st) = state {
        if ![store::ST_PENDING_VERIFY, store::ST_ACTIVE, store::ST_PAUSED].contains(&st) {
            return email_notify::fail_response(&fail(400, "usage", "state is pending_verify, active or paused"));
        }
    }
    let external_only = params.get("kind").is_some_and(|k| k.trim() == store::KIND_EXTERNAL);
    let now = now();
    respond(
        db(|c| store::list(c, &store::ListFilter { state, external_only, include_removed: false }))
            .map(|rows| json!({ "ok": true, "forwards": rows.iter().map(|f| forward_json(f, now)).collect::<Vec<_>>() })),
    )
}

/// POST `/cli/mail/address/person-link` `{address, person: "<login>"|"owner"|null}`
/// — owner and Admins only (FW-A8). Linking also marks the mailbox a
/// person's (`person_mailbox`).
pub fn handle_person_link(body: &[u8]) -> CliResponse {
    if crate::caller_workspace::request_principal().is_some() {
        return email_notify::fail_response(&fail(403, "owner_only", "only the owner or an Admin links a mailbox to a person"));
    }
    respond(body_json(body).and_then(|v| {
        let row = mailbox_from(&v)?;
        let person = match v.get("person") {
            Some(J::Null) => None,
            Some(J::String(p)) if !p.trim().is_empty() => {
                let p = p.trim();
                if p == k2_core::notify_prefs::OWNER {
                    Some(p.to_string())
                } else {
                    if k2_core::connect_users::role_for_user(p).is_none() {
                        return Err(fail(404, "not_found", format!("no login '{p}' on this server (k2 users list)")));
                    }
                    // The person key (a login the owner linked is the owner).
                    let people = k2_core::notify_prefs::People::load().map_err(email_notify::db_fail)?;
                    Some(people.login_person_key(p).ok_or_else(|| fail(409, "reserved_login", format!("'{p}' can't own a mailbox")))?)
                }
            }
            _ => return Err(fail(400, "usage", "send {address, person: \"<login>\"} to link, or person: null to unlink")),
        };
        db(|c| store::set_person_principal(c, &row.id, person.as_deref()))?;
        if person.is_some() {
            crate::mail::agent_creds::set_person_flag(&row.id, true).map_err(email_notify::db_fail)?;
        }
        Ok(json!({ "ok": true, "address": row.address, "person": person }))
    }))
}

// ── the person routes (/cli/mail/self/forward*) ─────────────────────

/// The mailbox `person` may act on: `address` if linked to them, else
/// their only linked mailbox.
fn person_mailbox(person: &str, address: Option<&str>) -> R<MailAddress> {
    let mine = db(|c| store::mailboxes_of_person(c, person))?;
    let pick = match address.map(|a| a.trim().to_ascii_lowercase()) {
        Some(a) => mine.iter().find(|(_, addr)| *addr == a).ok_or_else(|| {
            fail(403, "forbidden", format!("{a} isn't linked to you; the owner or an Admin links mailboxes (k2 hostmail person link)"))
        })?,
        None => match mine.as_slice() {
            [one] => one,
            [] => {
                return Err(fail(
                    404,
                    "not_found",
                    "no mailbox is linked to you; the owner or an Admin links one (k2 hostmail person link <address> <login>)",
                ))
            }
            _ => return Err(fail(400, "usage", "more than one mailbox is linked to you: say which with 'address'")),
        },
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    crate::mail::addresses::load_address(&conn, &pick.1)
        .filter(|r| r.status == "active")
        .ok_or_else(|| fail(404, "not_found", "that mailbox is gone"))
}

/// `/cli/mail/self/forward*` for `person` (resolved from the token by the
/// dispatcher: `owner`, or a login's person key). Never an agent.
pub fn self_route(is_post: bool, path: &str, body: &[u8], person: &str, params: &HashMap<String, String>) -> CliResponse {
    if path != SELF_PATH && !is_post {
        return CliResponse::method_not_allowed();
    }
    let caller = Caller::Person { key: person.to_string() };
    if !is_post {
        let address = params.get("address").map(String::as_str).filter(|a| !a.trim().is_empty());
        return respond(person_mailbox(person, address).and_then(|row| show(&row)));
    }
    respond(body_json(body).and_then(|v| {
        let row = person_mailbox(person, s(&v, "address"))?;
        match path {
            SELF_PATH => set(&row, &caller, &v),
            SELF_UNSET_PATH => unset(&row, &caller),
            SELF_PAUSE_PATH => pause(&row, &caller),
            SELF_RESUME_PATH => resume(&row, &caller),
            _ => Err(fail(404, "not_found", "no such route")),
        }
    }))
}

#[cfg(test)]
mod tests;
