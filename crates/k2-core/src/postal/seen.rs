//! K2's `SeenStore` for postal-sdk (K2's review rule 1): the seen list
//! **is** the insert of the message's delivery row on `msg_deliveries`'
//! unique `postal_msg_id` index, made in the same SQLite transaction that
//! stores the message. So:
//! - a message is "seen" exactly when it is stored: a crash or a failed
//!   store rolls both back, and the plane still holds it;
//! - a replay (a re-pulled hold item, its live copy, a restart after the
//!   store) finds the row and the SDK answers `Disposition::Ack`, with no
//!   second copy, even after someone deleted the inbox item;
//! - there is no second table to keep in step.
//!
//! An id already held by **another** sender is not a replay of this
//! message: it is refused as a storage error (retried, never delivered
//! under someone else's record).

use postal_sdk::replay::{SeenEntry, SeenStore};
use rusqlite::{params, Connection, ErrorCode};

use crate::hire::deliveries;

/// The row a seen insert makes, before the message is verified.
#[derive(Debug, Clone)]
pub struct Pending {
    pub delivery_id: String,
    pub target_workspace_id: String,
    pub target: String,
}

/// The seen list inside one storing transaction.
pub struct TxSeen<'a> {
    conn: &'a Connection,
    row: Pending,
    /// Set once `insert_new` made the row.
    pub inserted: bool,
}

impl<'a> TxSeen<'a> {
    /// `conn` must be inside the transaction that also stores the message.
    pub fn new(conn: &'a Connection, row: Pending) -> Self {
        Self { conn, row, inserted: false }
    }
}

impl SeenStore for TxSeen<'_> {
    fn insert_new(&mut self, e: &SeenEntry) -> Result<bool, String> {
        let r = self.conn.execute(
            "INSERT INTO msg_deliveries (id, sender_label, via, actor, target_workspace_id, target, delivered, at, \
             postal_msg_id, sender_address) VALUES (?1, ?2, 'postal', ?3, ?4, ?5, 0, unixepoch(), ?6, ?2)",
            params![self.row.delivery_id, e.from, format!("postal:{}", e.from), self.row.target_workspace_id, self.row.target, e.id],
        );
        match r {
            Ok(_) => {
                self.inserted = true;
                Ok(true)
            }
            Err(rusqlite::Error::SqliteFailure(f, _)) if f.code == ErrorCode::ConstraintViolation => {
                match deliveries::by_postal_msg_id(self.conn, &e.id) {
                    Some(d) if d.sender_address.as_deref() == Some(e.from.as_str()) => Ok(false),
                    Some(_) => Err("postal_id_collision: another sender's message holds this id".into()),
                    None => Err("postal seen insert refused".into()),
                }
            }
            Err(e) => Err(format!("postal seen insert: {e}")),
        }
    }
}

/// Fill in what the SDK verified (same transaction): the badge, the body
/// hash, the sender's key fingerprint and where it is to land.
pub fn complete(conn: &Connection, delivery_id: &str, badge: &str, body_sha256: &str, fingerprint: &str, landed: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE msg_deliveries SET badge = ?2, body_sha256 = ?3, sender_fingerprint = ?4, landed = ?5 WHERE id = ?1",
        params![delivery_id, badge, body_sha256, fingerprint, landed],
    )
    .map(|_| ())
    .map_err(|e| format!("postal delivery record: {e}"))
}

/// Where a stored message landed so far (`landed` JSON), for `notify`.
pub fn set_landed(conn: &Connection, delivery_id: &str, landed: &str, delivered: bool) -> Result<(), String> {
    conn.execute(
        "UPDATE msg_deliveries SET landed = ?2, delivered = ?3 WHERE id = ?1",
        params![delivery_id, landed, delivered as i64],
    )
    .map(|_| ())
    .map_err(|e| format!("postal delivery record: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().expect("memory");
        crate::db::run_migrations(&c).expect("migrate");
        c
    }

    fn pending(id: &str) -> Pending {
        Pending { delivery_id: id.into(), target_workspace_id: "ws-1".into(), target: "/w/scout".into() }
    }

    #[test]
    fn the_seen_list_is_the_delivery_row_in_the_storing_transaction() {
        let mut c = conn();
        let e = SeenEntry::new("alice::initech.postal.bot", "01ARZ3NDEKTSV4RRFFQ69G5FAV", 1_800_000_000);
        // A store that fails rolls the seen insert back: still deliverable.
        {
            let tx = c.transaction().unwrap();
            let mut s = TxSeen::new(&tx, pending("m_aaaaaaaaaa"));
            assert!(s.insert_new(&e).unwrap());
            drop(tx);
        }
        let tx = c.transaction().unwrap();
        let mut s = TxSeen::new(&tx, pending("m_bbbbbbbbbb"));
        assert!(s.insert_new(&e).unwrap(), "rolled back: new again");
        complete(&tx, "m_bbbbbbbbbb", "p_0123456789abcdef0123", &"ab".repeat(32), &"cd".repeat(32), "{}").unwrap();
        tx.commit().unwrap();
        // A replay: seen, no second row.
        let tx = c.transaction().unwrap();
        let mut s = TxSeen::new(&tx, pending("m_cccccccccc"));
        assert!(!s.insert_new(&e).unwrap());
        tx.commit().unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM msg_deliveries WHERE postal_msg_id = ?1", [&e.id], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // The same id from someone else is not a replay of this message.
        let other = SeenEntry::new("mallory::evil.postal.bot", &e.id, 1_800_000_000);
        let tx = c.transaction().unwrap();
        let mut s = TxSeen::new(&tx, pending("m_dddddddddd"));
        assert!(s.insert_new(&other).unwrap_err().starts_with("postal_id_collision"));
        let d = deliveries::get(&tx, "m_bbbbbbbbbb").unwrap();
        assert_eq!((d.via.as_str(), d.badge.as_deref(), d.delivered), ("postal", Some("p_0123456789abcdef0123"), false));
    }
}
