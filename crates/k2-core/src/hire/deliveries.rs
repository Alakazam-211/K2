//! Sender check (0161, S7, HT15). Every live message delivery gets a short
//! id that rides in the recipient's prefix (`[from job-j1 · m_7Qx2Ab9c]`).
//! `k2 msg who <id>` answers from this table, which only the daemon writes
//! from the caller's credential — so text in a body that imitates a prefix
//! has no row behind it.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::now_secs;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    /// The sending workspace (passport callers only).
    pub sender_workspace_id: Option<String>,
    /// What the recipient saw after `[from`.
    pub sender_label: Option<String>,
    /// `passport` | `owner` | `api-key`.
    pub via: String,
    /// Audit actor (`agent:<handle>`, `owner-token`, `user:<name>`, `api-key:<id>`).
    pub actor: Option<String>,
    /// App-asserted, not verified.
    pub on_behalf_of: Option<String>,
    pub target_workspace_id: Option<String>,
    pub target: String,
    pub delivered: bool,
    pub at: i64,
}

/// `m_` + 10 base62 characters (≈ 59 bits).
pub fn new_id() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    const A: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut body = String::with_capacity(12);
    body.push_str("m_");
    let mut scratch = [0u8; 32];
    while body.len() < 12 {
        OsRng.fill_bytes(&mut scratch);
        for &b in scratch.iter() {
            // Reject ≥ 248 (= 62·4) so the mapping is unbiased.
            if b < 248 && body.len() < 12 {
                body.push(A[(b % 62) as usize] as char);
            }
        }
    }
    body
}

/// Is `s` shaped like a delivery id?
pub fn is_delivery_id(s: &str) -> bool {
    let s = s.trim();
    s.len() == 12 && s.starts_with("m_") && s[2..].chars().all(|c| c.is_ascii_alphanumeric())
}

pub fn record(conn: &Connection, d: &Delivery) -> Result<(), String> {
    conn.execute(
        "INSERT INTO msg_deliveries (id, sender_workspace_id, sender_label, via, actor, on_behalf_of, \
         target_workspace_id, target, delivered, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            d.id,
            d.sender_workspace_id,
            d.sender_label,
            d.via,
            d.actor,
            d.on_behalf_of,
            d.target_workspace_id,
            d.target,
            d.delivered as i64,
            if d.at > 0 { d.at } else { now_secs() },
        ],
    )
    .map(|_| ())
    .map_err(|e| format!("msg_deliveries: {e}"))
}

pub fn mark_delivered(conn: &Connection, id: &str, delivered: bool) {
    let _ = conn.execute(
        "UPDATE msg_deliveries SET delivered = ?2 WHERE id = ?1",
        params![id, delivered as i64],
    );
}

pub fn get(conn: &Connection, id: &str) -> Option<Delivery> {
    conn.query_row(
        "SELECT id, sender_workspace_id, sender_label, via, actor, on_behalf_of, target_workspace_id, \
         target, delivered, at FROM msg_deliveries WHERE id = ?1",
        params![id.trim()],
        |r| {
            Ok(Delivery {
                id: r.get(0)?,
                sender_workspace_id: r.get(1)?,
                sender_label: r.get(2)?,
                via: r.get(3)?,
                actor: r.get(4)?,
                on_behalf_of: r.get(5)?,
                target_workspace_id: r.get(6)?,
                target: r.get(7)?,
                delivered: r.get::<_, i64>(8)? != 0,
                at: r.get(9)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_shaped_and_distinct() {
        let a = new_id();
        let b = new_id();
        assert!(is_delivery_id(&a), "{a}");
        assert_ne!(a, b);
        assert!(!is_delivery_id("m_short"));
        assert!(!is_delivery_id("x_0123456789"));
        assert!(!is_delivery_id("m_01234567-9"));
    }
}
