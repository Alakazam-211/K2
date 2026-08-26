//! Overlay threads + chatter (prd-overlay-threads-v1 S1).
//!
//! Catalog (rusqlite) keys named `conversation_id` — handle
//! `conversation_key` or pinned `workspace_sessions.session_id`, never
//! `v2_session_map`. Documents live in redb: one `docs/{id}` body,
//! collection tables store `seq → id` pointers only.

pub mod catalog;
pub mod doc;
pub mod store;

use parking_lot::Mutex;
use rusqlite::Connection;

pub use doc::{OverlayDoc, OverlayItem, OverlayLink};

static WRITE: Mutex<()> = Mutex::new(());

/// Post overlay text onto the Thread collection only (not Chatter).
pub fn post_thread(
    conn: &Connection,
    conversation_id: &str,
    project_id: &str,
    from: &str,
    to: &str,
    body: &str,
    via: &str,
) -> Result<(OverlayItem, Vec<OverlayLink>), String> {
    let _guard = WRITE.lock();
    let seq = catalog::next_thread_seq(conn, conversation_id, project_id)?;
    let id = uuid::Uuid::new_v4().to_string();
    let doc = OverlayDoc::text(
        id.clone(),
        from.trim().to_string(),
        to.trim().to_string(),
        body.to_string(),
        via,
    );
    store::commit_write(&doc, Some((conversation_id, seq)), &[], None)?;
    let item = OverlayItem {
        collection: "thread".to_string(),
        seq,
        id: id.clone(),
        doc,
        conversation_id: Some(conversation_id.to_string()),
    };
    let links = vec![OverlayLink {
        collection: "thread",
        conversation_id: Some(conversation_id.to_string()),
        seq,
        id,
    }];
    Ok((item, links))
}

/// Record a `k2 msg` / `k2 talk` / inject sibling as chatter.
///
/// One `docs/{id}`. Chatter pointers on the recipient mailbox and, when
/// `sender_conversation_id` is a distinct local conversation, the sender
/// mailbox. Always a ChatterLog pointer. Never a Thread pointer.
pub fn record_chatter(
    conn: &Connection,
    recipient_conversation_id: &str,
    recipient_project_id: &str,
    sender_conversation_id: Option<(&str, &str)>,
    from: &str,
    to: &str,
    body: &str,
    via: &str,
    inject: &str,
) -> Result<(OverlayDoc, Vec<OverlayLink>), String> {
    let _guard = WRITE.lock();
    let to_seq = catalog::next_chatter_seq(conn, recipient_conversation_id, recipient_project_id)?;
    let mut chatter_keys: Vec<(String, i64)> =
        vec![(recipient_conversation_id.to_string(), to_seq)];
    let mut links = vec![OverlayLink {
        collection: "chatter",
        conversation_id: Some(recipient_conversation_id.to_string()),
        seq: to_seq,
        id: String::new(),
    }];
    if let Some((from_conv, from_project)) = sender_conversation_id {
        if from_conv != recipient_conversation_id {
            let from_seq = catalog::next_chatter_seq(conn, from_conv, from_project)?;
            chatter_keys.push((from_conv.to_string(), from_seq));
            links.push(OverlayLink {
                collection: "chatter",
                conversation_id: Some(from_conv.to_string()),
                seq: from_seq,
                id: String::new(),
            });
        }
    }
    let log_seq = catalog::next_chatterlog_seq(conn)?;
    let id = uuid::Uuid::new_v4().to_string();
    for link in &mut links {
        link.id = id.clone();
    }
    links.push(OverlayLink {
        collection: "chatterlog",
        conversation_id: None,
        seq: log_seq,
        id: id.clone(),
    });
    let doc = OverlayDoc::chatter(
        id,
        from.trim().to_string(),
        to.trim().to_string(),
        body.to_string(),
        via,
        inject,
    );
    store::commit_write(&doc, None, &chatter_keys, Some(log_seq))?;
    Ok((doc, links))
}

pub fn read_thread(
    conversation_id: &str,
    since_seq: i64,
) -> Result<Vec<OverlayItem>, String> {
    store::read_thread(conversation_id, since_seq)
}

pub fn read_chatter(
    conversation_id: &str,
    since_seq: i64,
) -> Result<Vec<OverlayItem>, String> {
    store::read_chatter(conversation_id, since_seq)
}

pub fn read_chatterlog(since_seq: i64) -> Result<Vec<OverlayItem>, String> {
    store::read_chatterlog(since_seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn conn() -> std::sync::Arc<parking_lot::ReentrantMutex<Connection>> {
        crate::db::init_for_tests()
    }

    fn seed_project(handle: &str) -> String {
        let dbh = conn();
        let c = dbh.lock();
        let id = uuid::Uuid::new_v4().to_string();
        let path = format!("/tmp/overlay-{handle}-{id}");
        c.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            params![id, handle, path],
        )
        .expect("seed project");
        id
    }

    #[test]
    fn thread_post_then_read_has_from_and_seq() {
        let project_id = seed_project("ovl-thread");
        let conv = uuid::Uuid::new_v4().to_string();
        let dbh = conn();
        let c = dbh.lock();
        let (item, links) = post_thread(&c, &conv, &project_id, "k2", "ovl-thread", "hi", "thread")
            .expect("post_thread");
        assert_eq!(item.seq, 1, "first thread seq must be 1, got {}", item.seq);
        assert_eq!(item.doc.from, "k2", "from must be stamped, got {:?}", item.doc.from);
        assert_eq!(item.doc.kind, "text");
        assert_eq!(item.doc.body.as_deref(), Some("hi"));
        assert_eq!(links.len(), 1, "thread write links Thread only: {links:?}");
        assert_eq!(links[0].collection, "thread");

        let items = read_thread(&conv, 0).expect("read_thread");
        assert_eq!(items.len(), 1, "read must show the post, got {items:?}");
        assert_eq!(items[0].seq, 1);
        assert_eq!(items[0].doc.from, "k2");
        assert_eq!(items[0].doc.body.as_deref(), Some("hi"));
        assert_eq!(
            items[0].conversation_id.as_deref(),
            Some(conv.as_str()),
            "catalog/read keyed by named conversation_id, not v2_session_map"
        );

        let (project, last_thread, last_chatter) =
            catalog::get(&c, &conv).expect("catalog get").expect("row");
        assert_eq!(project, project_id, "catalog project_id");
        assert_eq!(last_thread, 1);
        assert_eq!(last_chatter, 0, "thread write must not bump chatter seq");
    }

    #[test]
    fn chatter_one_doc_links_mailboxes_and_log_not_thread() {
        let project_id = seed_project("ovl-chatter");
        let reviewer = uuid::Uuid::new_v4().to_string();
        let sender = uuid::Uuid::new_v4().to_string();
        let dbh = conn();
        let c = dbh.lock();
        crate::workspace_session_handles::allocate_ordinal(&c, &project_id, &reviewer)
            .expect("handle reviewer");
        let (doc, links) = record_chatter(
            &c,
            &reviewer,
            &project_id,
            Some((sender.as_str(), project_id.as_str())),
            "sales",
            "sales/reviewer",
            "ping",
            "msg",
            "accepted",
        )
        .expect("record_chatter");

        assert_eq!(doc.kind, "chatter");
        assert_eq!(doc.via.as_deref(), Some("msg"), "via must be msg, got {:?}", doc.via);
        assert!(
            store::debug_doc_exists(&doc.id).expect("doc exists"),
            "exactly one docs/{{id}} body must exist"
        );
        let (thread_n, chatter_n, log_n) =
            store::debug_pointer_count(&doc.id).expect("pointer count");
        assert_eq!(thread_n, 0, "A2A must not auto-link Thread, got {thread_n} thread pointers");
        assert_eq!(
            chatter_n, 2,
            "Chatter links on reviewer + sender, got {chatter_n} (links={links:?})"
        );
        assert_eq!(log_n, 1, "ChatterLog must have exactly one pointer, got {log_n}");

        let thread_items = read_thread(&reviewer, 0).expect("read thread");
        assert!(
            thread_items.is_empty(),
            "GET thread must not contain the ping, got {thread_items:?}"
        );
        let chatter_items = read_chatter(&reviewer, 0).expect("read chatter");
        assert_eq!(chatter_items.len(), 1, "reviewer mailbox: {chatter_items:?}");
        assert_eq!(chatter_items[0].doc.body.as_deref(), Some("ping"));
        let sender_box = read_chatter(&sender, 0).expect("sender mailbox");
        assert_eq!(sender_box.len(), 1, "sender mailbox: {sender_box:?}");
        assert_eq!(sender_box[0].id, doc.id, "same doc id, not a second body");
        let log = read_chatterlog(0).expect("chatterlog");
        assert!(
            log.iter().any(|i| i.id == doc.id),
            "chatterlog must contain ping id {}; log={log:?}",
            doc.id
        );
    }

    #[test]
    fn talk_stamps_via_talk() {
        let project_id = seed_project("ovl-talk");
        let conv = uuid::Uuid::new_v4().to_string();
        let dbh = conn();
        let c = dbh.lock();
        let (doc, _) = record_chatter(
            &c,
            &conv,
            &project_id,
            None,
            "sales",
            "sales/reviewer",
            "hello via talk",
            "talk",
            "accepted",
        )
        .expect("talk chatter");
        assert_eq!(
            doc.via.as_deref(),
            Some("talk"),
            "via: talk must be stamped or talk vanishes into msg; got {:?}",
            doc.via
        );
        let items = read_chatter(&conv, 0).expect("read");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].doc.via.as_deref(), Some("talk"));
    }

    #[test]
    fn catalog_row_uses_named_conversation_id_not_agent_name() {
        let project_id = seed_project("ovl-named");
        let session_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let dbh = conn();
        let c = dbh.lock();
        crate::db::schema::WorkspaceSession::upsert(
            &c,
            "ws-row",
            &project_id,
            None,
            Some(session_id),
            "claude",
            "system",
            "running",
        )
        .expect("pin");
        post_thread(&c, session_id, &project_id, "k2", "ovl-named", "pinned", "thread")
            .expect("post");
        let row = catalog::get(&c, session_id)
            .expect("get")
            .expect("catalog row for pinned session_id");
        assert_eq!(row.0, project_id);
        assert!(
            catalog::get(&c, "tab-not-a-conversation")
                .expect("get missing")
                .is_none(),
            "must not key overlay on v2_session_map agent_name"
        );
    }
}
