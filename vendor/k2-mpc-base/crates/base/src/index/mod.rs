//! The file-structure index: a tree of **nodes** read out of one version of a blob doc, kept
//! beside the blob (`docs/SESSIONS.md`, "The structure index"). Reading a file is the type
//! pack's ([`crate::session::BlobPack::index`]); everything here is generic: the kit knows
//! nodes, keys, hashes and bounds, and no file format. What a format wants to add goes in a
//! node's `attrs` and `flags`.
//!
//! **Bounded and untrusted.** The bytes come from a hostile file, read in a sandboxed worker
//! process ([`IndexOutcome`] and its wire form). Whatever comes back is an [`IndexDraft`], and
//! the kit checks its shape: node count and depth against [`IndexLimits`], every field against
//! the `INDEX_*` constants, parents only ever earlier nodes. A draft that breaks a cap is
//! recorded as not indexed (the file really is that big); one that breaks the shape is refused
//! and nothing is recorded (the worker is wrong, not the file).
//!
//! **Hashes are keyed.** A pack hands the kit raw digests ([`Digest`]); the kit stores only
//! `HMAC-SHA256(key derived from the space key, digest)` ([`NodeHash`]), so equal content in
//! two spaces is not comparable and destroying the space key makes the hashes meaningless.
//! Neither a raw digest nor the key reaches the store or an output.
//!
//! **Minted keys.** A node whose file gives it no unique ID takes a key the kit mints once
//! and carries forward by alignment (kind + name + parent path, then position; a tie never
//! pairs). They live per doc beside the content-keyed row, because a row is a pure function
//! of the bytes and a minted key depends on the version before it.
//!
//! **The view items** (row format 2, [`INDEX_ROW_FORMAT`]). What a view of a version draws that
//! the plain fields don't say, read by the pack's reader with the rest (so the bytes are parsed
//! where the reader runs, a host's sandboxed worker, never in the session): per node a
//! [`NodeView`] (its box on the page, the field kinds its text holds, the text a view shows,
//! whether an agent can set it, the fonts it uses) and a render digest, keyed like every hash;
//! per doc the fonts ([`IndexDraft::fonts`]). A row with none of them is stored as format 1,
//! byte for byte what a kit before them stored, and a row of format 1 reads with them absent.
//! A pack hands them only from a format it bumped to for them, naming the older one
//! ([`IndexSpec::with_older_format`]): an older kit, asking under the older format, never meets
//! a row of format 2.
//!
//! **A container's own record** (row format 3). A pack whose node record holds its children's
//! (a slide's whole XML) can also give each such node its own record's digest
//! ([`DraftNode::own`]): the record with each child's span replaced by a stand-in naming the
//! child. The kit's overlap checks (`--base` and a revert's) then count a change to that node
//! that its changed children wholly explain as theirs, not its own
//! ([`NodeChange::by_children`]). A row with no own digest is stored as format 2 (or 1), byte
//! for byte as before; a pack hands them only from a format it bumped to for them, naming the
//! older one, as for the view items.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::hash::{hex, sha256, HmacSha256, Sha256};
use crate::store::VersionId;

mod align;
mod draft;
pub(crate) mod read;
mod row;

pub use draft::{cut_text, index_to_wire, DraftNode, IndexDraft, IndexOutcome, IndexRefused};
pub use read::{
    Identity, IndexBlame, IndexDiff, ListQuery, MissingAddress, NodeBlame, NodeChange,
    NodeChangeKind, NodeLabel, NodeRead, NodesRead, Outline, OutlineNode, SaveCrossing,
    BLAME_WALK_MAX, CANDIDATES_MAX, READ_BUDGET, READ_BUDGET_MIN,
};

pub(crate) use align::mint_keys;

use crate::store::enc::{R, W};
use crate::store::StoreError;
use crate::view_index::{Bbox, EditableText, Flip, FontUse};
pub(crate) use draft::DraftFault;
pub(crate) use row::{
    build_row, dec_keys_row, dec_struct_row, enc_keys_row, enc_struct_row, keys_row_is_for,
    needs_mint, peek_row, view_of, KeysRow, ReadyRow, RowStatus, StructRow,
};

/// Longest text kept on a node, in bytes: a pack cuts longer text with [`cut_text`] and sets
/// the `text_truncated` flag.
pub const INDEX_TEXT_MAX: u32 = 64 << 10;
/// Deepest node chain (a root node has depth 0).
pub const INDEX_DEPTH_MAX: u16 = 64;
/// Longest [`IndexDraft::source`], in bytes.
pub const INDEX_SOURCE_MAX: usize = 64;
/// Longest node key, in bytes: a node's key can be a view region
/// ([`crate::session::BlobSession::set_region`]), so it is held to the same limit.
pub const INDEX_KEY_MAX: usize = crate::session::REGION_MAX_BYTES;
/// Longest node kind, in bytes.
pub const INDEX_KIND_MAX: usize = 64;
/// Longest node name, in bytes.
pub const INDEX_NAME_MAX: usize = 4096;
/// Most `attrs` (or `weak`) entries on one node, and most flags on one node.
pub const INDEX_ATTRS_MAX: usize = 256;
/// Longest attribute or flag name, in bytes.
pub const INDEX_ATTR_KEY_MAX: usize = 64;
/// Longest attribute value, in bytes.
pub const INDEX_ATTR_VALUE_MAX: usize = 4096;
/// Longest reason a refusal may carry, in bytes (it holds no file content).
pub const INDEX_WHY_MAX: usize = 256;
/// A pending index: how long a verb says to wait before retrying (`details.retry_after_ms`).
pub const INDEX_RETRY_AFTER_MS: u64 = 1_000;
/// The newest stored row format: 1 holds the nodes; 2 adds the view items ([`NodeView`], a
/// node's render digest, [`IndexDraft::fonts`]); 3 adds each node's own record digest
/// ([`DraftNode::own`]). A row is written as the lowest format that holds it: one with no own
/// digest as 2, one with no view item either as 1.
pub const INDEX_ROW_FORMAT: u8 = 3;
/// Most fonts a draft lists ([`IndexDraft::fonts`]), and most a node names
/// ([`NodeView::fonts`]).
pub const INDEX_FONTS_MAX: usize = 1024;

/// What a verb family is called for a file type; the kit registers them under the rights and
/// exits it defines ([`crate::parity::Registry::for_blob_pack`]), and
/// [`crate::session::BlobSession::run_tool`] runs them. The kit never contains the noun. Each
/// name is two lowercase words, the noun first (`part list`): the noun is the pack's, and
/// none may be a kit verb's first word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct IndexVerbs {
    pub list: &'static str,
    pub show: &'static str,
    pub diff: &'static str,
    pub blame: &'static str,
    /// `None`: the type has no per-node export.
    pub export: Option<&'static str>,
}

impl IndexVerbs {
    pub const fn new(
        list: &'static str,
        show: &'static str,
        diff: &'static str,
        blame: &'static str,
        export: Option<&'static str>,
    ) -> Self {
        Self {
            list,
            show,
            diff,
            blame,
            export,
        }
    }
}

/// A pack's index, identified: [`crate::session::BlobPack::index_spec`]. `Some` turns the
/// index on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct IndexSpec {
    /// Part of the row's key, so two packs can index the same bytes.
    pub pack: &'static str,
    /// Bump when the reader's output changes: older rows are ignored and recomputed (but a
    /// row of [`IndexSpec::older_format`]).
    pub format: u32,
    pub verbs: &'static IndexVerbs,
    /// An older format of this pack whose rows read as this one's with the view items absent:
    /// the same nodes, keys and fields ([`IndexSpec::with_older_format`]). `None`: only rows of
    /// `format` are read.
    pub older_format: Option<u32>,
    /// The pack gives its containers an own record digest ([`DraftNode::own`],
    /// [`IndexSpec::with_own_records`]): the kit then stores its rows and keys under the
    /// offset format ([`OWN_FORMAT_OFFSET`]).
    pub own_records: bool,
}

/// What the kit adds to a pack's `format` to store the rows of a spec with own records
/// ([`IndexSpec::with_own_records`]): a format no older kit asks for, whatever the pack's
/// own format is, so an older kit finds those versions pending and indexes them again, never
/// decoding a row of format 3. A pack's `format` stays below it.
pub const OWN_FORMAT_OFFSET: u32 = 1 << 24;

impl IndexSpec {
    pub const fn new(pack: &'static str, format: u32, verbs: &'static IndexVerbs) -> Self {
        Self {
            pack,
            format,
            verbs,
            older_format: None,
            own_records: false,
        }
    }

    /// **The pack gives own record digests** ([`DraftNode::own`]: a container whose record
    /// holds its children's). A draft with any from a spec that doesn't say so is refused
    /// (`MPC-E714`), nothing recorded. The kit picks where they are stored, so the pack needn't
    /// bump its format for them: rows and their minted keys go under `format` +
    /// [`OWN_FORMAT_OFFSET`], read in order: that format, then `format` itself (a row made
    /// before own records, or by an older kit), then [`IndexSpec::older_format`]. A version
    /// read from either of the last two has no own hash, and
    /// [`crate::session::BlobSession::index_job`] offers a job that indexes it again. An older
    /// kit never asks for the offset format: it finds such a version pending, indexes it under
    /// its own format, and its commit removes the offset row. Minted keys re-mint after such a
    /// round trip (a format's keys are its own), as after any format bump.
    pub const fn with_own_records(mut self) -> Self {
        self.own_records = true;
        self
    }

    /// The format the kit stores this spec's rows and keys under: `format`, or with own records
    /// `format` + [`OWN_FORMAT_OFFSET`] (the planted fault `own-offset-skipped`: `format`).
    pub(crate) fn stored_format(&self) -> u32 {
        let skipped = plant_on!(
            crate::differential::index_mutant()
                == Some(crate::differential::IndexMutant::OwnOffsetSkipped)
        );
        if self.own_records && !skipped {
            self.format.saturating_add(OWN_FORMAT_OFFSET)
        } else {
            self.format
        }
    }

    /// The formats read when [`IndexSpec::stored_format`] has no row, in order: with own
    /// records `format` (when it isn't the stored one) then the older format; else the older
    /// format.
    pub(crate) fn fallbacks(&self) -> Vec<u32> {
        let stored = self.stored_format();
        let mut out = Vec::with_capacity(2);
        if self.own_records && self.format != stored {
            out.push(self.format);
        }
        if let Some(o) = self.older() {
            out.push(o);
        }
        out
    }

    /// This spec read at the stored format `format` (one of [`IndexSpec::stored_format`] and
    /// [`IndexSpec::fallbacks`]): its rows and keys there, nothing more.
    pub(crate) fn at(&self, format: u32) -> IndexSpec {
        IndexSpec {
            format,
            older_format: None,
            own_records: false,
            ..*self
        }
    }

    /// This format's rows are format `older`'s plus the later items (the view items,
    /// [`NodeView`]), and a pack hands those only with this declared (`older` below `format`;
    /// a job refuses them otherwise, `MPC-E714`). A version whose only row is of format
    /// `older` reads from it, `Ready`, the items it lacks absent, with that format's minted
    /// keys, and
    /// [`crate::session::BlobSession::index_job`] offers a job that indexes it again (optional:
    /// until then it shows fewer details). An older kit, which reads only format `older`, never
    /// meets a row of format 2: it finds a version indexed only here pending, indexes it under
    /// `older`, and its commit removes this format's row.
    pub const fn with_older_format(mut self, older: u32) -> Self {
        self.older_format = Some(older);
        self
    }

    /// [`IndexSpec::older_format`] when it is below `format`.
    pub(crate) fn older(&self) -> Option<u32> {
        self.older_format.filter(|o| *o < self.format)
    }
}

/// The bounds on one index. A pack sees them on [`crate::session::BlobPack::index`]; the kit
/// enforces them on whatever comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct IndexLimits {
    pub max_nodes: u32,
    pub max_depth: u16,
    pub max_stored_bytes: u64,
    pub max_text_bytes: u32,
}

impl IndexLimits {
    pub const fn new(
        max_nodes: u32,
        max_depth: u16,
        max_stored_bytes: u64,
        max_text_bytes: u32,
    ) -> Self {
        Self {
            max_nodes,
            max_depth,
            max_stored_bytes,
            max_text_bytes,
        }
    }

    /// The largest result a worker may send ([`IndexOutcome::to_wire`]): the row cap plus
    /// room for the frame. Anything longer is refused unread.
    pub fn wire_max(&self) -> usize {
        usize::try_from(self.max_stored_bytes.saturating_add(1 << 20)).unwrap_or(usize::MAX)
    }
}

/// A node's address: its native ID (`@n:42`) or a minted key (`@k:r4.1`). See
/// [`crate::addr`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeKey(pub(crate) String);

impl NodeKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `s` if it is a node key: a scoped native ID, or a `@k:` key.
    pub fn parse(s: &str) -> Option<Self> {
        use crate::addr::{parse, Addr, Forms};
        if s.len() > INDEX_KEY_MAX {
            return None;
        }
        match parse(s, Forms::none().native()) {
            Ok(Addr::Native(_)) => Some(Self(s.to_owned())),
            Ok(Addr::Key(k)) if s == format!("@k:{k}") => Some(Self(s.to_owned())),
            _ => None,
        }
    }

    pub(crate) fn native_ok(s: &str) -> bool {
        use crate::addr::{parse, Addr, Forms};
        s.len() <= INDEX_KEY_MAX && matches!(parse(s, Forms::none().native()), Ok(Addr::Native(_)))
    }
}

impl fmt::Display for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a node's key comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyClass {
    /// The file's own ID, unique in this version.
    Native,
    /// A key the kit minted: the file gives this node no unique ID in this version. `weak`:
    /// it was carried from the version before by position alone (an anchor reads it as
    /// suspect).
    Minted { weak: bool },
}

/// A node's rectangle, in the file's own coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct NodeRect {
    pub left: i64,
    pub top: i64,
    pub right: i64,
    pub bottom: i64,
}

/// A node's box on the page, as a view draws it ([`NodeView::geom`]): composed through its
/// parents' transforms (a group's child), or the box it takes from elsewhere (`inherited`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewGeom {
    /// In the file's own coordinates, page space: the transform box, never the ink.
    pub bbox: Bbox,
    /// Clockwise about the box centre, in the view schema's `rot_unit`.
    pub rot: i64,
    pub flip: Flip,
    /// The box is taken from elsewhere (a placeholder's layout), not the node's own.
    pub inherited: bool,
    /// The box is approximate (a rotated group scaled unevenly).
    pub approx: bool,
}

/// What a view draws of one node that its plain fields don't say: an item of row format 2
/// ([`INDEX_ROW_FORMAT`]), `None` on a row of format 1 and on a node the pack gives none. On a
/// node that has one, the view takes these over what it reads from the plain fields
/// ([`crate::view_index::ViewIndex::blob`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct NodeView {
    /// Its box on the page; `None`: it has none.
    pub geom: Option<ViewGeom>,
    /// Whether an agent can set its text, and at what cost.
    pub editable_text: Option<EditableText>,
    /// The field kinds its text holds (`slidenum`); on a container, its units'. Words.
    pub fields: Vec<String>,
    /// The text a view shows (a table's rows, a slide number at its slide's position), at most
    /// [`IndexLimits::max_text_bytes`]; `text_truncated` when the pack cut it.
    pub text: Option<String>,
    pub text_truncated: bool,
    /// The fonts its text uses (a container's: its units'), as indexes into
    /// [`IndexDraft::fonts`].
    pub fonts: Vec<u32>,
}

impl NodeView {
    /// Nothing set: a pack fills the fields it has.
    pub fn new() -> Self {
        Self::default()
    }
}

/// A raw digest a pack computed: SHA-256 of a node's record or content. The kit keys it
/// before anything stores it; it never prints.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Digest([u8; 32]);

impl Digest {
    /// The digest of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(sha256(bytes))
    }

    /// A digest of data fed piece by piece (a node's record, a large content run).
    pub fn stream() -> DigestStream {
        DigestStream(Sha256::new())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub(crate) fn from_raw(b: [u8; 32]) -> Self {
        Self(b)
    }
}

/// [`Digest::stream`].
#[derive(Clone)]
pub struct DigestStream(Sha256);

impl DigestStream {
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub fn finish(self) -> Digest {
        Digest(self.0.finish())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Digest(<raw>)")
    }
}

/// A node hash as stored: keyed, so comparable only within one space.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeHash([u8; 32]);

impl NodeHash {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub(crate) fn from_raw(b: [u8; 32]) -> Self {
        Self(b)
    }

    /// Lowercase hex.
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }
}

impl fmt::Debug for NodeHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeHash({})", &self.to_hex()[..16])
    }
}

/// The public salt of the key that keys digests (HKDF-Extract's shape).
const DIGEST_KEY_DOMAIN: &[u8] = b"k2-mpc-base/blob_struct/digest/v1";

/// The key digests are keyed under, in space `space_key`: `HMAC(salt, space key)`, with the
/// space key as the **message**. A blob's id is `HMAC(space key, its bytes)`, which any
/// writer gets back for any bytes, so nothing keyed by the space key may be a secret here.
pub(crate) fn digest_key(space_key: &[u8; 32]) -> [u8; 32] {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::index_mutant()
        == Some(crate::differential::IndexMutant::DigestKeyInFileDomain)
    {
        let mut m = HmacSha256::new(space_key);
        m.update(DIGEST_KEY_DOMAIN);
        return m.finish();
    }
    let mut m = HmacSha256::new(DIGEST_KEY_DOMAIN);
    m.update(space_key);
    m.finish()
}

/// `digest`, keyed under [`digest_key`]'s output: the only form that is stored.
pub(crate) fn key_digest(key: &[u8; 32], digest: &Digest) -> NodeHash {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::index_mutant() == Some(crate::differential::IndexMutant::UnkeyedHash) {
        return NodeHash(*digest.as_bytes());
    }
    let mut m = HmacSha256::new(key);
    m.update(digest.as_bytes());
    NodeHash(m.finish())
}

/// Why an index job didn't record anything ([`crate::session::IndexJob`]). Nothing a job
/// reports here is stored, so a later job can retry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum IndexJobError {
    /// The version or its blob moved under the job (purged, deleted or rewritten): nothing
    /// was recorded. `MPC-E717` `index_stale`.
    #[error("the index job for version {version} is out of date: {reason}")]
    Stale {
        version: VersionId,
        reason: &'static str,
    },
    /// A worker's result, or a pack's draft, is refused: malformed, not trusted, nothing
    /// recorded. `MPC-E714` `index_bad_result`. Exit 1.
    #[error("the indexer's result was refused: {0}")]
    BadResult(String),
    /// The blob store failed, or the blob is damaged (`MPC-E705`): nothing recorded.
    #[error(transparent)]
    Blob(#[from] crate::blob::BlobError),
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
}

/// What identifies a node for an anchor: the node's kind, name and bounds, its keyed content
/// hash, and weak IDs the pack adds (`weak`). An anchor keeps the one of the version it was
/// made on, so a later version's node is judged against it alone
/// ([`crate::session::BlobPack::judge_anchor`]); a purge of that version takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Fingerprint {
    pub kind: String,
    pub name: String,
    pub bounds: Option<NodeRect>,
    /// [`IndexNode::content_hash`] then: keyed, comparable only within the doc's space.
    pub content: Option<NodeHash>,
    /// Weak IDs (a placed item's ID...): they may be shared by several nodes, so they only
    /// ever make a candidate, never an identity.
    pub weak: BTreeMap<String, String>,
}

/// One node, as read back: the draft's fields, the keyed hashes, and what the kit computed
/// (key, class, depth, order, visibility through its ancestors).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IndexNode {
    /// The canonical address.
    pub key: NodeKey,
    pub key_class: KeyClass,
    /// The file's own ID, kept even when it is duplicated in this version (then `key` is
    /// minted and the ID is ambiguous).
    pub native: Option<NodeKey>,
    pub kind: String,
    pub name: String,
    pub parent: Option<NodeKey>,
    /// Among siblings, top to bottom as a person sees it.
    pub order: u32,
    pub depth: u16,
    pub bounds: Option<NodeRect>,
    /// The node's own flag.
    pub visible: bool,
    /// Its own flag and every ancestor's.
    pub visible_in_tree: bool,
    /// Plain text, capped at [`INDEX_TEXT_MAX`]; never styling.
    pub text: Option<String>,
    pub attrs: BTreeMap<String, String>,
    pub flags: BTreeSet<String>,
    /// Keyed; the record without content data.
    pub record_hash: NodeHash,
    /// Keyed; the content data as stored. Never the data.
    pub content_hash: Option<NodeHash>,
    pub fingerprint: Fingerprint,
    /// The view items (row format 2); `None` on a row of format 1.
    pub view: Option<NodeView>,
    /// Keyed; what draws it (a container's render closure), from the pack's render digest
    /// (row format 2).
    pub render_hash: Option<NodeHash>,
    /// Keyed; its own record, its children's taken out ([`DraftNode::own`], row format 3);
    /// `None` on an older row and on a node the pack gives none.
    pub own_hash: Option<NodeHash>,
}

/// A version's index ([`crate::session::BlobSession::index_view`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IndexView {
    pub version: VersionId,
    /// The file's own format stamp (`"fmt-1"`).
    pub source: String,
    /// Doc-level attributes the pack gave.
    pub attrs: BTreeMap<String, String>,
    /// In reading order: a parent comes before its children.
    pub nodes: Vec<IndexNode>,
    /// The fonts the doc's text uses ([`IndexDraft::fonts`]; row format 2): empty on a row of
    /// format 1.
    pub fonts: Vec<crate::view_index::FontUse>,
}

/// Where a version's index stands ([`crate::session::BlobSession::index_status`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IndexStatus {
    /// The pack has no index ([`crate::session::BlobPack::index_spec`] is `None`).
    NoIndexer,
    Ready {
        version: VersionId,
        nodes: u32,
    },
    /// Not indexed yet; `running` when a job for these bytes is out.
    Pending {
        version: VersionId,
        running: bool,
    },
    NotIndexed {
        version: VersionId,
        why: NotIndexedWhy,
    },
}

/// Why a version has no index.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NotIndexedWhy {
    /// The blob is over [`crate::session::Config::blob_index_max_bytes`]. Not stored, so
    /// raising the limit works at once.
    TooLarge {
        size: u64,
        limit: u64,
    },
    TooManyNodes {
        count: u64,
        limit: u32,
    },
    TooDeep {
        depth: u32,
        limit: u16,
    },
    /// The index would be over [`crate::session::Config::blob_index_max_stored_bytes`].
    IndexTooBig {
        bytes: u64,
        limit: u64,
    },
    /// The pack can't walk this file's structure.
    Unreadable {
        why: String,
    },
    /// The pack doesn't index this file.
    Unsupported,
    /// The version was purged, or its space's blobs are gone.
    Purged,
}

impl fmt::Display for NotIndexedWhy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { size, limit } => {
                write!(f, "the file is {size} bytes, over the {limit}-byte limit")
            }
            Self::TooManyNodes { count, limit } => {
                write!(f, "{count} nodes, over the limit of {limit}")
            }
            Self::TooDeep { depth, limit } => {
                write!(f, "nodes {depth} deep, over the limit of {limit}")
            }
            Self::IndexTooBig { bytes, limit } => {
                write!(f, "an index of {bytes} bytes, over the limit of {limit}")
            }
            Self::Unreadable { why } => write!(f, "unreadable: {why}"),
            Self::Unsupported => f.write_str("this type can't index this file"),
            Self::Purged => f.write_str("the version was purged"),
        }
    }
}

// ----- the view items' encoding, shared by the wire and the stored row -----

const GEOM_FLIP_H: u8 = 1;
const GEOM_FLIP_V: u8 = 2;
const GEOM_INHERITED: u8 = 4;
const GEOM_APPROX: u8 = 8;

fn editable_code(e: Option<EditableText>) -> u8 {
    match e {
        None => 0,
        Some(EditableText::Plain) => 1,
        Some(EditableText::Formatted) => 2,
        Some(EditableText::Never) => 3,
    }
}

/// A node's view items, `None` as one zero byte.
pub(crate) fn put_view(w: &mut W, v: Option<&NodeView>) {
    let Some(v) = v else {
        w.u8(0);
        return;
    };
    w.u8(1);
    match &v.geom {
        None => {
            w.u8(0);
        }
        Some(g) => {
            let bit = |on: bool, b: u8| if on { b } else { 0 };
            let flags = bit(g.flip.h, GEOM_FLIP_H)
                | bit(g.flip.v, GEOM_FLIP_V)
                | bit(g.inherited, GEOM_INHERITED)
                | bit(g.approx, GEOM_APPROX);
            w.u8(1)
                .u64(g.bbox.x as u64)
                .u64(g.bbox.y as u64)
                .u64(g.bbox.w as u64)
                .u64(g.bbox.h as u64)
                .u64(g.rot as u64)
                .u8(flags);
        }
    }
    w.u8(editable_code(v.editable_text));
    w.strs(&v.fields);
    w.opt_str(v.text.as_deref()).u8(u8::from(v.text_truncated));
    w.u32(v.fonts.len() as u32);
    for f in &v.fonts {
        w.u32(*f);
    }
}

/// [`put_view`]'s bytes back: every count bounded, a text at most `text_max` bytes, a string's
/// and an entry's cost reported to `cost` as it is read. Anything else is `Corrupt`.
pub(crate) fn get_view_with(
    r: &mut R<'_>,
    text_max: usize,
    cost: &mut dyn FnMut(u64),
) -> Result<Option<NodeView>, StoreError> {
    const WHAT: &str = "index view items";
    let bad = || StoreError::Corrupt(WHAT);
    match r.u8()? {
        0 => return Ok(None),
        1 => {}
        _ => return Err(bad()),
    }
    let geom = match r.u8()? {
        0 => None,
        1 => {
            let (x, y, w, h, rot) = (
                r.u64()? as i64,
                r.u64()? as i64,
                r.u64()? as i64,
                r.u64()? as i64,
                r.u64()? as i64,
            );
            let flags = r.u8()?;
            if flags & !(GEOM_FLIP_H | GEOM_FLIP_V | GEOM_INHERITED | GEOM_APPROX) != 0 {
                return Err(bad());
            }
            Some(ViewGeom {
                bbox: Bbox { x, y, w, h },
                rot,
                flip: Flip {
                    h: flags & GEOM_FLIP_H != 0,
                    v: flags & GEOM_FLIP_V != 0,
                },
                inherited: flags & GEOM_INHERITED != 0,
                approx: flags & GEOM_APPROX != 0,
            })
        }
        _ => return Err(bad()),
    };
    let editable_text = match r.u8()? {
        0 => None,
        1 => Some(EditableText::Plain),
        2 => Some(EditableText::Formatted),
        3 => Some(EditableText::Never),
        _ => return Err(bad()),
    };
    let n = r.u32()? as usize;
    if n > INDEX_ATTRS_MAX {
        return Err(bad());
    }
    let mut fields = Vec::with_capacity(n);
    for _ in 0..n {
        let f = r.str_max(INDEX_ATTR_KEY_MAX)?;
        cost(f.len() as u64 + 96);
        fields.push(f);
    }
    let text = r.opt_str_max(text_max)?;
    if let Some(t) = &text {
        cost(t.len() as u64 + 32);
    }
    let text_truncated = match r.u8()? {
        0 => false,
        1 => true,
        _ => return Err(bad()),
    };
    let n = r.u32()? as usize;
    if n > INDEX_FONTS_MAX {
        return Err(bad());
    }
    let mut fonts = Vec::with_capacity(n);
    for _ in 0..n {
        fonts.push(r.u32()?);
    }
    cost(n as u64 * 4 + 128);
    Ok(Some(NodeView {
        geom,
        editable_text,
        fields,
        text,
        text_truncated,
        fonts,
    }))
}

/// The doc's fonts.
pub(crate) fn put_fonts(w: &mut W, fonts: &[FontUse]) {
    w.u32(fonts.len() as u32);
    for f in fonts {
        w.str(&f.family)
            .str(&f.script)
            .str(&f.from)
            .u8(u8::from(f.embedded));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::blob::{BlobConfig, BlobStore};
    use crate::store::DocStore;

    const V: IndexVerbs = IndexVerbs::new("l", "s", "d", "b", None);

    /// A spec with own records is stored under the kit's offset format and reads it, then its
    /// own format, then its older one; a spec without them under its format, then its older one.
    /// A spec read at a format is that format alone.
    #[test]
    fn own_records_are_stored_under_the_offset_format() {
        let own = IndexSpec::new("p", 2, &V)
            .with_older_format(1)
            .with_own_records();
        assert_eq!(
            (own.stored_format(), own.fallbacks()),
            (2 + OWN_FORMAT_OFFSET, vec![2, 1])
        );
        let plain = IndexSpec::new("p", 2, &V).with_older_format(1);
        assert_eq!((plain.stored_format(), plain.fallbacks()), (2, vec![1]));
        let at = own.at(2);
        assert_eq!(
            (at.stored_format(), at.fallbacks(), at.own_records),
            (2, Vec::new(), false)
        );
    }

    /// The digest key can't be read off a blob id: a file holding the derivation's domain
    /// string gets an id that isn't the key.
    #[test]
    fn a_file_holding_the_domain_string_does_not_yield_the_digest_key() {
        crate::differential::under_env_index_mutant(|| {
            let dir = std::env::temp_dir().join(format!("k2mb-digest-key-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let docs = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
            let blobs = BlobStore::open(
                docs.clone(),
                dir.join("blobs"),
                BlobConfig::default(),
                Arc::new(|| 0),
            )
            .unwrap();
            let info = blobs.put_bytes("ws", DIGEST_KEY_DOMAIN).unwrap();
            let key = docs.blob_space("ws").unwrap().unwrap().key;
            assert_ne!(
                *info.id.as_bytes(),
                digest_key(&key),
                "a file holding the domain string doesn't yield the digest key"
            );
            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}
