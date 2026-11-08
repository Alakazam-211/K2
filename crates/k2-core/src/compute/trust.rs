//! Node rosters, lanes, seats and signer pairing: the SQLite and pure parts
//! (0.46.10, prd-ci-on-compute-for-clients-v1 C1 + C3a; vs-live V3,
//! V11–V13, V17, V19–V22, V25; migration 0274). The routes, the roster key
//! (a vault engine key), the person confirms and the federation and node
//! sends are `k2-daemon`'s `compute_trust`.
//!
//! **Two sides, one server.** A server can be a node **owner** (it enrolled
//! the nodes first: it keeps the roster entries, signs the node roster and
//! seat statements, approves lanes and seats) and a **client** (it holds
//! seats on another owner's nodes: it pins that owner's roster key, keeps
//! the high-water roster `seq`, the secrets consent, and the revocations).
//! K2's own release server is a client of `rosson.k2.dev` like any other
//! (PRD §5).
//!
//! **People approve, agents ask.** Every roster change, lane template and
//! seat (offer on the owner's side, request on the client's) is a held
//! [`TrustRequest`] that only a person (owner or Admin, with a fresh vault
//! confirm) decides, exactly once ([`decide`] is a single guarded
//! `UPDATE`).
//!
//! Formats are frozen in `k2-node-proto` (`node_roster`, `statement`,
//! `lanes`); this module only stores and checks.

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use serde_json::{json, Value};

use super::proto::admin::ApprovalKey;
use super::proto::ci::{self, KeyStatus, RevokeReason, Signed, Taint, TaintBasis};
use super::proto::crypto;
use super::proto::frames::KeyRotation;
use super::proto::lanes::{LaneSeat, LaneTemplate, LanesFile};
use super::proto::node_roster::{NodeRoster, OwnerAttestation, RosterEntry, SeatStatement};
use super::proto::statement::{NodeStatement, RotationStatement};
use super::store::NodeRow;

/// A held roster change or lane template lives this long.
pub const REQUEST_TTL_SECS: i64 = 24 * 3600;
/// A seat request waits for the owner's person this long (either side).
pub const SEAT_REQUEST_TTL_SECS: i64 = 7 * 86_400;
/// The seat enrol code (one per seat, many nodes) lives this long.
pub const SEAT_CODE_TTL_SECS: i64 = 7 * 86_400;
/// An unchanged roster is re-signed (new `seq`, `issued_at`) when the newest
/// is older than this, so a signer's `roster_max_age_hours` (V13, 168
/// recommended) never trips on a quiet owner.
pub const ROSTER_REFRESH_SECS: i64 = 2 * 86_400;
/// Pending requests one proposer may hold.
pub const MAX_PENDING_PER_PROPOSER: i64 = 10;

/// Vault confirm actions (each bound to the request or pairing id).
pub const CONFIRM_TRUST: &str = "compute-trust";
pub const CONFIRM_SIGNER: &str = "ci-signer";

/// Stable error codes (exit 3 with teaching text).
pub mod codes {
    pub const PERSON_ONLY: &str = "person_only";
    pub const REQUEST_NOT_FOUND: &str = "request_not_found";
    pub const NOT_PENDING: &str = "not_pending";
    pub const APPROVAL_EXPIRED: &str = "approval_expired";
    pub const TOO_MANY_PENDING: &str = "too_many_pending";
    pub const BAD_ARGS: &str = "bad_args";
    pub const NO_STATEMENT: &str = "node_statement_missing";
    pub const NODE_KEY_TAINTED: &str = super::ci::codes::NODE_KEY_TAINTED;
    pub const LANE_NOT_FOUND: &str = "lane_not_found";
    pub const SEAT_NOT_FOUND: &str = "seat_not_found";
    pub const OWNER_NOT_TRUSTED: &str = "node_owner_not_trusted";
    pub const SECRET_OWNER_NOT_TRUSTED: &str = super::ci::codes::SECRET_OWNER_NOT_TRUSTED;
    pub const ROSTER_REPLAYED: &str = super::ci::codes::ROSTER_REPLAYED;
    pub const NODE_LACKS_LANES: &str = "node_lacks_lanes";
    pub const MAC_SEAT_NOT_OFFERED: &str = super::ci::codes::MAC_SEAT_NOT_OFFERED;
    pub const NODE_NOT_ON_ROSTER: &str = "node_not_on_roster";
    pub const SEAT_NOT_ACTIVE: &str = "seat_not_active";
    pub const KEY_REVOKED: &str = "node_key_revoked";
    pub const ROTATION_REFUSED: &str = "rotation_refused";
    pub const NODE_CLIENT_CAP: &str = super::ci::codes::NODE_CLIENT_CAP;
}

/// A name (`[a-z0-9._-]`, 1–63, starts alphanumeric: lanes, seats, owners).
pub fn name_ok(what: &str, s: &str) -> Result<(), String> {
    if ci::is_name(s) {
        Ok(())
    } else {
        Err(format!("{what}: {s:?} is not a name ([a-z0-9._-], 1-63, starts with a letter or digit)"))
    }
}

// ── node statements (owner and client alike) ─────────────────────────

/// Keep a node's latest self-statement. The caller verified it against the
/// key this server pinned for `node_id`.
pub fn save_statement(conn: &Connection, node_id: &str, s: &Signed, at: i64) -> Result<NodeStatement, String> {
    let st: NodeStatement = s.peek(ci::DOMAIN_NODE_STATEMENT)?;
    st.check()?;
    conn.execute(
        "INSERT INTO compute_node_statements (node_id, node_fp, body, sig, digest, received_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(node_id) DO UPDATE SET node_fp = ?2, body = ?3, sig = ?4, digest = ?5, received_at = ?6",
        params![node_id, st.node_fp, s.body, s.sig, s.digest(), at],
    )
    .map_err(|e| e.to_string())?;
    Ok(st)
}

/// The latest statement a node sent (signed text and parsed).
pub fn statement_of(conn: &Connection, node_id: &str) -> Option<(Signed, NodeStatement)> {
    let (body, sig): (String, String) = conn
        .query_row("SELECT body, sig FROM compute_node_statements WHERE node_id = ?1", params![node_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .ok()
        .flatten()?;
    let s = Signed { body, sig };
    let st = s.peek(ci::DOMAIN_NODE_STATEMENT).ok()?;
    Some((s, st))
}

/// The key epoch, previous key and seat of a node row (0274 columns).
pub fn node_key_facts(conn: &Connection, node_id: &str) -> (u32, Option<String>, Option<String>) {
    conn.query_row(
        "SELECT key_epoch, prev_fingerprint, seat_id FROM compute_nodes WHERE id = ?1",
        params![node_id],
        |r| Ok((r.get::<_, i64>(0)? as u32, r.get(1)?, r.get(2)?)),
    )
    .unwrap_or((0, None, None))
}

/// Why the owner may vouch for a key (V10, V12): a self-statement that says
/// `clean` vouches for itself; `unknown` needs the owner's attestation;
/// `tainted` (or a `--dev` / unwalled node) never counts for a lane.
pub fn taint_basis_for(st: &NodeStatement, attested: bool, rekeyed: bool) -> Result<TaintBasis, (&'static str, String)> {
    if st.dev || st.wall == ci::Wall::None || st.key_exposure.job_readable {
        return Err((codes::NODE_KEY_TAINTED, "this node runs without the job wall (or --dev): it can't serve a CI lane; turn the wall on and re-key it".into()));
    }
    match st.taint {
        Taint::Tainted => Err((codes::NODE_KEY_TAINTED, "this key ran a job without the job wall: only a re-key under the wall clears it".into())),
        Taint::Clean if rekeyed && st.key_epoch > 0 => Ok(TaintBasis::Rekeyed),
        Taint::Clean => Ok(TaintBasis::SelfClean),
        Taint::Unknown if attested => Ok(TaintBasis::OwnerAttested),
        Taint::Unknown => Err((
            codes::NODE_KEY_TAINTED,
            "this key predates the taint flag (taint: unknown): attest it (--attest \"what you checked\") or re-key it under the wall".into(),
        )),
    }
}

// ── roster entries (owner) ───────────────────────────────────────────

/// One key in the owner's working list.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryRow {
    pub entry: RosterEntry,
    pub node_id: Option<String>,
    pub attestation: Option<Signed>,
    pub updated_at: i64,
}

const ENTRY_COLS: &str = "node_fp, node_id, name, node_spki, key_epoch, statement_digest, status, revoked_reason, valid_from, valid_to, \
    taint_basis, attestation_body, attestation_sig, host, provider, site, admin_reach_json, pod, point, updated_at";

fn enum_from<T: serde::de::DeserializeOwned>(s: Option<String>) -> Option<T> {
    s.and_then(|s| serde_json::from_value(Value::String(s)).ok())
}

fn enum_text<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn entry_from_row(r: &Row<'_>) -> rusqlite::Result<EntryRow> {
    let reach: String = r.get(16)?;
    let att_body: Option<String> = r.get(11)?;
    let att_sig: Option<String> = r.get(12)?;
    let attestation = match (att_body, att_sig) {
        (Some(body), Some(sig)) => Some(Signed { body, sig }),
        _ => None,
    };
    let status: String = r.get(6)?;
    Ok(EntryRow {
        entry: RosterEntry {
            node_fp: r.get(0)?,
            node_spki: r.get(3)?,
            name: r.get(2)?,
            key_epoch: r.get::<_, i64>(4)? as u32,
            statement_digest: r.get(5)?,
            status: enum_from(Some(status)).unwrap_or(KeyStatus::Retired),
            revoked_reason: enum_from(r.get(7)?),
            valid_from: r.get(8)?,
            valid_to: r.get(9)?,
            taint_basis: enum_from(r.get(10)?),
            attestation_digest: attestation.as_ref().map(Signed::digest),
            host: r.get(13)?,
            provider: r.get(14)?,
            site: r.get(15)?,
            admin_reach: serde_json::from_str(&reach).unwrap_or_default(),
            pod: r.get(17)?,
            point: r.get::<_, Option<i64>>(18)?.map(|p| p as u32),
        },
        node_id: r.get(1)?,
        attestation,
        updated_at: r.get(19)?,
    })
}

pub fn entries(conn: &Connection) -> Vec<EntryRow> {
    let Ok(mut stmt) = conn.prepare(&format!("SELECT {ENTRY_COLS} FROM compute_roster_entries ORDER BY node_fp")) else { return Vec::new() };
    stmt.query_map([], entry_from_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

pub fn entry_by_fp(conn: &Connection, node_fp: &str) -> Option<EntryRow> {
    conn.query_row(&format!("SELECT {ENTRY_COLS} FROM compute_roster_entries WHERE node_fp = ?1"), params![node_fp], entry_from_row)
        .optional()
        .ok()
        .flatten()
}

pub fn active_entry_by_name(conn: &Connection, name: &str) -> Option<EntryRow> {
    conn.query_row(
        &format!("SELECT {ENTRY_COLS} FROM compute_roster_entries WHERE name = ?1 AND status = 'active'"),
        params![name],
        entry_from_row,
    )
    .optional()
    .ok()
    .flatten()
}

/// Write an entry (insert or replace by key). Checks the entry's own rules
/// and that no other key is active under the same name.
pub fn put_entry(conn: &Connection, row: &EntryRow, at: i64) -> Result<(), String> {
    let e = &row.entry;
    e.check()?;
    if e.status == KeyStatus::Active {
        if let Some(other) = active_entry_by_name(conn, &e.name) {
            if other.entry.node_fp != e.node_fp {
                return Err(format!("{} already has an active key ({}…): retire it first", e.name, &other.entry.node_fp[..12]));
            }
        }
    }
    let (ab, asig) = match &row.attestation {
        Some(s) => (Some(s.body.clone()), Some(s.sig.clone())),
        None => (None, None),
    };
    conn.execute(
        &format!(
            "INSERT OR REPLACE INTO compute_roster_entries ({ENTRY_COLS}) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)"
        ),
        params![
            e.node_fp,
            row.node_id,
            e.name,
            e.node_spki,
            e.key_epoch as i64,
            e.statement_digest,
            enum_text(&e.status),
            e.revoked_reason.as_ref().map(enum_text),
            e.valid_from,
            e.valid_to,
            e.taint_basis.as_ref().map(enum_text),
            ab,
            asig,
            e.host,
            e.provider,
            e.site,
            serde_json::to_string(&e.admin_reach).map_err(|x| x.to_string())?,
            e.pod,
            e.point.map(|p| p as i64),
            at
        ],
    )
    .map_err(|x| x.to_string())?;
    Ok(())
}

// ── node rosters (owner issues, client receives) ─────────────────────

/// One stored roster.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredRoster {
    pub signed: Signed,
    pub roster: NodeRoster,
    pub source: String,
    pub received_at: i64,
}

fn roster_from_row(r: &Row<'_>) -> rusqlite::Result<(String, String, String, i64)> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
}

fn stored(t: (String, String, String, i64)) -> Option<StoredRoster> {
    let signed = Signed { body: t.0, sig: t.1 };
    let roster = signed.peek(ci::DOMAIN_NODE_ROSTER).ok()?;
    Some(StoredRoster { signed, roster, source: t.2, received_at: t.3 })
}

/// The newest roster stored under a roster key.
pub fn latest_roster(conn: &Connection, key_fp: &str) -> Option<StoredRoster> {
    conn.query_row(
        "SELECT body, sig, source, received_at FROM compute_node_rosters WHERE key_fp = ?1 ORDER BY seq DESC LIMIT 1",
        params![key_fp],
        roster_from_row,
    )
    .optional()
    .ok()
    .flatten()
    .and_then(stored)
}

/// The newest roster this server signed itself.
pub fn latest_own_roster(conn: &Connection) -> Option<StoredRoster> {
    conn.query_row(
        "SELECT body, sig, source, received_at FROM compute_node_rosters WHERE source = 'self' ORDER BY issued_at DESC, seq DESC LIMIT 1",
        [],
        roster_from_row,
    )
    .optional()
    .ok()
    .flatten()
    .and_then(stored)
}

/// The roster to sign next from the working entries: `seq` + 1 over the
/// newest under this key, chained by `prev_digest` (V13).
pub fn next_roster(conn: &Connection, owner_id: &str, owner_server_fp: &str, key: &ApprovalKey, now: i64) -> Result<NodeRoster, String> {
    let prev = latest_roster(conn, &key.fingerprint());
    let mut list: Vec<RosterEntry> = entries(conn).into_iter().map(|e| e.entry).collect();
    list.sort_by(|a, b| a.node_fp.cmp(&b.node_fp));
    let r = NodeRoster {
        v: super::proto::node_roster::NODE_ROSTER_V,
        owner_id: owner_id.to_string(),
        owner_server_fp: owner_server_fp.to_string(),
        roster_key: key.encode(),
        seq: prev.as_ref().map(|p| p.roster.seq + 1).unwrap_or(1),
        issued_at: now.max(prev.as_ref().map(|p| p.roster.issued_at).unwrap_or(0)),
        prev_digest: prev.as_ref().map(|p| p.signed.digest()),
        entries: list,
    };
    r.check()?;
    Ok(r)
}

/// Store a roster. The caller verified the signature with the right key.
/// Returns false when that `seq` is already stored (idempotent).
pub fn store_roster(conn: &Connection, signed: &Signed, r: &NodeRoster, source: &str, at: i64) -> Result<bool, String> {
    let key_fp = r.key_fp()?;
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO compute_node_rosters (key_fp, seq, owner_id, body, sig, digest, issued_at, received_at, source) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![key_fp, r.seq as i64, r.owner_id, signed.body, signed.sig, signed.digest(), r.issued_at, at, source],
        )
        .map_err(|e| e.to_string())?;
    Ok(n == 1)
}

/// Does the owner's newest roster need a re-sign (V13 freshness)?
pub fn roster_stale(conn: &Connection, now: i64) -> bool {
    // The stored `issued_at` column (= the signed body's, written with it).
    let at: Option<i64> = conn.query_row("SELECT MAX(issued_at) FROM compute_node_rosters WHERE source = 'self'", [], |r| r.get(0)).ok().flatten();
    at.is_some_and(|at| now - at >= ROSTER_REFRESH_SECS)
}

/// Do the working entries differ from the newest signed roster (anything
/// a person must approve)? Only `seq`, `issued_at` and `prev_digest` may
/// change without one.
pub fn entries_changed(conn: &Connection) -> bool {
    let mut now: Vec<RosterEntry> = entries(conn).into_iter().map(|e| e.entry).collect();
    now.sort_by(|a, b| a.node_fp.cmp(&b.node_fp));
    match latest_own_roster(conn) {
        Some(r) => r.roster.entries != now,
        None => !now.is_empty(),
    }
}

// ── held requests (both sides) ───────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustRequest {
    pub id: String,
    pub kind: String,
    pub subject: String,
    pub body: Value,
    pub digest: String,
    pub summary: String,
    pub reason: Option<String>,
    pub proposed_by: String,
    pub proposer_workspace: Option<String>,
    pub proposed_at: i64,
    pub state: String,
    pub decided_by: Option<String>,
    pub decided_via: Option<String>,
    pub decided_at: Option<i64>,
    pub deny_reason: Option<String>,
    pub expires_at: i64,
    pub result: Option<Value>,
}

const REQ_COLS: &str = "id, kind, subject, body_json, digest, summary, reason, proposed_by, proposer_workspace, proposed_at, state, \
    decided_by, decided_via, decided_at, deny_reason, expires_at, result_json";

fn req_from_row(r: &Row<'_>) -> rusqlite::Result<TrustRequest> {
    let body: String = r.get(3)?;
    let result: Option<String> = r.get(16)?;
    Ok(TrustRequest {
        id: r.get(0)?,
        kind: r.get(1)?,
        subject: r.get(2)?,
        body: serde_json::from_str(&body).unwrap_or(Value::Null),
        digest: r.get(4)?,
        summary: r.get(5)?,
        reason: r.get(6)?,
        proposed_by: r.get(7)?,
        proposer_workspace: r.get(8)?,
        proposed_at: r.get(9)?,
        state: r.get(10)?,
        decided_by: r.get(11)?,
        decided_via: r.get(12)?,
        decided_at: r.get(13)?,
        deny_reason: r.get(14)?,
        expires_at: r.get(15)?,
        result: result.and_then(|x| serde_json::from_str(&x).ok()),
    })
}

/// A new held request.
pub struct NewTrustRequest<'a> {
    pub kind: &'a str,
    pub subject: &'a str,
    pub body: &'a Value,
    pub summary: &'a str,
    pub reason: Option<&'a str>,
    pub proposed_by: &'a str,
    pub proposer_workspace: Option<&'a str>,
    pub now: i64,
    pub ttl: i64,
}

pub fn insert_request(conn: &Connection, n: &NewTrustRequest<'_>) -> Result<TrustRequest, (&'static str, String)> {
    if !matches!(n.kind, "roster" | "lane" | "seat_offer" | "seat_request") {
        return Err((codes::BAD_ARGS, format!("no request kind {:?}", n.kind)));
    }
    let pending: i64 = match n.proposer_workspace {
        Some(ws) => conn.query_row("SELECT COUNT(*) FROM compute_trust_requests WHERE state = 'pending' AND proposer_workspace = ?1", params![ws], |r| r.get(0)),
        None => conn.query_row(
            "SELECT COUNT(*) FROM compute_trust_requests WHERE state = 'pending' AND proposer_workspace IS NULL AND proposed_by = ?1",
            params![n.proposed_by],
            |r| r.get(0),
        ),
    }
    .unwrap_or(0);
    if pending >= MAX_PENDING_PER_PROPOSER {
        return Err((codes::TOO_MANY_PENDING, format!("{MAX_PENDING_PER_PROPOSER} requests already wait for a person; withdraw some first")));
    }
    let digest = format!(
        "sha256:{}",
        crypto::sha256_hex(&super::proto::canonical::to_vec(&json!({"kind": n.kind, "subject": n.subject, "body": n.body})).map_err(|e| (codes::BAD_ARGS, e))?)
    );
    let id = format!("ctr_{}", crypto::random_id().map_err(|e| (codes::BAD_ARGS, e))?);
    conn.execute(
        "INSERT INTO compute_trust_requests (id, kind, subject, body_json, digest, summary, reason, proposed_by, proposer_workspace, \
         proposed_at, state, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending', ?11)",
        params![id, n.kind, n.subject, n.body.to_string(), digest, n.summary, n.reason, n.proposed_by, n.proposer_workspace, n.now, n.now + n.ttl],
    )
    .map_err(|e| (codes::BAD_ARGS, e.to_string()))?;
    request_by_id(conn, &id).ok_or((codes::REQUEST_NOT_FOUND, "request vanished after insert".into()))
}

pub fn request_by_id(conn: &Connection, id: &str) -> Option<TrustRequest> {
    conn.query_row(&format!("SELECT {REQ_COLS} FROM compute_trust_requests WHERE id = ?1"), params![id], req_from_row)
        .optional()
        .ok()
        .flatten()
}

/// Requests, newest first. `state`: `open` = pending; `None` = all.
pub fn list_requests(conn: &Connection, state: Option<&str>, workspace: Option<&str>, limit: i64) -> Vec<TrustRequest> {
    let st = match state {
        Some("open") => Some("pending"),
        other => other,
    };
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {REQ_COLS} FROM compute_trust_requests WHERE (?1 IS NULL OR state = ?1) AND (?2 IS NULL OR proposer_workspace = ?2) \
         ORDER BY proposed_at DESC, id LIMIT ?3"
    )) else {
        return Vec::new();
    };
    stmt.query_map(params![st, workspace, limit.clamp(1, 500)], req_from_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// Pending past their time → `expired`.
pub fn expire_due(conn: &Connection, now: i64) -> Vec<TrustRequest> {
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM compute_trust_requests WHERE state = 'pending' AND expires_at <= ?1")
        .and_then(|mut s| s.query_map(params![now], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    ids.into_iter()
        .filter(|id| {
            conn.execute("UPDATE compute_trust_requests SET state = 'expired', decided_at = ?2 WHERE id = ?1 AND state = 'pending'", params![id, now])
                .unwrap_or(0)
                == 1
        })
        .filter_map(|id| request_by_id(conn, &id))
        .collect()
}

/// Decide a pending request exactly once (`approved`, `denied`,
/// `withdrawn`). Err: `request_not_found`, `approval_expired`, `not_pending`.
pub fn decide(conn: &Connection, id: &str, to: &str, by: &str, via: Option<&str>, deny_reason: Option<&str>, at: i64) -> Result<TrustRequest, &'static str> {
    if !matches!(to, "approved" | "denied" | "withdrawn") {
        return Err(codes::NOT_PENDING);
    }
    let n = conn
        .execute(
            "UPDATE compute_trust_requests SET state = ?2, decided_by = ?3, decided_via = ?4, decided_at = ?5, deny_reason = ?6 \
             WHERE id = ?1 AND state = 'pending' AND expires_at > ?5",
            params![id, to, by, via, at, deny_reason],
        )
        .unwrap_or(0);
    if n == 1 {
        return request_by_id(conn, id).ok_or(codes::REQUEST_NOT_FOUND);
    }
    match request_by_id(conn, id) {
        None => Err(codes::REQUEST_NOT_FOUND),
        Some(r) if r.state == "pending" || r.state == "expired" => {
            let _ = conn.execute("UPDATE compute_trust_requests SET state = 'expired', decided_at = ?2 WHERE id = ?1 AND state = 'pending'", params![id, at]);
            Err(codes::APPROVAL_EXPIRED)
        }
        Some(_) => Err(codes::NOT_PENDING),
    }
}

/// Record what an approved request did (or `failed` with why).
pub fn set_result(conn: &Connection, id: &str, failed: bool, result: &Value) -> Result<(), String> {
    conn.execute(
        "UPDATE compute_trust_requests SET result_json = ?2, state = CASE WHEN ?3 THEN 'failed' ELSE state END WHERE id = ?1",
        params![id, result.to_string(), failed],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Replace a still-pending request's body (an operator names the nodes of a
/// seat request before a person approves it). The digest and summary follow.
pub fn rewrite_pending(conn: &Connection, id: &str, body: &Value, summary: &str) -> Result<TrustRequest, &'static str> {
    let r = request_by_id(conn, id).ok_or(codes::REQUEST_NOT_FOUND)?;
    let digest = format!(
        "sha256:{}",
        crypto::sha256_hex(&super::proto::canonical::to_vec(&json!({"kind": r.kind, "subject": r.subject, "body": body})).map_err(|_| codes::BAD_ARGS)?)
    );
    let n = conn
        .execute(
            "UPDATE compute_trust_requests SET body_json = ?2, digest = ?3, summary = ?4 WHERE id = ?1 AND state = 'pending'",
            params![id, body.to_string(), digest, summary],
        )
        .unwrap_or(0);
    if n != 1 {
        return Err(codes::NOT_PENDING);
    }
    request_by_id(conn, id).ok_or(codes::REQUEST_NOT_FOUND)
}

// ── lanes (owner) ────────────────────────────────────────────────────

pub fn lane(conn: &Connection, name: &str) -> Option<LaneTemplate> {
    let t: String = conn.query_row("SELECT template_json FROM compute_lanes WHERE name = ?1", params![name], |r| r.get(0)).optional().ok().flatten()?;
    serde_json::from_str(&t).ok()
}

pub fn lanes(conn: &Connection) -> BTreeMap<String, (LaneTemplate, String, i64)> {
    let Ok(mut stmt) = conn.prepare("SELECT name, template_json, approved_by, approved_at FROM compute_lanes ORDER BY name") else { return BTreeMap::new() };
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))
        .map(|rows| {
            rows.filter_map(Result::ok)
                .filter_map(|(n, t, by, at)| serde_json::from_str(&t).ok().map(|t| (n, (t, by, at))))
                .collect()
        })
        .unwrap_or_default()
}

pub fn set_lane(conn: &Connection, name: &str, t: &LaneTemplate, by: &str, via: &str, request: Option<&str>, at: i64) -> Result<(), String> {
    name_ok("lane", name)?;
    t.check()?;
    conn.execute(
        "INSERT INTO compute_lanes (name, template_json, approved_by, approved_via, approved_at, request_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(name) DO UPDATE SET template_json = ?2, approved_by = ?3, approved_via = ?4, approved_at = ?5, request_id = ?6",
        params![name, serde_json::to_string(t).map_err(|e| e.to_string())?, by, via, at, request],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ── seats (both sides) ───────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatRow {
    pub id: String,
    pub side: String,
    pub peer_fp: String,
    pub peer_host: String,
    pub owner_id: String,
    pub owner_key_fp: Option<String>,
    pub client_fp: String,
    pub client_label: String,
    pub client_url: String,
    pub lane: String,
    pub nodes: Vec<String>,
    pub state: String,
    #[serde(skip)]
    pub statement: Option<Signed>,
    pub roster_seq: Option<i64>,
    #[serde(skip)]
    pub code_binding: Option<String>,
    pub request_id: Option<String>,
    pub batch_id: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

const SEAT_COLS: &str = "id, side, peer_fp, peer_host, owner_id, owner_key_fp, client_fp, client_label, client_url, lane, nodes_json, state, \
    statement_body, statement_sig, roster_seq, code_binding, request_id, batch_id, note, created_at, updated_at";

fn seat_from_row(r: &Row<'_>) -> rusqlite::Result<SeatRow> {
    let nodes: String = r.get(10)?;
    let sb: Option<String> = r.get(12)?;
    let ss: Option<String> = r.get(13)?;
    Ok(SeatRow {
        id: r.get(0)?,
        side: r.get(1)?,
        peer_fp: r.get(2)?,
        peer_host: r.get(3)?,
        owner_id: r.get(4)?,
        owner_key_fp: r.get(5)?,
        client_fp: r.get(6)?,
        client_label: r.get(7)?,
        client_url: r.get(8)?,
        lane: r.get(9)?,
        nodes: serde_json::from_str(&nodes).unwrap_or_default(),
        state: r.get(11)?,
        statement: match (sb, ss) {
            (Some(body), Some(sig)) => Some(Signed { body, sig }),
            _ => None,
        },
        roster_seq: r.get(14)?,
        code_binding: r.get(15)?,
        request_id: r.get(16)?,
        batch_id: r.get(17)?,
        note: r.get(18)?,
        created_at: r.get(19)?,
        updated_at: r.get(20)?,
    })
}

pub fn put_seat(conn: &Connection, s: &SeatRow) -> Result<(), String> {
    name_ok("seat id", &s.id)?;
    if !matches!(s.side.as_str(), "owner" | "client") {
        return Err("seat side is owner or client".into());
    }
    // An upsert, not INSERT OR REPLACE: the seat's lane template (0275,
    // [`set_seat_lane`]) is kept across updates.
    let set: Vec<String> = SEAT_COLS.split(", ").filter(|c| *c != "id").map(|c| format!("{c} = excluded.{c}")).collect();
    conn.execute(
        &format!(
            "INSERT INTO compute_seats ({SEAT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21) \
             ON CONFLICT(id) DO UPDATE SET {}",
            set.join(", ")
        ),
        params![
            s.id,
            s.side,
            s.peer_fp,
            s.peer_host,
            s.owner_id,
            s.owner_key_fp,
            s.client_fp,
            s.client_label,
            s.client_url,
            s.lane,
            serde_json::to_string(&s.nodes).map_err(|e| e.to_string())?,
            s.state,
            s.statement.as_ref().map(|x| x.body.clone()),
            s.statement.as_ref().map(|x| x.sig.clone()),
            s.roster_seq,
            s.code_binding,
            s.request_id,
            s.batch_id,
            s.note,
            s.created_at,
            s.updated_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The lane template a seat carries (0275): on the client, the one the
/// owner sent with its offer (what the controller checks before it seals
/// a lane secret, V25); on the owner, the one the seat was approved under.
pub fn seat_lane(conn: &Connection, side: &str, id: &str) -> Option<LaneTemplate> {
    let t: String = conn
        .query_row("SELECT lane_json FROM compute_seats WHERE side = ?1 AND id = ?2", params![side, id], |r| r.get::<_, Option<String>>(0))
        .optional()
        .ok()
        .flatten()
        .flatten()?;
    serde_json::from_str(&t).ok()
}

/// Keep a seat's lane template (checked). False when there's no such seat.
pub fn set_seat_lane(conn: &Connection, side: &str, id: &str, t: &LaneTemplate) -> Result<bool, String> {
    t.check()?;
    let n = conn
        .execute("UPDATE compute_seats SET lane_json = ?3 WHERE side = ?1 AND id = ?2", params![side, id, serde_json::to_string(t).map_err(|e| e.to_string())?])
        .map_err(|e| e.to_string())?;
    Ok(n == 1)
}

pub fn seat(conn: &Connection, side: &str, id: &str) -> Option<SeatRow> {
    conn.query_row(&format!("SELECT {SEAT_COLS} FROM compute_seats WHERE side = ?1 AND id = ?2"), params![side, id], seat_from_row)
        .optional()
        .ok()
        .flatten()
}

pub fn seats(conn: &Connection, side: Option<&str>) -> Vec<SeatRow> {
    let Ok(mut stmt) = conn.prepare(&format!("SELECT {SEAT_COLS} FROM compute_seats WHERE (?1 IS NULL OR side = ?1) ORDER BY side, id")) else {
        return Vec::new();
    };
    stmt.query_map(params![side], seat_from_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// The owner's live seats (offered or active) on a node, by its roster name.
pub fn owner_seats_on(conn: &Connection, node_name: &str) -> Vec<SeatRow> {
    seats(conn, Some("owner")).into_iter().filter(|s| matches!(s.state.as_str(), "offered" | "active") && s.nodes.iter().any(|n| n == node_name)).collect()
}

/// The whole lanes file for one of the owner's nodes: its live seats and
/// the lanes they name (V22: the owner writes it whole).
pub fn lanes_file_for(conn: &Connection, node_name: &str) -> Result<LanesFile, String> {
    let mut f = LanesFile::default();
    for s in owner_seats_on(conn, node_name) {
        let t = lane(conn, &s.lane).ok_or_else(|| format!("seat {} names lane {}, which isn't approved here", s.id, s.lane))?;
        f.lanes.insert(s.lane.clone(), t);
        f.seats.push(LaneSeat { seat: s.id.clone(), server_fp: s.client_fp.clone(), lane: s.lane.clone(), label: s.client_label.clone() });
    }
    f.seats.sort_by(|a, b| a.server_fp.cmp(&b.server_fp));
    f.check()?;
    Ok(f)
}

/// How many distinct client servers hold live seats on a node (the pod cap
/// is 0.46.11; 0.46.10 keeps the node's own slot cap: at most
/// `share::MAX_SLOTS - 1` clients).
pub fn clients_on(conn: &Connection, node_name: &str) -> usize {
    let mut fps: Vec<String> = owner_seats_on(conn, node_name).into_iter().map(|s| s.client_fp).collect();
    fps.sort();
    fps.dedup();
    fps.len()
}

// ── client side: owners, consent, revocations ────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerTrust {
    pub key_fp: String,
    pub roster_key: String,
    pub owner_id: String,
    pub peer_fp: String,
    pub pinned_by: String,
    pub pinned_via: String,
    pub pinned_at: i64,
    pub secrets_consent: bool,
    pub consent_by: Option<String>,
    pub consent_at: Option<i64>,
    pub high_seq: i64,
}

const TRUST_COLS: &str = "key_fp, roster_key, owner_id, peer_fp, pinned_by, pinned_via, pinned_at, secrets_consent, consent_by, consent_at, high_seq";

fn trust_from_row(r: &Row<'_>) -> rusqlite::Result<OwnerTrust> {
    Ok(OwnerTrust {
        key_fp: r.get(0)?,
        roster_key: r.get(1)?,
        owner_id: r.get(2)?,
        peer_fp: r.get(3)?,
        pinned_by: r.get(4)?,
        pinned_via: r.get(5)?,
        pinned_at: r.get(6)?,
        secrets_consent: r.get::<_, i64>(7)? != 0,
        consent_by: r.get(8)?,
        consent_at: r.get(9)?,
        high_seq: r.get(10)?,
    })
}

pub fn owner_trusts(conn: &Connection) -> Vec<OwnerTrust> {
    let Ok(mut stmt) = conn.prepare(&format!("SELECT {TRUST_COLS} FROM compute_owner_trust ORDER BY owner_id")) else { return Vec::new() };
    stmt.query_map([], trust_from_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

pub fn owner_trust(conn: &Connection, key_fp: &str) -> Option<OwnerTrust> {
    conn.query_row(&format!("SELECT {TRUST_COLS} FROM compute_owner_trust WHERE key_fp = ?1"), params![key_fp], trust_from_row)
        .optional()
        .ok()
        .flatten()
}

pub fn owner_trust_by_peer(conn: &Connection, peer_fp: &str) -> Option<OwnerTrust> {
    conn.query_row(&format!("SELECT {TRUST_COLS} FROM compute_owner_trust WHERE peer_fp = ?1"), params![peer_fp], trust_from_row)
        .optional()
        .ok()
        .flatten()
}

/// A person pins an owner's roster key here (and says whether this server's
/// vault secrets may go to that owner's nodes: V25, PRD §4.4 rule 3).
#[allow(clippy::too_many_arguments)]
pub fn pin_owner(conn: &Connection, key: &ApprovalKey, owner_id: &str, peer_fp: &str, by: &str, via: &str, consent: bool, at: i64) -> Result<OwnerTrust, String> {
    name_ok("owner", owner_id)?;
    conn.execute(
        "INSERT INTO compute_owner_trust (key_fp, roster_key, owner_id, peer_fp, pinned_by, pinned_via, pinned_at, secrets_consent, consent_by, consent_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT(key_fp) DO UPDATE SET owner_id = ?3, peer_fp = ?4, pinned_by = ?5, pinned_via = ?6, pinned_at = ?7, \
         secrets_consent = ?8, consent_by = ?9, consent_at = ?10",
        params![key.fingerprint(), key.encode(), owner_id, peer_fp, by, via, at, consent as i64, consent.then_some(by), consent.then_some(at)],
    )
    .map_err(|e| e.to_string())?;
    owner_trust(conn, &key.fingerprint()).ok_or_else(|| "owner trust vanished".into())
}

/// Give the secrets consent for a pinned owner key (the caller took a
/// person's fresh confirm bound to `consent:<key_fp>`, V17). The pin itself
/// (who pinned, how) stays. False when no such key is pinned.
pub fn give_consent(conn: &Connection, key_fp: &str, by: &str, at: i64) -> Result<bool, String> {
    let n = conn
        .execute("UPDATE compute_owner_trust SET secrets_consent = 1, consent_by = ?2, consent_at = ?3 WHERE key_fp = ?1", params![key_fp, by, at])
        .map_err(|e| e.to_string())?;
    Ok(n == 1)
}

/// Withdraw the secrets consent (a narrowing: at once, by anyone with
/// Admin, or the owner).
pub fn withdraw_consent(conn: &Connection, key_fp: &str) -> Result<bool, String> {
    let n = conn.execute("UPDATE compute_owner_trust SET secrets_consent = 0, consent_by = NULL, consent_at = NULL WHERE key_fp = ?1", params![key_fp]).map_err(|e| e.to_string())?;
    Ok(n == 1)
}

/// What receiving a roster changed here.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterEffects {
    pub stored: bool,
    pub seq: u64,
    /// Node keys this roster revoked that this server had not recorded yet.
    pub revoked: Vec<String>,
}

/// A client receives an owner's roster (through the node or federation):
/// verify it with the pinned key, refuse a lower `seq` than the high-water
/// mark (V13), store it, record revocations. A roster under a key no person
/// pinned here is refused (`node_owner_not_trusted`).
pub fn receive_roster(conn: &Connection, s: &Signed, source: &str, at: i64) -> Result<RosterEffects, (&'static str, String)> {
    let peek: NodeRoster = s.peek(ci::DOMAIN_NODE_ROSTER).map_err(|e| (codes::BAD_ARGS, e))?;
    let key = ApprovalKey::parse(&peek.roster_key).ok_or((codes::BAD_ARGS, "the roster's key is not ed25519".to_string()))?;
    let trust = owner_trust(conn, &key.fingerprint())
        .ok_or_else(|| (codes::OWNER_NOT_TRUSTED, format!("no person here pinned {}'s roster key ({}…)", peek.owner_id, &key.fingerprint()[..16])))?;
    let pinned = ApprovalKey::parse(&trust.roster_key).ok_or((codes::BAD_ARGS, "pinned roster key unreadable".to_string()))?;
    let r = NodeRoster::open(s, &pinned).map_err(|e| (codes::BAD_ARGS, e))?;
    if (r.seq as i64) < trust.high_seq {
        return Err((codes::ROSTER_REPLAYED, format!("roster seq {} is older than {} already seen from {}", r.seq, trust.high_seq, r.owner_id)));
    }
    let stored = store_roster(conn, s, &r, source, at).map_err(|e| (codes::BAD_ARGS, e))?;
    conn.execute("UPDATE compute_owner_trust SET high_seq = MAX(high_seq, ?2) WHERE key_fp = ?1", params![trust.key_fp, r.seq as i64])
        .map_err(|e| (codes::BAD_ARGS, e.to_string()))?;
    let mut revoked = Vec::new();
    for e in r.entries.iter().filter(|e| e.status == KeyStatus::Revoked) {
        let reason = e.revoked_reason.unwrap_or(RevokeReason::Other);
        let n = conn
            .execute(
                "INSERT OR IGNORE INTO compute_key_revocations (node_fp, key_fp, reason, roster_seq, at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![e.node_fp, trust.key_fp, enum_text(&reason), r.seq as i64, at],
            )
            .unwrap_or(0);
        if n == 1 {
            revoked.push(e.node_fp.clone());
        }
    }
    Ok(RosterEffects { stored, seq: r.seq, revoked })
}

/// The owner side records its own revocations too (its receipts view).
pub fn record_revocation(conn: &Connection, node_fp: &str, key_fp: &str, reason: RevokeReason, seq: u64, at: i64) {
    let _ = conn.execute(
        "INSERT OR IGNORE INTO compute_key_revocations (node_fp, key_fp, reason, roster_seq, at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![node_fp, key_fp, enum_text(&reason), seq as i64, at],
    );
}

/// Was this node key revoked by its owner's roster (and why)?
pub fn key_revoked(conn: &Connection, node_fp: &str) -> Option<RevokeReason> {
    let r: String = conn.query_row("SELECT reason FROM compute_key_revocations WHERE node_fp = ?1", params![node_fp], |r| r.get(0)).optional().ok().flatten()?;
    enum_from(Some(r))
}

/// A receipt from this key is void (PRD §4.1): revoked as compromised voids
/// every receipt, past and future; any other revocation voids receipts
/// that started after it was recorded.
pub fn receipt_void(conn: &Connection, node_fp: &str, started_at: Option<i64>) -> Option<String> {
    let (reason, at): (String, i64) = conn
        .query_row("SELECT reason, at FROM compute_key_revocations WHERE node_fp = ?1", params![node_fp], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .ok()
        .flatten()?;
    match reason.as_str() {
        "compromised" => Some("the node's owner revoked this key as compromised: every receipt it signed is void".into()),
        _ if started_at.is_none_or(|s| s >= at) => Some(format!("the node's owner revoked this key ({reason}) before this job started")),
        _ => None,
    }
}

// ── the seat enrol code (client) ─────────────────────────────────────

/// A node enrolling with a seat's code (V20 with today's node path: the
/// owner's `servers.add` carries it). The code is good for the seat's own
/// nodes only: a key that is active in the owner's newest stored roster,
/// under a name the owner's seat statement lists. Returns the seat and the
/// node's roster name. Constant-time compare on the binding.
pub fn seat_code_match(conn: &Connection, binding: &str, node_fp: &str, now: i64) -> Option<(SeatRow, String)> {
    let mut hit = None;
    for s in seats(conn, Some("client")) {
        let Some(b) = s.code_binding.as_deref() else { continue };
        if crypto::ct_eq(b.as_bytes(), binding.as_bytes()) && matches!(s.state.as_str(), "requested" | "offered" | "active") && now < s.created_at + SEAT_CODE_TTL_SECS {
            hit = Some(s);
        }
    }
    let s = hit?;
    let name = seat_node_name(conn, &s, node_fp)?;
    Some((s, name))
}

/// The seat node a key belongs to: active in the owner's newest stored
/// roster under a name the seat lists, and not revoked.
pub fn seat_node_name(conn: &Connection, s: &SeatRow, node_fp: &str) -> Option<String> {
    let r = latest_roster(conn, s.owner_key_fp.as_deref()?)?;
    let e = r.roster.entry(node_fp).filter(|e| e.status == KeyStatus::Active)?;
    if key_revoked(conn, node_fp).is_some() || !s.nodes.iter().any(|n| n == &e.name) {
        return None;
    }
    Some(e.name.clone())
}

/// Tie a node row to its seat (client).
pub fn set_node_seat(conn: &Connection, node_id: &str, seat_id: Option<&str>) -> Result<(), String> {
    conn.execute("UPDATE compute_nodes SET seat_id = ?2 WHERE id = ?1", params![node_id, seat_id]).map_err(|e| e.to_string())?;
    Ok(())
}

// ── V25 ──────────────────────────────────────────────────────────────
//
// The controller's seal check for a seat node (the seat active, the key's
// roster standing, the consent, the lane's rules and the node's live
// offer) is one function: `crate::compute::lane_secrets::lane_gate`, fed
// from these tables by `lane_secrets::TrustSeats` and
// `lane_secrets::consent_for`.

// ── re-key: a pinned key hands over to a new one (V3) ────────────────

/// A node proves a new key with its old one (`Hello.rotation`): check the
/// rotation with the key pinned on `node`, the new key's statement, the
/// epoch, then move the row to the new key **in place** (same id, so grants
/// and jobs keep working). Refuses a key revoked as compromised.
pub fn apply_rotation(conn: &Connection, node: &NodeRow, rot: &KeyRotation, at: i64) -> Result<NodeRow, (&'static str, String)> {
    let refuse = |m: String| (codes::ROTATION_REFUSED, m);
    let old_spki = crypto::pem_decode("PUBLIC KEY", &node.public_key_pem).map_err(|e| refuse(format!("pinned key unreadable: {e}")))?;
    let r = RotationStatement::open(&rot.rotation, &old_spki).map_err(refuse)?;
    if r.old_fp != node.fingerprint {
        return Err(refuse("the rotation is for another key".into()));
    }
    if key_revoked(conn, &node.fingerprint) == Some(RevokeReason::Compromised) {
        return Err((codes::KEY_REVOKED, "the old key was revoked as compromised: it can't vouch for a new one; enrol the node again".into()));
    }
    let new_spki = crypto::unb64(&r.new_spki).map_err(refuse)?;
    let st = NodeStatement::open(&rot.statement).map_err(refuse)?;
    if crypto::fingerprint_of_spki_der(&new_spki) != st.node_fp {
        return Err(refuse("the new statement is not signed by the rotation's new key".into()));
    }
    r.continued_by(&st).map_err(refuse)?;
    let (epoch, _, _) = node_key_facts(conn, &node.id);
    if r.new_epoch != epoch + 1 {
        return Err(refuse(format!("rotation to epoch {} but this server pins epoch {epoch}", r.new_epoch)));
    }
    let pem = crypto::pem_encode("PUBLIC KEY", &new_spki);
    conn.execute(
        "UPDATE compute_nodes SET fingerprint = ?2, public_key_pem = ?3, key_epoch = ?4, prev_fingerprint = ?5, rekeyed_at = ?6 WHERE id = ?1 AND fingerprint = ?5",
        params![node.id, st.node_fp, pem, r.new_epoch as i64, node.fingerprint, at],
    )
    .map_err(|e| refuse(e.to_string()))?;
    save_statement(conn, &node.id, &rot.statement, at).map_err(refuse)?;
    super::store::node_by_id(conn, &node.id).ok_or_else(|| refuse("the node vanished".into()))
}

/// The owner side's roster update after a rotation: the old key retired at
/// the rotation, the new key active under the same name and facts, with
/// `rekeyed` when its statement is clean. Returns the new entry (the caller
/// files it as a held roster change: a person still approves).
pub fn rotated_entries(old: &EntryRow, st: &NodeStatement, statement_digest: &str, at: i64) -> Result<(EntryRow, EntryRow), (&'static str, String)> {
    let mut retired = old.clone();
    retired.entry.status = KeyStatus::Retired;
    retired.entry.valid_to = Some(at.max(old.entry.valid_from));
    let basis = taint_basis_for(st, false, true).ok();
    let mut new = old.clone();
    new.entry.node_fp = st.node_fp.clone();
    new.entry.node_spki = st.node_spki.clone();
    new.entry.key_epoch = st.key_epoch;
    new.entry.statement_digest = statement_digest.to_string();
    new.entry.status = KeyStatus::Active;
    new.entry.revoked_reason = None;
    new.entry.valid_from = at;
    new.entry.valid_to = None;
    new.entry.taint_basis = basis;
    new.entry.attestation_digest = None;
    new.attestation = None;
    Ok((retired, new))
}

// ── signers (V17) ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignerRow {
    pub signer_fp: String,
    #[serde(skip)]
    pub signer_spki: String,
    pub label: String,
    pub paired_by: String,
    pub paired_via: String,
    pub paired_at: i64,
    pub removed_at: Option<i64>,
}

pub fn add_signer(conn: &Connection, fp: &str, spki: &str, label: &str, by: &str, via: &str, at: i64) -> Result<(), String> {
    if !ci::is_fp(fp) {
        return Err("signer: not a 64-hex fingerprint".into());
    }
    conn.execute(
        "INSERT INTO ci_signers (signer_fp, signer_spki, label, paired_by, paired_via, paired_at, removed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL) \
         ON CONFLICT(signer_fp) DO UPDATE SET signer_spki = ?2, label = ?3, paired_by = ?4, paired_via = ?5, paired_at = ?6, removed_at = NULL",
        params![fp, spki, label, by, via, at],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn signers(conn: &Connection, include_removed: bool) -> Vec<SignerRow> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT signer_fp, signer_spki, label, paired_by, paired_via, paired_at, removed_at FROM ci_signers WHERE (?1 OR removed_at IS NULL) ORDER BY paired_at",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![include_removed], |r| {
        Ok(SignerRow {
            signer_fp: r.get(0)?,
            signer_spki: r.get(1)?,
            label: r.get(2)?,
            paired_by: r.get(3)?,
            paired_via: r.get(4)?,
            paired_at: r.get(5)?,
            removed_at: r.get(6)?,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

pub fn remove_signer(conn: &Connection, fp: &str, at: i64) -> Result<bool, String> {
    let n = conn.execute("UPDATE ci_signers SET removed_at = ?2 WHERE signer_fp = ?1 AND removed_at IS NULL", params![fp, at]).map_err(|e| e.to_string())?;
    Ok(n == 1)
}

/// The attestation (V12) for a key whose statement says `unknown`: what the
/// owner checked, by whom. The caller signs it with the roster key.
pub fn attestation_for(owner_id: &str, key: &ApprovalKey, st: &NodeStatement, statement_digest: &str, checked: &str, by: &str, at: i64) -> OwnerAttestation {
    OwnerAttestation {
        v: super::proto::node_roster::ATTESTATION_V,
        owner_id: owner_id.to_string(),
        roster_key: key.encode(),
        node_fp: st.node_fp.clone(),
        key_epoch: st.key_epoch,
        statement_digest: statement_digest.to_string(),
        installed_with_wall: st.wall == ci::Wall::JobUser,
        unwalled_jobs_seen: st.taint == Taint::Tainted,
        checked: checked.to_string(),
        approved_by: by.to_string(),
        at,
    }
}

/// Seat statement for an owner seat row (signed by the caller).
pub fn seat_statement_for(s: &SeatRow, key: &ApprovalKey, roster_seq: u64, state: ci::SeatState, at: i64) -> SeatStatement {
    let mut nodes = s.nodes.clone();
    nodes.sort();
    nodes.dedup();
    SeatStatement {
        v: super::proto::node_roster::SEAT_V,
        owner_id: s.owner_id.clone(),
        roster_key: key.encode(),
        seat_id: s.id.clone(),
        client_fp: s.client_fp.clone(),
        client_label: s.client_label.clone(),
        lane: s.lane.clone(),
        nodes,
        roster_seq,
        state,
        issued_at: at,
        pod: None,
        line: None,
    }
}

#[cfg(test)]
mod tests;
