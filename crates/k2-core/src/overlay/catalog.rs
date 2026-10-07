//! rusqlite overlay catalog — last_seq only. Bodies stay in redb.

use rusqlite::{params, Connection, OptionalExtension};

pub fn ensure_conversation(
    conn: &Connection,
    conversation_id: &str,
    project_id: &str,
) -> Result<(), String> {
    let conversation_id = conversation_id.trim();
    let project_id = project_id.trim();
    if conversation_id.is_empty() {
        return Err("overlay: conversation_id required".to_string());
    }
    if project_id.is_empty() {
        return Err("overlay: project_id required".to_string());
    }
    conn.execute(
        "INSERT INTO overlay_conversations (conversation_id, project_id, last_thread_seq, last_chatter_seq) \
         VALUES (?1, ?2, 0, 0) \
         ON CONFLICT(conversation_id) DO UPDATE SET project_id = excluded.project_id",
        params![conversation_id, project_id],
    )
    .map_err(|e| format!("overlay catalog ensure: {e}"))?;
    Ok(())
}

pub fn next_thread_seq(
    conn: &Connection,
    conversation_id: &str,
    project_id: &str,
) -> Result<i64, String> {
    ensure_conversation(conn, conversation_id, project_id)?;
    conn.execute(
        "UPDATE overlay_conversations SET last_thread_seq = last_thread_seq + 1 \
         WHERE conversation_id = ?1",
        params![conversation_id],
    )
    .map_err(|e| format!("overlay next_thread_seq: {e}"))?;
    conn.query_row(
        "SELECT last_thread_seq FROM overlay_conversations WHERE conversation_id = ?1",
        params![conversation_id],
        |r| r.get(0),
    )
    .map_err(|e| format!("overlay last_thread_seq: {e}"))
}

pub fn next_chatter_seq(
    conn: &Connection,
    conversation_id: &str,
    project_id: &str,
) -> Result<i64, String> {
    ensure_conversation(conn, conversation_id, project_id)?;
    conn.execute(
        "UPDATE overlay_conversations SET last_chatter_seq = last_chatter_seq + 1 \
         WHERE conversation_id = ?1",
        params![conversation_id],
    )
    .map_err(|e| format!("overlay next_chatter_seq: {e}"))?;
    conn.query_row(
        "SELECT last_chatter_seq FROM overlay_conversations WHERE conversation_id = ?1",
        params![conversation_id],
        |r| r.get(0),
    )
    .map_err(|e| format!("overlay last_chatter_seq: {e}"))
}

pub fn next_chatterlog_seq(conn: &Connection) -> Result<i64, String> {
    let n = conn
        .execute(
            "UPDATE overlay_host SET last_chatterlog_seq = last_chatterlog_seq + 1 WHERE id = 1",
            [],
        )
        .map_err(|e| format!("overlay next_chatterlog_seq: {e}"))?;
    if n == 0 {
        conn.execute(
            "INSERT INTO overlay_host (id, last_chatterlog_seq) VALUES (1, 1)",
            [],
        )
        .map_err(|e| format!("overlay overlay_host insert: {e}"))?;
        return Ok(1);
    }
    conn.query_row(
        "SELECT last_chatterlog_seq FROM overlay_host WHERE id = 1",
        [],
        |r| r.get(0),
    )
    .map_err(|e| format!("overlay last_chatterlog_seq: {e}"))
}

/// Catalog half of a conversation move: `to` gets `from`'s project and
/// the merged collections' last seqs (pointers were renumbered
/// `1..=n`; a higher last seq already on `to` is kept, so a seq is never
/// handed out twice), and `from`'s row goes. Run inside the caller's
/// savepoint.
pub fn apply_move(
    conn: &Connection,
    from: &str,
    to: &str,
    project_id: &str,
    last_thread_seq: i64,
    last_chatter_seq: i64,
) -> Result<(), String> {
    ensure_conversation(conn, to, project_id)?;
    conn.execute(
        "UPDATE overlay_conversations \
         SET last_thread_seq = MAX(last_thread_seq, ?2), last_chatter_seq = MAX(last_chatter_seq, ?3) \
         WHERE conversation_id = ?1",
        params![to, last_thread_seq, last_chatter_seq],
    )
    .map_err(|e| format!("overlay catalog move: {e}"))?;
    conn.execute(
        "DELETE FROM overlay_conversations WHERE conversation_id = ?1",
        params![from],
    )
    .map_err(|e| format!("overlay catalog move: {e}"))?;
    Ok(())
}

pub fn get(
    conn: &Connection,
    conversation_id: &str,
) -> Result<Option<(String, i64, i64)>, String> {
    conn.query_row(
        "SELECT project_id, last_thread_seq, last_chatter_seq \
         FROM overlay_conversations WHERE conversation_id = ?1",
        params![conversation_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
    .map_err(|e| format!("overlay catalog get: {e}"))
}
