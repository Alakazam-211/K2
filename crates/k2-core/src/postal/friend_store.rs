//! postal-sdk's `FriendStore` on `postal_friends` (0311), keyed by `(local
//! handle, peer)` the way the SDK keys it: a friend of one handle is not a
//! friend of another handle on the same box.
//!
//! The store never makes a friend: only a person's card does
//! (`postal.pair.accept`). So:
//! - `get` answers `trusted` only for an `accepted` row; a `rekey_pending`
//!   row is untrusted (its mail is held) with `rekey_pending: None`, since
//!   K2 doesn't keep the new key (the SDK does, until a person accepts);
//! - `put` pins the key (fingerprint, PEM) on a row a person accepted, and
//!   records a changed key (`rekey_pending`) on a friend; it refuses a peer
//!   no person here accepted (`no_friend`) and never changes a status
//!   otherwise;
//! - `remove` revokes (kept as history).
//!
//! The daemon's pairing pins friends through [`super::store::pin_accepted`]
//! (only after both signed accepts verify); this is the SDK-shaped view of
//! the same rows.

use postal_sdk::host::{Friend, FriendStore, StoreError};
use rusqlite::Connection;

use super::store;

/// The SDK's friend for `(local_handle, peer)`.
pub fn get_on(conn: &Connection, local_handle: &str, peer: &str) -> Option<Friend> {
    let h = store::handle_by_address(conn, local_handle)?;
    let f = store::friend_by_peer(conn, &h.id, peer)?;
    if !matches!(f.status.as_str(), "accepted" | "rekey_pending") {
        return None;
    }
    Some(Friend {
        local_handle: h.address,
        peer: f.peer_address.clone(),
        pinned_fingerprint: f.peer_fingerprint.clone().unwrap_or_default(),
        pinned_public_key_pem: store::peer_key_pem(conn, &f.id).unwrap_or_default(),
        rekey_pending: None,
        trusted: f.status == "accepted",
    })
}

/// Pin or note a changed key; see the module doc.
pub fn put_on(conn: &Connection, friend: &Friend) -> Result<(), StoreError> {
    let no_friend = || StoreError(format!("no_friend: {} isn't a friend of {} (a person accepts friends here)", friend.peer, friend.local_handle));
    let h = store::handle_by_address(conn, &friend.local_handle).ok_or_else(no_friend)?;
    let f = store::friend_by_peer(conn, &h.id, &friend.peer).ok_or_else(no_friend)?;
    if !matches!(f.status.as_str(), "accepted" | "rekey_pending") {
        return Err(no_friend());
    }
    if friend.rekey_pending.is_some() {
        return store::mark_rekey(conn, &h.id, &friend.peer).map(|_| ()).map_err(|e| StoreError(e.code().to_string()));
    }
    if f.status == "accepted" && !friend.pinned_fingerprint.is_empty() {
        store::pin_key(conn, &f.id, &friend.pinned_fingerprint, &friend.pinned_public_key_pem).map_err(|e| StoreError(e.hint()))?;
    }
    Ok(())
}

/// Revoke `(local_handle, peer)`; an unknown pair is already gone.
pub fn remove_on(conn: &Connection, local_handle: &str, peer: &str) -> Result<(), StoreError> {
    let Some(h) = store::handle_by_address(conn, local_handle) else { return Ok(()) };
    match store::remove(conn, &h.id, peer, "postal-sdk") {
        Ok(_) | Err(store::StoreError::NotFound) => Ok(()),
        Err(e) => Err(StoreError(e.hint())),
    }
}

/// K2's `FriendStore`: the server's database.
pub struct K2FriendStore;

impl FriendStore for K2FriendStore {
    fn get(&self, local_handle: &str, peer: &str) -> Option<Friend> {
        crate::db::read(|conn| get_on(conn, local_handle, peer))
    }

    fn put(&self, friend: Friend) -> Result<(), StoreError> {
        let db = crate::db::shared();
        let conn = db.lock();
        put_on(&conn, &friend)
    }

    fn remove(&self, local_handle: &str, peer: &str) -> Result<(), StoreError> {
        let db = crate::db::shared();
        let conn = db.lock();
        remove_on(&conn, local_handle, peer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::postal::parse_address;
    use crate::postal::store::{claim, decide, record_incoming, Incoming};

    fn conn() -> Connection {
        let c = Connection::open_in_memory().expect("memory");
        crate::db::run_migrations(&c).expect("migrate");
        c
    }

    #[test]
    fn only_a_friend_a_person_accepted_is_trusted_and_pinned() {
        let c = conn();
        let h = claim(&c, "ws-1", &parse_address("scout::acme.postal.bot").unwrap(), "owner").unwrap();
        let peer = parse_address("ops::beta.postal.bot").unwrap();
        let sdk = |trusted: bool, rekey: Option<&str>| Friend {
            local_handle: "scout::acme.postal.bot".into(),
            peer: "ops::beta.postal.bot".into(),
            pinned_fingerprint: "ab".repeat(32),
            pinned_public_key_pem: "-----BEGIN PUBLIC KEY-----\nx\n-----END PUBLIC KEY-----\n".into(),
            rekey_pending: rekey.map(str::to_string),
            trusted,
        };
        // A stranger, then a waiting request: no friend, and the SDK can't make one.
        assert_eq!(get_on(&c, "scout::acme.postal.bot", "ops::beta.postal.bot"), None);
        assert!(put_on(&c, &sdk(true, None)).unwrap_err().0.starts_with("no_friend"));
        let f = match record_incoming(&c, &h, &peer, None, None, Some("pr_1"), None).unwrap() {
            Incoming::New(f) => f,
            other => panic!("{other:?}"),
        };
        assert_eq!(get_on(&c, "scout::acme.postal.bot", "ops::beta.postal.bot"), None, "a request isn't a friend");
        assert!(put_on(&c, &sdk(true, None)).is_err());
        assert_eq!(store::friend(&c, &f.id).unwrap().status, "pending_in", "the SDK never accepts");
        // A person accepts: the SDK pins the key, and gets it back trusted.
        decide(&c, &f.id, true, "owner").unwrap();
        put_on(&c, &sdk(true, None)).unwrap();
        let got = get_on(&c, "scout::acme.postal.bot", "ops::beta.postal.bot").unwrap();
        assert!(got.trusted && got.pinned_fingerprint == "ab".repeat(32) && got.pinned_public_key_pem.contains("PUBLIC KEY"), "{got:?}");
        // Another handle on this box: not its friend.
        claim(&c, "ws-2", &parse_address("other::acme.postal.bot").unwrap(), "owner").unwrap();
        assert_eq!(get_on(&c, "other::acme.postal.bot", "ops::beta.postal.bot"), None);
        // A changed key: untrusted until a person accepts it.
        put_on(&c, &sdk(false, Some(&"cd".repeat(32)))).unwrap();
        let got = get_on(&c, "scout::acme.postal.bot", "ops::beta.postal.bot").unwrap();
        assert!(!got.trusted && got.pinned_fingerprint == "ab".repeat(32), "the old pin stays: {got:?}");
        // Removed: gone for the SDK, kept as history.
        remove_on(&c, "scout::acme.postal.bot", "ops::beta.postal.bot").unwrap();
        assert_eq!(get_on(&c, "scout::acme.postal.bot", "ops::beta.postal.bot"), None);
        assert_eq!(store::friend(&c, &f.id).unwrap().status, "revoked");
        remove_on(&c, "nobody::acme.postal.bot", "ops::beta.postal.bot").unwrap();
    }
}
