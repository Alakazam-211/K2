//! A session's purge of a doc on the native engine (a Beta). The kit's tables are purged as for
//! a Yrs doc (`live/purge.rs`); what differs is the doc's own state. A Yrs doc keeps no deleted
//! content (it is re-encoded with garbage collection on); a native doc's records keep it, for
//! history, until a purge erases it. So a purge of a native doc, whatever its scope, has the
//! engine erase **every deleted element** of the doc's text, everywhere its records hold it
//! (the native engine purges deleted records), leaving its live text as it is, as on Yrs:
//!
//! - the engine's purge is one change: the session logs it as a change of no text by the
//!   purging owner (a log row naming its position, and a change row with an empty record);
//! - its record, the rewrites of every earlier record that held erased content, its marker(s),
//!   the log and change rows and the kit's purge of its tables are written in **one** store
//!   transaction (`DocStore::commit_native_purge`): a process that stops part-way leaves all of it
//!   or none;
//! - after the engine's purge, and before anything is written, the doc must hold no deleted
//!   bytes (its content bytes equal its live text's); a doc that still does is refused, and
//!   the staged purge is dropped by reloading the doc from its store, nothing written;
//! - another live instance of the doc refuses it (`MPC-E523` `purge_doc_open`), nothing
//!   written.

use super::*;
use crate::store::{DocPurge, PurgeCounts};

impl Session {
    /// Without the native engine no doc is native: never reached.
    pub(super) fn purge_native_records(
        &self,
        _: &mut State,
        _: &Arc<DocStore>,
        _: usize,
        _: &DocPurge<'_>,
        _: u64,
    ) -> Result<(PurgeCounts, Vec<crate::store::PurgedBlob>)> {
        Err(
            DocError::Malformed("a native purge on a build without the native engine".into())
                .into(),
        )
    }
}
