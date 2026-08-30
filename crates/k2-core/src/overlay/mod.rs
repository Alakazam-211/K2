//! Overlay threads + chatter (prd-overlay-threads-v1 S1+S3).
//!
//! Catalog (rusqlite) keys named `conversation_id` — handle
//! `conversation_key` or pinned `workspace_sessions.session_id`, never
//! `v2_session_map`. Documents live in redb: one `docs/{id}` body,
//! collection tables store `seq → id` pointers only.

pub mod catalog;
pub mod doc;
pub mod options;
pub mod store;
pub mod vault;

use parking_lot::Mutex;
use rusqlite::Connection;

pub use doc::{ChoiceBody, ChoiceOption, OverlayDoc, OverlayItem, OverlayLink, SecretBody};
pub use options::{parse_options_value, split_options_csv};
pub use store::OverlayPage;

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

/// Post a choice card onto Thread. Returns immediately; card is `pending`.
pub fn post_choice(
    conn: &Connection,
    conversation_id: &str,
    project_id: &str,
    from: &str,
    to: &str,
    prompt: &str,
    options: Vec<String>,
    allow_custom: bool,
) -> Result<(OverlayItem, Vec<OverlayLink>), String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("ask prompt must not be empty".to_string());
    }
    if options.is_empty() {
        return Err("ask requires at least one option".to_string());
    }
    let _guard = WRITE.lock();
    let seq = catalog::next_thread_seq(conn, conversation_id, project_id)?;
    let id = uuid::Uuid::new_v4().to_string();
    let doc = OverlayDoc::choice(
        id.clone(),
        from.trim().to_string(),
        to.trim().to_string(),
        prompt.to_string(),
        options,
        allow_custom,
    );
    store::commit_write(&doc, Some((conversation_id, seq)), &[], None)?;
    Ok(item_and_thread_link(conversation_id, seq, id, doc))
}

/// Post a secret card onto Thread. Value is not stored in the document.
pub fn post_secret(
    conn: &Connection,
    conversation_id: &str,
    project_id: &str,
    from: &str,
    to: &str,
    name: &str,
    prompt: Option<&str>,
) -> Result<(OverlayItem, Vec<OverlayLink>), String> {
    vault::validate_name(name)?;
    let _guard = WRITE.lock();
    let seq = catalog::next_thread_seq(conn, conversation_id, project_id)?;
    let id = uuid::Uuid::new_v4().to_string();
    let prompt = prompt
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let doc = OverlayDoc::secret_card(
        id.clone(),
        from.trim().to_string(),
        to.trim().to_string(),
        name.trim().to_string(),
        prompt,
    );
    store::commit_write(&doc, Some((conversation_id, seq)), &[], None)?;
    Ok(item_and_thread_link(conversation_id, seq, id, doc))
}

fn item_and_thread_link(
    conversation_id: &str,
    seq: i64,
    id: String,
    doc: OverlayDoc,
) -> (OverlayItem, Vec<OverlayLink>) {
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
    (item, links)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardCallback {
    pub conversation_id: String,
    pub doc_id: String,
    pub seq: i64,
    pub doc: OverlayDoc,
    /// Short line AFTER `[thread:<addr>] ` — never contains secret bytes.
    pub inject_line: String,
}

/// Tap an option (or custom) / submit a secret.
pub fn answer_card(
    conversation_id: &str,
    project_id: &str,
    card_id: &str,
    choice_answer: Option<&str>,
    secret_bytes: Option<&[u8]>,
) -> Result<CardCallback, String> {
    let _guard = WRITE.lock();
    let item = thread_item(conversation_id, card_id)?;
    let mut doc = item.doc;
    let inject_line = if doc.is_pending_choice() {
        let answer = choice_answer
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "answer required".to_string())?;
        let choice = doc.choice.as_mut().expect("pending choice");
        let in_options = choice.options.iter().any(|o| o.label == answer);
        if !in_options && !choice.allow_custom {
            return Err(format!("'{answer}' is not an option on this card"));
        }
        choice.status = "answered".to_string();
        choice.answer = Some(answer.to_string());
        format!("chose {answer}")
    } else if doc.is_pending_secret() {
        let bytes = secret_bytes.ok_or_else(|| "secret value required".to_string())?;
        let name = doc.secret.as_ref().expect("pending secret").name.clone();
        vault::put(project_id, &name, bytes)?;
        if let Some(s) = doc.secret.as_mut() {
            s.status = "set".to_string();
        }
        format!("secret {name} set")
    } else {
        return Err("card is not pending".to_string());
    };
    store::put_doc(&doc)?;
    Ok(CardCallback {
        conversation_id: conversation_id.to_string(),
        doc_id: doc.id.clone(),
        seq: item.seq,
        doc,
        inject_line,
    })
}

/// Explicit dismiss (secret X / chrome dismiss).
pub fn void_card(
    conversation_id: &str,
    project_id: &str,
    card_id: &str,
) -> Result<CardCallback, String> {
    let _guard = WRITE.lock();
    let item = thread_item(conversation_id, card_id)?;
    let mut doc = item.doc;
    let inject_line = if doc.is_pending_choice() {
        if let Some(c) = doc.choice.as_mut() {
            c.status = "voided".to_string();
            c.answer = None;
        }
        "card voided — human replied in chat".to_string()
    } else if doc.is_pending_secret() {
        let name = doc.secret.as_ref().expect("pending secret").name.clone();
        vault::delete(project_id, &name)?;
        if let Some(s) = doc.secret.as_mut() {
            s.status = "voided".to_string();
        }
        "card voided — human replied in chat".to_string()
    } else {
        return Err("card is not pending".to_string());
    };
    store::put_doc(&doc)?;
    Ok(CardCallback {
        conversation_id: conversation_id.to_string(),
        doc_id: doc.id.clone(),
        seq: item.seq,
        doc,
        inject_line,
    })
}

/// T25: human prose on this conversation. Matching option label marks
/// that choice; every other pending card voids. Secrets are never scraped.
pub fn apply_prose(
    conversation_id: &str,
    project_id: &str,
    text: &str,
) -> Result<Vec<CardCallback>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let _guard = WRITE.lock();
    let items = store::read_thread(conversation_id, 0)?;
    let mut out = Vec::new();
    for item in items {
        let mut doc = item.doc;
        if doc.is_pending_choice() {
            let choice = doc.choice.as_mut().expect("pending choice");
            if choice.options.iter().any(|o| o.label == trimmed) {
                choice.status = "answered".to_string();
                choice.answer = Some(trimmed.to_string());
                store::put_doc(&doc)?;
                out.push(CardCallback {
                    conversation_id: conversation_id.to_string(),
                    doc_id: doc.id.clone(),
                    seq: item.seq,
                    inject_line: format!("chose {trimmed}"),
                    doc,
                });
            } else {
                choice.status = "voided".to_string();
                choice.answer = None;
                store::put_doc(&doc)?;
                out.push(CardCallback {
                    conversation_id: conversation_id.to_string(),
                    doc_id: doc.id.clone(),
                    seq: item.seq,
                    inject_line: "card voided — human replied in chat".to_string(),
                    doc,
                });
            }
        } else if doc.is_pending_secret() {
            let name = doc.secret.as_ref().expect("pending secret").name.clone();
            vault::delete(project_id, &name)?;
            if let Some(s) = doc.secret.as_mut() {
                s.status = "voided".to_string();
            }
            store::put_doc(&doc)?;
            out.push(CardCallback {
                conversation_id: conversation_id.to_string(),
                doc_id: doc.id.clone(),
                seq: item.seq,
                inject_line: "card voided — human replied in chat".to_string(),
                doc,
            });
        }
    }
    Ok(out)
}

fn thread_item(conversation_id: &str, card_id: &str) -> Result<OverlayItem, String> {
    store::read_thread(conversation_id, 0)?
        .into_iter()
        .find(|i| i.id == card_id)
        .ok_or_else(|| format!("card not found: {card_id}"))
}

/// Snapshot JSON must never contain these secret bytes.
pub fn snapshot_contains_secret(items: &[OverlayItem], secret: &str) -> bool {
    let blob = serde_json::to_string(items).unwrap_or_default();
    blob.contains(secret)
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

pub fn read_thread(conversation_id: &str, since_seq: i64) -> Result<Vec<OverlayItem>, String> {
    store::read_thread(conversation_id, since_seq)
}

pub fn read_chatter(conversation_id: &str, since_seq: i64) -> Result<Vec<OverlayItem>, String> {
    store::read_chatter(conversation_id, since_seq)
}

/// Newest `limit` items with `seq > since_seq` (if set) and `seq < before_seq`
/// (if set), ascending seq. `limit == 0` is unbounded.
pub fn read_thread_page(
    conversation_id: &str,
    since_seq: Option<i64>,
    before_seq: Option<i64>,
    limit: usize,
) -> Result<OverlayPage, String> {
    store::read_thread_page(conversation_id, since_seq, before_seq, limit)
}

pub fn read_chatter_page(
    conversation_id: &str,
    since_seq: Option<i64>,
    before_seq: Option<i64>,
    limit: usize,
) -> Result<OverlayPage, String> {
    store::read_chatter_page(conversation_id, since_seq, before_seq, limit)
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
        assert_eq!(
            item.doc.from, "k2",
            "from must be stamped, got {:?}",
            item.doc.from
        );
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
        assert_eq!(
            doc.via.as_deref(),
            Some("msg"),
            "via must be msg, got {:?}",
            doc.via
        );
        assert!(
            store::debug_doc_exists(&doc.id).expect("doc exists"),
            "exactly one docs/{{id}} body must exist"
        );
        let (thread_n, chatter_n, log_n) =
            store::debug_pointer_count(&doc.id).expect("pointer count");
        assert_eq!(
            thread_n, 0,
            "A2A must not auto-link Thread, got {thread_n} thread pointers"
        );
        assert_eq!(
            chatter_n, 2,
            "Chatter links on reviewer + sender, got {chatter_n} (links={links:?})"
        );
        assert_eq!(
            log_n, 1,
            "ChatterLog must have exactly one pointer, got {log_n}"
        );

        let thread_items = read_thread(&reviewer, 0).expect("read thread");
        assert!(
            thread_items.is_empty(),
            "GET thread must not contain the ping, got {thread_items:?}"
        );
        let chatter_items = read_chatter(&reviewer, 0).expect("read chatter");
        assert_eq!(
            chatter_items.len(),
            1,
            "reviewer mailbox: {chatter_items:?}"
        );
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
        post_thread(
            &c,
            session_id,
            &project_id,
            "k2",
            "ovl-named",
            "pinned",
            "thread",
        )
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

    #[test]
    fn ask_pending_then_tap_go_marks_answered() {
        let project_id = seed_project("ovl-ask");
        let conv = uuid::Uuid::new_v4().to_string();
        let dbh = conn();
        let c = dbh.lock();
        let (item, links) = post_choice(
            &c,
            &conv,
            &project_id,
            "k2",
            "ovl-ask",
            "Ship it?",
            vec!["Go".to_string(), "Stop".to_string()],
            false,
        )
        .expect("post_choice");
        assert_eq!(item.doc.kind, "choice", "{item:?}");
        let choice = item.doc.choice.as_ref().expect("choice body");
        assert_eq!(choice.status, "pending", "{choice:?}");
        assert_eq!(choice.options[0].label, "Go", "first option is primary");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].collection, "thread");

        let cb = answer_card(&conv, &project_id, &item.id, Some("Go"), None).expect("tap Go");
        assert_eq!(cb.doc.choice.as_ref().expect("choice").status, "answered");
        assert_eq!(
            cb.doc.choice.as_ref().expect("choice").answer.as_deref(),
            Some("Go")
        );
        assert_eq!(cb.inject_line, "chose Go");
        let items = read_thread(&conv, 0).expect("read");
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].doc.choice.as_ref().expect("choice").status,
            "answered",
            "tap must mark answered; got {:?}",
            items[0].doc.choice
        );
    }

    #[test]
    fn prose_voids_pending_unless_exact_option_label() {
        let project_id = seed_project("ovl-prose");
        let conv = uuid::Uuid::new_v4().to_string();
        let dbh = conn();
        let c = dbh.lock();
        let (go, _) = post_choice(
            &c,
            &conv,
            &project_id,
            "k2",
            "ovl-prose",
            "?",
            vec!["Go".to_string(), "Stop".to_string()],
            false,
        )
        .expect("choice");
        let (secret, _) = post_secret(&c, &conv, &project_id, "k2", "ovl-prose", "API_TOKEN", None)
            .expect("secret");

        let voided = apply_prose(&conv, &project_id, "never mind").expect("prose");
        assert_eq!(voided.len(), 2, "both pending cards void: {voided:?}");
        assert!(
            voided.iter().all(|cb| cb.inject_line.contains("voided")),
            "void inject: {voided:?}"
        );
        let items = read_thread(&conv, 0).expect("read");
        let choice = items
            .iter()
            .find(|i| i.id == go.id)
            .expect("choice still on thread")
            .doc
            .choice
            .as_ref()
            .expect("choice");
        assert_eq!(choice.status, "voided", "{choice:?}");
        let secret_doc = items
            .iter()
            .find(|i| i.id == secret.id)
            .expect("secret still on thread")
            .doc
            .secret
            .as_ref()
            .expect("secret");
        assert_eq!(secret_doc.status, "voided", "{secret_doc:?}");
        assert!(
            !vault::exists(&project_id, "API_TOKEN"),
            "chat must not scrape a secret into the vault"
        );

        let conv2 = uuid::Uuid::new_v4().to_string();
        let (marked, _) = post_choice(
            &c,
            &conv2,
            &project_id,
            "k2",
            "ovl-prose",
            "?",
            vec!["Go".to_string(), "Stop".to_string()],
            false,
        )
        .expect("choice2");
        let (other, _) = post_choice(
            &c,
            &conv2,
            &project_id,
            "k2",
            "ovl-prose",
            "other",
            vec!["Hold".to_string()],
            false,
        )
        .expect("other");
        let cbs = apply_prose(&conv2, &project_id, "Go").expect("exact Go");
        assert_eq!(cbs.len(), 2, "{cbs:?}");
        let items = read_thread(&conv2, 0).expect("read2");
        let marked_doc = items.iter().find(|i| i.id == marked.id).expect("marked");
        assert_eq!(
            marked_doc.doc.choice.as_ref().expect("c").status,
            "answered",
            "exact option label marks rather than void: {:?}",
            marked_doc.doc.choice
        );
        assert_eq!(
            marked_doc.doc.choice.as_ref().expect("c").answer.as_deref(),
            Some("Go")
        );
        let other_doc = items.iter().find(|i| i.id == other.id).expect("other");
        assert_eq!(
            other_doc.doc.choice.as_ref().expect("c").status,
            "voided",
            "other pending still void: {:?}",
            other_doc.doc.choice
        );
        assert!(
            cbs.iter().any(|cb| cb.inject_line == "chose Go"),
            "chose inject: {cbs:?}"
        );
    }

    #[test]
    fn secret_submit_sets_vault_without_bytes_in_thread() {
        let project_id = seed_project("ovl-sec");
        let conv = uuid::Uuid::new_v4().to_string();
        let dbh = conn();
        let c = dbh.lock();
        let (item, _) = post_secret(
            &c,
            &conv,
            &project_id,
            "k2",
            "ovl-sec",
            "API_TOKEN",
            Some("Paste the Grok token"),
        )
        .expect("post_secret");
        assert_eq!(item.doc.kind, "secret");
        assert_eq!(item.doc.secret.as_ref().expect("secret").status, "pending");
        let secret_bytes = b"s3cr3t-NEVER-IN-REDB-xyz";
        let cb = answer_card(
            &conv,
            &project_id,
            &item.id,
            None,
            Some(secret_bytes.as_slice()),
        )
        .expect("submit");
        assert_eq!(cb.doc.secret.as_ref().expect("secret").status, "set");
        assert_eq!(cb.inject_line, "secret API_TOKEN set");
        assert!(
            !cb.inject_line.contains("s3cr3t"),
            "inject must never carry secret bytes: {}",
            cb.inject_line
        );
        let got = vault::debug_read(&project_id, "API_TOKEN").expect("vault");
        assert_eq!(got, secret_bytes, "vault must hold the bytes");
        let items = read_thread(&conv, 0).expect("read");
        assert!(
            !snapshot_contains_secret(&items, "s3cr3t-NEVER-IN-REDB-xyz"),
            "thread JSON must not contain secret: {items:?}"
        );
        assert!(
            !store::debug_docs_contain("s3cr3t-NEVER-IN-REDB-xyz").expect("scan"),
            "redb docs must not contain secret bytes"
        );
        let home = crate::paths::k2_home().join("thread-secrets");
        assert!(
            !vault::vault_root().starts_with(&home),
            "tests must not write production ~/.k2/thread-secrets; vault_root={:?}",
            vault::vault_root()
        );

        let conv2 = uuid::Uuid::new_v4().to_string();
        let (pending, _) = post_secret(
            &c,
            &conv2,
            &project_id,
            "k2",
            "ovl-sec",
            "OTHER_TOKEN",
            None,
        )
        .expect("pending");
        let voided = void_card(&conv2, &project_id, &pending.id).expect("void");
        assert_eq!(voided.doc.secret.as_ref().expect("s").status, "voided");
        assert!(
            !vault::exists(&project_id, "OTHER_TOKEN"),
            "dismiss/void must leave vault empty"
        );
    }

    #[test]
    fn thread_page_newest_50_then_older_before_seq() {
        let project_id = seed_project("ovl-page-t");
        let conv = uuid::Uuid::new_v4().to_string();
        let dbh = conn();
        let c = dbh.lock();
        for i in 1..=60 {
            post_thread(
                &c,
                &conv,
                &project_id,
                "k2",
                "ovl-page-t",
                &format!("t{i}"),
                "thread",
            )
            .expect("post");
        }
        let page = read_thread_page(&conv, None, None, 50).expect("page");
        assert_eq!(
            page.items.len(),
            50,
            "newest 50, got {:?}",
            page.items.len()
        );
        assert_eq!(page.items[0].seq, 11, "tail starts at seq 11");
        assert_eq!(page.items[49].seq, 60);
        assert!(
            page.items.windows(2).all(|w| w[0].seq < w[1].seq),
            "page must be ascending seq"
        );
        assert!(page.has_more, "60 items, page of 50 must set has_more");

        let older = read_thread_page(&conv, None, Some(11), 50).expect("older");
        assert_eq!(
            older.items.len(),
            10,
            "seq 1..=10, got {}",
            older.items.len()
        );
        assert_eq!(older.items[0].seq, 1);
        assert_eq!(older.items[9].seq, 10);
        assert!(!older.has_more, "no items below seq 1: {older:?}");

        let all = read_thread(&conv, 0).expect("unbounded helper");
        assert_eq!(all.len(), 60, "read_thread(_, 0) stays unbounded");

        let unbounded = read_thread_page(&conv, None, None, 0).expect("limit 0");
        assert_eq!(unbounded.items.len(), 60);
        assert!(!unbounded.has_more);
    }

    #[test]
    fn chatter_page_newest_50_then_older_before_seq() {
        let project_id = seed_project("ovl-page-c");
        let conv = uuid::Uuid::new_v4().to_string();
        let dbh = conn();
        let c = dbh.lock();
        for i in 1..=60 {
            record_chatter(
                &c,
                &conv,
                &project_id,
                None,
                "sales",
                "sales/reviewer",
                &format!("c{i}"),
                "msg",
                "accepted",
            )
            .expect("chatter");
        }
        let page = read_chatter_page(&conv, None, None, 50).expect("page");
        assert_eq!(page.items.len(), 50);
        assert_eq!(page.items[0].seq, 11);
        assert_eq!(page.items[49].seq, 60);
        assert!(page.has_more);

        let older = read_chatter_page(&conv, None, Some(11), 50).expect("older");
        assert_eq!(older.items.len(), 10);
        assert_eq!(older.items[0].seq, 1);
        assert_eq!(older.items[9].seq, 10);
        assert!(!older.has_more);

        let all = read_chatter(&conv, 0).expect("unbounded helper");
        assert_eq!(all.len(), 60);
    }

    #[test]
    fn empty_conv_page_is_empty_no_has_more() {
        let conv = uuid::Uuid::new_v4().to_string();
        let thread = read_thread_page(&conv, None, None, 50).expect("thread empty");
        assert!(thread.items.is_empty(), "{thread:?}");
        assert!(!thread.has_more);
        let chatter = read_chatter_page(&conv, None, None, 50).expect("chatter empty");
        assert!(chatter.items.is_empty(), "{chatter:?}");
        assert!(!chatter.has_more);
    }
}
