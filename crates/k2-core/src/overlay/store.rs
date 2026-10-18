//! redb document heap + three pointer tables.
//!
//! ```text
//! docs          id                  → JSON body
//! thread        conv/seq:020        → id
//! chatter       conv/seq:020        → id
//! chatterlog    seq:020             → id
//! card_expiry   expires:020/id      → conv   (cards with an expiresAt)
//! pending_cards conv/id             → ""     (cards waiting on a person)
//! meta          key                 → value  (one-time index builds)
//! import_ids    project/host/doc    → id     (imported lines, dedupe)
//! thread_changes conv/mark:020/id   → seq | removed:seq  (thread-delta-v1)
//! ```
//!
//! Never query by scanning `docs/`. Walk a collection prefix, then point-get.

use std::path::PathBuf;
use std::sync::OnceLock;

use redb::{Database, ReadableTable, TableDefinition};

use super::doc::{OverlayDoc, OverlayItem};

const DOCS: TableDefinition<&str, &[u8]> = TableDefinition::new("docs");
const THREAD: TableDefinition<&str, &str> = TableDefinition::new("thread");
const CHATTER: TableDefinition<&str, &str> = TableDefinition::new("chatter");
const CHATTERLOG: TableDefinition<&str, &str> = TableDefinition::new("chatterlog");
/// Cards that expire, by when: `{expires_at:020}/{card id}` → conversation.
/// The daemon's sweep walks the due prefix only (never the docs). An entry
/// whose card closed earlier is dropped when its time comes.
const CARD_EXPIRY: TableDefinition<&str, &str> = TableDefinition::new("card_expiry");
/// Pending choice / secret cards, by conversation: `{conv}/{card id}` →
/// `""` (thread-read-v1 `waiting`). Written with the card, removed when
/// it is answered, set, voided or expired (in the same write), moved with
/// its conversation. A card closes in place (no new seq), so without this
/// index "how many cards wait on a person" would mean reading every doc.
const PENDING_CARDS: TableDefinition<&str, &str> = TableDefinition::new("pending_cards");
/// One-time index builds: `pending_cards_v1` → `"1"` once every pending
/// card written before the index existed was added to it.
const META: TableDefinition<&str, &str> = TableDefinition::new("meta");
const META_PENDING_V1: &str = "pending_cards_v1";

/// Collected conversations (lines K2 gathers from an outside agent
/// product, e.g. a local Grok Bot): `{conv}/{product entry id}` → doc id.
/// The dedupe index: a line the product reports again (or under a second
/// id) updates or links the one doc instead of adding another.
const EXT_IDS: TableDefinition<&str, &str> = TableDefinition::new("ext_ids");
/// Collected conversations: `{conv}` → the collector's JSON cursor
/// (high-water marks, counters). Never a token.
const EXT_STATE: TableDefinition<&str, &[u8]> = TableDefinition::new("ext_state");
/// Imported Thread lines (a clone carries its Threads, CL6):
/// `{project_id}/{source_host}/{source_doc_id}` -> local doc id. Keyed by
/// doc, not conversation, so it survives [`move_conversation`] (a pending
/// import settling onto the pin) and a re-run never adds a line twice.
const IMPORT_IDS: TableDefinition<&str, &str> = TableDefinition::new("import_ids");
/// Thread docs that changed after they were posted (thread-delta-v1):
/// `{conv}/{mark:020}/{doc id}` → `"{seq}"` (rewritten in place: a card
/// answered, set, voided or expired, a handoff closed, a collected line
/// updated) or `"removed:{seq}"` (a split took it out of this
/// conversation). `mark` is the conversation's highest Thread seq when the
/// change was written (before it), so a reader that holds every seq up to
/// N has seen each change marked below N and needs the ones marked `>= N`
/// (`GET /cli/thread?after_seq=N`). Written in the change's own redb
/// transaction. One small row per change; never pruned (a card changes
/// once).
const THREAD_CHANGES: TableDefinition<&str, &str> = TableDefinition::new("thread_changes");
const CHANGE_REMOVED_PREFIX: &str = "removed:";

static DB: OnceLock<Result<Database, String>> = OnceLock::new();

fn store_path() -> PathBuf {
    if let Ok(p) = std::env::var("K2_THREADS_REDB") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    #[cfg(any(test, feature = "test-util"))]
    {
        return test_store_path();
    }
    #[cfg(not(any(test, feature = "test-util")))]
    {
        crate::paths::k2_home().join("k2-threads.redb")
    }
}

#[cfg(any(test, feature = "test-util"))]
fn test_store_path() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "k2-threads-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("k2-threads.redb")
    })
    .clone()
}

fn db() -> Result<&'static Database, String> {
    match DB.get_or_init(|| {
        let path = store_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("overlay redb mkdir {}: {e}", parent.display()))?;
        }
        let db = Database::create(&path)
            .map_err(|e| format!("overlay redb create {}: {e}", path.display()))?;
        // Thread message bodies: owner-only (redb is one file, no WAL).
        if let Err(e) = crate::private_home::restrict_path(&path) {
            crate::log_debug!("[overlay] WARN {e}");
        }
        Ok(db)
    }) {
        Ok(db) => Ok(db),
        Err(e) => Err(e.clone()),
    }
}

fn seq_pad(seq: i64) -> String {
    format!("{seq:020}")
}

fn conv_key(conversation_id: &str, seq: i64) -> String {
    format!("{}/{}", conversation_id, seq_pad(seq))
}

/// Exclusive end of a `{conv}/` prefix range. ':' is the ASCII char after '9'.
fn conv_end(conversation_id: &str) -> String {
    format!("{conversation_id}/:")
}

/// Insert one body. Collections store ids only — never copy the JSON.
pub fn put_doc(doc: &OverlayDoc) -> Result<(), String> {
    let json = serde_json::to_vec(doc).map_err(|e| format!("overlay doc json: {e}"))?;
    let db = db()?;
    let txn = db
        .begin_write()
        .map_err(|e| format!("overlay redb write: {e}"))?;
    {
        let mut table = txn
            .open_table(DOCS)
            .map_err(|e| format!("overlay open docs: {e}"))?;
        table
            .insert(doc.id.as_str(), json.as_slice())
            .map_err(|e| format!("overlay insert docs: {e}"))?;
    }
    txn.commit()
        .map_err(|e| format!("overlay redb commit: {e}"))?;
    Ok(())
}

/// A card whose status changed (answered, set, voided, expired, a handoff
/// landed or declined): the body, its `pending_cards` entry and its
/// `thread_changes` row (thread-delta-v1) in ONE write. A card that is no
/// longer pending leaves the index. `seq` is the card's Thread seq.
pub fn put_card_doc(conversation_id: &str, seq: i64, doc: &OverlayDoc) -> Result<(), String> {
    let json = serde_json::to_vec(doc).map_err(|e| format!("overlay doc json: {e}"))?;
    let db = db()?;
    let txn = db.begin_write().map_err(|e| format!("overlay redb write: {e}"))?;
    {
        let mut table = txn.open_table(DOCS).map_err(|e| format!("overlay open docs: {e}"))?;
        table
            .insert(doc.id.as_str(), json.as_slice())
            .map_err(|e| format!("overlay insert docs: {e}"))?;
        let mut pending = txn
            .open_table(PENDING_CARDS)
            .map_err(|e| format!("overlay open pending_cards: {e}"))?;
        let key = pending_key(conversation_id, &doc.id);
        if is_waiting_card(doc) {
            pending.insert(key.as_str(), "").map_err(|e| format!("overlay insert pending_cards: {e}"))?;
        } else {
            pending.remove(key.as_str()).map_err(|e| format!("overlay pending_cards remove: {e}"))?;
        }
    }
    note_change(&txn, conversation_id, &doc.id, Some(seq))?;
    txn.commit().map_err(|e| format!("overlay redb commit: {e}"))
}

// ── thread-delta-v1 change log ─────────────────────────────────────────

fn change_key(conversation_id: &str, mark: i64, doc_id: &str) -> String {
    format!("{conversation_id}/{}/{doc_id}", seq_pad(mark))
}

/// `(mark, doc id)` of a `thread_changes` key of `conversation_id`, or
/// None when the key belongs to another conversation that shares the
/// prefix (conversation ids may hold '/').
fn parse_change_key<'a>(conversation_id: &str, key: &'a str) -> Option<(i64, &'a str)> {
    let (rest, id) = key.rsplit_once('/')?;
    let (conv, mark) = rest.rsplit_once('/')?;
    if conv != conversation_id || mark.len() != 20 {
        return None;
    }
    Some((mark.parse().ok()?, id))
}

/// The highest Thread seq of `conversation_id` in `table` (0 when empty).
fn last_thread_seq<T: ReadableTable<&'static str, &'static str>>(
    table: &T,
    conversation_id: &str,
) -> Result<i64, String> {
    let start = format!("{conversation_id}/");
    let end = conv_end(conversation_id);
    let mut range = table
        .range(start.as_str()..end.as_str())
        .map_err(|e| format!("overlay thread range: {e}"))?;
    match range.next_back() {
        Some(entry) => {
            let (k, v) = entry.map_err(|e| format!("overlay thread iter: {e}"))?;
            Ok(parse_conv_pointer("thread", k.value(), v.value())?.0)
        }
        None => Ok(0),
    }
}

/// Record that Thread doc `doc_id` of `conversation_id` changed in place,
/// marked with the conversation's highest seq now (a split writes its
/// removals itself). `seq` None: look the doc's seq up (newest first); a
/// doc that is not on this Thread records nothing.
fn note_change(
    txn: &redb::WriteTransaction,
    conversation_id: &str,
    doc_id: &str,
    seq: Option<i64>,
) -> Result<(), String> {
    let (mark, seq) = {
        let thread = txn.open_table(THREAD).map_err(|e| format!("overlay open thread: {e}"))?;
        let mark = last_thread_seq(&thread, conversation_id)?;
        let seq = match seq {
            Some(s) => s,
            None => {
                let start = format!("{conversation_id}/");
                let end = conv_end(conversation_id);
                let mut found = None;
                for entry in thread
                    .range(start.as_str()..end.as_str())
                    .map_err(|e| format!("overlay thread range: {e}"))?
                    .rev()
                {
                    let (k, v) = entry.map_err(|e| format!("overlay thread iter: {e}"))?;
                    if v.value() == doc_id {
                        found = Some(parse_conv_pointer("thread", k.value(), v.value())?.0);
                        break;
                    }
                }
                match found {
                    Some(s) => s,
                    None => return Ok(()),
                }
            }
        };
        (mark, seq)
    };
    let value = seq.to_string();
    let mut changes = txn
        .open_table(THREAD_CHANGES)
        .map_err(|e| format!("overlay open thread_changes: {e}"))?;
    changes
        .insert(change_key(conversation_id, mark, doc_id).as_str(), value.as_str())
        .map_err(|e| format!("overlay insert thread_changes: {e}"))?;
    Ok(())
}

/// Drop every `thread_changes` row of `conversation_id`: a move or a
/// split's merge renumbered its seqs (readers start over on the new key).
fn drop_changes(txn: &redb::WriteTransaction, conversation_id: &str) -> Result<(), String> {
    let mut changes = txn
        .open_table(THREAD_CHANGES)
        .map_err(|e| format!("overlay open thread_changes: {e}"))?;
    let start = format!("{conversation_id}/");
    let end = conv_end(conversation_id);
    let mut keys = Vec::new();
    for entry in changes
        .range(start.as_str()..end.as_str())
        .map_err(|e| format!("overlay thread_changes range: {e}"))?
    {
        let (k, _) = entry.map_err(|e| format!("overlay thread_changes iter: {e}"))?;
        if parse_change_key(conversation_id, k.value()).is_some() {
            keys.push(k.value().to_string());
        }
    }
    for k in keys {
        changes.remove(k.as_str()).map_err(|e| format!("overlay thread_changes remove: {e}"))?;
    }
    Ok(())
}

/// A Thread doc that is a card waiting on a person: a pending choice or
/// secret card that is not imported history or a collected line.
/// (Whether its time has passed is the reader's question.)
pub fn is_waiting_card(doc: &OverlayDoc) -> bool {
    (doc.is_pending_choice() || doc.is_pending_secret()) && doc.imported.is_none() && doc.external.is_none()
}

fn pending_key(conversation_id: &str, card_id: &str) -> String {
    format!("{conversation_id}/{card_id}")
}

/// Exclusive end of a `{conv}/` pending-card prefix: card ids hold
/// letters, which sort after `conv_end`'s ':', so the end is `{conv}0`
/// ('0' is the ASCII char after '/').
fn pending_end(conversation_id: &str) -> String {
    format!("{conversation_id}0")
}

/// Every indexed pending card as `(conversation, card id)`, of one
/// conversation or (`None`) of all. The first call on a store written
/// before the index existed builds it from the Thread pointers once.
pub fn pending_cards(conversation_id: Option<&str>) -> Result<Vec<(String, String)>, String> {
    ensure_pending_index()?;
    let db = db()?;
    let txn = db.begin_read().map_err(|e| format!("overlay redb read: {e}"))?;
    let table = match txn.open_table(PENDING_CARDS) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
        Err(e) => return Err(format!("overlay open pending_cards: {e}")),
    };
    let mut out = Vec::new();
    match conversation_id {
        Some(conv) => {
            let start = format!("{conv}/");
            let end = pending_end(conv);
            for entry in table.range(start.as_str()..end.as_str()).map_err(|e| format!("overlay pending_cards range: {e}"))? {
                let (k, _) = entry.map_err(|e| format!("overlay pending_cards iter: {e}"))?;
                // Conversation ids may hold '/', card ids (uuids) never do;
                // a longer conversation id sharing the prefix is not ours.
                if let Some((c, card)) = k.value().rsplit_once('/') {
                    if c == conv {
                        out.push((c.to_string(), card.to_string()));
                    }
                }
            }
        }
        None => {
            for entry in table.iter().map_err(|e| format!("overlay pending_cards iter: {e}"))? {
                let (k, _) = entry.map_err(|e| format!("overlay pending_cards iter: {e}"))?;
                if let Some((c, card)) = k.value().rsplit_once('/') {
                    out.push((c.to_string(), card.to_string()));
                }
            }
        }
    }
    Ok(out)
}

/// Build `pending_cards` from the Thread pointers, once per store (a
/// store from before 0.46.1 has pending cards but no index). One write
/// transaction, so a concurrent card write lands before or after it.
fn ensure_pending_index() -> Result<(), String> {
    let db = db()?;
    {
        let txn = db.begin_read().map_err(|e| format!("overlay redb read: {e}"))?;
        match txn.open_table(META) {
            Ok(t) => {
                if t.get(META_PENDING_V1).map_err(|e| format!("overlay get meta: {e}"))?.is_some() {
                    return Ok(());
                }
            }
            Err(redb::TableError::TableDoesNotExist(_)) => {}
            Err(e) => return Err(format!("overlay open meta: {e}")),
        }
    }
    let txn = db.begin_write().map_err(|e| format!("overlay redb write: {e}"))?;
    {
        let mut meta = txn.open_table(META).map_err(|e| format!("overlay open meta: {e}"))?;
        if meta.get(META_PENDING_V1).map_err(|e| format!("overlay get meta: {e}"))?.is_none() {
            let thread = txn.open_table(THREAD).map_err(|e| format!("overlay open thread: {e}"))?;
            let docs = txn.open_table(DOCS).map_err(|e| format!("overlay open docs: {e}"))?;
            let mut pending = txn
                .open_table(PENDING_CARDS)
                .map_err(|e| format!("overlay open pending_cards: {e}"))?;
            let mut found: Vec<String> = Vec::new();
            for entry in thread.iter().map_err(|e| format!("overlay thread iter: {e}"))? {
                let (k, v) = entry.map_err(|e| format!("overlay thread iter: {e}"))?;
                let Some((conv, _seq)) = k.value().rsplit_once('/') else {
                    continue;
                };
                let Some(guard) = docs.get(v.value()).map_err(|e| format!("overlay get doc: {e}"))? else {
                    continue;
                };
                // A doc that does not parse is not a card anyone can answer.
                let Ok(doc) = serde_json::from_slice::<OverlayDoc>(guard.value()) else {
                    continue;
                };
                if is_waiting_card(&doc) {
                    found.push(pending_key(conv, &doc.id));
                }
            }
            for key in found {
                pending.insert(key.as_str(), "").map_err(|e| format!("overlay insert pending_cards: {e}"))?;
            }
            meta.insert(META_PENDING_V1, "1").map_err(|e| format!("overlay insert meta: {e}"))?;
        }
    }
    txn.commit().map_err(|e| format!("overlay redb commit: {e}"))
}

/// Pending-card entries follow their cards to the new conversation key
/// (in the move's own transaction). `take` limits it to the cards a split
/// moved.
fn retarget_pending_cards(
    txn: &redb::WriteTransaction,
    docs: &redb::Table<&str, &[u8]>,
    from: &str,
    to: &str,
    take: Option<&dyn Fn(&OverlayDoc) -> bool>,
) -> Result<(), String> {
    let mut table = txn.open_table(PENDING_CARDS).map_err(|e| format!("overlay open pending_cards: {e}"))?;
    let start = format!("{from}/");
    let end = pending_end(from);
    let mut hits = Vec::new();
    for entry in table.range(start.as_str()..end.as_str()).map_err(|e| format!("overlay pending_cards range: {e}"))? {
        let (k, _) = entry.map_err(|e| format!("overlay pending_cards iter: {e}"))?;
        if let Some((conv, card)) = k.value().rsplit_once('/') {
            if conv == from {
                hits.push(card.to_string());
            }
        }
    }
    for card in hits {
        if let Some(take) = take {
            let moved = match docs.get(card.as_str()).map_err(|e| format!("overlay get doc: {e}"))? {
                Some(g) => serde_json::from_slice::<OverlayDoc>(g.value()).map(|d| take(&d)).unwrap_or(false),
                None => false,
            };
            if !moved {
                continue;
            }
        }
        table
            .remove(pending_key(from, &card).as_str())
            .map_err(|e| format!("overlay pending_cards remove: {e}"))?;
        table
            .insert(pending_key(to, &card).as_str(), "")
            .map_err(|e| format!("overlay pending_cards retarget: {e}"))?;
    }
    Ok(())
}

pub fn get_doc(id: &str) -> Result<Option<OverlayDoc>, String> {
    let db = db()?;
    let txn = db
        .begin_read()
        .map_err(|e| format!("overlay redb read: {e}"))?;
    let table = txn
        .open_table(DOCS)
        .map_err(|e| format!("overlay open docs: {e}"))?;
    match table.get(id).map_err(|e| format!("overlay get doc: {e}"))? {
        Some(guard) => {
            let doc: OverlayDoc = serde_json::from_slice(guard.value())
                .map_err(|e| format!("overlay doc parse {id}: {e}"))?;
            Ok(Some(doc))
        }
        None => Ok(None),
    }
}

pub fn link_thread(conversation_id: &str, seq: i64, doc_id: &str) -> Result<(), String> {
    link_conv(THREAD, conversation_id, seq, doc_id)
}

pub fn link_chatter(conversation_id: &str, seq: i64, doc_id: &str) -> Result<(), String> {
    link_conv(CHATTER, conversation_id, seq, doc_id)
}

pub fn link_chatterlog(seq: i64, doc_id: &str) -> Result<(), String> {
    let key = seq_pad(seq);
    let db = db()?;
    let txn = db
        .begin_write()
        .map_err(|e| format!("overlay redb write: {e}"))?;
    {
        let mut table = txn
            .open_table(CHATTERLOG)
            .map_err(|e| format!("overlay open chatterlog: {e}"))?;
        table
            .insert(key.as_str(), doc_id)
            .map_err(|e| format!("overlay insert chatterlog: {e}"))?;
    }
    txn.commit()
        .map_err(|e| format!("overlay redb commit: {e}"))?;
    Ok(())
}

fn link_conv(
    def: TableDefinition<&str, &str>,
    conversation_id: &str,
    seq: i64,
    doc_id: &str,
) -> Result<(), String> {
    let key = conv_key(conversation_id, seq);
    let db = db()?;
    let txn = db
        .begin_write()
        .map_err(|e| format!("overlay redb write: {e}"))?;
    {
        let mut table = txn
            .open_table(def)
            .map_err(|e| format!("overlay open collection: {e}"))?;
        table
            .insert(key.as_str(), doc_id)
            .map_err(|e| format!("overlay insert collection: {e}"))?;
    }
    txn.commit()
        .map_err(|e| format!("overlay redb commit: {e}"))?;
    Ok(())
}

/// Write the body once and all collection pointers in a single redb txn.
pub fn commit_write(
    doc: &OverlayDoc,
    thread: Option<(&str, i64)>,
    chatter: &[(String, i64)],
    chatterlog_seq: Option<i64>,
) -> Result<(), String> {
    let json = serde_json::to_vec(doc).map_err(|e| format!("overlay doc json: {e}"))?;
    let db = db()?;
    let txn = db
        .begin_write()
        .map_err(|e| format!("overlay redb write: {e}"))?;
    {
        let mut docs = txn
            .open_table(DOCS)
            .map_err(|e| format!("overlay open docs: {e}"))?;
        docs.insert(doc.id.as_str(), json.as_slice())
            .map_err(|e| format!("overlay insert docs: {e}"))?;
    }
    if let Some((conv, seq)) = thread {
        let key = conv_key(conv, seq);
        let mut table = txn
            .open_table(THREAD)
            .map_err(|e| format!("overlay open thread: {e}"))?;
        table
            .insert(key.as_str(), doc.id.as_str())
            .map_err(|e| format!("overlay insert thread: {e}"))?;
        if is_waiting_card(doc) {
            let mut pending = txn
                .open_table(PENDING_CARDS)
                .map_err(|e| format!("overlay open pending_cards: {e}"))?;
            pending
                .insert(pending_key(conv, &doc.id).as_str(), "")
                .map_err(|e| format!("overlay insert pending_cards: {e}"))?;
        }
    }
    if !chatter.is_empty() {
        let mut table = txn
            .open_table(CHATTER)
            .map_err(|e| format!("overlay open chatter: {e}"))?;
        for (conv, seq) in chatter {
            let key = conv_key(conv, *seq);
            table
                .insert(key.as_str(), doc.id.as_str())
                .map_err(|e| format!("overlay insert chatter: {e}"))?;
        }
    }
    if let Some(seq) = chatterlog_seq {
        let key = seq_pad(seq);
        let mut table = txn
            .open_table(CHATTERLOG)
            .map_err(|e| format!("overlay open chatterlog: {e}"))?;
        table
            .insert(key.as_str(), doc.id.as_str())
            .map_err(|e| format!("overlay insert chatterlog: {e}"))?;
    }
    txn.commit()
        .map_err(|e| format!("overlay redb commit: {e}"))?;
    Ok(())
}

fn expiry_key(expires_at: i64, card_id: &str) -> String {
    format!("{:020}/{card_id}", expires_at.max(0))
}

/// Index a card that expires at `expires_at` (unix seconds).
pub fn index_card_expiry(card_id: &str, conversation_id: &str, expires_at: i64) -> Result<(), String> {
    let db = db()?;
    let txn = db.begin_write().map_err(|e| format!("overlay redb write: {e}"))?;
    {
        let mut table = txn.open_table(CARD_EXPIRY).map_err(|e| format!("overlay open card_expiry: {e}"))?;
        table
            .insert(expiry_key(expires_at, card_id).as_str(), conversation_id)
            .map_err(|e| format!("overlay insert card_expiry: {e}"))?;
    }
    txn.commit().map_err(|e| format!("overlay redb commit: {e}"))
}

/// One due expiry entry: (index key, card id, conversation).
pub type DueExpiry = (String, String, String);

/// Every indexed card whose time is `<= now`, oldest first.
pub fn due_card_expiries(now: i64) -> Result<Vec<DueExpiry>, String> {
    let db = db()?;
    let txn = db.begin_read().map_err(|e| format!("overlay redb read: {e}"))?;
    let table = match txn.open_table(CARD_EXPIRY) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
        Err(e) => return Err(format!("overlay open card_expiry: {e}")),
    };
    // Keys sort by time; every key at `now` sorts before `{now + 1}/`.
    let end = format!("{:020}/", now.max(0) + 1);
    let mut out = Vec::new();
    for entry in table.range(..end.as_str()).map_err(|e| format!("overlay card_expiry range: {e}"))? {
        let (k, v) = entry.map_err(|e| format!("overlay card_expiry iter: {e}"))?;
        let key = k.value().to_string();
        let card = key.split_once('/').map(|(_, id)| id.to_string()).unwrap_or_default();
        out.push((key, card, v.value().to_string()));
    }
    Ok(out)
}

/// Drop expiry entries the sweep handled.
pub fn remove_card_expiries(keys: &[String]) -> Result<(), String> {
    if keys.is_empty() {
        return Ok(());
    }
    let db = db()?;
    let txn = db.begin_write().map_err(|e| format!("overlay redb write: {e}"))?;
    {
        let mut table = txn.open_table(CARD_EXPIRY).map_err(|e| format!("overlay open card_expiry: {e}"))?;
        for k in keys {
            table.remove(k.as_str()).map_err(|e| format!("overlay card_expiry remove: {e}"))?;
        }
    }
    txn.commit().map_err(|e| format!("overlay redb commit: {e}"))
}

/// Expiry entries follow their cards to the new conversation key (in the
/// move's own transaction). `take` limits it to the cards a split moved.
fn retarget_card_expiry(
    txn: &redb::WriteTransaction,
    docs: &redb::Table<&str, &[u8]>,
    from: &str,
    to: &str,
    take: Option<&dyn Fn(&OverlayDoc) -> bool>,
) -> Result<(), String> {
    let mut table = txn.open_table(CARD_EXPIRY).map_err(|e| format!("overlay open card_expiry: {e}"))?;
    let mut hits = Vec::new();
    for entry in table.iter().map_err(|e| format!("overlay card_expiry iter: {e}"))? {
        let (k, v) = entry.map_err(|e| format!("overlay card_expiry iter: {e}"))?;
        if v.value() == from {
            hits.push(k.value().to_string());
        }
    }
    for key in hits {
        if let Some(take) = take {
            let id = key.split_once('/').map(|(_, id)| id).unwrap_or_default();
            let moved = match docs.get(id).map_err(|e| format!("overlay get doc: {e}"))? {
                Some(g) => serde_json::from_slice::<OverlayDoc>(g.value()).map(|d| take(&d)).unwrap_or(false),
                None => false,
            };
            if !moved {
                continue;
            }
        }
        table.insert(key.as_str(), to).map_err(|e| format!("overlay card_expiry retarget: {e}"))?;
    }
    Ok(())
}

/// Newest-N page of a conversation collection (HTTP `/cli/thread` + `/cli/chatter`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPage {
    /// Ascending seq.
    pub items: Vec<OverlayItem>,
    /// Older items exist than this page's minimum seq (range truncated).
    pub has_more: bool,
}

pub fn read_thread(conversation_id: &str, since_seq: i64) -> Result<Vec<OverlayItem>, String> {
    read_conv(THREAD, "thread", conversation_id, since_seq)
}

pub fn read_chatter(conversation_id: &str, since_seq: i64) -> Result<Vec<OverlayItem>, String> {
    read_conv(CHATTER, "chatter", conversation_id, since_seq)
}

pub fn read_thread_page(
    conversation_id: &str,
    since_seq: Option<i64>,
    before_seq: Option<i64>,
    limit: usize,
) -> Result<OverlayPage, String> {
    read_conv_page(
        THREAD,
        "thread",
        conversation_id,
        since_seq,
        before_seq,
        limit,
    )
}

pub fn read_chatter_page(
    conversation_id: &str,
    since_seq: Option<i64>,
    before_seq: Option<i64>,
    limit: usize,
) -> Result<OverlayPage, String> {
    read_conv_page(
        CHATTER,
        "chatter",
        conversation_id,
        since_seq,
        before_seq,
        limit,
    )
}

pub fn read_chatterlog(since_seq: i64) -> Result<Vec<OverlayItem>, String> {
    let db = db()?;
    let txn = db
        .begin_read()
        .map_err(|e| format!("overlay redb read: {e}"))?;
    let pointers = {
        let table = match txn.open_table(CHATTERLOG) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(format!("overlay open chatterlog: {e}")),
        };
        let start = seq_pad(since_seq.saturating_add(1));
        let mut pointers = Vec::new();
        let range = table
            .range(start.as_str()..":")
            .map_err(|e| format!("overlay chatterlog range: {e}"))?;
        for entry in range {
            let (k, v) = entry.map_err(|e| format!("overlay chatterlog iter: {e}"))?;
            let seq: i64 = k
                .value()
                .parse()
                .map_err(|e| format!("overlay chatterlog seq '{}': {e}", k.value()))?;
            pointers.push((seq, v.value().to_string()));
        }
        pointers
    };
    let mut items = Vec::new();
    for (seq, id) in pointers {
        let Some(doc) = point_get_doc(&txn, &id)? else {
            return Err(format!(
                "overlay chatterlog seq {seq} points at missing docs/{id}"
            ));
        };
        items.push(OverlayItem {
            collection: "chatterlog".to_string(),
            seq,
            id,
            doc,
            conversation_id: None,
        });
    }
    Ok(items)
}

fn read_conv(
    def: TableDefinition<&str, &str>,
    collection: &str,
    conversation_id: &str,
    since_seq: i64,
) -> Result<Vec<OverlayItem>, String> {
    let db = db()?;
    let txn = db
        .begin_read()
        .map_err(|e| format!("overlay redb read: {e}"))?;
    let pointers = {
        let table = match txn.open_table(def) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(format!("overlay open {collection}: {e}")),
        };
        let start = conv_key(conversation_id, since_seq.saturating_add(1));
        let end = conv_end(conversation_id);
        let mut pointers = Vec::new();
        let range = table
            .range(start.as_str()..end.as_str())
            .map_err(|e| format!("overlay {collection} range: {e}"))?;
        for entry in range {
            let (k, v) = entry.map_err(|e| format!("overlay {collection} iter: {e}"))?;
            pointers.push(parse_conv_pointer(collection, k.value(), v.value())?);
        }
        pointers
    };
    let mut items = Vec::new();
    for (seq, id) in pointers {
        let Some(doc) = point_get_doc(&txn, &id)? else {
            return Err(format!(
                "overlay {collection} seq {seq} points at missing docs/{id}"
            ));
        };
        items.push(OverlayItem {
            collection: collection.to_string(),
            seq,
            id,
            doc,
            conversation_id: Some(conversation_id.to_string()),
        });
    }
    Ok(items)
}

fn parse_conv_pointer(collection: &str, key: &str, id: &str) -> Result<(i64, String), String> {
    let seq = key
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or_else(|| format!("overlay {collection} bad key '{key}'"))?;
    Ok((seq, id.to_string()))
}

fn read_conv_page(
    def: TableDefinition<&str, &str>,
    collection: &str,
    conversation_id: &str,
    since_seq: Option<i64>,
    before_seq: Option<i64>,
    limit: usize,
) -> Result<OverlayPage, String> {
    let db = db()?;
    let txn = db
        .begin_read()
        .map_err(|e| format!("overlay redb read: {e}"))?;
    let (pointers, has_more) = {
        let table = match txn.open_table(def) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => {
                return Ok(OverlayPage {
                    items: Vec::new(),
                    has_more: false,
                });
            }
            Err(e) => return Err(format!("overlay open {collection}: {e}")),
        };
        // seq > since_seq (omit = no lower bound); seq < before_seq (omit = no upper).
        let start = match since_seq {
            Some(s) => conv_key(conversation_id, s.saturating_add(1)),
            None => format!("{conversation_id}/"),
        };
        let end = match before_seq {
            Some(b) => conv_key(conversation_id, b),
            None => conv_end(conversation_id),
        };
        let range = table
            .range(start.as_str()..end.as_str())
            .map_err(|e| format!("overlay {collection} range: {e}"))?;
        let mut pointers = Vec::new();
        let mut has_more = false;
        if limit == 0 {
            for entry in range {
                let (k, v) = entry.map_err(|e| format!("overlay {collection} iter: {e}"))?;
                pointers.push(parse_conv_pointer(collection, k.value(), v.value())?);
            }
        } else {
            // Newest-N: reverse the prefix range so a 10k mailbox does not
            // load 10k pointers to take 50.
            for entry in range.rev() {
                if pointers.len() >= limit {
                    has_more = true;
                    break;
                }
                let (k, v) = entry.map_err(|e| format!("overlay {collection} iter: {e}"))?;
                pointers.push(parse_conv_pointer(collection, k.value(), v.value())?);
            }
            pointers.reverse();
        }
        (pointers, has_more)
    };
    let mut items = Vec::with_capacity(pointers.len());
    for (seq, id) in pointers {
        let Some(doc) = point_get_doc(&txn, &id)? else {
            return Err(format!(
                "overlay {collection} seq {seq} points at missing docs/{id}"
            ));
        };
        items.push(OverlayItem {
            collection: collection.to_string(),
            seq,
            id,
            doc,
            conversation_id: Some(conversation_id.to_string()),
        });
    }
    Ok(OverlayPage { items, has_more })
}

/// A forward page of a Thread plus what changed below it (thread-delta-v1,
/// `GET /cli/thread?after_seq=N`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDelta {
    /// Items with `seq > after_seq`, OLDEST first, at most `limit`
    /// (`limit == 0`: all of them).
    pub items: Vec<OverlayItem>,
    /// More items newer than the last one in `items` exist.
    pub has_more: bool,
    /// Items with `seq <= after_seq` whose doc changed in place since the
    /// reader's copy (a card answered, voided, set, expired; a handoff
    /// closed; a collected line updated), in their current state,
    /// ascending seq. May repeat a change the reader already has (a change
    /// written while `after_seq` was the newest seq); applying it again is
    /// a no-op.
    pub changed: Vec<OverlayItem>,
    /// `(seq, id)` of items with `seq <= after_seq` that left this Thread
    /// since (a fork-tab split moved them to their own conversation).
    pub removed: Vec<(i64, String)>,
}

/// [`ThreadDelta`] of `conversation_id` after `after_seq`, read in ONE
/// redb read transaction (the page and the changes agree).
pub fn read_thread_delta(conversation_id: &str, after_seq: i64, limit: usize) -> Result<ThreadDelta, String> {
    let empty = ThreadDelta { items: Vec::new(), has_more: false, changed: Vec::new(), removed: Vec::new() };
    let after_seq = after_seq.max(0);
    let db = db()?;
    let txn = db.begin_read().map_err(|e| format!("overlay redb read: {e}"))?;
    let thread = match txn.open_table(THREAD) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(empty),
        Err(e) => return Err(format!("overlay open thread: {e}")),
    };
    let end = conv_end(conversation_id);
    let mut pointers = Vec::new();
    let mut has_more = false;
    {
        let start = conv_key(conversation_id, after_seq.saturating_add(1));
        let range = thread
            .range(start.as_str()..end.as_str())
            .map_err(|e| format!("overlay thread range: {e}"))?;
        for entry in range {
            if limit != 0 && pointers.len() >= limit {
                has_more = true;
                break;
            }
            let (k, v) = entry.map_err(|e| format!("overlay thread iter: {e}"))?;
            pointers.push(parse_conv_pointer("thread", k.value(), v.value())?);
        }
    }
    // Changes marked at or after `after_seq`, of items the reader holds
    // (seq <= after_seq; newer ones are in `items` or a later page in
    // their current state). Last row per doc wins; a removal is final.
    let mut touched: std::collections::BTreeMap<String, (i64, bool)> = std::collections::BTreeMap::new();
    match txn.open_table(THREAD_CHANGES) {
        Ok(changes) => {
            let start = format!("{conversation_id}/{}", seq_pad(after_seq));
            for entry in changes
                .range(start.as_str()..end.as_str())
                .map_err(|e| format!("overlay thread_changes range: {e}"))?
            {
                let (k, v) = entry.map_err(|e| format!("overlay thread_changes iter: {e}"))?;
                let Some((_mark, id)) = parse_change_key(conversation_id, k.value()) else {
                    continue;
                };
                let raw = v.value();
                let (removed, seq_raw) = match raw.strip_prefix(CHANGE_REMOVED_PREFIX) {
                    Some(rest) => (true, rest),
                    None => (false, raw),
                };
                let seq: i64 = seq_raw
                    .parse()
                    .map_err(|e| format!("overlay thread_changes value '{raw}': {e}"))?;
                if seq > after_seq {
                    continue;
                }
                let prior_removed = touched.get(id).is_some_and(|(_, r)| *r);
                touched.insert(id.to_string(), (seq, removed || prior_removed));
            }
        }
        Err(redb::TableError::TableDoesNotExist(_)) => {}
        Err(e) => return Err(format!("overlay open thread_changes: {e}")),
    }
    let mut changed_ptrs = Vec::new();
    let mut removed = Vec::new();
    for (id, (seq, gone)) in touched {
        if gone {
            removed.push((seq, id));
            continue;
        }
        // The pointer must still name this doc (a later move renumbers and
        // drops the rows; never hand back a stale seq).
        let here = thread
            .get(conv_key(conversation_id, seq).as_str())
            .map_err(|e| format!("overlay get thread: {e}"))?
            .is_some_and(|g| g.value() == id);
        if here {
            changed_ptrs.push((seq, id));
        }
    }
    changed_ptrs.sort();
    removed.sort();
    let load = |ptrs: Vec<(i64, String)>| -> Result<Vec<OverlayItem>, String> {
        let mut out = Vec::with_capacity(ptrs.len());
        for (seq, id) in ptrs {
            let Some(doc) = point_get_doc(&txn, &id)? else {
                return Err(format!("overlay thread seq {seq} points at missing docs/{id}"));
            };
            out.push(OverlayItem {
                collection: "thread".to_string(),
                seq,
                id,
                doc,
                conversation_id: Some(conversation_id.to_string()),
            });
        }
        Ok(out)
    };
    Ok(ThreadDelta { items: load(pointers)?, has_more, changed: load(changed_ptrs)?, removed })
}

fn point_get_doc(txn: &redb::ReadTransaction, id: &str) -> Result<Option<OverlayDoc>, String> {
    let table = txn
        .open_table(DOCS)
        .map_err(|e| format!("overlay open docs: {e}"))?;
    match table.get(id).map_err(|e| format!("overlay get doc: {e}"))? {
        Some(guard) => {
            let doc: OverlayDoc = serde_json::from_slice(guard.value())
                .map_err(|e| format!("overlay doc parse {id}: {e}"))?;
            Ok(Some(doc))
        }
        None => Ok(None),
    }
}

/// Count `docs/` entries that equal `id`. Tests only — not a query path.
pub fn debug_doc_exists(id: &str) -> Result<bool, String> {
    Ok(get_doc(id)?.is_some())
}

/// True if any `docs/` body contains `needle`. Tests only — heap scan,
/// not a product query path. Used to prove secret bytes never land in redb.
pub fn debug_docs_contain(needle: &str) -> Result<bool, String> {
    if needle.is_empty() {
        return Ok(false);
    }
    let db = db()?;
    let txn = db
        .begin_read()
        .map_err(|e| format!("overlay redb read: {e}"))?;
    let table = match txn.open_table(DOCS) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(false),
        Err(e) => return Err(format!("overlay open docs: {e}")),
    };
    let iter = table
        .iter()
        .map_err(|e| format!("overlay docs iter: {e}"))?;
    for entry in iter {
        let (_, v) = entry.map_err(|e| format!("overlay docs row: {e}"))?;
        let blob = String::from_utf8_lossy(v.value());
        if blob.contains(needle) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// How many collection keys point at `id`. Tests only.
pub fn debug_pointer_count(id: &str) -> Result<(u32, u32, u32), String> {
    let db = db()?;
    let txn = db
        .begin_read()
        .map_err(|e| format!("overlay redb read: {e}"))?;
    let thread_n = count_value(&txn, THREAD, id)?;
    let chatter_n = count_value(&txn, CHATTER, id)?;
    let log_n = count_value(&txn, CHATTERLOG, id)?;
    Ok((thread_n, chatter_n, log_n))
}

fn count_value(
    txn: &redb::ReadTransaction,
    def: TableDefinition<&str, &str>,
    id: &str,
) -> Result<u32, String> {
    let table = match txn.open_table(def) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
        Err(e) => return Err(format!("overlay open for count: {e}")),
    };
    let mut n = 0u32;
    let iter = table
        .iter()
        .map_err(|e| format!("overlay count iter: {e}"))?;
    for entry in iter {
        let (_, v) = entry.map_err(|e| format!("overlay count row: {e}"))?;
        if v.value() == id {
            n = n.saturating_add(1);
        }
    }
    Ok(n)
}

/// One collection's pointers after a [`move_conversation`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MovedCollection {
    /// Pointers that were under the old key.
    pub moved: usize,
    /// Pointers now under the new key (old + new, deduplicated by doc id).
    pub total: usize,
    /// Every row now under the new key, with where it was before: read
    /// cursors are remapped through this (seqs are renumbered).
    pub remap: Vec<SeqRemap>,
    /// `(old seq, doc id)` of the rows taken from the old key.
    pub taken: Vec<(i64, String)>,
}

/// One renumbered row of a move or split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeqRemap {
    /// True when the row came from the old key (`from`), false when it was
    /// already under the new key (`to`).
    pub from_old: bool,
    /// Its seq under that key.
    pub old_seq: i64,
    /// Its seq under the new key now.
    pub new_seq: i64,
}

/// Move a conversation's `thread` and `chatter` pointers from `from` to
/// `to` in ONE redb write transaction (Codex/Hermes adoption: pane key →
/// provider id). Rows already under `to` (a reply that raced in under the
/// provider id) are merged with the moved ones by doc id, ordered by the
/// doc's `created_at` (old key first on a tie, then by seq), and
/// renumbered `1..=n` under `to`. Doc bodies are never copied.
///
/// `before_commit(thread, chatter)` runs after the pointers are rewritten
/// and before the commit: the caller updates the SQLite catalog there. If
/// it fails, the redb transaction is dropped (aborted) and nothing moves.
pub fn move_conversation(
    from: &str,
    to: &str,
    before_commit: impl FnOnce(&MovedCollection, &MovedCollection) -> Result<(), String>,
) -> Result<(MovedCollection, MovedCollection), String> {
    let db = db()?;
    let txn = db
        .begin_write()
        .map_err(|e| format!("overlay redb write: {e}"))?;
    let (thread, chatter) = {
        let docs = txn
            .open_table(DOCS)
            .map_err(|e| format!("overlay open docs: {e}"))?;
        let thread = move_collection(&txn, &docs, THREAD, "thread", from, to, None)?;
        let chatter = move_collection(&txn, &docs, CHATTER, "chatter", from, to, None)?;
        retarget_card_expiry(&txn, &docs, from, to, None)?;
        retarget_pending_cards(&txn, &docs, from, to, None)?;
        (thread, chatter)
    };
    // thread-delta-v1: both keys' seqs are gone or renumbered; a reader
    // sees the new conversation_id and starts over.
    drop_changes(&txn, from)?;
    drop_changes(&txn, to)?;
    before_commit(&thread, &chatter)?;
    txn.commit()
        .map_err(|e| format!("overlay redb commit: {e}"))?;
    Ok((thread, chatter))
}

/// Split a shared conversation: move only the `thread` / `chatter`
/// pointers of `from` whose doc `take` accepts to `to`, merged and
/// renumbered there like [`move_conversation`]. Pointers left under
/// `from` keep their seqs (gaps are fine; a client's high-water mark
/// stays valid), so the other owner's Thread is untouched (TR20: a fork
/// tab that shared another workspace's conversation id).
pub fn split_conversation(
    from: &str,
    to: &str,
    take: &dyn Fn(&OverlayDoc) -> bool,
    before_commit: impl FnOnce(&MovedCollection, &MovedCollection) -> Result<(), String>,
) -> Result<(MovedCollection, MovedCollection), String> {
    let db = db()?;
    let txn = db
        .begin_write()
        .map_err(|e| format!("overlay redb write: {e}"))?;
    // thread-delta-v1: `from`'s highest seq BEFORE the split marks the
    // removals, so every reader that saw a taken row gets it.
    let mark = {
        let t = txn.open_table(THREAD).map_err(|e| format!("overlay open thread: {e}"))?;
        last_thread_seq(&t, from)?
    };
    let (thread, chatter) = {
        let docs = txn
            .open_table(DOCS)
            .map_err(|e| format!("overlay open docs: {e}"))?;
        let thread = move_collection(&txn, &docs, THREAD, "thread", from, to, Some(take))?;
        let chatter = move_collection(&txn, &docs, CHATTER, "chatter", from, to, Some(take))?;
        retarget_card_expiry(&txn, &docs, from, to, Some(take))?;
        retarget_pending_cards(&txn, &docs, from, to, Some(take))?;
        (thread, chatter)
    };
    if !thread.taken.is_empty() {
        // `to` was merged and renumbered (no reader is on it yet); each row
        // taken from `from` is a removal there.
        drop_changes(&txn, to)?;
        let mut changes = txn
            .open_table(THREAD_CHANGES)
            .map_err(|e| format!("overlay open thread_changes: {e}"))?;
        for (seq, id) in &thread.taken {
            changes
                .insert(change_key(from, mark, id).as_str(), format!("{CHANGE_REMOVED_PREFIX}{seq}").as_str())
                .map_err(|e| format!("overlay insert thread_changes: {e}"))?;
        }
    }
    before_commit(&thread, &chatter)?;
    txn.commit()
        .map_err(|e| format!("overlay redb commit: {e}"))?;
    Ok((thread, chatter))
}

fn conv_pointers(
    table: &redb::Table<&str, &str>,
    collection: &str,
    conversation_id: &str,
) -> Result<Vec<(i64, String)>, String> {
    let start = format!("{conversation_id}/");
    let end = conv_end(conversation_id);
    let mut out = Vec::new();
    let range = table
        .range(start.as_str()..end.as_str())
        .map_err(|e| format!("overlay {collection} range: {e}"))?;
    for entry in range {
        let (k, v) = entry.map_err(|e| format!("overlay {collection} iter: {e}"))?;
        out.push(parse_conv_pointer(collection, k.value(), v.value())?);
    }
    Ok(out)
}

fn move_collection(
    txn: &redb::WriteTransaction,
    docs: &redb::Table<&str, &[u8]>,
    def: TableDefinition<&str, &str>,
    collection: &str,
    from: &str,
    to: &str,
    take: Option<&dyn Fn(&OverlayDoc) -> bool>,
) -> Result<MovedCollection, String> {
    let load = |id: &str| -> Result<OverlayDoc, String> {
        let guard = docs
            .get(id)
            .map_err(|e| format!("overlay get doc: {e}"))?
            .ok_or_else(|| format!("overlay {collection} points at missing docs/{id}"))?;
        serde_json::from_slice(guard.value()).map_err(|e| format!("overlay doc parse {id}: {e}"))
    };
    let created_at = |id: &str| -> Result<i64, String> { Ok(load(id)?.created_at) };
    let mut table = txn
        .open_table(def)
        .map_err(|e| format!("overlay open {collection}: {e}"))?;
    let mut old = conv_pointers(&table, collection, from)?;
    if let Some(take) = take {
        let mut kept = Vec::with_capacity(old.len());
        for (seq, id) in old {
            if take(&load(&id)?) {
                kept.push((seq, id));
            }
        }
        old = kept;
    }
    let new = conv_pointers(&table, collection, to)?;
    if old.is_empty() {
        return Ok(MovedCollection { moved: 0, total: new.len(), remap: Vec::new(), taken: Vec::new() });
    }
    // (created_at, source: 0 = the old key first on a tie, seq, id)
    let mut rows: Vec<(i64, u8, i64, String)> = Vec::with_capacity(old.len() + new.len());
    for (seq, id) in &old {
        rows.push((created_at(id)?, 0, *seq, id.clone()));
    }
    for (seq, id) in &new {
        rows.push((created_at(id)?, 1, *seq, id.clone()));
    }
    rows.sort();
    let mut seen = std::collections::HashSet::new();
    rows.retain(|r| seen.insert(r.3.clone()));
    for (seq, _) in &old {
        table
            .remove(conv_key(from, *seq).as_str())
            .map_err(|e| format!("overlay {collection} remove: {e}"))?;
    }
    for (seq, _) in &new {
        table
            .remove(conv_key(to, *seq).as_str())
            .map_err(|e| format!("overlay {collection} remove: {e}"))?;
    }
    let mut remap = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let key = conv_key(to, i as i64 + 1);
        table
            .insert(key.as_str(), row.3.as_str())
            .map_err(|e| format!("overlay insert {collection}: {e}"))?;
        remap.push(SeqRemap { from_old: row.1 == 0, old_seq: row.2, new_seq: i as i64 + 1 });
    }
    Ok(MovedCollection {
        moved: old.len(),
        total: rows.len(),
        remap,
        taken: old,
    })
}

// ── collected conversations ─────────────────────────────────────────────

fn ext_key(conversation_id: &str, ext_id: &str) -> String {
    format!("{conversation_id}/{ext_id}")
}

/// One write transaction on a collected conversation (see [`ext_write`]).
/// Seqs are allocated from the conversation's own `thread` range (these
/// conversations have no SQLite catalog row: they belong to no workspace).
pub struct ExtTxn<'a> {
    txn: &'a redb::WriteTransaction,
    conv: String,
    wrote: bool,
}

impl ExtTxn<'_> {
    pub fn conversation_id(&self) -> &str {
        &self.conv
    }

    /// The collector's cursor JSON.
    pub fn state(&self) -> Result<Option<Vec<u8>>, String> {
        let t = self
            .txn
            .open_table(EXT_STATE)
            .map_err(|e| format!("overlay open ext_state: {e}"))?;
        let out = t
            .get(self.conv.as_str())
            .map_err(|e| format!("overlay get ext_state: {e}"))?
            .map(|g| g.value().to_vec());
        Ok(out)
    }

    pub fn set_state(&mut self, json: &[u8]) -> Result<(), String> {
        let mut t = self
            .txn
            .open_table(EXT_STATE)
            .map_err(|e| format!("overlay open ext_state: {e}"))?;
        t.insert(self.conv.as_str(), json)
            .map_err(|e| format!("overlay insert ext_state: {e}"))?;
        self.wrote = true;
        Ok(())
    }

    /// The doc a product id names in this conversation.
    pub fn lookup(&self, ext_id: &str) -> Result<Option<String>, String> {
        let t = self
            .txn
            .open_table(EXT_IDS)
            .map_err(|e| format!("overlay open ext_ids: {e}"))?;
        let out = t
            .get(ext_key(&self.conv, ext_id).as_str())
            .map_err(|e| format!("overlay get ext_ids: {e}"))?
            .map(|g| g.value().to_string());
        Ok(out)
    }

    pub fn doc(&self, doc_id: &str) -> Result<Option<OverlayDoc>, String> {
        let t = self
            .txn
            .open_table(DOCS)
            .map_err(|e| format!("overlay open docs: {e}"))?;
        let out = match t.get(doc_id).map_err(|e| format!("overlay get doc: {e}"))? {
            Some(g) => Some(
                serde_json::from_slice::<OverlayDoc>(g.value())
                    .map_err(|e| format!("overlay doc parse {doc_id}: {e}"))?,
            ),
            None => None,
        };
        Ok(out)
    }

    /// Rewrite a doc in place (its seq and ids stay), with its
    /// `thread_changes` row (thread-delta-v1).
    pub fn put_doc(&mut self, doc: &OverlayDoc) -> Result<(), String> {
        self.write_doc(doc)?;
        note_change(self.txn, &self.conv, &doc.id, None)
    }

    fn write_doc(&mut self, doc: &OverlayDoc) -> Result<(), String> {
        let json = serde_json::to_vec(doc).map_err(|e| format!("overlay doc json: {e}"))?;
        let mut t = self
            .txn
            .open_table(DOCS)
            .map_err(|e| format!("overlay open docs: {e}"))?;
        t.insert(doc.id.as_str(), json.as_slice())
            .map_err(|e| format!("overlay insert docs: {e}"))?;
        self.wrote = true;
        Ok(())
    }

    /// Point another product id at an existing doc.
    pub fn alias(&mut self, ext_id: &str, doc_id: &str) -> Result<(), String> {
        let mut t = self
            .txn
            .open_table(EXT_IDS)
            .map_err(|e| format!("overlay open ext_ids: {e}"))?;
        t.insert(ext_key(&self.conv, ext_id).as_str(), doc_id)
            .map_err(|e| format!("overlay insert ext_ids: {e}"))?;
        self.wrote = true;
        Ok(())
    }

    /// Append `doc` as the conversation's next Thread line and index it
    /// under `ext_ids`. Returns its seq.
    pub fn append(&mut self, doc: &OverlayDoc, ext_ids: &[&str]) -> Result<i64, String> {
        let seq = {
            let t = self
                .txn
                .open_table(THREAD)
                .map_err(|e| format!("overlay open thread: {e}"))?;
            let start = format!("{}/", self.conv);
            let end = conv_end(&self.conv);
            let mut range = t
                .range(start.as_str()..end.as_str())
                .map_err(|e| format!("overlay thread range: {e}"))?;
            let last = match range.next_back() {
                Some(entry) => {
                    let (k, v) = entry.map_err(|e| format!("overlay thread iter: {e}"))?;
                    parse_conv_pointer("thread", k.value(), v.value())?.0
                }
                None => 0,
            };
            last + 1
        };
        self.write_doc(doc)?;
        {
            let mut t = self
                .txn
                .open_table(THREAD)
                .map_err(|e| format!("overlay open thread: {e}"))?;
            t.insert(conv_key(&self.conv, seq).as_str(), doc.id.as_str())
                .map_err(|e| format!("overlay insert thread: {e}"))?;
        }
        for id in ext_ids {
            self.alias(id, &doc.id)?;
        }
        Ok(seq)
    }
}

/// Run `f` in one redb write transaction on collected conversation
/// `conversation_id`. Commits when `f` returns Ok and wrote something; an
/// Err aborts everything `f` did. Callers go through
/// [`super::collected_write`], which holds the overlay write lock.
pub fn ext_write<R>(
    conversation_id: &str,
    f: impl FnOnce(&mut ExtTxn<'_>) -> Result<R, String>,
) -> Result<R, String> {
    let db = db()?;
    let txn = db
        .begin_write()
        .map_err(|e| format!("overlay redb write: {e}"))?;
    let (out, wrote) = {
        let mut x = ExtTxn {
            txn: &txn,
            conv: conversation_id.to_string(),
            wrote: false,
        };
        let out = f(&mut x)?;
        (out, x.wrote)
    };
    if wrote {
        txn.commit()
            .map_err(|e| format!("overlay redb commit: {e}"))?;
    } else {
        txn.abort()
            .map_err(|e| format!("overlay redb abort: {e}"))?;
    }
    Ok(out)
}

/// The collector's cursor JSON for a collected conversation (read only).
pub fn ext_state(conversation_id: &str) -> Result<Option<Vec<u8>>, String> {
    let db = db()?;
    let txn = db
        .begin_read()
        .map_err(|e| format!("overlay redb read: {e}"))?;
    let t = match txn.open_table(EXT_STATE) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(e) => return Err(format!("overlay open ext_state: {e}")),
    };
    let out = t
        .get(conversation_id)
        .map_err(|e| format!("overlay get ext_state: {e}"))?
        .map(|g| g.value().to_vec());
    Ok(out)
}

// ── imported conversations (a clone carries its Threads) ────────────────

/// What one [`import_docs`] wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportWrite {
    /// New lines written (the divider included).
    pub written: usize,
    /// Lines skipped because their import key was already present.
    pub skipped: usize,
    /// The conversation's last Thread seq after the write.
    pub last_seq: i64,
}

/// The local doc an import key names, if that line was imported already.
pub fn import_id(key: &str) -> Result<Option<String>, String> {
    let db = db()?;
    let txn = db.begin_read().map_err(|e| format!("overlay redb read: {e}"))?;
    let t = match txn.open_table(IMPORT_IDS) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(e) => return Err(format!("overlay open import_ids: {e}")),
    };
    let out = t
        .get(key)
        .map_err(|e| format!("overlay get import_ids: {e}"))?
        .map(|g| g.value().to_string());
    Ok(out)
}

/// Append imported lines to `conversation_id` in ONE redb write
/// transaction: bodies, Thread pointers (seqs after both `start_after` and
/// the last pointer already there) and `import_ids` entries commit
/// together or not at all. An entry whose key is already in `import_ids`
/// is skipped. `divider` (key, doc) is written first, and only when at
/// least one entry is new. No card-expiry rows (imported cards are
/// closed). `before_commit` runs before the commit (the caller's SQLite
/// catalog half); an Err aborts everything. Callers hold the overlay
/// write lock.
pub fn import_docs(
    conversation_id: &str,
    start_after: i64,
    entries: &[(String, OverlayDoc)],
    divider: Option<(String, OverlayDoc)>,
    before_commit: impl FnOnce(ImportWrite) -> Result<(), String>,
) -> Result<ImportWrite, String> {
    let db = db()?;
    let txn = db.begin_write().map_err(|e| format!("overlay redb write: {e}"))?;
    let out = {
        let mut ids = txn.open_table(IMPORT_IDS).map_err(|e| format!("overlay open import_ids: {e}"))?;
        let mut docs = txn.open_table(DOCS).map_err(|e| format!("overlay open docs: {e}"))?;
        let mut thread = txn.open_table(THREAD).map_err(|e| format!("overlay open thread: {e}"))?;
        let mut fresh: Vec<&(String, OverlayDoc)> = Vec::with_capacity(entries.len());
        let mut seen = std::collections::HashSet::new();
        let mut skipped = 0usize;
        for e in entries {
            let known = ids
                .get(e.0.as_str())
                .map_err(|e| format!("overlay get import_ids: {e}"))?
                .is_some();
            if known || !seen.insert(e.0.as_str()) {
                skipped += 1;
            } else {
                fresh.push(e);
            }
        }
        let last = {
            let start = format!("{conversation_id}/");
            let end = conv_end(conversation_id);
            let mut range = thread
                .range(start.as_str()..end.as_str())
                .map_err(|e| format!("overlay thread range: {e}"))?;
            match range.next_back() {
                Some(entry) => {
                    let (k, v) = entry.map_err(|e| format!("overlay thread iter: {e}"))?;
                    parse_conv_pointer("thread", k.value(), v.value())?.0
                }
                None => 0,
            }
        };
        let mut seq = last.max(start_after);
        let mut written = 0usize;
        let divider = match divider {
            Some((key, doc)) if !fresh.is_empty() => {
                let known = ids
                    .get(key.as_str())
                    .map_err(|e| format!("overlay get import_ids: {e}"))?
                    .is_some();
                (!known).then_some((key, doc))
            }
            _ => None,
        };
        let lines = divider.iter().chain(fresh.into_iter());
        for (key, doc) in lines {
            let json = serde_json::to_vec(doc).map_err(|e| format!("overlay doc json: {e}"))?;
            seq += 1;
            docs.insert(doc.id.as_str(), json.as_slice())
                .map_err(|e| format!("overlay insert docs: {e}"))?;
            thread
                .insert(conv_key(conversation_id, seq).as_str(), doc.id.as_str())
                .map_err(|e| format!("overlay insert thread: {e}"))?;
            ids.insert(key.as_str(), doc.id.as_str())
                .map_err(|e| format!("overlay insert import_ids: {e}"))?;
            written += 1;
        }
        ImportWrite { written, skipped, last_seq: seq }
    };
    before_commit(out)?;
    txn.commit().map_err(|e| format!("overlay redb commit: {e}"))?;
    Ok(out)
}
