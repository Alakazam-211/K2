//! App user invites and set-password links (prd-agent-app-users-v1 §5.4,
//! S4/S5, Rosson 2026-10-24).
//!
//! - `POST /cli/skin/users/invite` (owner, or an agent with "manage Apps"
//!   for its own workspace's Apps): mail a person a one-time link to set
//!   their own password, creating them App-only when they're new.
//! - `POST /cli/skin/users/reset-link` (owner only): a reset link that, once
//!   used, drops the person's agent tie (`owner_reset`). Mailed to the
//!   stored email by default; `to` mails another address (which becomes
//!   their email on use); `code` hands the link to the owner instead.
//! - `POST /cli/skin/access/invites` (owner, or the App's own agent with
//!   "…manage them end to end"): the App's link lives (`inviteTtl`,
//!   `resetTtl`), its set-password page and its own subject and body.
//! - An App's forgot-password (`password.forgot` with its pass) mails a
//!   `reset` link the same way ([`forgot_by_pass`]); the pass never sees it.
//!
//! Mail goes through the box's own hosted mail (Stalwart) only, from the
//! address [`invite_sender`] picks, under the sending workspace's
//! governance for an agent or an App (off → refused, approval → held, the
//! link's clock starting when it leaves) and as the owner's own send for
//! the owner. No mail path: an agent is refused and never given a link;
//! the owner gets the link back once. No password ever goes in a mail.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use k2_core::skin::{self, invites};

use crate::cli_response::CliResponse;
use crate::mail::send::{self, DbOutboundStore, Gate, OutboundMessage, OutboundRequest, SendError, SendOutcome, SubmitBackend};

pub const INVITE_ROUTE: &str = "/cli/skin/users/invite";
pub const RESET_LINK_ROUTE: &str = "/cli/skin/users/reset-link";
pub const SETTINGS_ROUTE: &str = "/cli/skin/access/invites";

// ── the hosted submit seam (tests: no real mail) ─────────────────────

type Backend = Arc<dyn SubmitBackend + Send + Sync>;

fn backend_slot() -> &'static Mutex<Option<Backend>> {
    static SLOT: std::sync::OnceLock<Mutex<Option<Backend>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Replace the hosted mail submit for this process (tests drive invites
/// end to end without a mail server; production never sets it). Also
/// used by the approvals queue for hosted mail when set.
pub fn set_backend_override(b: Option<Backend>) {
    *backend_slot().lock().unwrap_or_else(|e| e.into_inner()) = b;
}

/// The override, if a test set one.
pub fn backend_override() -> Option<Backend> {
    backend_slot().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

// ── answers ──────────────────────────────────────────────────────────

fn refusal(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

fn method_not_allowed() -> CliResponse {
    refusal("405 Method Not Allowed", "method_not_allowed", "POST required")
}

fn usage(hint: &str) -> CliResponse {
    refusal("400 Bad Request", "usage", hint)
}

fn owner_only(hint: &str) -> CliResponse {
    refusal("403 Forbidden", "owner_only", hint)
}

fn send_error(e: SendError) -> CliResponse {
    match e {
        SendError::Usage(h) => refusal("400 Bad Request", "usage", &h),
        SendError::NotFound(h) => refusal("404 Not Found", "not_found", &h),
        SendError::Gated(h) => refusal("403 Forbidden", "gated", &h),
        SendError::SendMode(h) => refusal("409 Conflict", "send_mode", &h),
        SendError::Guardrail(h) => refusal("409 Conflict", "guardrail", &h),
        SendError::Conflict(h) => refusal("409 Conflict", "conflict", &h),
        SendError::RateLimited { window, hint } => CliResponse {
            status: "429 Too Many Requests",
            content_type: "application/json",
            body: json!({ "ok": false, "error": { "code": "rate_limited", "hint": hint, "window": window } }).to_string(),
        },
        SendError::Engine(h) => refusal("502 Bad Gateway", "engine", &h),
    }
}

pub const NO_MAIL_HINT: &str = "Can't send the invite: this workspace has no address it can send from (k2 hostmail create). K2 never gives an invite link to an agent.";

fn gated_hint() -> String {
    "can't send the invite: outbound email is off for this workspace. Your human can turn it on (k2 mail sending <ws> on) or send it themselves (k2 app user invite <u> --app <app>).".to_string()
}

fn no_page_hint(app_label: &str) -> String {
    format!(
        "{app_label} has no set-password page: set one with k2 app login settings {app_label} --set-password-url <url> (or turn on the box front door, whose /reset page works for every App)"
    )
}

fn json_body(body: &[u8]) -> Result<Value, CliResponse> {
    serde_json::from_slice::<Value>(body).ok().filter(Value::is_object).ok_or_else(|| usage("the body must be a JSON object"))
}

fn text<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str)).map(str::trim).filter(|s| !s.is_empty())
}

// ── who, from where ──────────────────────────────────────────────────

/// Who asked.
#[derive(Debug, Clone)]
pub enum By {
    /// The owner (owner token, Owner login, a K2 shell tab).
    Owner { actor: String },
    /// An agent of `ws` that passed the "manage Apps" gate.
    Agent { ws: String, actor: String },
    /// An App's platform pass (forgot), for workspace `ws`.
    App { ws: String, actor: String },
}

impl By {
    fn actor(&self) -> &str {
        match self {
            By::Owner { actor } | By::Agent { actor, .. } | By::App { actor, .. } => actor,
        }
    }
    fn is_owner(&self) -> bool {
        matches!(self, By::Owner { .. })
    }
}

/// Where a mail comes from.
#[derive(Debug, Clone)]
pub struct Sender {
    pub project_id: String,
    pub project_path: String,
    pub address: String,
    pub account_id: String,
    pub from_name: String,
    /// The governance this send runs under.
    pub gate: Gate,
}

fn project_path(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![project_id], |r| r.get::<_, String>(0)).ok()
}

/// The ONE place that decides who an invite or reset comes from (Rosson
/// 2026-10-24: the sender will most likely become the App's own domain on
/// the box's mail; that design is a follow-up, and changes only here).
///
/// Today: the box's hosted mail (Stalwart) only, never a linked inbox.
/// - An agent or an App: the sending workspace's one sendable hosted
///   address, under that workspace's governance. It can't pick another.
/// - The owner: `from` (a hosted address of the App's workspace), else
///   that workspace's address, sent as the owner's own (`Gate::On`).
///
/// `Ok(None)`: no address to send from. `Err`: a refusal to show.
pub fn invite_sender(app_ws: Option<&str>, by: &By, from: Option<&str>) -> Result<Option<Sender>, CliResponse> {
    if from.is_some() && !by.is_owner() {
        return Err(usage("an agent's invites come from its workspace's own address; only the owner picks another (from)"));
    }
    let Some(ws) = app_ws else {
        return Ok(None);
    };
    let Some(path) = project_path(ws) else { return Ok(None) };
    let address = match from {
        Some(addr) => match crate::mail::access::can_send(ws, addr) {
            Ok(inbox) if inbox.source == crate::mail::access::Source::Hosted => inbox,
            Ok(_) => return Err(usage("invites leave through this box's own mail (a hosted address), never a linked inbox")),
            Err(_) => return Err(usage("from must be a hosted address the App's workspace sends from (k2 mail inboxes)")),
        },
        None => {
            let all = crate::mail::access::sendable_hosted(ws);
            match all.len() {
                0 => return Ok(None),
                1 => match crate::mail::access::can_send(ws, &all[0].0) {
                    Ok(i) => i,
                    Err(_) => return Ok(None),
                },
                n => {
                    return Err(refusal(
                        "409 Conflict",
                        "several_senders",
                        &format!(
                            "this workspace can send from {n} addresses; the owner picks one (k2 app user invite … --from <address>)"
                        ),
                    ))
                }
            }
        }
    };
    let Some(account_id) = address.account_id.clone() else { return Ok(None) };
    let gate = match by {
        By::Owner { .. } => Gate::On,
        _ => send::gate_for_path(&path).map_err(send_error)?,
    };
    let from_name = match by {
        By::Owner { .. } => box_label(),
        _ => k2_core::workspace::display::agent_display_name(&path),
    };
    Ok(Some(Sender { project_id: ws.to_string(), project_path: path, address: address.address, account_id, from_name, gate }))
}

/// How the owner is named in a mail: the box's front door host, else
/// "this K2 server".
fn box_label() -> String {
    skin::effective_front_door()
        .ok()
        .and_then(|d| d.url)
        .and_then(|u| u.split("://").nth(1).map(|h| h.split('/').next().unwrap_or(h).to_string()))
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "this K2 server".to_string())
}

/// The App's set-password page: its own `setPasswordUrl`, else the box
/// front door's `/reset`, else none.
pub fn set_password_page(settings: &invites::InviteSettings) -> Option<String> {
    if let Some(u) = &settings.set_password_url {
        return Some(u.clone());
    }
    skin::effective_front_door()
        .ok()
        .and_then(|d| d.url)
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
        .map(|u| format!("{u}/reset"))
}

/// The App's name for mails (`host`: the box).
fn app_label(app: &str) -> String {
    if app == skin::HOST_APP_ID {
        return box_label();
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::get_by_id(&conn, app).ok().flatten().map(|s| s.name).unwrap_or_else(|| app.to_string())
}

/// `72 hours`, `3 days`, `15 minutes`.
pub fn ttl_words(secs: i64) -> String {
    let plural = |n: i64, w: &str| if n == 1 { format!("1 {w}") } else { format!("{n} {w}s") };
    if secs % 86_400 == 0 && secs >= 4 * 86_400 {
        plural(secs / 86_400, "day")
    } else if secs % 3600 == 0 {
        plural(secs / 3600, "hour")
    } else {
        plural((secs / 60).max(1), "minute")
    }
}

// ── one send ─────────────────────────────────────────────────────────

/// How a link reached the person.
#[derive(Debug)]
pub enum Delivered {
    /// The mail left; the link expires at this time.
    Sent { outbound_id: String, expires_at: i64 },
    /// The mail waits for the owner's approval; the clock starts when it leaves.
    Held { outbound_id: String },
}

struct Mail<'a> {
    purpose: &'a str,
    app: &'a str,
    to: &'a str,
    person: &'a skin::SkinPrincipal,
    page: &'a str,
    sender: &'a Sender,
    by: &'a By,
    settings: &'a invites::InviteSettings,
}

/// Compose and dispatch the mail carrying link `minted`. Any failure
/// drops the link (a mail that never left leaves nothing usable behind).
fn dispatch(m: &Mail<'_>, minted: &invites::Minted) -> Result<Delivered, CliResponse> {
    let fail = |r: CliResponse| {
        if let Err(e) = invites::remove(&minted.id) {
            k2_core::log_debug!("[invites] drop {} after a failed send: {e}", minted.id);
        }
        r
    };
    let app_name = app_label(m.app);
    let name = m.person.full_name.clone().unwrap_or_else(|| m.person.username.clone());
    let sender_label = match m.by {
        By::Owner { .. } => format!("the owner of {}", box_label()),
        By::Agent { .. } | By::App { .. } => format!("the {} agent", m.sender.from_name),
    };
    let expires = format!("{} after it was sent", ttl_words(minted.ttl_secs));
    let vars = invites::MailVars { app: &app_name, name: &name, username: &m.person.username, sender: &sender_label, expires: &expires };
    let tpl = if m.purpose == invites::PURPOSE_INVITE { &m.settings.invite } else { &m.settings.reset };
    let composed = invites::compose(m.purpose, tpl, &vars, &invites::link_for(m.page, &minted.token));
    let to = match send::normalize_recipients(&[m.to.to_string()]) {
        Ok(t) => t,
        Err(e) => return Err(fail(send_error(e))),
    };
    let msg = OutboundMessage {
        from_name: Some(m.sender.from_name.clone()),
        from: m.sender.address.clone(),
        to,
        cc: Vec::new(),
        bcc: Vec::new(),
        policy_bcc: Vec::new(),
        subject: composed.subject,
        text_body: composed.body,
        in_reply_to: None,
        references: None,
        reply_source: None,
        attachments: Vec::new(),
    };
    if let Err(e) = send::check_domain_send_mode(&crate::mail::secrets::FileSecretStore::default(), &m.sender.address) {
        return Err(fail(send_error(e)));
    }
    let engine_client;
    let override_backend = backend_override();
    let backend: Option<&dyn SubmitBackend> = match (m.sender.gate, &override_backend) {
        (Gate::On, Some(b)) => Some(&**b as &dyn SubmitBackend),
        (Gate::On, None) => match crate::mail::domains::engine_from_db() {
            Ok((client, _)) => {
                engine_client = client;
                Some(&engine_client)
            }
            Err(hint) => return Err(fail(refusal("503 Service Unavailable", "not_ready", &hint))),
        },
        _ => None,
    };
    let agent_name = match m.by {
        By::Owner { actor } => actor.clone(),
        _ => m.sender.from_name.clone(),
    };
    let req = OutboundRequest { project_id: &m.sender.project_id, agent_name: &agent_name, account_id: &m.sender.account_id, message: &msg, send_after: None };
    let store = DbOutboundStore::default();
    let now = k2_core::deploy::now();
    match send::gate_and_dispatch(&store, backend, m.sender.gate, &req, now) {
        Ok(SendOutcome::Submitted { id }) => match invites::activate(&minted.id) {
            Ok(Some(exp)) => Ok(Delivered::Sent { outbound_id: id, expires_at: exp }),
            Ok(None) => Err(CliResponse::internal_error("the link vanished while its mail was sent")),
            Err(e) => Err(CliResponse::internal_error(e)),
        },
        Ok(SendOutcome::Queued { id, .. }) => match invites::hold_for(&minted.id, &id) {
            Ok(()) => Ok(Delivered::Held { outbound_id: id }),
            Err(e) => Err(fail(CliResponse::internal_error(e))),
        },
        Ok(SendOutcome::Scheduled { .. }) => Err(fail(CliResponse::internal_error("an invite is never scheduled"))),
        Ok(SendOutcome::SubmitFailed { error, .. }) => Err(fail(refusal("502 Bad Gateway", "engine", &format!("the mail server didn't take the mail: {error}")))),
        Err(e) => Err(fail(send_error(e))),
    }
}

/// The approvals queue sent a held mail: start the clock of the link it
/// carries (no-op for any other mail).
pub fn outbound_sent(outbound_id: &str) {
    match invites::activate_for_outbound(outbound_id) {
        Ok(Some(exp)) => k2_core::log_debug!("[invites] held mail {outbound_id} left; its link expires at {exp}"),
        Ok(None) => {}
        Err(e) => k2_core::log_debug!("[invites] activate for {outbound_id}: {e}"),
    }
}

fn audit(event: &str, actor: &str, line: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, actor, line, "-", "-", "cli"));
}

fn principal(username: &str) -> Result<Option<skin::SkinPrincipal>, CliResponse> {
    let Some(u) = skin::login_principal_username(username).map_err(CliResponse::internal_error)? else { return Ok(None) };
    skin::list_principals().map(|ps| ps.into_iter().find(|p| p.username == u)).map_err(CliResponse::internal_error)
}

fn holds_app(principal_id: &str, app: &str) -> Result<bool, CliResponse> {
    Ok(skin::list_grants()
        .map_err(CliResponse::internal_error)?
        .iter()
        .any(|g| g.subject_kind == "principal" && g.subject_id == principal_id && g.kind == "app" && g.target_id == app))
}

// ── POST /cli/skin/users/invite ──────────────────────────────────────

/// `POST /cli/skin/users/invite {username, app, email?, role?, from?}`.
/// `agent_ws`: the agent's workspace once the "manage Apps" gate passed.
pub fn handle_invite(body: &[u8], actor: &str, agent_ws: Option<&str>, is_post: bool) -> CliResponse {
    if !is_post {
        return method_not_allowed();
    }
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = text(&v, &["username", "name"]) else { return usage("missing username") };
    let Some(raw_app) = text(&v, &["app", "appId"]) else { return usage("missing app: invites are for one App (k2 app user invite <u> --app <app>)") };
    let by = match agent_ws {
        Some(ws) => By::Agent { ws: ws.to_string(), actor: actor.to_string() },
        None if crate::skin_routes::actor_is_agent(actor) => return owner_only("an agent invites through its workspace's \"manage Apps\" switch"),
        None => By::Owner { actor: actor.to_string() },
    };
    let app = match crate::skin_routes::resolve_app_target(raw_app) {
        Ok(a) => a,
        Err(e) => return usage(&e),
    };
    let app_ws = skin::ties::app_workspace(&app);
    if let By::Agent { ws, .. } = &by {
        if app == skin::HOST_APP_ID || app_ws.as_deref() != Some(ws.as_str()) {
            return owner_only(&format!("'{raw_app}' isn't an App of your workspace: only the owner invites people to it (ask your human)."));
        }
    }
    // The person: an existing one must be invitable by this caller and
    // have an email; a new one needs an email.
    let existing = match principal(username) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let email = match &existing {
        Some(p) => {
            if let By::Agent { ws, .. } = &by {
                let roles = match skin::list_roles() {
                    Ok(r) => r,
                    Err(e) => return CliResponse::internal_error(e),
                };
                let grants = match skin::list_grants() {
                    Ok(g) => g,
                    Err(e) => return CliResponse::internal_error(e),
                };
                let test_guest = skin::test_guests::get(&p.id).ok().flatten().is_some();
                let in_ws = crate::skin_routes::principal_in_workspace(p, &roles, &grants, ws);
                let on_app = grants.iter().any(|g| g.subject_kind == "principal" && g.subject_id == p.id && g.kind == "app" && g.target_id == app);
                if test_guest || !in_ws || !on_app {
                    return owner_only(&format!(
                        "'{}' isn't your workspace's App user on that App: only the owner invites them (ask your human).",
                        p.username
                    ));
                }
            } else if !matches!(holds_app(&p.id, &app), Ok(true)) {
                return refusal("409 Conflict", "no_access", &format!("'{}' has no access to that App yet: add it first (k2 app user rooms {} --app {raw_app} …)", p.username, p.username));
            }
            match p.email.clone() {
                Some(e) => e,
                None => {
                    return refusal(
                        "409 Conflict",
                        "no_email",
                        &format!("'{}' has no email: set one with k2 app user email {} --email <e>, then invite.", p.username, p.username),
                    )
                }
            }
        }
        None => match text(&v, &["email"]) {
            Some(e) => match skin::normalize_email(e) {
                Ok(Some(e)) => e,
                _ => return usage("invalid email"),
            },
            None => return usage("an invite needs an email (the person is new)"),
        },
    };
    let settings = match invites::settings(&app) {
        Ok(s) => s,
        Err(e) => return CliResponse::internal_error(e),
    };
    let Some(page) = set_password_page(&settings) else {
        return refusal("409 Conflict", "no_set_password_page", &no_page_hint(raw_app));
    };
    let sender = match invite_sender(app_ws.as_deref(), &by, text(&v, &["from"])) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Some(s) = &sender {
        if s.gate == Gate::Off {
            return refusal("403 Forbidden", "gated", &gated_hint());
        }
    }
    if sender.is_none() && !by.is_owner() {
        return refusal("409 Conflict", "no_mail", NO_MAIL_HINT);
    }
    // Create a new person now (App-only; stamped and tied for an agent).
    let person = match existing {
        Some(p) => p,
        None => {
            let role = text(&v, &["role"]);
            if let By::Agent { ws, .. } = &by {
                // The same checks as an agent's plain add (§5.3).
                let add = json!({ "username": username, "app": app, "role": role, "email": email });
                let r = crate::skin_routes::handle_users_post_as(Some(ws), add.to_string().as_bytes(), actor);
                if !r.status.starts_with("200") {
                    return r;
                }
            } else {
                let created = if app == skin::HOST_APP_ID {
                    skin::add_principal_with_email(username, Some(&email))
                } else {
                    skin::add_principal_for_app(username, Some(&email), &app, role, actor)
                };
                if let Err(e) = created {
                    return usage(&e);
                }
            }
            match principal(username) {
                Ok(Some(p)) => p,
                Ok(None) => return CliResponse::internal_error("the new person vanished"),
                Err(r) => return r,
            }
        }
    };
    if let By::Agent { ws, actor } = &by {
        let t = skin::ties::TieWrite { workspace_id: ws, set_by: actor, reason: skin::ties::REASON_INVITE };
        if let Err(e) = skin::ties::set(&person.id, t) {
            return CliResponse::internal_error(e);
        }
    }
    let minted = match invites::mint(&person.id, &app, invites::PURPOSE_INVITE, settings.invite_ttl_secs, sender.as_ref().map(|_| email.as_str()), false, by.actor()) {
        Ok(m) => m,
        Err(invites::MintError::Throttled) => {
            return refusal("429 Too Many Requests", "throttled", "one invite per person a minute: wait, then send it again")
        }
        Err(invites::MintError::Store(e)) => return CliResponse::internal_error(e),
    };
    let Some(sender) = sender else {
        // The owner with no mail path: the link, once.
        return returned_link(&minted, &page, &person, &app, by.actor(), "invite");
    };
    let mail = Mail { purpose: invites::PURPOSE_INVITE, app: &app, to: &email, person: &person, page: &page, sender: &sender, by: &by, settings: &settings };
    match dispatch(&mail, &minted) {
        Ok(d) => {
            audit("app_user_invite", by.actor(), &format!("{} app={app} to={email}", person.username));
            delivered_json(d, &person, &app, &email, &sender, minted.ttl_secs, "Invite")
        }
        Err(r) => r,
    }
}

fn delivered_json(d: Delivered, person: &skin::SkinPrincipal, app: &str, to: &str, sender: &Sender, ttl: i64, what: &str) -> CliResponse {
    let mut v = json!({
        "ok": true,
        "username": person.username,
        "app": app,
        "sentTo": to,
        "from": sender.address,
        "ttlSecs": ttl,
    });
    match d {
        Delivered::Sent { outbound_id, expires_at } => {
            v["delivered"] = json!("sent");
            v["outboundId"] = json!(outbound_id);
            v["expiresAt"] = json!(expires_at);
        }
        Delivered::Held { outbound_id } => {
            v["delivered"] = json!("held");
            v["outboundId"] = json!(outbound_id);
            v["expiresAt"] = Value::Null;
            v["hint"] = json!(format!(
                "{what} to {to} is waiting for your human's approval. The link's {} start when it's sent.",
                ttl_words(ttl)
            ));
        }
    }
    CliResponse::ok_json(v.to_string())
}

/// The owner's link, handed back once (no mail).
fn returned_link(minted: &invites::Minted, page: &str, person: &skin::SkinPrincipal, app: &str, actor: &str, what: &str) -> CliResponse {
    let exp = match invites::activate(&minted.id) {
        Ok(Some(e)) => e,
        Ok(None) => return CliResponse::internal_error("the link vanished"),
        Err(e) => return CliResponse::internal_error(e),
    };
    audit(&format!("app_user_{what}_link"), actor, &format!("{} app={app} returned to the owner", person.username));
    CliResponse::ok_json(
        json!({
            "ok": true,
            "username": person.username,
            "app": app,
            "delivered": "returned",
            "link": invites::link_for(page, &minted.token),
            "expiresAt": exp,
            "ttlSecs": minted.ttl_secs,
            "hint": "shown once: pass it to the person yourself (it works once)",
        })
        .to_string(),
    )
}

// ── POST /cli/skin/users/reset-link (owner) ──────────────────────────

/// `POST /cli/skin/users/reset-link {username, app?, to?, code?, from?}`
/// (the owner only). Using the link sets the password, ends every session
/// and drops the person's agent tie.
pub fn handle_reset_link(body: &[u8], actor: &str, is_post: bool) -> CliResponse {
    if !is_post {
        return method_not_allowed();
    }
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = text(&v, &["username", "name"]) else { return usage("missing username") };
    let person = match principal(username) {
        Ok(Some(p)) => p,
        Ok(None) => return refusal("404 Not Found", "not_found", &format!("no App user {username:?}")),
        Err(r) => return r,
    };
    let grants = match skin::list_grants() {
        Ok(g) => g,
        Err(e) => return CliResponse::internal_error(e),
    };
    let mine: Vec<&skin::SkinGrant> = grants.iter().filter(|g| g.subject_kind == "principal" && g.subject_id == person.id && g.kind == "app").collect();
    let app = match text(&v, &["app", "appId"]) {
        Some(raw) => match crate::skin_routes::resolve_app_target(raw) {
            Ok(a) => a,
            Err(e) => return usage(&e),
        },
        // The tie's App first (the one the person keeps using), then any App
        // grant, then the box login.
        None => {
            let tie_ws = skin::ties::get(&person.id).ok().flatten().map(|t| t.workspace_id);
            let pick = mine
                .iter()
                .filter(|g| g.target_id != skin::HOST_APP_ID)
                .find(|g| tie_ws.is_some() && skin::ties::app_workspace(&g.target_id) == tie_ws)
                .or_else(|| mine.iter().find(|g| g.target_id != skin::HOST_APP_ID))
                .map(|g| g.target_id.clone());
            pick.unwrap_or_else(|| skin::HOST_APP_ID.to_string())
        }
    };
    let code = v.get("code").and_then(Value::as_bool).unwrap_or(false);
    let to = match text(&v, &["to"]) {
        Some(raw) => match skin::normalize_email(raw) {
            Ok(Some(e)) => Some(e),
            _ => return usage("invalid to address"),
        },
        None => None,
    };
    if code && to.is_some() {
        return usage("pass to or code, not both");
    }
    let settings = match invites::settings(&app) {
        Ok(s) => s,
        Err(e) => return CliResponse::internal_error(e),
    };
    let Some(page) = set_password_page(&settings) else {
        return refusal("409 Conflict", "no_set_password_page", &no_page_hint(&app));
    };
    let by = By::Owner { actor: actor.to_string() };
    let recipient = to.clone().or_else(|| person.email.clone());
    if !code && recipient.is_none() {
        return refusal(
            "409 Conflict",
            "no_email",
            &format!("'{}' has no email: give one (to), or ask for a one-time link (code).", person.username),
        );
    }
    let sender = if code {
        None
    } else {
        match invite_sender(skin::ties::app_workspace(&app).as_deref(), &by, text(&v, &["from"])) {
            Ok(s) => s,
            Err(r) => return r,
        }
    };
    let sent_to = sender.as_ref().and(recipient.as_deref());
    let minted = match invites::mint(&person.id, &app, invites::PURPOSE_OWNER_RESET, settings.reset_ttl_secs, sent_to.or(to.as_deref()), to.is_some(), actor) {
        Ok(m) => m,
        Err(invites::MintError::Throttled) => return refusal("429 Too Many Requests", "throttled", "wait a minute, then send it again"),
        Err(invites::MintError::Store(e)) => return CliResponse::internal_error(e),
    };
    let Some(sender) = sender else {
        return returned_link(&minted, &page, &person, &app, actor, "reset");
    };
    let to_addr = recipient.unwrap_or_default();
    let mail = Mail { purpose: invites::PURPOSE_OWNER_RESET, app: &app, to: &to_addr, person: &person, page: &page, sender: &sender, by: &by, settings: &settings };
    match dispatch(&mail, &minted) {
        Ok(d) => {
            audit("app_user_reset_link", actor, &format!("{} app={app} to={to_addr}", person.username));
            delivered_json(d, &person, &app, &to_addr, &sender, minted.ttl_secs, "Reset link")
        }
        Err(r) => r,
    }
}

// ── an App's forgot (S0 follow-up) ───────────────────────────────────

/// An App's `password.forgot` for a guest its pass may reset (`ws`: the
/// workspace it matched): a `reset` link mailed to the guest's own email
/// from that workspace's address, under its governance. The caller always
/// answers the bare `{ok:true}`; a guest with no password or email, no
/// set-password page, no mail path or mail off sends nothing (logged).
pub fn forgot_by_pass(pass: &skin::SkinPass, ws: &str, username: &str) {
    let skip = |why: &str| k2_core::log_debug!("[invites] forgot via pass {} for {username}: nothing sent ({why})", pass.id);
    let person = match principal(username) {
        Ok(Some(p)) => p,
        _ => return skip("no such guest"),
    };
    let (Some(email), true) = (person.email.clone(), person.has_password) else {
        return skip("no password or no email");
    };
    let app = if pass.app.is_empty() { skin::HOST_APP_ID.to_string() } else { pass.app.clone() };
    let settings = match invites::settings(&app) {
        Ok(s) => s,
        Err(e) => return skip(&e),
    };
    let Some(page) = set_password_page(&settings) else { return skip("no set-password page") };
    let by = By::App { ws: ws.to_string(), actor: format!("pass:{}", pass.id) };
    let sender = match invite_sender(Some(ws), &by, None) {
        Ok(Some(s)) if s.gate != Gate::Off => s,
        Ok(Some(_)) => return skip("outbound mail is off for the workspace"),
        Ok(None) => return skip("no address to send from"),
        Err(r) => return skip(&r.body),
    };
    let minted = match invites::mint(&person.id, &app, invites::PURPOSE_RESET, settings.reset_ttl_secs, Some(&email), false, by.actor()) {
        Ok(m) => m,
        Err(e) => return skip(&format!("{e:?}")),
    };
    let mail = Mail { purpose: invites::PURPOSE_RESET, app: &app, to: &email, person: &person, page: &page, sender: &sender, by: &by, settings: &settings };
    match dispatch(&mail, &minted) {
        Ok(_) => audit("app_user_forgot", by.actor(), &format!("{} app={app}", person.username)),
        Err(r) => skip(&r.body),
    }
}

// ── POST /cli/skin/access/invites ────────────────────────────────────

fn ttl_field(v: &Value, key: &str) -> Result<Option<Option<i64>>, CliResponse> {
    match v.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(s)) if s.trim().is_empty() || s.trim() == "default" => Ok(Some(None)),
        Some(Value::String(s)) => invites::parse_ttl(s).map(|t| Some(Some(t))).map_err(|e| usage(&e)),
        Some(Value::Number(n)) => match n.as_i64() {
            Some(t) if (invites::MIN_TTL_SECS..=invites::MAX_TTL_SECS).contains(&t) => Ok(Some(Some(t))),
            _ => Err(usage("a link lives between 15 minutes and 30 days (seconds, or 15m / 72h / 30d)")),
        },
        _ => Err(usage(&format!("{key}: a duration like 72h, or null for the default"))),
    }
}

fn text_field(v: &Value, key: &str) -> Result<Option<Option<String>>, CliResponse> {
    match v.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(s)) => Ok(Some(Some(s.clone()))),
        _ => Err(usage(&format!("{key}: text, or null for K2's own"))),
    }
}

/// `POST /cli/skin/access/invites {app, inviteTtl?, resetTtl?,
/// setPasswordUrl?, inviteSubject?, inviteBody?, resetSubject?,
/// resetBody?}`: the owner, or the App's own agent with "…manage them end
/// to end" (who builds the App). `null` (or "") puts a field back to K2's.
/// A GET-free route: read them with `GET /cli/skin/access?app=`.
pub fn handle_settings(body: &[u8], who: &crate::compute_routes::Who, is_post: bool) -> CliResponse {
    if !is_post {
        return method_not_allowed();
    }
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(raw_app) = text(&v, &["app", "appId"]) else { return usage("name the App (app)") };
    let app = match crate::skin_routes::resolve_app_target(raw_app) {
        Ok(a) => a,
        Err(e) => return usage(&e),
    };
    let allowed = match skin::ties::app_workspace(&app) {
        Some(ws) => crate::app_mobile_routes::may_set_listed(who, &ws),
        // `host` (the box login): the owner's alone.
        None => crate::app_mobile_routes::may_set_listed(who, ""),
    };
    if !allowed {
        return owner_only("only the owner, or this App's own agent with \"…manage them end to end\" on, sets an App's invite settings");
    }
    let patch = (|| -> Result<invites::InviteSettingsPatch, CliResponse> {
        Ok(invites::InviteSettingsPatch {
            invite_ttl_secs: ttl_field(&v, "inviteTtl")?,
            reset_ttl_secs: ttl_field(&v, "resetTtl")?,
            set_password_url: text_field(&v, "setPasswordUrl")?,
            invite_subject: text_field(&v, "inviteSubject")?,
            invite_body: text_field(&v, "inviteBody")?,
            reset_subject: text_field(&v, "resetSubject")?,
            reset_body: text_field(&v, "resetBody")?,
        })
    })();
    let patch = match patch {
        Ok(p) => p,
        Err(r) => return r,
    };
    let actor = who.label();
    match invites::set_settings(&app, &patch, &actor) {
        Ok(s) => {
            audit("app_invite_settings", &actor, &format!("app={app} invite={}s reset={}s", s.invite_ttl_secs, s.reset_ttl_secs));
            CliResponse::ok_json(json!({ "ok": true, "app": app, "settings": s }).to_string())
        }
        Err(e) => usage(&e),
    }
}

// ── the owner's card (§5.5) ──────────────────────────────────────────

/// The card's action id ([`crate::action_cards::run`]).
pub const CARD_ACTION: &str = "app_user.reset";
pub const CARD_SEND: &str = "Send reset link";
pub const CARD_OTHER_EMAIL: &str = "Send to another email…";
pub const CARD_SHOW_LINK: &str = "Show a one-time link";
pub const CARD_NOT_NOW: &str = "Not now";

fn notified() -> &'static Mutex<std::collections::HashSet<String>> {
    static SET: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    SET.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// What the owner gave that waits: "the box login", "App Meet", "room
/// Sales", "files:download", "chat".
fn waiting_parts(tie: &skin::ties::Tie, grants: &[skin::SkinGrant], roles: &[skin::SkinRole]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    for g in grants.iter().filter(|g| g.subject_kind == "principal" && g.subject_id == tie.principal_id && g.enabled) {
        if g.kind != "app" {
            push(g.kind.clone());
            continue;
        }
        if g.target_id == skin::HOST_APP_ID {
            if !tie.host_ok {
                push("the box login".to_string());
                continue;
            }
        } else if skin::ties::app_workspace(&g.target_id).as_deref() != Some(tie.workspace_id.as_str()) {
            push(format!("App {}", app_label(&g.target_id)));
            continue;
        }
        let policy: skin::RoomPolicy = match g.role_id.as_deref() {
            Some(rid) => roles.iter().find(|r| r.id == rid).map(|r| r.room_policy.clone()).unwrap_or_default(),
            None => skin::parse_rooms_json(g.scope.as_deref()).into_iter().map(|r| (r, Vec::new())).collect(),
        };
        for (room, caps) in policy {
            if room != tie.workspace_id {
                push(format!("room {}", crate::apps::routes::ws_handle(&room)));
            } else {
                for c in caps.iter().filter(|c| skin::PERSON_ONLY_CAPS.contains(&c.as_str())) {
                    push(c.clone());
                }
            }
        }
    }
    out
}

/// After an owner's skin write: each tied person whose grants now reach
/// past their ceiling gets one card on their tie's Thread (an inbox note
/// when that workspace has no Thread). One per person while it stays
/// pending; nothing again until it clears and comes back.
pub fn sweep_pending() {
    let tied = match crate::skin_routes::tie_display() {
        Ok(t) => t,
        Err(e) => {
            k2_core::log_debug!("[invites] pending sweep: {e}");
            return;
        }
    };
    let pending: Vec<&crate::skin_routes::TieDisplay> = tied.values().filter(|t| t.pending).collect();
    {
        let mut seen = notified().lock().unwrap_or_else(|e| e.into_inner());
        seen.retain(|pid| tied.get(pid).is_some_and(|t| t.pending));
    }
    if pending.is_empty() {
        return;
    }
    let (Ok(grants), Ok(roles), Ok(people)) = (skin::list_grants(), skin::list_roles(), skin::list_principals()) else { return };
    for t in pending {
        let pid = &t.tie.principal_id;
        if !notified().lock().unwrap_or_else(|e| e.into_inner()).insert(pid.clone()) {
            continue;
        }
        let Some(p) = people.iter().find(|p| &p.id == pid) else { continue };
        if let Err(e) = post_card(t, p, &grants, &roles) {
            k2_core::log_debug!("[invites] card for {}: {e}", p.username);
        }
    }
}

fn card_text(t: &crate::skin_routes::TieDisplay, p: &skin::SkinPrincipal, grants: &[skin::SkinGrant], roles: &[skin::SkinRole]) -> (String, String) {
    let agent = k2_core::workspace_session_handles::workspace_address_name_shared(&t.tie.workspace_id).unwrap_or_else(|_| t.handle.clone());
    let u = &p.username;
    let did: Vec<String> = t
        .tie
        .reasons
        .iter()
        .map(|r| match r.as_str() {
            "created" => format!("created {u}"),
            "password" => format!("set {u}'s password"),
            "email" => format!("set {u}'s email"),
            "invite" => format!("invited {u}"),
            _ => format!("made {u} through an App key"),
        })
        .collect();
    let parts = waiting_parts(&t.tie, grants, roles);
    let what = if parts.is_empty() { "more access".to_string() } else { parts.join(", ") };
    let to = match p.email.as_deref() {
        Some(e) if t.tie.reasons.iter().any(|r| r == "email") => format!("Reset link goes to {e} — set by {agent}."),
        Some(e) => format!("Reset link goes to {e}."),
        None => format!("{u} has no email yet: send to another email, or show a one-time link."),
    };
    let title = format!("Extra access for {u} waits for a password reset");
    let body = format!(
        "{title}\n\n{agent} {}, so the extra access you gave ({what}) starts only after {u} resets their password. Until then {u} keeps {agent}'s workspace access.\n{to}",
        did.join(" and ")
    );
    (title, body)
}

fn post_card(t: &crate::skin_routes::TieDisplay, p: &skin::SkinPrincipal, grants: &[skin::SkinGrant], roles: &[skin::SkinRole]) -> Result<(), String> {
    let (title, prompt) = card_text(t, p, grants, roles);
    let action = k2_core::overlay::CardAction {
        id: CARD_ACTION.to_string(),
        params: json!({ "username": p.username, "principalId": p.id }),
        dedupe: p.id.clone(),
        result: None,
        view: None,
    };
    let options = [CARD_SEND, CARD_OTHER_EMAIL, CARD_SHOW_LINK, CARD_NOT_NOW];
    match crate::overlay_routes::present_action_card(
        None,
        &t.tie.workspace_id,
        &prompt,
        &options,
        action,
        false,
        k2_core::overlay::now_secs() + k2_core::overlay::DEFAULT_CARD_EXPIRY_SECS,
        crate::overlay_routes::CardAudience::AgentPost,
    )? {
        crate::overlay_routes::Presented::Card { .. } | crate::overlay_routes::Presented::Waiting { .. } => Ok(()),
        crate::overlay_routes::Presented::NoThread => {
            let body = format!(
                "{prompt}\n\nSend the reset link: Settings → User Access → {u} → Send reset link, or k2 app user reset {u} (--to <email>, or --code for a one-time link) from a terminal outside K2.",
                u = p.username
            );
            crate::feedback_routes::file_owners_only_notice(&t.tie.workspace_id, &t.handle, &title, &body).map(|_| ())
        }
    }
}

/// A person tapped the card (`by`: `owner` or a login). The link itself is
/// never shown on a Thread: the agent reads it.
pub fn run_card(params: &Value, answer: &str, by: &str) -> Result<Value, String> {
    let u = params["username"].as_str().filter(|s| !s.is_empty()).ok_or("the card names nobody")?;
    match answer {
        CARD_SEND => {
            let actor = if by == "owner" { "owner".to_string() } else { format!("user:{by}") };
            let r = handle_reset_link(json!({ "username": u }).to_string().as_bytes(), &actor, true);
            let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
            if !r.status.starts_with("200") {
                return Err(format!("not sent: {}", v["error"]["hint"].as_str().unwrap_or(&r.body)));
            }
            let message = match v["delivered"].as_str() {
                Some("sent") => format!("Reset link sent to {}.", v["sentTo"].as_str().unwrap_or("their email")),
                Some("held") => format!("Reset link to {} is waiting for approval.", v["sentTo"].as_str().unwrap_or("their email")),
                // No mail path: never put the link on a Thread.
                _ => format!("This box can't mail it. Run k2 app user reset {u} --code from a terminal outside K2 for a one-time link."),
            };
            Ok(json!({ "sent": v["delivered"] == "sent", "message": message }))
        }
        CARD_OTHER_EMAIL => Ok(json!({ "message": format!("Send it elsewhere: k2 app user reset {u} --to <email> from a terminal outside K2, or Settings → User Access → {u}.") })),
        CARD_SHOW_LINK => Ok(json!({ "message": format!("A one-time link is never shown on a Thread (the agent reads it): k2 app user reset {u} --code from a terminal outside K2, or Settings → User Access → {u}.") })),
        _ => Ok(json!({ "message": "Not now. Settings → User Access shows who waits for a reset." })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_words_read_like_people_write() {
        assert_eq!(ttl_words(72 * 3600), "72 hours");
        assert_eq!(ttl_words(24 * 3600), "24 hours");
        assert_eq!(ttl_words(3600), "1 hour");
        assert_eq!(ttl_words(15 * 60), "15 minutes");
        assert_eq!(ttl_words(30 * 86_400), "30 days");
    }

    #[test]
    fn both_routes_refuse_a_get() {
        let r = handle_invite(b"{}", "owner-token", None, false);
        assert_eq!(r.status, "405 Method Not Allowed", "{}", r.body);
        let r = handle_reset_link(b"{}", "owner-token", false);
        assert_eq!(r.status, "405 Method Not Allowed", "{}", r.body);
    }
}
