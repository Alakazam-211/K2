//! Node rosters, lanes, seats and signer pairing (0.46.10,
//! prd-ci-on-compute-for-clients-v1 C1 + C3a; vs-live V3, V11–V13, V17,
//! V19–V22, V25). The store is `k2_core::compute::trust`; the formats are
//! frozen in `k2-node-proto` (`node_roster`, `statement`, `lanes`).
//!
//! These ride the compute router (`compute_routes::handle_from`): the same
//! callers, the preview flag, both sockets. The federation leg
//! (`/cli/federation/compute-seat`) is its own route, authenticated by the
//! signed federation envelope like the vault handoff.
//!
//! **Who decides (interim decisions, pending Rosson; vs-live §6 items 2, 3):**
//! - The node-roster key is a vault engine key (`compute/node-roster`,
//!   Ed25519). The daemon signs with it only after a **person** approved
//!   the change: the owner or an Admin login, with a fresh vault confirm
//!   bound to the request (`compute-trust`). Agents and the owner token
//!   alone never do (agents can read `daemon.token`). The one exception is
//!   an **unchanged** re-sign (new `seq` and `issued_at`, the same entries)
//!   so a quiet owner's roster never goes stale for signers (V13).
//! - Existing nodes are vouched for with an [`OwnerAttestation`] (V12)
//!   instead of a forced re-key; a re-key arrives only from a node whose
//!   every server offers `node-rekey` (V3), and moves the pinned row in place.
//! - A seat's node plans are sealed with the node-admin approval key, so a
//!   seat offer is approved by the **owner** only (node admin rule Q2);
//!   roster changes, lanes and seat requests by the owner or an Admin.
//!
//! **Owner side.** `roster` (show), `roster/key`, `roster/propose` (set /
//! attest / retire / revoke a node key: held), `roster/publish` (deliver the
//! newest roster; re-sign only if unchanged and old), `lanes`, `lanes/set`
//! (held), seats offered by clients (`seats/offer` names the nodes; held),
//! `trust/approve|deny|withdraw|requests`. An approved roster is handed to
//! every owned node (`node_roster.set`, the node relays it to its tenants,
//! V11) and to every seated client over federation. An approved seat signs
//! the client's [`SeatStatement`], sends the offer, then seals one plan per
//! seat node (`lanes.write` + `servers.add` with the client's seat code) in
//! one batch (V21).
//!
//! **Client side.** `seats/request` (an agent may ask; held), whose person
//! approval pins the owner's roster key, records the secrets consent (V25),
//! mints the seat's enrol code and sends the request. Seat nodes enrol with
//! that code and are confirmed **only** when their key is active in the
//! owner's roster under a name the owner's seat lists (V20, through today's
//! node path: no node change). `owners`, `owners/consent`.
//!
//! **Signer pairing (V17).** `k2 ci signer add` → `/cli/ci/signer/add`
//! (`k2_signer::pairing::server_offer`), `reveal` (`server_sas`: the SAS is
//! answered only to a person at this server, owner or Admin with a fresh
//! confirm, never to whoever carries the messages), `finish`
//! (`server_finish`), `signers`, `signer/remove`.

use std::collections::HashMap;
use std::sync::Mutex;

use k2_core::awareness::{AgentAddress, AgentSignal, Delivery, SignalKind, WorkspaceId};
use k2_core::compute::proto::admin::ApprovalKey;
use k2_core::compute::proto::ci::{self, KeyStatus, RevokeReason, SeatState, Signed, TaintBasis};
use k2_core::compute::proto::frames::KeyRotation;
use k2_core::compute::proto::lanes::{self as plane, LaneTemplate, NodeRosterDoc};
use k2_core::compute::proto::node_roster::{NodeRoster, OwnerAttestation, SeatStatement};
use k2_core::compute::proto::{crypto, pairing, share as pshare};
use k2_core::compute::trust::{self, codes as tcodes, EntryRow, SeatRow, TrustRequest};
use k2_core::compute::{self as compute, codes, store};
use k2_core::federation::{self, ingress, FederationPeer, PeerStore, PeerTrust};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::compute_routes::{json_body, need_credential, no, s, Who, R};

pub const OWNERS: &str = "/cli/compute/owners";
pub const OWNERS_CONSENT: &str = "/cli/compute/owners/consent";
pub const ROSTER: &str = "/cli/compute/roster";
pub const ROSTER_KEY: &str = "/cli/compute/roster/key";
pub const ROSTER_PROPOSE: &str = "/cli/compute/roster/propose";
pub const ROSTER_PUBLISH: &str = "/cli/compute/roster/publish";
pub const TRUST_REQUESTS: &str = "/cli/compute/trust/requests";
pub const TRUST_APPROVE: &str = "/cli/compute/trust/approve";
pub const TRUST_DENY: &str = "/cli/compute/trust/deny";
pub const TRUST_WITHDRAW: &str = "/cli/compute/trust/withdraw";
pub const LANES: &str = "/cli/compute/lanes";
pub const LANES_SET: &str = "/cli/compute/lanes/set";
pub const SEATS: &str = "/cli/compute/seats";
pub const SEATS_REQUEST: &str = "/cli/compute/seats/request";
pub const SEATS_OFFER: &str = "/cli/compute/seats/offer";
pub const SIGNERS: &str = "/cli/ci/signers";
pub const SIGNER_ADD: &str = "/cli/ci/signer/add";
pub const SIGNER_REVEAL: &str = "/cli/ci/signer/reveal";
pub const SIGNER_FINISH: &str = "/cli/ci/signer/finish";
pub const SIGNER_REMOVE: &str = "/cli/ci/signer/remove";

/// The federation leg (envelope-authenticated, Public in the route table).
pub const FEDERATION_ROUTE: &str = "/cli/federation/compute-seat";
/// Client → owner: the owner's roster key (read only).
pub const KIND_OWNER_KEY: &str = "compute-owner-key/1";
/// Client → owner: a seat request (after the client's person approved it).
pub const KIND_SEAT_REQUEST: &str = "compute-seat-request/1";
/// Owner → client: the signed seat and the roster (after the owner's person
/// approved it).
pub const KIND_SEAT_OFFER: &str = "compute-seat-offer/1";
/// Owner → client: a newer roster and that client's seats.
pub const KIND_NODE_ROSTER: &str = "compute-node-roster/1";
/// Body cap on the federation leg.
pub const MAX_FED_BODY: usize = 512 * 1024;

pub const ROUTES: &[&str] = &[
    OWNERS,
    OWNERS_CONSENT,
    ROSTER,
    ROSTER_KEY,
    ROSTER_PROPOSE,
    ROSTER_PUBLISH,
    TRUST_REQUESTS,
    TRUST_APPROVE,
    TRUST_DENY,
    TRUST_WITHDRAW,
    LANES,
    LANES_SET,
    SEATS,
    SEATS_REQUEST,
    SEATS_OFFER,
    SIGNERS,
    SIGNER_ADD,
    SIGNER_REVEAL,
    SIGNER_FINISH,
    SIGNER_REMOVE,
];
pub const POST_ROUTES: &[&str] = &[
    OWNERS_CONSENT,
    ROSTER_KEY,
    ROSTER_PROPOSE,
    ROSTER_PUBLISH,
    TRUST_APPROVE,
    TRUST_DENY,
    TRUST_WITHDRAW,
    LANES_SET,
    SEATS_REQUEST,
    SEATS_OFFER,
    SIGNER_ADD,
    SIGNER_REVEAL,
    SIGNER_FINISH,
    SIGNER_REMOVE,
];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub(crate) fn handle(p: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<Value> {
    match p {
        OWNERS => owners(who),
        OWNERS_CONSENT => owners_consent(who, body),
        ROSTER => roster_show(who),
        ROSTER_KEY => roster_key_route(who),
        ROSTER_PROPOSE => roster_propose(who, body),
        ROSTER_PUBLISH => roster_publish(who, body),
        TRUST_REQUESTS => requests(who, params),
        TRUST_APPROVE => approve(who, body),
        TRUST_DENY => deny(who, body),
        TRUST_WITHDRAW => withdraw(who, body),
        LANES => lanes_show(who),
        LANES_SET => lanes_set(who, body),
        SEATS => seats_show(who),
        SEATS_REQUEST => seats_request(who, body),
        SEATS_OFFER => seats_offer(who, body),
        SIGNERS => signers_show(who),
        SIGNER_ADD => signer_add(who, body),
        SIGNER_REVEAL => signer_reveal(who, body),
        SIGNER_FINISH => signer_finish(who, body),
        SIGNER_REMOVE => signer_remove(who, body),
        _ => Err(no("not_found", 404, "no such route")),
    }
}

// ── who ──────────────────────────────────────────────────────────────

/// A person here: the owner token or an Admin login, never a passport (a
/// K2 tab or an agent). The confirm token proves the person behind it.
fn person(who: &Who) -> R<()> {
    need_credential(who)?;
    if who.passport().is_some() || !who.is_admin() {
        return Err(no(
            tcodes::PERSON_ONLY,
            403,
            "a person decides this (the owner or an Admin), in person: from a terminal outside K2 (k2 compute trust approve <id>), \
             never an agent or a K2 tab. An agent files the request; a person approves it.",
        ));
    }
    Ok(())
}

/// The owner in person (node admin rule Q2: a seat seals node plans with
/// the owner approval key).
fn owner_person(who: &Who) -> R<()> {
    person(who)?;
    if !who.is_owner() {
        return Err(no(
            tcodes::PERSON_ONLY,
            403,
            "only the server's owner approves a seat: it signs node admin plans with the owner's approval key",
        ));
    }
    Ok(())
}

/// Who may file or read: a person with Admin, or an agent whose workspace
/// has "Allow this agent to operate compute nodes".
fn operator(who: &Who) -> R<(String, Option<String>)> {
    crate::compute_admin::operator_check(who)?;
    Ok((who.label(), who.passport().map(|(ws, _)| ws.to_string())))
}

/// Take a person's confirm token (`action`, bound to `bound`). Returns how
/// they confirmed.
fn take_confirm(who: &Who, v: &Value, action: &str, bound: &str) -> R<String> {
    let token = s(v, "confirmToken").ok_or_else(|| {
        no(
            "vault_confirm_required",
            403,
            "this needs a fresh confirm from a person (Touch ID, your login or the vault password): the k2 command asks for it",
        )
    })?;
    let stamp = crate::compute_admin::person_stamp(who).ok_or_else(crate::compute_routes::owner_only)?;
    let vault = crate::compute_admin::trust_vault()?;
    match vault.confirm.take(token, action, Some(bound), &stamp) {
        Ok(k2_vault::HumanProof::Confirmed { method }) => Ok(method),
        _ => Err(no("vault_confirm_required", 403, "confirm again: a confirm works once, for this request, within 60 seconds")),
    }
}

fn now() -> i64 {
    compute::now()
}

fn db_err(e: impl std::fmt::Display) -> crate::compute_routes::Refusal {
    no("store", 500, e.to_string())
}

fn audit(who: &str, kind: &str, node: Option<&str>, detail: Value) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::record_event(&conn, who, kind, node, None, None, detail);
}

// ── this server's identity ───────────────────────────────────────────

/// This server as a node owner: its host (a roster `owner_id`).
fn owner_id() -> R<String> {
    #[cfg(test)]
    if let Some(id) = test_seams::owner_id() {
        return Ok(id);
    }
    let id = crate::compute_ws::controller_label().to_ascii_lowercase();
    if !ci::is_name(&id) {
        return Err(no("owner_id", 500, format!("this server's host {id:?} isn't a roster name ([a-z0-9._-])")));
    }
    Ok(id)
}

fn my_url() -> String {
    #[cfg(test)]
    if let Some(id) = test_seams::owner_id() {
        return format!("https://{id}");
    }
    format!("https://{}", crate::compute_ws::controller_label())
}

// ── the node-roster key (vault engine key) ───────────────────────────

/// Serializes roster-key use: the first use mints the key.
static ROSTER_KEY_LOCK: Mutex<()> = Mutex::new(());

/// The roster public key (minted in the vault on first use).
fn roster_key() -> R<ApprovalKey> {
    let v = crate::compute_admin::trust_vault()?;
    let _g = ROSTER_KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    v.engine_public_key(ci::ROSTER_KEY_ENGINE, ci::ROSTER_KEY_NAME)
        .map(ApprovalKey)
        .map_err(|e| no("vault_closed", e.status, format!("node-roster key: {}", e.hint)))
}

/// Sign an owner document. Called only after a person's confirm (or for an
/// unchanged roster re-sign, [`refresh_roster`]).
fn sign_owner<T: serde::Serialize>(domain: &str, doc: &T) -> R<Signed> {
    let v = crate::compute_admin::trust_vault()?;
    let body = k2_core::compute::proto::canonical::to_string(doc).map_err(|e| no("sign", 500, e))?;
    let sig = {
        let _g = ROSTER_KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        v.engine_sign(ci::ROSTER_KEY_ENGINE, ci::ROSTER_KEY_NAME, &ci::owner_message(domain, &body))
            .map_err(|e| no("vault_closed", e.status, format!("node-roster key: {}", e.hint)))?
    };
    Ok(Signed::from_owner_sig(body, &sig))
}

fn key_json(k: &ApprovalKey) -> Value {
    json!({"keyId": k.encode(), "fingerprint": k.fingerprint()})
}

// ── owner: show, key, propose, publish ───────────────────────────────

fn entry_json(e: &EntryRow) -> Value {
    let mut v = serde_json::to_value(&e.entry).unwrap_or_default();
    v["nodeId"] = e.node_id.clone().into();
    v["attested"] = e.attestation.is_some().into();
    v
}

fn roster_summary(r: &trust::StoredRoster) -> Value {
    json!({
        "ownerId": r.roster.owner_id,
        "seq": r.roster.seq,
        "issuedAt": r.roster.issued_at,
        "digest": r.signed.digest(),
        "keyFingerprint": r.roster.key_fp().unwrap_or_default(),
        "source": r.source,
        "entries": r.roster.entries.iter().map(|e| json!({
            "name": e.name, "nodeFp": e.node_fp, "keyEpoch": e.key_epoch, "status": e.status,
            "revokedReason": e.revoked_reason, "taintBasis": e.taint_basis, "host": e.host,
            "provider": e.provider, "site": e.site, "adminReach": e.admin_reach,
        })).collect::<Vec<_>>(),
    })
}

/// `k2 compute roster show`: this server's roster key, newest signed
/// roster, the working entries and what waits for a person.
fn roster_show(who: &Who) -> R<Value> {
    operator(who)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let latest = trust::latest_own_roster(&conn);
    let pending: Vec<Value> = trust::list_requests(&conn, Some("open"), None, 100).iter().filter(|r| r.kind == "roster").map(request_json).collect();
    let nodes: Vec<Value> = store::list_nodes(&conn, false)
        .into_iter()
        .filter(|n| !n.is_tenant() && n.state == "active")
        .map(|n| {
            let st = trust::statement_of(&conn, &n.id).map(|(s, st)| json!({"digest": s.digest(), "taint": st.taint, "wall": st.wall, "keyEpoch": st.key_epoch, "version": st.k2node_version}));
            let on = trust::entry_by_fp(&conn, &n.fingerprint).map(|e| e.entry.status);
            json!({"name": n.name, "fingerprint": n.fingerprint, "statement": st, "roster": on})
        })
        .collect();
    Ok(json!({
        "latest": latest.as_ref().map(roster_summary),
        "stale": trust::roster_stale(&conn, now()),
        "changed": trust::entries_changed(&conn),
        "entries": trust::entries(&conn).iter().map(entry_json).collect::<Vec<_>>(),
        "nodes": nodes,
        "pending": pending,
    }))
}

/// `k2 compute roster key`: mint (first use) and show the roster key's
/// fingerprint, for clients to compare (`k2 compute owners` on their side,
/// and `k2 signer trust-owner`). A person only: it mints a key.
fn roster_key_route(who: &Who) -> R<Value> {
    person(who)?;
    let k = roster_key()?;
    Ok(json!({"ownerId": owner_id()?, "key": key_json(&k)}))
}

fn owned_node(name: &str) -> R<store::NodeRow> {
    let node = crate::compute_routes::node_or_404(Some(name))?;
    if node.is_tenant() || node.state != "active" {
        return Err(no(codes::NODE_NOT_FOUND, 404, format!("{} isn't an active node this server owns", node.name)));
    }
    Ok(node)
}

fn name_list(v: &Value, k: &str) -> R<Vec<String>> {
    let Some(x) = v.get(k) else { return Ok(Vec::new()) };
    let list: Vec<String> = match x {
        Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Value::String(s) => s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect(),
        _ => return Err(no(tcodes::BAD_ARGS, 400, format!("{k} is a list of names"))),
    };
    for n in &list {
        trust::name_ok(k, n).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
    }
    let mut l = list;
    l.sort();
    l.dedup();
    Ok(l)
}

/// `k2 compute roster set|attest|retire|revoke`: a held change (anyone who
/// operates compute nodes files it; a person approves it).
fn roster_propose(who: &Who, body: &[u8]) -> R<Value> {
    let (by, ws) = operator(who)?;
    let v = json_body(body)?;
    let op = s(&v, "op").unwrap_or("");
    let node = s(&v, "node").ok_or_else(|| no(tcodes::BAD_ARGS, 400, "name a node"))?;
    let (subject, body_v, summary) = match op {
        "set" => {
            let n = owned_node(node)?;
            let (st_signed, st) = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                trust::statement_of(&conn, &n.id)
            }
            .ok_or_else(|| no(tcodes::NO_STATEMENT, 409, format!("{} hasn't sent its signed self-statement: update its k2-node (protocol 9) first", n.name)))?;
            if st.node_fp != n.fingerprint {
                return Err(no(tcodes::NO_STATEMENT, 409, format!("{}'s statement is for another key: wait for it to reconnect", n.name)));
            }
            let attest = s(&v, "attest").map(str::trim).filter(|x| !x.is_empty());
            trust::taint_basis_for(&st, attest.is_some(), false).map_err(|(c, h)| no(c, 409, format!("{}: {h}", n.name)))?;
            let host = s(&v, "host").unwrap_or(&n.name).to_ascii_lowercase();
            trust::name_ok("host", &host).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
            for k in ["provider", "site"] {
                if let Some(x) = s(&v, k) {
                    trust::name_ok(k, x).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
                }
            }
            let reach = name_list(&v, "adminReach")?;
            if let Some(a) = attest {
                if a.len() > 500 || a.chars().any(char::is_control) {
                    return Err(no(tcodes::BAD_ARGS, 400, "--attest is what you checked, in plain words (≤ 500 characters)"));
                }
            }
            let summary = format!(
                "list {} (key {}…, epoch {}) as active in the node roster: host {}{}{}{}",
                n.name,
                &n.fingerprint[..12],
                st.key_epoch,
                host,
                s(&v, "provider").map(|p| format!(", provider {p}")).unwrap_or_default(),
                if reach.is_empty() { String::new() } else { format!(", root held by {}", reach.join(", ")) },
                attest.map(|a| format!("; the owner attests (V12): \"{a}\"")).unwrap_or_default()
            );
            (
                n.name.clone(),
                json!({"op": "set", "node": n.name, "nodeId": n.id, "nodeFp": n.fingerprint, "statementDigest": st_signed.digest(),
                    "host": host, "provider": s(&v, "provider"), "site": s(&v, "site"), "adminReach": reach, "attest": attest}),
                summary,
            )
        }
        "retire" | "revoke" => {
            let e = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let by_fp = if ci::is_fp(node) { trust::entry_by_fp(&conn, node) } else { None };
                by_fp.or_else(|| trust::active_entry_by_name(&conn, node))
            }
            .ok_or_else(|| no(tcodes::NODE_NOT_ON_ROSTER, 404, format!("no active roster key for {node} (k2 compute roster show)")))?;
            if op == "retire" {
                let summary = format!("retire {}'s key {}… in the node roster (receipts it signs after now are void)", e.entry.name, &e.entry.node_fp[..12]);
                (e.entry.name.clone(), json!({"op": "retire", "nodeFp": e.entry.node_fp}), summary)
            } else {
                let reason = s(&v, "reason").unwrap_or("compromised");
                if !matches!(reason, "compromised" | "key_lost" | "other") {
                    return Err(no(tcodes::BAD_ARGS, 400, "--reason is compromised, key_lost or other"));
                }
                let summary = format!(
                    "revoke {}'s key {}… ({reason}){}",
                    e.entry.name,
                    &e.entry.node_fp[..12],
                    if reason == "compromised" { ": every receipt it ever signed becomes void, past and future" } else { "" }
                );
                (e.entry.name.clone(), json!({"op": "revoke", "nodeFp": e.entry.node_fp, "reason": reason}), summary)
            }
        }
        _ => return Err(no(tcodes::BAD_ARGS, 400, "op is set, retire or revoke")),
    };
    file_request("roster", &subject, &body_v, &summary, s(&v, "reason"), &by, ws.as_deref(), trust::REQUEST_TTL_SECS)
}

#[allow(clippy::too_many_arguments)]
fn file_request(kind: &str, subject: &str, body: &Value, summary: &str, reason: Option<&str>, by: &str, ws: Option<&str>, ttl: i64) -> R<Value> {
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let r = trust::insert_request(
            &conn,
            &trust::NewTrustRequest { kind, subject, body, summary, reason, proposed_by: by, proposer_workspace: ws, now: now(), ttl },
        )
        .map_err(|(c, h)| no(c, if c == tcodes::TOO_MANY_PENDING { 429 } else { 400 }, h))?;
        store::record_event(&conn, by, "trust_proposed", None, ws, None, json!({"request": r.id, "kind": kind, "summary": r.summary, "digest": r.digest}));
        r
    };
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "trust_request", "id": r.id, "kind": kind}));
    Ok(json!({
        "held": true,
        "request": request_json(&r),
        "hint": format!("held for a person (the owner or an Admin): k2 compute trust approve {} from a terminal outside K2", r.id),
    }))
}

fn request_json(r: &TrustRequest) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or_default();
    // A seat request's enrol code is a credential: never shown.
    if let Some(b) = v.get_mut("body").and_then(Value::as_object_mut) {
        if b.contains_key("code") {
            b.insert("code".into(), "[redacted]".into());
        }
    }
    v
}

/// `k2 compute roster publish`: deliver the newest roster to the owned
/// nodes and seated clients. With `resign` (or when it is older than two
/// days) an **unchanged** roster is signed again first (seq + 1); a changed
/// one waits for its person.
fn roster_publish(who: &Who, body: &[u8]) -> R<Value> {
    let (by, _) = operator(who)?;
    let v = json_body(body)?;
    let resign = v.get("resign").and_then(Value::as_bool).unwrap_or(false);
    let (changed, stale, have) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (trust::entries_changed(&conn), trust::roster_stale(&conn, now()), trust::latest_own_roster(&conn).is_some())
    };
    if !have {
        return Err(no(tcodes::NODE_NOT_ON_ROSTER, 409, "no node roster yet: list a node first (k2 compute roster set <node> …), then approve it"));
    }
    if changed && resign {
        return Err(no("roster_change_pending", 409, "the working entries differ from the newest roster: a person approves that change first (k2 compute trust requests)"));
    }
    let r = if !changed && (resign || stale) {
        refresh_roster(&by)?
    } else {
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::latest_own_roster(&conn).ok_or_else(|| no(tcodes::NODE_NOT_ON_ROSTER, 409, "no node roster"))?
    };
    let out = deliver_roster(&r);
    Ok(json!({"roster": roster_summary(&r), "delivered": out}))
}

/// Sign the next roster from the working entries and store it.
fn issue_roster(by: &str) -> R<trust::StoredRoster> {
    let key = roster_key()?;
    let ctl = crate::compute_admin::trust_controller_key()?;
    let owner = owner_id()?;
    let t = now();
    let next = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::next_roster(&conn, &owner, &ctl.fingerprint(), &key, t).map_err(|e| no(tcodes::BAD_ARGS, 409, e))?
    };
    let signed = sign_owner(ci::DOMAIN_NODE_ROSTER, &next)?;
    let opened = NodeRoster::open(&signed, &key).map_err(|e| no("sign", 500, e))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    trust::store_roster(&conn, &signed, &opened, "self", t).map_err(db_err)?;
    for e in opened.entries.iter().filter(|e| e.status == KeyStatus::Revoked) {
        trust::record_revocation(&conn, &e.node_fp, &key.fingerprint(), e.revoked_reason.unwrap_or(RevokeReason::Other), opened.seq, t);
    }
    store::record_event(&conn, by, "node_roster_issued", None, None, None, json!({"seq": opened.seq, "digest": signed.digest(), "entries": opened.entries.len()}));
    Ok(trust::StoredRoster { signed, roster: opened, source: "self".into(), received_at: t })
}

/// An unchanged roster signed again (V13 freshness): no person, because
/// nothing a person approved changes (only `seq`, `issued_at`,
/// `prev_digest`). Refuses when the entries changed.
fn refresh_roster(by: &str) -> R<trust::StoredRoster> {
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if trust::entries_changed(&conn) {
            return Err(no("roster_change_pending", 409, "a roster change waits for a person"));
        }
    }
    issue_roster(by)
}

/// Hand the roster to every owned node that relays it (`node-roster`) and
/// to every seated client. Best effort: each failure is reported, and the
/// node relay or the next publish covers it.
/// The roster `seq` each owned node was last handed (this process). A node
/// that was offline at publish time gets the newest at the next tick.
static HANDED: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// Hand the roster to the owned nodes that relay it. `only_new`: skip a
/// node already handed this `seq` (the tick's catch-up).
fn deliver_to_nodes(r: &trust::StoredRoster, only_new: bool) -> Vec<Value> {
    let (nodes, seats) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::list_nodes(&conn, false), trust::seats(&conn, Some("owner")))
    };
    let mut to_nodes = Vec::new();
    for n in nodes.iter().filter(|n| !n.is_tenant() && n.state == "active") {
        let Some(e) = r.roster.entry(&n.fingerprint).filter(|e| e.status == KeyStatus::Active) else { continue };
        if only_new
            && (!crate::compute_admin::online(&n.id)
                || HANDED.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|m| m.get(&n.id)) == Some(&r.roster.seq))
        {
            continue;
        }
        if !crate::compute_admin::node_features(&n.id).iter().any(|f| f == ci::FEATURE_NODE_ROSTER) {
            to_nodes.push(json!({"node": n.name, "sent": false, "why": "its k2-node doesn't relay node rosters yet"}));
            continue;
        }
        let doc = NodeRosterDoc {
            roster: r.signed.clone(),
            seats: seats
                .iter()
                .filter(|s| matches!(s.state.as_str(), "active" | "offered" | "suspended") && s.nodes.contains(&e.name))
                .filter_map(|s| s.statement.clone())
                .collect(),
        };
        let args = serde_json::to_value(&doc).unwrap_or_default();
        match crate::compute_admin::send_operate_system("controller:node-roster", n, plane::VERB_NODE_ROSTER_SET, &args) {
            Ok(id) => {
                HANDED.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).insert(n.id.clone(), r.roster.seq);
                to_nodes.push(json!({"node": n.name, "sent": true, "cmdId": id}))
            }
            Err(x) => to_nodes.push(json!({"node": n.name, "sent": false, "why": x.hint})),
        }
    }
    to_nodes
}

fn deliver_roster(r: &trust::StoredRoster) -> Value {
    let to_nodes = deliver_to_nodes(r, false);
    let seats = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::seats(&conn, Some("owner"))
    };
    let mut to_clients = Vec::new();
    let mut done: Vec<String> = Vec::new();
    for s in seats.iter().filter(|s| matches!(s.state.as_str(), "active" | "offered" | "suspended")) {
        if done.contains(&s.peer_fp) {
            continue;
        }
        done.push(s.peer_fp.clone());
        let mine: Vec<Signed> = seats.iter().filter(|x| x.peer_fp == s.peer_fp).filter_map(|x| x.statement.clone()).collect();
        let payload = json!({"roster": r.signed, "seats": mine});
        match fed_send(&s.peer_fp, KIND_NODE_ROSTER, payload) {
            Ok(_) => to_clients.push(json!({"client": s.peer_host, "sent": true})),
            Err(x) => to_clients.push(json!({"client": s.peer_host, "sent": false, "why": x.hint})),
        }
    }
    json!({"nodes": to_nodes, "clients": to_clients})
}

/// The daemon's own loop (60 s, headless): expire requests; re-sign an
/// unchanged roster before it goes stale, and deliver it.
pub fn tick() {
    let t = now();
    let (stale, changed) = {
        let Some(db) = k2_core::db::try_shared() else { return };
        let conn = db.lock();
        for r in trust::expire_due(&conn, t) {
            store::record_event(&conn, "controller", "trust_expired", None, r.proposer_workspace.as_deref(), None, json!({"request": r.id}));
        }
        (trust::roster_stale(&conn, t), trust::entries_changed(&conn))
    };
    if stale && !changed {
        match refresh_roster("controller:refresh") {
            Ok(r) => {
                deliver_roster(&r);
            }
            Err(e) => k2_core::log_debug!("[compute] node roster refresh: {}", e.hint),
        }
        return;
    }
    // Catch up owned nodes that were offline when the newest was handed out.
    let latest = {
        let Some(db) = k2_core::db::try_shared() else { return };
        let conn = db.lock();
        trust::latest_own_roster(&conn)
    };
    if let Some(r) = latest {
        deliver_to_nodes(&r, true);
    }
}

// ── decide ───────────────────────────────────────────────────────────

fn requests(who: &Who, params: &HashMap<String, String>) -> R<Value> {
    need_credential(who)?;
    let own_ws = match who.passport() {
        Some((ws, _)) => Some(ws.to_string()),
        None if who.is_admin() => None,
        None => return Err(crate::compute_routes::owner_only()),
    };
    let state = params.get("state").map(String::as_str).filter(|x| !x.is_empty()).unwrap_or("open");
    let db = k2_core::db::shared();
    let conn = db.lock();
    for r in trust::expire_due(&conn, now()) {
        store::record_event(&conn, "controller", "trust_expired", None, r.proposer_workspace.as_deref(), None, json!({"request": r.id}));
    }
    let list = trust::list_requests(&conn, if state == "all" { None } else { Some(state) }, own_ws.as_deref(), 100);
    Ok(json!({"requests": list.iter().map(request_json).collect::<Vec<_>>(), "canApprove": who.passport().is_none() && who.is_admin()}))
}

fn request_or_404(v: &Value) -> R<TrustRequest> {
    let id = s(v, "request").ok_or_else(|| no(tcodes::BAD_ARGS, 400, "name a request (ctr_…)"))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    trust::request_by_id(&conn, id).ok_or_else(|| no(tcodes::REQUEST_NOT_FOUND, 404, format!("no request '{id}' (k2 compute trust requests)")))
}

fn decide_err(code: &'static str) -> crate::compute_routes::Refusal {
    match code {
        tcodes::APPROVAL_EXPIRED => no(code, 410, "this request expired; file it again"),
        tcodes::REQUEST_NOT_FOUND => no(code, 404, "no such request"),
        _ => no(tcodes::NOT_PENDING, 409, "this request was already decided"),
    }
}

/// `k2 compute trust approve <id>`: a person, in person (confirm bound to
/// the request). Each kind re-checks what it changes at approval time.
fn approve(who: &Who, body: &[u8]) -> R<Value> {
    person(who)?;
    let v = json_body(body)?;
    let req = request_or_404(&v)?;
    if req.state != "pending" {
        return Err(decide_err(tcodes::NOT_PENDING));
    }
    if req.kind == "seat_offer" {
        owner_person(who)?;
    }
    let via = take_confirm(who, &v, trust::CONFIRM_TRUST, &req.id)?;
    let by = who.label();
    let result = match req.kind.as_str() {
        "roster" => approve_roster(&req, &by, &via)?,
        "lane" => approve_lane(&req, &by, &via)?,
        "seat_offer" => approve_seat_offer(&req, &by, &via)?,
        "seat_request" => approve_seat_request(&req, &by, &via)?,
        k => return Err(no(tcodes::BAD_ARGS, 400, format!("no request kind {k}"))),
    };
    let after = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::set_result(&conn, &req.id, false, &result).map_err(db_err)?;
        store::record_event(&conn, &by, "trust_approved", None, req.proposer_workspace.as_deref(), None, json!({"request": req.id, "kind": req.kind, "via": via, "digest": req.digest}));
        trust::request_by_id(&conn, &req.id)
    };
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({"compute": "trust_request", "id": req.id}));
    Ok(json!({"ok": true, "request": after.as_ref().map(request_json), "result": result}))
}

fn decide_now(req: &TrustRequest, by: &str, via: &str) -> R<()> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    trust::decide(&conn, &req.id, "approved", by, Some(via), None, now()).map_err(decide_err)?;
    Ok(())
}

fn deny(who: &Who, body: &[u8]) -> R<Value> {
    person(who)?;
    let v = json_body(body)?;
    let req = request_or_404(&v)?;
    let why = s(&v, "why").map(|w| w.chars().take(500).collect::<String>());
    let db = k2_core::db::shared();
    let conn = db.lock();
    let r = trust::decide(&conn, &req.id, "denied", &who.label(), None, why.as_deref(), now()).map_err(decide_err)?;
    store::record_event(&conn, &who.label(), "trust_denied", None, r.proposer_workspace.as_deref(), None, json!({"request": r.id, "why": why}));
    Ok(json!({"ok": true, "request": request_json(&r)}))
}

fn withdraw(who: &Who, body: &[u8]) -> R<Value> {
    need_credential(who)?;
    let v = json_body(body)?;
    let req = request_or_404(&v)?;
    let mine = match who.passport() {
        Some((ws, _)) => req.proposer_workspace.as_deref() == Some(ws),
        None => who.is_admin() && req.proposer_workspace.is_none() && req.proposed_by == who.label(),
    };
    if !mine {
        return Err(no("forbidden", 403, "only the proposer withdraws a request (a person denies it instead)"));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let r = trust::decide(&conn, &req.id, "withdrawn", &who.label(), None, None, now()).map_err(decide_err)?;
    Ok(json!({"ok": true, "request": request_json(&r)}))
}

// ── roster changes ───────────────────────────────────────────────────

fn approve_roster(req: &TrustRequest, by: &str, via: &str) -> R<Value> {
    let b = &req.body;
    let t = now();
    let op = s(b, "op").unwrap_or("");
    let key = roster_key()?;
    let owner = owner_id()?;
    // Work out the new entries first; nothing is written until it checks.
    let rows: Vec<EntryRow> = match op {
        "set" => {
            let node_id = s(b, "nodeId").unwrap_or("");
            let (node, st) = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                (store::node_by_id(&conn, node_id), trust::statement_of(&conn, node_id))
            };
            let node = node.filter(|n| n.state == "active" && !n.is_tenant()).ok_or_else(|| no(codes::NODE_NOT_FOUND, 404, "the node was removed"))?;
            let (st_signed, st) = st.ok_or_else(|| no(tcodes::NO_STATEMENT, 409, "the node's statement is gone"))?;
            if node.fingerprint != s(b, "nodeFp").unwrap_or("") || st.node_fp != node.fingerprint {
                return Err(no(tcodes::NO_STATEMENT, 409, format!("{} changed key since this was filed: file it again", node.name)));
            }
            let attest = s(b, "attest");
            let basis = trust::taint_basis_for(&st, attest.is_some(), false).map_err(|(c, h)| no(c, 409, h))?;
            let attestation = match (basis, attest) {
                (TaintBasis::OwnerAttested, Some(checked)) => {
                    let a = trust::attestation_for(&owner, &key, &st, &st_signed.digest(), checked, by, t);
                    a.check().map_err(|e| no(tcodes::NODE_KEY_TAINTED, 409, e))?;
                    let signed = sign_owner(ci::DOMAIN_NODE_ATTEST, &a)?;
                    OwnerAttestation::open(&signed, &key).map_err(|e| no("sign", 500, e))?;
                    Some(signed)
                }
                _ => None,
            };
            let prev = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                trust::entry_by_fp(&conn, &node.fingerprint)
            };
            let name = s(b, "node").unwrap_or(&node.name).to_ascii_lowercase();
            trust::name_ok("node", &name).map_err(|e| no(tcodes::BAD_ARGS, 400, format!("{e}: rename the node first (k2 compute node rename)")))?;
            let reach: Vec<String> = b.get("adminReach").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            let entry = k2_core::compute::proto::node_roster::RosterEntry {
                node_fp: node.fingerprint.clone(),
                node_spki: st.node_spki.clone(),
                name,
                key_epoch: st.key_epoch,
                statement_digest: st_signed.digest(),
                status: KeyStatus::Active,
                revoked_reason: None,
                valid_from: prev.as_ref().map(|p| p.entry.valid_from).unwrap_or(t),
                valid_to: None,
                taint_basis: Some(basis),
                attestation_digest: attestation.as_ref().map(Signed::digest),
                host: s(b, "host").unwrap_or(&node.name).to_string(),
                provider: s(b, "provider").map(str::to_string),
                site: s(b, "site").map(str::to_string),
                admin_reach: reach,
                pod: None,
                point: None,
            };
            entry.check().map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
            vec![EntryRow { entry, node_id: Some(node.id.clone()), attestation, updated_at: t }]
        }
        "retire" | "revoke" => {
            let fp = s(b, "nodeFp").unwrap_or("");
            let mut e = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                trust::entry_by_fp(&conn, fp)
            }
            .ok_or_else(|| no(tcodes::NODE_NOT_ON_ROSTER, 404, "that key is no longer in the working roster"))?;
            if op == "retire" {
                e.entry.status = KeyStatus::Retired;
                e.entry.valid_to = Some(t.max(e.entry.valid_from));
            } else {
                e.entry.status = KeyStatus::Revoked;
                e.entry.revoked_reason = serde_json::from_value(json!(s(b, "reason").unwrap_or("compromised"))).ok();
                e.entry.valid_to = Some(t.max(e.entry.valid_from));
            }
            e.updated_at = t;
            vec![e]
        }
        "rotate" => {
            let old: EntryRow = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                trust::entry_by_fp(&conn, s(b, "oldFp").unwrap_or(""))
            }
            .ok_or_else(|| no(tcodes::NODE_NOT_ON_ROSTER, 404, "the old key isn't in the working roster"))?;
            let (st_signed, st) = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                trust::statement_of(&conn, s(b, "nodeId").unwrap_or(""))
            }
            .ok_or_else(|| no(tcodes::NO_STATEMENT, 409, "the node's new statement is gone"))?;
            if st.node_fp != s(b, "newFp").unwrap_or("") {
                return Err(no(tcodes::NO_STATEMENT, 409, "the node changed key again: file it again"));
            }
            let (retired, new) = trust::rotated_entries(&old, &st, &st_signed.digest(), t).map_err(|(c, h)| no(c, 409, h))?;
            vec![retired, new]
        }
        _ => return Err(no(tcodes::BAD_ARGS, 400, "unknown roster change")),
    };
    decide_now(req, by, via)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for row in &rows {
            if let Err(e) = trust::put_entry(&conn, row, t) {
                let _ = trust::set_result(&conn, &req.id, true, &json!({"error": e}));
                return Err(no(tcodes::BAD_ARGS, 409, e));
            }
        }
    }
    let r = issue_roster(by)?;
    let delivered = deliver_roster(&r);
    Ok(json!({"roster": roster_summary(&r), "delivered": delivered}))
}

// ── lanes ────────────────────────────────────────────────────────────

fn lanes_show(who: &Who) -> R<Value> {
    operator(who)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let list: Vec<Value> = trust::lanes(&conn)
        .into_iter()
        .map(|(n, (t, by, at))| json!({"name": n, "template": t, "summary": t.summary(), "approvedBy": by, "approvedAt": at}))
        .collect();
    Ok(json!({"lanes": list, "shipped": {plane::LANE_CI_STANDARD: LaneTemplate::ci_standard()}}))
}

/// `k2 compute lanes set <name> [--from ci-standard] [--key value …]`: a
/// held template (an agent may file it; a person approves).
fn lanes_set(who: &Who, body: &[u8]) -> R<Value> {
    let (by, ws) = operator(who)?;
    let v = json_body(body)?;
    let name = s(&v, "name").ok_or_else(|| no(tcodes::BAD_ARGS, 400, "name the lane"))?;
    trust::name_ok("lane", name).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
    let base = match s(&v, "from") {
        Some(f) => LaneTemplate::named(f).ok_or_else(|| no(tcodes::BAD_ARGS, 400, format!("no shipped lane {f:?} (ci-standard)")))?,
        None => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            trust::lane(&conn, name).unwrap_or_else(LaneTemplate::ci_standard)
        }
    };
    let mut t = serde_json::to_value(&base).map_err(db_err)?;
    if let Some(set) = v.get("set").and_then(Value::as_object) {
        for (k, x) in set {
            if t.get(k).is_none() && k != "other_tenants_max_job_secs" {
                return Err(no(tcodes::BAD_ARGS, 400, format!("no lane key {k:?}")));
            }
            t[k] = x.clone();
        }
    }
    let t: LaneTemplate = serde_json::from_value(t).map_err(|e| no(tcodes::BAD_ARGS, 400, e.to_string()))?;
    t.check().map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
    let summary = format!("set lane {name}: {}", t.summary());
    file_request("lane", name, &json!({"name": name, "template": t}), &summary, s(&v, "reason"), &by, ws.as_deref(), trust::REQUEST_TTL_SECS)
}

fn approve_lane(req: &TrustRequest, by: &str, via: &str) -> R<Value> {
    let name = s(&req.body, "name").unwrap_or("").to_string();
    let t: LaneTemplate = serde_json::from_value(req.body.get("template").cloned().unwrap_or_default()).map_err(|e| no(tcodes::BAD_ARGS, 400, e.to_string()))?;
    t.check().map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
    decide_now(req, by, via)?;
    let seat_nodes: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::set_lane(&conn, &name, &t, by, via, Some(&req.id), now()).map_err(db_err)?;
        let mut n: Vec<String> = trust::seats(&conn, Some("owner"))
            .into_iter()
            .filter(|s| s.lane == name && matches!(s.state.as_str(), "active" | "offered"))
            .flat_map(|s| s.nodes)
            .collect();
        n.sort();
        n.dedup();
        n
    };
    // Live seats under this lane: their nodes' files follow, in one batch.
    let batch = format!("lane-{}", crypto::random_id().map_err(db_err)?);
    let mut plans = Vec::new();
    let vault = crate::compute_admin::trust_vault()?;
    for name in seat_nodes {
        let node = match node_for_entry(&name) {
            Ok(n) => n,
            Err(e) => {
                plans.push(json!({"node": name, "queued": false, "why": e.hint}));
                continue;
            }
        };
        let file = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            trust::lanes_file_for(&conn, &name).map_err(|e| no(tcodes::BAD_ARGS, 409, e))?
        };
        let steps = vec![(plane::VERB_LANES_WRITE.to_string(), json!({"file": file}))];
        match crate::compute_admin::queue_approved_plan(&vault, &node, &steps, "controller:lanes", by, via, &req.id, &batch) {
            Ok(id) => plans.push(json!({"node": node.name, "queued": true, "request": id})),
            Err(e) => plans.push(json!({"node": node.name, "queued": false, "why": e.hint})),
        }
    }
    Ok(json!({"lane": name, "template": t, "batch": batch, "plans": plans}))
}

/// The owned node whose active roster entry has this name.
fn node_for_entry(name: &str) -> R<store::NodeRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let e = trust::active_entry_by_name(&conn, name).ok_or_else(|| no(tcodes::NODE_NOT_ON_ROSTER, 409, format!("{name} has no active key in this server's node roster")))?;
    store::live_node_by_fp(&conn, &e.entry.node_fp)
        .filter(|n| n.state == "active" && !n.is_tenant())
        .ok_or_else(|| no(codes::NODE_NOT_FOUND, 404, format!("{name}'s roster key isn't an active node here")))
}

// ── seats ────────────────────────────────────────────────────────────

fn seat_json(s: &SeatRow) -> Value {
    let mut v = serde_json::to_value(s).unwrap_or_default();
    v["signed"] = s.statement.is_some().into();
    v
}

fn seats_show(who: &Who) -> R<Value> {
    operator(who)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let all = trust::seats(&conn, None);
    Ok(json!({
        "offered": all.iter().filter(|s| s.side == "owner").map(seat_json).collect::<Vec<_>>(),
        "held": all.iter().filter(|s| s.side == "client").map(seat_json).collect::<Vec<_>>(),
    }))
}

fn owners(who: &Who) -> R<Value> {
    operator(who)?;
    let mine = {
        let v = crate::compute_admin::trust_vault().ok();
        v.and_then(|v| v.engine_public_key_if_exists(ci::ROSTER_KEY_ENGINE, ci::ROSTER_KEY_NAME).ok().flatten()).map(ApprovalKey)
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let trusted: Vec<Value> = trust::owner_trusts(&conn)
        .into_iter()
        .map(|t| {
            let latest = trust::latest_roster(&conn, &t.key_fp).map(|r| json!({"seq": r.roster.seq, "issuedAt": r.roster.issued_at, "entries": r.roster.entries.len()}));
            let mut v = serde_json::to_value(&t).unwrap_or_default();
            v["latest"] = latest.into();
            v
        })
        .collect();
    Ok(json!({
        "self": mine.map(|k| json!({"ownerId": owner_id().ok(), "key": key_json(&k)})),
        "trusted": trusted,
    }))
}

/// `k2 compute owners consent <owner> on|off`: off narrows (at once, any
/// operator); on is a person's, with a confirm.
fn owners_consent(who: &Who, body: &[u8]) -> R<Value> {
    let v = json_body(body)?;
    let owner = s(&v, "owner").ok_or_else(|| no(tcodes::BAD_ARGS, 400, "name the node owner"))?;
    let on = v.get("on").and_then(Value::as_bool).ok_or_else(|| no(tcodes::BAD_ARGS, 400, "on is true or false"))?;
    let t = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::owner_trusts(&conn).into_iter().find(|t| t.owner_id == owner || t.key_fp == owner)
    }
    .ok_or_else(|| no(tcodes::OWNER_NOT_TRUSTED, 404, format!("no pinned node owner {owner} (k2 compute owners)")))?;
    if !on {
        operator(who)?;
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::withdraw_consent(&conn, &t.key_fp).map_err(db_err)?;
        store::record_event(&conn, &who.label(), "secrets_consent_off", None, None, None, json!({"owner": t.owner_id, "key": t.key_fp}));
        return Ok(json!({"ok": true, "owner": t.owner_id, "secretsConsent": false}));
    }
    person(who)?;
    // `preview`: what the person agrees to, before the confirm.
    if v.get("preview").and_then(Value::as_bool) == Some(true) {
        return Ok(json!({"preview": true, "owner": t.owner_id, "keyFp": t.key_fp, "text": k2_core::compute::lane_secrets::consent_text(&t.owner_id),
            "confirm": {"action": trust::CONFIRM_TRUST, "subject": format!("consent:{}", t.key_fp)}}));
    }
    let via = take_confirm(who, &v, trust::CONFIRM_TRUST, &format!("consent:{}", t.key_fp))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    trust::give_consent(&conn, &t.key_fp, &who.label(), now()).map_err(db_err)?;
    store::record_event(&conn, &who.label(), "secrets_consent_on", None, None, None, json!({"owner": t.owner_id, "key": t.key_fp, "via": via}));
    Ok(json!({"ok": true, "owner": t.owner_id, "secretsConsent": true}))
}

/// This server's controller fingerprint (what a seat names).
fn my_fp() -> R<String> {
    Ok(crate::compute_admin::trust_controller_key()?.fingerprint())
}

/// `k2 compute seats request <owner-host> [--lane L]` (client): learn the
/// owner's roster key (shown on the card for the person to compare with
/// `k2 compute roster key` on the owner's screen) and file a held request.
fn seats_request(who: &Who, body: &[u8]) -> R<Value> {
    let (by, ws) = operator(who)?;
    let v = json_body(body)?;
    let owner_host = s(&v, "owner").ok_or_else(|| no(tcodes::BAD_ARGS, 400, "name the node owner's server (its host)"))?;
    let lane = s(&v, "lane").unwrap_or(plane::LANE_CI_STANDARD);
    trust::name_ok("lane", lane).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
    let peer = trusted_peer(owner_host)?;
    let answer = fed_send(&peer.fingerprint, KIND_OWNER_KEY, json!({}))?;
    let roster_key = answer.get("rosterKey").and_then(Value::as_str).unwrap_or("");
    let key = ApprovalKey::parse(roster_key).ok_or_else(|| no("peer_answer", 502, format!("{owner_host} has no node roster key yet (it runs k2 compute roster key first)")))?;
    let owner_id = answer.get("ownerId").and_then(Value::as_str).unwrap_or("").to_string();
    trust::name_ok("owner", &owner_id).map_err(|e| no("peer_answer", 502, e))?;
    let label = owner_id_or_label();
    let seat = s(&v, "seat").map(str::to_string).unwrap_or_else(|| format!("{}-{lane}", label.split('.').next().unwrap_or("client")));
    trust::name_ok("seat", &seat).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
    let consent = v.get("secrets").and_then(Value::as_bool).unwrap_or(true);
    let summary = format!(
        "ask {owner_id} for a CI seat (lane {lane}, seat {seat}). This pins {owner_id}'s node-roster key {}… (compare it with `k2 compute roster key` on that server's own screen){}",
        &key.fingerprint()[..16],
        if consent {
            format!(" and lets this server send vault secrets to {owner_id}'s nodes for lane jobs: that owner's root can read them")
        } else {
            ", without secrets (no vault secret goes to its nodes)".to_string()
        }
    );
    file_request(
        "seat_request",
        &seat,
        &json!({"owner": owner_host, "peerFp": peer.fingerprint, "ownerId": owner_id, "rosterKey": key.encode(), "keyFp": key.fingerprint(),
            "lane": lane, "seat": seat, "secrets": consent, "why": s(&v, "why")}),
        &summary,
        s(&v, "why"),
        &by,
        ws.as_deref(),
        trust::SEAT_REQUEST_TTL_SECS,
    )
}

fn owner_id_or_label() -> String {
    owner_id().unwrap_or_else(|_| "client".into())
}

/// The client's person approved: pin, consent, mint the seat code, send.
fn approve_seat_request(req: &TrustRequest, by: &str, via: &str) -> R<Value> {
    let b = &req.body;
    let key = ApprovalKey::parse(s(b, "rosterKey").unwrap_or("")).ok_or_else(|| no(tcodes::BAD_ARGS, 400, "the request's roster key is unreadable"))?;
    let peer_fp = s(b, "peerFp").unwrap_or("").to_string();
    let owner = s(b, "ownerId").unwrap_or("").to_string();
    let seat_id = s(b, "seat").unwrap_or("").to_string();
    let lane = s(b, "lane").unwrap_or("").to_string();
    let consent = b.get("secrets").and_then(Value::as_bool).unwrap_or(false);
    let ctl = my_fp()?;
    let code = pairing::new_code().map_err(db_err)?;
    let es = pairing::EnrollString::new(&code, &ctl).ok_or_else(|| no("internal", 500, "could not make the seat code"))?;
    let binding = pairing::code_binding(&es.code);
    decide_now(req, by, via)?;
    let t = now();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::pin_owner(&conn, &key, &owner, &peer_fp, by, via, consent, t).map_err(db_err)?;
        trust::put_seat(
            &conn,
            &SeatRow {
                id: seat_id.clone(),
                side: "client".into(),
                peer_fp: peer_fp.clone(),
                peer_host: s(b, "owner").unwrap_or("").to_string(),
                owner_id: owner.clone(),
                owner_key_fp: Some(key.fingerprint()),
                client_fp: ctl.clone(),
                client_label: owner_id_or_label(),
                client_url: my_url(),
                lane: lane.clone(),
                nodes: Vec::new(),
                state: "requested".into(),
                statement: None,
                roster_seq: None,
                code_binding: Some(binding),
                request_id: Some(req.id.clone()),
                batch_id: None,
                note: None,
                created_at: t,
                updated_at: t,
            },
        )
        .map_err(db_err)?;
        store::record_event(&conn, by, "owner_pinned", None, None, None, json!({"owner": owner, "key": key.fingerprint(), "secretsConsent": consent, "via": via}));
    }
    let payload = json!({"seat": seat_id, "controllerFp": ctl, "controllerUrl": my_url(), "label": owner_id_or_label(), "lane": lane,
        "code": es.render(), "why": s(b, "why")});
    let sent = fed_send(&peer_fp, KIND_SEAT_REQUEST, payload);
    let db = k2_core::db::shared();
    let conn = db.lock();
    match sent {
        Ok(a) => Ok(json!({"seat": seat_id, "owner": owner, "sent": true, "ownerRequest": a.get("request").cloned()})),
        Err(e) => {
            if let Some(mut row) = trust::seat(&conn, "client", &seat_id) {
                row.note = Some(format!("the request didn't reach {owner}: {}", e.hint));
                row.updated_at = now();
                let _ = trust::put_seat(&conn, &row);
            }
            Ok(json!({"seat": seat_id, "owner": owner, "sent": false, "why": e.hint}))
        }
    }
}

/// `k2 compute seats offer <request> --nodes a,b,c` (owner): name the
/// seat's nodes on a client's request before a person approves it.
fn seats_offer(who: &Who, body: &[u8]) -> R<Value> {
    let (by, _) = operator(who)?;
    let v = json_body(body)?;
    let req = request_or_404(&v)?;
    if req.kind != "seat_offer" {
        return Err(no(tcodes::BAD_ARGS, 400, "that request isn't a client's seat request"));
    }
    let nodes = name_list(&v, "nodes")?;
    if nodes.len() < 2 || nodes.len() > k2_core::compute::proto::node_roster::MAX_SEAT_NODES {
        return Err(no(tcodes::BAD_ARGS, 400, "a seat has 2–16 nodes (a dual build needs two)"));
    }
    for n in &nodes {
        seat_node_ok(n, s(&req.body, "clientFp").unwrap_or(""))?;
    }
    let mut body_v = req.body.clone();
    body_v["nodes"] = json!(nodes);
    let summary = seat_offer_summary(&body_v);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let r = trust::rewrite_pending(&conn, &req.id, &body_v, &summary).map_err(decide_err)?;
    store::record_event(&conn, &by, "seat_nodes_named", None, None, None, json!({"request": r.id, "nodes": nodes}));
    Ok(json!({"held": true, "request": request_json(&r)}))
}

fn seat_offer_summary(b: &Value) -> String {
    let nodes: Vec<&str> = b.get("nodes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    format!(
        "give {} ({}…) a CI seat, lane {}, on {}: each node gets the lane file and the client as a `trust team` server (one approval, every node)",
        s(b, "clientLabel").unwrap_or("?"),
        &s(b, "clientFp").unwrap_or("????????????????")[..16.min(s(b, "clientFp").unwrap_or("").len())],
        s(b, "lane").unwrap_or("?"),
        if nodes.is_empty() { "nodes not named yet (k2 compute seats offer <request> --nodes a,b)".to_string() } else { nodes.join(", ") }
    )
}

/// A node may join a seat: an active roster entry (with a taint basis) of
/// an owned, walled Linux node offering `lanes`, under the client cap.
fn seat_node_ok(name: &str, client_fp: &str) -> R<store::NodeRow> {
    let node = node_for_entry(name)?;
    let (entry, clients) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (trust::active_entry_by_name(&conn, name), trust::owner_seats_on(&conn, name))
    };
    if entry.as_ref().is_none_or(|e| e.entry.taint_basis.is_none()) {
        return Err(no(tcodes::NODE_KEY_TAINTED, 409, format!("{name}: the roster vouches for nothing on its key (attest it or re-key it first)")));
    }
    if matches!(node.os.as_deref(), Some("macos") | Some("darwin")) {
        return Err(no(tcodes::MAC_SEAT_NOT_OFFERED, 409, format!("{name} is a Mac: Mac nodes share one uid between tenants, so they are never seated for another client (V18)")));
    }
    let f = crate::compute_admin::node_features(&node.id);
    for (feat, why) in [
        (ci::FEATURE_LANES, "its k2-node has no lanes (update it to the 0.46.10 k2-node)"),
        (k2_core::compute::proto::frames::FEATURE_JOB_WALL, "it has no job wall"),
        (pshare::FEATURE_TENANT_WALL, "its tenant wall isn't on"),
    ] {
        if !f.iter().any(|x| x == feat) {
            return Err(no(if feat == ci::FEATURE_LANES { tcodes::NODE_LACKS_LANES } else { pshare::codes::TEAM_NEEDS_TENANT_WALL }, 409, format!("{name}: {why}")));
        }
    }
    // S7: the plan's `servers.add` and `lanes.write` need remote admin
    // `full`; a level the node never reported is not `full` (the node would
    // refuse half a seat).
    let level = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::compute::admin::admin_cols(&conn, &node.id).remote_admin
    };
    if level.as_deref() != Some("full") {
        return Err(no(
            k2_core::compute::admin::codes::REMOTE_ADMIN_OFF,
            409,
            format!(
                "{name}: a seat needs remote admin `full` (it's {}): raise it at the machine (sudo k2-node admin level full)",
                level.as_deref().unwrap_or("not reported yet")
            ),
        ));
    }
    let mut fps: Vec<&str> = clients.iter().map(|s| s.client_fp.as_str()).collect();
    fps.push(client_fp);
    fps.sort();
    fps.dedup();
    // 0.46.10 keeps the node's own cap: the owner's slot plus up to
    // `max_servers - 1` others (pods' 3-client rule is 0.46.11).
    if fps.len() >= pshare::MAX_SLOTS as usize {
        return Err(no(tcodes::NODE_CLIENT_CAP, 409, format!("{name} already serves {} clients", fps.len() - 1)));
    }
    Ok(node)
}

/// The owner's person approved a seat: sign it, send the offer, then seal
/// one plan per node in one batch (V21).
fn approve_seat_offer(req: &TrustRequest, by: &str, via: &str) -> R<Value> {
    let b = &req.body;
    let nodes: Vec<String> = b.get("nodes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if nodes.len() < 2 {
        return Err(no(tcodes::BAD_ARGS, 409, "name the seat's nodes first: k2 compute seats offer <request> --nodes a,b"));
    }
    let client_fp = s(b, "clientFp").unwrap_or("").to_string();
    let lane = s(b, "lane").unwrap_or("").to_string();
    let lane_t = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::lane(&conn, &lane)
    }
    .ok_or_else(|| no(tcodes::LANE_NOT_FOUND, 409, format!("lane {lane} isn't approved here (k2 compute lanes set {lane} --from ci-standard)")))?;
    let node_rows: Vec<store::NodeRow> = nodes.iter().map(|n| seat_node_ok(n, &client_fp)).collect::<R<_>>()?;
    let (binding, prefix) = k2_core::compute::share::split_add_code(s(b, "code").unwrap_or("")).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
    if !client_fp.starts_with(&prefix) {
        return Err(no(tcodes::BAD_ARGS, 400, "the seat code names another server than the client"));
    }
    let key = roster_key()?;
    let roster = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::latest_own_roster(&conn)
    }
    .ok_or_else(|| no(tcodes::NODE_NOT_ON_ROSTER, 409, "no signed node roster yet"))?;
    for n in &nodes {
        if !roster.roster.entries.iter().any(|e| &e.name == n && e.status == KeyStatus::Active) {
            return Err(no(tcodes::NODE_NOT_ON_ROSTER, 409, format!("{n} isn't active in the newest signed roster (seq {}): approve its roster change first", roster.roster.seq)));
        }
    }
    let t = now();
    let mut row = SeatRow {
        id: req.subject.clone(),
        side: "owner".into(),
        peer_fp: s(b, "peerFp").unwrap_or("").to_string(),
        peer_host: s(b, "peerHost").unwrap_or("").to_string(),
        owner_id: owner_id()?,
        owner_key_fp: Some(key.fingerprint()),
        client_fp: client_fp.clone(),
        client_label: s(b, "clientLabel").unwrap_or("").to_string(),
        client_url: s(b, "clientUrl").unwrap_or("").to_string(),
        lane: lane.clone(),
        nodes: nodes.clone(),
        state: "active".into(),
        statement: None,
        roster_seq: Some(roster.roster.seq as i64),
        code_binding: None,
        request_id: Some(req.id.clone()),
        batch_id: None,
        note: None,
        created_at: t,
        updated_at: t,
    };
    if let Some(other) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::seat(&conn, "owner", &row.id)
    } {
        if other.client_fp != client_fp && other.state != "ended" {
            return Err(no(tcodes::BAD_ARGS, 409, format!("seat id {} is taken by another client", row.id)));
        }
    }
    let st = trust::seat_statement_for(&row, &key, roster.roster.seq, SeatState::Active, t);
    let signed = sign_owner(ci::DOMAIN_NODE_SEAT, &st)?;
    SeatStatement::open(&signed, &key).map_err(|e| no("sign", 500, e))?;
    // The client needs the roster and its seat before a node enrols there.
    fed_send(&row.peer_fp, KIND_SEAT_OFFER, json!({"seat": signed, "roster": roster.signed, "lane": lane_t}))
        .map_err(|e| no("client_unreachable", 502, format!("the seat wasn't sent to {}: {} (nothing changed; approve again when it's back)", row.peer_host, e.hint)))?;
    decide_now(req, by, via)?;
    let batch = format!("seat-{}", crypto::random_id().map_err(db_err)?);
    row.statement = Some(signed);
    row.batch_id = Some(batch.clone());
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::put_seat(&conn, &row).map_err(db_err)?;
        // The template this seat was approved under (0275), as sent.
        trust::set_seat_lane(&conn, "owner", &row.id, &lane_t).map_err(db_err)?;
    }
    let vault = crate::compute_admin::trust_vault()?;
    let mut plans = Vec::new();
    for node in &node_rows {
        let file = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            trust::lanes_file_for(&conn, &nodes[node_rows.iter().position(|x| x.id == node.id).unwrap_or(0)]).map_err(|e| no(tcodes::BAD_ARGS, 409, e))?
        };
        let add = pshare::AddArgs {
            url: row.client_url.clone(),
            code_binding: binding.clone(),
            fp_prefix: prefix.clone(),
            trust: pshare::Trust::Team,
            for_secs: Some(0),
            limits: Default::default(),
        };
        let add_v = serde_json::to_value(&add).map_err(db_err)?;
        pshare::check_verb_args(pshare::VERB_ADD, &add_v).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
        let steps = vec![(plane::VERB_LANES_WRITE.to_string(), json!({"file": file})), (pshare::VERB_ADD.to_string(), add_v)];
        match crate::compute_admin::queue_approved_plan(&vault, node, &steps, &req.proposed_by, by, via, &req.id, &batch) {
            Ok(id) => plans.push(json!({"node": node.name, "queued": true, "request": id})),
            Err(e) => plans.push(json!({"node": node.name, "queued": false, "why": e.hint})),
        }
    }
    let delivered = deliver_roster(&roster);
    Ok(json!({"seat": seat_json(&row), "batch": batch, "plans": plans, "delivered": delivered}))
}

// ── the federation leg ───────────────────────────────────────────────

fn trusted_peer(host: &str) -> R<FederationPeer> {
    #[cfg(test)]
    if let Some(p) = test_seams::peer(host) {
        return Ok(p);
    }
    if !federation::enabled() {
        return Err(no("federation_off", 422, "federation is off on this server; turn it on and pair with the node owner first"));
    }
    let store = PeerStore::load().map_err(db_err)?;
    let peer = store
        .list()
        .iter()
        .find(|p| crate::federation_routes::peer_matches_user_host(p, host) || p.fingerprint == host)
        .cloned()
        .ok_or_else(|| no("not_paired", 404, format!("no paired server matches {host}: pair the two servers first (k2 fed pair, one SAS)")))?;
    if peer.trust != PeerTrust::Trusted {
        return Err(no("not_connected", 403, format!("{host} isn't trusted yet")));
    }
    Ok(peer)
}

/// Seal `payload` as `kind` to a pinned, trusted peer and post it to its
/// compute-seat route. The answer's JSON, or the refusal.
fn fed_send(peer_fp: &str, kind: &str, payload: Value) -> R<Value> {
    #[cfg(test)]
    if let Some(r) = test_seams::fed_send(peer_fp, kind, &payload) {
        return r;
    }
    let peer = trusted_peer(peer_fp)?;
    let base = crate::federation_routes::gated_peer_base_url(&peer).map_err(|e| no("not_connected", 403, e))?;
    let me = federation::local_fingerprint().unwrap_or_default();
    let signal = AgentSignal {
        delivery: Delivery::Inbox,
        ..AgentSignal::new(
            AgentAddress::Agent { workspace: WorkspaceId(format!("peer:{me}")), name: "compute".into() },
            AgentAddress::Agent { workspace: WorkspaceId(format!("peer:{}", peer.fingerprint)), name: "compute".into() },
            SignalKind::Custom { kind: kind.to_string(), payload },
        )
    };
    let key = federation::identity::load_or_create().map_err(|e| no("federation_key", 500, e))?;
    let bytes = federation::seal(&signal, &key, &peer.subdomain, 8).map_err(|e| no("seal", 500, e.to_string()))?;
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(db_err)?;
    let resp = client
        .post(format!("{base}{FEDERATION_ROUTE}"))
        .header("Content-Type", "application/json")
        .body(bytes)
        .send()
        .map_err(|e| no("peer_unreachable", 502, format!("{} didn't answer: {e}", peer.label)))?;
    let status = resp.status().as_u16();
    let v: Value = serde_json::from_str(&resp.text().unwrap_or_default()).unwrap_or(Value::Null);
    if status == 200 {
        return Ok(v);
    }
    let hint = v.pointer("/error/hint").and_then(Value::as_str).unwrap_or("").to_string();
    Err(no(if status == 404 { "peer_too_old" } else { "peer_refused" }, 502, format!("{} answered {status}: {hint}", peer.label)))
}

/// `POST /cli/federation/compute-seat`: a pinned, trusted peer's signed
/// envelope (body capped by the dispatcher).
pub fn handle_federation(body: &[u8]) -> CliResponse {
    if body.len() > MAX_FED_BODY {
        return crate::compute_routes::refusal_response(&no("too_large", 413, "body too large"));
    }
    if !compute::enabled() {
        return crate::compute_routes::dark();
    }
    let store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return crate::compute_routes::refusal_response(&db_err(e)),
    };
    let me = federation::local_fingerprint().unwrap_or_default();
    let ingested = match ingress::ingest(body, &store, ingress::global_nonce_cache(), &me, ingress::DEFAULT_SKEW_SECS) {
        Ok(v) => v,
        Err(e) => return crate::federation_routes::ingress_reject(e),
    };
    let host = store.get(&ingested.peer_fingerprint).map(crate::federation_routes::peer_host).unwrap_or_default();
    let SignalKind::Custom { kind, payload } = &ingested.signal.kind else {
        return crate::compute_routes::refusal_response(&no("wrong_kind", 400, "this route takes compute seat kinds only"));
    };
    match receive(&ingested.peer_fingerprint, &host, kind, payload) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => crate::compute_routes::refusal_response(&e),
    }
}

/// What a peer's verified envelope asks (testable without envelopes).
pub(crate) fn receive(peer_fp: &str, peer_host: &str, kind: &str, p: &Value) -> R<Value> {
    match kind {
        KIND_OWNER_KEY => {
            let v = crate::compute_admin::trust_vault()?;
            let k = v
                .engine_public_key_if_exists(ci::ROSTER_KEY_ENGINE, ci::ROSTER_KEY_NAME)
                .map_err(|e| no("vault_closed", e.status, e.hint))?
                .map(ApprovalKey)
                .ok_or_else(|| no("no_roster_key", 404, "this server has no node roster key yet"))?;
            Ok(json!({"ownerId": owner_id()?, "rosterKey": k.encode(), "keyFp": k.fingerprint(), "ownerServerFp": my_fp()?}))
        }
        KIND_SEAT_REQUEST => receive_seat_request(peer_fp, peer_host, p),
        KIND_SEAT_OFFER => receive_seat_offer(peer_fp, p),
        KIND_NODE_ROSTER => {
            let doc: NodeRosterDoc = serde_json::from_value(p.clone()).map_err(|e| no(tcodes::BAD_ARGS, 400, e.to_string()))?;
            let fx = client_take_roster(Some(peer_fp), &doc, "peer")?;
            Ok(json!({"ok": true, "seq": fx.seq}))
        }
        _ => Err(no("wrong_kind", 400, format!("unknown kind {kind}"))),
    }
}

/// Owner: a client's seat request lands as a held seat offer.
fn receive_seat_request(peer_fp: &str, peer_host: &str, p: &Value) -> R<Value> {
    let seat = s(p, "seat").unwrap_or("");
    trust::name_ok("seat", seat).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
    let ctl = s(p, "controllerFp").unwrap_or("");
    if !ci::is_fp(ctl) {
        return Err(no(tcodes::BAD_ARGS, 400, "controllerFp is the client's 64-hex key fingerprint"));
    }
    let url = s(p, "controllerUrl").unwrap_or("");
    if !url.starts_with("https://") || url.len() > 253 {
        return Err(no(tcodes::BAD_ARGS, 400, "controllerUrl is https://<host>"));
    }
    let (_, prefix) = k2_core::compute::share::split_add_code(s(p, "code").unwrap_or("")).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
    if !ctl.starts_with(&prefix) {
        return Err(no(tcodes::BAD_ARGS, 400, "the seat code names another server"));
    }
    let lane = s(p, "lane").unwrap_or(plane::LANE_CI_STANDARD);
    trust::name_ok("lane", lane).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
    let label: String = s(p, "label").unwrap_or(peer_host).chars().filter(|c| !c.is_control()).take(253).collect();
    let body = json!({"seat": seat, "peerFp": peer_fp, "peerHost": peer_host, "clientFp": ctl, "clientLabel": label, "clientUrl": url,
        "lane": lane, "code": s(p, "code"), "nodes": [], "why": s(p, "why").map(|w| w.chars().take(200).collect::<String>())});
    let summary = seat_offer_summary(&body);
    let out = file_request("seat_offer", seat, &body, &summary, s(p, "why"), &format!("peer:{peer_host}"), None, trust::SEAT_REQUEST_TTL_SECS)?;
    Ok(json!({"held": true, "request": out["request"]["id"]}))
}

/// Client: the owner's signed seat and roster arrive (after this server's
/// person asked for the seat), with the lane's template: kept on the seat
/// (0275) for the controller's lane-secret check (C3b, V25). A template
/// that doesn't parse or check is no template (no lane secrets).
fn receive_seat_offer(peer_fp: &str, p: &Value) -> R<Value> {
    let seat: Signed = serde_json::from_value(p.get("seat").cloned().unwrap_or_default()).map_err(|e| no(tcodes::BAD_ARGS, 400, e.to_string()))?;
    let roster: Signed = serde_json::from_value(p.get("roster").cloned().unwrap_or_default()).map_err(|e| no(tcodes::BAD_ARGS, 400, e.to_string()))?;
    let peek: SeatStatement = seat.peek(ci::DOMAIN_NODE_SEAT).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
    let fx = client_take_roster(Some(peer_fp), &NodeRosterDoc { roster, seats: vec![seat] }, "peer")?;
    let lane: Option<LaneTemplate> = p.get("lane").cloned().and_then(|l| serde_json::from_value(l).ok()).filter(|t: &LaneTemplate| t.check().is_ok());
    if let Some(t) = lane {
        let db = k2_core::db::shared();
        let conn = db.lock();
        // Only a seat that took this statement (same owner key and lane).
        if trust::seat(&conn, "client", &peek.seat_id).is_some_and(|row| row.lane == peek.lane && row.statement.is_some() && row.peer_fp == peer_fp) {
            trust::set_seat_lane(&conn, "client", &peek.seat_id, &t).map_err(db_err)?;
        }
    }
    Ok(json!({"ok": true, "seq": fx.seq}))
}

/// Client: take an owner's roster (federation, `peer_fp` set, or a node
/// relay) and this server's seat statements in it. Seats are only ever
/// updated, never created: a seat exists here only when a person asked.
fn client_take_roster(peer_fp: Option<&str>, doc: &NodeRosterDoc, source: &str) -> R<trust::RosterEffects> {
    let me = my_fp()?;
    let t = now();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let peek: NodeRoster = doc.roster.peek(ci::DOMAIN_NODE_ROSTER).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
    let key = ApprovalKey::parse(&peek.roster_key).ok_or_else(|| no(tcodes::BAD_ARGS, 400, "roster key unreadable"))?;
    let trust_row = trust::owner_trust(&conn, &key.fingerprint())
        .ok_or_else(|| no(tcodes::OWNER_NOT_TRUSTED, 403, format!("no person here pinned {}'s roster key", peek.owner_id)))?;
    if let Some(fp) = peer_fp {
        if trust_row.peer_fp != fp {
            return Err(no(tcodes::OWNER_NOT_TRUSTED, 403, "that roster key was pinned for another server"));
        }
    }
    let fx = trust::receive_roster(&conn, &doc.roster, source, t).map_err(|(c, h)| no(c, 409, h))?;
    let pinned = ApprovalKey::parse(&trust_row.roster_key).ok_or_else(|| no("store", 500, "pinned key unreadable"))?;
    for s_signed in &doc.seats {
        let st = SeatStatement::open(s_signed, &pinned).map_err(|e| no(tcodes::BAD_ARGS, 400, e))?;
        if st.client_fp != me {
            continue;
        }
        let Some(mut row) = trust::seat(&conn, "client", &st.seat_id) else { continue };
        if row.owner_key_fp.as_deref() != Some(key.fingerprint().as_str()) || row.lane != st.lane {
            store::record_event(&conn, "controller", "seat_refused", None, None, None, json!({"seat": st.seat_id, "why": "another owner key or lane than asked"}));
            continue;
        }
        if row.roster_seq.is_some_and(|q| (st.roster_seq as i64) < q) {
            continue;
        }
        row.nodes = st.nodes.clone();
        row.state = match st.state {
            SeatState::Offered => "offered",
            SeatState::Active => "active",
            SeatState::Suspended => "suspended",
            SeatState::Ended => "ended",
        }
        .into();
        row.statement = Some(s_signed.clone());
        row.roster_seq = Some(st.roster_seq as i64);
        row.owner_id = st.owner_id.clone();
        row.note = None;
        row.updated_at = t;
        trust::put_seat(&conn, &row).map_err(db_err)?;
        store::record_event(&conn, "controller", "seat_updated", None, None, None, json!({"seat": row.id, "state": row.state, "nodes": row.nodes}));
    }
    for fp in &fx.revoked {
        if let Some(n) = store::live_node_by_fp(&conn, fp) {
            store::record_event(&conn, "controller", "node_key_revoked", Some(&n.id), None, None, json!({"owner": peek.owner_id, "seq": fx.seq}));
        }
    }
    Ok(fx)
}

// ── node hooks (compute_ws) ──────────────────────────────────────────

/// A node's verified self-statement: keep the latest.
pub fn on_statement(node_id: &str, s: &Signed) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = trust::save_statement(&conn, node_id, s, now()) {
        store::record_event(&conn, "node", "statement_refused", Some(node_id), None, None, json!({"why": e}));
    }
}

/// The node relays its owner's roster (V11). A tenant (client) takes it if
/// a person here pinned that owner's key; the owner ignores its own.
pub fn on_node_roster(node_id: &str, doc: NodeRosterDoc) {
    let node = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, node_id)
    };
    let Some(node) = node else { return };
    if !node.is_tenant() {
        return;
    }
    if let Err(e) = client_take_roster(None, &doc, "node") {
        audit("node", "node_roster_refused", Some(node_id), json!({"code": e.code, "why": e.hint}));
    }
}

/// A node attaches with a key this server doesn't pin, carrying a rotation
/// signed by one it does (V3): move the row in place. On a node this server
/// owns, file the roster change for a person (the new key is listed only
/// after approval).
pub fn on_rotation(rot: &KeyRotation) -> Result<store::NodeRow, (&'static str, String)> {
    let r: k2_core::compute::proto::statement::RotationStatement = rot.rotation.peek(ci::DOMAIN_NODE_ROTATION).map_err(|e| (tcodes::ROTATION_REFUSED, e))?;
    let (moved, owned_entry) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let node = store::live_node_by_fp(&conn, &r.old_fp).ok_or((tcodes::ROTATION_REFUSED, "no node pins the rotation's old key".to_string()))?;
        let moved = trust::apply_rotation(&conn, &node, rot, now())?;
        store::record_event(&conn, "node", "node_rekeyed", Some(&moved.id), None, None, json!({"old": r.old_fp, "new": r.new_fp, "epoch": r.new_epoch, "reason": r.reason}));
        let e = (!moved.is_tenant()).then(|| trust::entry_by_fp(&conn, &r.old_fp)).flatten();
        (moved, e)
    };
    if let Some(e) = owned_entry {
        let summary = format!("{} re-keyed ({}… → {}…, epoch {}): retire the old key and list the new one", moved.name, &r.old_fp[..12], &r.new_fp[..12], r.new_epoch);
        let _ = file_request(
            "roster",
            &e.entry.name,
            &json!({"op": "rotate", "nodeId": moved.id, "oldFp": r.old_fp, "newFp": r.new_fp}),
            &summary,
            None,
            "controller",
            None,
            trust::REQUEST_TTL_SECS,
        );
    }
    Ok(moved)
}

/// A node enrolling with a seat code (client, V20): the seat and the node's
/// roster name, when the key is the seat's.
pub fn seat_enrol(binding: &str, node_fp: &str) -> Option<(SeatRow, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    trust::seat_code_match(&conn, binding, node_fp, now())
}

/// A seat node's name on this server: its roster name when that is a valid
/// node name here and free, else `<seat>-<roster name>` cut to fit, else a
/// numbered one.
pub fn seat_node_local_name(roster_name: &str, seat: &str) -> String {
    let clean = |s: &str| -> String {
        let t: String = s.chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' { c } else { '-' }).collect();
        t.trim_matches('-').chars().take(32).collect::<String>().trim_end_matches('-').to_string()
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let free = |n: &str| compute::enroll::valid_name(n) && store::live_node_by_name(&conn, n).is_none();
    let first = clean(roster_name);
    if free(&first) {
        return first;
    }
    let second = clean(&format!("{seat}-{roster_name}"));
    if free(&second) {
        return second;
    }
    for i in 2..100 {
        let n = clean(&format!("{}-{i}", &first.chars().take(28).collect::<String>()));
        if free(&n) {
            return n;
        }
    }
    clean(&format!("seat-{}", crypto::random_id().unwrap_or_default()))
}

/// After a seat node's pending row exists: confirm it (the owner's roster
/// is the trust anchor, V20), tie it to the seat, mark it the owner's.
pub fn seat_enrolled(node_id: &str, seat: &SeatRow) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let by = format!("seat:{}", seat.id);
    if let Err(e) = store::confirm_node(&conn, node_id, &by, now()) {
        store::record_event(&conn, &by, "seat_confirm_failed", Some(node_id), None, None, json!({"why": e}));
        return;
    }
    let _ = trust::set_node_seat(&conn, node_id, Some(&seat.id));
    let _ = store::set_tenant_of(&conn, node_id, &seat.owner_id);
    store::record_event(&conn, &by, "seat_node_confirmed", Some(node_id), None, None, json!({"seat": seat.id, "owner": seat.owner_id}));
}

/// `/cli/compute/jobs/receipt`: why a receipt is void, if its key was
/// revoked by its owner (PRD §4.1 revocation).
pub fn receipt_void(node_fp: &str, started_at: Option<i64>) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    trust::receipt_void(&conn, node_fp, started_at)
}

// ── signer pairing (V17) ─────────────────────────────────────────────

#[cfg(unix)]
mod signer {
    use super::*;
    use k2_signer::pairing as sp;
    use std::sync::OnceLock;

    struct Pairing {
        pending: sp::ServerPending,
        created: i64,
        sas_ready: bool,
        by: String,
    }

    fn book() -> &'static Mutex<HashMap<String, Pairing>> {
        static B: OnceLock<Mutex<HashMap<String, Pairing>>> = OnceLock::new();
        B.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn sweep(m: &mut HashMap<String, Pairing>, t: i64) {
        m.retain(|_, p| t < p.created + sp::CODE_TTL_SECS);
    }

    fn signed_of(v: &Value, k: &str) -> R<Signed> {
        serde_json::from_value(v.get(k).cloned().unwrap_or_default()).map_err(|e| no(tcodes::BAD_ARGS, 400, format!("{k}: {e}")))
    }

    /// `k2 ci signer add <code> --hello <file>`: a person, with a confirm
    /// bound to the signer's fingerprint prefix in the code.
    pub(super) fn add(who: &Who, body: &[u8]) -> R<Value> {
        person(who)?;
        let v = json_body(body)?;
        let code = s(&v, "code").ok_or_else(|| no(tcodes::BAD_ARGS, 400, "the code k2-signer-admin pair-begin printed (ABCDE-FGHJK.<16 hex>)"))?;
        let es = pairing::EnrollString::parse(code).ok_or_else(|| no(tcodes::BAD_ARGS, 400, "not a signer pairing code"))?;
        let via = take_confirm(who, &v, trust::CONFIRM_SIGNER, &format!("add:{}", es.controller_fp_prefix))?;
        let hello: sp::Hello = serde_json::from_value(v.get("hello").cloned().unwrap_or_default()).map_err(|e| no(tcodes::BAD_ARGS, 400, format!("hello: {e}")))?;
        let key = crate::compute_admin::trust_controller_key()?;
        let label: String = owner_id_or_label().chars().take(sp::MAX_LABEL).collect();
        let t = now();
        let (msg, pending) = sp::server_offer(&key, &label, code, &hello, t).map_err(|e| no("signer_pairing", 400, e))?;
        let id = format!("csp_{}", crypto::random_id().map_err(db_err)?);
        {
            let mut m = book().lock().unwrap_or_else(|p| p.into_inner());
            sweep(&mut m, t);
            if m.len() >= 8 {
                return Err(no("too_many_pending", 429, "8 signer pairings already wait; let one expire (10 minutes)"));
            }
            m.insert(id.clone(), Pairing { pending, created: t, sas_ready: false, by: who.label() });
        }
        audit(&who.label(), "signer_pair_offer", None, json!({"pairing": id, "signer": hello.signer_fp, "via": via}));
        Ok(json!({"pairing": id, "offer": msg, "next": "on the signing machine: sudo k2-signer-admin pair-reveal < offer.json; then k2 ci signer reveal <pairing> --reveal reveal.json here"}))
    }

    /// `k2 ci signer reveal <pairing> --reveal <file>`: the SAS, answered only
    /// to a person at this server (owner or Admin, a fresh confirm bound to
    /// the pairing). Whoever carried the hello/offer/reveal can't read it.
    pub(super) fn reveal(who: &Who, body: &[u8]) -> R<Value> {
        person(who)?;
        let v = json_body(body)?;
        let id = s(&v, "pairing").ok_or_else(|| no(tcodes::BAD_ARGS, 400, "name the pairing (csp_…)"))?.to_string();
        let via = take_confirm(who, &v, trust::CONFIRM_SIGNER, &id)?;
        let reveal = signed_of(&v, "reveal")?;
        let mut m = book().lock().unwrap_or_else(|p| p.into_inner());
        sweep(&mut m, now());
        let p = m.get_mut(&id).ok_or_else(|| no(tcodes::REQUEST_NOT_FOUND, 404, "no such pairing (they last 10 minutes): start again"))?;
        let sas = match sp::server_sas(&p.pending, &reveal) {
            Ok(x) => x,
            Err(e) => {
                m.remove(&id);
                return Err(no("signer_pairing", 400, format!("{e}: this pairing is void; start again")));
            }
        };
        p.sas_ready = true;
        drop(m);
        audit(&who.label(), "signer_pair_sas_shown", None, json!({"pairing": id, "via": via}));
        Ok(json!({"pairing": id, "sas": sas, "show": "Type these 6 digits at the signing machine: sudo k2-signer-admin pair-accept <digits>"}))
    }

    /// `k2 ci signer finish <pairing> --done <file>`: record the signer.
    pub(super) fn finish(who: &Who, body: &[u8]) -> R<Value> {
        person(who)?;
        let v = json_body(body)?;
        let id = s(&v, "pairing").ok_or_else(|| no(tcodes::BAD_ARGS, 400, "name the pairing (csp_…)"))?.to_string();
        let done = signed_of(&v, "done")?;
        let p = {
            let mut m = book().lock().unwrap_or_else(|p| p.into_inner());
            sweep(&mut m, now());
            let ready = m.get(&id).is_some_and(|p| p.sas_ready);
            if !ready {
                return Err(no(tcodes::REQUEST_NOT_FOUND, 409, "no revealed pairing by that id (reveal first; pairings last 10 minutes)"));
            }
            m.remove(&id).expect("checked above")
        };
        let fp = sp::server_finish(&p.pending, &done).map_err(|e| no("signer_pairing", 400, e))?;
        let db = k2_core::db::shared();
        let conn = db.lock();
        trust::add_signer(&conn, &fp, &p.pending.hello.signer_spki, &p.pending.hello.signer_label, &p.by, "sas", now()).map_err(db_err)?;
        store::record_event(&conn, &who.label(), "signer_paired", None, None, None, json!({"signer": fp, "label": p.pending.hello.signer_label}));
        Ok(json!({"ok": true, "signer": fp, "label": p.pending.hello.signer_label}))
    }
}

fn signer_add(who: &Who, body: &[u8]) -> R<Value> {
    #[cfg(unix)]
    return signer::add(who, body);
    #[cfg(not(unix))]
    {
        let _ = (who, body);
        Err(no("not_supported", 501, "signer pairing runs on macOS and Linux servers"))
    }
}

fn signer_reveal(who: &Who, body: &[u8]) -> R<Value> {
    #[cfg(unix)]
    return signer::reveal(who, body);
    #[cfg(not(unix))]
    {
        let _ = (who, body);
        Err(no("not_supported", 501, "signer pairing runs on macOS and Linux servers"))
    }
}

fn signer_finish(who: &Who, body: &[u8]) -> R<Value> {
    #[cfg(unix)]
    return signer::finish(who, body);
    #[cfg(not(unix))]
    {
        let _ = (who, body);
        Err(no("not_supported", 501, "signer pairing runs on macOS and Linux servers"))
    }
}

fn signers_show(who: &Who) -> R<Value> {
    need_credential(who)?;
    crate::compute_routes::need_admin(who)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    Ok(json!({"signers": trust::signers(&conn, false), "controllerFp": my_fp().ok()}))
}

fn signer_remove(who: &Who, body: &[u8]) -> R<Value> {
    person(who)?;
    let v = json_body(body)?;
    let fp = s(&v, "signer").ok_or_else(|| no(tcodes::BAD_ARGS, 400, "name the signer's fingerprint"))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let gone = trust::remove_signer(&conn, fp, now()).map_err(db_err)?;
    if !gone {
        return Err(no(tcodes::REQUEST_NOT_FOUND, 404, "no paired signer with that fingerprint"));
    }
    store::record_event(&conn, &who.label(), "signer_removed", None, None, None, json!({"signer": fp}));
    Ok(json!({"ok": true}))
}

#[cfg(test)]
pub(crate) mod test_seams {
    use super::*;

    thread_local! {
        static OWNER: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
        static PEERS: std::cell::RefCell<Vec<FederationPeer>> = const { std::cell::RefCell::new(Vec::new()) };
        #[allow(clippy::type_complexity)]
        static FED: std::cell::RefCell<Option<Box<dyn FnMut(&str, &str, &Value) -> R<Value>>>> = const { std::cell::RefCell::new(None) };
        static SENT: std::cell::RefCell<Vec<(String, String, Value)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    pub fn set_owner_id(id: Option<&str>) {
        OWNER.with(|o| *o.borrow_mut() = id.map(str::to_string));
    }

    pub(super) fn owner_id() -> Option<String> {
        OWNER.with(|o| o.borrow().clone())
    }

    pub fn owner_id_now() -> Option<String> {
        owner_id()
    }

    pub fn add_peer(p: FederationPeer) {
        PEERS.with(|l| l.borrow_mut().push(p));
    }

    pub(super) fn peer(sel: &str) -> Option<FederationPeer> {
        PEERS.with(|l| l.borrow().iter().find(|p| p.fingerprint == sel || p.label == sel || p.subdomain == sel).cloned())
    }

    /// Route federation sends through `f` (the other server, in-process).
    pub fn set_fed(f: Option<Box<dyn FnMut(&str, &str, &Value) -> R<Value>>>) {
        FED.with(|x| *x.borrow_mut() = f);
    }

    pub fn take_sent() -> Vec<(String, String, Value)> {
        SENT.with(|s| std::mem::take(&mut *s.borrow_mut()))
    }

    pub(super) fn fed_send(peer: &str, kind: &str, p: &Value) -> Option<R<Value>> {
        SENT.with(|s| s.borrow_mut().push((peer.to_string(), kind.to_string(), p.clone())));
        FED.with(|x| x.borrow_mut().as_mut().map(|f| f(peer, kind, p)))
    }
}

#[cfg(test)]
mod tests;
