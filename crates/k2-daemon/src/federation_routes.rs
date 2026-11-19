//! `/cli/federation/*` route handlers (Federation V1 functional core).
//!
//! SSOT: `.k2/prds/prd-cross-server-agent-comms.md` (Phases 2/3/4). The whole
//! surface is **dark by default** — the dispatcher 404s every
//! `/cli/federation/*` path unless [`k2_core::federation::enabled`]
//! (`K2_FEDERATION`, default OFF) is true — so a shipped 0.40.x build has zero
//! behavior change. These handlers wrap the k2-core primitives:
//!
//!   - [`handle_pair_request`] — P2 UNAUTH pair-request → creates only a
//!     `Pending` peer (fail-closed; promotion needs the owner SAS confirm).
//!   - [`handle_pair_confirm`] — P2 OWNER-gated SAS confirm → `Trusted` + caps.
//!   - [`handle_inbound`] — P3 the SECURITY CORE: verify → require_peer →
//!     replay/skew/ttl → sanitize → deliver to the canonical chat IFF the
//!     recipient opted into remote instruction, else DECLINE (never trusted).
//!   - [`handle_send`] — P4 seal an [`AgentSignal`], durably enqueue, dial the
//!     peer's E2E listener, POST to its `/cli/federation/inbound`.
//!   - [`handle_roster`] — P5 peer-facing roster: signed-challenge auth
//!     (`verify_roster_request` → `require_peer(fp,"roster")`) → this daemon's
//!     exposed agents (read-only, no spawn).
//!   - [`handle_peers`] / [`handle_peer_roster`] — P5 LOCAL owner-gated seams
//!     the renderer calls: list pinned peers, and fetch a paired peer's roster
//!     (the local daemon dials the peer's signed roster GET).
//!
//! The dispatcher owns method/auth gating (POST-only via `require_post`;
//! owner-or-admin on pair/confirm/outbox/pubkey; dual-auth owner-or-admin OR
//! scoped passport on peers/peer-roster/send — PR1; the inbound route is
//! authenticated by the ENVELOPE itself, not a token — DECISION-2).

use std::path::Path;

use crate::cli_response::CliResponse;

use k2_core::awareness::{AgentAddress, AgentSignal, Delivery, SignalKind, WorkspaceId};
use k2_core::federation::{self, ingress, outbox, pairing, roster, PeerStore};
use k2_core::workspace::handle::{roster_display_matches, HealReport, RosterEntry, RosterPeerScope};

/// Default hop budget stamped on outbound envelopes (Risk M2).
const OUTBOUND_TTL: u8 = 8;

/// Body cap on `/cli/federation/inbound` (H0 / H8 of
/// prd-agent-secret-handoff-v1): the largest tray package (1 MiB of
/// files, base64 in a signed envelope) fits with room to spare.
pub const INBOUND_MAX_BYTES: usize = 2 * 1024 * 1024;

/// `Custom` signal kinds that belong to the vault handoff routes. `/inbound`
/// refuses them (H4): a handoff must never be rendered as chat text.
pub const VAULT_KIND_PREFIX: &str = "vault-";

/// 400 `wrong_route` for a vault kind on `/inbound`.
pub(crate) fn wrong_route_response() -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: serde_json::json!({
            "error": "wrong_route: vault handoffs go to /cli/federation/vault-handoff, never to a chat",
            "code": "wrong_route",
        })
        .to_string(),
    }
}

fn json_err(status: &'static str, msg: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "error": msg.to_string() }).to_string(),
    }
}

// ─────────────────────────────────────────────────────────────────────
// P2 — pairing
// ─────────────────────────────────────────────────────────────────────

/// `POST /cli/federation/pair/request` — UNAUTH, the pre-0.45.2 (v1)
/// pairing. Since 0.45.2 it never writes the peer store: it answers the v1
/// code only for a peer already Trusted from a v1 pairing, and 409
/// `pairing_v2_required` for anything else (unknown key or a Pending row).
pub fn handle_pair_request(body: &[u8]) -> CliResponse {
    // label / subdomain / baseUrl in the body are ignored (0.45.2): this
    // route never writes an address.
    #[derive(serde::Deserialize)]
    struct Req {
        public_key_pem: String,
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("bad pair-request body: {e}")),
    };
    let local_fp = match federation::local_fingerprint() {
        Ok(fp) => fp,
        Err(e) => return json_err("500 Internal Server Error", format!("local fingerprint: {e}")),
    };
    let store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };
    // 0.45.2: a NEW pairing must use pairing v2 (commit-then-reveal code).
    // This v1 route only re-sees keys pinned before; a key it has not
    // pinned is refused, never downgraded to the v1 code.
    let presented_fp = match k2_core::federation::peers::fingerprint_of_spki_pem(&req.public_key_pem) {
        Ok(fp) => fp,
        Err(e) => return CliResponse::bad_request(format!("bad public key: {e}")),
    };
    // This public route NEVER writes the store. The only thing it still
    // answers is the v1 code for a peer already Trusted from a pairing made
    // before 0.45.2 (an old app re-confirming). A Pending row (v1 or one a v2
    // pairing just pinned) is never touched: rewriting its address here would
    // let anyone who knows the public key move it before the owner confirms.
    match store.get(&presented_fp) {
        Some(p)
            if p.trust == k2_core::federation::PeerTrust::Trusted
                && p.pairing_version < k2_core::federation::pairing_v2::PAIRING_VERSION =>
        {
            CliResponse::ok_json(
                serde_json::json!({
                    "fingerprint": presented_fp,
                    "sas": pairing::sas_code(&local_fp, &presented_fp),
                    "trust": "trusted",
                    "created": false,
                    "local_fingerprint": local_fp,
                    "pairing_version": p.pairing_version,
                })
                .to_string(),
            )
        }
        _ => CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: serde_json::json!({
                "error": "This server pairs with a newer, safer pairing code (K2 0.45.2 or later on \
                          both servers). Update the server or app that started the pairing, then pair again.",
                "code": "pairing_v2_required",
            })
            .to_string(),
        },
    }
}

/// `POST /cli/federation/pair/confirm` — OWNER-gated (dispatcher enforces).
/// The owner approves a `Pending` peer by fingerprint after comparing the
/// SAS. On a SAS match the peer flips `Trusted` and is granted its caps.
pub fn handle_pair_confirm(body: &[u8]) -> CliResponse {
    #[derive(serde::Deserialize)]
    struct Req {
        fingerprint: String,
        sas: String,
        #[serde(default)]
        capabilities: Vec<String>,
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("bad pair-confirm body: {e}")),
    };
    let local_fp = match federation::local_fingerprint() {
        Ok(fp) => fp,
        Err(e) => return json_err("500 Internal Server Error", format!("local fingerprint: {e}")),
    };
    // v2 code when a v2 pairing is waiting; the v1 code only for a peer
    // that is already Trusted from a pairing before 0.45.2.
    let tx = PeerStore::transact(|store| {
        let kind = k2_core::federation::pairing_v2::confirm(
            store,
            &local_fp,
            &req.fingerprint,
            &req.sas,
            &req.capabilities,
            chrono::Utc::now(),
        )?;
        let version = store.get(&req.fingerprint).map(|p| p.pairing_version).unwrap_or_default();
        Ok::<_, String>(((kind, version), true))
    });
    let (kind, pairing_version) = match tx {
        Err(e) => return json_err("500 Internal Server Error", format!("peer store: {e}")),
        // Code mismatch / expired / blocked / unknown → fail-closed 403.
        Ok(Err(e)) => return json_err("403 Forbidden", e),
        Ok(Ok(v)) => v,
    };
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "fingerprint": req.fingerprint,
            "trust": "trusted",
            "pairing_version": pairing_version,
            "legacy": kind == k2_core::federation::pairing_v2::ConfirmKind::LegacyTrusted,
        })
        .to_string(),
    )
}

// ── Pairing v2 (0.45.2): commit-then-reveal code. Four owner-run steps;
// see `k2_core::federation::pairing_v2`. Nonces never appear in logs.

fn pair_v2_err(e: k2_core::federation::pairing_v2::PairV2Error) -> CliResponse {
    CliResponse {
        status: e.status(),
        content_type: "application/json",
        body: serde_json::json!({ "error": e.to_string(), "code": e.code() }).to_string(),
    }
}

/// This server's own pairing identity: (SPKI PEM, tunnel subdomain,
/// advertised LAN base URL).
fn local_pairing_identity() -> Result<(String, String, String), CliResponse> {
    let key = k2_core::federation::identity::load_or_create()
        .map_err(|e| json_err("500 Internal Server Error", format!("load identity key: {e}")))?;
    let subdomain = k2_core::tunnel::config::load().map(|c| c.subdomain).unwrap_or_default();
    Ok((key.public_key_pem(), subdomain, federation::advertised_federation_base()))
}

#[derive(serde::Deserialize, Default)]
struct PairAddress {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    subdomain: Option<String>,
    #[serde(default, alias = "baseUrl", alias = "url")]
    base_url: Option<String>,
}

fn pick(given: Option<String>, fallback: String) -> String {
    given.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or(fallback)
}

/// `POST /cli/federation/pair/v2/init` (owner/admin). Body (optional):
/// `{label, subdomain, baseUrl}` — the address the other server should pin
/// for this one (defaults: this server's tunnel subdomain and advertised
/// LAN URL). Returns the offer to hand to the other server.
pub fn handle_pair_v2_init(body: &[u8]) -> CliResponse {
    let addr: PairAddress = if body.iter().all(|b| b.is_ascii_whitespace()) {
        PairAddress::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(a) => a,
            Err(e) => return CliResponse::bad_request(format!("bad pair init body: {e}")),
        }
    };
    let (pem, sub, base) = match local_pairing_identity() {
        Ok(v) => v,
        Err(r) => return r,
    };
    let subdomain = pick(addr.subdomain, sub);
    let base_url = pick(addr.base_url, base);
    match k2_core::federation::pairing_v2::init(
        &pem,
        &addr.label.unwrap_or_default(),
        &subdomain,
        &base_url,
        chrono::Utc::now(),
    ) {
        Ok(offer) => CliResponse::ok_json(serde_json::json!({ "offer": offer }).to_string()),
        Err(e) => pair_v2_err(e),
    }
}

/// `POST /cli/federation/pair/v2/respond` (owner/admin). Body: the offer
/// (`{"offer": {...}}` or the bare offer) plus optional `label`,
/// `subdomain`, `baseUrl` for this server's own address. Pins the initiator
/// as Pending (or keeps its existing row) and returns the answer.
pub fn handle_pair_v2_respond(body: &[u8]) -> CliResponse {
    #[derive(serde::Deserialize)]
    struct Req {
        offer: k2_core::federation::pairing_v2::Offer,
        #[serde(flatten)]
        addr: PairAddress,
    }
    let parsed: Req = match serde_json::from_slice::<Req>(body) {
        Ok(r) => r,
        Err(_) => match serde_json::from_slice::<k2_core::federation::pairing_v2::Offer>(body) {
            Ok(offer) => Req { offer, addr: PairAddress::default() },
            Err(e) => return CliResponse::bad_request(format!("bad pair offer: {e}")),
        },
    };
    let (pem, sub, base) = match local_pairing_identity() {
        Ok(v) => v,
        Err(r) => return r,
    };
    let label = parsed.addr.label.unwrap_or_default();
    let subdomain = pick(parsed.addr.subdomain, sub);
    let base_url = pick(parsed.addr.base_url, base);
    let tx = PeerStore::transact(|store| {
        k2_core::federation::pairing_v2::respond(
            store,
            &parsed.offer,
            &pem,
            &label,
            &subdomain,
            &base_url,
            chrono::Utc::now(),
        )
        .map(|a| (a, true))
    });
    match tx {
        Err(e) => json_err("500 Internal Server Error", format!("peer store: {e}")),
        Ok(Err(e)) => pair_v2_err(e),
        Ok(Ok(answer)) => CliResponse::ok_json(serde_json::json!({ "answer": answer }).to_string()),
    }
}

/// `POST /cli/federation/pair/v2/reveal` (owner/admin). Body: the answer
/// (`{"answer": {...}}` or bare) plus optional `peer_subdomain` /
/// `peer_base_url` to pin the other server under another address. Pins it
/// as Pending (or keeps its row), returns `{reveal, sas, peer_fingerprint}`.
pub fn handle_pair_v2_reveal(body: &[u8]) -> CliResponse {
    #[derive(serde::Deserialize)]
    struct Req {
        answer: k2_core::federation::pairing_v2::Answer,
        #[serde(default, alias = "peerSubdomain")]
        peer_subdomain: Option<String>,
        #[serde(default, alias = "peerBaseUrl")]
        peer_base_url: Option<String>,
    }
    let parsed: Req = match serde_json::from_slice::<Req>(body) {
        Ok(r) => r,
        Err(_) => match serde_json::from_slice::<k2_core::federation::pairing_v2::Answer>(body) {
            Ok(answer) => Req { answer, peer_subdomain: None, peer_base_url: None },
            Err(e) => return CliResponse::bad_request(format!("bad pair answer: {e}")),
        },
    };
    let tx = PeerStore::transact(|store| {
        k2_core::federation::pairing_v2::reveal(
            store,
            &parsed.answer,
            parsed.peer_subdomain.as_deref(),
            parsed.peer_base_url.as_deref(),
            chrono::Utc::now(),
        )
        .map(|r| (r, true))
    });
    let r = match tx {
        Err(e) => return json_err("500 Internal Server Error", format!("peer store: {e}")),
        Ok(Err(e)) => return pair_v2_err(e),
        Ok(Ok(r)) => r,
    };
    CliResponse::ok_json(
        serde_json::json!({ "reveal": r.reveal, "sas": r.sas, "peer_fingerprint": r.peer_fingerprint })
            .to_string(),
    )
}

/// `POST /cli/federation/pair/v2/verify` (owner/admin). Body: the reveal
/// (`{"reveal": {...}}` or bare). Checks it against the commitment (aborts
/// on a mismatch) and returns `{sas, peer_fingerprint}`.
pub fn handle_pair_v2_verify(body: &[u8]) -> CliResponse {
    #[derive(serde::Deserialize)]
    struct Req {
        reveal: k2_core::federation::pairing_v2::Reveal,
    }
    let rv = match serde_json::from_slice::<Req>(body) {
        Ok(r) => r.reveal,
        Err(_) => match serde_json::from_slice::<k2_core::federation::pairing_v2::Reveal>(body) {
            Ok(r) => r,
            Err(e) => return CliResponse::bad_request(format!("bad pair reveal: {e}")),
        },
    };
    let local_fp = match federation::local_fingerprint() {
        Ok(fp) => fp,
        Err(e) => return json_err("500 Internal Server Error", format!("local fingerprint: {e}")),
    };
    match k2_core::federation::pairing_v2::verify(&local_fp, &rv, chrono::Utc::now()) {
        Ok((sas, fp_a)) => CliResponse::ok_json(
            serde_json::json!({ "sas": sas, "peer_fingerprint": fp_a }).to_string(),
        ),
        Err(e) => pair_v2_err(e),
    }
}

/// `GET /cli/federation/pubkey` — OWNER-gated (dispatcher enforces). Returns
/// THIS daemon's federation identity so a peer can PIN it during pairing:
///
///   - `public_key_pem` — the SPKI PEM of the federation identity key
///     (`~/.k2/federation/identity.pem`; the key `seal`/`pair` derive their
///     fingerprint from).
///   - `fingerprint`    — `sha256(SPKI DER)` hex (== [`federation::local_fingerprint`]).
///   - `subdomain`      — this daemon's tunnel subdomain (or `""` when none).
///
/// The renderer's owner-driven auto-pair reads BOTH daemons' pubkey here (it
/// holds an owner token for each) so it can present each side's key to the
/// other and confirm the SAS programmatically — no human out-of-band step.
pub fn handle_pubkey() -> CliResponse {
    let key = match k2_core::federation::identity::load_or_create() {
        Ok(k) => k,
        Err(e) => return json_err("500 Internal Server Error", format!("load keypair: {e}")),
    };
    let fingerprint = match federation::local_fingerprint() {
        Ok(fp) => fp,
        Err(e) => return json_err("500 Internal Server Error", format!("local fingerprint: {e}")),
    };
    // Best-effort: a missing/unconfigured tunnel just yields an empty
    // subdomain — the renderer falls back to deriving it from the host.
    let subdomain = k2_core::tunnel::config::load()
        .map(|c| c.subdomain)
        .unwrap_or_default();
    let base_url = federation::advertised_federation_base();
    CliResponse::ok_json(
        serde_json::json!({
            "public_key_pem": key.public_key_pem(),
            "fingerprint": fingerprint,
            "subdomain": subdomain,
            "base_url": base_url,
        })
        .to_string(),
    )
}

// ─────────────────────────────────────────────────────────────────────
// P3 — inbound ingress (the security core)
// ─────────────────────────────────────────────────────────────────────

/// `POST /cli/federation/inbound` — accept a signed envelope. Authenticated by
/// the ENVELOPE (verify against the pinned key + `require_peer`), NOT by any
/// token.
///
/// Two stages, in order:
///
///   1. **Security gate** — [`ingress::ingest`] runs EVERY check (decode →
///      verify signature vs pinned key → `require_peer(fp,"inbound")` →
///      replay → skew → ttl → loop → control-byte sanitize). Any failure
///      REJECTS here, before any delivery. It returns the verified,
///      sanitized signal + the AUTHENTICATED peer fingerprint.
///   2. **Delivery** — make a cross-server message behave EXACTLY like a
///      local `k2 msg`, gated by the recipient workspace's per-project
///      "allow remote instruction" opt-in:
///        - opt-in ON  → [`crate::hire_routes::deliver_recorded_federated`] (inject if live,
///          wake+resume+inject if dormant) — the canonical chat.
///        - opt-in OFF → DECLINE (200, `delivered:false`,
///          `mode:"declined"`). The message is NOT written anywhere trusted
///          (the `.k2/inbox/` is inherently trusted work), and the sender is
///          told it did not land. A remote peer can NEVER drive — nor seed
///          trusted work for — a session the owner hasn't opted into.
///
/// Delivery always targets EXACTLY the agent named in the verified, signed
/// `to`; the sender host attribution is derived from the VERIFIED peer,
/// never from request plaintext (Risk C1).
pub fn handle_inbound(body: &[u8]) -> CliResponse {
    let local_fp = federation::local_fingerprint().unwrap_or_default();
    let store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };

    // ── STAGE 1: SECURITY GATE (fail-closed; no delivery on any error) ──
    let ingested = match ingress::ingest(
        body,
        &store,
        ingress::global_nonce_cache(),
        &local_fp,
        ingress::DEFAULT_SKEW_SECS,
    ) {
        Ok(v) => v,
        Err(e) => return ingress_reject(e),
    };

    // H4 (prd-agent-secret-handoff-v1): a vault kind never reaches a chat.
    // A buggy or hostile sender that posts one here gets 400 `wrong_route`
    // and nothing is injected or recorded.
    if let SignalKind::Custom { kind, .. } = &ingested.signal.kind {
        if kind.starts_with(VAULT_KIND_PREFIX) {
            return wrong_route_response();
        }
    }

    // The sender just proved it is online (the envelope verified against its
    // pinned key) — kick a background drain of anything WE have queued for
    // it. This is the "peer became reachable" trigger of the outbox drain;
    // fire-and-forget so it never extends this request.
    crate::federation_drain::note_peer_reachable(&ingested.peer_fingerprint);

    // ── IDEMPOTENCY GATE — durable per-peer msg_uuid dedupe ──
    // The outbox drain is at-least-once: a delivery whose RESPONSE was lost
    // gets re-dialed (possibly re-sealed with a fresh nonce, so the
    // in-memory replay LRU can't catch it — and that cache resets on
    // restart anyway). The signed msg_uuid is stable across re-seals, so a
    // uuid we already terminally handled is a duplicate: 409 WITHOUT
    // touching the chat. The drain maps this reject to "already delivered".
    if k2_core::federation::seen::already_seen(&ingested.peer_fingerprint, &ingested.msg_uuid) {
        return json_err(
            "409 Conflict",
            format!("duplicate message (already delivered): {}", ingested.msg_uuid),
        );
    }

    // ── STAGE 2: DELIVERY (the gate has passed) ──
    let signal = ingested.signal;

    // The recipient is EXACTLY the signed `to` agent — never a different
    // workspace. Resolve its workspace UUID → registered project path.
    let (ws_id, to_name) = match &signal.to {
        AgentAddress::Agent { workspace, name } => (workspace.0.clone(), name.clone()),
        other => {
            return json_err(
                "400 Bad Request",
                format!("inbound signal is not addressed to an agent: {other:?}"),
            );
        }
    };
    let project_path = match resolve_project_path_by_id(&ws_id) {
        Some(p) => p,
        None => return unknown_agent_response(),
    };

    // RECEIVER GATE (CA1). The signed `to` names a workspace AND an agent.
    // Deliver only when that workspace is one this server lists to peers
    // (its own roster rules) and the name is one of that workspace's names.
    // This holds whatever the sender's own connection gate decided, and it
    // covers live text and tray packages alike. Same 404 as an unknown id,
    // so the answer never tells a peer which hidden workspaces exist; the
    // sender's drain dead-letters any 4xx.
    //
    // `name/handle` (0.46.11) names a sidecar chat in that workspace: the
    // workspace half passes the same gate, and the chat must exist there.
    let (to_ws_name, to_sidecar) = split_remote_sidecar(&to_name);
    if !roster::accepts_inbound(&ws_id, &project_path, to_ws_name) {
        k2_core::log_debug!(
            "[federation] inbound refused: peer {} addressed a name/workspace pair this server does not list",
            ingested.peer_fingerprint
        );
        return unknown_agent_response();
    }
    if let Some(handle) = to_sidecar {
        let known = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace_session_handles::resolve_for_delivery(&conn, &ws_id, handle).is_ok()
        };
        if !known {
            return unknown_sidecar_response(to_ws_name, handle);
        }
    }

    // Sender attribution `<sender-agent>::<peer-host>` — always `::`, never
    // `@`. Using `@` made receiving agents reach for `k2 mail` instead of
    // `k2 msg` (GH #47). Host comes from the VERIFIED peer pin (AUTHENTICATED
    // fingerprint), never request plaintext. Both sides lowercase.
    let host = store
        .get(&ingested.peer_fingerprint)
        .map(peer_host)
        .unwrap_or_default();
    let sender_agent = match &signal.from {
        AgentAddress::Agent { name, .. } => name.as_str(),
        AgentAddress::Workspace { workspace } => workspace.0.as_str(),
        _ => "peer",
    };
    // The label is typed into an agent's terminal and written into its
    // inbox: both parts must be in shape, or nothing is delivered.
    let sender_agent = match sender_handle(sender_agent) {
        Some(name) if host.is_empty() || is_peer_host(&host) => name,
        _ => {
            k2_core::log_debug!(
                "[federation] inbound refused: peer {} sent a sender address that is not a handle. Raw name: {}; host: {}",
                ingested.peer_fingerprint,
                reason_for_log(&serde_json::Value::from(sender_agent)),
                reason_for_log(&serde_json::Value::from(host.as_str()))
            );
            return bad_sender_response();
        }
    };
    let from_label = federated_from_label(&sender_agent, &host);

    // Tray packages (`k2 msg agent::host --inbox-wake`) ride this same
    // sealed envelope. Land in `.k2/inbox/`; never dump file bytes into
    // the live PTY. remote_instruct gates the optional wake knock only.
    if let SignalKind::Custom { kind, payload } = &signal.kind {
        if kind == federation::TRAY_SIGNAL_KIND {
            return deliver_inbound_tray(
                &project_path,
                payload,
                &from_label,
                &ingested.peer_fingerprint,
                &ingested.msg_uuid,
                &signal.id.to_string(),
            );
        }
    }

    // The deliverable text (federation sends `Msg`; render any other kind
    // so nothing is silently lost).
    // H14 backstop: a known vault value (8+ characters) pasted into a
    // federated message never reaches the chat or Chatter.
    let text = match &signal.kind {
        SignalKind::Msg { text } => k2_core::redaction::redact(text),
        other => k2_core::redaction::redact(&serde_json::to_string(other).unwrap_or_default()),
    };

    // CONSENT GATE — the recipient workspace's per-project "allow remote
    // instruction" opt-in is the consent for canonical-chat DRIVE.
    let opt_in = k2_core::workspace::settings::remote_instruct_allowed_for_path(&project_path);

    if opt_in {
        // Identical to local `k2 msg`: inject if live, wake+resume+inject if
        // dormant. (wake=true, like `k2 talk`.)
        // A sidecar is reached as `<workspace id>/<handle>`, the same
        // address a local `k2 msg` resolves (never the workspace's main chat).
        let deliver_to = match to_sidecar {
            Some(handle) => format!("{ws_id}/{handle}"),
            None => project_path.clone(),
        };
        // 0310 (P28): recorded with `via = federation` and the peer's
        // pinned key, so the `[from agent::host · m_…]` stamp is checkable.
        let (result, _) =
            crate::hire_routes::deliver_recorded_federated(&deliver_to, &text, &from_label, &ingested.peer_fingerprint);
        // Terminal SUCCESS → remember the msg_uuid so a drain redelivery
        // (lost-response race) dedupes instead of injecting twice. A FAILED
        // live delivery is deliberately NOT recorded — it is still owed, and
        // the sender's drain keeps it queued for retry.
        if result.success {
            k2_core::federation::seen::record(&ingested.peer_fingerprint, &ingested.msg_uuid);
        }
        CliResponse::ok_json(
            serde_json::json!({
                "delivered": result.success,
                "mode": "live",
                "signal_id": signal.id.to_string(),
                "result": result,
            })
            .to_string(),
        )
    } else {
        // Opt-in OFF: "allow remote instruction = OFF" means remote messages
        // must NOT instruct this agent. The `.k2/inbox/` is inherently TRUSTED
        // (agents treat inbox items as work), so writing there would contradict
        // the opt-out. DECLINE explicitly instead: deliver nowhere trusted, and
        // tell the sender it did not land (a future distrusted, email-style
        // inbox is where un-instructed remote messages will belong). The dial
        // still succeeds (200) so the sender can distinguish a clean decline
        // from a transport/security failure — but `delivered:false`.
        //
        // A decline is TERMINAL (senders dead-letter it; re-dialing can
        // never flip an opt-out) — record the msg_uuid so a redelivery
        // dedupes here too.
        k2_core::federation::seen::record(&ingested.peer_fingerprint, &ingested.msg_uuid);
        CliResponse::ok_json(
            serde_json::json!({
                "delivered": false,
                "mode": "declined",
                "reason": "remote_instruction_disabled",
                "signal_id": signal.id.to_string(),
            })
            .to_string(),
        )
    }
}

/// The sender-facing hint for a peer's decline. `remote_instruction_disabled`
/// (the wire reason, kept for old senders) means the recipient's "Let agents
/// on other servers message this agent" switch is off.
pub(crate) fn declined_hint(target: &str, reason: &str) -> String {
    if reason == "remote_instruction_disabled" {
        format!(
            "'{target}' declined the message: that agent doesn't take messages from agents on \
             other servers (remote_instruction_disabled). Its owner can turn on \"Let agents on \
             other servers message this agent\" in that workspace's settings."
        )
    } else {
        format!("'{target}' declined the message: {reason}")
    }
}

/// True when a peer's decline `reason` is a code (`^[a-z0-9_]{1,64}$`).
/// Only a code is passed on to the sender; anything else is
/// `declined_by_peer`.
pub(crate) fn is_reason_code(reason: &str) -> bool {
    (1..=64).contains(&reason.len())
        && reason.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The longest handle a peer's sender name or roster entry may be (the
/// postal handle cap).
const HANDLE_MAX: usize = 64;

/// True when `name` is in the workspace handle shape: the address-token
/// check `k2 msg` uses for a workspace (lowercase letters and digits,
/// single hyphens between them), at most 64 characters. A peer's sender
/// name reaches an agent only in this shape.
pub(crate) fn is_handle(name: &str) -> bool {
    name.len() <= HANDLE_MAX && k2_core::workspace_session_handles::is_address_token(name)
}

/// A peer's sender name as a handle: trimmed and lowercased first (an
/// older sender may send capitals), then [`is_handle`]. `None` when it
/// still is not one (a space, a bad character).
pub(crate) fn sender_handle(raw: &str) -> Option<String> {
    let name = raw.trim().to_ascii_lowercase();
    is_handle(&name).then_some(name)
}

/// True when a pinned peer's host is a host name or IP, with an optional
/// port: `name` or `name:port`.
pub(crate) fn is_peer_host(host: &str) -> bool {
    let (name, port) = match host.rsplit_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (host, None),
    };
    let port_ok = port.is_none_or(|p| {
        !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|v| v > 0)
    });
    port_ok && k2_core::tunnel::relay_list::valid_host(name)
}

/// 400 for a message whose sender name (or this server's pin of its host)
/// is not in shape: nothing is delivered or recorded. `code` is the stable
/// reason; the sender's drain dead-letters any 4xx.
pub(crate) fn bad_sender_response() -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: serde_json::json!({
            "error": "bad_sender: a sender's name reaches an agent only when it is a handle \
                      (lowercase letters, digits and single hyphens, at most 64 characters)",
            "code": "bad_sender",
        })
        .to_string(),
    }
}

/// The cap on a peer's free text: in this daemon's log, and in what an
/// agent sees (a tray title, an answer's `title` and `from`).
pub(crate) const PEER_TEXT_CAP: usize = 200;

/// A peer's free text cleaned: the first `cap` characters, every character
/// outside printable ASCII escaped (`\u{1b}`), `...` when cut.
pub(crate) fn escape_capped(text: &str, cap: usize) -> String {
    let mut out = String::new();
    for c in text.chars().take(cap) {
        if c == ' ' || c.is_ascii_graphic() {
            out.push(c);
        } else {
            out.extend(c.escape_unicode());
        }
    }
    if text.chars().count() > cap {
        out.push_str("...");
    }
    out
}

/// A peer's raw `reason` for this daemon's log only: the first 200
/// characters, every character outside printable ASCII escaped.
pub(crate) fn reason_for_log(raw: &serde_json::Value) -> String {
    let text = raw.as_str().map_or_else(|| raw.to_string(), str::to_string);
    escape_capped(&text, PEER_TEXT_CAP)
}

/// The longest cover note a peer's tray may carry.
pub(crate) const TRAY_NOTE_MAX: usize = 64 * 1024;

/// The longest tray file name (a file name on disk).
const TRAY_FILE_NAME_MAX: usize = 255;

/// True when a peer's tray file name is already a plain, safe file name:
/// [`k2_core::inbox::sanitize_sidecar_filename`] leaves it unchanged
/// (letters, digits, `.`, `-` and `_`, no path), at most 255 bytes.
pub(crate) fn is_tray_file_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= TRAY_FILE_NAME_MAX && k2_core::inbox::sanitize_sidecar_filename(name) == name
}

/// 400 for a tray whose file names or cover note are not in shape:
/// nothing lands. `code` is the stable reason.
pub(crate) fn bad_tray_response() -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: serde_json::json!({
            "error": "bad_tray: a tray lands only when each file name is a plain file name \
                      (letters, digits, '.', '-' and '_', at most 255 bytes) and its note is at most 64 KiB",
            "code": "bad_tray",
        })
        .to_string(),
    }
}

/// The largest `fileCount` a peer's inbox answer may give.
const REPLY_FILE_COUNT_MAX: u64 = 1000;

/// The longest inbox id: its cover `<id>.md` is one file name.
const INBOX_ID_MAX: usize = 252;

/// The longest path a peer's inbox answer may give.
const PEER_PATH_MAX: usize = 4096;

/// True when a peer's inbox package id is in the shape the inbox mints
/// (a title slug, maybe with `-2`: the address-token shape), at most 252
/// characters.
pub(crate) fn is_inbox_id(id: &str) -> bool {
    id.len() <= INBOX_ID_MAX && k2_core::workspace_session_handles::is_address_token(id)
}

/// The parts of a peer's inbox path after its `inbox/` under a workspace
/// folder (any of `WORKSPACE_DOT_DIR_NAMES`: `.k5`, `.k2`, `.k2so`):
/// `None` unless the path is absolute (`/…` or `C:\…`),
/// printable ASCII, at most 4096 bytes, and has no `.` or `..` part.
fn peer_inbox_parts(path: &str) -> Option<Vec<&str>> {
    let b = path.as_bytes();
    if b.is_empty() || b.len() > PEER_PATH_MAX || !b.iter().all(|&c| c == b' ' || c.is_ascii_graphic()) {
        return None;
    }
    let absolute = b[0] == b'/' || (b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\'));
    if !absolute {
        return None;
    }
    let parts: Vec<&str> = path.split(['/', '\\']).collect();
    if parts.iter().any(|p| *p == "." || *p == "..") {
        return None;
    }
    let at = (1..parts.len())
        .rev()
        .find(|&i| parts[i] == "inbox" && k2_core::paths::WORKSPACE_DOT_DIR_NAMES.contains(&parts[i - 1]))?;
    Some(parts[at + 1..].to_vec())
}

/// True when `path` is the cover of inbox package `id`: `<…>/.k2/inbox/<id>.md`.
pub(crate) fn is_peer_cover_path(path: &str, id: &str) -> bool {
    peer_inbox_parts(path).is_some_and(|rest| rest.len() == 1 && rest[0] == format!("{id}.md"))
}

/// True when `path` is a file of inbox package `id`:
/// `<…>/.k2/inbox/<id>.files/<plain file name>`.
pub(crate) fn is_peer_sidecar_path(path: &str, id: &str) -> bool {
    peer_inbox_parts(path).is_some_and(|rest| {
        rest.len() == 2 && rest[0] == format!("{id}.files") && is_tray_file_name(rest[1])
    })
}

/// True for a session id: 1 to 128 of `A-Z a-z 0-9 - _`.
fn is_plain_id(s: &str) -> bool {
    (1..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn log_left_out(peer_fp: &str, key: &str, raw: &serde_json::Value) {
    k2_core::log_debug!(
        "[federation/send] peer {peer_fp}'s inbox answer: {key} is not in shape and is left out. Raw: {}",
        reason_for_log(raw)
    );
}

/// A peer's `wake` result rebuilt from the fields in shape: `success` (a
/// bool, or no `wake` at all), `reason` (a code), `woke`, `wakeMs`,
/// `attempts` and `targetSessionId` (an id). Free text (`hint`) and any
/// other field are never passed on.
fn checked_wake(raw: &serde_json::Value, peer_fp: &str) -> Option<serde_json::Value> {
    let obj = raw.as_object()?;
    let success = obj.get("success")?.as_bool()?;
    let mut out = serde_json::Map::new();
    out.insert("success".into(), serde_json::Value::Bool(success));
    for (key, v) in obj {
        let keep = match key.as_str() {
            "success" => continue,
            "hint" if v.is_null() => continue,
            "reason" => v.is_null() || v.as_str().is_some_and(is_reason_code),
            "targetSessionId" => v.is_null() || v.as_str().is_some_and(is_plain_id),
            "woke" => v.is_boolean(),
            "wakeMs" => v.is_null() || v.is_u64(),
            "attempts" => v.as_u64().is_some_and(|n| n <= u64::from(u8::MAX)),
            _ => false,
        };
        if keep {
            out.insert(key.clone(), v.clone());
        } else {
            log_left_out(peer_fp, &format!("wake.{key}"), v);
        }
    }
    Some(serde_json::Value::Object(out))
}

/// The fields of a peer's inbox answer (`mode: "inbox"`) that reach the
/// caller, each checked by its shape: `id` an inbox id; `filename` its
/// `<id>.md`; `coverPath` and `sidecarPaths` that package's paths under a
/// `.k2/inbox/`; `fileCount` a whole number from 1 to 1000; `wake` as
/// [`checked_wake`]. `title` and `from` are capped and escaped. A field
/// that fails is left out and its raw value goes to this daemon's log.
pub(crate) fn checked_inbox_answer(
    parsed: &serde_json::Value,
    peer_fp: &str,
) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::Value;
    let mut out = serde_json::Map::new();
    let id = match parsed.get("id") {
        Some(Value::String(id)) if is_inbox_id(id) => Some(id.as_str()),
        Some(raw) => {
            log_left_out(peer_fp, "id", raw);
            None
        }
        None => None,
    };
    if let Some(id) = id {
        out.insert("id".into(), Value::from(id));
    }
    let mut keep = |key: &str, ok: bool| {
        if let Some(raw) = parsed.get(key) {
            if ok {
                out.insert(key.into(), raw.clone());
            } else {
                log_left_out(peer_fp, key, raw);
            }
        }
    };
    let str_of = |key: &str| parsed.get(key).and_then(Value::as_str);
    keep("filename", id.zip(str_of("filename")).is_some_and(|(id, f)| f == format!("{id}.md")));
    keep("coverPath", id.zip(str_of("coverPath")).is_some_and(|(id, p)| is_peer_cover_path(p, id)));
    keep(
        "sidecarPaths",
        id.zip(parsed.get("sidecarPaths").and_then(Value::as_array)).is_some_and(|(id, paths)| {
            paths.iter().all(|p| p.as_str().is_some_and(|p| is_peer_sidecar_path(p, id)))
        }),
    );
    keep(
        "fileCount",
        parsed.get("fileCount").and_then(Value::as_u64).is_some_and(|n| (1..=REPLY_FILE_COUNT_MAX).contains(&n)),
    );
    if let Some(raw) = parsed.get("wake") {
        match checked_wake(raw, peer_fp) {
            Some(wake) => {
                out.insert("wake".into(), wake);
            }
            None => log_left_out(peer_fp, "wake", raw),
        }
    }
    for key in ["title", "from"] {
        match parsed.get(key) {
            Some(Value::String(text)) => {
                let clean = escape_capped(text, PEER_TEXT_CAP);
                if clean != *text {
                    k2_core::log_debug!(
                        "[federation/send] peer {peer_fp}'s inbox answer: {key} is capped and escaped. Raw: {}",
                        reason_for_log(&Value::from(text.as_str()))
                    );
                }
                out.insert(key.into(), Value::from(clean));
            }
            Some(raw) => log_left_out(peer_fp, key, raw),
            None => {}
        }
    }
    out
}

/// Fail-closed: map each ingress reject to a status. Auth/authorization
/// failures are 403; malformed/bad-sig are 400; replay/skew/ttl/loop are
/// 409 (the request was well-formed but not acceptable now).
pub(crate) fn ingress_reject(e: ingress::IngressError) -> CliResponse {
    let status = match e {
        ingress::IngressError::UnknownPeer | ingress::IngressError::NotTrusted | ingress::IngressError::CapabilityDenied => {
            "403 Forbidden"
        }
        ingress::IngressError::Decode(_) | ingress::IngressError::BadSignature => "400 Bad Request",
        ingress::IngressError::Replay
        | ingress::IngressError::SkewTooLarge { .. }
        | ingress::IngressError::TtlExpired
        | ingress::IngressError::LoopDetected => "409 Conflict",
    };
    json_err(status, e)
}

/// 404 for a signed `to` that does not name an agent this server lists.
/// `error` stays a string (the drain and older senders read it as one);
/// `code` is the stable machine reason.
pub(crate) fn unknown_agent_response() -> CliResponse {
    CliResponse {
        status: "404 Not Found",
        content_type: "application/json",
        body: serde_json::json!({
            "error": "unknown_agent: no agent by that name at that address on this server",
            "code": "unknown_agent",
        })
        .to_string(),
    }
}

/// Split a signed `to` name into the workspace name and an optional sidecar
/// handle: `ada/notes` → (`ada`, `notes`). A name without a
/// slash, or with an empty half, is a plain workspace name.
pub(crate) fn split_remote_sidecar(name: &str) -> (&str, Option<&str>) {
    match name.trim().split_once('/') {
        Some((ws, handle)) if !ws.trim().is_empty() && !handle.trim().is_empty() => {
            (ws.trim(), Some(handle.trim()))
        }
        _ => (name.trim(), None),
    }
}

/// 404 for `name/handle` when the workspace takes messages but has no chat
/// at that handle.
pub(crate) fn unknown_sidecar_response(ws_name: &str, handle: &str) -> CliResponse {
    CliResponse {
        status: "404 Not Found",
        content_type: "application/json",
        body: serde_json::json!({
            "error": format!(
                "unknown_sidecar: {ws_name} has no chat at '{handle}' on this server \
                 (its owner can see the addresses with k2 sessions)"
            ),
            "code": "unknown_sidecar",
        })
        .to_string(),
    }
}

/// Land a federated tray package into `project_path`'s `.k2/inbox/`.
/// Package success is primary. Wake knock is a short pointer (not the file
/// body) and only runs when the workspace opted into remote instruction.
fn deliver_inbound_tray(
    project_path: &str,
    payload: &serde_json::Value,
    from_label: &str,
    peer_fingerprint: &str,
    msg_uuid: &str,
    signal_id: &str,
) -> CliResponse {
    let spec: federation::TraySpec = match serde_json::from_value(payload.clone()) {
        Ok(s) => s,
        Err(e) => return CliResponse::bad_request(format!("inbox-tray payload: {e}")),
    };
    // Each file name must already be a plain file name, and the note fit
    // the cap; otherwise nothing lands.
    if let Some(bad) = spec.files.iter().find(|f| !is_tray_file_name(&f.name)) {
        k2_core::log_debug!(
            "[federation] inbound tray refused: peer {peer_fingerprint} sent a file name that is not a plain file name. Raw: {}",
            reason_for_log(&serde_json::Value::from(bad.name.as_str()))
        );
        return bad_tray_response();
    }
    if let Some(note) = spec.body.as_deref().filter(|b| b.len() > TRAY_NOTE_MAX) {
        k2_core::log_debug!(
            "[federation] inbound tray refused: peer {peer_fingerprint} sent a note of {} bytes (cap {TRAY_NOTE_MAX})",
            note.len()
        );
        return bad_tray_response();
    }
    let decoded = match federation::decode_tray_spec(&spec) {
        Ok(d) => d,
        Err(e) => return CliResponse::bad_request(e),
    };
    // The title is typed into the agent's terminal (the wake pointer) and
    // written into the cover: one line, capped, escaped. The note goes to
    // the cover only: control characters other than line breaks and tabs
    // are dropped.
    let note = decoded.body.as_deref().map(ingress::sanitize_text);
    let named: Vec<k2_core::inbox::NamedBytes> = decoded
        .files
        .iter()
        .map(|(name, bytes)| k2_core::inbox::NamedBytes {
            name: name.clone(),
            bytes: bytes.clone(),
        })
        .collect();
    // With no title, the inbox would take the cover's title from a `.md`
    // file's heading: clean that the same way and give it as the title.
    let title = decoded
        .title
        .clone()
        .or_else(|| k2_core::inbox::md_title_for_named(&named))
        .map(|t| escape_capped(&t, PEER_TEXT_CAP));
    let package = match k2_core::inbox::deliver_named_bytes(
        std::path::Path::new(project_path),
        &named,
        title.as_deref(),
        Some(from_label),
        Some("msg-inbox"),
        note.as_deref(),
    ) {
        Ok(p) => p,
        Err(e) => {
            return json_err(
                "500 Internal Server Error",
                format!("tray inbox deliver: {e}"),
            )
        }
    };

    let file_count = if package.sidecar_paths.is_empty() {
        1
    } else {
        package.sidecar_paths.len()
    };
    // A title from file names only is clean already; clean it anyway
    // before an agent sees it (a given title is clean already).
    let shown_title = title.clone().unwrap_or_else(|| escape_capped(&package.title, PEER_TEXT_CAP));
    let opt_in = k2_core::workspace::settings::remote_instruct_allowed_for_path(project_path);
    let wake = if decoded.wake {
        if opt_in {
            let pointer = k2_core::inbox::wake_pointer_text_n(
                &package.id,
                &shown_title,
                Some(file_count),
            );
            let (result, _) =
                crate::hire_routes::deliver_recorded_federated(project_path, &pointer, from_label, peer_fingerprint);
            serde_json::json!({
                "success": result.success,
                "reason": result.reason,
                "hint": result.hint,
                "targetSessionId": result.target_session_id,
                "woke": result.woke,
                "wakeMs": result.wake_ms,
                "attempts": result.attempts,
            })
        } else {
            serde_json::json!({
                "success": false,
                "reason": "remote_instruction_disabled",
                "hint": "package landed; wake knock skipped (this agent's \"Let agents on other servers message this agent\" switch is off)",
            })
        }
    } else {
        serde_json::Value::Null
    };

    // Package write is terminal — drain redelivery must not duplicate the tray.
    k2_core::federation::seen::record(peer_fingerprint, msg_uuid);

    let mut out = serde_json::json!({
        "delivered": true,
        "mode": "inbox",
        "ok": true,
        "id": package.id,
        "title": shown_title,
        "filename": package.filename,
        "coverPath": package.cover_path,
        "sidecarPaths": package.sidecar_paths,
        "fileCount": file_count,
        "signal_id": signal_id,
        "from": from_label,
    });
    if !wake.is_null() {
        out["wake"] = wake;
    }
    CliResponse::ok_json(out.to_string())
}

/// Canonical federated `[from …]` attribution shown in the recipient chat.
///
/// **Always** `agent::host` (lowercase). Never `@` — that sigil made
/// receiving agents treat the handle as mail and call `k2 mail` instead of
/// `k2 msg` (GH #47). Empty host → bare agent; empty agent → `"peer"`.
pub(crate) fn federated_from_label(sender_agent: &str, host: &str) -> String {
    let agent = sender_agent.trim().to_ascii_lowercase();
    let host = host.trim().to_ascii_lowercase();
    let agent = if agent.is_empty() {
        "peer".to_string()
    } else {
        agent
    };
    if host.is_empty() {
        agent
    } else {
        format!("{agent}::{host}")
    }
}

/// Look up `projects.path` by `projects.id` — the addressed workspace UUID
/// carried in the verified `to` address. `None` when no project matches
/// (the addressed workspace isn't on this server).
pub(crate) fn resolve_project_path_by_id(workspace_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT path FROM projects WHERE id = ?1",
        rusqlite::params![workspace_id],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

// ─────────────────────────────────────────────────────────────────────
// P4 — outbound send
// ─────────────────────────────────────────────────────────────────────

/// The sender namespace of a federation send (the passport's workspace, else
/// the owner-or-admin token the dispatcher let in).
fn send_sender() -> String {
    match crate::caller_workspace::request_principal() {
        Some(p) => crate::client_msg_id::agent_ns(&p.workspace_uuid),
        None => "c:owner".to_string(),
    }
}

/// `POST /cli/federation/send` as the TCP dispatcher and the cell socket
/// serve it: [`handle_send`] once per `idempotency_key` (0.46.15,
/// [`crate::send_once`]).
pub fn handle_send_route(body: &[u8]) -> CliResponse {
    let key = crate::send_once::key_in_json(body);
    crate::send_once::once_keyed("/cli/federation/send", &send_sender(), key.as_deref(), || handle_send(body))
}

/// `POST /cli/federation/send` — dual-auth (dispatcher enforces owner-or-admin
/// OR scoped passport). Body: `{"to":"<peer>::<workspace>::<agent>","text":"..."}`
/// or the same `to` plus `tray` (cover + small attachments for
/// `k2 msg --inbox-wake|--inbox-silent` on `agent::host`).
/// Seals an `AgentSignal`, enqueues it durably, then dials the peer and POSTs
/// to its `/cli/federation/inbound`. On a confirmed delivery the queued copy
/// is removed; otherwise it stays for the retry loop. Blocking (network I/O)
/// — the dispatcher runs it on a blocking worker.
///
/// **PR1 principal bind (Wave 0 PR-C):** when a scoped principal is installed
/// via [`crate::caller_workspace::with_request_principal`], `from_workspace`
/// is forced from `principal.workspace_uuid` (body claim ignored). Owner path
/// (no principal) keeps optional body `from_workspace`.
pub fn handle_send(body: &[u8]) -> CliResponse {
    #[derive(serde::Deserialize)]
    struct Req {
        to: String,
        #[serde(default)]
        text: String,
        /// GAP #3: the SOURCE workspace (filesystem path) the calling
        /// agent is in. When present, the cross-daemon send is GATED on
        /// `is_remote_connection(from_workspace, "<agent>::<host>")` —
        /// an agent may only message a peer its workspace is connected
        /// to. Absent ⇒ the connection gate is skipped (owner-remote
        /// `k2 talk` and legacy owner sends are a different, ungated
        /// path); the `peer.trust == Trusted` check still always runs.
        /// With a scoped principal this field is **ignored** — identity
        /// comes from the passport only.
        #[serde(default)]
        from_workspace: Option<String>,
        /// Federated `--inbox-wake|--inbox-silent` package. When set, the
        /// signal is `SignalKind::Custom { kind: inbox-tray }` instead of
        /// live `Msg` text. Mutually preferred over `text`.
        #[serde(default)]
        tray: Option<federation::TraySpec>,
        /// 0.46.15: the CLI's per-invocation key. The envelope's
        /// `msg_uuid` is derived from it, so a repeat after this daemon
        /// forgot the key still reaches the peer with the uuid it already
        /// dedupes on (`federation::seen`). An older peer dedupes on that
        /// uuid too; it never sees the key itself.
        #[serde(default)]
        idempotency_key: Option<String>,
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("bad send body: {e}")),
    };

    // Resolve effective source workspace: principal wins over body.
    let from_workspace: Option<String> =
        match crate::caller_workspace::request_principal() {
            Some(principal) => {
                match crate::caller_workspace::resolve_caller_workspace(
                    Some(&principal),
                    None,
                ) {
                    Ok(ws) => Some(ws.path),
                    Err(e) => {
                        return CliResponse {
                            status: "403 Forbidden",
                            content_type: "application/json",
                            body: serde_json::json!({
                                "ok": false,
                                "error": {
                                    "code": "forbidden",
                                    "hint": e.hint(),
                                },
                            })
                            .to_string(),
                        };
                    }
                }
            }
            None => req
                .from_workspace
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
        };

    // Parse `<peer>::<workspace>::<agent>`.
    let parts: Vec<&str> = req.to.split("::").collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return CliResponse::bad_request("address must be <peer>::<workspace>::<agent>");
    }
    let (peer_sel, ws, agent) = (parts[0], parts[1], parts[2]);

    let store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };
    // Resolve the peer by fingerprint, label, OR subdomain (identity is the
    // key; label/subdomain are convenience selectors).
    let peer = store.list().iter().find(|p| peer_selector_matches(p, peer_sel));
    let peer = match peer {
        Some(p) => p.clone(),
        None => return json_err("404 Not Found", format!("no pinned peer matches '{peer_sel}'")),
    };
    if peer.trust != k2_core::federation::PeerTrust::Trusted {
        return json_err(
            "403 Forbidden",
            format!("peer '{peer_sel}' is not Trusted (state={:?})", peer.trust),
        );
    }

    // GAP #3 — the CROSS-DAEMON CONNECTION GATE (fail-closed). When
    // `from_workspace` is set (agent-initiated send, or owner body claim),
    // the send is allowed ONLY IF that source workspace is connected to the
    // target. This is IN ADDITION to the trust check above — trust says
    // "I've paired with this peer"; the connection says "THIS workspace is
    // allowed to message THIS remote workspace". Scoped principals always
    // have from_workspace forced, so the gate always runs for agents.
    // Owner-remote `k2 talk` (no principal, no body from_workspace) is the
    // ungated path; Trusted peer check still always runs.
    //
    // A7: the gate decides by the remote WORKSPACE id in `to`, not by the
    // typed name — any name the peer lists for a connected workspace
    // passes, and a connected name paired with another workspace's id does
    // not (F1). See [`remote_send_gate_check`].
    //
    // `name/handle` (a sidecar chat in that workspace) is gated as the
    // workspace: the connection is to the workspace, its chats come with it.
    if let Some(from_ws) = from_workspace.as_deref() {
        let (gate_name, _) = split_remote_sidecar(agent);
        if let Err(resp) = remote_send_gate_check(from_ws, &peer, ws, gate_name) {
            return resp;
        }
    }

    let key = match k2_core::federation::identity::load_or_create() {
        Ok(k) => k,
        Err(e) => return json_err("500 Internal Server Error", format!("load keypair: {e}")),
    };
    let local_fp = federation::local_fingerprint().unwrap_or_default();

    // `from` carries the SENDER AGENT IDENTITY so the recipient's chat can
    // render `[from <agent>::<host>]` and reply with `k2 msg <agent>::<host>`.
    //
    // A7 S4: `from.name` is the source workspace's HANDLE — exactly what
    // its roster lists as `agent` (`projects.handle`, else the resolved
    // agent name, else the folder basename), so the stamp is always a
    // name the recipient can reply to and its connection binds to.
    // CA14 / F3: `from.workspace` is the source project id, never its
    // filesystem path (receivers ignore it for routing; it only leaked a
    // home-folder name). Owner-remote sends (`k2 talk`, no
    // `from_workspace`) carry no source agent → attributed to `owner`. The
    // whole signal is signed end-to-end, so this identity is AUTHENTICATED
    // to the verified peer (no forgeable plaintext `[from]`, Risk C1).
    let from = match from_workspace.as_deref() {
        Some(src) => {
            let (project_id, handle) = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                conn.query_row(
                    "SELECT id, handle FROM projects WHERE path = ?1 OR id = ?1 LIMIT 1",
                    rusqlite::params![src],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .map(|(id, h)| (Some(id), h))
                .unwrap_or((None, None))
            };
            let name = handle
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .or_else(|| {
                    k2_core::workspace::agent_identity::resolve_agent_name(src)
                        .map(|n| n.trim().to_ascii_lowercase())
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or_else(|| {
                    Path::new(src)
                        .file_name()
                        .map(|s| s.to_string_lossy().to_ascii_lowercase())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| src.to_ascii_lowercase())
                });
            AgentAddress::Agent {
                workspace: WorkspaceId(project_id.unwrap_or_else(|| format!("peer:{local_fp}"))),
                name,
            }
        }
        None => AgentAddress::Agent {
            workspace: WorkspaceId(format!("peer:{local_fp}")),
            name: "owner".to_string(),
        },
    };
    let kind = match req.tray.as_ref() {
        Some(spec) => {
            if let Err(e) = federation::decode_tray_spec(spec) {
                return CliResponse::bad_request(e);
            }
            SignalKind::Custom {
                kind: federation::TRAY_SIGNAL_KIND.to_string(),
                payload: serde_json::to_value(spec).unwrap_or(serde_json::json!({})),
            }
        }
        None => {
            if req.text.trim().is_empty() {
                return CliResponse::bad_request(
                    "text required — or pass tray for k2 msg --inbox-wake",
                );
            }
            SignalKind::Msg {
                text: req.text.clone(),
            }
        }
    };
    let signal = AgentSignal {
        from,
        to: AgentAddress::Agent {
            workspace: WorkspaceId(ws.to_string()),
            name: agent.to_string(),
        },
        delivery: Delivery::Inbox,
        ..AgentSignal::new(
            AgentAddress::Broadcast,
            AgentAddress::Broadcast,
            kind,
        )
    };

    let dial_base = match gated_peer_base_url(&peer) {
        Ok(b) => b,
        Err(e) => return json_err("403 Forbidden", e),
    };

    let stable_uuid = req
        .idempotency_key
        .as_deref()
        .map(str::trim)
        .filter(|k| crate::send_once::is_valid(k))
        .map(|k| crate::send_once::federation_msg_uuid(&send_sender(), if req.tray.is_some() { "tray" } else { "msg" }, k));
    let bytes = match federation::seal_with_msg_uuid(&signal, &key, &peer.subdomain, OUTBOUND_TTL, stable_uuid.as_deref()) {
        Ok(b) => b,
        Err(e) => return json_err("500 Internal Server Error", format!("seal envelope: {e}")),
    };
    let msg_uuid = serde_json::from_slice::<federation::FederationEnvelope>(&bytes)
        .map(|e| e.payload.msg_uuid)
        .unwrap_or_default();

    // Durable enqueue BEFORE the dial (restart-survival; Risk M1).
    let queued_path = match outbox::enqueue(&peer.fingerprint, &bytes) {
        Ok(p) => p,
        Err(e) => return json_err("500 Internal Server Error", format!("enqueue outbox: {e}")),
    };

    // IN-ORDER GUARD: when this peer already has OLDER messages queued, a
    // direct dial would deliver the new message ahead of them. Hand the whole
    // queue (including this message) to the single-flight drain instead —
    // it delivers oldest-first — and report "queued" honestly.
    let queued_ahead = outbox::list_for_peer(&peer.fingerprint)
        .iter()
        .filter(|i| i.msg_uuid != msg_uuid)
        .count();
    if queued_ahead > 0 {
        crate::federation_drain::kick(&peer.fingerprint);
        return CliResponse::ok_json(
            serde_json::json!({
                "status": "queued",
                "msg_uuid": msg_uuid,
                "peer": peer.fingerprint,
                "hint": format!(
                    "{queued_ahead} earlier message(s) queued for this peer — delivering in order"
                ),
            })
            .to_string(),
        );
    }

    // Dial the peer's E2E listener over the Connect tunnel + POST the envelope.
    match post_inbound(&dial_base, &bytes) {
        Ok(resp_body) => {
            // The transport succeeded and the message left this daemon, so the
            // queued copy is consumed either way (a DECLINE is a terminal
            // outcome, not a retryable failure — re-dialing would never flip an
            // opt-out). But a 200 is NOT necessarily a delivery: the receive
            // side returns `delivered:false` / `mode:"declined"` when the
            // recipient has remote-instruction OFF. Surface that to the sender
            // as `status:"declined"` (distinct from `"sent"`) so it isn't told
            // a message landed when it did not.
            outbox::remove(&queued_path);
            let parsed: serde_json::Value =
                serde_json::from_str(&resp_body).unwrap_or(serde_json::Value::Null);
            let delivered = parsed.get("delivered").and_then(|v| v.as_bool());
            if delivered == Some(false) {
                // Only a code reaches the caller (the answer and the hint);
                // any other value goes to this daemon's log.
                let raw_reason = parsed.get("reason");
                let reason = match raw_reason.and_then(|v| v.as_str()) {
                    Some(r) if is_reason_code(r) => r,
                    _ => {
                        if let Some(raw) = raw_reason {
                            k2_core::log_debug!(
                                "[federation/send] peer {} declined with a reason that is not a code; \
                                 passed on as declined_by_peer. Raw: {}",
                                peer.fingerprint,
                                reason_for_log(raw)
                            );
                        }
                        "declined_by_peer"
                    }
                };
                let target = format!("{agent}::{}", peer_host(&peer));
                CliResponse::ok_json(
                    serde_json::json!({
                        "status": "declined",
                        "delivered": false,
                        "reason": reason,
                        "msg_uuid": msg_uuid,
                        "peer": peer.fingerprint,
                        "hint": declined_hint(&target, reason),
                    })
                    .to_string(),
                )
            } else {
                let mut out = serde_json::json!({
                    "status": "sent",
                    "msg_uuid": msg_uuid,
                    "peer": peer.fingerprint,
                });
                if parsed.get("mode").and_then(|v| v.as_str()) == Some("inbox") {
                    out["ok"] = serde_json::json!(true);
                    out["mode"] = serde_json::json!("inbox");
                    // Only fields in shape reach the caller.
                    for (key, v) in checked_inbox_answer(&parsed, &peer.fingerprint) {
                        out[key.as_str()] = v;
                    }
                }
                CliResponse::ok_json(out.to_string())
            }
        }
        // Not delivered: the message waits in this server's outbox and the
        // drain re-dials it (the next sweep, about 30 s). `delivered:false`
        // + `reason` let a caller tell this apart from "sent" (0.46.11).
        Err(e) => CliResponse::ok_json(
            serde_json::json!({
                "status": "queued",
                "delivered": false,
                "reason": e,
                "msg_uuid": msg_uuid,
                "peer": peer.fingerprint,
                "hint": format!("delivery deferred to retry: {e}")
            })
            .to_string(),
        ),
    }
}

/// `k2 msg <agent>::<host> --inbox-wake` — resolve the user form against
/// pinned peers + that peer's roster, then [`handle_send`] a tray package
/// on the same plane as live federated msg. No Connect token.
pub fn handle_send_user_tray(
    agent: &str,
    host: &str,
    spec: federation::TraySpec,
    from_workspace: Option<String>,
    idempotency_key: Option<String>,
) -> CliResponse {
    if !federation::enabled() {
        return CliResponse::bad_request(
            "\"Connect agents across servers\" is off on this server — turn it on (Settings → Network → Tunnel → Policies, or k2 fed enable) and pair first (k2 fed pair-request)",
        );
    }
    if agent.trim().is_empty() || host.trim().is_empty() {
        return CliResponse::bad_request("agent::host required for federated tray send");
    }
    if let Err(e) = federation::decode_tray_spec(&spec) {
        return CliResponse::bad_request(e);
    }
    let store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };
    let peer = store
        .list()
        .iter()
        .find(|p| peer_matches_user_host(p, host))
        .cloned();
    let peer = match peer {
        Some(p) => p,
        None => {
            return json_err(
                "404 Not Found",
                format!("no paired peer matching {host} — pair it first (k2 fed peers)"),
            )
        }
    };
    if peer.trust != k2_core::federation::PeerTrust::Trusted {
        return json_err(
            "403 Forbidden",
            format!(
                "peer {host} is not trusted yet (state={:?}) — confirm the pairing (k2 fed confirm)",
                peer.trust
            ),
        );
    }
    let roster_resp = handle_peer_roster(&peer.fingerprint);
    if !roster_resp.status.starts_with("200") {
        return roster_resp;
    }
    let wsid = match workspace_id_from_roster_body(&roster_resp.body, agent) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let to = format!("{}::{}::{}", peer.fingerprint, wsid, agent);
    let mut body = serde_json::json!({
        "to": to,
        "tray": spec,
    });
    if let Some(obj) = body.as_object_mut() {
        if let Some(ws) = from_workspace.filter(|s| !s.trim().is_empty()) {
            obj.insert("from_workspace".into(), serde_json::json!(ws));
        }
        if let Some(k) = idempotency_key.filter(|s| !s.trim().is_empty()) {
            obj.insert(crate::send_once::PARAM.into(), serde_json::json!(k));
        }
    }
    let resp = handle_send(&body.to_string().into_bytes());
    // A7 S5: typed by another name of that workspace → tell the caller
    // the reply address (the CLI prints it as a `note:` line).
    let handle = handle_for_workspace_in_roster_body(&roster_resp.body, &wsid);
    match handle {
        Some(h) if resp.status.starts_with("200")
            && !k2_core::workspace::handle::names_loosely_match(agent, &h) =>
        {
            let mut v: serde_json::Value =
                serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null);
            match v.as_object_mut() {
                Some(obj) => {
                    obj.insert(
                        "addressNote".into(),
                        serde_json::json!(format!("{agent}::{host} is {h}::{host}")),
                    );
                    CliResponse::ok_json(v.to_string())
                }
                None => resp,
            }
        }
        _ => resp,
    }
}

/// The roster `agent` (handle) listed for `workspace_id` in a peer-roster
/// route body, if any. It becomes a peer address an agent sees and
/// answers, so only a handle ([`is_handle`]) counts: anything else is no
/// match, and the raw value goes to this daemon's log.
pub(crate) fn handle_for_workspace_in_roster_body(body: &str, workspace_id: &str) -> Option<String> {
    let d: serde_json::Value = serde_json::from_str(body).ok()?;
    let handle = roster_entries_from_json(&d)?
        .into_iter()
        .find(|e| e.workspace_id == workspace_id)
        .map(|e| e.handle)?;
    if is_handle(&handle) {
        Some(handle)
    } else {
        k2_core::log_debug!(
            "[federation] a peer roster lists a handle that is not in shape; taken as no match. Raw: {}",
            reason_for_log(&serde_json::Value::from(handle.as_str()))
        );
        None
    }
}

fn bare_host(h: &str) -> String {
    let mut h = h.trim().to_ascii_lowercase();
    while h.ends_with(".k2.dev.k2.dev") {
        let new_len = h.len().saturating_sub(k2_core::hosts::ZONE_SUFFIX.len());
        h.truncate(new_len);
    }
    if h.ends_with(k2_core::hosts::ZONE_SUFFIX) {
        let new_len = h.len().saturating_sub(k2_core::hosts::ZONE_SUFFIX.len());
        h.truncate(new_len);
    }
    h
}

pub(crate) fn peer_matches_user_host(peer: &k2_core::federation::FederationPeer, host: &str) -> bool {
    if peer_selector_matches(peer, host) {
        return true;
    }
    let want = bare_host(host);
    if want.is_empty() {
        return false;
    }
    if bare_host(&peer.subdomain) == want || bare_host(&peer.label) == want {
        return true;
    }
    if !peer.base_url.is_empty() {
        let hp = federation::host_port(&peer.base_url);
        if hp.eq_ignore_ascii_case(host) || bare_host(&hp) == want {
            return true;
        }
    }
    let sub = peer.subdomain.to_ascii_lowercase();
    host.eq_ignore_ascii_case(&sub)
        || host.eq_ignore_ascii_case(&k2_core::hosts::box_host(&sub))
        || want.eq_ignore_ascii_case(&format!("{}.k2.dev", bare_host(&peer.subdomain)))
}

fn slugify_agent(s: &str) -> String {
    // One definition with the receiver gate's matcher (CA2).
    k2_core::workspace::handle::dash_fold_name(s)
}

/// A peer's `error` as the caller may see it: its code (`error` or a
/// `code` beside it, when it is a code), else the error text capped and
/// escaped. Any other value is not passed on; the raw value goes to this
/// daemon's log.
pub(crate) fn peer_error_text(body: &serde_json::Value, err: &serde_json::Value) -> String {
    let code = [err.get("code"), body.get("code"), Some(err)]
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .find(|c| is_reason_code(c));
    match (code, err.as_str()) {
        (Some(code), _) => code.to_string(),
        (None, Some(text)) => escape_capped(text, PEER_TEXT_CAP),
        (None, None) => {
            k2_core::log_debug!(
                "[federation] a peer's error is not a code or text; not passed on. Raw: {}",
                reason_for_log(err)
            );
            "(not shown)".to_string()
        }
    }
}

pub(crate) fn workspace_id_from_roster_body(body: &str, agent: &str) -> Result<String, String> {
    let d: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("parse peer roster: {e}"))?;
    // The error of this server's roster route, or of the peer's roster it
    // carries under `roster`.
    let holder = match d.get("roster") {
        Some(r) if d.get("error").is_none() && r.get("error").is_some() => r,
        _ => &d,
    };
    if let Some(err) = holder.get("error") {
        return Err(format!("peer roster error: {}", peer_error_text(holder, err)));
    }
    let agents = d
        .get("roster")
        .and_then(|r| r.get("agents"))
        .or_else(|| d.get("agents"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let want = slugify_agent(agent);
    let want_raw = agent.trim().to_ascii_lowercase();
    let mut ids: Vec<String> = Vec::new();
    for a in &agents {
        let handle = a
            .get("agent")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let wname = a
            .get("workspace_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let aliases: Vec<String> = a
            .get("aliases")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let hit = slugify_agent(handle) == want
            || handle.eq_ignore_ascii_case(&want_raw)
            || slugify_agent(wname) == want
            || aliases.iter().any(|x| {
                slugify_agent(x) == want || x.trim().eq_ignore_ascii_case(&want_raw)
            });
        if !hit {
            continue;
        }
        if let Some(id) = a.get("workspace_id").and_then(|v| v.as_str()) {
            if !id.is_empty() && !ids.iter().any(|e| e == id) {
                ids.push(id.to_string());
            }
        }
    }
    match ids.len() {
        0 => {
            // Only handles are named back to the caller.
            let names: Vec<&str> = agents
                .iter()
                .filter_map(|a| a.get("agent").and_then(|v| v.as_str()))
                .filter(|h| is_handle(h))
                .collect();
            if names.is_empty() {
                Err(format!("no agent \"{agent}\" on that host (its roster is empty)"))
            } else {
                Err(format!(
                    "no agent \"{agent}\" on that host. Available: {}.",
                    names.join(", ")
                ))
            }
        }
        1 => Ok(ids.remove(0)),
        _ => Err(format!(
            "ambiguous agent \"{agent}\" on that host — multiple workspaces expose it"
        )),
    }
}

/// Resolve a peer's inbound base URL. Prefer the stored per-peer base URL
/// (F2). `K2_FEDERATION_INBOUND_BASE` overrides it for local/loopback testing.
/// `pub(crate)` so the outbox drain dials the SAME target a direct send would.
pub(crate) fn peer_base_url(peer: &k2_core::federation::FederationPeer) -> String {
    peer_base_url_from(&peer.subdomain, &peer.base_url)
}

/// Same as [`peer_base_url`] from the raw store fields (tests / drain).
pub(crate) fn peer_base_url_from(subdomain: &str, base_url: &str) -> String {
    if let Ok(base) = std::env::var("K2_FEDERATION_INBOUND_BASE") {
        if !base.trim().is_empty() {
            return base.trim().trim_end_matches('/').to_string();
        }
    }
    federation::resolve_peer_base_url(subdomain, base_url)
}

/// Dial URL after the F6/F7 gate. Empty / Connect-zone under air-gap → Err
/// (no SYN).
pub(crate) fn gated_peer_base_url(
    peer: &k2_core::federation::FederationPeer,
) -> Result<String, String> {
    let base = peer_base_url(peer);
    federation::assert_may_dial(&base)?;
    Ok(base)
}

pub(crate) fn peer_selector_matches(peer: &k2_core::federation::FederationPeer, sel: &str) -> bool {
    if peer.fingerprint == sel || peer.label == sel || peer.subdomain == sel {
        return true;
    }
    if !peer.base_url.is_empty() {
        let hp = federation::host_port(&peer.base_url);
        if hp == sel || peer.base_url == sel {
            return true;
        }
    }
    false
}

/// The peer's full HOST (no scheme/path) — the right-hand side of the
/// `<agent>::<host>` connection address (F5: saved host:port, not `name::lan`).
pub(crate) fn peer_host(peer: &k2_core::federation::FederationPeer) -> String {
    let base = peer_base_url(peer);
    if !base.is_empty() {
        return k2_core::connections::normalize_remote_host(&federation::host_port(&base));
    }
    federation::peer_address_host(&peer.subdomain, &peer.base_url)
}

/// POST a sealed envelope to `<base>/cli/federation/inbound`. Blocking
/// (reqwest::blocking). A non-2xx or transport error returns `Err` so the
/// caller leaves the message queued for retry. On a 2xx the peer's RESPONSE
/// BODY is returned so the caller can distinguish a real delivery from a
/// receive-side DECLINE (a 200 with `delivered:false` / `mode:"declined"`).
pub fn post_inbound(base: &str, envelope_bytes: &[u8]) -> Result<String, String> {
    let url = format!("{base}/cli/federation/inbound");
    let mut builder = reqwest::blocking::Client::builder()
        // Never follow a redirect: a peer must not steer us to another host.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20));
    // Loopback (127.0.0.1, ::1, localhost) and plain http never use a
    // proxy; HTTPS to a remote K2 may use the box's proxy settings.
    if k2_core::proxy_rule::direct(&url) {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|e| format!("build http client: {e}"))?;
    let resp = send_with_retry(DialRetry::NeverSent, std::time::Duration::from_secs(20), || {
        client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(envelope_bytes.to_vec())
    })
    .map_err(|e| format!("POST {url}: {}", error_chain(&e)))?;
    let status = resp.status();
    if status.is_success() {
        // Return the body (best-effort): an unreadable body is NOT a delivery
        // failure — the transport succeeded — so fall back to empty.
        Ok(resp.text().unwrap_or_default())
    } else {
        Err(format!("peer rejected inbound: HTTP {}", status.as_u16()))
    }
}

/// Pauses before the 2nd and 3rd try of a federation dial whose request
/// failed at the transport (0.46.11). A peer reached through K2 Connect
/// sometimes drops a fresh connection (tunnel reconnect, a busy box); one
/// quick retry turns that into a delivery instead of a "deferred" or a
/// failed roster lookup.
pub(crate) const DIAL_RETRY_DELAYS_MS: [u64; 2] = [300, 1200];

/// Which transport failures a dial retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialRetry {
    /// Only failures where the request never left (connect, DNS, TLS
    /// handshake). For a POST: a retry can never deliver twice.
    NeverSent,
    /// Any transport failure without a response, timeouts excepted. For an
    /// idempotent GET (the roster).
    AnyTransport,
}

/// True when `e` is a transport failure that `policy` retries. A timeout is
/// never retried: it already spent the caller's time budget.
pub(crate) fn retryable_transport(e: &reqwest::Error, policy: DialRetry) -> bool {
    if e.is_timeout() || e.status().is_some() {
        return false;
    }
    match policy {
        DialRetry::NeverSent => e.is_connect(),
        DialRetry::AnyTransport => e.is_connect() || e.is_request(),
    }
}

/// Send the request `build` makes, retrying per `policy` after
/// [`DIAL_RETRY_DELAYS_MS`], never starting a try past `budget` (the
/// caller's own time cap). Blocking.
pub(crate) fn send_with_retry(
    policy: DialRetry,
    budget: std::time::Duration,
    build: impl Fn() -> reqwest::blocking::RequestBuilder,
) -> Result<reqwest::blocking::Response, reqwest::Error> {
    let started = std::time::Instant::now();
    let mut attempt = 0;
    loop {
        match build().send() {
            Ok(resp) => return Ok(resp),
            Err(e)
                if attempt < DIAL_RETRY_DELAYS_MS.len()
                    && retryable_transport(&e, policy)
                    && started.elapsed()
                        + std::time::Duration::from_millis(DIAL_RETRY_DELAYS_MS[attempt])
                        < budget =>
            {
                k2_core::log_debug!(
                    "[federation] dial attempt {} failed ({}); retrying",
                    attempt + 1,
                    error_chain(&e)
                );
                std::thread::sleep(std::time::Duration::from_millis(DIAL_RETRY_DELAYS_MS[attempt]));
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// An error and every cause under it, `outer: inner: innermost`. reqwest's
/// own text stops at "error sending request for url (…)" and hides why (a
/// reset, a TLS failure, a refused connect); the cause is what an owner
/// needs to see.
pub(crate) fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        let text = c.to_string();
        if !text.is_empty() && !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        cur = c.source();
    }
    out
}

// ─────────────────────────────────────────────────────────────────────
// P5 — roster (peer-facing) + local roster helpers for the renderer
// ─────────────────────────────────────────────────────────────────────

/// `GET /cli/federation/roster?fp&ts&sig` — peer-facing roster projection.
///
/// Authenticated by a SIGNED CHALLENGE, not a token (the GET carries no signed
/// envelope; DECISION-2 — never `token_ok`/owner). The calling peer signs
/// `roster_challenge(fp, ts)` with its pinned key; we verify the signature,
/// bound `ts` by a skew window, and gate with `require_peer(fp, "roster")`.
/// Every failure is a generic 403 DENY (no reason enumeration). On success we
/// return THIS daemon's exposed agents (read-only; no spawn).
pub fn handle_roster(fp: Option<&str>, ts: Option<&str>, sig: Option<&str>) -> CliResponse {
    let (fp, ts, sig) = match (fp, ts, sig) {
        (Some(f), Some(t), Some(s)) if !f.is_empty() && !t.is_empty() && !s.is_empty() => (f, t, s),
        // Fail-closed: a roster read MUST present peer authentication.
        _ => return json_err("403 Forbidden", "roster requires peer authentication"),
    };
    let ts: i64 = match ts.parse() {
        Ok(v) => v,
        Err(_) => return CliResponse::bad_request("ts must be a unix timestamp"),
    };
    let store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };
    match roster::verify_roster_request(&store, fp, ts, sig, roster::DEFAULT_ROSTER_SKEW_SECS) {
        Ok(_peer) => {
            let projection = roster::build_local_roster();
            match serde_json::to_string(&projection) {
                Ok(body) => CliResponse::ok_json(body),
                Err(e) => json_err("500 Internal Server Error", format!("serialize roster: {e}")),
            }
        }
        // Generic deny — never leak which check failed (no peer enumeration).
        Err(_) => json_err("403 Forbidden", "roster denied"),
    }
}

/// `GET /cli/federation/peers` — OWNER-gated (dispatcher enforces). Lists the
/// LOCALLY pinned peers (the renderer matches the active host against these to
/// decide whether to surface the cross-server agent picker). Secrets (the
/// pinned public key, epoch) are omitted — only what the picker needs.
pub fn handle_peers() -> CliResponse {
    let store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };
    // 0.45.2: outbound address notices (this box told the peer it moved).
    // An unreadable outbox file only hides that column; the list still loads.
    let notices = k2_core::federation::address_notice::rows().unwrap_or_else(|e| {
        k2_core::log_debug!("[federation] peers: address-notice outbox unreadable: {e}");
        Default::default()
    });
    // 0.46.1: A to Z by the name shown, then host (the order every client
    // shows: Settings, the Connections picker, `k2 fed peers`), not the
    // order they were paired in.
    let peers: Vec<serde_json::Value> = crate::federation_rosters::peers_in_order(store.list())
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "fingerprint": p.fingerprint,
                "label": p.label,
                "subdomain": p.subdomain,
                "base_url": p.base_url,
                "trust": serde_json::to_value(p.trust).unwrap_or(serde_json::Value::Null),
                "capabilities": p.capabilities,
                "address_notice": crate::federation_address::notice_view(&notices, &p.fingerprint),
                "address_updated_at": p.address_updated_at.map(|t| t.to_rfc3339()),
                // 0.45.2: v1 = paired with the fingerprint-only code.
                "pairing_version": p.pairing_version,
                "repair_recommended": p.pairing_version
                    < k2_core::federation::pairing_v2::PAIRING_VERSION,
            })
        })
        .collect();
    CliResponse::ok_json(serde_json::json!({ "peers": peers }).to_string())
}

/// `GET /cli/federation/outbox` — OWNER-gated (dispatcher enforces). The
/// truthful-surface companion of the outbox drain: per peer, how many
/// messages are QUEUED (with the oldest message's age) and how many are
/// DEAD-LETTERED (terminal failures kept for inspection, with their
/// reasons). Covers every fingerprint that has outbox state, joined against
/// the pinned-peer store for label/subdomain when known.
pub fn handle_outbox() -> CliResponse {
    let store = PeerStore::load().unwrap_or_default();
    let now = chrono::Utc::now();

    // Every fingerprint with any outbox presence (queued OR dead).
    let mut fps: std::collections::BTreeSet<String> = outbox::peers_with_queued().into_iter().collect();
    for p in store.list() {
        if !outbox::list_dead_for_peer(&p.fingerprint).is_empty() {
            fps.insert(p.fingerprint.clone());
        }
    }

    let mut total_queued = 0usize;
    let peers: Vec<serde_json::Value> = fps
        .iter()
        .map(|fp| {
            let queued = outbox::list_for_peer_ordered(fp);
            total_queued += queued.len();
            let oldest_age_secs = queued
                .first()
                .map(|i| (now - i.signal_at).num_seconds().max(0));
            let dead = outbox::list_dead_for_peer(fp);
            let peer = store.get(fp);
            serde_json::json!({
                "fingerprint": fp,
                "label": peer.map(|p| p.label.clone()).unwrap_or_default(),
                "subdomain": peer.map(|p| p.subdomain.clone()).unwrap_or_default(),
                "queued": queued.len(),
                "oldest_age_secs": oldest_age_secs,
                "dead": dead.len(),
                "dead_letters": dead
                    .iter()
                    .map(|d| serde_json::json!({
                        "msg_uuid": d.msg_uuid,
                        "reason": d.reason,
                        "dead_at": d.dead_at.to_rfc3339(),
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();

    CliResponse::ok_json(
        serde_json::json!({ "peers": peers, "total_queued": total_queued }).to_string(),
    )
}

/// `GET /cli/federation/peer-roster?peer=<selector>` — OWNER-gated (dispatcher
/// enforces). The renderer convenience seam: the LOCAL daemon dials a PAIRED
/// peer's `/cli/federation/roster` (signing the challenge with our own key as
/// that peer's pinned counterpart) and returns the peer's agent projection so
/// the renderer can populate the dropdown. Blocking (network I/O).
pub fn handle_peer_roster(peer_selector: &str) -> CliResponse {
    if peer_selector.is_empty() {
        return CliResponse::bad_request("peer selector required");
    }
    let store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };
    // Resolve the peer by fingerprint, label, OR subdomain (identity is the
    // key; label/subdomain are convenience selectors — mirrors handle_send).
    let peer = store
        .list()
        .iter()
        .find(|p| peer_selector_matches(p, peer_selector))
        .cloned();
    let peer = match peer {
        Some(p) => p,
        None => return json_err("404 Not Found", format!("no pinned peer matches '{peer_selector}'")),
    };
    if peer.trust != k2_core::federation::PeerTrust::Trusted {
        return json_err(
            "403 Forbidden",
            format!("peer '{peer_selector}' is not Trusted (state={:?})", peer.trust),
        );
    }

    let key = match k2_core::federation::identity::load_or_create() {
        Ok(k) => k,
        Err(e) => return json_err("500 Internal Server Error", format!("load keypair: {e}")),
    };
    let ts = chrono::Utc::now().timestamp();
    let (fp, sig) = match roster::sign_roster_request(&key, ts) {
        Ok(v) => v,
        Err(e) => return json_err("500 Internal Server Error", format!("sign roster request: {e}")),
    };

    let roster_base = match gated_peer_base_url(&peer) {
        Ok(b) => b,
        Err(e) => return json_err("403 Forbidden", e),
    };
    match get_peer_roster(&roster_base, &fp, ts, &sig, std::time::Duration::from_secs(20)) {
        Ok(body) => {
            let mut parsed: serde_json::Value =
                serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
            // A7 S2: heal + bind this peer's rows (scoped to the peer, CA6).
            let _ = heal_rows_from_roster(&peer, &parsed);
            // 0.46.1: A to Z here, at the local seam, so a peer on an older
            // version (which sends its sidebar order) is sorted too.
            crate::federation_rosters::sort_roster_json(&mut parsed);
            CliResponse::ok_json(
                serde_json::json!({ "peer": peer.fingerprint, "roster": parsed }).to_string(),
            )
        }
        Err(e) => json_err("502 Bad Gateway", format!("fetch peer roster: {e}")),
    }
}

// ─────────────────────────────────────────────────────────────────────
// A7 — connections bind to the remote workspace id
// ─────────────────────────────────────────────────────────────────────

/// Cap on the roster fetch the send gate does for an unbound row (CA10).
/// An offline peer must not hold a send for the 20 s roster client.
const GATE_ROSTER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The parts of a peer roster body the heal needs. Accepts both the bare
/// projection (`{"agents":[…]}`) and the peer-roster route's wrapper
/// (`{"roster":{"agents":[…]}}`). `None` when there is no agents array
/// (an error body is not a successful fetch, CA9).
fn roster_entries_from_json(parsed: &serde_json::Value) -> Option<Vec<RosterEntry>> {
    let agents = parsed
        .get("agents")
        .or_else(|| parsed.get("roster").and_then(|r| r.get("agents")))
        .and_then(|v| v.as_array())?;
    let s = |a: &serde_json::Value, k: &str| {
        a.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string()
    };
    Some(
        agents
            .iter()
            .filter_map(|a| {
                let handle = s(a, "agent");
                let workspace_id = s(a, "workspace_id");
                if handle.is_empty() || workspace_id.is_empty() {
                    return None;
                }
                Some(RosterEntry {
                    workspace_id,
                    handle,
                    aliases: a
                        .get("aliases")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default(),
                    workspace_name: s(a, "workspace_name"),
                })
            })
            .collect(),
    )
}

/// Fetch `peer`'s roster (signed challenge), bounded by `timeout`.
fn fetch_peer_roster(
    peer: &k2_core::federation::FederationPeer,
    timeout: std::time::Duration,
) -> Result<serde_json::Value, String> {
    let key = k2_core::federation::identity::load_or_create()
        .map_err(|e| format!("load keypair: {e}"))?;
    let ts = chrono::Utc::now().timestamp();
    let (fp, sig) = roster::sign_roster_request(&key, ts)?;
    let base = gated_peer_base_url(peer)?;
    let body = get_peer_roster(&base, &fp, ts, &sig, timeout)?;
    serde_json::from_str(&body).map_err(|e| format!("parse peer roster: {e}"))
}

/// A peer's roster entries with only handles in them: an entry whose
/// handle is not a handle ([`is_handle`]) is skipped, and so is an alias
/// that is not one. What is skipped goes to this daemon's log. Healing,
/// binding and the connection check read rosters through this.
pub(crate) fn checked_roster(entries: Vec<RosterEntry>) -> Vec<RosterEntry> {
    entries
        .into_iter()
        .filter_map(|mut e| {
            if !is_handle(&e.handle) {
                k2_core::log_debug!(
                    "[federation] a peer roster entry's handle is not in shape; skipped. Raw: {}",
                    reason_for_log(&serde_json::Value::from(e.handle.as_str()))
                );
                return None;
            }
            e.aliases.retain(|a| {
                let ok = is_handle(a);
                if !ok {
                    k2_core::log_debug!(
                        "[federation] a peer roster alias is not in shape; skipped. Raw: {}",
                        reason_for_log(&serde_json::Value::from(a.as_str()))
                    );
                }
                ok
            });
            Some(e)
        })
        .collect()
}

/// Heal + bind `peer`'s rows from a fetched roster body (S2). `None` when
/// the body has no agents array (nothing is touched).
pub(crate) fn heal_rows_from_roster(
    peer: &k2_core::federation::FederationPeer,
    parsed: &serde_json::Value,
) -> Option<HealReport> {
    let roster = checked_roster(roster_entries_from_json(parsed)?);
    let host_matches = peer_row_host_matcher(peer);
    let scope = RosterPeerScope {
        fingerprint: &peer.fingerprint,
        host_matches: &host_matches,
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let report =
        k2_core::workspace::handle::heal_remote_connections_from_roster(&conn, &scope, &roster);
    if report != HealReport::default() {
        k2_core::log_debug!(
            "[federation] roster heal for {}: {report:?}",
            peer.fingerprint
        );
    }
    Some(report)
}

/// The cross-daemon send gate by identity (A7 S3 / CA3 / CA10). `Ok` to
/// send; `Err` is the 403 `not_connected` response.
pub(crate) fn remote_send_gate_check(
    from_ws: &str,
    peer: &k2_core::federation::FederationPeer,
    to_workspace_id: &str,
    typed_name: &str,
) -> Result<(), CliResponse> {
    use k2_core::connections::{remote_send_gate, RemoteSendGate};
    let host_matches = peer_row_host_matcher(peer);
    let scope = RosterPeerScope {
        fingerprint: &peer.fingerprint,
        host_matches: &host_matches,
    };
    match remote_send_gate(from_ws, &scope, to_workspace_id, typed_name) {
        RemoteSendGate::Allowed => Ok(()),
        RemoteSendGate::Denied => {
            Err(not_connected_response(from_ws, peer, to_workspace_id, typed_name, None))
        }
        RemoteSendGate::NeedsBind { name_match } => {
            let fetched = fetch_peer_roster(peer, GATE_ROSTER_TIMEOUT).and_then(|parsed| {
                roster_entries_from_json(&parsed)
                    .map(|roster| (parsed, roster))
                    .ok_or_else(|| "peer answered without a roster".to_string())
            });
            match fetched {
                Ok((parsed, roster)) => {
                    // Bind first, then decide by id: a successful fetch is
                    // the only thing that ends the name rule for a row.
                    let _ = heal_rows_from_roster(peer, &parsed);
                    match remote_send_gate(from_ws, &scope, to_workspace_id, typed_name) {
                        RemoteSendGate::Allowed => Ok(()),
                        _ => Err(not_connected_response(
                            from_ws,
                            peer,
                            to_workspace_id,
                            typed_name,
                            Some(&roster),
                        )),
                    }
                }
                Err(e) => {
                    // CA10: the peer is unreachable. Keep today's name rule so
                    // the send queues for the outbox; the receiver's gate
                    // (CA1) still refuses a forged workspace id on delivery.
                    k2_core::log_debug!(
                        "[federation] send gate: roster fetch for {} failed ({e}); name rule → {}",
                        peer.fingerprint,
                        if name_match { "pass" } else { "deny" }
                    );
                    if name_match {
                        Ok(())
                    } else {
                        Err(not_connected_response(from_ws, peer, to_workspace_id, typed_name, None))
                    }
                }
            }
        }
    }
}

/// 403 `not_connected` with a hint that names the connection the caller
/// does have on that host, when there is one (A7 S3).
fn not_connected_response(
    from_ws: &str,
    peer: &k2_core::federation::FederationPeer,
    to_workspace_id: &str,
    typed_name: &str,
    roster: Option<&[RosterEntry]>,
) -> CliResponse {
    let host = peer_host(peer);
    let typed_addr = format!("{typed_name}::{host}");
    let host_matches = peer_row_host_matcher(peer);
    let scope = RosterPeerScope {
        fingerprint: &peer.fingerprint,
        host_matches: &host_matches,
    };
    let rows = k2_core::connections::remote_rows_for_peer(from_ws, &scope);
    // The address the target workspace answers to, when the roster told us.
    let target_handle = roster
        .and_then(|r| r.iter().find(|e| e.workspace_id == to_workspace_id))
        .map(|e| e.handle.clone())
        .unwrap_or_else(|| typed_name.to_string());
    let add_hint = format!(
        "add it with `k2 connections add {target_handle}::{host}` (or ask your human: \
         Settings → Workspaces / Agents → <workspace> → Connections)"
    );

    // 0159: a row saved under the typed name whose bound workspace left
    // this peer's roster. It never follows a new workspace with that name.
    let gone = rows.iter().find(|r| {
        r.remote_workspace_gone_at.is_some()
            && (k2_core::workspace::handle::names_loosely_match(typed_name, &r.agent)
                || r.remote_handle.as_deref().is_some_and(|h| {
                    k2_core::workspace::handle::names_loosely_match(typed_name, h)
                }))
    });
    if let Some(r) = gone {
        return remote_workspace_gone_response(r, &host);
    }
    // A row saved under the typed name but bound to another workspace.
    let bound_elsewhere = rows.iter().find(|r| {
        r.remote_workspace_id.is_some()
            && k2_core::workspace::handle::names_loosely_match(typed_name, &r.agent)
    });
    // A row saved under a display name that several peer workspaces share.
    let ambiguous = roster.and_then(|roster| {
        rows.iter()
            .filter(|r| r.remote_workspace_id.is_none())
            .find_map(|r| {
                let hits = k2_core::workspace::handle::roster_display_matches(&r.agent, roster);
                (hits.len() > 1).then(|| (r.agent.clone(), hits))
            })
    });
    let hint = if let Some(r) = bound_elsewhere {
        let handle = r.remote_handle.clone().unwrap_or_else(|| r.agent.clone());
        format!(
            "you're connected to {handle}::{host} as {agent}; that's a different workspace — \
             {add_hint}.",
            agent = r.agent
        )
    } else if let Some((agent, hits)) = ambiguous {
        let names: Vec<String> = hits
            .iter()
            .map(|e| format!("{}::{host}", e.handle))
            .collect();
        format!(
            "your connection {agent}::{host} matches {} workspaces on {host} ({}), so it can't \
             tell which one you mean — add the one you want by its handle: \
             `k2 connections add <handle>::{host}`.",
            hits.len(),
            names.join(", ")
        )
    } else {
        format!("'{typed_addr}' is not a connection — {add_hint}.")
    };
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": "not_connected", "hint": hint },
        })
        .to_string(),
    }
}

/// 0159: 410 `remote_workspace_gone` for a send through a connection whose
/// workspace is no longer on the peer's roster (removed, re-created, or
/// hidden there). Handles are never reused, so the connection does not
/// move to another workspace that has the name now.
fn remote_workspace_gone_response(
    row: &k2_core::db::schema::WorkspaceRemoteConnection,
    host: &str,
) -> CliResponse {
    let name = row.remote_handle.clone().unwrap_or_else(|| row.agent.clone());
    let hint = format!(
        "the workspace you connected to as {agent}::{host} is no longer on {host} (it was \
         removed, re-created, or hidden there). A connection never moves to a different \
         workspace that has the name {name} now. If {host} lists that workspace again, this \
         connection works again. To reach a different workspace there, ask your human to \
         check it and add it again: `k2 connections add <handle>::{host}`.",
        agent = row.agent,
    );
    CliResponse {
        status: "410 Gone",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": "remote_workspace_gone", "hint": hint },
        })
        .to_string(),
    }
}

/// CA11: when a peer proves it's online, bind its unbound rows in the
/// background (one roster fetch, at most once per 10 min per peer).
pub(crate) fn bind_unbound_rows_soon(fp: &str) {
    #[cfg(test)]
    if !AUTO_BIND_IN_TESTS.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    const MIN_GAP: std::time::Duration = std::time::Duration::from_secs(600);
    static LAST: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
    > = std::sync::OnceLock::new();
    let last = LAST.get_or_init(Default::default);
    if let Ok(map) = last.lock() {
        if map.get(fp).is_some_and(|t| t.elapsed() < MIN_GAP) {
            return;
        }
    }
    let Some(peer) = trusted_peer_by_fingerprint(fp) else {
        return;
    };
    if !peer_has_unbound_rows(&peer) {
        return;
    }
    if let Ok(mut map) = last.lock() {
        map.insert(fp.to_string(), std::time::Instant::now());
    }
    let _ = std::thread::Builder::new()
        .name("fed-bind".into())
        .spawn(move || match bind_unbound_rows_now(&peer) {
            Ok(r) => k2_core::log_debug!("[federation] auto-bind {}: {r:?}", peer.fingerprint),
            Err(e) => k2_core::log_debug!("[federation] auto-bind {}: {e}", peer.fingerprint),
        });
}

/// Tests turn the CA11 background bind on explicitly so inbound tests
/// never start roster fetches of their own.
#[cfg(test)]
pub(crate) static AUTO_BIND_IN_TESTS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn trusted_peer_by_fingerprint(fp: &str) -> Option<k2_core::federation::FederationPeer> {
    let store = PeerStore::load().ok()?;
    store
        .get(fp)
        .filter(|p| p.trust == k2_core::federation::PeerTrust::Trusted)
        .cloned()
}

fn peer_has_unbound_rows(peer: &k2_core::federation::FederationPeer) -> bool {
    let host_matches = peer_row_host_matcher(peer);
    let scope = RosterPeerScope {
        fingerprint: &peer.fingerprint,
        host_matches: &host_matches,
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceRemoteConnection::list_all(&conn)
        .unwrap_or_default()
        .iter()
        .any(|r| {
            r.remote_workspace_id.is_none()
                && scope.owns_row(&r.host, r.peer_fingerprint.as_deref())
        })
}

/// One roster fetch + heal for `peer` (CA11 worker; also used at
/// `connections add`). Errors leave every binding as it was.
pub(crate) fn bind_unbound_rows_now(
    peer: &k2_core::federation::FederationPeer,
) -> Result<HealReport, String> {
    let parsed = fetch_peer_roster(peer, GATE_ROSTER_TIMEOUT)?;
    heal_rows_from_roster(peer, &parsed).ok_or_else(|| "peer roster has no agents list".to_string())
}

/// `GET /cli/connections?action=resolve&target=` (0.46.4 field report:
/// `k2 agent hire --dry-run` showed `--connect agent::host` as "would
/// apply", then the real run failed `not_found`). Read only, no source
/// workspace needed. Answers `{ok, kind: local|remote, …}` or a refusal
/// `{ok:false, error:{code, hint}}`:
/// - a local workspace: found, or `not_found` / `ambiguous` (the add's
///   did-you-mean words);
/// - `agent::host`: `not_found` when no Trusted federation peer routes to
///   the host (federation off included); else the peer's roster is read
///   (5 s cap): the agent on it is found, a roster without it is
///   `not_found` (with the names it lists), two display-name matches are
///   `ambiguous`, and a peer that doesn't answer is found with
///   `onRoster: null` and a `warning` (the add would store it unbound).
///
/// `full` = the caller may see what the add would see (owner, admin, an
/// agent). Otherwise (a Member): a remote target is 403 `owner_only`
/// before anything is looked up, and a local one says found or not found
/// without the workspace's path or id.
pub(crate) fn connections_resolve(target: &str, full: bool) -> CliResponse {
    use k2_core::connections::{resolve_connect_target, ConnectTarget};
    let refuse = |status: &'static str, code: &str, hint: String| CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    };
    if !full && k2_core::connections::is_remote_target(target) {
        return refuse(
            "403 Forbidden",
            "owner_only",
            "looking up an agent on another server reads that server's roster: the owner, an admin or an agent does it".into(),
        );
    }
    let found = match resolve_connect_target(target) {
        Ok(t) => t,
        // A Member gets found / not found only: the did-you-mean and
        // ambiguous answers name other workspaces' handles.
        Err(("bad_usage", hint)) if !full => return refuse("400 Bad Request", "bad_usage", hint),
        Err(_) if !full => return refuse("404 Not Found", "not_found", format!("no workspace '{}' here", target.trim())),
        Err(("bad_usage", hint)) => return refuse("400 Bad Request", "bad_usage", hint),
        Err(("ambiguous", hint)) => return refuse("409 Conflict", "ambiguous", hint),
        Err((code, hint)) => return refuse("404 Not Found", code, hint),
    };
    let (address, agent, host, fp) = match found {
        ConnectTarget::Local { project_id, path, name } => {
            let v = if full {
                serde_json::json!({ "ok": true, "kind": "local", "target": name, "path": path, "projectId": project_id })
            } else {
                serde_json::json!({ "ok": true, "kind": "local", "target": name })
            };
            return CliResponse::ok_json(v.to_string());
        }
        ConnectTarget::Remote { address, agent, host, peer_fingerprint } => (address, agent, host, peer_fingerprint),
    };
    let not_paired = |why: &str| {
        refuse(
            "404 Not Found",
            "not_found",
            format!(
                "{address}: {why}, so nothing would reach {host}. Pair that server first (k2 fed pair-start on one \
                 side, k2 fed pair-join on the other), then connect"
            ),
        )
    };
    if !k2_core::federation::enabled() {
        return not_paired("\"Connect agents across servers\" is off on this server");
    }
    let Some(peer) = fp.as_deref().and_then(trusted_peer_by_fingerprint) else {
        return not_paired("no paired server answers for that host");
    };
    let remote = |on_roster: serde_json::Value, extra: serde_json::Value| {
        let mut v = serde_json::json!({
            "ok": true, "kind": "remote", "target": address, "address": address, "agent": agent,
            "host": host, "peerFingerprint": peer.fingerprint, "onRoster": on_roster,
        });
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                o.insert(k.clone(), x.clone());
            }
        }
        CliResponse::ok_json(v.to_string())
    };
    let roster = match fetch_peer_roster(&peer, GATE_ROSTER_TIMEOUT).map(|p| roster_entries_from_json(&p)) {
        Ok(Some(r)) => checked_roster(r),
        Ok(None) => {
            return remote(serde_json::Value::Null, serde_json::json!({ "warning": format!("{host} answered without a roster; couldn't check {agent}") }))
        }
        Err(e) => {
            return remote(
                serde_json::Value::Null,
                serde_json::json!({ "warning": format!("couldn't reach {host} to check {agent} ({e}); a connection would be saved and bind when it answers") }),
            )
        }
    };
    if let Some(e) = roster
        .iter()
        .find(|e| k2_core::workspace::handle::roster_entry_matches(&agent, &e.handle, &e.aliases, ""))
    {
        return remote(serde_json::json!(true), serde_json::json!({ "handle": e.handle, "remoteWorkspaceId": e.workspace_id }));
    }
    let display = roster_display_matches(&agent, &roster);
    match display.len() {
        1 => remote(
            serde_json::json!(true),
            serde_json::json!({ "handle": display[0].handle, "remoteWorkspaceId": display[0].workspace_id }),
        ),
        0 => {
            let names: Vec<&str> = roster.iter().map(|e| e.handle.as_str()).collect();
            refuse(
                "404 Not Found",
                "not_found",
                if names.is_empty() {
                    format!("{host} lists no agents right now, so {agent} isn't there")
                } else {
                    format!("{agent} isn't on {host}'s roster (it lists: {})", names.join(", "))
                },
            )
        }
        _ => refuse(
            "409 Conflict",
            "ambiguous",
            format!(
                "{agent} matches {} workspaces on {host} ({}); name it by handle",
                display.len(),
                display.iter().map(|e| e.handle.as_str()).collect::<Vec<_>>().join(", ")
            ),
        ),
    }
}

/// CA8: after `k2 connections add <agent>::<host>` stored a row, try to
/// bind it right away. Warn, never refuse: the row stays either way.
/// Returns fields to merge into the add response.
pub(crate) fn bind_after_connection_add(project_path: &str, target: &str) -> serde_json::Value {
    let Ok((agent, host)) = k2_core::connections::parse_remote_addr(target.trim()) else {
        return serde_json::json!({});
    };
    let store = PeerStore::load().unwrap_or_default();
    let Some(peer) = store
        .list()
        .iter()
        .find(|p| {
            p.trust == k2_core::federation::PeerTrust::Trusted && peer_matches_user_host(p, &host)
        })
        .cloned()
    else {
        return serde_json::json!({});
    };
    let host = peer_host(&peer);
    let parsed = match fetch_peer_roster(&peer, GATE_ROSTER_TIMEOUT) {
        Ok(p) => p,
        Err(e) => {
            return serde_json::json!({
                "bound": false,
                "bindWarning": format!(
                    "couldn't reach {host} to check that name ({e}); the connection is saved \
                     and binds to that workspace the next time {host} is reachable"
                ),
            })
        }
    };
    let Some(roster) = roster_entries_from_json(&parsed).map(checked_roster) else {
        return serde_json::json!({
            "bound": false,
            "bindWarning": format!("{host} answered without a roster; the connection is saved unbound"),
        });
    };
    let _ = heal_rows_from_roster(&peer, &parsed);
    let host_matches = peer_row_host_matcher(&peer);
    let scope = RosterPeerScope {
        fingerprint: &peer.fingerprint,
        host_matches: &host_matches,
    };
    let row = k2_core::connections::remote_rows_for_peer(project_path, &scope)
        .into_iter()
        .find(|r| {
            k2_core::workspace_session_handles::address_tokens_match(&agent, &r.agent)
                || r.remote_handle.as_deref().is_some_and(|h| {
                    k2_core::workspace_session_handles::address_tokens_match(&agent, h)
                })
        });
    match row.and_then(|r| r.remote_workspace_id.clone().map(|id| (r, id))) {
        Some((r, id)) => serde_json::json!({
            "bound": true,
            "remoteWorkspaceId": id,
            "handle": r.remote_handle,
            "displayName": r.remote_display_name,
        }),
        None => {
            let display = roster_display_matches(&agent, &roster);
            let warning = if display.len() > 1 {
                format!(
                    "{agent} matches {} workspaces on {host} ({}); messages can't tell which one \
                     you mean — add it by handle instead",
                    display.len(),
                    display.iter().map(|e| e.handle.as_str()).collect::<Vec<_>>().join(", ")
                )
            } else {
                let names: Vec<&str> = roster.iter().map(|e| e.handle.as_str()).collect();
                if names.is_empty() {
                    format!("{host} lists no agents right now; the connection is saved unbound")
                } else {
                    format!(
                        "{agent} isn't on {host}'s roster (it lists: {}); the connection is saved, \
                         but messages to it won't go through until the name matches",
                        names.join(", ")
                    )
                }
            };
            serde_json::json!({ "bound": false, "bindWarning": warning })
        }
    }
}

/// True for a stored remote-connection `host` that routes to `peer`: the
/// host this daemon dials for it (what the send gate compares), or any
/// spelling of its subdomain / saved base URL.
fn peer_row_host_matcher(
    peer: &k2_core::federation::FederationPeer,
) -> impl Fn(&str) -> bool + '_ {
    let dial_host = peer_host(peer);
    move |h: &str| {
        let n = k2_core::connections::normalize_remote_host(h);
        (!n.is_empty() && n == dial_host) || k2_core::connections::host_routes_to_peer(h, peer)
    }
}

/// Why a roster GET failed, kept apart so the Connections picker can say
/// "offline" for a server that never answered and something else for one
/// that answered with a refusal (0.46.1 `peer-rosters`).
#[derive(Debug)]
pub(crate) enum RosterFetchError {
    /// No answer: connect refused, DNS, TLS, or the time cap ran out.
    Transport { timed_out: bool, message: String },
    /// The peer answered with a non-2xx status.
    Status(u16),
    /// The peer answered 2xx but the body could not be read.
    Body(String),
}

impl std::fmt::Display for RosterFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RosterFetchError::Transport { message, .. } => f.write_str(message),
            RosterFetchError::Status(code) => write!(f, "peer rejected roster: HTTP {code}"),
            RosterFetchError::Body(message) => f.write_str(message),
        }
    }
}

/// GET `<base>/cli/federation/roster?fp&ts&sig` and return the body. Blocking
/// (reqwest::blocking); a non-2xx or transport error returns `Err`.
fn get_peer_roster(
    base: &str,
    fp: &str,
    ts: i64,
    sig: &str,
    timeout: std::time::Duration,
) -> Result<String, String> {
    get_peer_roster_typed(base, fp, ts, sig, timeout).map_err(|e| e.to_string())
}

/// [`get_peer_roster`] with the failure kind kept (see [`RosterFetchError`]).
/// `timeout` bounds the whole request, connect included.
pub(crate) fn get_peer_roster_typed(
    base: &str,
    fp: &str,
    ts: i64,
    sig: &str,
    timeout: std::time::Duration,
) -> Result<String, RosterFetchError> {
    let url = format!("{base}/cli/federation/roster");
    let mut builder = reqwest::blocking::Client::builder()
        // Never follow a redirect: a peer must not steer us to another host.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(timeout);
    // Loopback (127.0.0.1, ::1, localhost) and plain http never use a
    // proxy; HTTPS to a remote K2 may use the box's proxy settings.
    if k2_core::proxy_rule::direct(&url) {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|e| RosterFetchError::Body(format!("build http client: {e}")))?;
    let ts_s = ts.to_string();
    let resp = send_with_retry(DialRetry::AnyTransport, timeout, || {
        client.get(&url).query(&[("fp", fp), ("ts", ts_s.as_str()), ("sig", sig)])
    })
    .map_err(|e| RosterFetchError::Transport {
        timed_out: e.is_timeout(),
        message: format!("GET {url}: {}", error_chain(&e)),
    })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(RosterFetchError::Status(status.as_u16()));
    }
    read_roster_body(resp)
}

/// The largest roster body read from a paired server. One roster entry
/// (id, display name, handle, address, a couple of aliases) is about 250
/// bytes, so 1 MiB holds about 4,000 workspaces: far more than a server
/// lists, while a peer can't make this server hold an unbounded answer.
pub(crate) const ROSTER_MAX_BYTES: u64 = 1024 * 1024;

/// Read a roster answer of at most [`ROSTER_MAX_BYTES`]; a bigger one is a
/// `Body` error and is not read further.
fn read_roster_body(resp: reqwest::blocking::Response) -> Result<String, RosterFetchError> {
    use std::io::Read;
    let too_big = || RosterFetchError::Body(format!("the roster is over the {ROSTER_MAX_BYTES} byte cap; not read"));
    if resp.content_length().is_some_and(|n| n > ROSTER_MAX_BYTES) {
        return Err(too_big());
    }
    let mut buf = Vec::new();
    resp.take(ROSTER_MAX_BYTES + 1).read_to_end(&mut buf).map_err(|e| {
        let timed_out = e.kind() == std::io::ErrorKind::TimedOut
            || e.get_ref().and_then(|i| i.downcast_ref::<reqwest::Error>()).is_some_and(reqwest::Error::is_timeout);
        if timed_out {
            RosterFetchError::Transport { timed_out: true, message: format!("read roster body: {e}") }
        } else {
            RosterFetchError::Body(format!("read roster body: {e}"))
        }
    })?;
    if buf.len() as u64 > ROSTER_MAX_BYTES {
        return Err(too_big());
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
#[path = "federation_routes_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "federation_reach_tests.rs"]
mod reach_tests;
