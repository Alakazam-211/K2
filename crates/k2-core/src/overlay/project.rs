//! project-chat-thread-v1: a project's chat is the Thread conversation
//! `project:<group id>`.
//!
//! The overlay half: the key, the write of one project line (a Thread text
//! doc, `via: "project"`, under the overlay write lock, with an optional
//! send-once key), the backfill's import of a project's old
//! `project_group_messages` rows, and the purge the daily sweep and a
//! project delete run. The project rules (who may post, delivery to the
//! point of contact, pushes) live in `crate::project_chat_thread` and the
//! daemon; this module only stores.
//!
//! The conversation's catalog row has `kind = 'project'`, `group_id` = the
//! group id, and `project_id` = the group id as well: it is NOT a
//! workspace. The point of contact changes and its workspace can be
//! deleted; the chat stays with the project.

use rusqlite::Connection;

use super::doc::{AUTHOR_GUEST, AUTHOR_OWNER, AUTHOR_USER};
use super::{catalog, store, ClientMsgKey, OverlayDoc, OverlayItem, OverlayLink, PostAuthor, ThreadAttachment, ThreadPost, WRITE};

/// The key prefix of a project's chat.
pub const PROJECT_PREFIX: &str = "project:";

/// `via` on every project line doc.
pub const VIA_PROJECT: &str = "project";

/// `project:<group id>` for project `group_id` (a FULL id).
pub fn conversation_id(group_id: &str) -> String {
    format!("{PROJECT_PREFIX}{}", group_id.trim())
}

/// The group id a `project:<id>` key names (`None` for any other key, or
/// a bare prefix).
pub fn group_of(conversation_id: &str) -> Option<&str> {
    conversation_id
        .trim()
        .strip_prefix(PROJECT_PREFIX)
        .map(str::trim)
        .filter(|g| !g.is_empty())
}

/// Is this key a project's chat?
pub fn is_project(conversation_id: &str) -> bool {
    group_of(conversation_id).is_some()
}

/// `project_group_messages.author_kind` values (the same strings as
/// `crate::project_groups::AUTHOR_KIND_*`).
const KIND_PERSON: &str = "person";
const KIND_GUEST: &str = "guest";
const KIND_AGENT: &str = "agent";

/// The Thread author stamp of a project line written as `author` with
/// `author_kind` (§4.3 of the PRD):
/// - `person` / `owner` → the owner;
/// - `person` / a Connect login → that login (`user`);
/// - `guest` / `guest:<u>` → the App guest `<u>`;
/// - `agent` (a workspace's name) → none (an agent's line, as on agent
///   Threads; it can never be deleted).
pub fn stamp_of(author: &str, author_kind: &str) -> Option<PostAuthor> {
    let a = author.trim();
    match author_kind {
        KIND_PERSON if a == "owner" => Some(PostAuthor::owner()),
        KIND_PERSON => Some(PostAuthor::user(a)),
        KIND_GUEST => Some(PostAuthor::guest(a.strip_prefix("guest:").unwrap_or(a))),
        _ => None,
    }
}

/// The `author_kind` a stored line reads back as, from its stamp (none, or
/// any kind that isn't a person's or a guest's, is an agent's).
pub fn kind_of(author: Option<&PostAuthor>) -> &'static str {
    match author.map(|a| a.kind.as_str()) {
        Some(k) if k == AUTHOR_OWNER || k == AUTHOR_USER => KIND_PERSON,
        Some(k) if k == AUTHOR_GUEST => KIND_GUEST,
        _ => KIND_AGENT,
    }
}

/// One project line to store.
#[derive(Debug, Clone)]
pub struct ProjectLine<'a> {
    pub group_id: &'a str,
    /// The stored author name: what `messages[].author` reads (`owner`, a
    /// Connect login, `guest:<u>`, a workspace's name).
    pub from: &'a str,
    /// `person` | `agent` | `guest`: the stamp comes from it ([`stamp_of`]).
    pub author_kind: &'a str,
    /// Redacted on store (as every Thread text).
    pub body: &'a str,
    pub attachments: Vec<ThreadAttachment>,
}

/// The doc a project line is stored as.
fn line_doc(id: String, l: &ProjectLine<'_>) -> OverlayDoc {
    let mut doc = OverlayDoc::text(id, l.from.trim().to_string(), conversation_id(l.group_id), l.body.to_string(), VIA_PROJECT);
    doc.author = stamp_of(l.from, l.author_kind);
    doc.attachments = l.attachments.clone();
    doc
}

/// Store one project line as the next Thread item of `project:<id>`.
/// `conn` is the caller's (it holds the shared DB lock); the catalog row
/// is made on the first line. With `client`, a key already kept answers
/// [`ThreadPost::Duplicate`] and stores nothing.
pub fn post(conn: &Connection, l: &ProjectLine<'_>, client: Option<&ClientMsgKey>) -> Result<ThreadPost, String> {
    if l.group_id.trim().is_empty() {
        return Err("project line: group id required".to_string());
    }
    let conv = conversation_id(l.group_id);
    let _guard = WRITE.lock();
    if let Some(c) = client {
        if let Some(hit) = store::client_id_get(&c.key)? {
            return Ok(ThreadPost::Duplicate(hit));
        }
    }
    let seq = catalog::next_thread_seq(conn, &conv, l.group_id.trim())?;
    let mut doc = line_doc(uuid::Uuid::new_v4().to_string(), l);
    doc.client_msg_id = client.map(|c| c.client_msg_id.clone());
    let write = client.map(|c| store::ClientIdWrite { key: &c.key, at_ms: c.at_ms, body_hash: &c.body_hash });
    store::commit_write_keyed(&doc, Some((conv.as_str(), seq)), &[], None, write)?;
    let id = doc.id.clone();
    let links = vec![OverlayLink { collection: "thread", conversation_id: Some(conv.clone()), seq, id: id.clone() }];
    Ok(ThreadPost::Posted(OverlayItem { collection: "thread".to_string(), seq, id, doc, conversation_id: Some(conv) }, links))
}

/// One old `project_group_messages` row, for [`import_lines`].
#[derive(Debug, Clone)]
pub struct OldLine<'a> {
    pub message_id: &'a str,
    pub line: ProjectLine<'a>,
    pub created_at: i64,
    pub client_msg_id: Option<&'a str>,
}

/// The backfill's import key of an old line (`project/<message id>`): a
/// re-run never adds a line twice.
pub fn import_key(message_id: &str) -> String {
    format!("project/{}", message_id.trim())
}

/// Move old lines into `project:<id>` in ONE redb write plus one SQLite
/// savepoint, under the overlay write lock: the docs (id = the old message
/// id), their pointers in the order given, the import keys, the catalog
/// row and `mark` (the caller's own SQL, e.g. `messages_in_thread`) commit
/// together or not at all. The lines are NOT marked imported: a moved line
/// is an ordinary line (it can be unread, and its sender can delete it).
/// Returns what the store wrote.
pub fn import_lines(
    conn: &Connection,
    group_id: &str,
    lines: &[OldLine<'_>],
    mark: impl FnOnce(&Connection, &store::ImportWrite) -> Result<(), String>,
) -> Result<store::ImportWrite, String> {
    let conv = conversation_id(group_id);
    let entries: Vec<(String, OverlayDoc)> = lines
        .iter()
        .map(|l| {
            let mut doc = line_doc(l.message_id.to_string(), &l.line);
            doc.created_at = l.created_at;
            doc.client_msg_id = l.client_msg_id.map(str::to_string);
            (import_key(l.message_id), doc)
        })
        .collect();
    let _guard = WRITE.lock();
    conn.execute_batch("SAVEPOINT project_import").map_err(|e| format!("project import savepoint: {e}"))?;
    let res = (|| -> Result<store::ImportWrite, String> {
        catalog::ensure_conversation(conn, &conv, group_id.trim())?;
        let start_after = catalog::get(conn, &conv)?.map(|r| r.1).unwrap_or(0);
        store::import_docs(&conv, start_after, &entries, None, |w| {
            conn.execute(
                "UPDATE overlay_conversations SET last_thread_seq = MAX(last_thread_seq, ?2) WHERE conversation_id = ?1",
                rusqlite::params![conv, w.last_seq],
            )
            .map_err(|e| format!("project import catalog: {e}"))?;
            mark(conn, &w)
        })
    })();
    match res {
        Ok(w) => {
            conn.execute_batch("RELEASE project_import").map_err(|e| format!("project import release: {e}"))?;
            Ok(w)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO project_import; RELEASE project_import");
            Err(e)
        }
    }
}

/// Remove a project's chat from the Thread store (its catalog row is the
/// caller's, or the delete trigger's). Returns how many lines went.
pub fn purge(conversation_id: &str) -> Result<usize, String> {
    let _guard = WRITE.lock();
    store::purge_thread(conversation_id)
}

/// The `project:` conversations that still hold lines in the store.
pub fn stored_conversations() -> Result<Vec<String>, String> {
    store::thread_conversations_with_prefix(PROJECT_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_stamps() {
        assert_eq!(conversation_id(" g1 "), "project:g1");
        assert_eq!(group_of("project:g1"), Some("g1"));
        assert_eq!(group_of("project:"), None);
        assert_eq!(group_of("ticket:g1"), None);
        assert!(is_project("project:a b"));
        assert_eq!(stamp_of("owner", "person"), Some(PostAuthor::owner()));
        assert_eq!(stamp_of("alice", "person"), Some(PostAuthor::user("alice")));
        assert_eq!(stamp_of("guest:dana", "guest"), Some(PostAuthor::guest("dana")));
        assert_eq!(stamp_of("scout", "agent"), None);
        assert_eq!(kind_of(stamp_of("owner", "person").as_ref()), "person");
        assert_eq!(kind_of(stamp_of("alice", "person").as_ref()), "person");
        assert_eq!(kind_of(stamp_of("guest:dana", "guest").as_ref()), "guest");
        assert_eq!(kind_of(None), "agent");
        assert_eq!(kind_of(Some(&PostAuthor::agent("scout"))), "agent");
    }
}
