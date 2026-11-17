//! The native engine (a Beta) in sessions: where a native doc's keys come from, and how a doc
//! is loaded on the engine its stamp names. A native doc has one replica, the host's: people,
//! agents and readers join it through the session's verbs, each participant its own writer, as
//! on Yrs; nothing here takes or gives sync bytes. Its log isn't compacted: its compaction is the
//! engine's cut, which waits for the engine's single-replica mode. Its disk replica is text only
//! (`text_replica.rs`), and its live text is capped (`text_cap.rs`).

use std::collections::VecDeque;
use std::sync::Arc;

use super::{check_engine, State, StoredEngine, AUTHORS_KEPT};
use crate::doc::Doc;
use crate::schema::{Kind, Schema};
use crate::session::config::Config;
use crate::session::error::Result;
use crate::store::{DocId, DocMeta, DocStore, Seq};

/// Where a session would get its native docs' keys: a build without the native engine has none.
#[derive(Clone, Default)]
pub(crate) struct KeysSource {}

/// A stored doc as a load reads it, on the engine its stamp names.
pub(super) struct LoadedDoc {
    pub doc: Doc,
    pub version: Seq,
    /// The log since the snapshot: (entries, bytes).
    pub log: (u64, u64),
    pub state_bytes: u64,
    pub authors: VecDeque<(Seq, String)>,
}

/// Load doc `doc_id` (its metadata `meta`) on the engine its stamp names: a Yrs doc from its
/// snapshot and log; a native doc by the engine's own record load, under the keys the host gives
/// for it (`MPCE-E566` without them, nothing written). A stamp this build doesn't read is
/// refused before anything is read ([`check_engine`]).
pub(super) fn load_doc(
    store: &Arc<DocStore>,
    doc_id: DocId,
    meta: &DocMeta,
    schema: Schema,
    config: &Config,
    keys: &KeysSource,
) -> Result<LoadedDoc> {
    let engine = check_engine(meta)?;
    let loaded = {
        timed!(LoadStoreRead);
        store.load(doc_id)?
    };
    let mut doc = {
        let (StoredEngine::Yrs, _) = (engine, keys);
        Doc::from_store(schema, &loaded)?
    };
    doc.set_edit_check_every(config.edit_check_every);
    doc.set_edit_check_scale(config.edit_check_scale_units);
    doc.set_position_check_every(config.position_check_every);
    let snap_seq = loaded.snapshot.as_ref().map_or(0, |(s, _)| *s);
    let version = loaded.updates.last().map_or(snap_seq, |(s, _)| *s);
    let log_bytes: u64 = loaded.updates.iter().map(|(_, e)| e.body.yrs_len()).sum();
    let state_bytes = if doc.is_native() {
        // no kit snapshot or log bytes: the doc's live text stands in for its size
        text_bytes(&doc, &schema)?
    } else {
        loaded.snapshot.as_ref().map_or(0, |(_, s)| s.len() as u64) + log_bytes
    };
    let mut authors: VecDeque<(Seq, String)> = loaded
        .updates
        .iter()
        .map(|(s, e)| (*s, e.author.clone()))
        .collect();
    while authors.len() > AUTHORS_KEPT {
        authors.pop_front();
    }
    Ok(LoadedDoc {
        doc,
        version,
        log: (loaded.updates.len() as u64, log_bytes),
        state_bytes,
        authors,
    })
}

/// The stamp of a doc made on the native engine (`native`) or on Yrs: its engine's name and
/// stored format.
pub(super) fn stamp(native: bool) -> (&'static str, u32) {
    let _ = native;
    (crate::engine::ENGINE_NAME, crate::engine::ENGINE_FORMAT)
}

/// Every text root's bytes, together.
fn text_bytes(doc: &Doc, schema: &Schema) -> Result<u64> {
    let mut n = 0u64;
    for (r, k) in schema.roots {
        if *k == Kind::Text {
            n += doc.text(r)?.len() as u64;
        }
    }
    Ok(n)
}

/// A native doc's pending engine records, and the position of each change among them.
pub(super) type Pending = (Vec<Vec<u8>>, Vec<u64>);

impl super::Session {
    /// A native doc's pending engine records (what its next commit writes) and the position of
    /// each change among them, in order, checked against `changes` (the changes the commit
    /// logs: one position each). `None` on Yrs. A count that doesn't match is a bug: refused
    /// before anything is written.
    pub(super) fn native_pending(&self, st: &State, changes: usize) -> Result<Option<Pending>> {
        let _ = (st, changes);
        Ok(None)
    }
}
