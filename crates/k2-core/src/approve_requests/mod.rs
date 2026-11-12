//! Signed approvals, the daemon's store (prd-signed-approvals-v1 rev 2, S1;
//! migration 0303).
//!
//! The daemon carries a relying party's payload bytes to a person's device
//! and the device's approval record back. It never re-writes the bytes and
//! never decides whether an approval is valid: the relying party verifies
//! the signature against keys it pinned itself (PRD §2, §7). What lives
//! here is the carrier's own state:
//!
//! - [`ApprovalRequest`]: one ask, `waiting` until the person answers, the
//!   agent cancels or `answer_by` passes.
//! - the `approve.request` permission ([`agent_may_request`]): off until an
//!   Owner or Admin turns it on for a workspace.
//! - who an agent may ask ([`may_ask`], Rosson's question 3): by default
//!   only the person it works for; anyone else allows it for themselves,
//!   and anyone can block an agent from asking them.
//! - limits ([`limits`]): a few waiting requests per person and per agent,
//!   and asks per hour, so nobody is worn down by prompts (SA-16).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests;

/// Request id prefix (`ap_` + 16 lowercase hex).
pub const ID_PREFIX: &str = "ap_";

/// The largest payload the daemon carries (PRD §5).
pub const MAX_PAYLOAD: usize = 16 * 1024;

/// The longest `answer_by` the daemon accepts, from the time of the ask:
/// approvals are answered in minutes, not hours (PRD §5).
pub const MAX_ANSWER_WINDOW_SECS: i64 = 3600;

pub mod limits {
    /// Waiting requests one person may have at once (SA-16).
    pub const WAITING_PER_PERSON: usize = 5;
    /// Waiting requests one agent (workspace) may have at once.
    pub const WAITING_PER_AGENT: usize = 3;
    /// Asks one agent may make in an hour.
    pub const ASKS_PER_AGENT_PER_HOUR: usize = 20;
    /// Asks one person may receive in an hour.
    pub const ASKS_PER_PERSON_PER_HOUR: usize = 30;
}

/// Refusal codes the routes return.
pub mod codes {
    pub const DUPLICATE_NONCE: &str = "approval_duplicate_nonce";
    pub const TOO_MANY_WAITING: &str = "approval_too_many_waiting";
    pub const RATE_LIMITED: &str = "approval_rate_limited";
    pub const NOT_FOUND: &str = "approval_not_found";
    pub const NOT_WAITING: &str = "approval_not_waiting";
    pub const EXPIRED: &str = "approval_expired";
    pub const DB: &str = "approval_store";
}

/// A store refusal: a code from [`codes`] and plain words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError {
    pub code: &'static str,
    pub message: String,
}

impl StoreError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        StoreError { code, message: message.into() }
    }
    fn db(e: impl std::fmt::Display) -> Self {
        StoreError::new(codes::DB, e.to_string())
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// Where a request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Waiting,
    Approved,
    Declined,
    Expired,
    Cancelled,
}

impl State {
    pub const ALL: [State; 5] = [State::Waiting, State::Approved, State::Declined, State::Expired, State::Cancelled];

    pub fn as_str(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Approved => "approved",
            State::Declined => "declined",
            State::Expired => "expired",
            State::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<State> {
        State::ALL.into_iter().find(|st| st.as_str() == s)
    }

    pub fn is_final(self) -> bool {
        self != State::Waiting
    }
}

/// One `approval_requests` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub id: String,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub person_id: String,
    pub person_login: String,
    /// The bytes, exactly as the agent sent them.
    pub payload: Vec<u8>,
    pub payload_sha256: String,
    pub kind: String,
    pub rp: String,
    pub nonce: String,
    pub answer_by: i64,
    pub state: State,
    pub conversation_id: Option<String>,
    pub card_id: Option<String>,
    /// The approval record exactly as the device posted it.
    pub record: Option<String>,
    pub key_id: Option<String>,
    pub alg: Option<String>,
    pub device: Option<String>,
    pub answered_by: Option<String>,
    pub answered_via: Option<String>,
    pub created_at: i64,
    pub answered_at: Option<i64>,
}

/// What an ask stores. The shape fields (`kind`, `rp`, `nonce`,
/// `answer_by`) come from the caller's shape check of `payload`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRequest {
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub person_id: String,
    pub person_login: String,
    pub payload: Vec<u8>,
    pub kind: String,
    pub rp: String,
    pub nonce: String,
    pub answer_by: i64,
}

/// The person's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The approval record as posted (kept byte for byte), with the two
    /// fields read from it for lists.
    Approve { record: String, key_id: String, alg: String },
    /// Unsigned: a decline only stops things.
    Decline,
}

/// Who answered and from where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    /// The login that answered.
    pub by: String,
    /// `phone`, `desktop` or `web`.
    pub via: String,
    /// The device's own name for itself (display only).
    pub device: Option<String>,
}

/// An ask rule (question 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    Allow,
    Block,
}

impl Rule {
    pub fn as_str(self) -> &'static str {
        match self {
            Rule::Allow => "allow",
            Rule::Block => "block",
        }
    }
}

/// A fresh request id.
pub fn new_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{ID_PREFIX}{}", &hex[..16])
}

/// `ap_` + exactly 16 lowercase hex.
pub fn valid_id(s: &str) -> bool {
    s.strip_prefix(ID_PREFIX)
        .is_some_and(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

const COLS: &str = "id, workspace_id, session_id, person_id, person_login, payload, payload_sha256, kind, rp, nonce, \
     answer_by, state, conversation_id, card_id, record, key_id, alg, device, answered_by, answered_via, created_at, answered_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ApprovalRequest> {
    let state: String = r.get(11)?;
    Ok(ApprovalRequest {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        session_id: r.get(2)?,
        person_id: r.get(3)?,
        person_login: r.get(4)?,
        payload: r.get(5)?,
        payload_sha256: r.get(6)?,
        kind: r.get(7)?,
        rp: r.get(8)?,
        nonce: r.get(9)?,
        answer_by: r.get(10)?,
        state: State::parse(&state).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, format!("unknown approval state {state}").into())
        })?,
        conversation_id: r.get(12)?,
        card_id: r.get(13)?,
        record: r.get(14)?,
        key_id: r.get(15)?,
        alg: r.get(16)?,
        device: r.get(17)?,
        answered_by: r.get(18)?,
        answered_via: r.get(19)?,
        created_at: r.get(20)?,
        answered_at: r.get(21)?,
    })
}

fn count(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<usize, StoreError> {
    let n: i64 = conn.query_row(sql, p, |r| r.get(0)).map_err(StoreError::db)?;
    Ok(n.max(0) as usize)
}

/// Store a new `waiting` request after the limits and the one-nonce rule.
/// The caller has already checked the payload's shape, the permission and
/// who may be asked.
pub fn insert(conn: &Connection, n: &NewRequest, now: i64) -> Result<ApprovalRequest, StoreError> {
    let tx = conn.unchecked_transaction().map_err(StoreError::db)?;
    let dup = count(&tx, "SELECT COUNT(*) FROM approval_requests WHERE rp = ?1 AND nonce = ?2", params![n.rp, n.nonce])?;
    if dup > 0 {
        return Err(StoreError::new(codes::DUPLICATE_NONCE, "this relying party's nonce was already sent for approval; make a new payload"));
    }
    let waiting_agent = count(
        &tx,
        "SELECT COUNT(*) FROM approval_requests WHERE workspace_id = ?1 AND state = 'waiting' AND answer_by > ?2",
        params![n.workspace_id, now],
    )?;
    if waiting_agent >= limits::WAITING_PER_AGENT {
        return Err(StoreError::new(
            codes::TOO_MANY_WAITING,
            format!("you already have {} approvals waiting; wait for an answer or cancel one", limits::WAITING_PER_AGENT),
        ));
    }
    let waiting_person = count(
        &tx,
        "SELECT COUNT(*) FROM approval_requests WHERE person_id = ?1 AND state = 'waiting' AND answer_by > ?2",
        params![n.person_id, now],
    )?;
    if waiting_person >= limits::WAITING_PER_PERSON {
        return Err(StoreError::new(
            codes::TOO_MANY_WAITING,
            format!("{} already has {} approvals waiting; try again after they answer", n.person_login, limits::WAITING_PER_PERSON),
        ));
    }
    let hour = now - 3600;
    let agent_hour = count(&tx, "SELECT COUNT(*) FROM approval_requests WHERE workspace_id = ?1 AND created_at > ?2", params![n.workspace_id, hour])?;
    let person_hour = count(&tx, "SELECT COUNT(*) FROM approval_requests WHERE person_id = ?1 AND created_at > ?2", params![n.person_id, hour])?;
    if agent_hour >= limits::ASKS_PER_AGENT_PER_HOUR || person_hour >= limits::ASKS_PER_PERSON_PER_HOUR {
        return Err(StoreError::new(codes::RATE_LIMITED, "too many approval requests in the last hour; try again later"));
    }
    let id = new_id();
    tx.execute(
        "INSERT INTO approval_requests (id, workspace_id, session_id, person_id, person_login, payload, payload_sha256, kind, rp, nonce, answer_by, state, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'waiting', ?12)",
        params![
            id,
            n.workspace_id,
            n.session_id,
            n.person_id,
            n.person_login,
            n.payload,
            sha256_hex(&n.payload),
            n.kind,
            n.rp,
            n.nonce,
            n.answer_by,
            now
        ],
    )
    .map_err(StoreError::db)?;
    tx.commit().map_err(StoreError::db)?;
    get(conn, &id)?.ok_or_else(|| StoreError::new(codes::DB, "the new request did not read back"))
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<ApprovalRequest>, StoreError> {
    conn.query_row(&format!("SELECT {COLS} FROM approval_requests WHERE id = ?1"), params![id], row)
        .optional()
        .map_err(StoreError::db)
}

/// The card this request's Thread shows.
pub fn set_card(conn: &Connection, id: &str, conversation_id: &str, card_id: &str) -> Result<(), StoreError> {
    conn.execute("UPDATE approval_requests SET conversation_id = ?2, card_id = ?3 WHERE id = ?1", params![id, conversation_id, card_id])
        .map_err(StoreError::db)?;
    Ok(())
}

/// A workspace's requests, newest first.
pub fn for_workspace(conn: &Connection, workspace_id: &str, limit: usize) -> Result<Vec<ApprovalRequest>, StoreError> {
    let mut st = conn
        .prepare(&format!("SELECT {COLS} FROM approval_requests WHERE workspace_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2"))
        .map_err(StoreError::db)?;
    let rows = st.query_map(params![workspace_id, limit as i64], row).map_err(StoreError::db)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(StoreError::db)
}

/// A person's requests still open at `now`, oldest first.
pub fn waiting_for_person(conn: &Connection, person_id: &str, now: i64) -> Result<Vec<ApprovalRequest>, StoreError> {
    let mut st = conn
        .prepare(&format!(
            "SELECT {COLS} FROM approval_requests WHERE person_id = ?1 AND state = 'waiting' AND answer_by > ?2 ORDER BY created_at, rowid"
        ))
        .map_err(StoreError::db)?;
    let rows = st.query_map(params![person_id, now], row).map_err(StoreError::db)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(StoreError::db)
}

/// Turn one waiting request final (compare-and-set on `waiting`).
fn finish(conn: &Connection, id: &str, to: State, set: &str, p: &[&dyn rusqlite::ToSql]) -> Result<bool, StoreError> {
    let sql = format!("UPDATE approval_requests SET state = '{}'{set} WHERE id = ?1 AND state = 'waiting'", to.as_str());
    let mut all: Vec<&dyn rusqlite::ToSql> = vec![&id];
    all.extend_from_slice(p);
    Ok(conn.execute(&sql, all.as_slice()).map_err(StoreError::db)? == 1)
}

fn not_waiting(r: &ApprovalRequest) -> StoreError {
    StoreError::new(codes::NOT_WAITING, format!("approval {} is {}, not waiting", r.id, r.state.as_str()))
}

/// The person's answer. Refused once the request is final; a request past
/// its `answer_by` turns `expired` and the answer is refused.
pub fn answer(conn: &Connection, id: &str, a: &Answer, who: &Answered, now: i64) -> Result<ApprovalRequest, StoreError> {
    let r = get(conn, id)?.ok_or_else(|| StoreError::new(codes::NOT_FOUND, format!("no approval {id}")))?;
    if r.state.is_final() {
        return Err(not_waiting(&r));
    }
    if now >= r.answer_by {
        finish(conn, id, State::Expired, "", &[])?;
        return Err(StoreError::new(codes::EXPIRED, format!("approval {id} expired before it was answered")));
    }
    let done = match a {
        Answer::Approve { record, key_id, alg } => finish(
            conn,
            id,
            State::Approved,
            ", record = ?2, key_id = ?3, alg = ?4, device = ?5, answered_by = ?6, answered_via = ?7, answered_at = ?8",
            &[record, key_id, alg, &who.device, &who.by, &who.via, &now],
        )?,
        Answer::Decline => finish(
            conn,
            id,
            State::Declined,
            ", device = ?2, answered_by = ?3, answered_via = ?4, answered_at = ?5",
            &[&who.device, &who.by, &who.via, &now],
        )?,
    };
    let after = get(conn, id)?.ok_or_else(|| StoreError::new(codes::NOT_FOUND, format!("no approval {id}")))?;
    if !done {
        return Err(not_waiting(&after));
    }
    Ok(after)
}

/// The asking workspace withdraws its request. Another workspace's id is
/// "not found", never cancelled.
pub fn cancel(conn: &Connection, id: &str, workspace_id: &str, now: i64) -> Result<ApprovalRequest, StoreError> {
    let r = get(conn, id)?
        .filter(|r| r.workspace_id == workspace_id)
        .ok_or_else(|| StoreError::new(codes::NOT_FOUND, format!("no approval {id} from this workspace")))?;
    if !finish(conn, id, State::Cancelled, ", answered_at = ?2", &[&now])? {
        return Err(not_waiting(&get(conn, id)?.unwrap_or(r)));
    }
    get(conn, id)?.ok_or_else(|| StoreError::new(codes::NOT_FOUND, format!("no approval {id}")))
}

/// Every waiting request whose `answer_by` has passed turns `expired`.
/// Returns them (now expired).
pub fn expire_due(conn: &Connection, now: i64) -> Result<Vec<ApprovalRequest>, StoreError> {
    let ids: Vec<String> = {
        let mut st = conn
            .prepare("SELECT id FROM approval_requests WHERE state = 'waiting' AND answer_by <= ?1")
            .map_err(StoreError::db)?;
        let rows = st.query_map(params![now], |r| r.get::<_, String>(0)).map_err(StoreError::db)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(StoreError::db)?
    };
    let mut out = Vec::new();
    for id in ids {
        if finish(conn, &id, State::Expired, "", &[])? {
            if let Some(r) = get(conn, &id)? {
                out.push(r);
            }
        }
    }
    Ok(out)
}

// ── the `approve.request` permission ────────────────────────────────

/// The workspace may ask people for signed approvals.
pub fn agent_may_request(conn: &Connection, workspace_id: &str) -> Result<bool, StoreError> {
    Ok(count(conn, "SELECT COUNT(*) FROM approval_agents WHERE workspace_id = ?1", params![workspace_id])? > 0)
}

/// Turn the permission on or off for a workspace.
pub fn set_agent(conn: &Connection, workspace_id: &str, on: bool, by: &str, now: i64) -> Result<(), StoreError> {
    if on {
        conn.execute(
            "INSERT INTO approval_agents (workspace_id, granted_by, granted_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT(workspace_id) DO UPDATE SET granted_by = excluded.granted_by, granted_at = excluded.granted_at",
            params![workspace_id, by, now],
        )
    } else {
        conn.execute("DELETE FROM approval_agents WHERE workspace_id = ?1", params![workspace_id])
    }
    .map_err(StoreError::db)?;
    Ok(())
}

/// Every workspace holding the permission.
pub fn agents(conn: &Connection) -> Result<Vec<(String, String, i64)>, StoreError> {
    let mut st = conn.prepare("SELECT workspace_id, granted_by, granted_at FROM approval_agents ORDER BY workspace_id").map_err(StoreError::db)?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(StoreError::db)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(StoreError::db)
}

// ── who an agent may ask (question 3) ───────────────────────────────

pub fn rule_for(conn: &Connection, person_id: &str, workspace_id: &str) -> Result<Option<Rule>, StoreError> {
    let r: Option<String> = conn
        .query_row(
            "SELECT rule FROM approval_ask_rules WHERE person_id = ?1 AND workspace_id = ?2",
            params![person_id, workspace_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(StoreError::db)?;
    Ok(match r.as_deref() {
        Some("allow") => Some(Rule::Allow),
        Some("block") => Some(Rule::Block),
        _ => None,
    })
}

/// Set (or clear, with `None`) a person's rule for one agent.
pub fn set_rule(conn: &Connection, person_id: &str, workspace_id: &str, rule: Option<Rule>, now: i64) -> Result<(), StoreError> {
    match rule {
        Some(r) => conn.execute(
            "INSERT INTO approval_ask_rules (person_id, workspace_id, rule, set_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(person_id, workspace_id) DO UPDATE SET rule = excluded.rule, set_at = excluded.set_at",
            params![person_id, workspace_id, r.as_str(), now],
        ),
        None => conn.execute("DELETE FROM approval_ask_rules WHERE person_id = ?1 AND workspace_id = ?2", params![person_id, workspace_id]),
    }
    .map_err(StoreError::db)?;
    Ok(())
}

/// May this agent ask this person? A block always wins; an allow lets it;
/// with no rule, only the person it works for (`works_for`).
pub fn may_ask(conn: &Connection, person_id: &str, workspace_id: &str, works_for: bool) -> Result<bool, StoreError> {
    Ok(match rule_for(conn, person_id, workspace_id)? {
        Some(Rule::Block) => false,
        Some(Rule::Allow) => true,
        None => works_for,
    })
}
