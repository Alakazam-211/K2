//! Ticket discussions on the Thread store (ticket-thread-v1, 0.46.17,
//! migration 0322): the ticket side of `ticket:<id>`.
//!
//! - **Reads**: a ticket's messages as [`FeedbackComment`]s, from its
//!   Thread ([`read_comments`]), the same shape `show.comments` always had.
//!   A deleted message (a tombstone) is left out, and the row's count drops
//!   with it ([`note_deleted`]).
//! - **Moves**: a ticket filed before 0.46.17 keeps its old
//!   `feedback_comments` rows until [`move_ticket_with`] imports them
//!   (import key `ticket/<comment id>`, the doc id IS the comment id, files
//!   as they are) and sets `comments_in_thread`, in one savepoint. The
//!   background backfill ([`spawn_backfill_and_sweep`], after `ready`) moves
//!   every ticket; it is resumable (each ticket is all or nothing, and a
//!   re-run skips lines already imported). A ticket's next message moves it
//!   first if the backfill hasn't yet. `feedback_comments` is never written
//!   again (kept one release for rollback).
//! - **Lifetime**: a ticket conversation's catalog row goes with its ticket
//!   (migration 0322's trigger, same transaction); the daily
//!   [`sweep_orphans`] removes, and logs, any ticket conversation whose
//!   ticket row is gone (catalog row, stored messages and read cursors).
//! - **Live**: every stored ticket message is handed to the post listener
//!   the daemon registers ([`set_post_listener`]: the overlay socket frames),
//!   whichever door wrote it.

use rusqlite::{params, Connection, OptionalExtension};

use crate::feedback::{parse_attachments_column, FeedbackComment};
use crate::overlay::ticket::{self, OldComment, TicketMessage};
use crate::overlay::{OverlayItem, OverlayLink, PostAuthor};

/// The `/boot-status` feature key: ticket discussions are Thread
/// conversations `ticket:<id>` (older servers keep the ticket column).
pub const FEATURE: &str = "ticket-thread-v1";

static POST_LISTENER: std::sync::OnceLock<fn(&OverlayItem, &[OverlayLink])> = std::sync::OnceLock::new();

/// Register the listener every stored ticket message is handed to (first
/// call wins). k2-core has no event bus; the daemon's emits the frames.
pub fn set_post_listener(f: fn(&OverlayItem, &[OverlayLink])) {
    let _ = POST_LISTENER.set(f);
}

/// Hand a just-stored ticket message to the listener. Callers hold no DB
/// lock.
pub fn notify_posted(item: &OverlayItem, links: &[OverlayLink]) {
    if let Some(f) = POST_LISTENER.get() {
        f(item, links);
    }
}

/// `(project_id, comments_in_thread)` of ticket `id`, `None` when there is
/// no such ticket.
pub fn ticket_row(conn: &Connection, id: &str) -> Result<Option<(String, bool)>, String> {
    conn.query_row(
        "SELECT project_id, comments_in_thread FROM feedback WHERE id = ?1",
        params![id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)),
    )
    .optional()
    .map_err(|e| format!("ticket row: {e}"))
}

/// Are ticket `id`'s messages in the Thread store? False for an unknown id.
pub fn in_thread(id: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    match ticket_row(&conn, id) {
        Ok(row) => row.is_some_and(|(_, moved)| moved),
        Err(e) => {
            crate::log_debug!("[feedback/thread] {id}: {e}");
            false
        }
    }
}

/// A Thread item of `ticket:<id>` as the comment `show` returns. The stored
/// name is the doc's `from`.
pub fn comment_of(ticket_id: &str, item: &OverlayItem) -> FeedbackComment {
    FeedbackComment {
        id: item.id.clone(),
        feedback_id: ticket_id.to_string(),
        author: item.doc.from.clone(),
        body: item.doc.body.clone().unwrap_or_default(),
        created_at: item.doc.created_at,
        attachments: item.doc.attachments.clone(),
    }
}

/// Ticket `id`'s messages in order, deleted ones left out.
pub fn read_comments(id: &str) -> Result<Vec<FeedbackComment>, String> {
    Ok(ticket::read_messages(id)?
        .iter()
        .filter(|i| !i.doc.deleted)
        .map(|i| comment_of(id, i))
        .collect())
}

/// One message of ticket `id` by its id (None when it isn't that ticket's,
/// or was deleted).
pub fn read_comment(id: &str, comment_id: &str) -> Option<FeedbackComment> {
    let conv = ticket::conversation_id(id);
    let item = match crate::overlay::find_thread_item(&conv, comment_id.trim()) {
        Ok(found) => found?,
        Err(e) => {
            crate::log_debug!("[feedback/thread] {id}/{comment_id}: {e}");
            return None;
        }
    };
    (!item.doc.deleted).then(|| comment_of(id, &item))
}

/// The author stamp an old comment row gets: the ticket's agent (no stamp,
/// as an agent's Thread lines before 0.46.13), the owner token's `owner`,
/// else a person by that name.
fn old_author(author: &str, agent_name: &str) -> Option<PostAuthor> {
    let a = author.trim();
    if a == agent_name.trim() {
        None
    } else if a == crate::notify_prefs::OWNER {
        Some(PostAuthor::owner())
    } else {
        Some(PostAuthor::user(a))
    }
}

/// Move ticket `id`'s old `feedback_comments` rows into `ticket:<id>` and
/// set its `comments_in_thread`, all or nothing (see the module docs). The
/// caller holds the shared DB lock (`conn`); the overlay write lock is
/// taken inside. A ticket already moved is left alone. Returns the lines
/// written.
pub fn move_ticket_with(conn: &Connection, id: &str) -> Result<usize, String> {
    let row: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT project_id, agent_name, comments_in_thread FROM feedback WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("ticket move: {e}"))?;
    let Some((project_id, agent_name, moved)) = row else {
        return Err(format!("no feedback item with id {id}"));
    };
    if moved != 0 {
        return Ok(0);
    }
    type Row = (String, String, String, i64, Option<String>);
    let rows: Vec<Row> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, author, body, created_at, attachments_json FROM feedback_comments \
                 WHERE feedback_id = ?1 ORDER BY created_at ASC, rowid ASC",
            )
            .map_err(|e| format!("ticket move: {e}"))?;
        let mapped = stmt
            .query_map(params![id], |r| {
                // TA29: a column that isn't text reads as no files.
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, Option<String>>(4).unwrap_or(None)))
            })
            .map_err(|e| format!("ticket move: {e}"))?;
        mapped.collect::<Result<_, _>>().map_err(|e| format!("ticket move row: {e}"))?
    };
    let comments: Vec<OldComment<'_>> = rows
        .iter()
        .map(|(cid, author, body, at, raw)| OldComment {
            comment_id: cid,
            created_at: *at,
            message: TicketMessage {
                ticket_id: id,
                project_id: &project_id,
                from: author,
                author: old_author(author, &agent_name),
                body,
                attachments: parse_attachments_column(cid, raw.as_deref()),
            },
        })
        .collect();
    ticket::import_comments(conn, id, &project_id, &comments, |c| {
        c.execute("UPDATE feedback SET comments_in_thread = 1 WHERE id = ?1", params![id])
            .map_err(|e| format!("ticket move mark: {e}"))?;
        Ok(())
    })
}

/// Tickets per backfill batch.
const BACKFILL_BATCH: usize = 50;

/// One backfill batch: move up to `limit` tickets with ids after `after`
/// that are still on their old rows. Returns the last id it looked at
/// (`None` = nothing left). A ticket that fails is logged and skipped this
/// run (the next run, or its next message, tries again).
pub fn backfill_step(after: &str, limit: usize) -> Result<Option<String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM feedback WHERE comments_in_thread = 0 AND id > ?1 ORDER BY id LIMIT ?2")
            .map_err(|e| format!("ticket backfill: {e}"))?;
        let rows = stmt
            .query_map(params![after, limit as i64], |r| r.get::<_, String>(0))
            .map_err(|e| format!("ticket backfill: {e}"))?;
        rows.collect::<Result<_, _>>().map_err(|e| format!("ticket backfill row: {e}"))?
    };
    for id in &ids {
        if let Err(e) = move_ticket_with(&conn, id) {
            crate::log_debug!("[feedback/thread] WARN ticket {id} stays on its old rows for now: {e}");
        }
    }
    Ok(ids.last().cloned())
}

/// Move every ticket still on its old rows (the whole backfill, one batch
/// per lock). Returns how many batches ran.
pub fn backfill_all() -> Result<usize, String> {
    let mut after = String::new();
    let mut batches = 0usize;
    while let Some(last) = backfill_step(&after, BACKFILL_BATCH)? {
        after = last;
        batches += 1;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(batches)
}

/// What one [`sweep_orphans`] removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Ticket conversations removed (catalog row and/or stored messages).
    pub conversations: Vec<String>,
    /// Stored messages removed with them.
    pub messages: usize,
}

/// Remove every ticket conversation whose ticket row is gone: catalog rows
/// whose ticket is missing, and stored `ticket:` messages with neither a
/// ticket nor a catalog row (a create whose transaction rolled back after
/// its seed was stored). Each removal is logged.
pub fn sweep_orphans() -> Result<SweepReport, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut orphans: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT conversation_id FROM overlay_conversations WHERE kind = 'ticket' \
                 AND ticket_id NOT IN (SELECT id FROM feedback) ORDER BY conversation_id",
            )
            .map_err(|e| format!("ticket sweep: {e}"))?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| format!("ticket sweep: {e}"))?;
        rows.collect::<Result<_, _>>().map_err(|e| format!("ticket sweep row: {e}"))?
    };
    for conv in ticket::stored_conversations()? {
        if orphans.contains(&conv) {
            continue;
        }
        let Some(tid) = ticket::ticket_of(&conv) else { continue };
        let ticket_exists: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM feedback WHERE id = ?1", params![tid], |r| r.get(0))
            .map_err(|e| format!("ticket sweep: {e}"))?;
        let catalogued: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM overlay_conversations WHERE conversation_id = ?1", params![conv], |r| r.get(0))
            .map_err(|e| format!("ticket sweep: {e}"))?;
        if !ticket_exists && !catalogued {
            orphans.push(conv);
        }
    }
    let mut report = SweepReport::default();
    for conv in orphans {
        let n = ticket::purge(&conv)?;
        conn.execute("DELETE FROM overlay_conversations WHERE conversation_id = ?1 AND kind = 'ticket'", params![conv])
            .map_err(|e| format!("ticket sweep delete: {e}"))?;
        crate::read_state::delete_conversation(&conn, crate::read_state::KIND_THREAD, &conv)?;
        crate::log_debug!("[feedback/thread] swept {conv}: its ticket is gone ({n} messages removed)");
        report.messages += n;
        report.conversations.push(conv);
    }
    Ok(report)
}

/// How often the sweep runs.
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// The background backfill, then the daily sweep, on one thread. Called by
/// the daemon after `ready`; returns at once.
pub fn spawn_backfill_and_sweep() {
    let spawned = std::thread::Builder::new().name("k2-ticket-threads".into()).spawn(|| {
        match backfill_all() {
            Ok(0) => {}
            Ok(n) => crate::log_debug!("[feedback/thread] backfill done: {n} batches"),
            Err(e) => crate::log_debug!("[feedback/thread] WARN backfill stopped (tickets read their old rows meanwhile): {e}"),
        }
        loop {
            match sweep_orphans() {
                Ok(r) if r.conversations.is_empty() => {}
                Ok(r) => crate::log_debug!("[feedback/thread] sweep removed {} ticket conversations", r.conversations.len()),
                Err(e) => crate::log_debug!("[feedback/thread] WARN sweep: {e}"),
            }
            std::thread::sleep(SWEEP_EVERY);
        }
    });
    if let Err(e) = spawned {
        crate::log_debug!("[feedback/thread] WARN backfill thread: {e}");
    }
}

/// A message of a ticket's Thread was deleted (a tombstone): its row's
/// counts drop by one message and its files. Returns the ticket id, or
/// `None` when `conversation_id` isn't a ticket's.
pub fn note_deleted(conn: &Connection, conversation_id: &str, files: usize) -> Result<Option<String>, String> {
    let Some(tid) = ticket::ticket_of(conversation_id) else {
        return Ok(None);
    };
    conn.execute(
        "UPDATE feedback SET comment_count = MAX(comment_count - 1, 0), file_count = MAX(file_count - ?2, 0), \
         updated_at = CAST(strftime('%s', 'now') AS INTEGER) WHERE id = ?1",
        params![tid, files as i64],
    )
    .map_err(|e| format!("ticket delete count: {e}"))?;
    Ok(Some(tid.to_string()))
}

#[cfg(test)]
#[path = "feedback_thread_tests.rs"]
mod tests;
