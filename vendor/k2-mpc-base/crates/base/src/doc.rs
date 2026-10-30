//! The loaded doc: what sessions and packs use.
//!
//! Engine-independent surface:
//! - wire and store bytes are opaque `Vec<u8>`;
//! - edits are data ([`Edit`]), applied as one participant in one change;
//! - positions never cross the API (text edits are [`Hunk`]s in UTF-8 byte offsets of the
//!   current text; wire positions are the engine's own relative positions).
//!
//! **Untrusted bytes.** Functions that decode bytes from a remote client (`encode_diff`,
//! `check_client_update`, `apply_client_update`) exist only with the `untrusted-wire`
//! feature. A host must not enable it until the strict decoder (decode, validate,
//! re-encode before the engine sees anything) lands. Without it, every byte the engine
//! decodes was produced by this kit: agent edits arrive as text, become validated hunks, and
//! are encoded by the kit itself.
//!
//! Invariants:
//! - The doc's own client ID never authors content. Every change is attributed to a
//!   participant's client ID (a connection, an agent, "external edit", "import"...).
//! - [`Doc::edit_as`] keeps nothing per participant after it returns: memory is the doc,
//!   not the doc times the number of agents (tested in `tests/ephemeral_replicas.rs`). Since
//!   v0.9.3 the edit is made on the doc itself, as the participant's client (the vendored
//!   engine's `transact_mut_as`): no replica at all. A sampled runtime check (one edit in
//!   [`DEFAULT_EDIT_CHECK_EVERY`] by default, [`Doc::set_edit_check_every`]) also makes the
//!   edit the old way, through a throwaway replica, and compares the two updates byte for
//!   byte; a mismatch is never served (the doc takes the replica's update and uses that path
//!   until reloaded).

use crate::engine::{BlameRun, EngineDoc, YrsDoc};
use crate::schema::{Kind, Schema};
use crate::text::diff::{self, Hunk, HunkError};

/// A participant's client ID. 53 bits, so it is safe as a JavaScript number.
pub type ClientId = u64;

/// Largest valid client ID (2^53 - 1).
pub const MAX_CLIENT_ID: ClientId = (1 << 53) - 1;

/// How often [`Doc::edit_as`]'s runtime check samples an edit by default: one in 1,000.
pub const DEFAULT_EDIT_CHECK_EVERY: u32 = 1000;

/// What a checked client update contains.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateInfo {
    /// Client IDs the update inserts content under (at most the allowed one).
    pub inserts_from: Vec<ClientId>,
    /// True when the update only deletes (or carries nothing).
    pub delete_only: bool,
}

/// Placeholder for [`DocError::Unaccepted`]'s error id: the slot is reserved, and the id is
/// assigned when the error-surface registry merges.
pub const RESERVED_ERROR_ID_UNACCEPTED: Option<&str> = None;
/// Placeholder for [`DocError::Quarantined`]'s error id (reserved, as above).
pub const RESERVED_ERROR_ID_QUARANTINED: Option<&str> = None;

/// An engine-level error. `#[non_exhaustive]` since v0.9.5 (which added two variants): match
/// it with a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DocError {
    #[error("update could not be decoded: {0}")]
    Decode(String),
    #[error("update inserts as client {found}, but this connection is client {allowed}")]
    ForeignClient { allowed: ClientId, found: ClientId },
    #[error("update depends on changes this doc doesn't have (client {client} from clock {from}, doc has {have}); resync")]
    MissingDeps {
        client: ClientId,
        from: u32,
        have: u32,
    },
    /// An update left unresolved dependencies (or an apply failed after decoding): the
    /// in-memory doc is poisoned and refuses to encode or edit until reloaded. Sessions
    /// reload from the store on their own.
    #[error("update left unresolved dependencies after apply; the doc needs a reload or resync")]
    Pending,
    #[error("update could not be applied: {0}")]
    Apply(String),
    #[error("the engine panicked on this input")]
    Panic,
    #[error("state could not be loaded: {0}")]
    Load(String),
    /// A client update was refused, unapplied (v0.9.5): it carries content the kit never
    /// uses (subdocuments). Exit 3 (`tools::exit::REFUSED`). Error id: reserved,
    /// [`RESERVED_ERROR_ID_UNACCEPTED`].
    #[error("update contains content this host doesn't accept")]
    Unaccepted,
    /// A stored doc holds content the kit never uses (subdocuments): it is quarantined, not
    /// loaded as live (the store-load scan, v0.9.5). Exit 3. Error id: reserved,
    /// [`RESERVED_ERROR_ID_QUARANTINED`].
    #[error("stored doc contains content this host doesn't accept; it was not loaded")]
    Quarantined,
    #[error("invalid client id {0}")]
    BadClient(ClientId),
    #[error("schema {schema:?} has no root {root:?}")]
    NoRoot { schema: &'static str, root: String },
    #[error("root {root:?} is {actual:?}, not {wanted:?}")]
    WrongKind {
        root: String,
        wanted: Kind,
        actual: Kind,
    },
    #[error("primitive {0:?} is not implemented yet")]
    Unsupported(Kind),
    #[error(transparent)]
    Hunk(#[from] HunkError),
}

/// A root's content as plain data, independent of the engine (versions, export, restore).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Neutral {
    Text(String),
}

/// One change to one root, as data. A participant's edit is a list of these, applied together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Replace ranges of a text root (hunks against its current text).
    Text { root: String, hunks: Vec<Hunk> },
}

/// A doc on a pack's schema.
pub struct Doc {
    schema: Schema,
    inner: YrsDoc,
}

pub(crate) fn check_client(id: ClientId) -> Result<(), DocError> {
    if id == 0 || id > MAX_CLIENT_ID || id == crate::engine::SESSION_CLIENT {
        Err(DocError::BadClient(id))
    } else {
        Ok(())
    }
}

fn check_schema(schema: &Schema) -> Result<(), DocError> {
    for (_, k) in schema.roots {
        if !k.implemented() {
            return Err(DocError::Unsupported(*k));
        }
    }
    Ok(())
}

impl Doc {
    /// A new, empty doc.
    pub fn new(schema: Schema) -> Result<Self, DocError> {
        check_schema(&schema)?;
        Ok(Self {
            inner: YrsDoc::new(&schema),
            schema,
        })
    }

    /// Load from [`Doc::encode_state`] output (trusted: from our own store).
    pub fn load(schema: Schema, state: &[u8]) -> Result<Self, DocError> {
        check_schema(&schema)?;
        Ok(Self {
            inner: YrsDoc::load(&schema, state)?,
            schema,
        })
    }

    /// Rebuild a doc from what the kit's store holds: the snapshot, then the log after it.
    /// Store bytes are trusted: the kit wrote them (see [`crate::store`]).
    pub fn from_store(schema: Schema, loaded: &crate::store::Loaded) -> Result<Self, DocError> {
        let mut d = match &loaded.snapshot {
            Some((_, state)) => Self::load(schema, state)?,
            None => Self::new(schema)?,
        };
        for (_, e) in &loaded.updates {
            d.inner.apply_trusted(&e.update)?;
        }
        Ok(d)
    }

    /// Every root's content as plain data, in schema order.
    pub fn export_neutral(&self) -> Vec<(String, Neutral)> {
        self.schema
            .roots
            .iter()
            .map(|(name, kind)| match kind {
                Kind::Text => ((*name).to_owned(), Neutral::Text(self.inner.text(name))),
                // check_schema refuses unimplemented kinds, so only Text reaches here
                other => unreachable!("unimplemented kind {other:?} in a live doc"),
            })
            .collect()
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    fn root(&self, root: &str, wanted: Kind) -> Result<(), DocError> {
        match self.schema.kind_of(root) {
            None => Err(DocError::NoRoot {
                schema: self.schema.name,
                root: root.to_owned(),
            }),
            Some(k) if k != wanted => Err(DocError::WrongKind {
                root: root.to_owned(),
                wanted,
                actual: k,
            }),
            Some(_) => Ok(()),
        }
    }

    /// The current text of a text root.
    pub fn text(&self, root: &str) -> Result<String, DocError> {
        self.root(root, Kind::Text)?;
        Ok(self.inner.text(root))
    }

    /// Encoded state vector (what this doc has), for sync step 1.
    pub fn state_vector(&self) -> Vec<u8> {
        self.inner.state_vector()
    }

    /// Full state, in the compact store format. Fails with [`DocError::Pending`] on a
    /// poisoned doc (see [`DocError::Pending`]) and with [`DocError::Panic`] if the engine
    /// panics; it never unwinds.
    pub fn encode_state(&self) -> Result<Vec<u8>, DocError> {
        self.inner.encode_state()
    }

    /// What a peer with state vector `remote_sv` lacks, in the wire format (sync step 2).
    #[cfg(feature = "untrusted-wire")]
    pub fn encode_diff(&self, remote_sv: &[u8]) -> Result<Vec<u8>, DocError> {
        self.inner.encode_diff(remote_sv)
    }

    /// Check an update from a connection that owns client `allowed`, without applying it:
    /// it must be well formed, and not skip ahead of this doc for any client. **New content
    /// only from `allowed`:** blocks from another client are accepted only when this doc
    /// already has all of them (for each such client, the update's highest clock is within
    /// this doc's state for it), as a Yjs client's resync carries an update it held as
    /// pending. Those are no-ops on apply (blocks are identified by client and clock, and
    /// known ones are skipped), so even forged content under a known ID changes nothing.
    /// Any block from another client past this doc's state is
    /// [`DocError::ForeignClient`]. `inserts_from` lists only clients with new content.
    #[cfg(feature = "untrusted-wire")]
    pub fn check_client_update(
        &self,
        update: &[u8],
        allowed: ClientId,
    ) -> Result<UpdateInfo, DocError> {
        check_client(allowed)?;
        self.inner.check_update(update, allowed)
    }

    /// Check, then apply, an update from a connection that owns client `allowed`.
    #[cfg(feature = "untrusted-wire")]
    pub fn apply_client_update(
        &mut self,
        update: &[u8],
        allowed: ClientId,
    ) -> Result<UpdateInfo, DocError> {
        check_client(allowed)?;
        self.inner.apply_client(update, allowed)
    }

    /// Apply an update this kit made itself (from `edit_as` or its own log). Never client bytes.
    #[allow(dead_code)] // used by sessions and the store (next milestones)
    pub(crate) fn apply_trusted(&mut self, update: &[u8]) -> Result<(), DocError> {
        self.inner.apply_trusted(update)
    }

    /// Apply `edits` as participant `client`, as one change. Returns the update to broadcast
    /// and log, or `None` when nothing changes.
    pub fn edit_as(
        &mut self,
        client: ClientId,
        edits: &[Edit],
    ) -> Result<Option<Vec<u8>>, DocError> {
        check_client(client)?;
        let mut any = false;
        for e in edits {
            match e {
                Edit::Text { root, hunks } => {
                    let current = self.text(root)?;
                    diff::validate(&current, hunks)?;
                    any |= !hunks.is_empty();
                }
            }
        }
        if !any {
            return Ok(None);
        }
        self.inner.edit_as(client, edits).map(Some)
    }

    /// Sample one edit in `every` for the runtime check of the edit path and the engine's
    /// text cache (0: never; default [`DEFAULT_EDIT_CHECK_EVERY`]; tests use 1). A sampled
    /// edit costs a full encode and a throwaway replica, O(doc), as every edit did before
    /// v0.9.3.
    pub fn set_edit_check_every(&mut self, every: u32) {
        self.inner.set_check_every(every);
    }

    /// What the runtime check found since the last call (the session reports each).
    pub(crate) fn take_engine_checks(&mut self) -> Vec<crate::engine::EngineCheck> {
        self.inner.take_checks()
    }

    /// Whether the runtime check switched this doc to the throwaway path (until reloaded).
    pub fn edit_path_fallback(&self) -> bool {
        self.inner.on_fallback()
    }

    /// Test-only (the `differential` feature): make the engine's cached text of `root` say
    /// `text`, as a bug in the cache would.
    #[cfg(any(test, feature = "differential"))]
    pub fn corrupt_text_cache_for_tests(&mut self, root: &str, text: &str) {
        self.inner.corrupt_text_cache(root, text);
    }

    /// How many clock units `client` has inserted into this doc: a change by `client`
    /// occupies the clocks between this before it and this after it (blame's index).
    pub(crate) fn clock_of(&self, client: ClientId) -> u32 {
        self.inner.clock_of(client)
    }

    /// Test-only: each text root's text read afresh from the engine (not the cache), sorted
    /// by root (the soak's diagnosis).
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn engine_texts_for_tests(&self) -> Vec<(&'static str, String)> {
        self.inner.texts_for_tests()
    }

    /// Whether this doc and `other` have the same state vector (the same inserts; deletes
    /// aren't in it). O(clients). The disk replica's marker (v0.9.4).
    pub(crate) fn same_state_vector(&self, other: &Doc) -> bool {
        self.inner.same_state_vector(&other.inner)
    }

    /// Whether this doc holds updates it couldn't integrate (missing dependencies) or is
    /// poisoned: K-AUD-1's no-pending check (v0.9.5). O(1).
    pub(crate) fn unresolved(&self) -> bool {
        self.inner.unresolved()
    }

    /// Text root `root`'s live text as runs of the client (and clocks) that inserted it, in
    /// order, covering the text exactly. **O(doc) per call** (a throwaway replica); leaves
    /// this doc as it was. Fails on a poisoned doc.
    pub(crate) fn blame_runs(&self, root: &str) -> Result<Vec<BlameRun>, DocError> {
        self.root(root, Kind::Text)?;
        self.inner.blame_runs(root)
    }

    /// The state vector as sorted `(client, clock)` pairs. The kit's unit tests only.
    #[cfg(test)]
    pub(crate) fn clocks_for_tests(&self) -> Vec<(ClientId, u32)> {
        self.inner.snapshot_for_tests().1
    }

    /// Deliver everything `from` has that this doc lacks. Both docs are this kit's, in one
    /// process: the diff is encoded against this doc's own state vector and applied as
    /// trusted bytes. Test-only ([`crate::conformance::replica`]).
    #[cfg(feature = "conformance")]
    pub(crate) fn sync_from_local(&mut self, from: &Doc) -> Result<(), DocError> {
        let update = from.inner.encode_diff(&self.inner.state_vector())?;
        self.inner.apply_trusted(&update)
    }

    /// The state vector as sorted `(client, clock)` pairs. Test-only.
    #[cfg(feature = "conformance")]
    pub(crate) fn state_clocks(&self) -> Vec<(ClientId, u32)> {
        self.inner.state_clocks()
    }

    /// What this doc has past `clocks`, a state it has seen. Test-only.
    #[cfg(feature = "conformance")]
    pub(crate) fn events_since(&self, clocks: &[(ClientId, u32)]) -> Result<Vec<u8>, DocError> {
        self.inner.events_since(clocks)
    }

    /// Deliver an update another of this kit's docs made in this process, refusing one that
    /// skips ahead of a client's history. Test-only.
    #[cfg(feature = "conformance")]
    pub(crate) fn deliver_local(&mut self, update: &[u8]) -> Result<(), DocError> {
        self.inner.deliver_local(update)
    }

    /// A relative position at byte offset `at` of text root `root`, stuck to the character
    /// after it (`right`) or before it. `None` when there is no character on that side.
    /// Test-only.
    #[cfg(feature = "conformance")]
    pub(crate) fn sticky(
        &self,
        root: &str,
        at: usize,
        right: bool,
    ) -> Result<Option<crate::engine::Sticky>, DocError> {
        let text = self.text(root)?;
        if at > text.len() || !text.is_char_boundary(at) {
            return Ok(None);
        }
        let at16 = text[..at].encode_utf16().count() as u32;
        Ok(self.inner.sticky(root, at16, right))
    }

    /// Where `pos` is now, as a byte offset of text root `root`. Test-only.
    #[cfg(feature = "conformance")]
    pub(crate) fn resolve_sticky(
        &self,
        root: &str,
        pos: &crate::engine::Sticky,
    ) -> Result<Option<usize>, DocError> {
        let text = self.text(root)?;
        let Some(at16) = self.inner.resolve_sticky(pos) else {
            return Ok(None);
        };
        let (mut n16, mut byte) = (0u32, 0usize);
        for ch in text.chars() {
            if n16 >= at16 {
                break;
            }
            n16 += ch.len_utf16() as u32;
            byte += ch.len_utf8();
        }
        // an offset inside a surrogate pair, or past the end, is not a position of the text
        Ok((n16 == at16).then_some(byte))
    }

    /// Replace a text root with `new` as participant `client`, through a minimal diff.
    pub fn replace_text_as(
        &mut self,
        client: ClientId,
        root: &str,
        new: &str,
    ) -> Result<Option<Vec<u8>>, DocError> {
        let hunks = diff::hunks(&self.text(root)?, new);
        self.edit_as(
            client,
            &[Edit::Text {
                root: root.to_owned(),
                hunks,
            }],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_TEXT: Schema = Schema {
        name: "t",
        roots: &[("source", Kind::Text)],
    };
    const TWO_TEXT: Schema = Schema {
        name: "t2",
        roots: &[("body", Kind::Text), ("notes", Kind::Text)],
    };

    #[test]
    fn unimplemented_primitives_are_refused_loudly() {
        const S: Schema = Schema {
            name: "x",
            roots: &[("t", Kind::Text), ("tree", Kind::Tree)],
        };
        assert!(matches!(
            Doc::new(S),
            Err(DocError::Unsupported(Kind::Tree))
        ));
    }

    #[test]
    fn roots_are_checked() {
        let mut d = Doc::new(ONE_TEXT).unwrap();
        assert!(matches!(d.text("nope"), Err(DocError::NoRoot { .. })));
        assert!(matches!(
            d.replace_text_as(5, "nope", "x"),
            Err(DocError::NoRoot { .. })
        ));
    }

    #[test]
    fn several_roots_in_one_change() {
        let mut d = Doc::new(TWO_TEXT).unwrap();
        d.edit_as(
            7,
            &[
                Edit::Text {
                    root: "body".into(),
                    hunks: diff::hunks("", "# Body\n"),
                },
                Edit::Text {
                    root: "notes".into(),
                    hunks: diff::hunks("", "n1\n"),
                },
            ],
        )
        .unwrap()
        .unwrap();
        assert_eq!(d.text("body").unwrap(), "# Body\n");
        assert_eq!(d.text("notes").unwrap(), "n1\n");
        let l = Doc::load(TWO_TEXT, &d.encode_state().unwrap()).unwrap();
        assert_eq!(l.text("notes").unwrap(), "n1\n");
    }

    #[test]
    fn invalid_hunks_change_nothing() {
        let mut d = Doc::new(ONE_TEXT).unwrap();
        d.replace_text_as(7, "source", "héllo").unwrap();
        let bad = Edit::Text {
            root: "source".into(),
            hunks: vec![Hunk {
                start: 2,
                end: 2,
                insert: "x".into(),
            }],
        };
        assert!(matches!(d.edit_as(7, &[bad]), Err(DocError::Hunk(_))));
        assert_eq!(d.text("source").unwrap(), "héllo");
    }
}
