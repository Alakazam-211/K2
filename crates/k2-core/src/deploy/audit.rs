//! The hash-chained deploy audit (§7.2, §9.1 `deploy_audit`), shared by
//! Apps (`subject_kind = app`) and Garden widget versions
//! (`garden_widget`, S9).
//!
//! Each row's `hash = sha256(prev_hash || canonical row)`, with
//! `prev_hash` the previous row's hash (64 zeros for the first). A row
//! edited, removed or re-ordered breaks [`verify`] at that row. Appends
//! run in one `BEGIN IMMEDIATE` transaction on the caller's connection, so
//! two writers can't both chain onto the same previous row.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value as J;
use sha2::{Digest, Sha256};

pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub const SUBJECT_APP: &str = "app";
pub const SUBJECT_GARDEN_WIDGET: &str = "garden_widget";

/// One audit row.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRow {
    pub seq: i64,
    pub subject_kind: String,
    pub subject_id: String,
    pub event: String,
    pub actor: String,
    pub on_behalf_of: Option<String>,
    pub payload: J,
    pub prev_hash: String,
    pub hash: String,
    pub at: i64,
}

/// The bytes a row's hash covers (everything but `seq` and `hash`).
/// `payload` is serialized by serde_json from a `Value`, whose object keys
/// are sorted (no `preserve_order` feature), so it is canonical.
fn canonical(subject_kind: &str, subject_id: &str, event: &str, actor: &str, on_behalf_of: Option<&str>, payload: &J, at: i64) -> String {
    serde_json::json!([subject_kind, subject_id, event, actor, on_behalf_of, payload, at]).to_string()
}

fn chain(prev: &str, canon: &str) -> String {
    let mut h = Sha256::new();
    h.update(prev.as_bytes());
    h.update(canon.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Append one row; returns its hash.
pub fn append(
    conn: &Connection,
    subject_kind: &str,
    subject_id: &str,
    event: &str,
    actor: &str,
    on_behalf_of: Option<&str>,
    payload: &J,
) -> rusqlite::Result<String> {
    let at = super::now();
    // Nested inside a caller's transaction? Then just run in it.
    let own_tx = conn.is_autocommit();
    if own_tx {
        conn.execute_batch("BEGIN IMMEDIATE")?;
    }
    let res = (|| {
        let prev: String = conn
            .query_row("SELECT hash FROM deploy_audit ORDER BY seq DESC LIMIT 1", [], |r| r.get(0))
            .optional()?
            .unwrap_or_else(|| GENESIS.to_string());
        let canon = canonical(subject_kind, subject_id, event, actor, on_behalf_of, payload, at);
        let hash = chain(&prev, &canon);
        conn.execute(
            "INSERT INTO deploy_audit (subject_kind, subject_id, event, actor, on_behalf_of, payload_json, prev_hash, hash, at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![subject_kind, subject_id, event, actor, on_behalf_of, payload.to_string(), prev, hash, at],
        )?;
        Ok(hash)
    })();
    if own_tx {
        match &res {
            Ok(_) => conn.execute_batch("COMMIT")?,
            Err(_) => {
                let _ = conn.execute_batch("ROLLBACK");
            }
        }
    }
    res
}

/// Why the chain doesn't verify.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainBreak {
    pub seq: i64,
    pub reason: String,
}

/// Walk the whole chain. `Ok(rows)` when every row links and re-hashes.
pub fn verify(conn: &Connection) -> rusqlite::Result<Result<usize, ChainBreak>> {
    let mut st = conn.prepare(
        "SELECT seq, subject_kind, subject_id, event, actor, on_behalf_of, payload_json, prev_hash, hash, at \
         FROM deploy_audit ORDER BY seq ASC",
    )?;
    let mut rows = st.query([])?;
    let mut prev = GENESIS.to_string();
    let mut n = 0usize;
    while let Some(r) = rows.next()? {
        let seq: i64 = r.get(0)?;
        let payload_text: String = r.get(6)?;
        let prev_hash: String = r.get(7)?;
        let hash: String = r.get(8)?;
        if prev_hash != prev {
            return Ok(Err(ChainBreak { seq, reason: "prev_hash does not match the row before (a row was removed or re-ordered)".into() }));
        }
        let payload: J = match serde_json::from_str(&payload_text) {
            Ok(v) => v,
            Err(e) => return Ok(Err(ChainBreak { seq, reason: format!("payload is not JSON: {e}") })),
        };
        let on_behalf: Option<String> = r.get(5)?;
        let canon = canonical(
            &r.get::<_, String>(1)?,
            &r.get::<_, String>(2)?,
            &r.get::<_, String>(3)?,
            &r.get::<_, String>(4)?,
            on_behalf.as_deref(),
            &payload,
            r.get(9)?,
        );
        if chain(&prev_hash, &canon) != hash {
            return Ok(Err(ChainBreak { seq, reason: "row content does not match its hash (the row was edited)".into() }));
        }
        prev = hash;
        n += 1;
    }
    Ok(Ok(n))
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuditRow> {
    let payload_text: String = r.get(6)?;
    Ok(AuditRow {
        seq: r.get(0)?,
        subject_kind: r.get(1)?,
        subject_id: r.get(2)?,
        event: r.get(3)?,
        actor: r.get(4)?,
        on_behalf_of: r.get(5)?,
        payload: serde_json::from_str(&payload_text).unwrap_or(J::String(payload_text)),
        prev_hash: r.get(7)?,
        hash: r.get(8)?,
        at: r.get(9)?,
    })
}

/// One subject's rows, newest first.
pub fn list_for(conn: &Connection, subject_kind: &str, subject_id: &str, limit: usize) -> rusqlite::Result<Vec<AuditRow>> {
    let mut st = conn.prepare(
        "SELECT seq, subject_kind, subject_id, event, actor, on_behalf_of, payload_json, prev_hash, hash, at \
         FROM deploy_audit WHERE subject_kind = ?1 AND subject_id = ?2 ORDER BY seq DESC LIMIT ?3",
    )?;
    let v = st.query_map(params![subject_kind, subject_id, limit as i64], row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../drizzle_sql/0165_app_runtime_core.sql").replace("--> statement-breakpoint", "").as_str())
            .unwrap();
        c
    }

    #[test]
    fn chain_verifies_and_catches_edits_removals_and_reorders() {
        let c = conn();
        assert_eq!(verify(&c).unwrap(), Ok(0));
        let h1 = append(&c, SUBJECT_APP, "jim", "deploy", "owner", None, &serde_json::json!({"to": "a", "b": 1})).unwrap();
        append(&c, SUBJECT_APP, "jim", "live", "system", None, &serde_json::json!({})).unwrap();
        append(&c, SUBJECT_GARDEN_WIDGET, "arcade", "last_good", "system", Some("user:rosson"), &serde_json::json!({"hash": "x"})).unwrap();
        assert_eq!(verify(&c).unwrap(), Ok(3));
        let first_prev: String = c.query_row("SELECT prev_hash FROM deploy_audit WHERE seq = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(first_prev, GENESIS);
        let second_prev: String = c.query_row("SELECT prev_hash FROM deploy_audit WHERE seq = 2", [], |r| r.get(0)).unwrap();
        assert_eq!(second_prev, h1);
        assert_eq!(list_for(&c, SUBJECT_APP, "jim", 10).unwrap().len(), 2);

        // Edit: caught at that row.
        c.execute("UPDATE deploy_audit SET actor = 'agent:x' WHERE seq = 2", []).unwrap();
        let b = verify(&c).unwrap().unwrap_err();
        assert_eq!(b.seq, 2);
        assert!(b.reason.contains("edited"), "{b:?}");
        c.execute("UPDATE deploy_audit SET actor = 'system' WHERE seq = 2", []).unwrap();
        assert_eq!(verify(&c).unwrap(), Ok(3));

        // Removal: the next row no longer links.
        c.execute("DELETE FROM deploy_audit WHERE seq = 2", []).unwrap();
        let b = verify(&c).unwrap().unwrap_err();
        assert_eq!(b.seq, 3);
        assert!(b.reason.contains("removed"), "{b:?}");
    }
}
