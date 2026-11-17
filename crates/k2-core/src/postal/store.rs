//! Postal handles and friends (0311, prd-postal-in-k2-v1 §5.6): the K2
//! side of the SDK's `FriendStore`, keyed by `(local handle, peer)`.
//!
//! This is the local half (S3): a person claims a workspace's address, and
//! every friend request, incoming or outgoing, is a row that waits for a
//! person's card. Nothing here talks to the Postal network:
//! - an outgoing request is `pending_out` with no `pair_id` until the
//!   network half sends it ([`unsent_outgoing`] is what it picks up);
//! - an incoming request arrives through [`record_incoming`], which the
//!   network half calls when the plane reports one.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// The most incoming requests one handle keeps waiting (§7, spam).
pub const PENDING_IN_CAP: i64 = 20;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Handle {
    pub id: String,
    pub workspace_id: String,
    pub address: String,
    pub key_name: Option<String>,
    /// The agent's routing rule (JSON); `None` is the default (terminal, wake).
    pub route: Option<String>,
    /// `local` | `claimed` | `released`.
    pub status: String,
    pub claimed_by: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Friend {
    pub id: String,
    pub handle_id: String,
    #[serde(rename = "peer")]
    pub peer_address: String,
    pub peer_fingerprint: Option<String>,
    pub pinned_at: Option<i64>,
    pub sas_version: Option<i64>,
    pub pair_id: Option<String>,
    /// `pending_in` | `pending_out` | `accepted` | `rejected` | `revoked` |
    /// `blocked` | `rekey_pending`.
    pub status: String,
    pub route: Option<String>,
    pub owner_name: Option<String>,
    /// The asker's words (an agent's `--why`, or the peer's note).
    pub note: Option<String>,
    pub asked_by: Option<String>,
    pub decided_by: Option<String>,
    pub decided_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Why a store call didn't happen. `code` is the route's error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The workspace already has an address (`postal_has_address`).
    HasAddress(String),
    /// Another workspace holds that address (`postal_address_taken`).
    AddressTaken,
    /// The workspace has no Postal address yet (`postal_no_address`).
    NoAddress,
    /// No such friend row (`not_found`).
    NotFound,
    /// Already friends (`postal_already_friends`).
    AlreadyFriends,
    /// The peer asked first: answer their request (`postal_they_asked`).
    TheyAsked(String),
    /// The peer is blocked (`postal_blocked`).
    Blocked,
    /// The request isn't waiting for an answer (`postal_not_pending`).
    NotPending(String),
    /// Too many requests wait already (`postal_too_many_pending`).
    TooManyPending,
    /// A peer can't be this handle itself (`postal_self`).
    SelfFriend,
    Db(String),
}

impl StoreError {
    pub fn code(&self) -> &'static str {
        match self {
            StoreError::HasAddress(_) => "postal_has_address",
            StoreError::AddressTaken => "postal_address_taken",
            StoreError::NoAddress => "postal_no_address",
            StoreError::NotFound => "not_found",
            StoreError::AlreadyFriends => "postal_already_friends",
            StoreError::TheyAsked(_) => "postal_they_asked",
            StoreError::Blocked => "postal_blocked",
            StoreError::NotPending(_) => "postal_not_pending",
            StoreError::TooManyPending => "postal_too_many_pending",
            StoreError::SelfFriend => "postal_self",
            StoreError::Db(_) => "internal",
        }
    }

    pub fn hint(&self) -> String {
        match self {
            StoreError::HasAddress(a) => format!("this workspace's Postal address is {a} already (one address per workspace)"),
            StoreError::AddressTaken => "another workspace on this server holds that address".into(),
            StoreError::NoAddress => "this workspace has no Postal address yet: a person claims one in its Postal tab".into(),
            StoreError::NotFound => "no such Postal friend or request for this workspace".into(),
            StoreError::AlreadyFriends => "you are friends already".into(),
            StoreError::TheyAsked(id) => format!("they asked to be friends first: a person answers request {id} in the Postal tab"),
            StoreError::Blocked => "that address is blocked for this workspace: a person removes the block in the Postal tab first".into(),
            StoreError::NotPending(s) => format!("this request isn't waiting for an answer (it is {s})"),
            StoreError::TooManyPending => format!("{PENDING_IN_CAP} friend requests wait for this workspace already"),
            StoreError::SelfFriend => "that is this workspace's own address".into(),
            StoreError::Db(e) => e.clone(),
        }
    }
}

fn db(e: rusqlite::Error) -> StoreError {
    StoreError::Db(format!("postal store: {e}"))
}

const HANDLE_COLS: &str = "id, workspace_id, address, key_name, route, status, claimed_by, created_at, updated_at";
const FRIEND_COLS: &str = "id, handle_id, peer_address, peer_fingerprint, pinned_at, sas_version, pair_id, status, route, \
     owner_name, note, asked_by, decided_by, decided_at, created_at, updated_at";

fn handle_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Handle> {
    Ok(Handle {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        address: r.get(2)?,
        key_name: r.get(3)?,
        route: r.get(4)?,
        status: r.get(5)?,
        claimed_by: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

fn friend_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Friend> {
    Ok(Friend {
        id: r.get(0)?,
        handle_id: r.get(1)?,
        peer_address: r.get(2)?,
        peer_fingerprint: r.get(3)?,
        pinned_at: r.get(4)?,
        sas_version: r.get(5)?,
        pair_id: r.get(6)?,
        status: r.get(7)?,
        route: r.get(8)?,
        owner_name: r.get(9)?,
        note: r.get(10)?,
        asked_by: r.get(11)?,
        decided_by: r.get(12)?,
        decided_at: r.get(13)?,
        created_at: r.get(14)?,
        updated_at: r.get(15)?,
    })
}

/// The workspace's live Postal address (not released), if any.
pub fn handle_for_workspace(conn: &Connection, workspace_id: &str) -> Option<Handle> {
    conn.query_row(
        &format!("SELECT {HANDLE_COLS} FROM postal_handles WHERE workspace_id = ?1 AND status != 'released'"),
        params![workspace_id],
        handle_row,
    )
    .optional()
    .ok()
    .flatten()
}

/// Every live handle on this server, oldest first.
pub fn handles(conn: &Connection) -> Vec<Handle> {
    let Ok(mut st) = conn.prepare(&format!("SELECT {HANDLE_COLS} FROM postal_handles WHERE status != 'released' ORDER BY created_at, id")) else {
        return Vec::new();
    };
    st.query_map([], handle_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// Claim `address` (a parsed Postal address) for the workspace, locally:
/// `status = local` until the network half confirms it.
pub fn claim(conn: &Connection, workspace_id: &str, address: &super::Address, by: &str) -> Result<Handle, StoreError> {
    let address = address.as_string();
    if let Some(h) = handle_for_workspace(conn, workspace_id) {
        return if h.address == address { Ok(h) } else { Err(StoreError::HasAddress(h.address)) };
    }
    let taken: Option<String> = conn
        .query_row("SELECT workspace_id FROM postal_handles WHERE address = ?1", params![address], |r| r.get(0))
        .optional()
        .map_err(db)?;
    match taken {
        Some(ws) if ws != workspace_id => return Err(StoreError::AddressTaken),
        // This workspace's own released address: take it back.
        Some(_) => {
            let t = now();
            conn.execute(
                "UPDATE postal_handles SET status = 'local', claimed_by = ?2, updated_at = ?3 WHERE address = ?1",
                params![address, by, t],
            )
            .map_err(db)?;
        }
        None => {
            let t = now();
            conn.execute(
                "INSERT INTO postal_handles (id, workspace_id, address, status, claimed_by, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, 'local', ?4, ?5, ?5)",
                params![uuid::Uuid::new_v4().to_string(), workspace_id, address, by, t],
            )
            .map_err(db)?;
        }
    }
    handle_for_workspace(conn, workspace_id).ok_or(StoreError::NoAddress)
}

/// Every friend row of a handle (all statuses), newest change first.
pub fn friends(conn: &Connection, handle_id: &str) -> Vec<Friend> {
    let Ok(mut st) = conn.prepare(&format!("SELECT {FRIEND_COLS} FROM postal_friends WHERE handle_id = ?1 ORDER BY updated_at DESC, id")) else {
        return Vec::new();
    };
    st.query_map(params![handle_id], friend_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

pub fn friend(conn: &Connection, id: &str) -> Option<Friend> {
    conn.query_row(&format!("SELECT {FRIEND_COLS} FROM postal_friends WHERE id = ?1"), params![id], friend_row).optional().ok().flatten()
}

pub fn friend_by_peer(conn: &Connection, handle_id: &str, peer: &str) -> Option<Friend> {
    conn.query_row(
        &format!("SELECT {FRIEND_COLS} FROM postal_friends WHERE handle_id = ?1 AND peer_address = ?2"),
        params![handle_id, peer],
        friend_row,
    )
    .optional()
    .ok()
    .flatten()
}

/// `(accepted, pending_in, pending_out)` for a handle.
pub fn counts(conn: &Connection, handle_id: &str) -> (i64, i64, i64) {
    let n = |s: &str| -> i64 {
        conn.query_row("SELECT COUNT(*) FROM postal_friends WHERE handle_id = ?1 AND status = ?2", params![handle_id, s], |r| r.get(0))
            .unwrap_or(0)
    };
    (n("accepted"), n("pending_in"), n("pending_out"))
}

fn set_status(conn: &Connection, id: &str, status: &str, by: Option<&str>) -> Result<(), StoreError> {
    let t = now();
    conn.execute(
        "UPDATE postal_friends SET status = ?2, decided_by = COALESCE(?3, decided_by), decided_at = CASE WHEN ?3 IS NULL THEN decided_at ELSE ?4 END, updated_at = ?4 WHERE id = ?1",
        params![id, status, by, t],
    )
    .map_err(db)?;
    Ok(())
}

fn insert_friend(conn: &Connection, handle_id: &str, peer: &str, status: &str, note: Option<&str>, asked_by: Option<&str>) -> Result<String, StoreError> {
    let id = uuid::Uuid::new_v4().to_string();
    let t = now();
    conn.execute(
        "INSERT INTO postal_friends (id, handle_id, peer_address, status, note, asked_by, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![id, handle_id, peer, status, note, asked_by, t],
    )
    .map_err(db)?;
    Ok(id)
}

/// A person allowed an outgoing friend request (`postal.invite`, or the
/// tab): `pending_out`, unsent until the network half sends it. Asking
/// again while one waits is the same request.
pub fn record_outgoing(conn: &Connection, handle: &Handle, peer: &super::Address, note: Option<&str>, asked_by: &str) -> Result<Friend, StoreError> {
    let peer = peer.as_string();
    if peer == handle.address {
        return Err(StoreError::SelfFriend);
    }
    let id = match friend_by_peer(conn, &handle.id, &peer) {
        Some(f) => match f.status.as_str() {
            "accepted" | "rekey_pending" => return Err(StoreError::AlreadyFriends),
            "blocked" => return Err(StoreError::Blocked),
            "pending_in" => return Err(StoreError::TheyAsked(f.id)),
            "pending_out" => return Ok(f),
            _ => {
                let t = now();
                conn.execute(
                    "UPDATE postal_friends SET status = 'pending_out', pair_id = NULL, note = ?2, asked_by = ?3, decided_by = NULL, decided_at = NULL, updated_at = ?4 WHERE id = ?1",
                    params![f.id, note, asked_by, t],
                )
                .map_err(db)?;
                f.id
            }
        },
        None => insert_friend(conn, &handle.id, &peer, "pending_out", note, Some(asked_by))?,
    };
    friend(conn, &id).ok_or(StoreError::NotFound)
}

/// The outgoing requests the network half hasn't sent yet (no `pair_id`).
pub fn unsent_outgoing(conn: &Connection) -> Vec<Friend> {
    let Ok(mut st) = conn.prepare(&format!("SELECT {FRIEND_COLS} FROM postal_friends WHERE status = 'pending_out' AND pair_id IS NULL ORDER BY created_at, id")) else {
        return Vec::new();
    };
    st.query_map([], friend_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// What an incoming request did to the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    /// A new request waits for a person: present its card.
    New(Friend),
    /// The same request was recorded already (its card is up or answered).
    Known(Friend),
}

/// An incoming friend request (the seam the network half calls; also used
/// when both sides asked: a waiting outgoing request becomes incoming, for
/// a person to accept). Blocked peers are refused; a full queue is refused.
pub fn record_incoming(
    conn: &Connection,
    handle: &Handle,
    peer: &super::Address,
    owner_name: Option<&str>,
    fingerprint: Option<&str>,
    pair_id: Option<&str>,
    note: Option<&str>,
) -> Result<Incoming, StoreError> {
    let peer = peer.as_string();
    if peer == handle.address {
        return Err(StoreError::SelfFriend);
    }
    let existing = friend_by_peer(conn, &handle.id, &peer);
    if let Some(f) = &existing {
        match f.status.as_str() {
            "blocked" => return Err(StoreError::Blocked),
            "accepted" | "rekey_pending" | "pending_in" => return Ok(Incoming::Known(f.clone())),
            _ => {}
        }
    }
    let waiting: i64 = conn
        .query_row("SELECT COUNT(*) FROM postal_friends WHERE handle_id = ?1 AND status = 'pending_in'", params![handle.id], |r| r.get(0))
        .map_err(db)?;
    if waiting >= PENDING_IN_CAP {
        return Err(StoreError::TooManyPending);
    }
    let id = match existing {
        Some(f) => f.id,
        None => insert_friend(conn, &handle.id, &peer, "pending_in", note, None)?,
    };
    let t = now();
    conn.execute(
        "UPDATE postal_friends SET status = 'pending_in', owner_name = ?2, peer_fingerprint = ?3, pair_id = COALESCE(?4, pair_id), \
         note = COALESCE(?5, note), decided_by = NULL, decided_at = NULL, updated_at = ?6 WHERE id = ?1",
        params![id, owner_name, fingerprint, pair_id, note, t],
    )
    .map_err(db)?;
    friend(conn, &id).map(Incoming::New).ok_or(StoreError::NotFound)
}

/// A person's answer to an incoming request (or a changed key): accepted
/// pins the key the request carried; rejected keeps the row as history.
pub fn decide(conn: &Connection, id: &str, accept: bool, by: &str) -> Result<Friend, StoreError> {
    let f = friend(conn, id).ok_or(StoreError::NotFound)?;
    if !matches!(f.status.as_str(), "pending_in" | "rekey_pending") {
        return Err(StoreError::NotPending(f.status));
    }
    if accept {
        let t = now();
        conn.execute(
            "UPDATE postal_friends SET status = 'accepted', pinned_at = CASE WHEN peer_fingerprint IS NULL THEN pinned_at ELSE ?2 END, \
             decided_by = ?3, decided_at = ?2, updated_at = ?2 WHERE id = ?1",
            params![id, t, by],
        )
        .map_err(db)?;
    } else {
        set_status(conn, id, "rejected", Some(by))?;
    }
    friend(conn, id).ok_or(StoreError::NotFound)
}

/// Remove a friend or withdraw a request (`revoked`), kept as history.
pub fn remove(conn: &Connection, handle_id: &str, peer: &str, by: &str) -> Result<Friend, StoreError> {
    let f = friend_by_peer(conn, handle_id, peer).ok_or(StoreError::NotFound)?;
    set_status(conn, &f.id, "revoked", Some(by))?;
    friend(conn, &f.id).ok_or(StoreError::NotFound)
}

/// Block an address for this handle (a row is made when there is none):
/// its requests are refused from now on.
pub fn block(conn: &Connection, handle: &Handle, peer: &super::Address, by: &str) -> Result<Friend, StoreError> {
    let peer = peer.as_string();
    if peer == handle.address {
        return Err(StoreError::SelfFriend);
    }
    let id = match friend_by_peer(conn, &handle.id, &peer) {
        Some(f) => f.id,
        None => insert_friend(conn, &handle.id, &peer, "blocked", None, None)?,
    };
    set_status(conn, &id, "blocked", Some(by))?;
    friend(conn, &id).ok_or(StoreError::NotFound)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
