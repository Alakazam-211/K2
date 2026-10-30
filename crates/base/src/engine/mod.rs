//! The engine seam. Private to the kit: no pack sees an engine type.
//!
//! To replace the engine, implement [`EngineDoc`] and switch [`crate::doc::Doc`] to it.
//! Packs, sessions and stored formats above this line don't change (stored state carries
//! an engine/format stamp so it can be re-encoded on upgrade).

mod yrs;

#[cfg(any(test, feature = "differential"))]
pub(crate) use self::yrs::TextPlan;
pub(crate) use self::yrs::{insert_clocks, YrsDoc};

/// A relative position in a text root (the conformance suite's anchors only).
#[cfg(feature = "conformance")]
pub(crate) type Sticky = ::yrs::StickyIndex;

use crate::doc::{ClientId, DocError, Edit, UpdateInfo};
use crate::schema::Schema;

/// The doc's own client ID. It never authors content.
pub(crate) const SESSION_CLIENT: ClientId = 1;

/// Engine identity and stored-state encoding, stamped on each doc in the store
/// ([`crate::store::DocMeta`]) so a later release can re-encode on upgrade.
pub(crate) const ENGINE_NAME: &str = "yrs-0.28";
/// Stored state is Yjs v2.
pub(crate) const ENGINE_FORMAT: u32 = 2;

/// One run of a text root's live text inserted by one client with consecutive clocks
/// ([`EngineDoc::blame_runs`]). The runs of a root, in order, cover its text exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlameRun {
    /// The run's length in UTF-8 bytes.
    pub bytes: usize,
    /// The client that inserted it.
    pub client: ClientId,
    /// The clock of its first character. Clocks count UTF-16 code units: the character at
    /// UTF-16 offset `k` of the run has clock `clock + k`.
    pub clock: u32,
}

/// What the engine's sampled runtime check (the canary, v0.9.3) found: an edit whose fast
/// update differed from the throwaway replica's (the throwaway's was served; the doc uses
/// that path until reload), or a cached text that differed from a fresh read (the fresh one
/// was used; the cache is off until reload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EngineCheck {
    EditPath {
        client: ClientId,
    },
    TextCache {
        root: &'static str,
    },
    /// The engine's position index (v0.9.7): a shadow check found the marker path and the
    /// full walk disagree on text `root` (`None`: a nested text) at `index`, from the marker
    /// at `marker`. The walk's result was used and the text's markers dropped.
    Position {
        root: Option<String>,
        index: u32,
        marker: u32,
    },
}

#[allow(dead_code)] // the trait documents the seam; one implementation today
pub(crate) trait EngineDoc: Sized + Send {
    fn new(schema: &Schema) -> Self;
    fn load(schema: &Schema, state: &[u8]) -> Result<Self, DocError>;
    fn text(&self, root: &str) -> String;
    fn state_vector(&self) -> Vec<u8>;
    /// Fails (never panics) on a poisoned doc: see [`crate::doc::DocError::Pending`].
    fn encode_state(&self) -> Result<Vec<u8>, DocError>;
    fn encode_diff(&self, remote_sv: &[u8]) -> Result<Vec<u8>, DocError>;
    fn check_update(&self, update: &[u8], allowed: ClientId) -> Result<UpdateInfo, DocError>;
    fn apply_trusted(&mut self, update: &[u8]) -> Result<(), DocError>;
    /// Apply already-validated edits as `client`, as one change on this doc (v0.9.3: on the doc
    /// itself, as the client, with a sampled check against a throwaway replica); return the
    /// update.
    fn edit_as(&mut self, client: ClientId, edits: &[Edit]) -> Result<Vec<u8>, DocError>;
    /// How many clock units `client` has inserted into this doc (0 if none): a change by
    /// `client` occupies the clocks between this before it and this after it.
    fn clock_of(&self, client: ClientId) -> u32;
    /// Blame (K-HIST-11): text root `root`'s live text as runs, each with the client and
    /// clock that inserted it. **O(doc) per call** (the engine copies the doc to a throwaway
    /// replica and walks it); never on the edit path. Leaves this doc as it was.
    fn blame_runs(&self, root: &str) -> Result<Vec<BlameRun>, DocError>;
}
