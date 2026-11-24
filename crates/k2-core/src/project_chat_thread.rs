//! Project chats on the Thread store (project-chat-thread-v1, migration
//! 0331): the project side of `project:<group id>`.
//!
//! - **One write path**: every project line, from every door (the old
//!   `/cli/project-group/msg`, an App's `project.post`, a Thread post to
//!   `project:<id>`), is [`post_line`]: a Thread text doc of
//!   `project:<id>` (`via: "project"`, body redacted, the author stamp from
//!   the line's kind, [`crate::overlay::project::stamp_of`]).
//! - **Reads**: the old routes are views over the Thread ([`list_page`],
//!   [`list_delta`], [`list_before`]), in the old `ProjectGroupMessage`
//!   shape; a deleted line (a tombstone) is left out of `messages`, and a
//!   delta lists it in `removed`. A project not moved yet reads its old
//!   rows (`crate::project_groups`).
//! - **Moves**: a project from before this release keeps its old
//!   `project_group_messages` rows until [`move_group_with`] imports them
//!   (import key `project/<message id>`, doc id = the message id, the time
//!   and send-once id kept) and sets `messages_in_thread`, in one
//!   savepoint. The lines are NOT marked imported (an imported line is
//!   history: never unread, never deletable), so nobody's unread count
//!   changes on upgrade. **Seqs are kept**: old seqs are 1..n with no gap
//!   and an import numbers lines `last + 1`, so seq n stays seq n and every
//!   read cursor, phone cache and push `seq` stays right. The move checks
//!   it line by line; on a mismatch (a hand-trimmed database) it moves the
//!   project's read cursors to the new seqs and keeps the head past every
//!   old seq ([`check_seqs`]), and logs it. The background backfill
//!   ([`spawn_backfill_and_sweep`], after `ready`) moves every project; a
//!   project's next line moves it first if the backfill hasn't yet.
//! - **Rollback window** (the keep-the-old-path rule). THE ONE PLACE THAT
//!   NAMES THE RELEASES: lands in 0.46.19; the dual write and
//!   [`reconcile_all`] are removed in 0.46.20; the old tables are dropped
//!   in 0.46.21. In the release this lands in, every line is also written
//!   to `project_group_messages` under the same id and seq, with
//!   `last_msg_seq = seq`, and its send-once key to
//!   `project_group_client_ids` ([`dual_write`]), so the release before it
//!   put back reads a complete chat with correct cursors and its next post
//!   takes the next seq. A delete removes the copy ([`note_deleted`]).
//!   After an upgrade again, [`reconcile_all`] imports what the older
//!   release wrote (those rows continue the same seq run). Reads always
//!   come from the Thread.
//! - **Lifetime**: the chat lives as long as the project, never its
//!   members or point of contact. Deleting the project removes the catalog
//!   row (migration 0331's trigger, same transaction) and then the stored
//!   lines ([`purge_group`]); the daily [`sweep_orphans`] removes, and
//!   logs, any project conversation whose project is gone.
//! - **Live**: every stored line is handed to the post listener the daemon
//!   registers ([`set_post_listener`]: the overlay socket frames).

use rusqlite::{params, Connection, OptionalExtension};

use crate::overlay::project::{self as kp, OldLine, ProjectLine};
use crate::overlay::{ClientMsgKey, OverlayItem, OverlayLink, ThreadAttachment, ThreadPost};
use crate::project_groups::{MessagePage, ProjectGroupMessage, MESSAGES_AFTER_LIMIT, MESSAGES_DEFAULT_LIMIT};

/// The `/boot-status` feature key: project chats are Thread conversations
/// `project:<group id>` (older servers keep only the project routes).
pub const FEATURE: &str = "project-chat-thread-v1";

static POST_LISTENER: std::sync::OnceLock<fn(&OverlayItem, &[OverlayLink])> = std::sync::OnceLock::new();

/// Register the listener every stored project line is handed to (first
/// call wins). k2-core has no event bus; the daemon's emits the frames.
pub fn set_post_listener(f: fn(&OverlayItem, &[OverlayLink])) {
    let _ = POST_LISTENER.set(f);
}

fn notify_posted(item: &OverlayItem, links: &[OverlayLink]) {
    if let Some(f) = POST_LISTENER.get() {
        f(item, links);
    }
}

/// `messages_in_thread` of project `id`, `None` when there is no such
/// project.
pub fn group_moved(conn: &Connection, id: &str) -> Result<Option<bool>, String> {
    conn.query_row("SELECT messages_in_thread FROM project_groups WHERE id = ?1", params![id], |r| r.get::<_, i64>(0))
        .optional()
        .map(|o| o.map(|v| v != 0))
        .map_err(|e| format!("project row: {e}"))
}

/// Are project `id`'s lines in the Thread store? False for an unknown id.
pub fn in_thread(id: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    match group_moved(&conn, id) {
        Ok(m) => m.unwrap_or(false),
        Err(e) => {
            crate::log_debug!("[project/thread] {id}: {e}");
            false
        }
    }
}

/// A Thread item of `project:<id>` as the line the project routes return.
pub fn message_of(group_id: &str, item: &OverlayItem) -> ProjectGroupMessage {
    ProjectGroupMessage {
        id: item.id.clone(),
        group_id: group_id.to_string(),
        author: item.doc.from.clone(),
        body: item.doc.body.clone().unwrap_or_default(),
        created_at: item.doc.created_at,
        seq: item.seq,
        author_kind: kp::kind_of(item.doc.author.as_ref()).to_string(),
        client_msg_id: item.doc.client_msg_id.clone(),
        attachments: item.doc.attachments.clone(),
    }
}

// ── the write path ───────────────────────────────────────────────────

/// A line's send-once key (post-idempotency-v1): the sender's namespace
/// from its credential (`p:<person>`, `g:<guest>`, `a:<workspace>`), the
/// client's id, the clock, and the hash of what was sent.
#[derive(Debug, Clone, Copy)]
pub struct LineClient<'a> {
    pub ns: &'a str,
    pub client_msg_id: &'a str,
    pub at_ms: i64,
    pub body_hash: &'a str,
}

/// What [`post_line`] did.
#[derive(Debug, Clone)]
pub enum LineOutcome {
    /// Stored (and handed to the post listener).
    Posted(OverlayItem),
    /// The key was kept already for this project: nothing was written.
    Duplicate { item: OverlayItem, mismatch: bool },
    /// The key names a line in ANOTHER chat (409 `client_msg_id_reused`).
    ReusedElsewhere,
}

/// One project line to write.
#[derive(Debug, Clone)]
pub struct NewLine<'a> {
    pub group_id: &'a str,
    pub author: &'a str,
    /// `person` | `agent` | `guest`.
    pub author_kind: &'a str,
    pub body: &'a str,
    pub attachments: Vec<ThreadAttachment>,
    pub client: Option<LineClient<'a>>,
}

/// The Thread send-once key of a project line: `<ns>/<id>`, the same key a
/// Thread post makes, so a retry through either door finds it.
pub fn thread_key(ns: &str, client_msg_id: &str) -> String {
    format!("{ns}/{client_msg_id}")
}

/// Store one project line (the one write path, see the module docs). A
/// project still on its old rows moves first, under the same DB lock. The
/// caller has resolved and admitted the author.
pub fn post_line(l: &NewLine<'_>) -> Result<LineOutcome, String> {
    let author = l.author.trim();
    let body = l.body.trim();
    if author.is_empty() {
        return Err("usage: author must not be empty".to_string());
    }
    if body.is_empty() && l.attachments.is_empty() {
        return Err("usage: message body must not be empty".to_string());
    }
    let conv = kp::conversation_id(l.group_id);
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(moved) = group_moved(&conn, l.group_id)? else {
        return Err(format!("no project group with id {}", l.group_id));
    };
    if !moved {
        move_group_with(&conn, l.group_id)?;
    }
    if let Some(c) = &l.client {
        // A key kept before the move (by the older release, inside its
        // 24 h) is in the old table; its line was moved under its own id.
        if let Some(out) = legacy_key_hit(&conn, l.group_id, &conv, c, body)? {
            return Ok(out);
        }
    }
    let key = l.client.map(|c| ClientMsgKey {
        key: thread_key(c.ns, c.client_msg_id),
        client_msg_id: c.client_msg_id.to_string(),
        body_hash: c.body_hash.to_string(),
        at_ms: c.at_ms,
    });
    let line = ProjectLine { group_id: l.group_id, from: author, author_kind: l.author_kind, body, attachments: l.attachments.clone() };
    let (item, links) = match kp::post(&conn, &line, key.as_ref())? {
        ThreadPost::Posted(item, links) => (item, links),
        ThreadPost::Duplicate(hit) => {
            if hit.conversation_id != conv {
                return Ok(LineOutcome::ReusedElsewhere);
            }
            return Ok(match crate::overlay::find_thread_item(&conv, &hit.doc_id)? {
                Some(item) => LineOutcome::Duplicate { mismatch: l.client.is_some_and(|c| c.body_hash != hit.body_hash), item },
                None => LineOutcome::ReusedElsewhere,
            });
        }
    };
    // Rollback copy (see the module docs: removed one release after this
    // lands).
    dual_write(&conn, l.group_id, &item, l.client.as_ref())?;
    drop(conn);
    notify_posted(&item, &links);
    Ok(LineOutcome::Posted(item))
}

/// A send-once key found in `project_group_client_ids` (kept by an older
/// release, or dual-written): `Some` outcome when it names a line.
fn legacy_key_hit(conn: &Connection, group_id: &str, conv: &str, c: &LineClient<'_>, body: &str) -> Result<Option<LineOutcome>, String> {
    if !crate::project_groups::client_keys_ready(conn) {
        return Ok(None);
    }
    let hit: Option<(String, String)> = conn
        .query_row(
            "SELECT group_id, message_id FROM project_group_client_ids WHERE ns = ?1 AND client_msg_id = ?2",
            params![c.ns, c.client_msg_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("client key read failed: {e}"))?;
    let Some((hit_group, message_id)) = hit else { return Ok(None) };
    if hit_group != group_id {
        return Ok(Some(LineOutcome::ReusedElsewhere));
    }
    // A dual-written key also has its Thread key (with the hash): that
    // one answers, below.
    if crate::overlay::client_msg_lookup(&thread_key(c.ns, c.client_msg_id))?.is_some() {
        return Ok(None);
    }
    Ok(crate::overlay::find_thread_item(conv, &message_id)?.map(|item| {
        let mismatch = item.doc.body.as_deref() != Some(crate::redaction::redact(body).as_str());
        LineOutcome::Duplicate { item, mismatch }
    }))
}

/// Rollback copy, for the release this lands in only (see the module
/// docs): the line in `project_group_messages` under the same id and seq,
/// `last_msg_seq` moved to it, and its key in `project_group_client_ids`.
/// The body is the stored (redacted) text; a line with files only names
/// them (the old table holds text).
pub(crate) fn dual_write(conn: &Connection, group_id: &str, item: &OverlayItem, client: Option<&LineClient<'_>>) -> Result<(), String> {
    let mut body = item.doc.body.clone().unwrap_or_default();
    if body.trim().is_empty() {
        body = item.doc.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join("\n");
    }
    conn.execute(
        "INSERT OR IGNORE INTO project_group_messages (id, group_id, author, body, created_at, seq, author_kind, client_msg_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            item.id,
            group_id,
            item.doc.from,
            body,
            item.doc.created_at,
            item.seq,
            kp::kind_of(item.doc.author.as_ref()),
            item.doc.client_msg_id
        ],
    )
    .map_err(|e| format!("project dual-write: {e}"))?;
    conn.execute("UPDATE project_groups SET last_msg_seq = MAX(last_msg_seq, ?2) WHERE id = ?1", params![group_id, item.seq])
        .map_err(|e| format!("project dual-write seq: {e}"))?;
    if let Some(c) = client {
        if crate::project_groups::client_keys_ready(conn) {
            conn.execute(
                "INSERT OR IGNORE INTO project_group_client_ids (ns, client_msg_id, group_id, message_id, created_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![c.ns, c.client_msg_id, group_id, item.id, c.at_ms],
            )
            .map_err(|e| format!("project dual-write key: {e}"))?;
        }
    }
    Ok(())
}

/// A line of a project's chat was deleted (a tombstone): its rollback copy
/// and its kept key go too (a deleted line leaves no copy). Returns the
/// group id, or `None` when `conversation_id` isn't a project's.
pub fn note_deleted(conn: &Connection, conversation_id: &str, doc_id: &str) -> Result<Option<String>, String> {
    let Some(gid) = kp::group_of(conversation_id) else {
        return Ok(None);
    };
    conn.execute("DELETE FROM project_group_messages WHERE id = ?1 AND group_id = ?2", params![doc_id, gid])
        .map_err(|e| format!("project delete copy: {e}"))?;
    if crate::project_groups::client_keys_ready(conn) {
        conn.execute("DELETE FROM project_group_client_ids WHERE message_id = ?1 AND group_id = ?2", params![doc_id, gid])
            .map_err(|e| format!("project delete key: {e}"))?;
    }
    Ok(Some(gid.to_string()))
}

// ── the move ─────────────────────────────────────────────────────────

/// One old row: (id, author, body, created_at, seq, author_kind, client_msg_id).
type OldRow = (String, String, String, i64, i64, String, Option<String>);

fn old_rows(conn: &Connection, id: &str, after_seq: i64) -> Result<Vec<OldRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, author, body, created_at, seq, author_kind, client_msg_id FROM project_group_messages \
             WHERE group_id = ?1 AND seq > ?2 ORDER BY seq ASC, rowid ASC",
        )
        .map_err(|e| format!("project move: {e}"))?;
    let mapped = stmt
        .query_map(params![id, after_seq], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
        .map_err(|e| format!("project move: {e}"))?;
    mapped.collect::<Result<_, _>>().map_err(|e| format!("project move row: {e}"))
}

fn import_rows(
    conn: &Connection,
    id: &str,
    rows: &[OldRow],
    mark: impl FnOnce(&Connection) -> Result<(), String>,
) -> Result<usize, String> {
    let lines: Vec<OldLine<'_>> = rows
        .iter()
        .map(|(mid, author, body, at, _, kind, client)| OldLine {
            message_id: mid,
            created_at: *at,
            client_msg_id: client.as_deref(),
            line: ProjectLine { group_id: id, from: author, author_kind: kind, body, attachments: Vec::new() },
        })
        .collect();
    let w = kp::import_lines(conn, id, &lines, |c, _| mark(c))?;
    check_seqs(conn, id, rows)?;
    Ok(w.written)
}

/// The seq check after a move: each moved line's Thread seq must equal its
/// old seq. On a mismatch, every read cursor of the project moves to the
/// new seq of the highest old seq at or below it, and the head is kept past
/// every old seq (so the next line, and its rollback copy, never reuses an
/// old seq). Returns whether it had to remap.
fn check_seqs(conn: &Connection, id: &str, rows: &[OldRow]) -> Result<bool, String> {
    let conv = kp::conversation_id(id);
    let mut pairs: Vec<(i64, i64)> = Vec::with_capacity(rows.len());
    for r in rows {
        let new = crate::overlay::store::thread_seq_of(&conv, &r.0)?.ok_or_else(|| format!("project move: line {} missing after its move", r.0))?;
        pairs.push((r.4, new));
    }
    if pairs.iter().all(|(old, new)| old == new) {
        return Ok(false);
    }
    crate::log_debug!("[project/thread] WARN project {id}: moved seqs differ from the old ones; read cursors remapped");
    pairs.sort();
    let cursors: Vec<(String, i64)> = {
        let mut stmt = conn
            .prepare("SELECT person, last_read_seq FROM read_cursors WHERE kind = 'project' AND conv_id = ?1")
            .map_err(|e| format!("project remap: {e}"))?;
        let rows = stmt.query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| format!("project remap: {e}"))?;
        rows.collect::<Result<_, _>>().map_err(|e| format!("project remap row: {e}"))?
    };
    for (person, at) in cursors {
        let to = pairs.iter().filter(|(old, _)| *old <= at).map(|(_, new)| *new).max().unwrap_or(0);
        conn.execute(
            "UPDATE read_cursors SET last_read_seq = ?3 WHERE person = ?1 AND kind = 'project' AND conv_id = ?2",
            params![person, id, to],
        )
        .map_err(|e| format!("project remap: {e}"))?;
    }
    conn.execute(
        "UPDATE overlay_conversations SET last_thread_seq = MAX(last_thread_seq, (SELECT last_msg_seq FROM project_groups WHERE id = ?2)) \
         WHERE conversation_id = ?1",
        params![conv, id],
    )
    .map_err(|e| format!("project remap head: {e}"))?;
    Ok(true)
}

/// Move project `id`'s old rows into `project:<id>` and set its
/// `messages_in_thread`, all or nothing (see the module docs). The caller
/// holds the shared DB lock (`conn`). A project already moved is left
/// alone. Returns the lines written.
pub fn move_group_with(conn: &Connection, id: &str) -> Result<usize, String> {
    let Some(moved) = group_moved(conn, id)? else {
        return Err(format!("no project group with id {id}"));
    };
    if moved {
        return Ok(0);
    }
    let rows = old_rows(conn, id, 0)?;
    import_rows(conn, id, &rows, |c| {
        c.execute("UPDATE project_groups SET messages_in_thread = 1 WHERE id = ?1", params![id])
            .map_err(|e| format!("project move mark: {e}"))?;
        Ok(())
    })
}

/// [`move_group_with`] on the shared connection.
pub fn move_group(id: &str) -> Result<usize, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    move_group_with(&conn, id)
}

/// After a rollback and an upgrade again: bring a moved project's Thread up
/// to the rows the older release wrote (seq past the Thread's head; rows
/// already in the Thread are skipped). Returns the lines written. Remove
/// with the dual write.
pub fn reconcile_group_with(conn: &Connection, id: &str) -> Result<usize, String> {
    if group_moved(conn, id)? != Some(true) {
        return Ok(0);
    }
    let conv = kp::conversation_id(id);
    let head = crate::overlay::catalog::get(conn, &conv)?.map(|r| r.1).unwrap_or(0);
    let mut missing = Vec::new();
    for row in old_rows(conn, id, head)? {
        if crate::overlay::store::thread_seq_of(&conv, &row.0)?.is_none() {
            missing.push(row);
        }
    }
    if missing.is_empty() {
        return Ok(0);
    }
    import_rows(conn, id, &missing, |_| Ok(()))
}

/// [`reconcile_group_with`] for every moved project whose old head is past
/// its Thread's (an older release wrote there). Returns how many projects
/// it brought up to date.
pub fn reconcile_all() -> Result<usize, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT g.id FROM project_groups g LEFT JOIN overlay_conversations c \
                 ON c.kind = 'project' AND c.group_id = g.id \
                 WHERE g.messages_in_thread = 1 AND g.last_msg_seq > COALESCE(c.last_thread_seq, 0)",
            )
            .map_err(|e| format!("project reconcile: {e}"))?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| format!("project reconcile: {e}"))?;
        rows.collect::<Result<_, _>>().map_err(|e| format!("project reconcile row: {e}"))?
    };
    let mut n = 0usize;
    for id in &ids {
        match reconcile_group_with(&conn, id) {
            Ok(_) => n += 1,
            Err(e) => crate::log_debug!("[project/thread] WARN project {id} not reconciled: {e}"),
        }
    }
    Ok(n)
}

/// Projects per backfill batch.
const BACKFILL_BATCH: usize = 50;

/// One backfill batch: move up to `limit` projects with ids after `after`
/// that are still on their old rows. Returns the last id it looked at
/// (`None` = nothing left). A project that fails is logged and skipped
/// this run (the next run, or its next line, tries again).
pub fn backfill_step(after: &str, limit: usize) -> Result<Option<String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM project_groups WHERE messages_in_thread = 0 AND id > ?1 ORDER BY id LIMIT ?2")
            .map_err(|e| format!("project backfill: {e}"))?;
        let rows = stmt
            .query_map(params![after, limit as i64], |r| r.get::<_, String>(0))
            .map_err(|e| format!("project backfill: {e}"))?;
        rows.collect::<Result<_, _>>().map_err(|e| format!("project backfill row: {e}"))?
    };
    for id in &ids {
        if let Err(e) = move_group_with(&conn, id) {
            crate::log_debug!("[project/thread] WARN project {id} stays on its old rows for now: {e}");
        }
    }
    Ok(ids.last().cloned())
}

/// Move every project still on its old rows (one batch per lock). Returns
/// how many batches ran.
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

// ── lifetime ─────────────────────────────────────────────────────────

/// After a project's DELETE committed (the trigger took its catalog row):
/// its stored lines go. A failure is logged; the daily sweep retries.
pub fn purge_group(group_id: &str) {
    let conv = kp::conversation_id(group_id);
    match kp::purge(&conv) {
        Ok(n) => crate::log_debug!("[project/thread] {conv} removed with its project ({n} lines)"),
        Err(e) => crate::log_debug!("[project/thread] WARN {conv} stays until the sweep: {e}"),
    }
}

/// What one [`sweep_orphans`] removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub conversations: Vec<String>,
    pub messages: usize,
}

/// Remove every project conversation whose project is gone: catalog rows
/// whose project is missing, and stored `project:` lines with neither a
/// project nor a catalog row (a delete that crashed before its purge).
/// Their read cursors go too. Each removal is logged.
pub fn sweep_orphans() -> Result<SweepReport, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut orphans: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT conversation_id FROM overlay_conversations WHERE kind = 'project' \
                 AND group_id NOT IN (SELECT id FROM project_groups) ORDER BY conversation_id",
            )
            .map_err(|e| format!("project sweep: {e}"))?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| format!("project sweep: {e}"))?;
        rows.collect::<Result<_, _>>().map_err(|e| format!("project sweep row: {e}"))?
    };
    for conv in kp::stored_conversations()? {
        if orphans.contains(&conv) {
            continue;
        }
        let Some(gid) = kp::group_of(&conv) else { continue };
        let group_exists: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM project_groups WHERE id = ?1", params![gid], |r| r.get(0))
            .map_err(|e| format!("project sweep: {e}"))?;
        let catalogued: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM overlay_conversations WHERE conversation_id = ?1", params![conv], |r| r.get(0))
            .map_err(|e| format!("project sweep: {e}"))?;
        if !group_exists && !catalogued {
            orphans.push(conv);
        }
    }
    let mut report = SweepReport::default();
    for conv in orphans {
        let n = kp::purge(&conv)?;
        conn.execute("DELETE FROM overlay_conversations WHERE conversation_id = ?1 AND kind = 'project'", params![conv])
            .map_err(|e| format!("project sweep delete: {e}"))?;
        crate::read_state::delete_conversation(&conn, crate::read_state::KIND_THREAD, &conv)?;
        if let Some(gid) = kp::group_of(&conv) {
            crate::read_state::delete_conversation(&conn, crate::read_state::KIND_PROJECT, gid)?;
        }
        crate::log_debug!("[project/thread] swept {conv}: its project is gone ({n} lines removed)");
        report.messages += n;
        report.conversations.push(conv);
    }
    Ok(report)
}

/// How often the sweep runs.
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// The background backfill, the reconcile, then the daily sweep, on one
/// thread. Called by the daemon after `ready`; returns at once.
pub fn spawn_backfill_and_sweep() {
    let spawned = std::thread::Builder::new().name("k2-project-threads".into()).spawn(|| {
        match backfill_all() {
            Ok(0) => {}
            Ok(n) => crate::log_debug!("[project/thread] backfill done: {n} batches"),
            Err(e) => crate::log_debug!("[project/thread] WARN backfill stopped (projects read their old rows meanwhile): {e}"),
        }
        match reconcile_all() {
            Ok(0) => {}
            Ok(n) => crate::log_debug!("[project/thread] {n} projects brought up to rows an older release wrote"),
            Err(e) => crate::log_debug!("[project/thread] WARN reconcile: {e}"),
        }
        loop {
            match sweep_orphans() {
                Ok(r) if r.conversations.is_empty() => {}
                Ok(r) => crate::log_debug!("[project/thread] sweep removed {} project conversations", r.conversations.len()),
                Err(e) => crate::log_debug!("[project/thread] WARN sweep: {e}"),
            }
            std::thread::sleep(SWEEP_EVERY);
        }
    });
    if let Err(e) = spawned {
        crate::log_debug!("[project/thread] WARN backfill thread: {e}");
    }
}

// ── the read views ───────────────────────────────────────────────────

/// Pointers read per store page while walking a chat.
const WALK_PAGE: usize = 200;

fn effective(limit: Option<i64>, default: i64) -> Result<usize, String> {
    if let Some(l) = limit {
        if l < 1 {
            return Err("usage: limit must be >= 1".to_string());
        }
    }
    Ok(limit.unwrap_or(default).min(MESSAGES_AFTER_LIMIT) as usize)
}

/// Up to `want` live (not deleted) items with seq < `before` (none = the
/// end), the newest of them, oldest first.
fn newest_live(conv: &str, mut before: Option<i64>, want: usize) -> Result<Vec<OverlayItem>, String> {
    let mut out: Vec<OverlayItem> = Vec::new();
    loop {
        let page = crate::overlay::store::read_thread_page(conv, None, before, WALK_PAGE)?;
        let first = page.items.first().map(|i| i.seq);
        for item in page.items.into_iter().rev() {
            if !item.doc.deleted {
                out.push(item);
                if out.len() >= want {
                    out.reverse();
                    return Ok(out);
                }
            }
        }
        match (page.has_more, first) {
            (true, Some(s)) => before = Some(s),
            _ => break,
        }
    }
    out.reverse();
    Ok(out)
}

/// The old `list_messages` over the Thread: with `after` (unix s), the
/// lines with `created_at > after`, oldest first, at most `limit`
/// (default and max 500); without, the latest `limit` (default 20).
/// `truncated` = more lines matched than the page holds.
pub fn list_page(group_id: &str, after: Option<i64>, limit: Option<i64>) -> Result<MessagePage, String> {
    let conv = kp::conversation_id(group_id);
    let to_page = |items: Vec<OverlayItem>, eff: usize, from_newest: bool| {
        let truncated = items.len() > eff;
        let mut items = items;
        if truncated {
            if from_newest {
                items.drain(..items.len() - eff);
            } else {
                items.truncate(eff);
            }
        }
        MessagePage { messages: items.iter().map(|i| message_of(group_id, i)).collect(), truncated }
    };
    match after {
        None => {
            let eff = effective(limit, MESSAGES_DEFAULT_LIMIT)?;
            Ok(to_page(newest_live(&conv, None, eff + 1)?, eff, true))
        }
        Some(ts) => {
            let eff = effective(limit, MESSAGES_AFTER_LIMIT)?;
            // The seq at or below which every line is at or before `ts`
            // (lines are stored in time order): walk back a page at a time.
            let mut start = 0i64;
            let mut before = None;
            loop {
                let page = crate::overlay::store::read_thread_page(&conv, None, before, WALK_PAGE)?;
                if let Some(last_old) = page.items.iter().rev().find(|i| i.doc.created_at <= ts) {
                    start = last_old.seq;
                    break;
                }
                match (page.has_more, page.items.first()) {
                    (true, Some(i)) => before = Some(i.seq),
                    _ => break,
                }
            }
            let mut out: Vec<OverlayItem> = Vec::new();
            let mut after_seq = start;
            loop {
                let d = crate::overlay::store::read_thread_delta(&conv, after_seq, WALK_PAGE)?;
                let last = d.items.last().map(|i| i.seq);
                out.extend(d.items.into_iter().filter(|i| !i.doc.deleted && i.doc.created_at > ts));
                if out.len() > eff {
                    break;
                }
                match (d.has_more, last) {
                    (true, Some(s)) => after_seq = s,
                    _ => break,
                }
            }
            Ok(to_page(out, eff, false))
        }
    }
}

/// The old `before_seq` page over the Thread: the newest `limit` lines with
/// seq < `before_seq`, oldest first; `truncated` = older lines remain.
pub fn list_before(group_id: &str, before_seq: i64, limit: Option<i64>) -> Result<MessagePage, String> {
    if before_seq < 1 {
        return Err("usage: before_seq must be >= 1".to_string());
    }
    let eff = effective(limit, MESSAGES_DEFAULT_LIMIT)?;
    let mut items = newest_live(&kp::conversation_id(group_id), Some(before_seq), eff + 1)?;
    let truncated = items.len() > eff;
    if truncated {
        items.remove(0);
    }
    Ok(MessagePage { messages: items.iter().map(|i| message_of(group_id, i)).collect(), truncated })
}

/// A project chat delta (thread-delta-v1): new lines, lines below the
/// cursor that changed in place, and lines that were deleted.
#[derive(Debug, Clone, Default)]
pub struct MessageDelta {
    /// Live lines with seq > `after_seq`, oldest first, at most `limit`.
    pub messages: Vec<ProjectGroupMessage>,
    /// Newer lines remain past the page.
    pub truncated: bool,
    /// Live lines at or below `after_seq` that changed since.
    pub changed: Vec<ProjectGroupMessage>,
    /// `(seq, id)` of deleted lines (tombstones), at or below the cursor
    /// or in the page, plus any that left the Thread.
    pub removed: Vec<(i64, String)>,
}

/// The old `after_seq` read over the Thread.
pub fn list_delta(group_id: &str, after_seq: i64, limit: Option<i64>) -> Result<MessageDelta, String> {
    let eff = effective(limit, MESSAGES_DEFAULT_LIMIT)?;
    let d = crate::overlay::store::read_thread_delta(&kp::conversation_id(group_id), after_seq, eff)?;
    let mut out = MessageDelta { truncated: d.has_more, removed: d.removed, ..Default::default() };
    for i in d.items {
        if i.doc.deleted {
            out.removed.push((i.seq, i.id.clone()));
        } else {
            out.messages.push(message_of(group_id, &i));
        }
    }
    for i in d.changed {
        if i.doc.deleted {
            out.removed.push((i.seq, i.id.clone()));
        } else {
            out.changed.push(message_of(group_id, &i));
        }
    }
    out.removed.sort();
    out.removed.dedup();
    Ok(out)
}

/// A project chat's head for read state: its Thread's last seq, never
/// below the old `last_msg_seq` (an unmoved project, or the rollback
/// copy's own head).
pub fn head_sql() -> &'static str {
    "SELECT MAX(g.last_msg_seq, COALESCE((SELECT c.last_thread_seq FROM overlay_conversations c \
     WHERE c.kind = 'project' AND c.group_id = g.id), 0)) FROM project_groups g WHERE g.id = ?1"
}

#[cfg(test)]
#[path = "project_chat_thread_tests.rs"]
mod tests;
