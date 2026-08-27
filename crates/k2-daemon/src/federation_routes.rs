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
use crate::workspace_msg;

use k2_core::awareness::{AgentAddress, AgentSignal, Delivery, SignalKind, WorkspaceId};
use k2_core::federation::{self, ingress, outbox, pairing, roster, PeerStore};

/// Default hop budget stamped on outbound envelopes (Risk M2).
const OUTBOUND_TTL: u8 = 8;

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

/// `POST /cli/federation/pair/request` — UNAUTH body. A peer presents its
/// identity; we create (or re-see) a `Pending` row keyed by the fingerprint
/// derived from the presented key. Returns the fingerprint + SAS the owner
/// compares out-of-band. Creates ONLY `Pending` — never trusts.
pub fn handle_pair_request(body: &[u8]) -> CliResponse {
    #[derive(serde::Deserialize)]
    struct Req {
        label: Option<String>,
        subdomain: Option<String>,
        /// F3: `--url` / `baseUrl` (LAN/Tailscale). Alias `url` for CLI.
        #[serde(default, alias = "baseUrl", alias = "url")]
        base_url: Option<String>,
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
    let mut store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };
    let pair_req = pairing::PairRequest {
        label: req.label.unwrap_or_default(),
        subdomain: req.subdomain.unwrap_or_default(),
        base_url: req.base_url.unwrap_or_default(),
        public_key_pem: req.public_key_pem,
    };
    let outcome = match pairing::apply_pair_request(&mut store, &pair_req, &local_fp) {
        Ok(o) => o,
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Err(e) = store.save() {
        return json_err("500 Internal Server Error", format!("save peer store: {e}"));
    }
    CliResponse::ok_json(
        serde_json::json!({
            "fingerprint": outcome.fingerprint,
            "sas": outcome.sas,
            "trust": "pending",
            "created": outcome.created,
            "local_fingerprint": local_fp,
        })
        .to_string(),
    )
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
    let mut store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };
    if let Err(e) =
        pairing::apply_pair_confirm(&mut store, &req.fingerprint, &req.sas, &local_fp, &req.capabilities)
    {
        // SAS mismatch / blocked / unknown → fail-closed 403.
        return json_err("403 Forbidden", e);
    }
    if let Err(e) = store.save() {
        return json_err("500 Internal Server Error", format!("save peer store: {e}"));
    }
    CliResponse::ok_json(
        serde_json::json!({ "ok": true, "fingerprint": req.fingerprint, "trust": "trusted" })
            .to_string(),
    )
}

/// `GET /cli/federation/pubkey` — OWNER-gated (dispatcher enforces). Returns
/// THIS daemon's federation identity so a peer can PIN it during pairing:
///
///   - `public_key_pem` — the SPKI PEM of the existing tunnel keypair (no new
///     key material; same key `seal`/`pair` derive their fingerprint from).
///   - `fingerprint`    — `sha256(SPKI DER)` hex (== [`federation::local_fingerprint`]).
///   - `subdomain`      — this daemon's tunnel subdomain (or `""` when none).
///
/// The renderer's owner-driven auto-pair reads BOTH daemons' pubkey here (it
/// holds an owner token for each) so it can present each side's key to the
/// other and confirm the SAS programmatically — no human out-of-band step.
pub fn handle_pubkey() -> CliResponse {
    let key = match k2_core::tunnel::tls::load_or_generate_keypair() {
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
    CliResponse::ok_json(
        serde_json::json!({
            "public_key_pem": key.public_key_pem(),
            "fingerprint": fingerprint,
            "subdomain": subdomain,
            // Pair body carries the URL to dial back (F3). Empty when we
            // only have a Connect subdomain (or loopback-unknown LAN IP).
            "base_url": "",
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
///        - opt-in ON  → [`workspace_msg::deliver_live`] (inject if live,
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
        Err(e) => {
            // Fail-closed: map each reject to a status. Auth/authorization
            // failures are 403; malformed/bad-sig are 400; replay/skew/ttl/loop
            // are 409 (the request was well-formed but not acceptable now).
            let status = match e {
                ingress::IngressError::UnknownPeer
                | ingress::IngressError::NotTrusted
                | ingress::IngressError::CapabilityDenied => "403 Forbidden",
                ingress::IngressError::Decode(_) | ingress::IngressError::BadSignature => {
                    "400 Bad Request"
                }
                ingress::IngressError::Replay
                | ingress::IngressError::SkewTooLarge { .. }
                | ingress::IngressError::TtlExpired
                | ingress::IngressError::LoopDetected => "409 Conflict",
            };
            return json_err(status, e);
        }
    };

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
    let ws_id = match &signal.to {
        AgentAddress::Agent { workspace, .. } => workspace.0.clone(),
        other => {
            return json_err(
                "400 Bad Request",
                format!("inbound signal is not addressed to an agent: {other:?}"),
            );
        }
    };
    let project_path = match resolve_project_path_by_id(&ws_id) {
        Some(p) => p,
        None => {
            return json_err(
                "404 Not Found",
                format!("addressed workspace '{ws_id}' is not registered on this server"),
            );
        }
    };

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
    let from_label = federated_from_label(sender_agent, &host);

    // The deliverable text (federation sends `Msg`; render any other kind
    // so nothing is silently lost).
    let text = match &signal.kind {
        SignalKind::Msg { text } => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };

    // CONSENT GATE — the recipient workspace's per-project "allow remote
    // instruction" opt-in is the consent for canonical-chat DRIVE.
    let opt_in = k2_core::workspace::settings::remote_instruct_allowed_for_path(&project_path);

    if opt_in {
        // Identical to local `k2 msg`: inject if live, wake+resume+inject if
        // dormant. (wake=true, like `k2 talk`.)
        let result = workspace_msg::deliver_live(
            &project_path,
            &text,
            &from_label,
            "",
            true,
            workspace_msg::DEFAULT_WAKE_TIMEOUT,
        );
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

/// Canonical federated `[from …]` attribution shown in the recipient chat.
///
/// **Always** `agent::host` (lowercase). Never `@` — that sigil made
/// receiving agents treat the handle as mail and call `k2 mail` instead of
/// `k2 msg` (GH #47). Empty host → bare agent; empty agent → `"peer"`.
fn federated_from_label(sender_agent: &str, host: &str) -> String {
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
fn resolve_project_path_by_id(workspace_id: &str) -> Option<String> {
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

/// `POST /cli/federation/send` — dual-auth (dispatcher enforces owner-or-admin
/// OR scoped passport). Body: `{"to":"<peer>::<workspace>::<agent>","text":"..."}`.
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
    // target `<agent>::<host>` (PR2 canonical; gate also matches legacy `@`
    // rows). This is IN ADDITION to the trust check above — trust says
    // "I've paired with this peer"; the connection says "THIS workspace is
    // allowed to message THIS remote agent". Scoped principals always have
    // from_workspace forced, so the gate always runs for agents.
    // Owner-remote `k2 talk` (no principal, no body from_workspace) is the
    // ungated path; Trusted peer check still always runs.
    if let Some(from_ws) = from_workspace.as_deref() {
        let remote_addr = format!("{agent}::{}", peer_host(&peer));
        if !k2_core::connections::is_remote_connection(from_ws, &remote_addr) {
            // C2 teaching shape parity with local not_connected (mail/dns):
            // stable code + hint so CLI exit-3 mappers recognize it.
            return CliResponse {
                status: "403 Forbidden",
                content_type: "application/json",
                body: serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": "not_connected",
                        "hint": format!(
                            "'{remote_addr}' is not a connection — add it with \
                             `k2 connections add {remote_addr}` (or ask your human: \
                             Settings → Connections)."
                        ),
                    },
                })
                .to_string(),
            };
        }
    }

    let key = match k2_core::tunnel::tls::load_or_generate_keypair() {
        Ok(k) => k,
        Err(e) => return json_err("500 Internal Server Error", format!("load keypair: {e}")),
    };
    let local_fp = federation::local_fingerprint().unwrap_or_default();

    // `from` carries the SENDER AGENT IDENTITY so the recipient's chat can
    // render `[from <agent>::<host>]` and reply with `k2 msg <agent>::<host>`.
    // In the workspace==agent model the agent name is the source workspace's
    // AUTHORITATIVE agent name — persona/display `name:` first, folder
    // basename fallback — resolved via `resolve_agent_name`. Using the
    // resolved name (not the bare basename) keeps the `[from]` attribution +
    // reply target consistent with what the recipient's roster sees, so a
    // reply to `<agent>::<host>` matches the connection row (gate is
    // case-insensitive + separator-normalized). Owner-remote sends
    // (`k2 talk`, no `from_workspace`) carry no source agent → attributed to
    // `owner`. The whole signal is signed end-to-end, so this identity is
    // AUTHENTICATED to the verified peer (no forgeable plaintext `[from]`,
    // Risk C1).
    let from = match from_workspace.as_deref() {
        Some(src) => AgentAddress::Agent {
            workspace: WorkspaceId(src.to_string()),
            name: k2_core::workspace::agent_identity::resolve_agent_name(src)
                .map(|n| n.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    Path::new(src)
                        .file_name()
                        .map(|s| s.to_string_lossy().to_ascii_lowercase())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| src.to_ascii_lowercase())
                }),
        },
        None => AgentAddress::Agent {
            workspace: WorkspaceId(format!("peer:{local_fp}")),
            name: "owner".to_string(),
        },
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
            SignalKind::Msg { text: req.text },
        )
    };

    let dial_base = match gated_peer_base_url(&peer) {
        Ok(b) => b,
        Err(e) => return json_err("403 Forbidden", e),
    };

    let bytes = match federation::seal(&signal, &key, &peer.subdomain, OUTBOUND_TTL) {
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
                let reason = parsed
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("declined_by_peer");
                let target = format!("{agent}::{}", peer_host(&peer));
                CliResponse::ok_json(
                    serde_json::json!({
                        "status": "declined",
                        "delivered": false,
                        "reason": reason,
                        "msg_uuid": msg_uuid,
                        "peer": peer.fingerprint,
                        "hint": format!("'{target}' declined the message: {reason}"),
                    })
                    .to_string(),
                )
            } else {
                CliResponse::ok_json(
                    serde_json::json!({ "status": "sent", "msg_uuid": msg_uuid, "peer": peer.fingerprint })
                        .to_string(),
                )
            }
        }
        Err(e) => CliResponse::ok_json(
            serde_json::json!({
                "status": "queued",
                "msg_uuid": msg_uuid,
                "peer": peer.fingerprint,
                "hint": format!("delivery deferred to retry: {e}")
            })
            .to_string(),
        ),
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

fn peer_selector_matches(peer: &k2_core::federation::FederationPeer, sel: &str) -> bool {
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
fn peer_host(peer: &k2_core::federation::FederationPeer) -> String {
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
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(envelope_bytes.to_vec())
        .send()
        .map_err(|e| format!("POST {url}: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        // Return the body (best-effort): an unreadable body is NOT a delivery
        // failure — the transport succeeded — so fall back to empty.
        Ok(resp.text().unwrap_or_default())
    } else {
        Err(format!("peer rejected inbound: HTTP {}", status.as_u16()))
    }
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
    let peers: Vec<serde_json::Value> = store
        .list()
        .iter()
        .map(|p| {
            serde_json::json!({
                "fingerprint": p.fingerprint,
                "label": p.label,
                "subdomain": p.subdomain,
                "base_url": p.base_url,
                "trust": serde_json::to_value(p.trust).unwrap_or(serde_json::Value::Null),
                "capabilities": p.capabilities,
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

    let key = match k2_core::tunnel::tls::load_or_generate_keypair() {
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
    match get_peer_roster(&roster_base, &fp, ts, &sig) {
        Ok(body) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
            lazy_heal_connections_from_roster_json(&parsed);
            CliResponse::ok_json(
                serde_json::json!({ "peer": peer.fingerprint, "roster": parsed }).to_string(),
            )
        }
        Err(e) => json_err("502 Bad Gateway", format!("fetch peer roster: {e}")),
    }
}

/// D10: rewrite stored connection agents that already match a roster
/// handle or alias. Never attach leftover `sales` to "the only agent".
fn lazy_heal_connections_from_roster_json(parsed: &serde_json::Value) {
    let agents = parsed
        .get("agents")
        .or_else(|| parsed.get("roster").and_then(|r| r.get("agents")))
        .and_then(|v| v.as_array());
    let Some(agents) = agents else {
        return;
    };
    let mut roster: Vec<(String, Vec<String>)> = Vec::new();
    for a in agents {
        let handle = a
            .get("agent")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if handle.is_empty() {
            continue;
        }
        let aliases = a
            .get("aliases")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        roster.push((handle, aliases));
    }
    if roster.is_empty() {
        return;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace::handle::heal_remote_connections_from_roster(&conn, &roster);
}

/// GET `<base>/cli/federation/roster?fp&ts&sig` and return the body. Blocking
/// (reqwest::blocking); a non-2xx or transport error returns `Err`.
fn get_peer_roster(base: &str, fp: &str, ts: i64, sig: &str) -> Result<String, String> {
    let url = format!("{base}/cli/federation/roster");
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let resp = client
        .get(&url)
        .query(&[("fp", fp), ("ts", &ts.to_string()), ("sig", sig)])
        .send()
        .map_err(|e| format!("GET {url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("peer rejected roster: HTTP {}", status.as_u16()));
    }
    resp.text().map_err(|e| format!("read roster body: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::with_temp_home;
    use k2_core::federation::{FederationPeer, PeerTrust};

    fn body(v: serde_json::Value) -> Vec<u8> {
        v.to_string().into_bytes()
    }

    #[test]
    fn pair_request_then_owner_confirm_promotes_to_trusted() {
        with_temp_home(|| {
            // A peer presents a fresh key.
            let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let pem = peer_key.public_key_pem();

            let req = handle_pair_request(&body(serde_json::json!({
                "label": "rosson@laptop",
                "subdomain": "rosson",
                "public_key_pem": pem,
            })));
            assert_eq!(req.status, "200 OK", "request body: {}", req.body);
            let v: serde_json::Value = serde_json::from_str(&req.body).unwrap();
            let fp = v["fingerprint"].as_str().unwrap().to_string();
            let sas = v["sas"].as_str().unwrap().to_string();
            assert_eq!(v["trust"], "pending");

            // The store has it Pending (delivers nothing yet).
            let store = PeerStore::load().unwrap();
            assert_eq!(store.get(&fp).unwrap().trust, PeerTrust::Pending);

            // Owner confirms with the matching SAS → Trusted + caps.
            let conf = handle_pair_confirm(&body(serde_json::json!({
                "fingerprint": fp,
                "sas": sas,
            })));
            assert_eq!(conf.status, "200 OK", "confirm body: {}", conf.body);
            let store = PeerStore::load().unwrap();
            let p = store.get(&fp).unwrap();
            assert_eq!(p.trust, PeerTrust::Trusted);
            assert!(p.capabilities.contains("inbound"));
        });
    }

    #[test]
    fn pair_confirm_rejects_wrong_sas() {
        with_temp_home(|| {
            let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let req = handle_pair_request(&body(serde_json::json!({
                "label": "p", "subdomain": "p", "public_key_pem": peer_key.public_key_pem(),
            })));
            let v: serde_json::Value = serde_json::from_str(&req.body).unwrap();
            let fp = v["fingerprint"].as_str().unwrap();
            let conf = handle_pair_confirm(&body(serde_json::json!({
                "fingerprint": fp, "sas": "000000",
            })));
            assert_eq!(conf.status, "403 Forbidden", "wrong SAS must 403");
            // Still Pending.
            let store = PeerStore::load().unwrap();
            assert_eq!(store.get(fp).unwrap().trust, PeerTrust::Pending);
        });
    }

    #[test]
    fn inbound_rejects_unknown_peer() {
        with_temp_home(|| {
            // Seal an envelope from a key nobody pinned.
            let key = k2_core::tunnel::tls::load_or_generate_keypair().unwrap();
            let signal = AgentSignal::new(
                AgentAddress::Broadcast,
                AgentAddress::Agent {
                    workspace: WorkspaceId("ws".into()),
                    name: "bob".into(),
                },
                SignalKind::Msg { text: "hi".into() },
            );
            let bytes = federation::seal(&signal, &key, "peer", 8).unwrap();
            let resp = handle_inbound(&bytes);
            assert_eq!(resp.status, "403 Forbidden", "unknown peer must reject: {}", resp.body);
        });
    }

    /// Register a local project row; returns its UUID. The verified `to`
    /// address carries this UUID and `handle_inbound` resolves it → path.
    fn register_project(path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, "fed-recv-ws", path],
        )
        .unwrap();
        id
    }

    /// A unique temp workspace path (not created on disk; inbox::compose
    /// makes the `.k2/inbox/` dir on demand).
    fn temp_ws_path() -> String {
        std::env::temp_dir()
            .join(format!("k2-fed-recv-{}-{}", std::process::id(), uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned()
    }

    /// Pin a DISTINCT peer key (not the local key, or the loop guard trips
    /// on self-in-trace) as Trusted+inbound under `subdomain`. Returns the
    /// peer keypair so the caller can seal an envelope with it.
    fn pin_trusted_peer(subdomain: &str) -> rcgen::KeyPair {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::default();
        let fp = store.upsert(FederationPeer::pin("peer", subdomain, key.public_key_pem()).unwrap());
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();
        key
    }

    /// Seal `text` from `cortana` (peer-attributed at inbound as
    /// `cortana::<host>`) addressed to agent `bob` in `ws_id`.
    fn seal_msg_to(ws_id: &str, key: &rcgen::KeyPair, subdomain: &str, text: &str) -> Vec<u8> {
        let signal = AgentSignal::new(
            AgentAddress::Agent {
                workspace: WorkspaceId("/src/cortana".into()),
                name: "cortana".into(),
            },
            AgentAddress::Agent {
                workspace: WorkspaceId(ws_id.into()),
                name: "bob".into(),
            },
            SignalKind::Msg { text: text.into() },
        );
        federation::seal(&signal, key, subdomain, 8).unwrap()
    }

    // ── P3 receive: opt-in OFF → DECLINE. "Allow remote instruction = OFF"
    //    means remote messages must NOT instruct this agent, and the
    //    `.k2/inbox/` is inherently TRUSTED work — so the verified message is
    //    written NOWHERE trusted and the sender is told it did not land
    //    (200, `delivered:false`, `mode:"declined"`). The security gate still
    //    passed (the peer is Trusted+inbound); only the trusted-inbox write is
    //    gone.
    #[test]
    fn inbound_opt_in_off_declines_no_trusted_inbox() {
        with_temp_home(|| {
            k2_core::db::init_for_tests();
            let ws_path = temp_ws_path();
            let ws_id = register_project(&ws_path); // opt-in defaults OFF

            let key = pin_trusted_peer("peer");
            let bytes = seal_msg_to(&ws_id, &key, "peer", "hello over the wire");
            let resp = handle_inbound(&bytes);
            // Received-but-not-delivered: the dial succeeds so the sender can
            // tell a clean decline from a transport/security failure.
            assert_eq!(resp.status, "200 OK", "valid envelope dial must succeed: {}", resp.body);

            let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
            assert_eq!(v["delivered"], false, "opt-in OFF must NOT report delivered");
            assert_eq!(v["mode"], "declined", "opt-in OFF must decline delivery");
            assert_eq!(
                v["reason"], "remote_instruction_disabled",
                "decline must carry the explicit reason"
            );

            // NOTHING was written to the TRUSTED inbox under `<ws>/.k2/inbox/`.
            let items = k2_core::inbox::list_root(Path::new(&ws_path));
            assert!(
                items.is_empty(),
                "opt-in OFF must NOT write the trusted inbox, found {}",
                items.len()
            );
        });
    }

    // ── P3 receive: opt-in ON → drive the canonical chat via the SAME
    //    `deliver_live` local `k2 msg` uses (NOT the inbox). Here the
    //    workspace has no agent/session so deliver_live reports
    //    `no_agent_mode` — but crucially it took the LIVE path and wrote NO
    //    inbox file (the orphaned-awareness-inbox bug regression-lock).
    #[test]
    fn inbound_opt_in_on_routes_to_live_delivery() {
        with_temp_home(|| {
            k2_core::db::init_for_tests();
            let ws_path = temp_ws_path();
            let ws_id = register_project(&ws_path);
            // Opt THIS workspace in to remote instruction.
            k2_core::workspace::settings::update_project_setting(
                &ws_path,
                "allow_remote_instruct",
                "1",
            )
            .expect("opt in");

            let key = pin_trusted_peer("peer");
            let bytes = seal_msg_to(&ws_id, &key, "peer", "drive my chat");
            let resp = handle_inbound(&bytes);
            assert_eq!(resp.status, "200 OK", "must deliver: {}", resp.body);

            let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
            assert_eq!(v["mode"], "live", "opt-in ON must route to the canonical chat");

            // The LIVE path was taken — nothing was written to the inbox.
            let items = k2_core::inbox::list_root(Path::new(&ws_path));
            assert!(items.is_empty(), "opt-in ON must NOT write an inbox file");
        });
    }

    // ── P3 receive: durable msg_uuid dedupe. The outbox drain is
    //    at-least-once — a redelivery of an already-handled envelope (same
    //    SIGNED msg_uuid, possibly re-sealed with a fresh nonce so the
    //    in-memory replay LRU can't catch it) must 409 as a duplicate
    //    instead of driving the chat / decline path twice. The drain maps
    //    that 409 back to "already delivered".
    #[test]
    fn inbound_redelivery_of_handled_msg_uuid_is_409_duplicate() {
        with_temp_home(|| {
            k2_core::db::init_for_tests();
            let ws_path = temp_ws_path();
            let ws_id = register_project(&ws_path); // opt-in OFF → decline (terminal)

            let key = pin_trusted_peer("peer");
            let bytes = seal_msg_to(&ws_id, &key, "peer", "deliver once");
            let first = handle_inbound(&bytes);
            assert_eq!(first.status, "200 OK", "first delivery: {}", first.body);
            let v: serde_json::Value = serde_json::from_str(&first.body).unwrap();
            assert_eq!(v["mode"], "declined", "opt-in OFF declines (terminal)");

            // Redeliver the SAME message RE-SEALED (fresh nonce + at — the
            // exact thing the drain does for stale envelopes), so only the
            // durable msg_uuid store can catch it.
            let resealed = federation::reseal(&bytes, &key).expect("reseal");
            let second = handle_inbound(&resealed);
            assert_eq!(
                second.status, "409 Conflict",
                "redelivered msg_uuid must dedupe: {}",
                second.body
            );
            assert!(
                second.body.contains("duplicate message"),
                "the dedupe reject must carry the drain's marker: {}",
                second.body
            );
        });
    }

    // ── P3 receive: a message addressed to a workspace NOT on this server is
    //    rejected (resolution failure, after the security gate passed).
    #[test]
    fn inbound_unknown_workspace_is_rejected() {
        with_temp_home(|| {
            k2_core::db::init_for_tests();
            let key = pin_trusted_peer("peer");
            // No project registered for this UUID.
            let bytes = seal_msg_to(&uuid::Uuid::new_v4().to_string(), &key, "peer", "nowhere");
            let resp = handle_inbound(&bytes);
            assert_eq!(resp.status, "404 Not Found", "unknown workspace must reject: {}", resp.body);
        });
    }

    // ── P4 outbound: seal + durable enqueue + dial + POST over the wire ──

    /// Stand up a loopback TCP stub mimicking a peer's
    /// `/cli/federation/inbound`. It reads one HTTP request, captures the body,
    /// answers 200, and ships the captured body back via a oneshot. Returns the
    /// bound port + the receiver.
    async fn spawn_inbound_stub() -> (u16, tokio::sync::oneshot::Receiver<Vec<u8>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind stub");
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.expect("accept");
            let mut acc: Vec<u8> = Vec::new();
            let mut clen: Option<usize> = None;
            let mut body_start: Option<usize> = None;
            let mut buf = [0u8; 4096];
            loop {
                let n = match s.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                acc.extend_from_slice(&buf[..n]);
                if body_start.is_none() {
                    if let Some(pos) = acc.windows(4).position(|w| w == b"\r\n\r\n") {
                        body_start = Some(pos + 4);
                        let head = String::from_utf8_lossy(&acc[..pos]);
                        clen = head.lines().find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        });
                    }
                }
                if let (Some(bs), Some(cl)) = (body_start, clen) {
                    if acc.len() >= bs + cl {
                        break;
                    }
                }
            }
            let body = body_start.map(|bs| acc[bs..].to_vec()).unwrap_or_default();
            let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
            let _ = s.write_all(resp.as_bytes()).await;
            let _ = s.flush().await;
            let _ = tx.send(body);
        });
        (port, rx)
    }

    /// Variant of [`spawn_inbound_stub`] that answers 200 with a CUSTOM JSON
    /// body — used to simulate the receive-side DECLINE (200 `delivered:false`).
    async fn spawn_inbound_stub_responding(resp_body: &'static str) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind stub");
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.expect("accept");
            let mut buf = [0u8; 4096];
            // Read until we see the header terminator; we don't need the body.
            let mut acc: Vec<u8> = Vec::new();
            loop {
                match s.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp_body.len(),
                resp_body
            );
            let _ = s.write_all(resp.as_bytes()).await;
            let _ = s.flush().await;
        });
        port
    }

    // ── P4 send: a receive-side DECLINE (200 `delivered:false`) must surface
    //    to the SENDER as `status:"declined"`, NOT `"sent"`. Regression lock
    //    for the 0.40.20 bug where a remote-instruction-OFF decline was
    //    reported to the sender as a successful send.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_surfaces_peer_decline_as_declined_not_sent() {
        let _home = crate::test_support::TempHome::new();

        let port = spawn_inbound_stub_responding(
            r#"{"delivered":false,"mode":"declined","reason":"remote_instruction_disabled"}"#,
        )
        .await;
        let base = format!("http://127.0.0.1:{port}");

        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::default();
        let fp = store.upsert(
            FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap(),
        );
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();

        std::env::set_var("K2_FEDERATION_INBOUND_BASE", &base);
        let send_body = body(serde_json::json!({ "to": "peer::ws-b::bob", "text": "are you in?" }));
        let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
            .await
            .unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(resp.status, "200 OK", "send body: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(
            v["status"], "declined",
            "a peer decline must NOT be reported as sent; got {}",
            resp.body
        );
        assert_eq!(v["delivered"], false, "declined send must report delivered:false");
        assert_eq!(
            v["reason"], "remote_instruction_disabled",
            "decline must propagate the peer's reason"
        );

        // A decline is terminal — the queued copy is consumed (re-dialing
        // would never flip the recipient's opt-out).
        assert!(
            outbox::list_for_peer(&fp).is_empty(),
            "a clean decline must drain the queued copy (not retry forever)"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_seals_enqueues_and_posts_to_peer_inbound() {
        // Hold the crate-wide HOME lock for the whole test (TempHome) so the
        // keypair/store/outbox live in an isolated ~/.k2 and the env override is
        // serialized against other HOME-swapping tests.
        let _home = crate::test_support::TempHome::new();

        let (port, rx) = spawn_inbound_stub().await;
        let base = format!("http://127.0.0.1:{port}");

        // Pin a Trusted peer (subdomain "peer") with a fresh key, and capture
        // OUR local pubkey to verify the sealed envelope the stub receives.
        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::default();
        let fp = store.upsert(
            FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap(),
        );
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();

        let local_key = k2_core::tunnel::tls::load_or_generate_keypair().unwrap();
        let local_pem = local_key.public_key_pem();

        // Run the blocking send on a worker (reqwest::blocking can't run on the
        // async thread). Override the dial target to our loopback stub.
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", &base);
        let send_body = body(serde_json::json!({ "to": "peer::ws-b::bob", "text": "hi over the wire" }));
        let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
            .await
            .unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(resp.status, "200 OK", "send body: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["status"], "sent", "must report sent on a 200 from the peer");

        // The peer received the EXACT sealed envelope; it opens against our
        // pinned key and carries the addressed signal.
        let received = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .expect("stub must receive within timeout")
            .expect("stub body");
        let signal = federation::open(&received, &local_pem).expect("envelope opens against sender key");
        match signal.to {
            AgentAddress::Agent { name, workspace } => {
                assert_eq!(name, "bob");
                assert_eq!(workspace.0, "ws-b");
            }
            other => panic!("unexpected to-address: {other:?}"),
        }
        // No `from_workspace` (owner-remote send) → attributed to `owner`.
        match signal.from {
            AgentAddress::Agent { name, .. } => assert_eq!(name, "owner"),
            other => panic!("unexpected from-address: {other:?}"),
        }

        // Delivered → outbox drained.
        assert!(
            outbox::list_for_peer(&fp).is_empty(),
            "a confirmed delivery must remove the queued copy"
        );
    }

    // ── P4 send: the SENDER AGENT IDENTITY is carried in `from` so the
    //    recipient can render `[from <agent>::<host>]` and reply. Regression
    //    lock for the dropped-identity bug (`from` used to be a bare
    //    `peer:<fp>` Workspace address). `from.name` = source workspace
    //    agent name (lowercased at inbound attribution).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_sets_from_agent_identity_from_source_basename() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();

        let (port, rx) = spawn_inbound_stub().await;
        let base = format!("http://127.0.0.1:{port}");

        // Source workspace whose folder basename is the agent name `cortana`.
        let src_path = std::env::temp_dir()
            .join(format!("k2-src-{}-{}", std::process::id(), uuid::Uuid::new_v4()))
            .join("cortana")
            .to_string_lossy()
            .into_owned();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), "cortana", src_path],
            )
            .unwrap();
        }

        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::default();
        let fp = store.upsert(FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap());
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();

        let local_pem = k2_core::tunnel::tls::load_or_generate_keypair().unwrap().public_key_pem();

        std::env::set_var("K2_FEDERATION_INBOUND_BASE", &base);
        // Agent-initiated send → satisfy the GAP #3 connection gate first.
        k2_core::connections::connections(
            &src_path,
            "add",
            Some(&format!("bob@127.0.0.1:{port}")),
            None,
        )
        .expect("add remote connection");

        let send_body = body(serde_json::json!({
            "to": "peer::ws-b::bob",
            "text": "from the source agent",
            "from_workspace": src_path,
        }));
        let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
            .await
            .unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(resp.status, "200 OK", "send body: {}", resp.body);

        let received = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .expect("stub must receive within timeout")
            .expect("stub body");
        let signal = federation::open(&received, &local_pem).expect("envelope opens against sender key");
        match signal.from {
            AgentAddress::Agent { name, .. } => {
                assert_eq!(name, "cortana", "from.name must be the source workspace basename");
            }
            other => panic!("from must carry the sender agent identity, got {other:?}"),
        }
    }

    /// `from.name` must come from the workspace's RESOLVED agent name
    /// (persona/display `name:` first), NOT the bare folder basename. Repro
    /// for the cross-server reply mismatch: when the persona name differs
    /// from the folder basename, the `[from]` attribution + reply target
    /// must use the resolved name so the recipient's reply matches the
    /// roster/connection.
    #[tokio::test]
    async fn send_sets_from_agent_identity_from_resolved_name_not_basename() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();

        let (port, rx) = spawn_inbound_stub().await;
        let base = format!("http://127.0.0.1:{port}");

        // Source workspace whose FOLDER BASENAME ("agent-box") deliberately
        // differs from the persona `name:` ("Aria"). The resolved name must
        // win.
        let src_path = std::env::temp_dir()
            .join(format!("k2-src-{}-{}", std::process::id(), uuid::Uuid::new_v4()))
            .join("agent-box")
            .to_string_lossy()
            .into_owned();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), "agent-box", src_path],
            )
            .unwrap();
        }
        // Write a persona file whose declared name differs from the basename.
        let agent_dir = k2_core::workspace::agent_identity::workspace_agent_path(&src_path);
        std::fs::create_dir_all(&agent_dir).expect("mkdir .k2/agent");
        std::fs::write(
            agent_dir.join("AGENT.md"),
            "---\nname: Aria\ntype: custom\n---\n# persona\n",
        )
        .expect("write AGENT.md");
        // Sanity: the resolver returns the persona name, not the basename.
        assert_eq!(
            k2_core::workspace::agent_identity::resolve_agent_name(&src_path).as_deref(),
            Some("Aria"),
        );

        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::default();
        let fp = store.upsert(FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap());
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();

        let local_pem = k2_core::tunnel::tls::load_or_generate_keypair().unwrap().public_key_pem();

        std::env::set_var("K2_FEDERATION_INBOUND_BASE", &base);
        // Satisfy the GAP #3 connection gate first.
        k2_core::connections::connections(
            &src_path,
            "add",
            Some(&format!("bob@127.0.0.1:{port}")),
            None,
        )
        .expect("add remote connection");

        let send_body = body(serde_json::json!({
            "to": "peer::ws-b::bob",
            "text": "from the resolved agent",
            "from_workspace": src_path,
        }));
        let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
            .await
            .unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(resp.status, "200 OK", "send body: {}", resp.body);

        let received = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .expect("stub must receive within timeout")
            .expect("stub body");
        let signal = federation::open(&received, &local_pem).expect("envelope opens against sender key");
        match signal.from {
            AgentAddress::Agent { name, .. } => {
                assert_eq!(
                    name, "aria",
                    "from.name must be the RESOLVED persona name (lowercased), not the folder basename"
                );
            }
            other => panic!("from must carry the sender agent identity, got {other:?}"),
        }
    }

    // GH #47 — inbound `[from …]` must use `::`, never `@`, so the receiving
    // agent routes replies through `k2 msg` rather than `k2 mail`.
    #[test]
    fn federated_from_label_uses_double_colon_never_at() {
        assert_eq!(
            federated_from_label("Cortana", "Z3thon.k2.dev"),
            "cortana::z3thon.k2.dev"
        );
        assert_eq!(
            federated_from_label("  argus  ", "  Peer.K2.DEV  "),
            "argus::peer.k2.dev"
        );
        assert_eq!(federated_from_label("cortana", ""), "cortana");
        assert_eq!(federated_from_label("", "host.k2.dev"), "peer::host.k2.dev");
        // Regression: the old `@` form must never reappear.
        assert!(!federated_from_label("a", "b.k2.dev").contains('@'));
    }

    // ── P5 roster: peer-authenticated GET + local owner helpers ──

    fn peer_key() -> rcgen::KeyPair {
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap()
    }

    #[test]
    fn roster_denies_unauthenticated_request() {
        with_temp_home(|| {
            // No fp/ts/sig → fail-closed 403 (no projection leaked).
            let resp = handle_roster(None, None, None);
            assert_eq!(resp.status, "403 Forbidden", "body: {}", resp.body);
        });
    }

    #[test]
    fn roster_denies_untrusted_peer() {
        with_temp_home(|| {
            // Pin the peer Pending (not Trusted) but granted roster → denied.
            let key = peer_key();
            let mut store = PeerStore::default();
            let fp = store.upsert(FederationPeer::pin("p", "p", key.public_key_pem()).unwrap());
            store.grant(&fp, k2_core::federation::CAP_ROSTER);
            store.save().unwrap();

            let ts = chrono::Utc::now().timestamp();
            let (signed_fp, sig) = roster::sign_roster_request(&key, ts).unwrap();
            let resp = handle_roster(
                Some(&signed_fp),
                Some(&ts.to_string()),
                Some(&sig),
            );
            assert_eq!(resp.status, "403 Forbidden", "untrusted must deny: {}", resp.body);
        });
    }

    #[test]
    fn roster_returns_agents_for_trusted_peer() {
        with_temp_home(|| {
            k2_core::db::init_for_tests();
            // A configured workspace this daemon exposes.
            let ws_path = std::env::temp_dir().join(format!(
                "k2-fedrt-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            let ws_path = ws_path.to_string_lossy().into_owned();
            let ws_id = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let id = uuid::Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO projects (id, name, path, agent_enabled) VALUES (?1, ?2, ?3, 1)",
                    rusqlite::params![id, "fed-roster-ws", ws_path],
                )
                .unwrap();
                id
            };
            // Roster contact filter: master remote may be off in tests —
            // opt this workspace into federated contact via Remote Access.
            k2_core::workspace::settings::update_project_setting(
                &ws_path,
                "allow_remote_instruct",
                "1",
            )
            .expect("opt workspace into remote contact for roster");

            // Pin the calling peer Trusted + roster, then sign a valid request.
            let key = peer_key();
            let mut store = PeerStore::default();
            let fp = store.upsert(FederationPeer::pin("peer", "peer", key.public_key_pem()).unwrap());
            store.set_trust(&fp, PeerTrust::Trusted);
            store.grant(&fp, k2_core::federation::CAP_ROSTER);
            store.save().unwrap();

            let ts = chrono::Utc::now().timestamp();
            let (signed_fp, sig) = roster::sign_roster_request(&key, ts).unwrap();
            let resp = handle_roster(Some(&signed_fp), Some(&ts.to_string()), Some(&sig));
            assert_eq!(resp.status, "200 OK", "trusted+roster must serve: {}", resp.body);

            let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
            let agents = v["agents"].as_array().expect("agents array");
            assert!(
                agents.iter().any(|a| a["workspace_id"] == ws_id),
                "the configured workspace must be exposed in the roster: {}",
                resp.body
            );
        });
    }

    #[test]
    fn peers_lists_pinned_peers() {
        with_temp_home(|| {
            let key = peer_key();
            let mut store = PeerStore::default();
            let fp = store.upsert(
                FederationPeer::pin("teammate", "rosson", key.public_key_pem()).unwrap(),
            );
            store.set_trust(&fp, PeerTrust::Trusted);
            store.grant(&fp, k2_core::federation::CAP_ROSTER);
            store.save().unwrap();

            let resp = handle_peers();
            assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
            let peers = v["peers"].as_array().expect("peers array");
            let p = peers
                .iter()
                .find(|p| p["fingerprint"] == fp)
                .expect("pinned peer must be listed");
            assert_eq!(p["subdomain"], "rosson");
            assert_eq!(p["trust"], "trusted");
            // Secrets are NOT exposed.
            assert!(p.get("public_key_pem").is_none(), "must not leak the pinned key");
        });
    }

    // ── GAP #3: pubkey identity for owner-driven auto-pair ──

    #[test]
    fn pubkey_returns_spki_pem_and_matching_fingerprint() {
        with_temp_home(|| {
            let resp = handle_pubkey();
            assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
            let pem = v["public_key_pem"].as_str().expect("public_key_pem present");
            assert!(
                pem.contains("BEGIN PUBLIC KEY"),
                "must return an SPKI PEM; got {pem}"
            );
            let fp = v["fingerprint"].as_str().expect("fingerprint present");
            // The advertised fingerprint MUST equal the local fingerprint a
            // peer would compute from the presented PEM (so the pin is
            // self-consistent).
            assert_eq!(fp, federation::local_fingerprint().unwrap());
            assert_eq!(
                fp,
                k2_core::federation::peers::fingerprint_of_spki_pem(pem).unwrap(),
                "fingerprint must be sha256(SPKI DER) of the presented PEM"
            );
            // `subdomain` is always present (empty when no tunnel configured).
            assert!(v.get("subdomain").is_some(), "subdomain key must be present");
        });
    }

    // ── GAP #3: the cross-daemon CONNECTION GATE on `handle_send` ──
    //
    // When the caller supplies `from_workspace`, the send is allowed ONLY
    // IF that source workspace is connected to the target `<agent>@<host>`
    // (a `workspace_remote_connections` row). Fail-closed: no connection ⇒
    // 403, never a dial. With a connection it falls through to the normal
    // seal/enqueue/dial path (here the dial fails on a dead port → queued,
    // which proves the gate let it through).

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_is_gated_on_source_workspace_remote_connection() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();

        // Register a LOCAL source workspace (the agent's workspace).
        let src_path = std::env::temp_dir().join(format!(
            "k2-gate-src-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let src_path = src_path.to_string_lossy().into_owned();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), "gate-src-ws", src_path],
            )
            .unwrap();
        }

        // Pin a Trusted peer (subdomain "peer", inbound granted).
        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::default();
        let fp = store.upsert(
            FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap(),
        );
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();

        // Override the dial host to a dead port → peer_host() == "127.0.0.1:1".
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", "http://127.0.0.1:1");

        // (A) No connection for the source workspace → 403, fail-closed.
        let send_body = body(serde_json::json!({
            "to": "peer::ws::bob",
            "text": "should be blocked",
            "from_workspace": src_path,
        }));
        let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
            .await
            .unwrap();
        assert_eq!(
            resp.status, "403 Forbidden",
            "an agent send without a remote connection must be rejected: {}",
            resp.body
        );
        assert!(
            resp.body.contains("not a connection"),
            "the 403 must explain the missing connection; got {}",
            resp.body
        );

        // Add the matching remote connection `bob@127.0.0.1:1` for the source.
        k2_core::connections::connections(
            &src_path,
            "add",
            Some("bob@127.0.0.1:1"),
            None,
        )
        .expect("add remote connection");

        // (B) Now the gate opens; the dead-port dial then fails → queued.
        let send_body = body(serde_json::json!({
            "to": "peer::ws::bob",
            "text": "now allowed",
            "from_workspace": src_path,
        }));
        let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
            .await
            .unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(
            resp.status, "200 OK",
            "a connected source workspace must pass the gate: {}",
            resp.body
        );
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(
            v["status"], "queued",
            "past the gate, the dead-port dial leaves the message queued: {}",
            resp.body
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_to_unreachable_peer_leaves_message_queued() {
        let _home = crate::test_support::TempHome::new();

        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::default();
        let fp = store.upsert(
            FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap(),
        );
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();

        // Point at a dead port (nothing listening) → dial fails.
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", "http://127.0.0.1:1");
        let send_body = body(serde_json::json!({ "to": "peer::ws::bob", "text": "queued please" }));
        let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
            .await
            .unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["status"], "queued", "an undeliverable send must stay queued");
        assert_eq!(
            outbox::list_for_peer(&fp).len(),
            1,
            "the durable outbox must retain the message for retry"
        );
    }

    // ── PR1: dual-auth under passports — principal-bound from_workspace ──
    //
    // A5.1 Scoped + trusted peer + connection → send ok/queued
    // A5.2 Scoped + no connection → not_connected
    // A5.4 Spoofed from_workspace ignored with principal
    // (A5.3 scoped cannot pair/confirm lives in session_token allowlist tests;
    //  A5.5 owner send still works = existing tests above.)

    /// Register a projects row and return (path, uuid) for a scoped principal.
    fn register_src_workspace(label: &str) -> (String, String) {
        let src_path = std::env::temp_dir().join(format!(
            "k2-pr1-src-{}-{}-{}",
            label,
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let src_path = src_path.to_string_lossy().into_owned();
        let uuid = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![uuid, format!("pr1-{label}"), src_path],
            )
            .unwrap();
        }
        (src_path, uuid)
    }

    fn pin_trusted_peer_for_send(subdomain: &str) -> String {
        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::default();
        let fp = store.upsert(
            FederationPeer::pin("teammate", subdomain, peer_key.public_key_pem()).unwrap(),
        );
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();
        fp
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pr1_scoped_principal_send_with_connection_queues() {
        // A5.1: principal present + trusted peer + remote connection → gate
        // opens; dead-port dial → queued.
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let (src_path, ws_uuid) = register_src_workspace("ok");
        let _fp = pin_trusted_peer_for_send("peer");
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", "http://127.0.0.1:1");
        k2_core::connections::connections(
            &src_path,
            "add",
            Some("bob@127.0.0.1:1"),
            None,
        )
        .expect("add remote connection");

        let principal = crate::session_token::HookPrincipal {
            workspace_uuid: ws_uuid,
            agent_address: "src-agent".into(),
        };
        // Body omits from_workspace — principal supplies it.
        let send_body = body(serde_json::json!({
            "to": "peer::ws::bob",
            "text": "passport send",
        }));
        let resp = tokio::task::spawn_blocking(move || {
            crate::caller_workspace::with_request_principal(Some(principal), || {
                handle_send(&send_body)
            })
        })
        .await
        .unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(
            resp.status, "200 OK",
            "scoped principal with connection must send: {}",
            resp.body
        );
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["status"], "queued", "dead-port dial → queued: {}", resp.body);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pr1_scoped_principal_send_without_connection_is_not_connected() {
        // A5.2: principal present + trusted peer + NO connection → not_connected.
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let (_src_path, ws_uuid) = register_src_workspace("noconnect");
        let _fp = pin_trusted_peer_for_send("peer");
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", "http://127.0.0.1:1");

        let principal = crate::session_token::HookPrincipal {
            workspace_uuid: ws_uuid,
            agent_address: "src-agent".into(),
        };
        let send_body = body(serde_json::json!({
            "to": "peer::ws::bob",
            "text": "blocked",
        }));
        let resp = tokio::task::spawn_blocking(move || {
            crate::caller_workspace::with_request_principal(Some(principal), || {
                handle_send(&send_body)
            })
        })
        .await
        .unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(resp.status, "403 Forbidden", "must fail closed: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(
            v["error"]["code"], "not_connected",
            "stable not_connected code for CLI exit 3: {}",
            resp.body
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pr1_scoped_principal_ignores_spoofed_from_workspace() {
        // A5.4: body claims a DIFFERENT workspace that HAS a connection;
        // principal binds the real workspace which does NOT — spoof ignored
        // → not_connected on the principal workspace.
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let (_real_path, real_uuid) = register_src_workspace("real");
        let (spoof_path, _spoof_uuid) = register_src_workspace("spoof");
        let _fp = pin_trusted_peer_for_send("peer");
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", "http://127.0.0.1:1");

        // Only the spoofed workspace is connected — if body won, send would queue.
        k2_core::connections::connections(
            &spoof_path,
            "add",
            Some("bob@127.0.0.1:1"),
            None,
        )
        .expect("add spoof connection");

        let principal = crate::session_token::HookPrincipal {
            workspace_uuid: real_uuid,
            agent_address: "real-agent".into(),
        };
        let send_body = body(serde_json::json!({
            "to": "peer::ws::bob",
            "text": "spoof attempt",
            "from_workspace": spoof_path,
        }));
        let resp = tokio::task::spawn_blocking(move || {
            crate::caller_workspace::with_request_principal(Some(principal), || {
                handle_send(&send_body)
            })
        })
        .await
        .unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(
            resp.status, "403 Forbidden",
            "spoofed from_workspace must be ignored; principal workspace has no connection: {}",
            resp.body
        );
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(
            v["error"]["code"], "not_connected",
            "must gate on principal workspace, not body spoof: {}",
            resp.body
        );
    }

    #[test]
    fn pr1_scoped_allowlist_admits_send_surface_not_pair_confirm() {
        // A5.3: scoped cannot pair/confirm (allowlist); peers/roster/send ok.
        use crate::session_token::is_agent_verb;
        assert!(is_agent_verb("/cli/federation/peers"));
        assert!(is_agent_verb("/cli/federation/peer-roster"));
        assert!(is_agent_verb("/cli/federation/send"));
        assert!(!is_agent_verb("/cli/federation/pair/confirm"));
        assert!(!is_agent_verb("/cli/federation/pair/request"));
        assert!(!is_agent_verb("/cli/federation/outbox"));
    }

    // ── Air-gap LAN federation (prd-airgap-lan-federation-v1) ──

    struct AirgapEnv(Option<std::ffi::OsString>);
    impl AirgapEnv {
        fn set(val: Option<&str>) -> Self {
            let prev = std::env::var_os("K2_AIRGAP");
            match val {
                Some(v) => std::env::set_var("K2_AIRGAP", v),
                None => std::env::remove_var("K2_AIRGAP"),
            }
            k2_core::airgap::set_setting_enabled(false);
            Self(prev)
        }
    }
    impl Drop for AirgapEnv {
        fn drop(&mut self) {
            match &self.0 {
                Some(p) => std::env::set_var("K2_AIRGAP", p),
                None => std::env::remove_var("K2_AIRGAP"),
            }
            k2_core::airgap::set_setting_enabled(false);
        }
    }

    #[test]
    fn peer_base_url_airgap_never_concatenates_k2_dev() {
        with_temp_home(|| {
            let _env = AirgapEnv::set(Some("1"));
            std::env::remove_var("K2_FEDERATION_INBOUND_BASE");
            let url = peer_base_url_from("rpm", "");
            assert!(
                !url.contains("k2.dev"),
                "air-gap peer_base_url must not append .k2.dev; got {url:?}"
            );
            let lan = peer_base_url_from("", "http://192.168.1.50:38471");
            assert_eq!(lan, "http://192.168.1.50:38471");
            assert!(!lan.contains("k2.dev"));
        });
    }

    #[cfg(not(feature = "airgap"))]
    #[test]
    fn peer_base_url_connect_unchanged_when_not_airgap() {
        with_temp_home(|| {
            let _env = AirgapEnv::set(Some("0"));
            std::env::remove_var("K2_FEDERATION_INBOUND_BASE");
            assert_eq!(peer_base_url_from("rpm", ""), "https://rpm.k2.dev");
        });
    }

    #[test]
    fn pair_request_url_then_confirm_is_trusted_no_k2_dev() {
        with_temp_home(|| {
            let _env = AirgapEnv::set(Some("1"));
            let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let req = handle_pair_request(&body(serde_json::json!({
                "label": "lan-box",
                "baseUrl": "http://192.168.1.50:38471",
                "public_key_pem": peer_key.public_key_pem(),
            })));
            assert_eq!(req.status, "200 OK", "body: {}", req.body);
            let v: serde_json::Value = serde_json::from_str(&req.body).unwrap();
            let fp = v["fingerprint"].as_str().unwrap().to_string();
            let sas = v["sas"].as_str().unwrap().to_string();
            let conf = handle_pair_confirm(&body(serde_json::json!({
                "fingerprint": fp,
                "sas": sas,
            })));
            assert_eq!(conf.status, "200 OK", "{}", conf.body);
            let store = PeerStore::load().unwrap();
            let p = store.get(&fp).unwrap();
            assert_eq!(p.trust, PeerTrust::Trusted);
            assert_eq!(p.base_url, "http://192.168.1.50:38471");
            assert!(
                !p.base_url.contains("k2.dev"),
                "stored dial URL must not contain .k2.dev"
            );
            let dial = peer_base_url(p);
            assert!(!dial.contains("k2.dev"), "got {dial}");
        });
    }

    #[test]
    fn pair_request_empty_url_and_subdomain_is_400() {
        with_temp_home(|| {
            let _env = AirgapEnv::set(Some("1"));
            let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let req = handle_pair_request(&body(serde_json::json!({
                "public_key_pem": peer_key.public_key_pem(),
            })));
            assert!(
                req.status.starts_with("400"),
                "empty url+subdomain must 400; got {} {}",
                req.status,
                req.body
            );
            assert!(PeerStore::load().unwrap().list().is_empty());
        });
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn airgap_send_to_k2_dev_refuses_without_syn() {
        let _home = crate::test_support::TempHome::new();
        let _env = AirgapEnv::set(Some("1"));
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");
        let spy = std::net::TcpListener::bind("127.0.0.1:0").expect("spy");
        spy.set_nonblocking(true).expect("nb");
        let fp = pin_trusted_peer_for_send("foo");
        let send_body = body(serde_json::json!({ "to": format!("{fp}::ws::bob"), "text": "nope" }));
        let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
            .await
            .unwrap();
        assert_eq!(resp.status, "403 Forbidden", "must refuse Connect zone: {}", resp.body);
        assert!(
            resp.body.contains("k2.dev") || resp.body.contains("Air-gap"),
            "got {}",
            resp.body
        );
        assert!(spy.accept().is_err(), "must not SYN");
        assert!(
            outbox::list_for_peer(&fp).is_empty(),
            "refused Connect send must not enqueue leftover k2.dev outbox"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn airgap_send_to_lan_http_allowed_fake_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let _home = crate::test_support::TempHome::new();
        let _env = AirgapEnv::set(Some("1"));
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.expect("accept");
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf).await;
            let body = r#"{"delivered":true,"mode":"live"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes()).await;
        });

        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::default();
        let fp = store.upsert(
            FederationPeer::pin("lan", "127.0.0.1", peer_key.public_key_pem())
                .unwrap()
                .with_base_url(format!("http://127.0.0.1:{port}")),
        );
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        let to = format!("{fp}::ws::bob");
        let send_body = body(serde_json::json!({ "to": to, "text": "lan ping" }));
        let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
            .await
            .unwrap();
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["status"], "sent", "{}", resp.body);
    }
}
