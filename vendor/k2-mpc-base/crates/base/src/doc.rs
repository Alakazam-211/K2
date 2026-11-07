//! The loaded doc: what sessions and packs use.
//!
//! Engine-independent surface:
//! - wire and store bytes are opaque `Vec<u8>`;
//! - edits are data ([`Edit`]), applied as one participant in one change;
//! - positions never cross the API (text edits are [`Hunk`]s in UTF-8 byte offsets of the
//!   current text; list elements are named by [`Key`]s, writes by opaque [`WriteId`]s; wire
//!   positions are the engine's own relative positions).
//! - Structured kinds (MovableList, Map): ops, readers and the applied stream are in
//!   [`crate::elem`].
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
//!   until reloaded). Past [`DEFAULT_EDIT_CHECK_SCALE_UNITS`] of history the interval grows
//!   with the doc's size ([`Doc::set_edit_check_scale`]), so the check's cost per edit stays
//!   bounded.

use std::collections::BTreeMap;

use crate::elem::{
    keyed_diff, Applied, Edited, ElemState, Facet, Key, KeyedRoot, KeyedState, ListOp, MapOp,
    OpRefusal, Value, WriteId,
};
use crate::engine::{BlameRun, Engine, EngineDoc};
use crate::schema::{Kind, Schema};
use crate::text::diff::{self, Hunk, HunkError};

/// A participant's client ID. 53 bits, so it is safe as a JavaScript number.
pub type ClientId = u64;

/// Largest valid client ID (2^53 - 1).
pub const MAX_CLIENT_ID: ClientId = (1 << 53) - 1;

/// Reserved for the native engine's import agent: the kit never hands it out.
pub const IMPORT_CLIENT: ClientId = MAX_CLIENT_ID;

/// Largest client ID the kit mints (2^53 - 2). A session's client IDs are
/// `2..=MAX_KIT_CLIENT`: never 0, never 1 (the session's own), never [`IMPORT_CLIENT`]; a
/// random draw outside that range is drawn again, never shifted into it.
pub const MAX_KIT_CLIENT: ClientId = MAX_CLIENT_ID - 1;

/// How often [`Doc::edit_as`]'s runtime check samples an edit by default: one in 1,000.
pub const DEFAULT_EDIT_CHECK_EVERY: u32 = 1000;

/// The doc size past which [`Doc::edit_as`]'s runtime check samples less often by default
/// (v0.9.7): 50,000 UTF-16 units of history ([`Doc::set_edit_check_scale`]), about the
/// history at version 10,000 in the 32-participant soak. Past it the interval grows with the
/// history, so the check's amortised cost per edit stays about what it is there.
pub const DEFAULT_EDIT_CHECK_SCALE_UNITS: u64 = 50_000;

/// The default rate of the engine's position index check (v0.9.7): one marker lookup in this
/// many also walks the text from its start and compares ([`Doc::set_position_check_every`]).
pub const DEFAULT_POSITION_CHECK_EVERY: u32 = 1000;

/// What a checked client update contains.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateInfo {
    /// Client IDs the update inserts content under (at most the allowed one).
    pub inserts_from: Vec<ClientId>,
    /// True when the update only deletes (or carries nothing).
    pub delete_only: bool,
}

/// [`DocError::Unaccepted`]'s error id (`None` in v0.9.5, before the registry): `MPC-E152`
/// `unaccepted` (`docs/ERRORS.md`).
pub const RESERVED_ERROR_ID_UNACCEPTED: Option<&str> = Some("MPC-E152");
/// [`DocError::Quarantined`]'s error id (`None` in v0.9.5, before the registry): `MPC-E518`
/// `quarantined` (`docs/ERRORS.md`).
pub const RESERVED_ERROR_ID_QUARANTINED: Option<&str> = Some("MPC-E518");

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
    /// A client update was refused, unapplied (hardening, v0.9.5): it carries content the
    /// kit never uses (subdocuments). Exit 3 (`tools::exit::REFUSED`). Error id `MPC-E152`
    /// ([`RESERVED_ERROR_ID_UNACCEPTED`]).
    #[error("update contains content this host doesn't accept")]
    Unaccepted,
    /// A stored doc holds content the kit never uses (subdocuments): it is quarantined, not
    /// loaded as live (hardening: the store-load scan, v0.9.5). Exit 3. Error id `MPC-E518`
    /// ([`RESERVED_ERROR_ID_QUARANTINED`]).
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
    /// A schema's root name: empty, holding `/` (the engine's layout uses it), or twice.
    #[error("schema root name {0:?}: names must be non-empty, unique and free of '/'")]
    RootName(String),
    /// A structured op is refused; nothing of the change is applied.
    #[error("op {index} on root {root:?}: {refusal}")]
    Op {
        root: String,
        index: usize,
        refusal: OpRefusal,
    },
    /// The doc holds something the kit never writes (a bug or a damaged store).
    #[error("the doc's state is malformed: {0}")]
    Malformed(String),
}

/// A root's content as plain data, independent of the engine (versions, export, restore).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Neutral {
    Text(String),
    /// A MovableList's live elements, in order.
    List(Vec<(Key, Value)>),
    /// A Map's keys and values.
    Map(BTreeMap<String, Value>),
}

impl Neutral {
    /// The text, for a text root.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Neutral::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn into_text(self) -> Option<String> {
        match self {
            Neutral::Text(t) => Some(t),
            _ => None,
        }
    }
}

/// One change to one root, as data. A participant's edit is a list of these, applied together.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Replace ranges of a text root (hunks against its current text).
    Text { root: String, hunks: Vec<Hunk> },
    /// Ops on a MovableList root, applied in order ([`crate::elem::ListOp`]).
    List { root: String, ops: Vec<ListOp> },
    /// Ops on a Map root, applied in order ([`crate::elem::MapOp`]).
    Map { root: String, ops: Vec<MapOp> },
}

impl Edit {
    /// The root the edit changes.
    pub fn root(&self) -> &str {
        match self {
            Edit::Text { root, .. } | Edit::List { root, .. } | Edit::Map { root, .. } => root,
        }
    }
}

/// A doc on a pack's schema, on its engine (Yrs, and with feature `native` the native engine,
/// chosen by `EngineChoice`).
pub struct Doc {
    schema: Schema,
    inner: Engine,
}

/// A writer's client ID: `2..=MAX_KIT_CLIENT`. Refused: 0 (no client), 1 (the session's own),
/// [`IMPORT_CLIENT`] (2^53 - 1, the native engine's import agent) and anything past 53 bits.
pub(crate) fn check_client(id: ClientId) -> Result<(), DocError> {
    if id == 0 || id > MAX_KIT_CLIENT || id == crate::engine::SESSION_CLIENT {
        Err(DocError::BadClient(id))
    } else {
        Ok(())
    }
}

/// Every kind implemented; root names non-empty, unique and free of `/` (a MovableList root
/// `R` is several engine roots named `R/...`, so `/` would let two roots collide).
fn check_schema(schema: &Schema) -> Result<(), DocError> {
    for (i, (name, k)) in schema.roots.iter().enumerate() {
        if !k.implemented() {
            return Err(DocError::Unsupported(*k));
        }
        if name.is_empty() || name.contains('/') || schema.roots[..i].iter().any(|(n, _)| n == name)
        {
            return Err(DocError::RootName((*name).to_owned()));
        }
    }
    Ok(())
}

impl Doc {
    /// The Yrs doc, for a Yrs-only hook of a test, `differential` or `conformance` build (C2:
    /// [`crate::engine::Engine::yrs`] is `None` on another engine). A hook with a meaning on
    /// every engine says what it does on each instead of calling this.
    #[allow(dead_code)] // test and feature builds only
    fn yrs_only(&self) -> &crate::engine::YrsDoc {
        self.inner
            .yrs()
            .expect("a Yrs-only test hook, on a doc that runs on another engine")
    }

    #[allow(dead_code)] // test and feature builds only
    fn yrs_only_mut(&mut self) -> &mut crate::engine::YrsDoc {
        self.inner
            .yrs_mut()
            .expect("a Yrs-only test hook, on a doc that runs on another engine")
    }

    /// A new, empty doc.
    pub fn new(schema: Schema) -> Result<Self, DocError> {
        check_schema(&schema)?;
        Ok(Self {
            inner: Engine::new(&schema)?,
            schema,
        })
    }

    /// Load from [`Doc::encode_state`] output (trusted: from our own store).
    pub fn load(schema: Schema, state: &[u8]) -> Result<Self, DocError> {
        check_schema(&schema)?;
        Ok(Self {
            inner: Engine::load(&schema, state)?,
            schema,
        })
    }

    /// Rebuild a doc from what the kit's store holds: the snapshot, then the log after it.
    /// Store bytes are trusted: the kit wrote them (see [`crate::store`]). The log is
    /// replayed through [`crate::store::Loaded::yrs_updates`] only: a log holding a native
    /// entry is [`DocError::Malformed`] before anything is replayed.
    pub fn from_store(schema: Schema, loaded: &crate::store::Loaded) -> Result<Self, DocError> {
        timed!(LoadDoc);
        let updates = loaded
            .yrs_updates()
            .map_err(|e| DocError::Malformed(format!("the log can't be replayed on Yrs: {e}")))?;
        let mut d = match &loaded.snapshot {
            Some((_, state)) => Self::load(schema, state)?,
            None => Self::new(schema)?,
        };
        for u in updates {
            #[cfg(test)]
            YRS_REPLAYS.with(|n| n.set(n.get() + 1));
            d.inner.apply_trusted(u)?;
        }
        Ok(d)
    }

    /// Every root's content as plain data, in schema order. Fails only on a malformed doc.
    pub fn export_neutral(&self) -> Result<Vec<(String, Neutral)>, DocError> {
        self.schema
            .roots
            .iter()
            .map(|(name, kind)| {
                let n = match kind {
                    Kind::Text => Neutral::Text(self.inner.text(name)?),
                    Kind::MovableList => Neutral::List(self.inner.list(name)?),
                    Kind::Map => Neutral::Map(self.inner.map(name)?),
                    // check_schema refuses unimplemented kinds
                    other => unreachable!("unimplemented kind {other:?} in a live doc"),
                };
                Ok(((*name).to_owned(), n))
            })
            .collect()
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// Every root's content by key, deleted list elements' values included: the state a
    /// keyed diff compares ([`Doc::applied_since`]). O(doc). Fails only on a malformed doc.
    pub fn keyed_state(&self) -> Result<KeyedState, DocError> {
        let roots = self
            .schema
            .roots
            .iter()
            .map(|(name, kind)| {
                let r = match kind {
                    Kind::Text => KeyedRoot::Text(self.inner.text(name)?),
                    Kind::MovableList => KeyedRoot::List {
                        live: self.inner.list(name)?,
                        hidden: self.inner.hidden(name)?,
                        places: self.inner.places(name)?,
                    },
                    Kind::Map => KeyedRoot::Map(self.inner.map(name)?),
                    // check_schema refuses unimplemented kinds
                    other => unreachable!("unimplemented kind {other:?} in a live doc"),
                };
                Ok(((*name).to_owned(), r))
            })
            .collect::<Result<_, DocError>>()?;
        Ok(KeyedState { roots })
    }

    /// What changed since `before` (this doc's [`Doc::keyed_state`], taken earlier), as an
    /// applied stream: how a **merged update** (another replica's changes, applied as
    /// bytes) reports what it did, derived by a keyed diff ([`crate::elem::keyed_diff`],
    /// CSV seam design §4.2). Each op's write is the facet's winner now, `None` where this
    /// engine keeps no identity (a restore, a map delete). A change made here reports its
    /// own ops exactly ([`Doc::edit_applied`]); for the same change the two agree, up to the
    /// writes this engine can't name. O(doc).
    pub fn applied_since(
        &self,
        before: &KeyedState,
    ) -> Result<Vec<Applied<Option<WriteId>>>, DocError> {
        let after = self.keyed_state()?;
        let mut err = None;
        let out = keyed_diff(before, &after, |root, key, facet| {
            match self.inner.winner(root, key, facet) {
                Ok(w) => w,
                Err(e) => {
                    err.get_or_insert(e);
                    None
                }
            }
        });
        match err {
            Some(e) => Err(e),
            None => Ok(out),
        }
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
        self.inner.text(root)
    }

    /// List root `root`'s elements in place order, live and deleted (a deleted element keeps
    /// its place). O(slots) when the order isn't cached.
    pub(crate) fn places(&self, root: &str) -> Result<Vec<Key>, DocError> {
        self.root(root, Kind::MovableList)?;
        self.inner.places(root)
    }

    /// A MovableList root's live elements, in order, with their values. O(elements).
    pub fn list(&self, root: &str) -> Result<Vec<(Key, Value)>, DocError> {
        self.root(root, Kind::MovableList)?;
        self.inner.list(root)
    }

    /// Where `key` is in a MovableList root: live (and at which index), deleted, or never
    /// inserted. O(1) between changes (the live order is cached).
    pub fn elem(&self, root: &str, key: &Key) -> Result<ElemState, DocError> {
        self.root(root, Kind::MovableList)?;
        self.inner.elem(root, key.as_str())
    }

    /// The value of a MovableList root's element, live or deleted; `None` if `key` was never
    /// inserted.
    pub fn elem_value(&self, root: &str, key: &Key) -> Result<Option<Value>, DocError> {
        self.root(root, Kind::MovableList)?;
        self.inner.elem_value(root, key.as_str())
    }

    /// A Map root's keys and values.
    pub fn map(&self, root: &str) -> Result<BTreeMap<String, Value>, DocError> {
        self.root(root, Kind::Map)?;
        self.inner.map(root)
    }

    pub fn map_get(&self, root: &str, key: &str) -> Result<Option<Value>, DocError> {
        self.root(root, Kind::Map)?;
        self.inner.map_get(root, key)
    }

    /// The write that holds a register now (engine answer E4): for a MovableList element its
    /// `Value`, `Position` or `Existence` facet, for a Map key its `Value`. O(1). `None` when
    /// no write holds it: an absent map key or an unknown element, a value never set since an
    /// insert that gave none, and the existence of a live element (on this engine an undelete
    /// leaves no identity, so only a delete is a winner of that facet).
    pub fn winner(&self, root: &str, key: &str, facet: Facet) -> Result<Option<WriteId>, DocError> {
        match self.schema.kind_of(root) {
            Some(Kind::MovableList | Kind::Map) => self.inner.winner(root, key, facet),
            Some(k) => Err(DocError::WrongKind {
                root: root.to_owned(),
                wanted: Kind::Map,
                actual: k,
            }),
            None => Err(DocError::NoRoot {
                schema: self.schema.name,
                root: root.to_owned(),
            }),
        }
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
        self.client_checked(update, allowed)
    }

    /// Check, then apply, an update from a connection that owns client `allowed`.
    #[cfg(feature = "untrusted-wire")]
    pub fn apply_client_update(
        &mut self,
        update: &[u8],
        allowed: ClientId,
    ) -> Result<UpdateInfo, DocError> {
        self.client_applied(update, allowed)
    }

    /// The one client check: [`Doc::check_client_update`]'s body, and the conformance
    /// suite's hook ([`crate::conformance::replica::check_from`], kit-made bytes only).
    #[cfg(any(feature = "untrusted-wire", feature = "conformance"))]
    pub(crate) fn client_checked(
        &self,
        update: &[u8],
        allowed: ClientId,
    ) -> Result<UpdateInfo, DocError> {
        check_client(allowed)?;
        self.inner.check_update(update, allowed)
    }

    /// The one client check-then-apply: [`Doc::apply_client_update`]'s body, and the
    /// conformance suite's hook ([`crate::conformance::replica::deliver_from`], kit-made
    /// bytes only).
    #[cfg(any(feature = "untrusted-wire", feature = "conformance"))]
    pub(crate) fn client_applied(
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
    /// and log, or `None` when nothing changes ([`Doc::edit_applied`] also says what each op
    /// did).
    pub fn edit_as(
        &mut self,
        client: ClientId,
        edits: &[Edit],
    ) -> Result<Option<Vec<u8>>, DocError> {
        Ok(self.edit_applied(client, edits)?.update)
    }

    /// [`Doc::edit_as`], with the applied stream (A5): per edit, in order, its text hunks,
    /// or each structured op as applied (live indices and values before and after, the
    /// op's [`WriteId`], its effect). Structured ops are checked one by one in order, each
    /// against the state the previous ones left; a refused op ([`DocError::Op`]) refuses
    /// the whole change. Ops that change nothing (deleting a deleted element, restoring a
    /// live one, deleting an absent map key) are reported with [`crate::elem::Effect::None`];
    /// when every op is one and no text changes, `update` is `None`.
    pub fn edit_applied(&mut self, client: ClientId, edits: &[Edit]) -> Result<Edited, DocError> {
        Ok(self.edit_made(client, edits)?.edited)
    }

    /// [`Doc::edit_applied`], with each text hunk's first inserted clock as the engine
    /// applied it ([`crate::engine::Made`]): what the session's provenance and clock spans
    /// name the change's elements by (C2: never predicted after the fact).
    pub(crate) fn edit_made(
        &mut self,
        client: ClientId,
        edits: &[Edit],
    ) -> Result<crate::engine::Made, DocError> {
        // every edit path's shared body (`edit_as`, `edit_applied`, the session's `edit_made`)
        timed!(EditAs);
        check_client(client)?;
        let mut text = false;
        for e in edits {
            match e {
                Edit::Text { root, hunks } => {
                    let current = self.text(root)?;
                    diff::validate(&current, hunks)?;
                    text |= !hunks.is_empty();
                }
                Edit::List { root, .. } => self.root(root, Kind::MovableList)?,
                Edit::Map { root, .. } => self.root(root, Kind::Map)?,
            }
        }
        if !text && edits.iter().all(|e| matches!(e, Edit::Text { .. })) {
            return Ok(crate::engine::Made {
                edited: Edited {
                    update: None,
                    applied: Vec::new(),
                },
                firsts: vec![Vec::new(); edits.len()],
            });
        }
        self.inner.edit_as(client, edits)
    }

    /// Sample one edit in `every` for the runtime check of the edit path and the engine's
    /// text cache (0: never; default [`DEFAULT_EDIT_CHECK_EVERY`]; tests use 1). A sampled
    /// edit costs a full encode and a throwaway replica, O(doc), as every edit did before
    /// v0.9.3.
    pub fn set_edit_check_every(&mut self, every: u32) {
        self.inner.set_check_every(every);
    }

    /// The runtime check's adaptive interval (v0.9.7): a sampled edit costs O(the doc's
    /// history), so past `units` of history (UTF-16 units: the sum of the state vector's
    /// clocks, every unit ever inserted) the interval grows in proportion to it, `every x
    /// history / units` edits, never fewer than `every` ([`Doc::set_edit_check_every`]). The
    /// check's amortised cost per edit stays bounded; a doc below `units` is checked one edit
    /// in `every`. 0: a fixed rate. Default [`DEFAULT_EDIT_CHECK_SCALE_UNITS`]. Both setters
    /// (the session calls them after every load) place the next sample from the doc's
    /// history alone: `every` edits away below `units`, and past it at a phase spread over
    /// the interval, so the long-run rate is one edit in the interval however often the doc
    /// is reloaded.
    pub fn set_edit_check_scale(&mut self, units: u64) {
        self.inner.set_check_scale(units);
    }

    /// The engine's position index (v0.9.7) finds a text position from a recent one (a
    /// marker) instead of walking the text from its start. One marker lookup in `every` also
    /// walks and compares (0: never; default [`DEFAULT_POSITION_CHECK_EVERY`]; tests use 1); a
    /// mismatch uses the walk, drops the text's markers and is reported.
    pub fn set_position_check_every(&mut self, every: u32) {
        self.inner.set_position_every(every);
    }

    /// The engine's position index counters since this doc was made or loaded (test-only:
    /// the soak's hit rate, v0.9.7).
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn position_index_stats(&self) -> crate::differential::PositionIndexStats {
        let [lookups, marker_hits, walks, shadow_checks, mismatches, stale_markers] =
            self.yrs_only().position_stats();
        crate::differential::PositionIndexStats {
            lookups,
            marker_hits,
            walks,
            shadow_checks,
            mismatches,
            stale_markers,
        }
    }

    /// Test-only: the doc's history in UTF-16 units, the runtime check's measure of its size
    /// ([`Doc::set_edit_check_scale`]; the soak prints it, v0.9.7).
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn history_units_for_tests(&self) -> u64 {
        self.yrs_only().history_units_for_tests()
    }

    /// Test-only: each text root's text read afresh from the engine (not the cache), sorted
    /// by root (the soak's diagnosis, v0.9.7).
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn engine_texts_for_tests(&self) -> Vec<(&'static str, String)> {
        self.yrs_only().texts_for_tests()
    }

    /// Test-only: plant a bug in the engine's position index (until the doc is reloaded).
    #[cfg(feature = "planted-position-bugs")]
    #[doc(hidden)]
    pub fn plant_position_bug(&self, bug: crate::differential::PlantedPositionBug) {
        self.yrs_only().plant_position_bug(bug.into());
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
        self.yrs_only_mut().corrupt_text_cache(root, text);
    }

    /// How many clock units `client` has inserted into this doc: a change by `client`
    /// occupies the clocks between this before it and this after it (blame's index).
    pub(crate) fn clock_of(&self, client: ClientId) -> u64 {
        self.inner.clock_of(client)
    }

    /// The largest clock the doc's engine makes ([`EngineDoc::max_clock`]): the bound on
    /// clocks the store holds for it.
    pub(crate) fn max_clock(&self) -> u64 {
        self.inner.max_clock()
    }

    /// The doc's engine units ([`crate::engine::Units`]): what its clocks count.
    pub(crate) fn units(&self) -> crate::engine::Units {
        self.inner.units()
    }

    /// Whether a session may store this doc (C2): until C3 gives native docs their own
    /// stamp and record format, only a doc on Yrs (a native doc would be stamped as Yrs's).
    /// Every doc a session makes or loads is on Yrs today; this holds that line.
    pub(crate) fn check_storable(&self) -> Result<(), DocError> {
        Ok(())
    }

    /// How the doc's engine numbers what a change inserts ([`crate::engine::IdOrder`]): the
    /// prediction undo's planner uses before anything is applied.
    pub(crate) fn id_order(&self) -> crate::engine::IdOrder {
        self.inner.id_order()
    }

    /// Whether this doc and `other` have the same state vector (the same inserts; deletes
    /// aren't in it). O(clients). The disk replica's marker (v0.9.4).
    pub(crate) fn same_state_vector(&self, other: &Doc) -> bool {
        self.inner.same_state_vector(&other.inner)
    }

    /// Whether this doc and `other` hold the same content: the same inserts (state
    /// vectors), the same deletions (delete sets) and the same texts, whatever their block
    /// layouts. Two docs given the same changes may split equal content into blocks
    /// differently, so they can encode differently: compare docs with this, not with their
    /// encodings. O(doc). (v0.9.7)
    pub fn same_content(&self, other: &Doc) -> bool {
        self.inner.same_content(&other.inner)
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
        self.yrs_only().snapshot_for_tests().1
    }

    /// Deliver everything `from` has that this doc lacks: two of this kit's docs in one
    /// process, kit-made bytes only (the concurrency tests' merge). Unit tests only.
    #[cfg(test)]
    pub(crate) fn merge_from(&mut self, from: &Doc) -> Result<(), DocError> {
        let update = from.inner.encode_diff(&self.inner.state_vector())?;
        self.inner.apply_trusted(&update)
    }

    #[cfg(test)]
    pub(crate) fn engine(&self) -> &crate::engine::YrsDoc {
        self.yrs_only()
    }

    /// Deliver everything `from` has that this doc lacks. Both docs are this kit's, in one
    /// process: the diff is encoded against this doc's own state vector and applied as
    /// trusted bytes. Test-only ([`crate::conformance::replica`]).
    #[cfg(feature = "conformance")]
    pub(crate) fn sync_from_local(&mut self, from: &Doc) -> Result<(), DocError> {
        let update = from.inner.encode_diff(&self.inner.state_vector())?;
        self.inner.apply_trusted(&update)
    }

    /// The state vector as sorted `(client, clock)` pairs (the native engine: its version
    /// vector, each agent's next seq). Test-only.
    #[cfg(feature = "conformance")]
    pub(crate) fn state_clocks(&self) -> Vec<(ClientId, u64)> {
        self.yrs_only()
            .state_clocks()
            .into_iter()
            .map(|(c, k)| (c, u64::from(k)))
            .collect()
    }

    /// What this doc has past `clocks`, a state it has seen. Test-only.
    #[cfg(feature = "conformance")]
    pub(crate) fn events_since(&self, clocks: &[(ClientId, u64)]) -> Result<Vec<u8>, DocError> {
        let mut c32 = Vec::with_capacity(clocks.len());
        for &(c, k) in clocks {
            let k = u32::try_from(k)
                .map_err(|_| DocError::Malformed(format!("a Yrs clock past u32: {k}")))?;
            c32.push((c, k));
        }
        self.yrs_only().events_since(&c32)
    }

    /// Deliver an update another of this kit's docs made in this process, refusing one that
    /// skips ahead of a client's history. Test-only.
    #[cfg(feature = "conformance")]
    pub(crate) fn deliver_local(&mut self, update: &[u8]) -> Result<(), DocError> {
        self.yrs_only_mut().deliver_local(update)
    }

    /// This doc loaded from what it keeps at rest: on Yrs its encoded state; on the native
    /// engine its records, with its digest key (the engine's own load). Test-only.
    #[cfg(feature = "conformance")]
    pub(crate) fn reloaded(&self) -> Result<Doc, DocError> {
        Doc::load(self.schema, &self.encode_state()?)
    }

    /// Hardening (test-only): the next apply after a passed check on a native doc errs after
    /// the engine applied it. `false` on a doc on Yrs (no such hook there).
    #[cfg(feature = "conformance")]
    pub(crate) fn fail_next_apply(&mut self) -> bool {
        false
    }

    /// How many times a native doc's replica was reopened after an apply error (test-only);
    /// `None` on Yrs.
    #[cfg(feature = "conformance")]
    pub(crate) fn reopens(&self) -> Option<u32> {
        None
    }

    /// How many runs the current places of `keys` form in list root `root` (adjacent slots,
    /// consecutive engine IDs); `None` when a key was never inserted. Test-only.
    #[cfg(feature = "conformance")]
    pub(crate) fn slot_runs(&self, root: &str, keys: &[Key]) -> Result<Option<usize>, DocError> {
        self.root(root, Kind::MovableList)?;
        self.yrs_only().slot_runs(root, keys)
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
        Ok(self.yrs_only().sticky(root, at16, right))
    }

    /// Where `pos` is now, as a byte offset of text root `root`. Test-only.
    #[cfg(feature = "conformance")]
    pub(crate) fn resolve_sticky(
        &self,
        root: &str,
        pos: &crate::engine::Sticky,
    ) -> Result<Option<usize>, DocError> {
        let text = self.text(root)?;
        let Some(at16) = self.yrs_only().resolve_sticky(pos) else {
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

    /// Replace a text root with `new` as participant `client`, through a minimal diff that
    /// pairs lines first ([`crate::text::lines::edit_hunks`], v0.9.8, as the session's edits
    /// do): a line deleted or inserted next to a line with the same start is that line, so
    /// the neighbour keeps its own elements. One text applied to the text it replaces: there
    /// is no other side and nothing merges.
    pub fn replace_text_as(
        &mut self,
        client: ClientId,
        root: &str,
        new: &str,
    ) -> Result<Option<Vec<u8>>, DocError> {
        let hunks = crate::text::lines::edit_hunks(&self.text(root)?, new);
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
thread_local! {
    /// The Yrs updates [`Doc::from_store`] has replayed on this thread (the store rows'
    /// replay counter).
    pub(crate) static YRS_REPLAYS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
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

    /// `replace_text_as` pairs lines first (v0.9.8): deleting `seed:2` keeps every element of
    /// `seed:1` and `seed:3` (a character diff deleted `2\nseed:` and left `seed:3` built from
    /// the deleted line's `seed:`), and a line inserted before `seed:3` is whole, at its start.
    #[test]
    fn replace_text_pairs_lines_first() {
        let run = |bytes, client, clock| BlameRun {
            bytes,
            client,
            clock,
        };
        let mut d = Doc::new(ONE_TEXT).unwrap();
        d.replace_text_as(7, "source", "seed:1\nseed:2\nseed:3\n")
            .unwrap();
        d.replace_text_as(8, "source", "seed:1\nseed:3\n").unwrap();
        assert_eq!(
            d.blame_runs("source").unwrap(),
            vec![run(7, 7, 0), run(7, 7, 14)]
        );
        d.replace_text_as(9, "source", "seed:1\nseed:9\nseed:3\n")
            .unwrap();
        assert_eq!(
            d.blame_runs("source").unwrap(),
            vec![run(7, 7, 0), run(7, 9, 0), run(7, 7, 14)]
        );
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
