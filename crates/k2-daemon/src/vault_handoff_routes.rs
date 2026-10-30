//! Agent-to-agent secret handoff routes (`prd-agent-secret-handoff-v1`
//! §4, §5.1, §13, as amended by the vs-live H1–H23 and Rosson's rulings
//! of 2026-10-17).
//!
//! - `POST /cli/vault/send` — an agent (or a person) sends a vault secret
//!   **by name** to another agent: this server (vault to vault in one
//!   call) or a federated one (`federation_vault::send_remote`, sealed to
//!   the receiving vault). A person may pipe a value in on the person
//!   channel (`--stdin`), cross server only (§4.4, H15).
//! - `GET /cli/vault/incoming` / `GET /cli/vault/sent` — names only.
//! - `POST /cli/vault/incoming/decide` — a person accepts or declines.
//! - `GET|POST /cli/vault/accept-from` — the per-agent allow-list (POST:
//!   people only, H19).
//!
//! Every step writes a `vault-handoff` line to the auth audit (Settings →
//! Logs → Access Audit) and a row to the vault's own audit chain: send,
//! receive, accept, decline, expire, and every refusal with its reason.
//! Names and delivery ids only, never a value.

use std::collections::HashMap;
use std::sync::Arc;

use k2_vault::handoff::{self, Decision, HandoffItem, HandoffRecord, Incoming};
use k2_vault::{Actor, LocalVault, Requester, Scope, SecretBytes, SecretStore, Selector, Use, VaultError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::vault::actor::VaultCaller;
use crate::vault_routes::{refuse, Sel};

pub const SEND: &str = "/cli/vault/send";
pub const INCOMING: &str = "/cli/vault/incoming";
pub const DECIDE: &str = "/cli/vault/incoming/decide";
pub const SENT: &str = "/cli/vault/sent";
pub const ACCEPT_FROM: &str = "/cli/vault/accept-from";
/// H5: ask an agent for a secret (a stamp message, never a value).
pub const ASK: &str = "/cli/vault/ask";
/// H5: this workspace's open and closed asks.
pub const ASKS: &str = "/cli/vault/asks";
/// H5: take back a delivery (not a revoke: the key may be in use).
pub const UNSEND: &str = "/cli/vault/unsend";

/// Routes this module serves.
pub const ROUTES: &[&str] = &[SEND, INCOMING, DECIDE, SENT, ACCEPT_FROM, ASK, ASKS, UNSEND];
/// POST-only routes among them (`accept-from` serves both).
pub const POST_ROUTES: &[&str] = &[SEND, DECIDE, ASK, UNSEND];

/// The auth-audit event every handoff line uses (Settings → Logs → Access
/// Audit filters on it).
pub const AUDIT_EVENT: &str = "vault-handoff";

pub fn is_route(path: &str) -> bool {
    ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

pub fn handle(
    vault: &Arc<LocalVault>,
    path: &str,
    c: &VaultCaller,
    body: &[u8],
    params: &HashMap<String, String>,
    ingress: &str,
) -> CliResponse {
    let r = match path {
        SEND => return send(vault, c, body, ingress),
        INCOMING => list(vault, c, params, "in"),
        SENT => list(vault, c, params, "out"),
        DECIDE => decide(vault, c, body, ingress),
        ACCEPT_FROM if body.iter().all(|b| b.is_ascii_whitespace()) => accept_from_get(vault, c, params),
        ACCEPT_FROM => accept_from_post(vault, c, body, ingress),
        ASK => ask(vault, c, body, ingress),
        ASKS => asks(vault, c, params),
        UNSEND => unsend(vault, c, body, ingress),
        _ => Err(VaultError::not_found("no such vault route")),
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refuse(&e),
    }
}

/// One `vault-handoff` line in `auth-audit.jsonl` (Settings → Logs).
pub(crate) fn audit(user: &str, outcome: &str, ingress: &str) {
    crate::vault::auth_audit(AUDIT_EVENT, user, outcome, ingress);
}

fn parse<T: for<'de> Deserialize<'de> + Default>(body: &[u8]) -> Result<T, VaultError> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    // Never echo body text (it may hold a value).
    serde_json::from_slice(body).map_err(|_| VaultError::bad_request("bad_json", "bad request body"))
}

// ── send ────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendBody {
    name: Option<String>,
    to: Option<String>,
    #[serde(rename = "as")]
    as_name: Option<String>,
    #[serde(default)]
    note: String,
    ttl_secs: Option<i64>,
    reply_to: Option<String>,
    /// People: which secret (`--scope ws:<name>`).
    scope: Option<String>,
    workspace: Option<String>,
    /// `--stdin` (person channel, cross server only).
    value_b64: Option<String>,
    /// `--kind`: what it is (a plain value relabels; B, 2026-10-30).
    kind: Option<String>,
    /// `--delivery fetch|file|env`: how the receiving agent gets it (B).
    /// Absent: the sender's own delivery, as before.
    delivery: Option<String>,
    #[serde(default)]
    hosts: Vec<String>,
    description: Option<String>,
    /// `msg`: `k2 msg … --secret` (a longer note, §4.2).
    via: Option<String>,
    /// A vault confirm (`handoff-send`) for a person-given secret going to
    /// another server (§7.6).
    confirm_token: Option<String>,
}

/// Where a send goes.
pub(crate) enum Target {
    Local(String),
    Remote { agent: String, host: String, legacy_at: bool },
}

pub(crate) fn parse_target(to: &str) -> Result<Target, VaultError> {
    let t = to.trim();
    if t.is_empty() {
        return Err(VaultError::bad_request("to_required", "name who gets it: --to <agent> or --to <agent>::<host>"));
    }
    if let Some((a, h)) = t.split_once("::") {
        let (a, h) = (a.trim(), h.trim());
        if a.is_empty() || h.is_empty() || h.contains("::") {
            return Err(VaultError::bad_request("bad_address", "addresses are <agent> or <agent>::<host>"));
        }
        return Ok(Target::Remote { agent: a.to_string(), host: h.to_string(), legacy_at: false });
    }
    if let Some((a, h)) = t.split_once('@') {
        if !a.is_empty() && h.contains('.') {
            return Ok(Target::Remote { agent: a.to_string(), host: h.to_string(), legacy_at: true });
        }
    }
    Ok(Target::Local(t.to_string()))
}

/// H20: the owner token, a live passport, a Connect session or the tunnel
/// token never travel (exact match; prefixes are checked in k2-vault).
pub(crate) fn is_k2_credential(value: &[u8]) -> bool {
    let Some(s) = std::str::from_utf8(value).ok().map(str::trim) else { return false };
    if s.len() < 8 {
        return false;
    }
    if let Some(home) = k2_core::paths::home_dir() {
        for f in ["daemon.token", "heartbeat.token"] {
            if let Ok(t) = std::fs::read_to_string(home.join(".k2").join(f)) {
                if !t.trim().is_empty() && crate::routes::http::ct_eq_token(t.trim(), s) {
                    return true;
                }
            }
        }
    }
    if crate::session_token::validate_hook(s).is_some() {
        return true;
    }
    if k2_core::connect_users::validate_session(s).is_some() {
        return true;
    }
    if let Ok(cfg) = k2_core::tunnel::config::load() {
        if !cfg.token.is_empty() && crate::routes::http::ct_eq_token(&cfg.token, s) {
            return true;
        }
    }
    false
}

/// What a send resolved to before it leaves this server.
pub(crate) struct Prepared {
    pub item: HandoffItem,
    pub value: SecretBytes,
    pub note: String,
    /// The sending workspace (none for a person's `--stdin`).
    pub sender_ws: Option<String>,
    /// The source secret (id, version), for the out row's audit.
    pub source: Option<(String, u32)>,
}

fn send(vault: &Arc<LocalVault>, c: &VaultCaller, body: &[u8], ingress: &str) -> CliResponse {
    let b: SendBody = match parse(body) {
        Ok(b) => b,
        Err(e) => return refuse(&e),
    };
    let stamp = c.actor.stamp();
    let name = b.name.clone().unwrap_or_default();
    let to = b.to.clone().unwrap_or_default();
    match send_inner(vault, c, &b, ingress) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(SendError::Vault(e)) => {
            audit(&stamp, &format!("deny send {name} to {to}: {}", e.code), ingress);
            vault.handoff_audit("send", &stamp, &to, &format!("deny:{}", e.code));
            refuse(&e)
        }
        Err(SendError::Response(code, r)) => {
            audit(&stamp, &format!("deny send {name} to {to}: {code}"), ingress);
            vault.handoff_audit("send", &stamp, &to, &format!("deny:{code}"));
            r
        }
    }
}

/// A refusal: a vault error, or a ready response (with extra fields).
pub(crate) enum SendError {
    Vault(VaultError),
    Response(&'static str, CliResponse),
}

impl From<VaultError> for SendError {
    fn from(e: VaultError) -> Self {
        SendError::Vault(e)
    }
}

fn send_inner(vault: &Arc<LocalVault>, c: &VaultCaller, b: &SendBody, ingress: &str) -> Result<Value, SendError> {
    let target = parse_target(b.to.as_deref().unwrap_or(""))?;
    let note_max = if b.via.as_deref() == Some("msg") { handoff::MSG_NOTE_MAX } else { handoff::NOTE_MAX };
    if let Some(t) = b.ttl_secs {
        if !(handoff::TTL_MIN_SECS..=handoff::TTL_MAX_SECS).contains(&t) {
            return Err(VaultError::bad_request("bad_ttl", "--ttl is 1 hour to 90 days").into());
        }
    }
    let as_name = b.as_name.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(a) = &as_name {
        k2_vault::model::validate_name(a, true)?;
    }
    let mut prep = match b.value_b64.as_deref() {
        Some(v64) => {
            // §4.4 / H15: only a person types a value in, cross server only.
            if c.is_passport() || !c.channel.is_person() {
                return Err(VaultError::new(
                    "send_by_name_only",
                    422,
                    "agents send secrets by name: put it in your vault first (k2 vault set NAME < file), then k2 vault send NAME --to <agent>",
                )
                .into());
            }
            if matches!(target, Target::Local(_)) {
                return Err(VaultError::new(
                    "use_vault_set",
                    422,
                    "on this server, give it straight to the agent: k2 vault set NAME --scope ws:<agent> < file",
                )
                .into());
            }
            prepare_stdin(b, v64, as_name.clone(), note_max)?
        }
        None => prepare_named(vault, c, b, as_name.clone(), note_max)?,
    };
    if is_k2_credential(prep.value.expose()) {
        return Err(VaultError::new("never_sendable", 422, "that value is a K2 credential (an owner token, passport, login session or tunnel token); those are never sent").into());
    }
    prep.note = crate::vault::redact_str(&prep.note);
    match target {
        Target::Local(token) => send_local(vault, c, &token, prep, b.reply_to.clone(), ingress),
        Target::Remote { agent, host, legacy_at } => {
            let mut v = crate::federation_vault::send_remote(vault, c, &agent, &host, prep, b.reply_to.clone(), b.confirm_token.as_deref(), ingress)?;
            if legacy_at {
                v["addressNote"] = json!(format!("{agent}@{host} is written {agent}::{host} now"));
            }
            Ok(v)
        }
    }
}

fn prepare_stdin(b: &SendBody, v64: &str, as_name: Option<String>, note_max: usize) -> Result<Prepared, VaultError> {
    use base64::Engine as _;
    let name = b.name.as_deref().map(str::trim).unwrap_or("");
    k2_vault::model::validate_name(name, true)?;
    let value = SecretBytes::new(
        base64::engine::general_purpose::STANDARD
            .decode(v64.trim())
            .map_err(|_| VaultError::bad_request("bad_value", "valueB64 isn't base64"))?,
    );
    handoff::check_value(value.expose())?;
    let kind = k2_vault::Kind::parse(b.kind.as_deref().unwrap_or("api_key"))?;
    let hosts = k2_vault::hosts::normalise_list(&b.hosts, false)?;
    let item = HandoffItem {
        name: name.to_string(),
        as_name,
        kind: kind.as_str().to_string(),
        hosts,
        inject: None,
        description: b.description.clone().unwrap_or_default(),
        delivery: kind.default_delivery().as_str().to_string(),
        ceiling: "env".into(),
        human_given: true,
        ttl_secs: b.ttl_secs,
    };
    let mut item = item;
    item.shape_for(None, b.delivery.as_deref())?;
    item.validate()?;
    let note = handoff::check_note(&b.note, note_max, value.expose())?;
    Ok(Prepared { item, value, note, sender_ws: None, source: None })
}

fn prepare_named(vault: &Arc<LocalVault>, c: &VaultCaller, b: &SendBody, as_name: Option<String>, note_max: usize) -> Result<Prepared, VaultError> {
    let name = b.name.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| VaultError::bad_request("name_required", "name the secret"))?;
    let sel = match c.workspace_id() {
        // An agent sends its own workspace's secrets (§7.1).
        Some(ws) => Selector::user(Scope::workspace(ws), name),
        None => crate::vault_routes::selector(
            c,
            &Sel { name: Some(name.to_string()), scope: b.scope.clone(), workspace: b.workspace.clone(), ..Default::default() },
        )?,
    };
    let meta = vault.meta(&sel, &c.actor).map_err(|e| {
        if e.status == 404 {
            VaultError::not_found(format!("no secret named {name} here (k2 vault list)"))
        } else {
            e
        }
    })?;
    handoff::check_sendable(&meta)?;
    if let Actor::Login { username, role: k2_vault::Role::Member } = &c.actor {
        // Members send what they keep themselves (Member `can_manage`).
        if meta.created_by != format!("connect:{username}") {
            return Err(VaultError::forbidden("vault_not_yours", "Members send only secrets they gave; ask an Owner or Admin"));
        }
    }
    let value = vault.resolve(&Selector::Id(meta.id.clone()), Use::System, &Requester::System(handoff::SYSTEM_ACTOR.into()))?.value;
    handoff::check_value(value.expose())?;
    let note = handoff::check_note(&b.note, note_max, value.expose())?;
    let mut item = HandoffItem::from_meta(&meta, as_name, b.ttl_secs);
    // B (2026-10-30): the sender says what it is. Never above its own
    // ceiling; nothing said keeps today's item.
    item.shape_for(b.kind.as_deref(), b.delivery.as_deref())?;
    item.validate()?;
    Ok(Prepared { item, value, note, sender_ws: Some(meta.scope.id.clone()), source: Some((meta.id.clone(), meta.latest_version)) })
}

/// The accept-from keys a local sender matches: `ws:<id>` and
/// `team:<group>` for each project group it belongs to.
pub(crate) fn local_sender_keys(ws: &str) -> Vec<String> {
    let mut out = vec![format!("ws:{ws}")];
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Ok(mut st) = conn.prepare("SELECT group_id FROM project_group_members WHERE workspace_id = ?1") {
        if let Ok(it) = st.query_map(rusqlite::params![ws], |r| r.get::<_, String>(0)) {
            out.extend(it.flatten().map(|g| format!("team:{g}")));
        }
    }
    out
}

/// A reply to an open ask this workspace made, from the asked address,
/// for the asked name (H5, R1: it lands straight in).
pub(crate) fn reply_matches(vault: &LocalVault, ws: &str, reply_to: Option<&str>, peer_addr: &str, name: &str) -> bool {
    match reply_to {
        Some(id) => vault.handoff_ask_matches(id, ws, peer_addr, name),
        None => false,
    }
}

/// After a receive: a landed reply answers its ask.
pub(crate) fn close_answered_ask(vault: &LocalVault, rec: &HandoffRecord, replied: bool) {
    if let (true, "landed", Some(id)) = (replied, rec.status.as_str(), rec.reply_to.as_deref()) {
        if let Err(e) = vault.handoff_close_ask(id, "ask_out", "answered") {
            k2_core::log_debug!("[vault-handoff] close ask {id}: {}", e.code);
        }
    }
}

fn send_local(vault: &Arc<LocalVault>, c: &VaultCaller, token: &str, prep: Prepared, reply_to: Option<String>, ingress: &str) -> Result<Value, SendError> {
    let Prepared { item, value, note, sender_ws, source } = prep;
    let sender_ws = sender_ws.ok_or_else(|| VaultError::internal("a named send has a workspace"))?;
    let Some((target_path, target_id)) = crate::comms::resolve_target_project(token) else {
        return Err(VaultError::not_found(format!("no agent named {token} on this server")).into());
    };
    if target_id == sender_ws {
        return Err(VaultError::bad_request("same_workspace", "that's the workspace the secret is already in").into());
    }
    let target_label = crate::vault::workspace_label(&target_id);
    if let Some(caller_ws) = c.workspace_id() {
        // The local message gate (workspace_relations), same as `k2 msg`.
        if let Err(e) = crate::comms::require_local_peer(caller_ws, &target_id, &target_label) {
            return Err(VaultError::forbidden("not_connected", e.hint().to_string()).into());
        }
    }
    let sender_label = crate::vault::workspace_label(&sender_ws);
    let stamp = c.actor.stamp();
    let replied = reply_matches(vault, &target_id, reply_to.as_deref(), &sender_label, &item.name);
    let auto = vault.accepts_from(&target_id, &local_sender_keys(&sender_ws)) || replied;
    let delivery_id = handoff::new_delivery_id();
    let name = item.name.clone();
    let landed_name = item.landed_name().to_string();
    let inc = Incoming {
        delivery_id: delivery_id.clone(),
        workspace_id: target_id.clone(),
        peer_addr: sender_label.clone(),
        peer_fp: None,
        sender_actor: stamp.clone(),
        source_vault_id: None,
        item: item.clone(),
        note: note.clone(),
        reply_to: reply_to.clone(),
        auto_land: auto,
        remote_off: false,
        sender_label: format!("{sender_label} on this server"),
    };
    let got = vault.handoff_receive(inc, value)?;
    let rec = got.record;
    close_answered_ask(vault, &rec, replied);
    let out = HandoffRecord {
        delivery_id: delivery_id.clone(),
        direction: "out".into(),
        status: rec.status.clone(),
        local_workspace_id: sender_ws.clone(),
        peer_addr: target_label.clone(),
        peer_fp: None,
        sender_actor: stamp.clone(),
        source_vault_id: None,
        name: name.clone(),
        landed_name: Some(landed_name.clone()),
        kind: item.kind.clone(),
        note: note.clone(),
        reply_to: reply_to.clone(),
        item: item.clone(),
        secret_id: source.as_ref().map(|s| s.0.clone()),
        version: source.as_ref().map(|s| s.1),
        reason: rec.reason.clone(),
        card: None,
        created_at: rec.created_at.clone(),
        decided_at: None,
        decided_by: None,
        expires_at: None,
    };
    vault.handoff_record_out(out, source.as_ref().map(|(i, v)| (i.as_str(), *v)))?;
    audit(&stamp, &format!("ok send {name} to {target_label} {delivery_id} {}", rec.status), ingress);
    audit(&format!("handoff:{sender_label}"), &format!("ok receive {landed_name} from {sender_label} into {target_label} {delivery_id} {}", rec.status), ingress);
    let reference = got.meta.as_ref().and_then(crate::vault::display_ref);
    after_receive(vault, &rec, &target_path, &target_id, &sender_label, None, reference.as_deref(), true, sender_for(c, &sender_ws));
    Ok(send_answer(&rec, &target_label, false, reference))
}

/// The `msg_deliveries` sender for a stamp message (who really sent it).
pub(crate) fn sender_for(c: &VaultCaller, sender_ws: &str) -> crate::hire_routes::Sender {
    match &c.actor {
        Actor::Agent(_) => crate::hire_routes::Sender {
            workspace_id: Some(sender_ws.to_string()),
            via: "passport",
            actor: c.actor.stamp(),
            on_behalf_of: None,
        },
        _ => crate::hire_routes::Sender { workspace_id: None, via: "owner", actor: c.actor.stamp(), on_behalf_of: None },
    }
}

/// The send's answer (names only). `who` is the one teaching line saying
/// who must act (exit 3 in the CLI).
pub(crate) fn send_answer(rec: &HandoffRecord, to: &str, cross: bool, reference: Option<String>) -> Value {
    let who = match rec.status.as_str() {
        "landed" => None,
        "pending" if cross => Some(format!("a person on {} must accept it (k2 vault incoming there)", to.split_once("::").map(|x| x.1).unwrap_or(to))),
        "pending" => Some("a person on this server must accept it (k2 vault incoming, or the card on that agent's Thread)".to_string()),
        _ => Some(format!("it was {}", rec.status)),
    };
    json!({
        "ok": true,
        "deliveryId": rec.delivery_id,
        "status": rec.status,
        "reason": rec.reason,
        "name": rec.name,
        "landedName": rec.landed_name,
        "to": to,
        "cross": cross,
        "who": who,
        "ref": reference,
        "landsAs": lands_as(rec),
    })
}

/// What the receiving agent gets (the accept card shows the same): the
/// delivery and the ceiling a person's accept lets its agents take.
pub(crate) fn lands_as(rec: &HandoffRecord) -> Value {
    let (d, c) = rec.item.landing_delivery();
    json!({ "delivery": d.as_str(), "ceiling": c.as_str(), "sites": !rec.item.hosts.is_empty() })
}

/// The stamp message the receiving agent gets (§6.5): built by the daemon
/// from names, the reference and the (checked, redacted) note. This is the
/// only text a handoff ever injects into a PTY.
///
/// `landed`: the copy's delivery as stored (`None`: what the item lands
/// as). The "Use:" line follows it (bug 1, 2026-10-28: a file copy was
/// told to use `k2 vault fetch`).
pub(crate) fn stamp_text(rec: &HandoffRecord, from_label: &str, reference: Option<&str>, landed: Option<&k2_vault::SecretMeta>) -> String {
    let name = rec.landed_name.clone().unwrap_or_else(|| rec.name.clone());
    let note = if rec.note.is_empty() { String::new() } else { format!(" Note: \"{}\"", rec.note) };
    match rec.status.as_str() {
        "landed" => {
            let d = landed.map(|m| m.default_delivery).unwrap_or_else(|| rec.item.landing_delivery().0);
            // C (2026-10-30): fetch only for an HTTP key with sites.
            let sites = landed.map(|m| !m.hosts.is_empty()).unwrap_or(!rec.item.hosts.is_empty());
            format!(
                "[k2 vault] {from_label} sent {name} → your vault ({}, delivered to you as {d}{}). Use: {}.{note}",
                reference.unwrap_or("k2 vault list"),
                expiry_words(rec.expires_at.as_deref()),
                handoff::use_hint_for(d, &name, sites)
            )
        }
        _ => format!("[k2 vault] {from_label} sent {name}. It waits for your owner to accept (k2 vault incoming).{note}"),
    }
}

/// `, until 2026-10-28T13:00:00Z` for a copy with an expiry (`--ttl`).
fn expiry_words(expires_at: Option<&str>) -> String {
    expires_at.map(|x| format!(", until {x}")).unwrap_or_default()
}

/// The landed copy's own meta (delivery, ceiling, expiry), when there is one.
pub(crate) fn landed_meta(vault: &LocalVault, r: &HandoffRecord) -> Option<k2_vault::SecretMeta> {
    r.secret_id
        .as_ref()
        .filter(|_| r.direction == "in" && r.status == "landed")
        .and_then(|id| vault.meta(&Selector::Id(id.clone()), &Actor::System(handoff::SYSTEM_ACTOR.into())).ok())
}

/// What the receiving agent has: delivery, ceiling, expiry and how to use
/// it (the accept answer, `k2 vault incoming`). Names only.
pub(crate) fn landed_json(m: &k2_vault::SecretMeta) -> Value {
    json!({
        "name": m.name,
        "delivery": m.default_delivery.as_str(),
        "ceiling": m.delivery_ceiling.as_str(),
        "expiresAt": m.expires_at,
        "use": handoff::use_hint_for(m.default_delivery, &m.name, !m.hosts.is_empty()),
    })
}

/// After the receiving vault answered: the Thread card (pending) and the
/// stamp message (§6.5). Best-effort; a failed knock never undoes a
/// landing. `knock`: false when the receiver's remote-instruct opt-in is
/// off (R4: pending, no knock). `local_sender`: a same-server send's
/// `msg_deliveries` sender; cross server the stamp is written by this
/// (receiving) daemon on the sender's behalf.
#[allow(clippy::too_many_arguments)]
pub(crate) fn after_receive(
    vault: &LocalVault,
    rec: &HandoffRecord,
    target_path: &str,
    target_id: &str,
    from_label: &str,
    host: Option<&str>,
    reference: Option<&str>,
    knock: bool,
    sender: crate::hire_routes::Sender,
) {
    let name = rec.landed_name.clone().unwrap_or_else(|| rec.name.clone());
    if rec.status == "pending" {
        let clash = rec.reason.as_deref() == Some("clash") || rec.reason.as_deref() == Some("resend");
        let card = k2_core::overlay::HandoffCardBody {
            delivery_id: rec.delivery_id.clone(),
            name: name.clone(),
            from: from_label.to_string(),
            host: host.map(str::to_string),
            kind: rec.kind.clone(),
            hosts: rec.item.hosts.clone(),
            note: rec.note.clone(),
            reason: rec.reason.clone().unwrap_or_default(),
            clash,
            status: "pending".into(),
            reference: None,
            answered_by: None,
            answered_at: None,
            expires_at: rec
                .expires_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp()),
            workspace: target_id.to_string(),
            // Bug 3: the card says how long the copy is kept (`--ttl`).
            ttl_secs: rec.item.ttl(),
            // A (2026-10-30): the person consents to what its agents get:
            // the delivery, and the ceiling they may take it up to.
            delivery: Some(rec.item.landing_delivery().0.as_str().to_string()),
            ceiling: Some(rec.item.landing_delivery().1.as_str().to_string()),
        };
        let body = format!("{from_label} sent {name}. Accept it in Settings → Vault → Incoming.");
        match crate::overlay_routes::post_handoff_card(target_path, from_label, &body, card) {
            Ok(card_ref) => {
                if let Err(e) = vault.handoff_set_card(&rec.delivery_id, &card_ref) {
                    k2_core::log_debug!("[vault-handoff] card ref for {}: {}", rec.delivery_id, e.code);
                }
            }
            Err(e) => k2_core::log_debug!("[vault-handoff] card for {}: {e}", rec.delivery_id),
        }
    }
    if !knock {
        return;
    }
    let landed = landed_meta(vault, rec);
    let text = stamp_text(rec, from_label, reference, landed.as_ref());
    let wake = rec.status == "landed";
    let (resp, _) = crate::hire_routes::deliver_recorded(
        target_path,
        &text,
        from_label,
        "",
        wake,
        crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
        "msg",
        &sender,
    );
    if !resp.success {
        k2_core::log_debug!("[vault-handoff] stamp for {} not delivered: {:?}", rec.delivery_id, resp.reason);
    }
}

// ── lists ───────────────────────────────────────────────────────────

/// The workspace a list or allow-list call is about: a passport's own;
/// a person's named one (or every workspace).
fn workspace_param(c: &VaultCaller, params: &HashMap<String, String>) -> Result<Option<String>, VaultError> {
    let named = params.get("workspace").map(|s| s.trim()).filter(|s| !s.is_empty());
    match (c.workspace_id(), named) {
        (Some(own), None) => Ok(Some(own.to_string())),
        (Some(own), Some(w)) => {
            let id = crate::vault::workspace_id(w).unwrap_or_else(|| w.to_string());
            if id != own {
                return Err(VaultError::forbidden("vault_not_yours", "agents see their own workspace's handoffs only"));
            }
            Ok(Some(id))
        }
        (None, Some(w)) => Ok(Some(crate::vault::workspace_id(w).ok_or_else(|| VaultError::not_found(format!("no workspace named {w}")))?)),
        (None, None) => Ok(None),
    }
}

pub(crate) fn view(vault: &LocalVault, r: &HandoffRecord) -> Value {
    let landed = landed_meta(vault, r);
    let reference = landed.as_ref().and_then(crate::vault::display_ref);
    // A pending copy says how it would land: its delivery (and ceiling)
    // and when it would expire (`--ttl`, counted from when it arrived).
    let (d, c) = r.item.landing_delivery();
    let copy_expires = chrono::DateTime::parse_from_rfc3339(&r.created_at)
        .ok()
        .and_then(|t| r.item.copy_expires_at(t.with_timezone(&chrono::Utc)))
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    let lands_as = json!({ "delivery": d.as_str(), "ceiling": c.as_str(), "expiresAt": copy_expires });
    json!({
        "deliveryId": r.delivery_id,
        "direction": r.direction,
        "status": r.status,
        "workspaceId": r.local_workspace_id,
        "workspace": crate::vault::workspace_label(&r.local_workspace_id),
        "peer": r.peer_addr,
        "cross": r.peer_addr.contains("::"),
        "name": r.name,
        "landedName": r.landed_name,
        "kind": r.kind,
        "hosts": r.item.hosts,
        "note": r.note,
        "reason": r.reason,
        "createdAt": r.created_at,
        "decidedAt": r.decided_at,
        "decidedBy": r.decided_by,
        "expiresAt": r.expires_at,
        "ref": reference,
        "ttlSecs": r.item.ttl(),
        "landsAs": lands_as,
        "secret": landed.as_ref().map(landed_json),
    })
}

fn list(vault: &LocalVault, c: &VaultCaller, params: &HashMap<String, String>, dir: &str) -> Result<Value, VaultError> {
    let ws = workspace_param(c, params)?;
    if ws.is_none() && c.actor.is_member() {
        return Err(VaultError::bad_request("workspace_required", "name the workspace (--workspace <name>)"));
    }
    let items = vault.handoffs(ws.as_deref(), Some(dir))?;
    Ok(json!({ "items": items.iter().map(|r| view(vault, r)).collect::<Vec<_>>() }))
}

// ── decide ──────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecideBody {
    id: Option<String>,
    #[serde(default)]
    action: String,
    #[serde(rename = "as")]
    as_name: Option<String>,
    #[serde(default)]
    replace: bool,
    #[serde(default)]
    always: bool,
}

pub(crate) fn person_author(a: &Actor) -> Option<k2_core::overlay::PostAuthor> {
    match a {
        Actor::Owner => Some(k2_core::overlay::PostAuthor::owner()),
        Actor::Login { username, .. } => Some(k2_core::overlay::PostAuthor::user(username)),
        _ => None,
    }
}

/// The accept-from key for a delivery's sender: `agent::host` cross
/// server, else `ws:<sending workspace id>` from this server's out row.
fn sender_key(vault: &LocalVault, r: &HandoffRecord) -> Option<String> {
    if r.peer_addr.contains("::") {
        return handoff::normalise_sender(&r.peer_addr).ok();
    }
    vault.handoff_get(&r.delivery_id, "out").ok().map(|o| format!("ws:{}", o.local_workspace_id))
}

fn decide(vault: &LocalVault, c: &VaultCaller, body: &[u8], ingress: &str) -> Result<Value, VaultError> {
    let b: DecideBody = parse(body)?;
    let id = b.id.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| VaultError::bad_request("id_required", "which delivery? (k2 vault incoming)"))?;
    let stamp = c.actor.stamp();
    if c.is_passport() {
        let e = VaultError::forbidden(
            "handoff_person_only",
            "a person on this server accepts or declines secrets from other agents (Settings → Vault → Incoming); agents can't",
        );
        audit(&stamp, &format!("deny {} {id}: {}", b.action, e.code), ingress);
        return Err(e);
    }
    let before = vault.handoff_get(id, "in")?;
    let d = Decision { action: b.action.clone(), as_name: b.as_name.clone(), replace: b.replace };
    let got = match vault.handoff_decide(id, &d, &c.actor) {
        Ok(g) => g,
        Err(e) => {
            audit(&stamp, &format!("deny {} {} from {} {id}: {}", b.action, before.name, before.peer_addr, e.code), ingress);
            if e.code == "not_pending" {
                // It may have expired just now: close its card.
                if let (Ok(r), Some(card)) = (vault.handoff_get(id, "in"), before.card.as_deref()) {
                    if r.status == "expired" {
                        crate::overlay_routes::close_handoff_card(card, "expired", None, None);
                    }
                }
            }
            return Err(e);
        }
    };
    let r = &got.record;
    let reference = got.meta.as_ref().and_then(crate::vault::display_ref);
    let landed = r.landed_name.clone().unwrap_or_else(|| r.name.clone());
    let what = if r.status == "landed" { "accept" } else { "decline" };
    let as_part = if r.status == "landed" && landed != r.name { format!(" as {landed}") } else { String::new() };
    audit(&stamp, &format!("ok {what} {} from {} {id}{as_part}", r.name, r.peer_addr), ingress);
    if let Some(card) = before.card.as_deref() {
        crate::overlay_routes::close_handoff_card(card, &r.status, reference.clone(), person_author(&c.actor));
    }
    // Same server: the sender's out row follows.
    if vault.handoff_get(id, "out").is_ok() {
        let _ = vault.handoff_update_out(id, &r.status, None);
    }
    let mut always_added = None;
    if b.always {
        if let Some(key) = sender_key(vault, r) {
            vault.accept_from_add(&r.local_workspace_id, &key, &c.actor)?;
            audit(&stamp, &format!("ok accept-from add {key} for {}", crate::vault::workspace_label(&r.local_workspace_id)), ingress);
            always_added = Some(key);
        }
    }
    if r.status == "landed" {
        // Knock the receiving agent: its owner said yes. The "Use:" line
        // follows the copy's real delivery (bug 1).
        if let Some(path) = crate::vault::workspace_path(&r.local_workspace_id) {
            let d = got.meta.as_ref().map(|m| m.default_delivery).unwrap_or_else(|| r.item.landing_delivery().0);
            let sites = got.meta.as_ref().map(|m| !m.hosts.is_empty()).unwrap_or(!r.item.hosts.is_empty());
            // A (2026-10-30): what the accept lets this agent take itself.
            let more = got.meta.as_ref().and_then(|m| accept_more(vault, m)).map(|t| format!(" {t}")).unwrap_or_default();
            let text = format!(
                "[k2 vault] your owner accepted {landed} from {} → your vault ({}, delivered to you as {d}{}). Use: {}.{more}",
                r.peer_addr,
                reference.as_deref().unwrap_or("k2 vault list"),
                expiry_words(got.meta.as_ref().and_then(|m| m.expires_at.as_deref())),
                handoff::use_hint_for(d, &landed, sites)
            );
            let sender = crate::hire_routes::Sender { workspace_id: None, via: "owner", actor: stamp.clone(), on_behalf_of: None };
            let _ = crate::hire_routes::deliver_recorded(&path, &text, "k2 vault", "", true, crate::workspace_msg::DEFAULT_WAKE_TIMEOUT, "msg", &sender);
        }
    }
    // Bug 2: the accept answer shows the copy's effective delivery,
    // ceiling and expiry (after a Replace too).
    Ok(json!({
        "ok": true,
        "delivery": view(vault, r),
        "ref": reference,
        "alwaysAdded": always_added,
        "secret": got.meta.as_ref().map(landed_json),
        "more": got.meta.as_ref().and_then(|m| accept_more(vault, m)),
    }))
}

/// The accept notice's extra line (A, 2026-10-30): a person accepted `m`
/// at a ceiling above its delivery, so its agents may take more without
/// asking. `None` when there is nothing more to take.
pub(crate) fn accept_more(vault: &LocalVault, m: &k2_vault::SecretMeta) -> Option<String> {
    let ceiling = vault.handoff_accepted_ceiling(m)?;
    if ceiling.level() <= m.default_delivery.level() || m.scope.kind != k2_vault::ScopeKind::Workspace {
        return None;
    }
    Some(format!(
        "Accepting it lets you take it up to {ceiling} yourself, no owner needed: k2 vault grant {} --to ws:{} --as {ceiling}.",
        m.name,
        crate::vault::workspace_label(&m.scope.id)
    ))
}

// ── accept-from ─────────────────────────────────────────────────────

fn entries_json(vault: &LocalVault, ws: Option<&str>) -> Result<Value, VaultError> {
    let list = vault.accept_from(ws)?;
    Ok(json!(list
        .iter()
        .map(|a| {
            let label = a.sender.strip_prefix("ws:").map(crate::vault::workspace_label);
            json!({ "workspaceId": a.workspace_id, "sender": a.sender, "senderLabel": label, "addedBy": a.added_by, "addedAt": a.added_at })
        })
        .collect::<Vec<_>>()))
}

fn accept_from_get(vault: &LocalVault, c: &VaultCaller, params: &HashMap<String, String>) -> Result<Value, VaultError> {
    let ws = workspace_param(c, params)?;
    Ok(json!({ "entries": entries_json(vault, ws.as_deref())? }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcceptFromBody {
    workspace: Option<String>,
    sender: Option<String>,
    #[serde(default)]
    action: String,
}

fn accept_from_post(vault: &LocalVault, c: &VaultCaller, body: &[u8], ingress: &str) -> Result<Value, VaultError> {
    let stamp = c.actor.stamp();
    let b: AcceptFromBody = parse(body)?;
    if c.is_passport() || !matches!(c.actor, Actor::Owner | Actor::Login { .. }) {
        let e = VaultError::forbidden("handoff_person_only", "only a person on this server edits who an agent accepts secrets from (H19)");
        audit(&stamp, &format!("deny accept-from {}: {}", b.action, e.code), ingress);
        return Err(e);
    }
    let w = b.workspace.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| VaultError::bad_request("workspace_required", "which agent? (--workspace <name>)"))?;
    let ws = crate::vault::workspace_id(w).ok_or_else(|| VaultError::not_found(format!("no workspace named {w}")))?;
    let raw = b.sender.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| VaultError::bad_request("sender_required", "who? ws:<agent>, team:<group id> or agent::host"))?;
    // A bare local name means that local agent.
    let sender = if !raw.contains(':') && !raw.contains('@') {
        format!("ws:{}", crate::vault::workspace_id(raw).ok_or_else(|| VaultError::not_found(format!("no agent named {raw} on this server")))?)
    } else if let Some(n) = raw.strip_prefix("ws:") {
        format!("ws:{}", crate::vault::workspace_id(n).unwrap_or_else(|| n.to_string()))
    } else {
        raw.to_string()
    };
    let label = crate::vault::workspace_label(&ws);
    match b.action.as_str() {
        "add" | "" => {
            let a = vault.accept_from_add(&ws, &sender, &c.actor)?;
            audit(&stamp, &format!("ok accept-from add {} for {label}", a.sender), ingress);
        }
        "remove" => {
            let removed = vault.accept_from_remove(&ws, &sender, &c.actor)?;
            audit(&stamp, &format!("ok accept-from remove {sender} for {label}{}", if removed { "" } else { " (was not listed)" }), ingress);
        }
        _ => return Err(VaultError::bad_request("bad_action", "add or remove")),
    }
    Ok(json!({ "ok": true, "entries": entries_json(vault, Some(&ws))? }))
}

// ── asks and unsend (H5) ────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AskBody {
    to: Option<String>,
    name: Option<String>,
    #[serde(default)]
    note: String,
    /// People: the asking workspace.
    workspace: Option<String>,
}

fn ask_row(id: &str, dir: &str, ws: &str, peer: &str, actor: &str, name: &str, note: &str) -> HandoffRecord {
    HandoffRecord {
        delivery_id: id.to_string(),
        direction: dir.into(),
        status: String::new(),
        local_workspace_id: ws.to_string(),
        peer_addr: peer.to_string(),
        peer_fp: None,
        sender_actor: actor.to_string(),
        source_vault_id: None,
        name: name.to_string(),
        landed_name: None,
        kind: "api_key".into(),
        note: note.to_string(),
        reply_to: None,
        item: HandoffItem {
            name: name.to_string(),
            as_name: None,
            kind: "api_key".into(),
            hosts: vec![],
            inject: None,
            description: String::new(),
            delivery: "fetch".into(),
            ceiling: "env".into(),
            human_given: false,
            ttl_secs: None,
        },
        secret_id: None,
        version: None,
        reason: None,
        card: None,
        created_at: String::new(),
        decided_at: None,
        decided_by: None,
        expires_at: None,
    }
}

/// The ask's stamp text (names, the note, how to answer). Never a value.
pub(crate) fn ask_text(id: &str, asker: &str, name: &str, note: &str, reply_to: &str) -> String {
    let note = if note.is_empty() { String::new() } else { format!(": \"{note}\"") };
    format!("[k2 vault ask {id}] {asker} asks for {name}{note}. To give it: k2 vault send {name} --to {reply_to} --reply {id}")
}

/// `POST /cli/vault/ask` (§4.3): record the ask and knock the asked agent
/// with a stamp. Cross server it travels as a plain signed federation
/// message (it holds no secret).
fn ask(vault: &LocalVault, c: &VaultCaller, body: &[u8], ingress: &str) -> Result<Value, VaultError> {
    let b: AskBody = parse(body)?;
    let stamp = c.actor.stamp();
    let name = b.name.as_deref().map(str::trim).unwrap_or("");
    k2_vault::model::validate_name(name, true)?;
    let to = b.to.as_deref().map(str::trim).unwrap_or("");
    let target = parse_target(to)?;
    let asker_ws = match (c.workspace_id(), b.workspace.as_deref().map(str::trim).filter(|s| !s.is_empty())) {
        (Some(ws), _) => ws.to_string(),
        (None, Some(w)) => crate::vault::workspace_id(w).ok_or_else(|| VaultError::not_found(format!("no workspace named {w}")))?,
        (None, None) => return Err(VaultError::bad_request("workspace_required", "which agent asks? (--workspace <name>)")),
    };
    let note = crate::vault::redact_str(&handoff::check_note(&b.note, handoff::NOTE_MAX, b"")?);
    let asker = crate::vault::workspace_label(&asker_ws);
    let id = handoff::new_ask_id();
    let (peer_addr, delivered) = match target {
        Target::Local(token) => {
            let (target_path, target_id) =
                crate::comms::resolve_target_project(&token).ok_or_else(|| VaultError::not_found(format!("no agent named {token} on this server")))?;
            let label = crate::vault::workspace_label(&target_id);
            if let Some(caller_ws) = c.workspace_id() {
                if let Err(e) = crate::comms::require_local_peer(caller_ws, &target_id, &label) {
                    return Err(VaultError::forbidden("not_connected", e.hint().to_string()));
                }
            }
            vault.handoff_record_ask(ask_row(&id, "ask_out", &asker_ws, &label, &stamp, name, &note))?;
            vault.handoff_record_ask(ask_row(&id, "ask_in", &target_id, &asker, &stamp, name, &note))?;
            let text = ask_text(&id, &asker, name, &note, &asker);
            let (resp, _) = crate::hire_routes::deliver_recorded(
                &target_path,
                &text,
                &asker,
                "",
                true,
                crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
                "msg",
                &sender_for(c, &asker_ws),
            );
            (label, resp.success)
        }
        Target::Remote { agent, host, .. } => crate::federation_vault::ask_remote(vault, c, &asker_ws, &asker, &agent, &host, &id, name, &note)?,
    };
    audit(&stamp, &format!("ok ask {name} from {peer_addr} {id}"), ingress);
    Ok(json!({ "ok": true, "askId": id, "name": name, "to": peer_addr, "delivered": delivered, "expiresInDays": handoff::PENDING_DAYS }))
}

fn asks(vault: &LocalVault, c: &VaultCaller, params: &HashMap<String, String>) -> Result<Value, VaultError> {
    let ws = workspace_param(c, params)?;
    let mut items = vec![];
    for dir in ["ask_out", "ask_in"] {
        for r in vault.handoffs(ws.as_deref(), Some(dir))? {
            items.push(json!({
                "askId": r.delivery_id,
                "direction": if dir == "ask_out" { "out" } else { "in" },
                "status": r.status,
                "name": r.name,
                "peer": r.peer_addr,
                "note": r.note,
                "workspace": crate::vault::workspace_label(&r.local_workspace_id),
                "createdAt": r.created_at,
                "expiresAt": r.expires_at,
            }));
        }
    }
    Ok(json!({ "items": items }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnsendBody {
    id: Option<String>,
}

/// `POST /cli/vault/unsend` (§9): the sender takes a delivery back. Same
/// server: this vault's copy; cross server: a signed revoke to the peer.
/// Unsend is not a revoke: rotate the key at the provider for that.
fn unsend(vault: &LocalVault, c: &VaultCaller, body: &[u8], ingress: &str) -> Result<Value, VaultError> {
    let b: UnsendBody = parse(body)?;
    let id = b.id.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| VaultError::bad_request("id_required", "which delivery? (k2 vault sent)"))?;
    let stamp = c.actor.stamp();
    let out = vault.handoff_get(id, "out")?;
    let mine = match c.workspace_id() {
        Some(ws) => ws == out.local_workspace_id,
        None => c.actor.is_admin_human(),
    };
    if !mine {
        let e = VaultError::forbidden("not_yours", "only the agent that sent a delivery (or an Owner/Admin) takes it back");
        audit(&stamp, &format!("deny unsend {} {id}: {}", out.name, e.code), ingress);
        return Err(e);
    }
    let (disabled, kept) = if out.peer_fp.is_some() {
        crate::federation_vault::unsend_remote(&out)?
    } else {
        let before = vault.handoff_get(id, "in")?;
        let (r, disabled) = vault.handoff_unsend(id, None, &stamp)?;
        if let Some(card) = before.card.as_deref() {
            crate::overlay_routes::close_handoff_card(card, "unsent", None, None);
        }
        (disabled, r.reason.as_deref() == Some("unsent_kept_replaced"))
    };
    vault.handoff_update_out(id, "unsent", Some(if kept { "unsent_kept_replaced" } else { "unsent" }))?;
    audit(&stamp, &format!("ok unsend {} to {} {id}{}", out.name, out.peer_addr, if disabled { " (copy disabled)" } else { "" }), ingress);
    Ok(json!({
        "ok": true,
        "deliveryId": id,
        "disabled": disabled,
        "keptReplaced": kept,
        "honest": "unsend is not a revoke: the receiver may already have used the key; rotate it at the provider to be sure",
    }))
}

// ── expiry (called from the vault ticker) ───────────────────────────

/// The vault ticker's handoff step (§9, H13): expire pending copies, close
/// their cards, and write the `expire` audit lines.
pub(crate) fn tick(vault: &LocalVault) {
    match vault.handoff_tick() {
        Ok(expired) => {
            for r in expired {
                if let Some(card) = r.card.as_deref() {
                    crate::overlay_routes::close_handoff_card(card, "expired", None, None);
                }
                if vault.handoff_get(&r.delivery_id, "out").is_ok() {
                    let _ = vault.handoff_update_out(&r.delivery_id, "expired", None);
                }
                audit("system:expiry", &format!("ok expire {} from {} {}", r.name, r.peer_addr, r.delivery_id), "-");
            }
        }
        Err(e) => k2_core::log_debug!("[vault-handoff] tick: {}", e.code),
    }
}

#[cfg(test)]
#[path = "vault_handoff_routes_tests.rs"]
mod tests;
