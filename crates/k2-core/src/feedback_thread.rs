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
//!   first if the backfill hasn't yet. A moved message is marked imported
//!   ([`BACKFILL_MARK`]) and its author is a reading of the old author text
//!   ([`old_author`]: never an App guest or K2 made a Connect user).
//! - **Rollback (0.46.17 only, removed in 0.46.18)**: every new message is
//!   also written to `feedback_comments` under its own id ([`dual_write`]),
//!   so 0.46.16 put back loses nothing; after an upgrade again,
//!   [`reconcile_all`] imports what 0.46.16 wrote and resets the counters.
//!   Reads always come from the Thread.
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
use crate::overlay::doc::{AUTHOR_GUEST, AUTHOR_K2, AUTHOR_UNKNOWN, AUTHOR_USER};
use crate::overlay::{ImportedMeta, OverlayItem, OverlayLink, PostAuthor};

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

/// `imported.cloneId` on every ticket message moved from `feedback_comments`
/// (the backfill, and the rows an older release wrote after a rollback):
/// history, with its author a best reading of the old author text (kept as
/// `imported.from` and `author.name`), never a stamp K2 checked.
pub const BACKFILL_MARK: &str = "ticket-backfill";

/// A notice K2 itself wrote (`k2`, `k2-disk`, `K2 Postgres`, …).
fn is_k2_name(author: &str) -> bool {
    let l = author.trim().to_ascii_lowercase();
    l == AUTHOR_K2 || l.strip_prefix(AUTHOR_K2).is_some_and(|rest| rest.starts_with(['-', ' ']))
}

/// Is `name` an App guest's username on this server? An unreadable answer
/// (no principals table yet) is "can't tell", never a yes.
fn is_app_user(conn: &Connection, name: &str) -> bool {
    match conn.query_row("SELECT COUNT(*) > 0 FROM principals WHERE username = ?1", params![name], |r| r.get::<_, bool>(0)) {
        Ok(found) => found,
        Err(e) => {
            crate::log_debug!("[feedback/thread] App user lookup for an old author: {e}");
            false
        }
    }
}

/// The Connect login names (lower-cased) for classifying old authors.
fn connect_names() -> std::collections::HashSet<String> {
    match crate::connect_users::list_users() {
        Ok(users) => users.into_iter().map(|u| u.username.trim().to_lowercase()).collect(),
        Err(e) => {
            crate::log_debug!("[feedback/thread] Connect users unreadable; old authors read as unknown: {e}");
            std::collections::HashSet::new()
        }
    }
}

/// The author stamp an old comment row gets. The old table kept a name
/// only, so this is a reading, and it says so (the doc is marked imported,
/// the name rides as `name`):
/// - K2's own notice author (`k2…`) → [`AUTHOR_K2`];
/// - the ticket's agent → no stamp (an agent's line, as before 0.46.13);
/// - `owner` (only the owner token ever wrote it) → the owner;
/// - an App guest's username and no Connect login of that name → guest;
/// - a Connect login and no App guest of that name → that login;
/// - anything else (both, or neither) → [`AUTHOR_UNKNOWN`].
///
/// An App guest or K2 is never made a Connect user.
pub fn old_author(conn: &Connection, author: &str, agent_name: &str, connect: &std::collections::HashSet<String>) -> Option<PostAuthor> {
    let a = author.trim();
    let named = |kind: &str, login: &str| PostAuthor { kind: kind.to_string(), login: login.to_string(), name: Some(a.to_string()) };
    if is_k2_name(a) {
        return Some(named(AUTHOR_K2, AUTHOR_K2));
    }
    if a == agent_name.trim() {
        return None;
    }
    if a == crate::notify_prefs::OWNER {
        return Some(PostAuthor::owner());
    }
    let app = is_app_user(conn, a);
    let person = connect.contains(&a.to_lowercase());
    Some(match (app, person) {
        (true, false) => named(AUTHOR_GUEST, a),
        (false, true) => named(AUTHOR_USER, a),
        _ => named(AUTHOR_UNKNOWN, a),
    })
}

type OldRow = (String, String, String, i64, Option<String>);

fn old_rows(conn: &Connection, id: &str) -> Result<Vec<OldRow>, String> {
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
    mapped.collect::<Result<_, _>>().map_err(|e| format!("ticket move row: {e}"))
}

/// Import `rows` of ticket `id` into its Thread, each marked imported
/// ([`BACKFILL_MARK`]) with its author read by [`old_author`], then `mark`.
fn import_old_rows(
    conn: &Connection,
    id: &str,
    project_id: &str,
    agent_name: &str,
    rows: &[OldRow],
    mark: impl FnOnce(&Connection) -> Result<(), String>,
) -> Result<usize, String> {
    let connect = connect_names();
    let now = crate::overlay::now_secs();
    let comments: Vec<OldComment<'_>> = rows
        .iter()
        .map(|(cid, author, body, at, raw)| {
            let stamp = old_author(conn, author, agent_name, &connect);
            OldComment {
                comment_id: cid,
                created_at: *at,
                imported: Some(ImportedMeta {
                    clone_id: BACKFILL_MARK.to_string(),
                    source_doc_id: cid.clone(),
                    imported_at: now,
                    author: stamp.clone(),
                    from: Some(author.clone()),
                    ..Default::default()
                }),
                message: TicketMessage {
                    ticket_id: id,
                    project_id,
                    from: author,
                    author: stamp,
                    body,
                    attachments: parse_attachments_column(cid, raw.as_deref()),
                },
            }
        })
        .collect();
    ticket::import_comments(conn, id, project_id, &comments, mark)
}

/// `(project_id, agent_name, comments_in_thread)` of ticket `id`.
fn move_row(conn: &Connection, id: &str) -> Result<(String, String, bool), String> {
    conn.query_row(
        "SELECT project_id, agent_name, comments_in_thread FROM feedback WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
    )
    .optional()
    .map_err(|e| format!("ticket move: {e}"))?
    .ok_or_else(|| format!("no feedback item with id {id}"))
}

/// Move ticket `id`'s old `feedback_comments` rows into `ticket:<id>` and
/// set its `comments_in_thread`, all or nothing (see the module docs). The
/// caller holds the shared DB lock (`conn`); the overlay write lock is
/// taken inside. A ticket already moved is left alone. Returns the lines
/// written.
pub fn move_ticket_with(conn: &Connection, id: &str) -> Result<usize, String> {
    let (project_id, agent_name, moved) = move_row(conn, id)?;
    if moved {
        return Ok(0);
    }
    let rows = old_rows(conn, id)?;
    import_old_rows(conn, id, &project_id, &agent_name, &rows, |c| {
        c.execute("UPDATE feedback SET comments_in_thread = 1 WHERE id = ?1", params![id])
            .map_err(|e| format!("ticket move mark: {e}"))?;
        Ok(())
    })
}

/// After a rollback to 0.46.16 and an upgrade again: bring a moved ticket's
/// Thread up to its `feedback_comments` rows (the older release wrote
/// there only). Rows already in the Thread (every message 0.46.17 wrote is
/// dual-written under its own id) are skipped; the rest are imported, and
/// the row's counters are set from `feedback_comments` again. Returns the
/// lines written. Remove with the dual-write (0.46.18).
pub fn reconcile_ticket_with(conn: &Connection, id: &str) -> Result<usize, String> {
    let (project_id, agent_name, moved) = move_row(conn, id)?;
    if !moved {
        return Ok(0);
    }
    let conv = ticket::conversation_id(id);
    let mut missing = Vec::new();
    for row in old_rows(conn, id)? {
        if crate::overlay::store::thread_seq_of(&conv, &row.0)?.is_none() {
            missing.push(row);
        }
    }
    import_old_rows(conn, id, &project_id, &agent_name, &missing, |c| {
        c.execute(
            "UPDATE feedback SET \
             comment_count = (SELECT COUNT(*) FROM feedback_comments c WHERE c.feedback_id = ?1), \
             file_count = (SELECT COALESCE(SUM(CASE WHEN json_valid(c.attachments_json) THEN json_array_length(c.attachments_json) ELSE 0 END), 0) \
                           FROM feedback_comments c WHERE c.feedback_id = ?1 AND c.attachments_json IS NOT NULL) \
             WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("ticket reconcile counts: {e}"))?;
        Ok(())
    })
}

/// [`reconcile_ticket_with`] for every moved ticket whose row count and
/// `feedback_comments` disagree (an older release wrote there). Returns how
/// many tickets it brought up to date.
pub fn reconcile_all() -> Result<usize, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT f.id FROM feedback f WHERE f.comments_in_thread = 1 \
                 AND f.comment_count <> (SELECT COUNT(*) FROM feedback_comments c WHERE c.feedback_id = f.id)",
            )
            .map_err(|e| format!("ticket reconcile: {e}"))?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| format!("ticket reconcile: {e}"))?;
        rows.collect::<Result<_, _>>().map_err(|e| format!("ticket reconcile row: {e}"))?
    };
    let mut n = 0usize;
    for id in &ids {
        match reconcile_ticket_with(&conn, id) {
            Ok(_) => n += 1,
            Err(e) => crate::log_debug!("[feedback/thread] WARN ticket {id} not reconciled: {e}"),
        }
    }
    Ok(n)
}

/// Rollback safety for 0.46.17 only (Rosson's keep-the-old-path rule):
/// every ticket message written to the Thread is also written to
/// `feedback_comments` under the same id, so putting 0.46.16 back loses
/// nothing. The Thread is the source of truth for reads. REMOVE in 0.46.18
/// with [`reconcile_ticket_with`].
pub(crate) fn dual_write(conn: &Connection, ticket_id: &str, item: &OverlayItem) -> Result<(), String> {
    let files = if item.doc.attachments.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&item.doc.attachments).map_err(|e| format!("attachments serialize: {e}"))?)
    };
    conn.execute(
        "INSERT OR IGNORE INTO feedback_comments (id, feedback_id, author, body, created_at, attachments_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![item.id, ticket_id, item.doc.from, item.doc.body.clone().unwrap_or_default(), item.doc.created_at, files],
    )
    .map_err(|e| format!("ticket dual-write: {e}"))?;
    Ok(())
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
        match reconcile_all() {
            Ok(0) => {}
            Ok(n) => crate::log_debug!("[feedback/thread] {n} tickets brought up to rows an older release wrote"),
            Err(e) => crate::log_debug!("[feedback/thread] WARN reconcile: {e}"),
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
/// counts drop by one message and its files, and its dual-written
/// `feedback_comments` row goes too (a deleted message leaves no copy).
/// Returns the ticket id, or `None` when `conversation_id` isn't a ticket's.
pub fn note_deleted(conn: &Connection, conversation_id: &str, doc_id: &str, files: usize) -> Result<Option<String>, String> {
    let Some(tid) = ticket::ticket_of(conversation_id) else {
        return Ok(None);
    };
    conn.execute("DELETE FROM feedback_comments WHERE id = ?1 AND feedback_id = ?2", params![doc_id, tid])
        .map_err(|e| format!("ticket delete copy: {e}"))?;
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
