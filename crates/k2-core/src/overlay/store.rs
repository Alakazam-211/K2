//! redb document heap + three pointer tables.
//!
//! ```text
//! docs          id                  → JSON body
//! thread        conv/seq:020        → id
//! chatter       conv/seq:020        → id
//! chatterlog    seq:020             → id
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
            std::fs::create_dir_all(parent).map_err(|e| {
                format!("overlay redb mkdir {}: {e}", parent.display())
            })?;
        }
        Database::create(&path)
            .map_err(|e| format!("overlay redb create {}: {e}", path.display()))
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

pub fn read_thread(conversation_id: &str, since_seq: i64) -> Result<Vec<OverlayItem>, String> {
    read_conv(THREAD, "thread", conversation_id, since_seq)
}

pub fn read_chatter(conversation_id: &str, since_seq: i64) -> Result<Vec<OverlayItem>, String> {
    read_conv(CHATTER, "chatter", conversation_id, since_seq)
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
            let key = k.value().to_string();
            let seq = key
                .rsplit('/')
                .next()
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or_else(|| format!("overlay {collection} bad key '{key}'"))?;
            pointers.push((seq, v.value().to_string()));
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

fn point_get_doc(
    txn: &redb::ReadTransaction,
    id: &str,
) -> Result<Option<OverlayDoc>, String> {
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
