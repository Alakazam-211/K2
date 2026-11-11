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
    IndexBlame, IndexDiff, ListQuery, MissingAddress, NodeBlame, NodeChange, NodeChangeKind,
    NodeLabel, NodeRead, NodesRead, Outline, OutlineNode, BLAME_WALK_MAX, CANDIDATES_MAX,
    READ_BUDGET, READ_BUDGET_MIN,
};

pub(crate) use align::mint_keys;
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
    /// Bump when the reader's output changes: older rows are ignored and recomputed.
    pub format: u32,
    pub verbs: &'static IndexVerbs,
}

impl IndexSpec {
    pub const fn new(pack: &'static str, format: u32, verbs: &'static IndexVerbs) -> Self {
        Self {
            pack,
            format,
            verbs,
        }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::blob::{BlobConfig, BlobStore};
    use crate::store::DocStore;

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
