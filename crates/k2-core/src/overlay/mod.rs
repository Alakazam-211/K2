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
pub mod transfer;
pub mod vault;

use parking_lot::Mutex;
use rusqlite::Connection;

pub use doc::{ChoiceBody, ChoiceOption, CollectedMeta, DesignOrigin, ImportedMeta, OverlayDoc, OverlayItem, OverlayLink, PostAuthor, SecretBody, SecretProposal, WidgetOrigin, WidgetPlacementClaim};
pub use options::{parse_options_value, split_options_csv};
pub use store::{ExtTxn, OverlayPage, SeqRemap};
pub use transfer::IMPORTED_READ_ONLY;

static WRITE: Mutex<()> = Mutex::new(());

/// Write a **collected** conversation (lines K2 gathers from an outside
/// agent product, e.g. a local Grok Bot at `gb:<name>`) in one redb
/// transaction under the overlay write lock: dedupe index, in-place
/// updates, appends and the collector's cursor commit together or not at
/// all. These conversations have no workspace: nothing here wakes an
/// agent, injects into a PTY, pushes a notification or federates.
pub fn collected_write<R>(
    conversation_id: &str,
    f: impl FnOnce(&mut ExtTxn<'_>) -> Result<R, String>,
) -> Result<R, String> {
    let _guard = WRITE.lock();
    store::ext_write(conversation_id, f)
}

/// A collected conversation's cursor JSON (see [`collected_write`]).
pub fn collected_state(conversation_id: &str) -> Result<Option<Vec<u8>>, String> {
    store::ext_state(conversation_id)
}

/// A conversation's Thread + Chatter moved to a new key (Codex/Hermes
/// adoption: the pane key → the provider's conversation id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationMove {
    pub project_id: String,
    pub from: String,
    pub to: String,
    /// Thread rows moved from `from`.
    pub thread_moved: usize,
    /// Thread rows now under `to` (moved + any already there).
    pub thread_total: usize,
    pub chatter_moved: usize,
    pub chatter_total: usize,
}

/// Called after every [`move_conversation`] that moved something. The
/// daemon registers one that tells live Thread views to follow the new
/// key and moves the open Thread turn; k2-core alone has no event bus.
static MOVE_LISTENER: std::sync::OnceLock<fn(&ConversationMove)> = std::sync::OnceLock::new();

/// Register the move listener (first call wins; later calls no-op).
pub fn set_move_listener(f: fn(&ConversationMove)) {
    let _ = MOVE_LISTENER.set(f);
}

/// Move the Thread (and Chatter) of conversation `from` to `to`: the
/// catalog row and the redb `seq → id` pointers, merged by doc id with
/// any rows already under `to` and renumbered after them in time order
/// (see [`store::move_conversation`]). One redb transaction plus one
/// SQLite savepoint, under the overlay write lock, so a concurrent post
/// can neither be lost nor land twice. Hold the shared DB lock (`conn`)
/// across the key change that made `to` the address's conversation, so
/// no post resolves in between.
///
/// `Ok(None)` when there is nothing to move (`from` has no catalog row,
/// or the keys are equal / empty). Calls the move listener on success.
pub fn move_conversation(
    conn: &Connection,
    from: &str,
    to: &str,
) -> Result<Option<ConversationMove>, String> {
    let from = from.trim();
    let to = to.trim();
    if from.is_empty() || to.is_empty() || from == to {
        return Ok(None);
    }
    let moved = {
        let _guard = WRITE.lock();
        let Some((project_id, _, _)) = catalog::get(conn, from)? else {
            return Ok(None);
        };
        conn.execute_batch("SAVEPOINT overlay_move")
            .map_err(|e| format!("overlay move savepoint: {e}"))?;
        let res = store::move_conversation(from, to, |thread, chatter| {
            catalog::apply_move(
                conn,
                from,
                to,
                &project_id,
                thread.total as i64,
                chatter.total as i64,
            )?;
            // The move renumbered the Thread: every person's read cursor
            // follows it in the same savepoint (thread-read-v1).
            crate::read_state::remap_thread_move(conn, from, to, &thread.remap, false)
        });
        match res {
            Ok((thread, chatter)) => {
                conn.execute_batch("RELEASE overlay_move")
                    .map_err(|e| format!("overlay move release: {e}"))?;
                ConversationMove {
                    project_id,
                    from: from.to_string(),
                    to: to.to_string(),
                    thread_moved: thread.moved,
                    thread_total: thread.total,
                    chatter_moved: chatter.moved,
                    chatter_total: chatter.total,
                }
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK TO overlay_move; RELEASE overlay_move");
                return Err(e);
            }
        }
    };
    if let Some(f) = MOVE_LISTENER.get() {
        f(&moved);
    }
    Ok(Some(moved))
}

/// Split a conversation two workspaces shared (TR20, fork tabs): move the
/// Thread/Chatter docs `take` accepts from `from` to `to` (catalog row in
/// `to_project_id`), under the overlay write lock in one redb transaction
/// plus one SQLite savepoint. `from`'s catalog row and its remaining
/// rows' seqs are left as they are. Fires no move listener: no client is
/// on `to` yet. `Ok(None)` when nothing matched.
pub fn split_conversation(
    conn: &Connection,
    from: &str,
    to: &str,
    to_project_id: &str,
    take: &dyn Fn(&OverlayDoc) -> bool,
) -> Result<Option<ConversationMove>, String> {
    let from = from.trim();
    let to = to.trim();
    if from.is_empty() || to.is_empty() || from == to {
        return Ok(None);
    }
    let _guard = WRITE.lock();
    if catalog::get(conn, from)?.is_none() {
        return Ok(None);
    }
    conn.execute_batch("SAVEPOINT overlay_split")
        .map_err(|e| format!("overlay split savepoint: {e}"))?;
    let res = store::split_conversation(from, to, take, |thread, chatter| {
        if thread.moved == 0 && chatter.moved == 0 {
            return Ok(());
        }
        catalog::ensure_conversation(conn, to, to_project_id)?;
        conn.execute(
            "UPDATE overlay_conversations \
             SET last_thread_seq = MAX(last_thread_seq, ?2), last_chatter_seq = MAX(last_chatter_seq, ?3) \
             WHERE conversation_id = ?1",
            rusqlite::params![to, thread.total as i64, chatter.total as i64],
        )
        .map_err(|e| format!("overlay catalog split: {e}"))?;
        // `to` inherits each person's read state for the rows it took
        // (thread-read-v1); `from` keeps its seqs and cursors.
        crate::read_state::remap_thread_move(conn, from, to, &thread.remap, true)
    });
    match res {
        Ok((thread, chatter)) => {
            conn.execute_batch("RELEASE overlay_split")
                .map_err(|e| format!("overlay split release: {e}"))?;
            if thread.moved == 0 && chatter.moved == 0 {
                return Ok(None);
            }
            Ok(Some(ConversationMove {
                project_id: to_project_id.to_string(),
                from: from.to_string(),
                to: to.to_string(),
                thread_moved: thread.moved,
                thread_total: thread.total,
                chatter_moved: chatter.moved,
                chatter_total: chatter.total,
            }))
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO overlay_split; RELEASE overlay_split");
            Err(e)
        }
    }
}

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
    post_thread_with_origin(conn, conversation_id, project_id, from, to, body, via, None, None)
}

/// [`post_thread`], recording the Garden widget that sent the post for the
/// person (prd-zen-user-widgets-v2 UWB12a) on the doc as `widget`, and the
/// person who sent it (the caller's credential, never the body) as `author`.
#[allow(clippy::too_many_arguments)]
pub fn post_thread_with_origin(
    conn: &Connection,
    conversation_id: &str,
    project_id: &str,
    from: &str,
    to: &str,
    body: &str,
    via: &str,
    widget: Option<WidgetOrigin>,
    author: Option<PostAuthor>,
) -> Result<(OverlayItem, Vec<OverlayLink>), String> {
    let _guard = WRITE.lock();
    let seq = catalog::next_thread_seq(conn, conversation_id, project_id)?;
    let id = uuid::Uuid::new_v4().to_string();
    let mut doc = OverlayDoc::text(
        id.clone(),
        from.trim().to_string(),
        to.trim().to_string(),
        body.to_string(),
        via,
    );
    doc.widget = widget;
    doc.author = author;
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

/// A person's Design-notes post (prd-browser-design-mode-v1 D-A10): one
/// Thread text item carrying `design`, otherwise exactly
/// [`post_thread_with_origin`].
#[allow(clippy::too_many_arguments)]
pub fn post_thread_design(
    conn: &Connection,
    conversation_id: &str,
    project_id: &str,
    from: &str,
    to: &str,
    body: &str,
    via: &str,
    design: DesignOrigin,
    author: Option<PostAuthor>,
) -> Result<(OverlayItem, Vec<OverlayLink>), String> {
    let _guard = WRITE.lock();
    let seq = catalog::next_thread_seq(conn, conversation_id, project_id)?;
    let id = uuid::Uuid::new_v4().to_string();
    let mut doc = OverlayDoc::text(
        id.clone(),
        from.trim().to_string(),
        to.trim().to_string(),
        body.to_string(),
        via,
    );
    doc.design = Some(design);
    doc.author = author;
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

/// The cards waiting on a person in one conversation (or, `None`, every
/// conversation), as `(conversation, card id)`: pending choice / secret
/// cards whose time has not passed (`now`, unix seconds). thread-read-v1
/// `waiting`: reading never clears it, only an answer, a dismiss/void or
/// the card's expiry.
pub fn waiting_cards(conversation_id: Option<&str>, now: i64) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (conv, card) in store::pending_cards(conversation_id)? {
        match store::get_doc(&card)? {
            Some(doc) if store::is_waiting_card(&doc) && !doc.is_due(now) => out.push((conv, card)),
            _ => {}
        }
    }
    Ok(out)
}

/// True when this Thread doc is a card waiting on a person now.
pub fn is_waiting_now(doc: &OverlayDoc, now: i64) -> bool {
    store::is_waiting_card(doc) && !doc.is_due(now)
}

/// One overlay document by id, whatever conversation it sits in (None when
/// there is no such doc). Callers check it belongs where they expect.
pub fn get_doc(id: &str) -> Result<Option<OverlayDoc>, String> {
    store::get_doc(id)
}

/// One Thread item of this conversation by id (None when it is not there).
pub fn find_thread_item(conversation_id: &str, id: &str) -> Result<Option<OverlayItem>, String> {
    Ok(store::read_thread(conversation_id, 0)?.into_iter().find(|i| i.id == id))
}

/// How long a choice card stays open by default (Rosson 2026-10-13: a
/// card is not an open gate forever). Same as a vault secret request.
pub const DEFAULT_CARD_EXPIRY_SECS: i64 = 7 * 24 * 60 * 60;

/// Unix seconds now (card times).
pub fn now_secs() -> i64 {
    doc::now_secs()
}

/// Post a choice card onto Thread. Returns immediately; card is `pending`.
/// `expires_at` (unix seconds): when the daemon's sweep expires it if it
/// is still pending; None keeps it open until answered or voided.
#[allow(clippy::too_many_arguments)]
pub fn post_choice(
    conn: &Connection,
    conversation_id: &str,
    project_id: &str,
    from: &str,
    to: &str,
    prompt: &str,
    options: Vec<String>,
    allow_custom: bool,
    expires_at: Option<i64>,
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
    let mut doc = OverlayDoc::choice(
        id.clone(),
        from.trim().to_string(),
        to.trim().to_string(),
        prompt.to_string(),
        options,
        allow_custom,
    );
    if let Some(c) = doc.choice.as_mut() {
        c.expires_at = expires_at;
    }
    store::commit_write(&doc, Some((conversation_id, seq)), &[], None)?;
    if let Some(at) = expires_at {
        store::index_card_expiry(&id, conversation_id, at)?;
    }
    Ok(item_and_thread_link(conversation_id, seq, id, doc))
}

/// Post a secret card onto Thread. Value is not stored in the document.
/// `expires_at`: the vault request's expiry (7 days), shown on the card
/// and swept like a choice card's (the vault expires its request too).
#[allow(clippy::too_many_arguments)]
pub fn post_secret(
    conn: &Connection,
    conversation_id: &str,
    project_id: &str,
    from: &str,
    to: &str,
    name: &str,
    prompt: Option<&str>,
    proposal: SecretProposal,
    expires_at: Option<i64>,
) -> Result<(OverlayItem, Vec<OverlayLink>), String> {
    vault::validate_name(name)?;
    let _guard = WRITE.lock();
    let seq = catalog::next_thread_seq(conn, conversation_id, project_id)?;
    let id = uuid::Uuid::new_v4().to_string();
    let prompt = prompt
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let mut doc = OverlayDoc::secret_card(
        id.clone(),
        from.trim().to_string(),
        to.trim().to_string(),
        name.trim().to_string(),
        prompt,
        proposal,
    );
    if let Some(s) = doc.secret.as_mut() {
        s.expires_at = expires_at;
    }
    store::commit_write(&doc, Some((conversation_id, seq)), &[], None)?;
    if let Some(at) = expires_at {
        store::index_card_expiry(&id, conversation_id, at)?;
    }
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

/// Error text for answering or voiding a card that is no longer pending
/// (answered, set, voided or expired). Routes map it to 409
/// `card_not_pending`; a stored answer is never overwritten.
pub const CARD_NOT_PENDING: &str = "card is not pending";

/// Whether `card_id` in this conversation is a secret card (any status).
pub fn card_is_secret(conversation_id: &str, card_id: &str) -> Result<bool, String> {
    Ok(thread_item(conversation_id, card_id)?.doc.secret.is_some())
}

/// Tap an option (or custom) / submit a secret. `by` is who tapped (the
/// caller's credential; None for an agent's own answer), stored on the
/// card as `answeredBy` with `answeredAt`.
pub fn answer_card(
    conversation_id: &str,
    project_id: &str,
    card_id: &str,
    choice_answer: Option<&str>,
    secret_bytes: Option<&[u8]>,
    by: Option<PostAuthor>,
) -> Result<CardCallback, String> {
    let _guard = WRITE.lock();
    let item = thread_item(conversation_id, card_id)?;
    let mut doc = item.doc;
    if doc.imported.is_some() {
        return Err(IMPORTED_READ_ONLY.to_string());
    }
    if doc.is_due(doc::now_secs()) {
        // Past its time: the sweep (or [`expire_if_due`]) closes it.
        return Err(CARD_NOT_PENDING.to_string());
    }
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
        choice.answered_by = by;
        choice.answered_at = Some(doc::now_secs());
        format!("chose {answer}")
    } else if doc.is_pending_secret() {
        // K2 vault (prd-k2-vault-v1 §10.1): the daemon stores the value in
        // the vault and then calls [`mark_secret`]. Nothing here writes a
        // value anywhere.
        let _ = (project_id, secret_bytes);
        return Err("secret cards are answered through the K2 vault".to_string());
    } else {
        return Err(CARD_NOT_PENDING.to_string());
    };
    store::put_card_doc(conversation_id, &doc)?;
    Ok(CardCallback {
        conversation_id: conversation_id.to_string(),
        doc_id: doc.id.clone(),
        seq: item.seq,
        doc,
        inject_line,
    })
}

/// After the vault stored a secret card's value (or the person declined):
/// mark the card `set` / `voided` with the reference. Never takes a value.
/// `by`: the person who answered (`answeredBy`), None for the daemon.
pub fn mark_secret(
    conversation_id: &str,
    card_id: &str,
    status: &str,
    reference: Option<String>,
    inject_line: String,
    by: Option<PostAuthor>,
) -> Result<CardCallback, String> {
    let _guard = WRITE.lock();
    let item = thread_item(conversation_id, card_id)?;
    let mut doc = item.doc;
    if !doc.is_pending_secret() {
        return Err("card is not pending".to_string());
    }
    if let Some(s) = doc.secret.as_mut() {
        s.status = status.to_string();
        s.reference = reference;
        s.answered_by = by;
        s.answered_at = Some(doc::now_secs());
    }
    store::put_card_doc(conversation_id, &doc)?;
    Ok(CardCallback {
        conversation_id: conversation_id.to_string(),
        doc_id: doc.id.clone(),
        seq: item.seq,
        doc,
        inject_line,
    })
}

/// Explicit dismiss (secret X / chrome dismiss) or the asking agent's
/// `k2 thread void` of its own superseded card. `by`: the person who
/// dismissed it (`answeredBy`), None for an agent.
pub fn void_card(
    conversation_id: &str,
    _project_id: &str,
    card_id: &str,
    by: Option<PostAuthor>,
) -> Result<CardCallback, String> {
    let _guard = WRITE.lock();
    let item = thread_item(conversation_id, card_id)?;
    let mut doc = item.doc;
    let at = Some(doc::now_secs());
    if doc.imported.is_some() {
        return Err(IMPORTED_READ_ONLY.to_string());
    }
    if doc.is_due(doc::now_secs()) {
        return Err(CARD_NOT_PENDING.to_string());
    }
    let inject_line = if doc.is_pending_choice() {
        if let Some(c) = doc.choice.as_mut() {
            c.status = "voided".to_string();
            c.answer = None;
            c.answered_by = by;
            c.answered_at = at;
        }
        "card voided".to_string()
    } else if doc.is_pending_secret() {
        // A pending card never wrote a value, so voiding it deletes
        // nothing: a stored value under the same name belongs to an
        // earlier, answered card and must survive.
        let name = doc.secret.as_ref().expect("pending secret").name.clone();
        if let Some(s) = doc.secret.as_mut() {
            s.status = "voided".to_string();
            s.answered_by = by;
            s.answered_at = at;
        }
        format!("secret {name} declined")
    } else {
        return Err(CARD_NOT_PENDING.to_string());
    };
    store::put_card_doc(conversation_id, &doc)?;
    Ok(CardCallback {
        conversation_id: conversation_id.to_string(),
        doc_id: doc.id.clone(),
        seq: item.seq,
        doc,
        inject_line,
    })
}

/// Close a pending card as `expired` (its time passed, or its vault
/// request expired). Not a person's act: `answeredBy` stays empty.
pub fn expire_card(conversation_id: &str, card_id: &str) -> Result<CardCallback, String> {
    let _guard = WRITE.lock();
    expire_locked(conversation_id, card_id)
}

fn expire_locked(conversation_id: &str, card_id: &str) -> Result<CardCallback, String> {
    let item = thread_item(conversation_id, card_id)?;
    let mut doc = item.doc;
    let at = Some(doc::now_secs());
    let inject_line = if doc.is_pending_choice() {
        let c = doc.choice.as_mut().expect("pending choice");
        c.status = "expired".to_string();
        c.answered_at = at;
        "card expired".to_string()
    } else if doc.is_pending_secret() {
        let s = doc.secret.as_mut().expect("pending secret");
        s.status = "expired".to_string();
        s.answered_at = at;
        format!("secret {} request expired", s.name)
    } else {
        return Err(CARD_NOT_PENDING.to_string());
    };
    store::put_card_doc(conversation_id, &doc)?;
    Ok(CardCallback {
        conversation_id: conversation_id.to_string(),
        doc_id: doc.id.clone(),
        seq: item.seq,
        doc,
        inject_line,
    })
}

/// Check-on-touch: a pending card past its `expiresAt` turns `expired`
/// now (the sweep may not have run yet). `Some` when it did.
pub fn expire_if_due(conversation_id: &str, card_id: &str, now: i64) -> Result<Option<CardCallback>, String> {
    let _guard = WRITE.lock();
    let item = thread_item(conversation_id, card_id)?;
    if !item.doc.is_due(now) {
        return Ok(None);
    }
    expire_locked(conversation_id, card_id).map(Some)
}

/// The daemon's sweep: every pending card whose `expiresAt` is `<= now`
/// turns `expired`. Walks the expiry index only. Entries for
/// cards that closed earlier, or whose doc is gone, are dropped.
pub fn expire_due_cards(now: i64) -> Result<Vec<CardCallback>, String> {
    let _guard = WRITE.lock();
    let due = store::due_card_expiries(now)?;
    let mut out = Vec::new();
    let mut done = Vec::with_capacity(due.len());
    for (key, card, conversation) in due {
        match thread_item(&conversation, &card) {
            Ok(item) if item.doc.is_due(now) => {
                out.push(expire_locked(&conversation, &card)?);
            }
            Ok(_) => {}
            Err(e) => crate::log_debug!("[overlay] expiry entry for {card} in {conversation}: {e}"),
        }
        done.push(key);
    }
    store::remove_card_expiries(&done)?;
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
///
/// `delivery_id` is the `m_…` id of the live message's `msg_deliveries`
/// receipt, when the send wrote one; the doc keeps only the id.
#[allow(clippy::too_many_arguments)]
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
    delivery_id: Option<&str>,
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
    let mut doc = OverlayDoc::chatter(
        id,
        from.trim().to_string(),
        to.trim().to_string(),
        body.to_string(),
        via,
        inject,
    );
    doc.delivery_id = delivery_id.map(str::trim).filter(|d| !d.is_empty()).map(str::to_string);
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
            None,
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
            None,
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
    fn chatter_keeps_the_delivery_id_and_only_the_id() {
        let project_id = seed_project("ovl-delivery");
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
            "with receipt",
            "msg",
            "accepted",
            Some("m_7Qx2Ab9cDe"),
        )
        .expect("chatter with id");
        assert_eq!(doc.delivery_id.as_deref(), Some("m_7Qx2Ab9cDe"));
        let items = read_chatter(&conv, 0).expect("read");
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].doc.delivery_id.as_deref(), Some("m_7Qx2Ab9cDe"));
        let wire = serde_json::to_value(&items[0].doc).expect("doc json");
        assert_eq!(wire["deliveryId"], "m_7Qx2Ab9cDe", "{wire}");
        assert_eq!(wire["from"], "sales", "the doc keeps its own from, not the receipt's: {wire}");
        assert!(wire.get("actor").is_none() && wire.get("sender").is_none(), "no receipt fields copied: {wire}");

        let (plain, _) = record_chatter(
            &c,
            &conv,
            &project_id,
            None,
            "sales",
            "sales/reviewer",
            "no receipt",
            "msg",
            "accepted",
            None,
        )
        .expect("chatter without id");
        assert_eq!(plain.delivery_id, None);
        let items = read_chatter(&conv, 0).expect("read");
        assert_eq!(items.len(), 2, "{items:?}");
        assert_eq!(items[1].doc.delivery_id, None);
        let wire = serde_json::to_value(&items[1].doc).expect("doc json");
        assert!(wire.get("deliveryId").is_none(), "absent, not null: {wire}");
    }

    #[test]
    fn chatter_doc_from_before_delivery_id_still_reads() {
        let old = serde_json::json!({
            "id": "d1",
            "kind": "chatter",
            "from": "sales",
            "to": "sales/reviewer",
            "created_at": 1,
            "body": "old",
            "via": "msg",
            "inject": "accepted",
        });
        let doc: OverlayDoc = serde_json::from_value(old).expect("old chatter doc parses");
        assert_eq!(doc.delivery_id, None);
        assert_eq!(doc.body.as_deref(), Some("old"));
        let back = serde_json::to_value(&doc).expect("json");
        assert!(back.get("deliveryId").is_none(), "{back}");
    }

    #[test]
    fn catalog_row_uses_named_conversation_id_not_agent_name() {
        let project_id = seed_project("ovl-named");
        let session_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let dbh = conn();
        let c = dbh.lock();
        crate::db::schema::WorkspaceSession::upsert(
            &c,
            &format!("ws-row-{project_id}"),
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
            None,
        )
        .expect("post_choice");
        assert_eq!(item.doc.kind, "choice", "{item:?}");
        let choice = item.doc.choice.as_ref().expect("choice body");
        assert_eq!(choice.status, "pending", "{choice:?}");
        assert_eq!(choice.options[0].label, "Go", "first option is primary");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].collection, "thread");

        let cb = answer_card(&conv, &project_id, &item.id, Some("Go"), None, Some(PostAuthor::guest("g1"))).expect("tap Go");
        assert_eq!(cb.doc.choice.as_ref().expect("choice").status, "answered");
        assert_eq!(
            cb.doc.choice.as_ref().expect("choice").answer.as_deref(),
            Some("Go")
        );
        assert_eq!(cb.inject_line, "chose Go");
        let answered = cb.doc.choice.as_ref().expect("choice");
        assert_eq!(answered.answered_by, Some(PostAuthor::guest("g1")), "who tapped is on the card");
        assert!(answered.answered_at.is_some_and(|t| t > 0), "when they tapped: {answered:?}");
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
    fn secret_cards_take_no_value_here_and_mark_set_with_a_reference() {
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
            SecretProposal {
                scope: Some("workspace".into()),
                hosts: vec!["https://api.x.ai:443".into()],
                delivery: Some("fetch".into()),
                kind: Some("api_key".into()),
            },
            None,
        )
        .expect("post_secret");
        assert_eq!(item.doc.kind, "secret");
        let sb = item.doc.secret.as_ref().expect("secret");
        assert_eq!(sb.status, "pending");
        assert_eq!(sb.hosts, vec!["https://api.x.ai:443".to_string()]);
        let secret_bytes = b"s3cr3t-NEVER-IN-REDB-xyz";
        let refused = answer_card(&conv, &project_id, &item.id, None, Some(secret_bytes.as_slice()), None);
        assert!(refused.is_err(), "values go to the K2 vault, never through here");
        assert!(!vault::exists(&project_id, "API_TOKEN"), "nothing written to the old folder");
        let cb = mark_secret(&conv, &item.id, "set", Some("k2v://ws/ovl-sec/API_TOKEN".into()), "secret API_TOKEN set".into(), Some(PostAuthor::user("alice")))
            .expect("mark set");
        assert_eq!(cb.doc.secret.as_ref().expect("secret").status, "set");
        assert_eq!(cb.doc.secret.as_ref().expect("secret").reference.as_deref(), Some("k2v://ws/ovl-sec/API_TOKEN"));
        assert_eq!(cb.doc.secret.as_ref().expect("secret").answered_by, Some(PostAuthor::user("alice")));
        assert!(mark_secret(&conv, &item.id, "set", None, "again".into(), None).is_err(), "no second answer");
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
            SecretProposal::default(),
            None,
        )
        .expect("pending");
        let voided = void_card(&conv2, &project_id, &pending.id, Some(PostAuthor::owner())).expect("void");
        assert_eq!(voided.doc.secret.as_ref().expect("s").status, "voided");
        assert!(
            !vault::exists(&project_id, "OTHER_TOKEN"),
            "dismiss/void must leave vault empty"
        );
    }

    /// An answered secret card is final (K2 vault model: the value lives in
    /// the vault, the card only records `set` + the reference): a second
    /// mark is refused, a void after the answer is refused with
    /// [`CARD_NOT_PENDING`], no path here takes a value, and voiding a
    /// later same-name card leaves the first card `set`.
    #[test]
    fn answered_secret_card_is_never_overwritten_or_deleted() {
        let project_id = seed_project("ovl-final");
        let conv = uuid::Uuid::new_v4().to_string();
        let dbh = conn();
        let c = dbh.lock();
        let (first, _) = post_secret(
            &c, &conv, &project_id, "k2", "ovl-final", "FINAL_TOKEN", None, SecretProposal::default(), None,
        )
        .expect("post first");
        assert!(card_is_secret(&conv, &first.id).expect("kind"), "secret card");
        answer_card(&conv, &project_id, &first.id, None, Some(b"human-value".as_slice()), None)
            .expect_err("values never go through answer_card");
        mark_secret(&conv, &first.id, "set", Some("k2v://ws/ovl-final/FINAL_TOKEN".into()), "set".into(), None)
            .expect("first answer");

        let again = mark_secret(&conv, &first.id, "set", Some("k2v://ws/ovl-final/OTHER".into()), "set".into(), None)
            .expect_err("second answer must be refused");
        assert_eq!(again, "card is not pending");
        let void_after = void_card(&conv, &project_id, &first.id, None)
            .expect_err("voiding an answered card must be refused");
        assert_eq!(void_after, CARD_NOT_PENDING);

        let (second, _) = post_secret(
            &c, &conv, &project_id, "k2", "ovl-final", "FINAL_TOKEN", None, SecretProposal::default(), None,
        )
        .expect("post same-name card");
        void_card(&conv, &project_id, &second.id, None).expect("void pending same-name card");
        let (third, _) = post_secret(
            &c, &conv, &project_id, "k2", "ovl-final", "FINAL_TOKEN", None, SecretProposal::default(), None,
        )
        .expect("post third");
        void_card(&conv, &project_id, &third.id, None).expect("void third");
        let first_now = thread_item(&conv, &first.id).expect("first card");
        let sb = first_now.doc.secret.as_ref().expect("secret body");
        assert_eq!(sb.status, "set", "the first answer stays set");
        assert_eq!(sb.reference.as_deref(), Some("k2v://ws/ovl-final/FINAL_TOKEN"));
        assert!(!vault::exists(&project_id, "FINAL_TOKEN"), "nothing written to the old folder");
    }

    /// Rosson 2026-10-13: a card can carry a time frame. The sweep turns a
    /// due pending card `expired` (index only, never a docs scan), leaves
    /// a card with time left or no time frame open, refuses answers after
    /// its time, and follows a conversation move.
    #[test]
    fn choice_cards_expire_on_the_sweep_and_refuse_late_answers() {
        let project_id = seed_project("ovl-exp");
        let conv = uuid::Uuid::new_v4().to_string();
        let dbh = conn();
        let c = dbh.lock();
        let now = now_secs();
        let ask = |prompt: &str, exp: Option<i64>| {
            post_choice(&c, &conv, &project_id, "k2", "ovl-exp", prompt, vec!["Go".into(), "Stop".into()], false, exp)
                .expect("post_choice")
                .0
        };
        let due = ask("due", Some(now - 5));
        let later = ask("later", Some(now + 3_600));
        let open = ask("open", None);
        assert_eq!(due.doc.choice.as_ref().expect("c").expires_at, Some(now - 5), "stored on the card");
        assert_eq!(open.doc.choice.as_ref().expect("c").expires_at, None, "--no-expiry stores none");

        // Past its time, the card refuses an answer and a void even before the sweep.
        assert_eq!(answer_card(&conv, &project_id, &due.id, Some("Go"), None, None).expect_err("late"), CARD_NOT_PENDING);
        assert_eq!(void_card(&conv, &project_id, &due.id, None).expect_err("late void"), CARD_NOT_PENDING);

        let swept = expire_due_cards(now).expect("sweep");
        let ids: Vec<&str> = swept.iter().filter(|cb| cb.conversation_id == conv).map(|cb| cb.doc_id.as_str()).collect();
        assert_eq!(ids, vec![due.id.as_str()], "only the due card: {swept:?}");
        let cb = swept.iter().find(|cb| cb.doc_id == due.id).expect("due cb");
        assert_eq!(cb.doc.choice.as_ref().expect("c").status, "expired");
        assert!(cb.doc.choice.as_ref().expect("c").answered_at.is_some());
        assert_eq!(cb.doc.choice.as_ref().expect("c").answered_by, None, "expiry is nobody's act");
        assert_eq!(cb.inject_line, "card expired");
        assert!(
            expire_due_cards(now).expect("again").iter().all(|cb| cb.doc_id != due.id),
            "a swept card is not swept twice"
        );
        let status = |id: &str| thread_item(&conv, id).expect("card").doc.choice.expect("c").status;
        assert_eq!(status(&due.id), "expired");
        assert_eq!(status(&later.id), "pending", "time left: still open");
        assert_eq!(status(&open.id), "pending", "no time frame: open");

        // An answered card's entry is dropped quietly when its time comes.
        answer_card(&conv, &project_id, &later.id, Some("Go"), None, None).expect("answer in time");
        assert!(expire_due_cards(now + 3_600).expect("later sweep").iter().all(|cb| cb.doc_id != later.id));
        assert_eq!(status(&later.id), "answered");

        // check-on-touch expires a due card without the sweep.
        let touch = ask("touch", Some(now - 1));
        let hit = expire_if_due(&conv, &touch.id, now).expect("touch").expect("expired now");
        assert_eq!(hit.doc.choice.as_ref().expect("c").status, "expired");
        assert!(expire_if_due(&conv, &open.id, now).expect("open").is_none());

        // The index follows a conversation move.
        let moving = ask("moving", Some(now - 2));
        let to = uuid::Uuid::new_v4().to_string();
        move_conversation(&c, &conv, &to).expect("move").expect("moved");
        let moved = expire_due_cards(now).expect("sweep after move");
        let cb = moved.iter().find(|cb| cb.doc_id == moving.id).expect("moved card swept");
        assert_eq!(cb.conversation_id, to, "the frame names the new conversation");
    }

    #[test]
    fn expire_card_closes_a_pending_secret_card_once() {
        let project_id = seed_project("ovl-exps");
        let conv = uuid::Uuid::new_v4().to_string();
        let dbh = conn();
        let c = dbh.lock();
        let (item, _) = post_secret(&c, &conv, &project_id, "k2", "ovl-exps", "OLD_TOKEN", None, SecretProposal::default(), Some(now_secs() + 60))
            .expect("secret");
        assert_eq!(item.doc.secret.as_ref().expect("s").expires_at.map(|t| t > 0), Some(true));
        assert!(expire_if_due(&conv, &item.id, now_secs()).expect("not yet").is_none(), "time left");
        let cb = expire_card(&conv, &item.id).expect("expire");
        assert_eq!(cb.doc.secret.as_ref().expect("s").status, "expired");
        assert_eq!(cb.inject_line, "secret OLD_TOKEN request expired");
        assert_eq!(expire_card(&conv, &item.id).expect_err("once"), CARD_NOT_PENDING);
        assert!(mark_secret(&conv, &item.id, "set", None, "late".into(), None).is_err(), "no answer after expiry");
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
                None,
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

    fn bodies(items: &[OverlayItem]) -> Vec<String> {
        items
            .iter()
            .map(|i| i.doc.body.clone().expect("text body"))
            .collect()
    }

    /// S6 (prd-thread-survives-tab-rename-v1 Side finding A): the pane
    /// key's Thread moves to the provider id. A reply that raced in under
    /// the provider id is merged by time, nothing is lost or duplicated,
    /// and seqs are renumbered 1..n with the next post after them.
    #[test]
    fn move_conversation_merges_raced_reply_in_time_order_without_duplicates() {
        let project_id = seed_project("ovl-move");
        let pane = format!("pane-{}", uuid::Uuid::new_v4());
        let provider = uuid::Uuid::new_v4().to_string();
        let dbh = conn();
        let c = dbh.lock();

        let (first, _) = post_thread(&c, &pane, &project_id, "rosson", "ovl-move/1", "first", "compose")
            .expect("first");
        let (second, _) = post_thread(&c, &pane, &project_id, "rosson", "ovl-move/1", "second", "compose")
            .expect("second");
        record_chatter(&c, &pane, &project_id, None, "sales", "ovl-move/1", "ping", "msg", "accepted", None)
            .expect("chatter");
        // The agent's reply raced in under the provider id (same second as
        // the pane rows: a tie keeps the pane rows first).
        post_thread(&c, &provider, &project_id, "ovl-move/1", "ovl-move/1", "reply", "thread")
            .expect("raced reply");
        // An older doc under the provider id sorts by time, not by key.
        let early_id = uuid::Uuid::new_v4().to_string();
        let mut early = OverlayDoc::text(
            early_id.clone(),
            "ovl-move/1".into(),
            "ovl-move/1".into(),
            "early".into(),
            "thread",
        );
        early.created_at = first.doc.created_at - 100;
        let early_seq = catalog::next_thread_seq(&c, &provider, &project_id).expect("seq");
        store::commit_write(&early, Some((provider.as_str(), early_seq)), &[], None).expect("early");
        // The same doc linked under both keys must come out once.
        let dup_seq = catalog::next_thread_seq(&c, &provider, &project_id).expect("seq");
        store::link_thread(&provider, dup_seq, &second.id).expect("dup link");

        let mv = move_conversation(&c, &pane, &provider)
            .expect("move")
            .expect("something moved");
        assert_eq!(mv.from, pane);
        assert_eq!(mv.to, provider);
        assert_eq!(mv.project_id, project_id);
        assert_eq!(mv.thread_moved, 2, "{mv:?}");
        assert_eq!(mv.thread_total, 4, "first, second, reply, early (dup dropped): {mv:?}");
        assert_eq!(mv.chatter_moved, 1, "{mv:?}");
        assert_eq!(mv.chatter_total, 1, "{mv:?}");

        let items = read_thread(&provider, 0).expect("read provider");
        assert_eq!(bodies(&items), ["early", "first", "second", "reply"], "{items:?}");
        let seqs: Vec<i64> = items.iter().map(|i| i.seq).collect();
        assert_eq!(seqs, [1, 2, 3, 4], "renumbered after the move: {items:?}");
        assert_eq!(items[1].id, first.id, "doc ids are kept");
        assert_eq!(items[2].id, second.id);
        let (thread_n, _, _) = store::debug_pointer_count(&second.id).expect("count");
        assert_eq!(thread_n, 1, "one pointer per doc after the merge");
        assert!(read_thread(&pane, 0).expect("read pane").is_empty(), "nothing left on the pane key");
        let chatter = read_chatter(&provider, 0).expect("chatter");
        assert_eq!(chatter.len(), 1, "{chatter:?}");
        assert_eq!(chatter[0].doc.body.as_deref(), Some("ping"));
        assert!(read_chatter(&pane, 0).expect("pane chatter").is_empty());

        assert!(catalog::get(&c, &pane).expect("get").is_none(), "pane catalog row must go");
        let (proj, last_thread, last_chatter) =
            catalog::get(&c, &provider).expect("get").expect("provider row");
        assert_eq!(proj, project_id);
        assert_eq!(last_thread, 4, "last seq = the merged count (above the old 3)");
        assert_eq!(last_chatter, 1);

        let (next, _) = post_thread(&c, &provider, &project_id, "rosson", "ovl-move/1", "next", "compose")
            .expect("next");
        assert_eq!(next.seq, 5, "the next post lands after the moved rows");
        assert_eq!(
            bodies(&read_thread(&provider, 0).expect("read")),
            ["early", "first", "second", "reply", "next"]
        );

        // Idempotent: a second move (the stamp runs again) is a no-op.
        assert!(move_conversation(&c, &pane, &provider).expect("again").is_none());
        assert_eq!(read_thread(&provider, 0).expect("read").len(), 5);
    }

    /// The real adoption door: `stamp_session_id` on a Codex tab keyed on
    /// its pane moves the Thread with the handle, so `ws/1` still shows
    /// the first message. A Claude-style tab (id known at launch) is
    /// untouched.
    #[test]
    fn stamp_session_id_moves_pane_keyed_thread_with_the_handle() {
        let project_id = seed_project("ovl-stamp");
        let dbh = conn();
        let c = dbh.lock();
        let pane = format!("pane{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let agent = format!("tab-{pane}");
        c.execute(
            "INSERT INTO workspace_tab_sessions (project_id, pane_group_id, agent_name, command, last_seen_at) \
             VALUES (?1, ?2, ?3, 'codex', unixepoch())",
            params![project_id, pane, agent],
        )
        .expect("codex tab");
        let handle = crate::workspace_session_handles::ensure_sidecar_handle(
            &c, &project_id, &agent, Some("codex"), None, &pane,
        )
        .expect("handle")
        .expect("a codex tab is a sidecar");
        assert_eq!(
            crate::workspace_session_handles::resolve_handle(&c, &project_id, &handle).expect("resolve"),
            pane,
            "before discovery the Thread key is the pane"
        );
        post_thread(&c, &pane, &project_id, "rosson", "ovl-stamp/1", "hello codex", "compose")
            .expect("first message");
        c.execute(
            "INSERT INTO llm_session_logins (session_key, tool, conversation_id, account_id, home, recorded_at) \
             VALUES (?1, 'codex', '', 'acct-1', '/slot', 1)",
            params![agent],
        )
        .expect("fresh spawn login");

        let codex_id = uuid::Uuid::new_v4().to_string();
        crate::db::schema::WorkspaceTabSession::stamp_session_id(&c, &project_id, &pane, &codex_id)
            .expect("stamp");
        assert_eq!(
            crate::workspace_session_handles::resolve_handle(&c, &project_id, &handle).expect("resolve"),
            codex_id,
            "the handle follows the provider id"
        );
        post_thread(&c, &codex_id, &project_id, "ovl-stamp/1", "ovl-stamp/1", "hi from codex", "thread")
            .expect("reply");
        assert_eq!(
            bodies(&read_thread(&codex_id, 0).expect("read")),
            ["hello codex", "hi from codex"],
            "the address shows the first message and the reply"
        );
        assert!(read_thread(&pane, 0).expect("pane").is_empty());
        let login: Option<String> = c
            .query_row(
                "SELECT account_id FROM llm_session_logins WHERE session_key = ?1 AND tool = 'codex' AND conversation_id = ?2",
                params![agent, codex_id],
                |r| r.get(0),
            )
            .expect("login recorded under the adopted id");
        assert_eq!(login.as_deref(), Some("acct-1"));

        // Re-stamping the same id is a no-op (no duplicates).
        crate::db::schema::WorkspaceTabSession::stamp_session_id(&c, &project_id, &pane, &codex_id)
            .expect("stamp again");
        assert_eq!(read_thread(&codex_id, 0).expect("read").len(), 2);

        // Claude/Grok: keyed on the provider id from launch. Stamping does
        // not touch its Thread (nothing under its pane key).
        let cpane = format!("pane{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let cagent = format!("tab-{cpane}");
        let claude_id = uuid::Uuid::new_v4().to_string();
        c.execute(
            "INSERT INTO workspace_tab_sessions (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
             VALUES (?1, ?2, ?3, ?4, 'claude', unixepoch())",
            params![project_id, cpane, cagent, claude_id],
        )
        .expect("claude tab");
        crate::workspace_session_handles::ensure_sidecar_handle(
            &c, &project_id, &cagent, Some("claude"), Some(&claude_id), &cpane,
        )
        .expect("claude handle");
        post_thread(&c, &claude_id, &project_id, "rosson", "ovl-stamp/2", "to claude", "compose")
            .expect("claude post");
        crate::db::schema::WorkspaceTabSession::stamp_session_id(&c, &project_id, &cpane, &claude_id)
            .expect("claude stamp");
        let claude_items = read_thread(&claude_id, 0).expect("claude read");
        assert_eq!(bodies(&claude_items), ["to claude"]);
        assert_eq!(claude_items[0].seq, 1);
        assert!(catalog::get(&c, &cpane).expect("get").is_none());
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
