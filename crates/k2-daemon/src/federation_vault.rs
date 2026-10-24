//! Cross-server secret handoff (`prd-agent-secret-handoff-v1` §5.2, vs-live
//! H1, H3–H9, H16, H17).
//!
//! Sender (`send_remote`, called by `POST /cli/vault/send`):
//!   1. learn the peer's vault recipient through the signed
//!      `POST /cli/federation/vault-recipient` (cached in vault.db
//!      `peer_vault` for 24 hours; a 404 means an old or federation-off
//!      peer → `peer_too_old`, and nothing is sent: no plaintext fallback);
//!   2. seal `k2-vault-handoff/1` to that recipient (the federation key is
//!      signing-only, H1);
//!   3. post it on `POST /cli/federation/vault-handoff` inside the normal
//!      ECDSA federation envelope, synchronously, with no outbox (H7).
//!      Never `/inbound` (H4).
//!
//! Receiver (`handle_vault_recipient`, `handle_vault_handoff`): the body is
//! capped by the dispatcher (H8); `federation::ingest` authenticates the
//! peer; the seal must name this vault, this server, the authenticated
//! peer and the outer delivery id (H5); the delivery id is replay-checked
//! durably (H9); the roster gate, the remote-instruct opt-in (R4) and the
//! receiver's own accept-from + connection rows (H16) decide land or pend.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use k2_core::awareness::{AgentAddress, AgentSignal, Delivery, SignalKind, WorkspaceId};
use k2_core::federation::{self, ingress, vault_recipient as vr, FederationPeer, PeerStore, PeerTrust};
use k2_vault::handoff::{self, HandoffPayload, HandoffRecord, Incoming, OpenExpect, Party, PeerVault};
use k2_vault::{LocalVault, VaultError};
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::vault::actor::VaultCaller;
use crate::vault_handoff_routes::{self as routes, Prepared, SendError};

/// The `Custom` kind of a signed unsend (§9, H5).
pub const REVOKE_KIND: &str = "vault-handoff-revoke/1";

/// Body cap on the two vault federation routes (§5.3, H8).
pub const MAX_BODY_BYTES: usize = 160 * 1024;
/// Handoffs a peer may send in an hour (H17).
pub const RATE_PER_PEER_HOUR: usize = 30;
/// A cached peer vault is refetched after this long.
const PEER_VAULT_TTL_SECS: i64 = 24 * 3600;
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);

fn reply(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "error": { "code": code, "hint": hint.into() }, "code": code }).to_string(),
    }
}

fn vault_reply(e: &VaultError) -> CliResponse {
    let status = match e.status {
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        422 => "422 Unprocessable Entity",
        429 => "429 Too Many Requests",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    };
    reply(status, e.code, e.hint.clone())
}

fn local_fp() -> String {
    federation::local_fingerprint().unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────
// Receiver
// ─────────────────────────────────────────────────────────────────────

/// `POST /cli/federation/vault-recipient`: a pinned, Trusted peer's signed
/// request → this vault's recipient in a statement signed by this
/// server's federation key and bound to the requester (H3).
pub fn handle_vault_recipient(body: &[u8]) -> CliResponse {
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return vault_reply(&e),
    };
    let store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return reply("500 Internal Server Error", "peer_store", e.to_string()),
    };
    let me = local_fp();
    let ingested = match ingress::ingest(body, &store, ingress::global_nonce_cache(), &me, ingress::DEFAULT_SKEW_SECS) {
        Ok(v) => v,
        Err(e) => return crate::federation_routes::ingress_reject(e),
    };
    match &ingested.signal.kind {
        SignalKind::Custom { kind, .. } if kind == vr::REQUEST_KIND => {}
        _ => return reply("400 Bad Request", "wrong_kind", "this route answers vault-recipient/1 requests only"),
    }
    let recipient = match vault.carry_recipient() {
        Ok(r) => r.encode(),
        Err(e) => return vault_reply(&e),
    };
    let key = match federation::identity::load_or_create() {
        Ok(k) => k,
        Err(e) => return reply("500 Internal Server Error", "identity", e),
    };
    let statement = vr::Statement {
        v: vr::VERSION,
        fingerprint: me,
        to_fingerprint: ingested.peer_fingerprint.clone(),
        vault_id: vault.vault_id().to_string(),
        recipient,
        features: vec![handoff::FEATURE.to_string(), handoff::FEATURE_SIGN.to_string()],
        issued_at: chrono::Utc::now().to_rfc3339(),
    };
    match vr::sign(&statement, &key) {
        Ok(s) => CliResponse::ok_json(serde_json::to_string(&s).unwrap_or_default()),
        Err(e) => reply("500 Internal Server Error", "sign", e),
    }
}

/// Per-peer sliding window (H17).
fn rate_ok(peer_fp: &str) -> bool {
    static W: OnceLock<Mutex<HashMap<String, VecDeque<Instant>>>> = OnceLock::new();
    let mut m = W.get_or_init(|| Mutex::new(HashMap::new())).lock();
    let q = m.entry(peer_fp.to_string()).or_default();
    let now = Instant::now();
    while q.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(3600)) {
        q.pop_front();
    }
    if q.len() >= RATE_PER_PEER_HOUR {
        return false;
    }
    q.push_back(now);
    true
}

/// `POST /cli/federation/vault-handoff`: a sealed handoff in a signed
/// envelope (§5.2 step 3). Answers `{status, deliveryId, name, landedName,
/// reason}`; refusals carry a stable `code` and leave a `deny receive` line.
pub fn handle_vault_handoff(body: &[u8]) -> CliResponse {
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return vault_reply(&e),
    };
    let store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return reply("500 Internal Server Error", "peer_store", e.to_string()),
    };
    let me = local_fp();
    let ingested = match ingress::ingest(body, &store, ingress::global_nonce_cache(), &me, ingress::DEFAULT_SKEW_SECS) {
        Ok(v) => v,
        Err(e) => return crate::federation_routes::ingress_reject(e),
    };
    let peer_fp = ingested.peer_fingerprint.clone();
    let host = store.get(&peer_fp).map(crate::federation_routes::peer_host).unwrap_or_default();
    let signal = ingested.signal;
    let payload = match &signal.kind {
        SignalKind::Custom { kind, payload } if kind == vr::HANDOFF_KIND => payload.clone(),
        SignalKind::Custom { kind, payload } if kind == REVOKE_KIND => return receive_revoke(&vault, &peer_fp, &host, &signal, payload),
        _ => return reply("400 Bad Request", "wrong_kind", "this route takes vault-handoff/1 and vault-handoff-revoke/1 only"),
    };
    let sender_agent = match &signal.from {
        AgentAddress::Agent { name, .. } => name.trim().to_ascii_lowercase(),
        _ => "peer".to_string(),
    };
    let peer_addr = crate::federation_routes::federated_from_label(&sender_agent, &host);
    let deny = |code: &str| {
        routes::audit(&format!("handoff:{peer_addr}"), &format!("deny receive from {peer_addr}: {code}"), "federation");
        vault.handoff_audit("receive", &format!("handoff:{peer_addr}"), &peer_addr, &format!("deny:{code}"));
    };
    if !rate_ok(&peer_fp) {
        deny("rate_limited");
        return reply("429 Too Many Requests", "rate_limited", "too many handoffs from your server this hour (30)");
    }
    let delivery_id = payload.get("deliveryId").and_then(Value::as_str).unwrap_or_default().to_string();
    let to_ws = payload.get("toWorkspaceId").and_then(Value::as_str).unwrap_or_default().to_string();
    let sealed = match payload
        .get("sealedB64")
        .and_then(Value::as_str)
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s.trim()).ok())
    {
        Some(b) => b,
        None => {
            deny("bad_handoff");
            return reply("400 Bad Request", "bad_handoff", "sealedB64 is missing or not base64");
        }
    };
    let (sig_ws, to_name) = match &signal.to {
        AgentAddress::Agent { workspace, name } => (workspace.0.clone(), name.clone()),
        _ => (String::new(), String::new()),
    };
    if to_ws.is_empty() || sig_ws != to_ws {
        deny("bad_handoff");
        return reply("400 Bad Request", "bad_handoff", "the envelope and the payload name different workspaces");
    }
    let p: HandoffPayload = match vault.handoff_open(&sealed, OpenExpect { local_fingerprint: &me, peer_fingerprint: &peer_fp, delivery_id: &delivery_id }) {
        Ok(p) => p,
        Err(e) => {
            deny(e.code);
            return vault_reply(&e);
        }
    };
    if p.to.workspace_id != to_ws {
        deny("bad_handoff");
        return reply("400 Bad Request", "bad_handoff", "the sealed workspace isn't the addressed one");
    }
    let Some(path) = crate::federation_routes::resolve_project_path_by_id(&to_ws) else {
        deny("unknown_agent");
        return crate::federation_routes::unknown_agent_response();
    };
    if !federation::roster::accepts_inbound(&to_ws, &path, &to_name) {
        deny("unknown_agent");
        return crate::federation_routes::unknown_agent_response();
    }
    let value = match p.value() {
        Ok(v) => v,
        Err(e) => {
            deny(e.code);
            return vault_reply(&e);
        }
    };
    // A hostile peer could put the value in the note (which reaches the
    // agent's chat): refuse it the same way the sender does (H14).
    let note = match handoff::check_note(&p.note, handoff::MSG_NOTE_MAX, value.expose()) {
        Ok(n) => crate::vault::redact_str(&n),
        Err(e) => {
            deny(e.code);
            return vault_reply(&e);
        }
    };
    let opt_in = k2_core::workspace::settings::remote_instruct_allowed_for_path(&path);
    // H16: auto-land also needs the receiving workspace's own connection.
    let connected = k2_core::connections::is_remote_connection(&path, &peer_addr);
    let listed = vault.accepts_from(&to_ws, &[peer_addr.clone()]);
    let replied = routes::reply_matches(&vault, &to_ws, p.reply_to.as_deref(), &peer_addr, &p.item.name);
    // The peer's own stamp for who sent it (shown, never trusted).
    let actor = if p.from.actor.is_empty() {
        format!("peer:{peer_addr}")
    } else {
        format!("{}@{host}", p.from.actor.chars().filter(|c| !c.is_control()).take(200).collect::<String>())
    };
    let handle = if p.from.handle.is_empty() { sender_agent.clone() } else { p.from.handle.clone() };
    let inc = Incoming {
        delivery_id: delivery_id.clone(),
        workspace_id: to_ws.clone(),
        peer_addr: peer_addr.clone(),
        peer_fp: Some(peer_fp.clone()),
        sender_actor: actor,
        source_vault_id: Some(p.from.vault_id.clone()),
        item: p.item.clone(),
        note,
        reply_to: p.reply_to.clone(),
        auto_land: (listed && connected) || replied,
        remote_off: !opt_in,
        sender_label: format!("{handle} on {host}"),
    };
    let got = match vault.handoff_receive(inc, value) {
        Ok(g) => g,
        Err(e) => {
            deny(e.code);
            return vault_reply(&e);
        }
    };
    let rec = got.record;
    routes::close_answered_ask(&vault, &rec, replied);
    let landed_name = rec.landed_name.clone().unwrap_or_else(|| rec.name.clone());
    let label = crate::vault::workspace_label(&to_ws);
    routes::audit(
        &format!("handoff:{peer_addr}"),
        &format!("ok receive {landed_name} from {peer_addr} into {label} {delivery_id} {}", rec.status),
        "federation",
    );
    let reference = got.meta.as_ref().and_then(crate::vault::display_ref);
    let sender = crate::hire_routes::Sender { workspace_id: None, via: "federation", actor: peer_addr.clone(), on_behalf_of: None };
    routes::after_receive(&vault, &rec, &path, &to_ws, &peer_addr, Some(&host), reference.as_deref(), opt_in, sender);
    CliResponse::ok_json(
        json!({
            "status": rec.status,
            "deliveryId": rec.delivery_id,
            "name": rec.name,
            "landedName": rec.landed_name,
            "reason": rec.reason,
        })
        .to_string(),
    )
}

/// A peer takes back a delivery it sent us (§9): only the peer the row
/// came from; the copy is disabled if it is still the version that
/// delivery wrote; a pending copy is dropped and its card closed.
fn receive_revoke(vault: &LocalVault, peer_fp: &str, host: &str, signal: &AgentSignal, payload: &Value) -> CliResponse {
    let id = payload.get("deliveryId").and_then(Value::as_str).unwrap_or_default();
    let sender_agent = match &signal.from {
        AgentAddress::Agent { name, .. } => name.trim().to_ascii_lowercase(),
        _ => "peer".to_string(),
    };
    let peer_addr = crate::federation_routes::federated_from_label(&sender_agent, host);
    let before = match vault.handoff_get(id, "in") {
        Ok(r) => r,
        Err(e) => return vault_reply(&e),
    };
    match vault.handoff_unsend(id, Some(peer_fp), &format!("handoff:{peer_addr}")) {
        Ok((r, disabled)) => {
            if let Some(card) = before.card.as_deref() {
                crate::overlay_routes::close_handoff_card(card, "unsent", None, None);
            }
            routes::audit(
                &format!("handoff:{peer_addr}"),
                &format!("ok unsend {} from {peer_addr} {id}{}", r.name, if disabled { " (copy disabled)" } else { "" }),
                "federation",
            );
            CliResponse::ok_json(json!({ "status": "unsent", "deliveryId": id, "disabled": disabled, "reason": r.reason }).to_string())
        }
        Err(e) => {
            routes::audit(&format!("handoff:{peer_addr}"), &format!("deny unsend {id} from {peer_addr}: {}", e.code), "federation");
            vault_reply(&e)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Sender
// ─────────────────────────────────────────────────────────────────────

fn client() -> Result<reqwest::blocking::Client, VaultError> {
    reqwest::blocking::Client::builder()
        // A peer must never steer us to another host.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(DIAL_TIMEOUT)
        .build()
        .map_err(|e| VaultError::internal(format!("http client: {e}")))
}

fn sign_signal(peer: &FederationPeer, signal: &AgentSignal) -> Result<Vec<u8>, VaultError> {
    let key = federation::identity::load_or_create().map_err(|e| VaultError::internal(format!("federation key: {e}")))?;
    federation::seal(signal, &key, &peer.subdomain, 8).map_err(|e| VaultError::internal(format!("seal envelope: {e}")))
}

fn too_old(host: &str) -> VaultError {
    VaultError::new(
        "peer_too_old",
        422,
        format!("{host} can't take secret handoffs yet (it needs K2 0.46.1 or later, with federation on); nothing was sent"),
    )
}

/// The peer's vault (cached 24 h, or fetched and verified now).
fn peer_vault(vault: &LocalVault, peer: &FederationPeer, host: &str, refresh: bool) -> Result<PeerVault, VaultError> {
    if !refresh {
        if let Some(pv) = vault.peer_vault(&peer.fingerprint) {
            let fresh = chrono::DateTime::parse_from_rfc3339(&pv.fetched_at)
                .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds() < PEER_VAULT_TTL_SECS)
                .unwrap_or(false);
            if fresh && pv.features.iter().any(|f| f == handoff::FEATURE) {
                return Ok(pv);
            }
        }
    }
    let base = crate::federation_routes::gated_peer_base_url(peer).map_err(|e| VaultError::forbidden("not_connected", e))?;
    let me = local_fp();
    let signal = AgentSignal {
        delivery: Delivery::Inbox,
        ..AgentSignal::new(
            AgentAddress::Agent { workspace: WorkspaceId(format!("peer:{me}")), name: "vault".into() },
            AgentAddress::Agent { workspace: WorkspaceId(format!("peer:{}", peer.fingerprint)), name: "vault".into() },
            SignalKind::Custom { kind: vr::REQUEST_KIND.to_string(), payload: json!({}) },
        )
    };
    let bytes = sign_signal(peer, &signal)?;
    let url = format!("{base}/cli/federation/vault-recipient");
    let resp = client()?
        .post(&url)
        .header("Content-Type", "application/json")
        .body(bytes)
        .send()
        .map_err(|e| VaultError::new("peer_unreachable", 502, format!("{host} didn't answer: {e}")))?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    if status == 404 {
        return Err(too_old(host));
    }
    if status != 200 {
        return Err(VaultError::new("peer_refused", 502, format!("{host} refused the vault-recipient request (HTTP {status})")));
    }
    let signed: vr::SignedStatement = serde_json::from_str(&text).map_err(|_| too_old(host))?;
    vr::verify(&signed, &peer.public_key_pem, &peer.fingerprint, &me, handoff::FEATURE, chrono::Utc::now())
        .map_err(|e| VaultError::forbidden("bad_peer_statement", format!("{host}: {e}")))?;
    let s = signed.statement;
    let pv = PeerVault {
        peer_fp: peer.fingerprint.clone(),
        vault_id: s.vault_id,
        recipient: s.recipient,
        features: s.features,
        fetched_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Some(old) = vault.peer_vault_put(&pv)? {
        routes::audit(
            "system:handoff",
            &format!("ok peer-vault-changed {host} {} -> {} (signed by the pinned key)", old.vault_id, pv.vault_id),
            "-",
        );
    }
    Ok(pv)
}

/// What the receiver answered.
enum Answer {
    Ok(Value),
    Refused(u16, String, String),
}

fn post_handoff(base: &str, bytes: Vec<u8>, host: &str) -> Result<Answer, VaultError> {
    let url = format!("{base}/cli/federation/vault-handoff");
    let resp = client()?
        .post(&url)
        .header("Content-Type", "application/json")
        .body(bytes)
        .send()
        .map_err(|e| VaultError::new("peer_unreachable", 502, format!("{host} didn't answer: {e}")))?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status == 200 {
        return Ok(Answer::Ok(v));
    }
    let code = v
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| v.get("error").and_then(|e| e.get("code")).and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let hint = v
        .get("error")
        .and_then(|e| e.get("hint").and_then(Value::as_str).or_else(|| e.as_str()))
        .unwrap_or("")
        .to_string();
    Ok(Answer::Refused(status, code, hint))
}

/// A receiver's refusal as this server's error (codes we know stay stable).
fn refused_error(status: u16, code: &str, hint: &str, host: &str) -> VaultError {
    let h = format!("{host}: {}", if hint.is_empty() { code } else { hint });
    match code {
        "unknown_agent" => VaultError::not_found(h),
        "too_many_pending" => VaultError::new("too_many_pending", 429, h),
        "rate_limited" => VaultError::new("rate_limited", 429, h),
        "replay" => VaultError::conflict("replay", h),
        "never_sendable" => VaultError::new("never_sendable", 422, h),
        "note_has_value" => VaultError::new("note_has_value", 422, h),
        "handoff_expired" => VaultError::new("handoff_expired", 409, h),
        _ if status == 404 => too_old(host),
        _ if status == 403 => VaultError::forbidden("not_connected", h),
        _ => VaultError::new("peer_refused", 502, h),
    }
}

/// Send a prepared secret to `agent::host` (§5.2). Gates in order:
/// federation on; a pinned Trusted peer; the peer's signed vault recipient
/// (an old or federation-off peer stops here with `peer_too_old`, before
/// anyone is asked to confirm); a person for a person-given secret (R3,
/// with a vault confirm); the roster resolves the agent; the sender's
/// connection row (`remote_send_gate`) for an agent caller; then seal and
/// post synchronously.
#[allow(clippy::too_many_arguments)]
pub(crate) fn send_remote(
    vault: &Arc<LocalVault>,
    c: &VaultCaller,
    agent: &str,
    host: &str,
    prep: Prepared,
    reply_to: Option<String>,
    confirm_token: Option<&str>,
    ingress: &str,
) -> Result<Value, SendError> {
    if !federation::enabled() {
        return Err(VaultError::new(
            "federation_off",
            422,
            "federation is off on this server; turn it on and pair with that server first (k2 fed enable, k2 fed pair)",
        )
        .into());
    }
    let store = PeerStore::load().map_err(|e| VaultError::internal(format!("peer store: {e}")))?;
    let peer = store
        .list()
        .iter()
        .find(|p| crate::federation_routes::peer_matches_user_host(p, host))
        .cloned()
        .ok_or_else(|| VaultError::not_found(format!("no paired server matches {host} (k2 fed peers)")))?;
    if peer.trust != PeerTrust::Trusted {
        return Err(VaultError::forbidden("not_connected", format!("{host} isn't trusted yet; confirm the pairing first (k2 fed confirm)")).into());
    }
    let peer_host = crate::federation_routes::peer_host(&peer);
    let stamp = c.actor.stamp();
    let Prepared { item, value, note, sender_ws, source } = prep;
    // Can it take a handoff at all? An old or federation-off peer 404s
    // here: peer_too_old, before anything else is asked of anyone.
    let mut pv = peer_vault(vault, &peer, host, false)?;
    // Item 2: a signed secret goes only to a server that keeps the signer
    // (an older one would paste the value). The cached statement may
    // predate the peer's update, so ask again once before refusing.
    if item.inject.as_ref().is_some_and(|i| i.sign.is_some()) && !pv.features.iter().any(|f| f == handoff::FEATURE_SIGN) {
        pv = peer_vault(vault, &peer, host, true)?;
        if !pv.features.iter().any(|f| f == handoff::FEATURE_SIGN) {
            return Err(VaultError::new(
                "peer_too_old_for_signed",
                422,
                format!("{host} can't take a signed secret yet (it needs K2 0.46.4 or later); nothing was sent"),
            )
            .into());
        }
    }
    // R3: a person-given secret needs a person to send it, with the vault
    // (checked after the peer proved it can take a handoff, so nobody
    // confirms a send that can't happen)
    // confirm (a reveal-class widening to another authority, §7.6).
    if item.human_given {
        if let Some((secret_id, _)) = &source {
            if c.is_passport() || !c.actor.is_admin_human() {
                let hint = format!(
                    "a person gave {}, so a person on this server must send it to another server: k2 vault send {} --to {agent}::{host} from your own terminal (it asks for a vault confirm)",
                    item.name, item.name
                );
                return Err(SendError::Response(
                    "held",
                    CliResponse {
                        status: "403 Forbidden",
                        content_type: "application/json",
                        body: json!({ "error": { "code": "held", "hint": hint }, "status": "held", "who": "a person on this server must send it" }).to_string(),
                    },
                ));
            }
            match confirm_token {
                Some(t) => {
                    vault.confirm.take(t, crate::vault_routes::HANDOFF_CONFIRM_ACTION, Some(secret_id), &stamp)?;
                }
                None => {
                    return Err(SendError::Response(
                        "confirm_required",
                        CliResponse {
                            status: "403 Forbidden",
                            content_type: "application/json",
                            body: json!({
                                "error": { "code": "confirm_required", "hint": "a person gave this secret; confirm sending it to another server (vault confirm: handoff-send)" },
                                "action": crate::vault_routes::HANDOFF_CONFIRM_ACTION,
                                "secretId": secret_id,
                            })
                            .to_string(),
                        },
                    ));
                }
            }
        }
    }
    // The agent on that server (its roster), by workspace id (CL12).
    let roster = crate::federation_routes::handle_peer_roster(&peer.fingerprint);
    if !roster.status.starts_with("200") {
        return Err(VaultError::new("peer_unreachable", 502, format!("{host}'s roster didn't answer ({})", roster.status)).into());
    }
    let to_ws = crate::federation_routes::workspace_id_from_roster_body(&roster.body, agent).map_err(VaultError::not_found)?;
    // The sender's connection gate (an agent may send only where its
    // workspace is connected), as `k2 msg agent::host`.
    if let Some(ws) = c.workspace_id() {
        let path = crate::vault::workspace_path(ws).unwrap_or_else(|| ws.to_string());
        if let Err(resp) = crate::federation_routes::remote_send_gate_check(&path, &peer, &to_ws, agent) {
            return Err(SendError::Response("not_connected", resp));
        }
    }
    let base = crate::federation_routes::gated_peer_base_url(&peer).map_err(|e| VaultError::forbidden("not_connected", e))?;
    let me = local_fp();
    let (from_ws, from_handle) = match &sender_ws {
        Some(ws) => (ws.clone(), crate::vault::workspace_label(ws).to_ascii_lowercase()),
        None => (format!("peer:{me}"), "owner".to_string()),
    };
    let my_host = k2_core::tunnel::config::load().map(|c| c.subdomain).unwrap_or_default();
    let delivery_id = handoff::new_delivery_id();
    let to_addr = format!("{agent}::{peer_host}");
    let landed_name = item.landed_name().to_string();
    let mut attempt = 0;
    let answer = loop {
        attempt += 1;
        let from = Party {
            fingerprint: me.clone(),
            host: my_host.clone(),
            workspace_id: from_ws.clone(),
            handle: from_handle.clone(),
            actor: stamp.clone(),
            vault_id: String::new(),
        };
        let to = Party {
            fingerprint: peer.fingerprint.clone(),
            host: peer_host.clone(),
            workspace_id: to_ws.clone(),
            handle: agent.to_string(),
            actor: String::new(),
            vault_id: pv.vault_id.clone(),
        };
        let recipient = k2_vault::agefmt::Recipient::parse(&pv.recipient)?;
        let mut payload = HandoffPayload::new(&delivery_id, from, to, item.clone(), &value, note.clone(), reply_to.clone());
        let sealed = vault.handoff_seal(&recipient, &mut payload)?;
        drop(payload);
        let signal = AgentSignal {
            delivery: Delivery::Inbox,
            ..AgentSignal::new(
                AgentAddress::Agent { workspace: WorkspaceId(from_ws.clone()), name: from_handle.clone() },
                AgentAddress::Agent { workspace: WorkspaceId(to_ws.clone()), name: agent.to_string() },
                SignalKind::Custom {
                    kind: vr::HANDOFF_KIND.to_string(),
                    payload: json!({
                        "deliveryId": delivery_id,
                        "toWorkspaceId": to_ws,
                        "sealedB64": base64::engine::general_purpose::STANDARD.encode(&sealed),
                    }),
                },
            )
        };
        let bytes = sign_signal(&peer, &signal)?;
        match post_handoff(&base, bytes, host) {
            Ok(Answer::Refused(_, code, _)) if code == "carry_not_for_this_vault" && attempt == 1 => {
                // The peer's vault changed: learn it again, once.
                vault.peer_vault_forget(&peer.fingerprint)?;
                pv = peer_vault(vault, &peer, host, true)?;
                continue;
            }
            other => break other,
        }
    };
    let mk_out = |status: &str, reason: Option<String>| HandoffRecord {
        delivery_id: delivery_id.clone(),
        direction: "out".into(),
        status: status.to_string(),
        local_workspace_id: sender_ws.clone().unwrap_or_else(|| "owner".into()),
        peer_addr: to_addr.clone(),
        peer_fp: Some(peer.fingerprint.clone()),
        sender_actor: stamp.clone(),
        source_vault_id: Some(vault.vault_id().to_string()),
        name: item.name.clone(),
        landed_name: Some(landed_name.clone()),
        kind: item.kind.clone(),
        note: note.clone(),
        reply_to: reply_to.clone(),
        item: item.clone(),
        secret_id: source.as_ref().map(|s| s.0.clone()),
        version: source.as_ref().map(|s| s.1),
        reason,
        card: None,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        decided_at: None,
        decided_by: None,
        expires_at: None,
    };
    let src = source.as_ref().map(|(i, v)| (i.as_str(), *v));
    match answer {
        Err(e) if e.code == "peer_unreachable" => {
            // The receiver's durable replay check makes a retry with the
            // same id safe; we record `unknown` and say so.
            vault.handoff_record_out(mk_out("unknown", Some("peer_unreachable".into())), src)?;
            routes::audit(&stamp, &format!("deny send {} to {to_addr} {delivery_id}: peer_unreachable (status unknown)", item.name), ingress);
            Err(e.into())
        }
        Err(e) => Err(e.into()),
        Ok(Answer::Refused(status, code, hint)) => {
            let e = refused_error(status, &code, &hint, host);
            vault.handoff_record_out(mk_out("refused", Some(e.code.to_string())), src)?;
            Err(e.into())
        }
        Ok(Answer::Ok(v)) => {
            let status = v.get("status").and_then(Value::as_str).unwrap_or("unknown").to_string();
            let reason = v.get("reason").and_then(Value::as_str).map(str::to_string);
            let out = vault.handoff_record_out(mk_out(&status, reason), src)?;
            routes::audit(&stamp, &format!("ok send {} to {to_addr} {delivery_id} {status}", item.name), ingress);
            Ok(routes::send_answer(&out, &to_addr, true, None))
        }
    }
}

/// A pinned, Trusted peer by the host the caller typed.
fn trusted_peer(host: &str) -> Result<FederationPeer, VaultError> {
    if !federation::enabled() {
        return Err(VaultError::new("federation_off", 422, "federation is off on this server; turn it on and pair first"));
    }
    let store = PeerStore::load().map_err(|e| VaultError::internal(format!("peer store: {e}")))?;
    let peer = store
        .list()
        .iter()
        .find(|p| crate::federation_routes::peer_matches_user_host(p, host))
        .cloned()
        .ok_or_else(|| VaultError::not_found(format!("no paired server matches {host} (k2 fed peers)")))?;
    if peer.trust != PeerTrust::Trusted {
        return Err(VaultError::forbidden("not_connected", format!("{host} isn't trusted yet")));
    }
    Ok(peer)
}

/// `k2 vault ask agent::host NAME` (§4.3): the ask is a plain signed
/// federation message (no secret), sent like `k2 msg agent::host`, and an
/// `ask_out` row records who may answer. Returns (peer address, delivered).
#[allow(clippy::too_many_arguments)]
pub(crate) fn ask_remote(
    vault: &LocalVault,
    c: &VaultCaller,
    asker_ws: &str,
    asker: &str,
    agent: &str,
    host: &str,
    id: &str,
    name: &str,
    note: &str,
) -> Result<(String, bool), VaultError> {
    let peer = trusted_peer(host)?;
    let roster = crate::federation_routes::handle_peer_roster(&peer.fingerprint);
    if !roster.status.starts_with("200") {
        return Err(VaultError::new("peer_unreachable", 502, format!("{host}'s roster didn't answer ({})", roster.status)));
    }
    let to_ws = crate::federation_routes::workspace_id_from_roster_body(&roster.body, agent).map_err(VaultError::not_found)?;
    let handle = crate::federation_routes::handle_for_workspace_in_roster_body(&roster.body, &to_ws).unwrap_or_else(|| agent.to_string());
    let peer_addr = crate::federation_routes::federated_from_label(&handle, &crate::federation_routes::peer_host(&peer));
    let path = crate::vault::workspace_path(asker_ws).unwrap_or_else(|| asker_ws.to_string());
    vault.handoff_record_ask(ask_row_remote(id, asker_ws, &peer_addr, &c.actor.stamp(), name, note, &peer.fingerprint))?;
    let text = routes::ask_text(id, asker, name, note, "<the address in the [from …] line>");
    let body = json!({ "to": format!("{}::{to_ws}::{handle}", peer.fingerprint), "text": text, "from_workspace": path });
    let resp = crate::federation_routes::handle_send(body.to_string().as_bytes());
    if !resp.status.starts_with("200") {
        let _ = vault.handoff_close_ask(id, "ask_out", "refused");
        let v: Value = serde_json::from_str(&resp.body).unwrap_or(Value::Null);
        let code = v.get("error").and_then(|e| e.get("code")).and_then(Value::as_str).unwrap_or("");
        return Err(if code == "not_connected" || resp.status.starts_with("403") {
            VaultError::forbidden("not_connected", format!("{handle}::{host} isn't one of your connections (k2 connections add)"))
        } else {
            VaultError::new("peer_refused", 502, format!("{host} didn't take the ask ({})", resp.status))
        });
    }
    let v: Value = serde_json::from_str(&resp.body).unwrap_or(Value::Null);
    Ok((peer_addr, v.get("status").and_then(Value::as_str) == Some("sent")))
}

fn ask_row_remote(id: &str, ws: &str, peer_addr: &str, actor: &str, name: &str, note: &str, peer_fp: &str) -> HandoffRecord {
    let mut r = HandoffRecord {
        delivery_id: id.to_string(),
        direction: "ask_out".into(),
        status: String::new(),
        local_workspace_id: ws.to_string(),
        peer_addr: peer_addr.to_string(),
        peer_fp: Some(peer_fp.to_string()),
        sender_actor: actor.to_string(),
        source_vault_id: None,
        name: name.to_string(),
        landed_name: None,
        kind: "api_key".into(),
        note: note.to_string(),
        reply_to: None,
        item: handoff::HandoffItem {
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
    };
    r.reason = None;
    r
}

/// Unsend across servers (§9): a signed `vault-handoff-revoke/1` on the
/// vault-handoff route. Returns (copy disabled, kept because replaced).
pub(crate) fn unsend_remote(out: &HandoffRecord) -> Result<(bool, bool), VaultError> {
    let fp = out.peer_fp.as_deref().unwrap_or_default();
    let store = PeerStore::load().map_err(|e| VaultError::internal(format!("peer store: {e}")))?;
    let peer = store.get(fp).cloned().ok_or_else(|| VaultError::not_found("that server isn't paired any more"))?;
    let host = crate::federation_routes::peer_host(&peer);
    let base = crate::federation_routes::gated_peer_base_url(&peer).map_err(|e| VaultError::forbidden("not_connected", e))?;
    let from_handle = crate::vault::workspace_label(&out.local_workspace_id).to_ascii_lowercase();
    let signal = AgentSignal {
        delivery: Delivery::Inbox,
        ..AgentSignal::new(
            AgentAddress::Agent { workspace: WorkspaceId(out.local_workspace_id.clone()), name: from_handle },
            AgentAddress::Agent { workspace: WorkspaceId(format!("peer:{fp}")), name: "vault".into() },
            SignalKind::Custom { kind: REVOKE_KIND.to_string(), payload: json!({ "deliveryId": out.delivery_id }) },
        )
    };
    let bytes = sign_signal(&peer, &signal)?;
    match post_handoff(&base, bytes, &host)? {
        Answer::Ok(v) => Ok((
            v.get("disabled").and_then(Value::as_bool).unwrap_or(false),
            v.get("reason").and_then(Value::as_str) == Some("unsent_kept_replaced"),
        )),
        Answer::Refused(status, code, hint) => Err(refused_error(status, &code, &hint, &host)),
    }
}

#[cfg(test)]
#[path = "federation_vault_tests.rs"]
mod tests;
