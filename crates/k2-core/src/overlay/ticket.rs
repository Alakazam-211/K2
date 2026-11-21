//! ticket-thread-v1 (0.46.17): a ticket's discussion is the Thread
//! conversation `ticket:<id>`.
//!
//! The overlay half: the key, the write of one ticket message (a Thread
//! text doc carrying `ticket: <id>`, under the overlay write lock, with an
//! optional send-once key), the backfill's import of a ticket's old
//! `feedback_comments` rows, and the purge the daily sweep runs. The ticket
//! rules (who may post, status moves, events, delivery) live in
//! `crate::feedback` / `crate::feedback_thread` and the daemon; this module
//! only stores.

use rusqlite::Connection;

use super::{catalog, store, ClientMsgKey, OverlayDoc, OverlayItem, OverlayLink, PostAuthor, ThreadAttachment, ThreadPost, WRITE};

/// The key prefix of a ticket's discussion.
pub const TICKET_PREFIX: &str = "ticket:";

/// `via` on every ticket message doc.
pub const VIA_TICKET: &str = "ticket";

/// `ticket:<id>` for ticket `id` (a FULL id).
pub fn conversation_id(ticket_id: &str) -> String {
    format!("{TICKET_PREFIX}{}", ticket_id.trim())
}

/// The ticket id a `ticket:<id>` key names (`None` for any other key, or a
/// bare prefix).
pub fn ticket_of(conversation_id: &str) -> Option<&str> {
    conversation_id
        .trim()
        .strip_prefix(TICKET_PREFIX)
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// Is this key a ticket's discussion?
pub fn is_ticket(conversation_id: &str) -> bool {
    ticket_of(conversation_id).is_some()
}

/// One ticket message to store.
#[derive(Debug, Clone)]
pub struct TicketMessage<'a> {
    pub ticket_id: &'a str,
    /// The ticket's workspace (the catalog row's project).
    pub project_id: &'a str,
    /// The stored author name: what `show.comments[].author` reads.
    pub from: &'a str,
    /// Who sent it from the credential (a person, a guest, an agent), or
    /// none for K2's own notes and agent names from the old CLI path.
    pub author: Option<PostAuthor>,
    /// Redacted on store (as every Thread text).
    pub body: &'a str,
    pub attachments: Vec<ThreadAttachment>,
}

/// The doc a ticket message is stored as.
fn message_doc(id: String, m: &TicketMessage<'_>) -> OverlayDoc {
    let mut doc = OverlayDoc::text(id, m.from.trim().to_string(), conversation_id(m.ticket_id), m.body.to_string(), VIA_TICKET);
    doc.author = m.author.clone();
    doc.attachments = m.attachments.clone();
    doc.ticket = Some(m.ticket_id.trim().to_string());
    doc
}

/// Store one ticket message as the next Thread item of `ticket:<id>`.
/// `conn` is the caller's (it holds the shared DB lock, and may be inside
/// its own transaction); the catalog row is made on the first message.
/// With `client`, a key already kept answers [`ThreadPost::Duplicate`] and
/// stores nothing (post-idempotency-v1, as a Thread post).
pub fn post(conn: &Connection, m: &TicketMessage<'_>, client: Option<&ClientMsgKey>) -> Result<ThreadPost, String> {
    super::refuse_system_author(m.author.as_ref())?;
    if m.ticket_id.trim().is_empty() {
        return Err("ticket message: ticket id required".to_string());
    }
    let conv = conversation_id(m.ticket_id);
    let _guard = WRITE.lock();
    if let Some(c) = client {
        if let Some(hit) = store::client_id_get(&c.key)? {
            return Ok(ThreadPost::Duplicate(hit));
        }
    }
    let seq = catalog::next_thread_seq(conn, &conv, m.project_id)?;
    let mut doc = message_doc(uuid::Uuid::new_v4().to_string(), m);
    doc.client_msg_id = client.map(|c| c.client_msg_id.clone());
    let write = client.map(|c| store::ClientIdWrite { key: &c.key, at_ms: c.at_ms, body_hash: &c.body_hash });
    store::commit_write_keyed(&doc, Some((conv.as_str(), seq)), &[], None, write)?;
    let id = doc.id.clone();
    let links = vec![OverlayLink { collection: "thread", conversation_id: Some(conv.clone()), seq, id: id.clone() }];
    Ok(ThreadPost::Posted(OverlayItem { collection: "thread".to_string(), seq, id, doc, conversation_id: Some(conv) }, links))
}

/// One old `feedback_comments` row, for [`import_comments`].
#[derive(Debug, Clone)]
pub struct OldComment<'a> {
    pub comment_id: &'a str,
    pub message: TicketMessage<'a>,
    pub created_at: i64,
    /// The imported stamp (who and when, from the old row).
    pub imported: Option<super::ImportedMeta>,
}

/// The backfill's import key of an old comment (`ticket/<comment id>`):
/// a re-run never adds a line twice.
pub fn import_key(comment_id: &str) -> String {
    format!("ticket/{}", comment_id.trim())
}

/// Move a ticket's old comments into `ticket:<id>` in ONE redb write plus
/// one SQLite savepoint, under the overlay write lock: the docs (id = the
/// comment id, so an attachment's address stays ticket + comment + index),
/// their pointers in comment order, the import keys, the catalog row and
/// `mark` (the caller's own SQL, e.g. the ticket's `comments_in_thread`)
/// commit together or not at all. Returns how many lines were written.
pub fn import_comments(
    conn: &Connection,
    ticket_id: &str,
    project_id: &str,
    comments: &[OldComment<'_>],
    mark: impl FnOnce(&Connection) -> Result<(), String>,
) -> Result<usize, String> {
    let conv = conversation_id(ticket_id);
    let entries: Vec<(String, OverlayDoc)> = comments
        .iter()
        .map(|c| {
            let mut doc = message_doc(c.comment_id.to_string(), &c.message);
            doc.created_at = c.created_at;
            doc.imported = c.imported.clone();
            (import_key(c.comment_id), doc)
        })
        .collect();
    let _guard = WRITE.lock();
    conn.execute_batch("SAVEPOINT ticket_import").map_err(|e| format!("ticket import savepoint: {e}"))?;
    let res = (|| -> Result<usize, String> {
        catalog::ensure_conversation(conn, &conv, project_id)?;
        let start_after = catalog::get(conn, &conv)?.map(|r| r.1).unwrap_or(0);
        let w = store::import_docs(&conv, start_after, &entries, None, |w| {
            conn.execute(
                "UPDATE overlay_conversations SET last_thread_seq = MAX(last_thread_seq, ?2) WHERE conversation_id = ?1",
                rusqlite::params![conv, w.last_seq],
            )
            .map_err(|e| format!("ticket import catalog: {e}"))?;
            mark(conn)
        })?;
        Ok(w.written)
    })();
    match res {
        Ok(n) => {
            conn.execute_batch("RELEASE ticket_import").map_err(|e| format!("ticket import release: {e}"))?;
            Ok(n)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO ticket_import; RELEASE ticket_import");
            Err(e)
        }
    }
}

/// Every message of `ticket:<id>`, in order (deleted ones included, as
/// tombstones).
pub fn read_messages(ticket_id: &str) -> Result<Vec<OverlayItem>, String> {
    store::read_thread(&conversation_id(ticket_id), 0)
}

/// Remove a ticket's discussion from the Thread store (its catalog row is
/// the caller's). Returns how many messages went.
pub fn purge(conversation_id: &str) -> Result<usize, String> {
    let _guard = WRITE.lock();
    store::purge_thread(conversation_id)
}

/// The `ticket:` conversations that still hold messages in the store.
pub fn stored_conversations() -> Result<Vec<String>, String> {
    store::thread_conversations_with_prefix(TICKET_PREFIX)
}
