//! Sender check (0161, S7, HT15). Every live message delivery gets a short
//! id that rides in the recipient's prefix (`[from job-j1 · m_7Qx2Ab9c]`).
//! `k2 msg who <id>` answers from this table, which only the daemon writes
//! from the caller's credential — so text in a body that imitates a prefix
//! has no row behind it.
//!
//! 0310 (prd-postal-in-k2-v1 S2): every row also keeps the SHA-256 of the
//! text K2 delivered ([`body_sha256`]), so `k2 msg verify <code> --text-file
//! f` tells a real id copied into another body (`content: mismatch`) from
//! the message it belongs to. A native Postal delivery adds the SDK's `p_`
//! badge and the Postal message id (one row per id).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::now_secs;

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    /// The sending workspace (passport callers only).
    pub sender_workspace_id: Option<String>,
    /// What the recipient saw after `[from`.
    pub sender_label: Option<String>,
    /// `passport` | `owner` | `api-key` | `postal` | `federation`.
    pub via: String,
    /// Audit actor (`agent:<handle>`, `owner-token`, `user:<name>`, `api-key:<id>`).
    pub actor: Option<String>,
    /// App-asserted, not verified.
    pub on_behalf_of: Option<String>,
    pub target_workspace_id: Option<String>,
    pub target: String,
    pub delivered: bool,
    pub at: i64,
    /// [`body_sha256`] of the delivered text (0310; NULL on older rows).
    pub body_sha256: Option<String>,
    /// The Postal SDK's `p_` badge (native Postal deliveries).
    pub badge: Option<String>,
    /// The Postal message id: at most one row each.
    pub postal_msg_id: Option<String>,
    /// The sender outside this server: a Postal address or `agent::host`.
    pub sender_address: Option<String>,
    /// That sender's key fingerprint (a federated peer's pinned key).
    pub sender_fingerprint: Option<String>,
    /// Where it landed (JSON), for Postal routing.
    pub landed: Option<String>,
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

/// The text one delivery's hash is taken over: line breaks as `\n` (`\r\n`
/// and a lone `\r` count as one), trailing whitespace and breaks dropped. A
/// file that holds the message (an editor adds a last newline) hashes the
/// same as the message sent.
pub fn normalize_body(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").trim_end().to_string()
}

/// Lowercase hex SHA-256 of [`normalize_body`]`(text)`.
pub fn body_sha256(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(normalize_body(text).as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Is `s` a lowercase hex SHA-256?
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn record(conn: &Connection, d: &Delivery) -> Result<(), String> {
    conn.execute(
        "INSERT INTO msg_deliveries (id, sender_workspace_id, sender_label, via, actor, on_behalf_of, \
         target_workspace_id, target, delivered, at, body_sha256, badge, postal_msg_id, sender_address, \
         sender_fingerprint, landed) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
            d.body_sha256,
            d.badge,
            d.postal_msg_id,
            d.sender_address,
            d.sender_fingerprint,
            d.landed,
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

const COLS: &str = "id, sender_workspace_id, sender_label, via, actor, on_behalf_of, target_workspace_id, \
     target, delivered, at, body_sha256, badge, postal_msg_id, sender_address, sender_fingerprint, landed";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Delivery> {
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
        body_sha256: r.get(10)?,
        badge: r.get(11)?,
        postal_msg_id: r.get(12)?,
        sender_address: r.get(13)?,
        sender_fingerprint: r.get(14)?,
        landed: r.get(15)?,
    })
}

fn one(conn: &Connection, col: &str, value: &str) -> Option<Delivery> {
    conn.query_row(&format!("SELECT {COLS} FROM msg_deliveries WHERE {col} = ?1"), params![value.trim()], row)
        .optional()
        .ok()
        .flatten()
}

pub fn get(conn: &Connection, id: &str) -> Option<Delivery> {
    one(conn, "id", id)
}

/// The delivery a `p_` badge names.
pub fn by_badge(conn: &Connection, badge: &str) -> Option<Delivery> {
    one(conn, "badge", badge)
}

/// The delivery of a Postal message id, if K2 recorded one.
pub fn by_postal_msg_id(conn: &Connection, postal_msg_id: &str) -> Option<Delivery> {
    one(conn, "postal_msg_id", postal_msg_id)
}

/// Let a Postal message id be delivered again: its row (a try that never
/// reached the agent) keeps its record but no longer holds the id.
pub fn release_postal_msg_id(conn: &Connection, id: &str) {
    let _ = conn.execute("UPDATE msg_deliveries SET postal_msg_id = NULL WHERE id = ?1", params![id]);
}

/// What a text given to `k2 msg verify` says about a delivery's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Content {
    /// One of the given hashes is the delivered body's.
    Match,
    /// None is: this is not the text K2 delivered under that code.
    Mismatch,
    /// The row predates 0310 and keeps no hash.
    Unknown,
    /// No text was given.
    NotChecked,
}

/// Compare `candidates` (lowercase hex SHA-256s of [`normalize_body`]) to
/// the row's hash.
pub fn content_of(d: &Delivery, candidates: &[String]) -> Content {
    match (&d.body_sha256, candidates.is_empty()) {
        (_, true) => Content::NotChecked,
        (None, false) => Content::Unknown,
        (Some(h), false) => {
            if candidates.iter().any(|c| c.eq_ignore_ascii_case(h)) {
                Content::Match
            } else {
                Content::Mismatch
            }
        }
    }
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

    #[test]
    fn the_body_hash_is_stable_across_line_ends_and_a_last_newline() {
        let a = body_sha256("one\ntwo");
        assert_eq!(a, body_sha256("one\r\ntwo\n"));
        assert_eq!(a, body_sha256("one\rtwo  \n\n"));
        assert_ne!(a, body_sha256("one\ntwo!"));
        assert_ne!(a, body_sha256(" one\ntwo"), "leading space is content");
        assert!(is_sha256_hex(&a));
        assert_eq!(body_sha256(""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    fn migrated_through(last: &str) -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        crate::db::run_migrations_through(&conn, Some(last)).expect("migrate");
        conn
    }

    /// 0310 on a database from before it: every row stays, field for field,
    /// with no hash; new rows take postal and federation; a Postal message
    /// id and a badge each name one row.
    #[test]
    fn migration_0310_keeps_existing_rows_and_widens_via() {
        let conn = migrated_through("0307_home_chat_rows_apart");
        conn.execute(
            "INSERT INTO msg_deliveries (id, sender_workspace_id, sender_label, via, actor, on_behalf_of, \
             target_workspace_id, target, delivered, at) VALUES \
             ('m_OLD0000001', 'ws-a', 'a', 'passport', 'agent:a', NULL, 'ws-b', 'b', 1, 1700000000), \
             ('m_OLD0000002', NULL, 'rosson', 'owner', 'owner-token', 'app', 'ws-b', 'b', 0, 1700000001)",
            [],
        )
        .expect("0161 rows");
        assert!(
            conn.execute("INSERT INTO msg_deliveries (id, via, target) VALUES ('m_X', 'federation', 't')", []).is_err(),
            "before 0310 a federated sender has no via"
        );
        crate::db::run_migrations(&conn).expect("0310");
        let old = get(&conn, "m_OLD0000001").expect("kept");
        assert_eq!(
            old,
            Delivery {
                id: "m_OLD0000001".into(),
                sender_workspace_id: Some("ws-a".into()),
                sender_label: Some("a".into()),
                via: "passport".into(),
                actor: Some("agent:a".into()),
                target_workspace_id: Some("ws-b".into()),
                target: "b".into(),
                delivered: true,
                at: 1700000000,
                ..Default::default()
            }
        );
        let old2 = get(&conn, "m_OLD0000002").expect("kept");
        assert_eq!((old2.on_behalf_of.as_deref(), old2.delivered, old2.body_sha256.as_deref()), (Some("app"), false, None));
        assert_eq!(content_of(&old2, &[body_sha256("x")]), Content::Unknown);

        let mut fed = Delivery {
            id: new_id(),
            via: "federation".into(),
            target: "b".into(),
            sender_address: Some("sales::rosson.example".into()),
            sender_fingerprint: Some("fp".into()),
            body_sha256: Some(body_sha256("hello")),
            ..Default::default()
        };
        record(&conn, &fed).expect("federation via");
        let got = get(&conn, &fed.id).expect("row");
        assert_eq!(content_of(&got, &[body_sha256("hello\n")]), Content::Match);
        assert_eq!(content_of(&got, &[body_sha256("hello!")]), Content::Mismatch);
        assert_eq!(content_of(&got, &[]), Content::NotChecked);

        let p = Delivery {
            id: new_id(),
            via: "postal".into(),
            target: "b".into(),
            badge: Some("p_3f9a0c41d27be6058a1c".into()),
            postal_msg_id: Some("01JPOSTALMSG0000000000001".into()),
            ..Default::default()
        };
        record(&conn, &p).expect("postal via");
        assert_eq!(by_badge(&conn, "p_3f9a0c41d27be6058a1c").map(|d| d.id), Some(p.id.clone()));
        assert_eq!(by_postal_msg_id(&conn, "01JPOSTALMSG0000000000001").map(|d| d.id), Some(p.id.clone()));
        let again = Delivery { id: new_id(), ..p.clone() };
        assert!(record(&conn, &again).is_err(), "one row per Postal message id");
        release_postal_msg_id(&conn, &p.id);
        let again = Delivery { badge: None, ..again };
        record(&conn, &again).expect("a released id may be delivered again");
        fed.id = new_id();
        fed.via = "carrier-pigeon".into();
        assert!(record(&conn, &fed).is_err(), "via stays a closed set");
        fed.via = "owner".into();
        fed.body_sha256 = Some("short".into());
        assert!(record(&conn, &fed).is_err(), "a hash is 64 hex");
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM msg_deliveries", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 5);
    }
}
