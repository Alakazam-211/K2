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
//!   not the doc times the number of agents (tested in `tests/ephemeral_replicas.rs`).

use crate::engine::{EngineDoc, YrsDoc};
use crate::schema::{Kind, Schema};
use crate::text::diff::{self, Hunk, HunkError};

/// A participant's client ID. 53 bits, so it is safe as a JavaScript number.
pub type ClientId = u64;

/// Largest valid client ID (2^53 - 1).
pub const MAX_CLIENT_ID: ClientId = (1 << 53) - 1;

/// What a checked client update contains.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateInfo {
    /// Client IDs the update inserts content under (at most the allowed one).
    pub inserts_from: Vec<ClientId>,
    /// True when the update only deletes (or carries nothing).
    pub delete_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
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
        let info = self.check_client_update(update, allowed)?;
        self.inner.apply_trusted(update)?;
        Ok(info)
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
