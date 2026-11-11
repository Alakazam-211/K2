//! What a pack hands back for one file ([`IndexDraft`]), the kit's check of it, and its wire
//! form between a sandboxed worker process and the kit ([`IndexOutcome`]). The kit treats a
//! draft as untrusted input: whatever produced it, it passes [`IndexDraft::shape`].

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read};

use super::{
    Digest, IndexJobError, IndexLimits, NodeKey, NodeRect, NotIndexedWhy, INDEX_ATTRS_MAX,
    INDEX_ATTR_KEY_MAX, INDEX_ATTR_VALUE_MAX, INDEX_KEY_MAX, INDEX_KIND_MAX, INDEX_NAME_MAX,
    INDEX_SOURCE_MAX, INDEX_WHY_MAX,
};
use crate::session::BlobPack;
use crate::store::enc::{R, W};

#[cfg(any(test, feature = "differential"))]
fn mutant(m: crate::differential::IndexMutant) -> bool {
    crate::differential::index_mutant() == Some(m)
}

/// One node of a [`IndexDraft`]. The kit derives a node's depth, sibling order and
/// visibility through its ancestors from the tree, so a pack can't disagree with itself.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DraftNode {
    /// The file's own ID (`@n:42`, in the address grammar), when it has one. `None`, or an
    /// ID another node also has, gets a key the kit mints.
    pub key: Option<String>,
    /// The pack's lower-case word.
    pub kind: String,
    pub name: String,
    /// The parent's index in [`IndexDraft::nodes`]: an earlier node, or `None` at the root.
    pub parent: Option<u32>,
    pub bounds: Option<NodeRect>,
    /// The node's own flag.
    pub visible: bool,
    /// Plain text, at most [`IndexLimits::max_text_bytes`] (see [`cut_text`]).
    pub text: Option<String>,
    pub attrs: BTreeMap<String, String>,
    /// Marks like `unreadable` or `text_truncated`, and the pack's own.
    pub flags: BTreeSet<String>,
    /// The raw digest of the record without content data ([`Digest`]).
    pub record: Digest,
    /// The raw digest of the content data as stored, when it has some.
    pub content: Option<Digest>,
    /// Weak IDs for anchors, beside kind, name and bounds.
    pub weak: BTreeMap<String, String>,
}

impl DraftNode {
    /// A visible root node with no text, attributes or content.
    pub fn new(kind: impl Into<String>, name: impl Into<String>, record: Digest) -> Self {
        Self {
            key: None,
            kind: kind.into(),
            name: name.into(),
            parent: None,
            bounds: None,
            visible: true,
            text: None,
            attrs: BTreeMap::new(),
            flags: BTreeSet::new(),
            record,
            content: None,
            weak: BTreeMap::new(),
        }
    }
}

/// One file's structure, as a pack read it (`docs/SESSIONS.md`, "The structure index").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct IndexDraft {
    /// The file's own format stamp (`"fmt-1"`).
    pub source: String,
    pub attrs: BTreeMap<String, String>,
    /// A parent before its children; siblings in reading order (top to bottom).
    pub nodes: Vec<DraftNode>,
}

impl IndexDraft {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            ..Self::default()
        }
    }
}

/// Why a pack didn't produce a draft ([`BlobPack::index`]).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IndexRefused {
    /// The pack doesn't index this file.
    #[error("this type can't index this file")]
    Unsupported,
    #[error("{count} nodes, over the limit of {limit}")]
    TooManyNodes { count: u64, limit: u32 },
    #[error("nodes {depth} deep, over the limit of {limit}")]
    TooDeep { depth: u32, limit: u16 },
    #[error("an index of {bytes} bytes, over the limit of {limit}")]
    TooBig { bytes: u64, limit: u64 },
    /// The file's structure can't be walked. `why` holds no file content
    /// ([`INDEX_WHY_MAX`] bytes are kept).
    #[error("unreadable: {why}")]
    Unreadable { why: String },
    /// The stream failed: a damaged blob or a broken pipe, not a bad file.
    #[error("reading the file: {0}")]
    Read(io::Error),
}

impl IndexRefused {
    /// The stored form of this refusal; `None` for [`IndexRefused::Read`], which stores nothing.
    pub(crate) fn why(&self) -> Option<NotIndexedWhy> {
        Some(match self {
            Self::Unsupported => NotIndexedWhy::Unsupported,
            Self::TooManyNodes { count, limit } => NotIndexedWhy::TooManyNodes {
                count: *count,
                limit: *limit,
            },
            Self::TooDeep { depth, limit } => NotIndexedWhy::TooDeep {
                depth: *depth,
                limit: *limit,
            },
            Self::TooBig { bytes, limit } => NotIndexedWhy::IndexTooBig {
                bytes: *bytes,
                limit: *limit,
            },
            Self::Unreadable { why } => NotIndexedWhy::Unreadable {
                why: why[..floor_char_boundary(why, INDEX_WHY_MAX)].to_owned(),
            },
            Self::Read(_) => return None,
        })
    }
}

/// What a draft's check found wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DraftFault {
    /// Over a cap: the file really is that big. Recorded as not indexed.
    Over(NotIndexedWhy),
    /// Not a draft the kit can accept: the indexer is wrong, not the file. Nothing is recorded.
    Malformed(String),
}

/// What the kit derives from a draft's tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shape {
    pub depth: Vec<u16>,
    pub order: Vec<u32>,
    pub tree_visible: Vec<bool>,
}

/// A word in the pack's vocabulary (a kind, a flag, an attribute name): lower-case ASCII,
/// digits, `_`, `-` or `.`.
fn word_ok(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.')
        })
}

fn check_map(what: &str, m: &BTreeMap<String, String>) -> Result<(), DraftFault> {
    if m.len() > INDEX_ATTRS_MAX {
        return Err(DraftFault::Malformed(format!(
            "{what}: {} entries, the limit is {INDEX_ATTRS_MAX}",
            m.len()
        )));
    }
    for (k, v) in m {
        if !word_ok(k, INDEX_ATTR_KEY_MAX) {
            return Err(DraftFault::Malformed(format!(
                "{what}: a bad attribute name"
            )));
        }
        if v.len() > INDEX_ATTR_VALUE_MAX {
            return Err(DraftFault::Malformed(format!(
                "{what}: a value over {INDEX_ATTR_VALUE_MAX} bytes"
            )));
        }
    }
    Ok(())
}

impl IndexDraft {
    /// Check the draft against `limits` and every field's cap, and derive its tree (depth,
    /// sibling order, visibility through ancestors).
    pub(crate) fn shape(&self, limits: &IndexLimits) -> Result<Shape, DraftFault> {
        #[cfg(any(test, feature = "differential"))]
        let no_node_cap = mutant(crate::differential::IndexMutant::NoNodeCap);
        #[cfg(not(any(test, feature = "differential")))]
        let no_node_cap = false;
        if !no_node_cap && self.nodes.len() as u64 > u64::from(limits.max_nodes) {
            return Err(DraftFault::Over(NotIndexedWhy::TooManyNodes {
                count: self.nodes.len() as u64,
                limit: limits.max_nodes,
            }));
        }
        if self.source.len() > INDEX_SOURCE_MAX {
            return Err(DraftFault::Malformed(format!(
                "a source stamp over {INDEX_SOURCE_MAX} bytes"
            )));
        }
        check_map("the doc attributes", &self.attrs)?;
        #[cfg(any(test, feature = "differential"))]
        let no_depth_cap = mutant(crate::differential::IndexMutant::NoDepthCap);
        #[cfg(not(any(test, feature = "differential")))]
        let no_depth_cap = false;
        let n = self.nodes.len();
        let mut shape = Shape {
            depth: Vec::with_capacity(n),
            order: Vec::with_capacity(n),
            tree_visible: Vec::with_capacity(n),
        };
        let mut next_child = vec![0u32; n];
        let mut next_root = 0u32;
        let bad = |i: usize, what: &str| DraftFault::Malformed(format!("node {i}: {what}"));
        for (i, node) in self.nodes.iter().enumerate() {
            if !word_ok(&node.kind, INDEX_KIND_MAX) {
                return Err(bad(i, "a bad kind"));
            }
            if node.name.len() > INDEX_NAME_MAX {
                return Err(bad(i, "a name over the limit"));
            }
            if let Some(k) = &node.key {
                if !NodeKey::native_ok(k) {
                    return Err(bad(i, "a key that isn't a native ID"));
                }
            }
            if let Some(t) = &node.text {
                if t.len() > limits.max_text_bytes as usize {
                    return Err(bad(i, "text over the limit"));
                }
            }
            check_map("a node's attributes", &node.attrs)?;
            check_map("a node's weak IDs", &node.weak)?;
            if node.flags.len() > INDEX_ATTRS_MAX
                || !node.flags.iter().all(|f| word_ok(f, INDEX_ATTR_KEY_MAX))
            {
                return Err(bad(i, "bad flags"));
            }
            let (depth, order, visible) = match node.parent {
                None => {
                    let o = next_root;
                    next_root += 1;
                    (0u16, o, node.visible)
                }
                Some(p) => {
                    let p = p as usize;
                    if p >= i {
                        return Err(bad(i, "its parent is not an earlier node"));
                    }
                    let d = u32::from(shape.depth[p]) + 1;
                    if !no_depth_cap && d > u32::from(limits.max_depth) {
                        return Err(DraftFault::Over(NotIndexedWhy::TooDeep {
                            depth: d,
                            limit: limits.max_depth,
                        }));
                    }
                    let o = next_child[p];
                    next_child[p] += 1;
                    (
                        d.min(u32::from(u16::MAX)) as u16,
                        o,
                        node.visible && shape.tree_visible[p],
                    )
                }
            };
            shape.depth.push(depth);
            shape.order.push(order);
            shape.tree_visible.push(visible);
        }
        Ok(shape)
    }
}

/// `text` cut to at most `max` bytes at a character boundary, and whether it was cut: what a
/// pack does before it sets the `text_truncated` flag.
pub fn cut_text(text: &str, max: u32) -> (&str, bool) {
    let at = floor_char_boundary(text, max as usize);
    (&text[..at], at < text.len())
}

fn floor_char_boundary(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut at = max;
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

// ----- the wire -----

const WIRE_MAGIC: [u8; 4] = *b"K2IX";
const WIRE_VERSION: u8 = 1;
const TAG_DRAFT: u8 = 0;
const TAG_REFUSED: u8 = 1;
const R_UNSUPPORTED: u8 = 0;
const R_TOO_MANY: u8 = 1;
const R_TOO_DEEP: u8 = 2;
const R_TOO_BIG: u8 = 3;
const R_UNREADABLE: u8 = 4;

/// What a worker reports for one file: [`IndexOutcome::to_wire`] on its side,
/// [`crate::session::IndexJob::commit_wire`] on the kit's. Refused or a draft: the kit checks
/// the draft ([`IndexDraft`]) either way.
#[derive(Debug)]
#[non_exhaustive]
pub enum IndexOutcome {
    Draft(IndexDraft),
    Refused(IndexRefused),
}

fn wf<T>(r: Result<T, crate::store::StoreError>) -> Result<T, IndexJobError> {
    r.map_err(|_| IndexJobError::BadResult("the result is truncated or malformed".to_owned()))
}

fn put_map(w: &mut W, m: &BTreeMap<String, String>) {
    w.u32(m.len() as u32);
    for (k, v) in m {
        w.str(k).str(v);
    }
}

/// Where a decode stopped: malformed, or over its allocation budget.
enum Stop {
    Bad(IndexJobError),
    Over,
}

impl From<IndexJobError> for Stop {
    fn from(e: IndexJobError) -> Self {
        Self::Bad(e)
    }
}

/// What a decode has left to allocate, in estimated bytes: a frame within the wire cap can't
/// decode to more than this.
struct Budget(u64);

/// What a decoded string costs beyond its bytes (its `String` and the allocator's rounding).
const COST_STRING: u64 = 32;
/// What one map or set entry costs beyond its strings.
const COST_ENTRY: u64 = 64;

impl Budget {
    fn charge(&mut self, n: u64) -> Result<(), Stop> {
        self.0 = self.0.checked_sub(n).ok_or(Stop::Over)?;
        Ok(())
    }

    fn string(&mut self, s: &str) -> Result<(), Stop> {
        self.charge(s.len() as u64 + COST_STRING)
    }
}

fn get_map(r: &mut R<'_>, budget: &mut Budget) -> Result<BTreeMap<String, String>, Stop> {
    let n = wf(r.u32())? as usize;
    if n > INDEX_ATTRS_MAX {
        return Err(
            IndexJobError::BadResult("an attribute map is over the limit".to_owned()).into(),
        );
    }
    let mut out = BTreeMap::new();
    for _ in 0..n {
        let k = wf(r.str_max(INDEX_ATTR_KEY_MAX))?;
        let v = wf(r.str_max(INDEX_ATTR_VALUE_MAX))?;
        budget.string(&k)?;
        budget.string(&v)?;
        budget.charge(COST_ENTRY)?;
        out.insert(k, v);
    }
    Ok(out)
}

fn put_node(w: &mut W, n: &DraftNode) {
    w.opt_str(n.key.as_deref())
        .str(&n.kind)
        .str(&n.name)
        .u32(n.parent.map_or(0, |p| p + 1));
    match n.bounds {
        None => {
            w.u8(0);
        }
        Some(b) => {
            w.u8(1)
                .u64(b.left as u64)
                .u64(b.top as u64)
                .u64(b.right as u64)
                .u64(b.bottom as u64);
        }
    }
    w.u8(u8::from(n.visible));
    w.opt_str(n.text.as_deref());
    put_map(w, &n.attrs);
    w.u32(n.flags.len() as u32);
    for f in &n.flags {
        w.str(f);
    }
    w.bytes(n.record.as_bytes());
    w.opt_bytes(n.content.as_ref().map(|d| d.as_bytes().as_slice()));
    put_map(w, &n.weak);
}

fn get_digest(b: Vec<u8>) -> Result<Digest, IndexJobError> {
    let b: [u8; 32] = b
        .try_into()
        .map_err(|_| IndexJobError::BadResult("a digest isn't 32 bytes".to_owned()))?;
    Ok(Digest::from_raw(b))
}

fn get_node(r: &mut R<'_>, limits: &IndexLimits, budget: &mut Budget) -> Result<DraftNode, Stop> {
    let key = wf(r.opt_str_max(INDEX_KEY_MAX))?;
    let kind = wf(r.str_max(INDEX_KIND_MAX))?;
    let name = wf(r.str_max(INDEX_NAME_MAX))?;
    budget.charge(std::mem::size_of::<DraftNode>() as u64)?;
    budget.string(&kind)?;
    budget.string(&name)?;
    if let Some(k) = &key {
        budget.string(k)?;
    }
    let parent = match wf(r.u32())? {
        0 => None,
        p => Some(p - 1),
    };
    let bounds = match wf(r.u8())? {
        0 => None,
        1 => Some(NodeRect {
            left: wf(r.u64())? as i64,
            top: wf(r.u64())? as i64,
            right: wf(r.u64())? as i64,
            bottom: wf(r.u64())? as i64,
        }),
        _ => return Err(IndexJobError::BadResult("a bad bounds marker".to_owned()).into()),
    };
    let visible = match wf(r.u8())? {
        0 => false,
        1 => true,
        _ => return Err(IndexJobError::BadResult("a bad visible flag".to_owned()).into()),
    };
    let text = wf(r.opt_str_max(limits.max_text_bytes as usize))?;
    if let Some(t) = &text {
        budget.string(t)?;
    }
    let attrs = get_map(r, budget)?;
    let flag_count = wf(r.u32())? as usize;
    if flag_count > INDEX_ATTRS_MAX {
        return Err(IndexJobError::BadResult("too many flags on a node".to_owned()).into());
    }
    let mut flags = BTreeSet::new();
    for _ in 0..flag_count {
        let f = wf(r.str_max(INDEX_ATTR_KEY_MAX))?;
        budget.string(&f)?;
        budget.charge(COST_ENTRY)?;
        flags.insert(f);
    }
    let record = get_digest(wf(r.bytes())?)?;
    let content = wf(r.opt_bytes())?.map(get_digest).transpose()?;
    let weak = get_map(r, budget)?;
    Ok(DraftNode {
        key,
        kind,
        name,
        parent,
        bounds,
        visible,
        text,
        attrs,
        flags,
        record,
        content,
        weak,
    })
}

impl IndexOutcome {
    /// The result as a worker sends it. A draft over a cap, or too big for the wire, goes as
    /// the matching refusal; a draft the kit would refuse (its pack is wrong) is an error
    /// here, on the worker's side.
    pub fn to_wire(&self, limits: &IndexLimits) -> Result<Vec<u8>, IndexJobError> {
        let draft = match self {
            IndexOutcome::Refused(r) => return encode_refused(r),
            IndexOutcome::Draft(d) => d,
        };
        match draft.shape(limits) {
            Ok(_) => {}
            Err(DraftFault::Malformed(m)) => return Err(IndexJobError::BadResult(m)),
            Err(DraftFault::Over(w)) => return encode_refused(&refused_of(&w)),
        }
        let mut w = W::default();
        w.0.extend_from_slice(&WIRE_MAGIC);
        w.u8(WIRE_VERSION).u8(TAG_DRAFT);
        w.str(&draft.source);
        put_map(&mut w, &draft.attrs);
        w.u32(draft.nodes.len() as u32);
        for n in &draft.nodes {
            put_node(&mut w, n);
        }
        if w.0.len() > limits.wire_max() {
            return encode_refused(&IndexRefused::TooBig {
                bytes: w.0.len() as u64,
                limit: limits.max_stored_bytes,
            });
        }
        Ok(w.0)
    }

    /// A worker's result, checked: bounds on the wire (a frame over [`IndexLimits::wire_max`]
    /// refused unread, and a node count over the cap returns the refusal at once, with
    /// nothing read), UTF-8, lengths, and flags. The tree is checked after, as for any draft.
    pub fn from_wire(b: &[u8], limits: &IndexLimits) -> Result<Self, IndexJobError> {
        if b.len() > limits.wire_max() {
            return Err(IndexJobError::BadResult(format!(
                "a result of {} bytes is over the {}-byte limit",
                b.len(),
                limits.wire_max()
            )));
        }
        let mut r = R::new(b, "index result");
        for want in WIRE_MAGIC {
            if wf(r.u8())? != want {
                return Err(IndexJobError::BadResult("not an indexer result".to_owned()));
            }
        }
        if wf(r.u8())? != WIRE_VERSION {
            return Err(IndexJobError::BadResult(
                "an indexer result of another version".to_owned(),
            ));
        }
        let outcome = match wf(r.u8())? {
            TAG_REFUSED => IndexOutcome::Refused(decode_refused(&mut r)?),
            TAG_DRAFT => {
                let mut budget = Budget(decode_budget(limits));
                match decode_draft(&mut r, limits, b.len(), &mut budget) {
                    // a node count over the cap stops the read: nothing else is looked at
                    Ok(out @ IndexOutcome::Refused(_)) => return Ok(out),
                    Ok(out) => out,
                    Err(Stop::Bad(e)) => return Err(e),
                    Err(Stop::Over) => {
                        return Ok(IndexOutcome::Refused(IndexRefused::TooBig {
                            bytes: b.len() as u64,
                            limit: limits.max_stored_bytes,
                        }))
                    }
                }
            }
            _ => {
                return Err(IndexJobError::BadResult(
                    "an unknown result kind".to_owned(),
                ))
            }
        };
        wf(r.end())?;
        Ok(outcome)
    }
}

/// The shortest a node can be on the wire: lengths, flags and its record digest.
const MIN_WIRE_NODE: usize = 65;

/// What decoding one result may allocate: a multiple of the frame it fits in.
fn decode_budget(limits: &IndexLimits) -> u64 {
    #[cfg(any(test, feature = "differential"))]
    if mutant(crate::differential::IndexMutant::NoDecodeBudget) {
        return u64::MAX;
    }
    (limits.wire_max() as u64).saturating_mul(4)
}

fn decode_draft(
    r: &mut R<'_>,
    limits: &IndexLimits,
    wire_len: usize,
    budget: &mut Budget,
) -> Result<IndexOutcome, Stop> {
    let source = wf(r.str_max(INDEX_SOURCE_MAX))?;
    budget.string(&source)?;
    let attrs = get_map(r, budget)?;
    let count = wf(r.u32())?;
    #[cfg(any(test, feature = "differential"))]
    let no_node_cap = mutant(crate::differential::IndexMutant::NoNodeCap);
    #[cfg(not(any(test, feature = "differential")))]
    let no_node_cap = false;
    if !no_node_cap && count > limits.max_nodes {
        return Ok(IndexOutcome::Refused(IndexRefused::TooManyNodes {
            count: u64::from(count),
            limit: limits.max_nodes,
        }));
    }
    let mut nodes = Vec::with_capacity((count as usize).min(wire_len / MIN_WIRE_NODE));
    for _ in 0..count {
        nodes.push(get_node(r, limits, budget)?);
    }
    Ok(IndexOutcome::Draft(IndexDraft {
        source,
        attrs,
        nodes,
    }))
}

fn refused_of(w: &NotIndexedWhy) -> IndexRefused {
    match w {
        NotIndexedWhy::TooManyNodes { count, limit } => IndexRefused::TooManyNodes {
            count: *count,
            limit: *limit,
        },
        NotIndexedWhy::TooDeep { depth, limit } => IndexRefused::TooDeep {
            depth: *depth,
            limit: *limit,
        },
        NotIndexedWhy::IndexTooBig { bytes, limit } => IndexRefused::TooBig {
            bytes: *bytes,
            limit: *limit,
        },
        NotIndexedWhy::Unreadable { why } => IndexRefused::Unreadable { why: why.clone() },
        // not produced by a draft's check
        NotIndexedWhy::Unsupported | NotIndexedWhy::TooLarge { .. } | NotIndexedWhy::Purged => {
            IndexRefused::Unsupported
        }
    }
}

fn encode_refused(r: &IndexRefused) -> Result<Vec<u8>, IndexJobError> {
    let mut w = W::default();
    w.0.extend_from_slice(&WIRE_MAGIC);
    w.u8(WIRE_VERSION).u8(TAG_REFUSED);
    match r {
        IndexRefused::Unsupported => {
            w.u8(R_UNSUPPORTED);
        }
        IndexRefused::TooManyNodes { count, limit } => {
            w.u8(R_TOO_MANY).u64(*count).u32(*limit);
        }
        IndexRefused::TooDeep { depth, limit } => {
            w.u8(R_TOO_DEEP).u32(*depth).u32(u32::from(*limit));
        }
        IndexRefused::TooBig { bytes, limit } => {
            w.u8(R_TOO_BIG).u64(*bytes).u64(*limit);
        }
        IndexRefused::Unreadable { why } => {
            let keep = &why[..floor_char_boundary(why, INDEX_WHY_MAX)];
            w.u8(R_UNREADABLE).str(keep);
        }
        IndexRefused::Read(e) => {
            return Err(IndexJobError::BadResult(format!(
                "the worker couldn't read its input: {e}"
            )))
        }
    }
    Ok(w.0)
}

fn decode_refused(r: &mut R<'_>) -> Result<IndexRefused, IndexJobError> {
    Ok(match wf(r.u8())? {
        R_UNSUPPORTED => IndexRefused::Unsupported,
        R_TOO_MANY => IndexRefused::TooManyNodes {
            count: wf(r.u64())?,
            limit: wf(r.u32())?,
        },
        R_TOO_DEEP => IndexRefused::TooDeep {
            depth: wf(r.u32())?,
            limit: wf(r.u32())?.min(u32::from(u16::MAX)) as u16,
        },
        R_TOO_BIG => IndexRefused::TooBig {
            bytes: wf(r.u64())?,
            limit: wf(r.u64())?,
        },
        R_UNREADABLE => {
            let why = wf(r.str())?;
            if why.len() > INDEX_WHY_MAX {
                return Err(IndexJobError::BadResult(
                    "a refusal's reason is over the limit".to_owned(),
                ));
            }
            IndexRefused::Unreadable { why }
        }
        _ => return Err(IndexJobError::BadResult("an unknown refusal".to_owned())),
    })
}

/// What a sandboxed worker runs: `pack` reads one file (forward only; it may stop early) under
/// `limits`, and the result is put on the wire for [`crate::session::IndexJob::commit_wire`].
pub fn index_to_wire(
    pack: &dyn BlobPack,
    src: &mut dyn Read,
    size: u64,
    limits: &IndexLimits,
) -> Result<Vec<u8>, IndexJobError> {
    let outcome = match pack.index(src, size, limits) {
        Ok(d) => IndexOutcome::Draft(d),
        Err(r) => IndexOutcome::Refused(r),
    };
    outcome.to_wire(limits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> IndexLimits {
        IndexLimits::new(4, 2, 1 << 20, 16)
    }

    fn node(parent: Option<u32>) -> DraftNode {
        let mut n = DraftNode::new("kind", "name", Digest::of(b"record"));
        n.parent = parent;
        n
    }

    /// A draft on the wire with no checks, as a rogue worker could send one.
    fn raw_wire(d: &IndexDraft, count: u32) -> Vec<u8> {
        let mut w = W::default();
        w.0.extend_from_slice(&WIRE_MAGIC);
        w.u8(WIRE_VERSION).u8(TAG_DRAFT);
        w.str(&d.source);
        put_map(&mut w, &d.attrs);
        w.u32(count);
        for n in &d.nodes {
            put_node(&mut w, n);
        }
        w.0
    }

    fn decode(b: &[u8]) -> Result<IndexDraft, DraftFault> {
        match IndexOutcome::from_wire(b, &limits()) {
            Ok(IndexOutcome::Draft(d)) => {
                d.shape(&limits())?;
                Ok(d)
            }
            Ok(IndexOutcome::Refused(r)) => {
                Err(DraftFault::Over(r.why().expect("a recorded refusal")))
            }
            Err(e) => Err(DraftFault::Malformed(e.to_string())),
        }
    }

    #[test]
    fn a_draft_round_trips_on_the_wire() {
        let mut d = IndexDraft::new("fmt-1");
        d.attrs.insert("seed".into(), "7".into());
        let mut n = node(None);
        n.key = Some("@n:1".into());
        n.bounds = Some(NodeRect {
            left: -1,
            top: 2,
            right: 3,
            bottom: 4,
        });
        n.text = Some("hi".into());
        n.flags.insert("styles".into());
        n.attrs.insert("opacity".into(), "50".into());
        n.weak.insert("placed".into(), "x".into());
        n.content = Some(Digest::of(b"content"));
        d.nodes.push(n);
        d.nodes.push(node(Some(0)));
        let wire = IndexOutcome::Draft(d.clone()).to_wire(&limits()).unwrap();
        match IndexOutcome::from_wire(&wire, &limits()).unwrap() {
            IndexOutcome::Draft(back) => assert_eq!(back, d),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_parent_not_before_its_child_is_refused_as_malformed() {
        for parent in [0, 1, 7] {
            let mut d = IndexDraft::new("f");
            d.nodes.push(node(Some(parent)));
            assert!(
                matches!(decode(&raw_wire(&d, 1)), Err(DraftFault::Malformed(_))),
                "parent {parent}"
            );
        }
    }

    #[test]
    fn caps_on_the_wire_are_refusals_or_malformed_as_they_should_be() {
        let mut d = IndexDraft::new("f");
        for i in 0..5 {
            d.nodes.push(node(if i == 0 { None } else { Some(i - 1) }));
        }
        // five nodes, the cap is four: a refusal, nothing else read
        assert!(matches!(
            decode(&raw_wire(&d, 5)),
            Err(DraftFault::Over(NotIndexedWhy::TooManyNodes {
                count: 5,
                limit: 4
            }))
        ));
        // a chain of four, depth cap 2
        let chain = IndexDraft {
            nodes: d.nodes[..4].to_vec(),
            ..IndexDraft::new("f")
        };
        assert!(matches!(
            decode(&raw_wire(&chain, 4)),
            Err(DraftFault::Over(NotIndexedWhy::TooDeep { .. }))
        ));
        // text over its limit
        let mut long = IndexDraft::new("f");
        let mut n = node(None);
        n.text = Some("x".repeat(17));
        long.nodes.push(n);
        assert!(matches!(
            decode(&raw_wire(&long, 1)),
            Err(DraftFault::Malformed(_))
        ));
        // a lying count: more nodes claimed than sent, and over any read
        assert!(matches!(
            decode(&raw_wire(&IndexDraft::new("f"), 3)),
            Err(DraftFault::Malformed(_))
        ));
    }

    #[test]
    fn a_key_outside_the_native_grammar_is_refused() {
        for bad in ["@k:r1", "plain", "@1bad:7", ""] {
            let mut d = IndexDraft::new("f");
            let mut n = node(None);
            n.key = Some(bad.to_owned());
            d.nodes.push(n);
            assert!(
                matches!(decode(&raw_wire(&d, 1)), Err(DraftFault::Malformed(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_digest_on_the_wire_must_be_32_bytes() {
        // a truncated digest length on the wire is refused, not read past the end
        let mut w = W::default();
        w.0.extend_from_slice(&WIRE_MAGIC);
        w.u8(WIRE_VERSION).u8(TAG_DRAFT).str("f");
        put_map(&mut w, &BTreeMap::new());
        w.u32(1);
        w.opt_str(None).str("kind").str("n").u32(0).u8(0).u8(1);
        w.opt_str(None);
        put_map(&mut w, &BTreeMap::new());
        w.u32(0).bytes(&[1, 2, 3]);
        assert!(matches!(
            IndexOutcome::from_wire(&w.0, &limits()),
            Err(IndexJobError::BadResult(_))
        ));
    }

    /// The wire is untrusted: a flipped byte or a cut anywhere gives a typed refusal or a
    /// draft that its shape then checks; the decode always returns.
    #[test]
    fn a_flipped_or_cut_wire_is_refused_or_checked() {
        let mut d = IndexDraft::new("fmt-1");
        d.nodes.push(node(None));
        let mut n = node(Some(0));
        n.key = Some("@n:9".into());
        n.text = Some("text".into());
        n.attrs.insert("a".into(), "v".into());
        n.flags.insert("flag".into());
        n.weak.insert("w".into(), "x".into());
        d.nodes.push(n);
        let wire = IndexOutcome::Draft(d).to_wire(&limits()).unwrap();
        let mut rng = 0x9E37_79B9_u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for _ in 0..4096 {
            let mut b = wire.clone();
            for _ in 0..(1 + next() % 4) {
                let at = (next() as usize) % b.len();
                b[at] = (next() & 0xff) as u8;
            }
            if next() % 3 == 0 {
                b.truncate((next() as usize) % (b.len() + 1));
            }
            if let Ok(IndexOutcome::Draft(d)) = IndexOutcome::from_wire(&b, &limits()) {
                let _ = d.shape(&limits());
            }
        }
    }

    /// A wire that fits the frame but decodes to far more memory stops at its budget, as a
    /// refusal, not an allocation.
    #[test]
    fn a_hostile_wire_stops_at_its_decode_budget() {
        crate::differential::under_env_index_mutant(|| {
            let lim = IndexLimits::new(100_000, 8, 0, 16);
            let mut d = IndexDraft::new("f");
            for _ in 0..13_000 {
                d.nodes.push(DraftNode::new("k", "n", Digest::of(b"r")));
            }
            let wire = raw_wire(&d, 13_000);
            assert!(wire.len() <= lim.wire_max(), "the frame fits");
            let out = IndexOutcome::from_wire(&wire, &lim);
            assert!(
                matches!(out, Ok(IndexOutcome::Refused(IndexRefused::TooBig { .. }))),
                "the decode stops at its budget: {out:?}"
            );
        });
    }

    #[test]
    fn text_is_cut_at_a_character_boundary() {
        assert_eq!(cut_text("short", 16), ("short", false));
        assert_eq!(
            cut_text("héllo", 2),
            ("h", true),
            "not inside the two-byte letter"
        );
    }
}
