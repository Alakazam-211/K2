//! A version's index as stored: the content-keyed row (a pure function of the bytes, with
//! digests keyed) and the per-doc key overlay (minted keys), and the join of the two that
//! readers see.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::draft::{IndexDraft, Shape};
use super::{
    digest_key, key_digest, Fingerprint, IndexNode, IndexView, KeyClass, NodeHash, NodeKey,
    NodeRect, NotIndexedWhy,
};
use crate::store::enc::{R, W};
use crate::store::{StoreError, VersionId};

const ROW_VERSION: u8 = 1;
const TAG_READY: u8 = 0;
const TAG_NOT_INDEXED: u8 = 1;
const KEYS_VERSION: u8 = 1;

const WHY_TOO_LARGE: u8 = 0;
const WHY_TOO_MANY: u8 = 1;
const WHY_TOO_DEEP: u8 = 2;
const WHY_TOO_BIG: u8 = 3;
const WHY_UNREADABLE: u8 = 4;
const WHY_UNSUPPORTED: u8 = 5;
const WHY_PURGED: u8 = 6;

const WHAT: &str = "blob_struct row";
/// The shortest a stored node can be: lengths, flags and its record hash.
const MIN_NODE_BYTES: usize = 74;
/// At most this many nodes are reserved up front when decoding a row.
const RESERVE_NODES: usize = 4096;
const WHAT_KEYS: &str = "blob_struct keys";

/// One node of a [`ReadyRow`]: the draft's fields and what the kit derived, without its key
/// (the overlay's, for a node with no unique native ID).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowNode {
    pub native: Option<String>,
    pub kind: String,
    pub name: String,
    pub parent: Option<u32>,
    pub order: u32,
    pub depth: u16,
    pub bounds: Option<NodeRect>,
    pub visible: bool,
    pub visible_in_tree: bool,
    pub text: Option<String>,
    pub attrs: BTreeMap<String, String>,
    pub flags: BTreeSet<String>,
    pub record_hash: NodeHash,
    pub content_hash: Option<NodeHash>,
    pub weak: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadyRow {
    pub source: String,
    pub attrs: BTreeMap<String, String>,
    pub nodes: Vec<RowNode>,
}

/// One `blob_struct` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StructRow {
    Ready(ReadyRow),
    NotIndexed(NotIndexedWhy),
}

/// What [`peek_row`] reads: a row's status, without decoding its nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowStatus {
    Ready { nodes: u32 },
    NotIndexed(NotIndexedWhy),
}

/// `draft` as a stored row, its digests keyed under the space key `space_key`.
pub(crate) fn build_row(draft: &IndexDraft, shape: &Shape, space_key: &[u8; 32]) -> ReadyRow {
    let key = digest_key(space_key);
    let nodes = draft
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| RowNode {
            native: n.key.clone(),
            kind: n.kind.clone(),
            name: n.name.clone(),
            parent: n.parent,
            order: shape.order[i],
            depth: shape.depth[i],
            bounds: n.bounds,
            visible: n.visible,
            visible_in_tree: shape.tree_visible[i],
            text: n.text.clone(),
            attrs: n.attrs.clone(),
            flags: n.flags.clone(),
            record_hash: key_digest(&key, &n.record),
            content_hash: n.content.as_ref().map(|d| key_digest(&key, d)),
            weak: n.weak.clone(),
        })
        .collect();
    ReadyRow {
        source: draft.source.clone(),
        attrs: draft.attrs.clone(),
        nodes,
    }
}

fn put_map(w: &mut W, m: &BTreeMap<String, String>) {
    w.u32(m.len() as u32);
    for (k, v) in m {
        w.str(k).str(v);
    }
}

fn get_map(r: &mut R<'_>) -> Result<BTreeMap<String, String>, StoreError> {
    let n = r.u32()? as usize;
    let mut out = BTreeMap::new();
    for _ in 0..n {
        let k = r.str()?;
        let v = r.str()?;
        out.insert(k, v);
    }
    Ok(out)
}

fn put_why(w: &mut W, why: &NotIndexedWhy) {
    match why {
        NotIndexedWhy::TooLarge { size, limit } => {
            w.u8(WHY_TOO_LARGE).u64(*size).u64(*limit);
        }
        NotIndexedWhy::TooManyNodes { count, limit } => {
            w.u8(WHY_TOO_MANY).u64(*count).u32(*limit);
        }
        NotIndexedWhy::TooDeep { depth, limit } => {
            w.u8(WHY_TOO_DEEP).u32(*depth).u32(u32::from(*limit));
        }
        NotIndexedWhy::IndexTooBig { bytes, limit } => {
            w.u8(WHY_TOO_BIG).u64(*bytes).u64(*limit);
        }
        NotIndexedWhy::Unreadable { why } => {
            w.u8(WHY_UNREADABLE).str(why);
        }
        NotIndexedWhy::Unsupported => {
            w.u8(WHY_UNSUPPORTED);
        }
        NotIndexedWhy::Purged => {
            w.u8(WHY_PURGED);
        }
    }
}

fn get_why(r: &mut R<'_>) -> Result<NotIndexedWhy, StoreError> {
    Ok(match r.u8()? {
        WHY_TOO_LARGE => NotIndexedWhy::TooLarge {
            size: r.u64()?,
            limit: r.u64()?,
        },
        WHY_TOO_MANY => NotIndexedWhy::TooManyNodes {
            count: r.u64()?,
            limit: r.u32()?,
        },
        WHY_TOO_DEEP => NotIndexedWhy::TooDeep {
            depth: r.u32()?,
            limit: r.u32()? as u16,
        },
        WHY_TOO_BIG => NotIndexedWhy::IndexTooBig {
            bytes: r.u64()?,
            limit: r.u64()?,
        },
        WHY_UNREADABLE => NotIndexedWhy::Unreadable { why: r.str()? },
        WHY_UNSUPPORTED => NotIndexedWhy::Unsupported,
        WHY_PURGED => NotIndexedWhy::Purged,
        _ => return Err(StoreError::Corrupt(WHAT)),
    })
}

fn put_hash(w: &mut W, h: &NodeHash) {
    w.bytes(h.as_bytes());
}

fn get_hash(b: Vec<u8>) -> Result<NodeHash, StoreError> {
    let b: [u8; 32] = b.try_into().map_err(|_| StoreError::Corrupt(WHAT))?;
    Ok(NodeHash::from_raw(b))
}

fn put_node(w: &mut W, n: &RowNode) {
    w.opt_str(n.native.as_deref())
        .str(&n.kind)
        .str(&n.name)
        .u32(n.parent.map_or(0, |p| p + 1))
        .u32(n.order)
        .u32(u32::from(n.depth));
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
    w.u8(u8::from(n.visible)).u8(u8::from(n.visible_in_tree));
    w.opt_str(n.text.as_deref());
    put_map(w, &n.attrs);
    w.u32(n.flags.len() as u32);
    for f in &n.flags {
        w.str(f);
    }
    put_hash(w, &n.record_hash);
    match &n.content_hash {
        None => {
            w.u8(0);
        }
        Some(h) => {
            w.u8(1);
            put_hash(w, h);
        }
    }
    put_map(w, &n.weak);
}

fn get_node(r: &mut R<'_>) -> Result<RowNode, StoreError> {
    let native = r.opt_str()?;
    let kind = r.str()?;
    let name = r.str()?;
    let parent = match r.u32()? {
        0 => None,
        p => Some(p - 1),
    };
    let order = r.u32()?;
    let depth = u16::try_from(r.u32()?).map_err(|_| StoreError::Corrupt(WHAT))?;
    let bounds = match r.u8()? {
        0 => None,
        1 => Some(NodeRect {
            left: r.u64()? as i64,
            top: r.u64()? as i64,
            right: r.u64()? as i64,
            bottom: r.u64()? as i64,
        }),
        _ => return Err(StoreError::Corrupt(WHAT)),
    };
    let visible = r.u8()? != 0;
    let visible_in_tree = r.u8()? != 0;
    let text = r.opt_str()?;
    let attrs = get_map(r)?;
    let n = r.u32()? as usize;
    if n > (1 << 20) {
        return Err(StoreError::Corrupt(WHAT));
    }
    let mut flags = BTreeSet::new();
    for _ in 0..n {
        flags.insert(r.str()?);
    }
    let record_hash = get_hash(r.bytes()?)?;
    let content_hash = match r.u8()? {
        0 => None,
        1 => Some(get_hash(r.bytes()?)?),
        _ => return Err(StoreError::Corrupt(WHAT)),
    };
    let weak = get_map(r)?;
    Ok(RowNode {
        native,
        kind,
        name,
        parent,
        order,
        depth,
        bounds,
        visible,
        visible_in_tree,
        text,
        attrs,
        flags,
        record_hash,
        content_hash,
        weak,
    })
}

/// A row's stored bytes. The node count sits at a fixed place, for [`peek_row`].
pub(crate) fn enc_struct_row(row: &StructRow) -> Vec<u8> {
    let mut w = W::default();
    w.u8(ROW_VERSION);
    match row {
        StructRow::Ready(r) => {
            w.u8(TAG_READY).u32(r.nodes.len() as u32);
            w.str(&r.source);
            put_map(&mut w, &r.attrs);
            for n in &r.nodes {
                put_node(&mut w, n);
            }
        }
        StructRow::NotIndexed(why) => {
            w.u8(TAG_NOT_INDEXED);
            put_why(&mut w, why);
        }
    }
    w.0
}

/// A row's status from its stored bytes, without decoding its nodes.
pub(crate) fn peek_row(b: &[u8]) -> Result<RowStatus, StoreError> {
    let mut r = R::new(b, WHAT);
    if r.u8()? != ROW_VERSION {
        return Err(StoreError::Corrupt(WHAT));
    }
    match r.u8()? {
        TAG_READY => Ok(RowStatus::Ready { nodes: r.u32()? }),
        TAG_NOT_INDEXED => Ok(RowStatus::NotIndexed(get_why(&mut r)?)),
        _ => Err(StoreError::Corrupt(WHAT)),
    }
}

/// A row, decoded in full.
pub(crate) fn dec_struct_row(b: &[u8]) -> Result<StructRow, StoreError> {
    let mut r = R::new(b, WHAT);
    if r.u8()? != ROW_VERSION {
        return Err(StoreError::Corrupt(WHAT));
    }
    let row = match r.u8()? {
        TAG_READY => {
            let n = r.u32()? as usize;
            if n > b.len() / MIN_NODE_BYTES {
                return Err(StoreError::Corrupt(WHAT));
            }
            let source = r.str()?;
            let attrs = get_map(&mut r)?;
            // reserved from the length a damaged row can't lie about, and grown as nodes decode
            let mut nodes = Vec::with_capacity(n.min(RESERVE_NODES));
            #[cfg(any(test, feature = "differential"))]
            let check_parents = crate::differential::index_mutant()
                != Some(crate::differential::IndexMutant::RowParentsUnchecked);
            #[cfg(not(any(test, feature = "differential")))]
            let check_parents = true;
            for i in 0..n {
                let node = get_node(&mut r)?;
                if check_parents && node.parent.is_some_and(|p| p as usize >= i) {
                    return Err(StoreError::Corrupt(WHAT));
                }
                nodes.push(node);
            }
            StructRow::Ready(ReadyRow {
                source,
                attrs,
                nodes,
            })
        }
        TAG_NOT_INDEXED => StructRow::NotIndexed(get_why(&mut r)?),
        _ => return Err(StoreError::Corrupt(WHAT)),
    };
    r.end()?;
    Ok(row)
}

/// The minted keys of one version of one doc, by node position: only the nodes the file
/// gives no unique ID. Positions mean something only for the pack and format they were
/// minted under: read under another, a row is treated as absent ([`keys_row_is_for`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct KeysRow {
    pub pack: String,
    pub format: u32,
    /// Node index → (key without `@k:`, weakly aligned).
    pub minted: BTreeMap<u32, (String, bool)>,
}

impl KeysRow {
    /// Empty keys under `pack` and `format`.
    pub(crate) fn empty(pack: &str, format: u32) -> Self {
        Self {
            pack: pack.to_owned(),
            format,
            minted: BTreeMap::new(),
        }
    }

    /// Whether this row was minted under `pack` and `format`.
    pub(crate) fn is_for(&self, pack: &str, format: u32) -> bool {
        #[cfg(any(test, feature = "differential"))]
        if crate::differential::index_mutant()
            == Some(crate::differential::IndexMutant::KeysIgnoreFormat)
        {
            return true;
        }
        self.pack == pack && self.format == format
    }
}

/// Whether the keys row `b` was minted under `pack` and `format` (its header only).
pub(crate) fn keys_row_is_for(b: &[u8], pack: &str, format: u32) -> Result<bool, StoreError> {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::index_mutant()
        == Some(crate::differential::IndexMutant::KeysIgnoreFormat)
    {
        return Ok(true);
    }
    let mut r = R::new(b, WHAT_KEYS);
    if r.u8()? != KEYS_VERSION {
        return Err(StoreError::Corrupt(WHAT_KEYS));
    }
    Ok(r.bytes_ref()? == pack.as_bytes() && r.u32()? == format)
}

pub(crate) fn enc_keys_row(k: &KeysRow) -> Vec<u8> {
    let mut w = W::default();
    w.u8(KEYS_VERSION).str(&k.pack).u32(k.format);
    w.u32(k.minted.len() as u32);
    for (at, (key, weak)) in &k.minted {
        w.u32(*at).str(key).u8(u8::from(*weak));
    }
    w.0
}

pub(crate) fn dec_keys_row(b: &[u8]) -> Result<KeysRow, StoreError> {
    let mut r = R::new(b, WHAT_KEYS);
    if r.u8()? != KEYS_VERSION {
        return Err(StoreError::Corrupt(WHAT_KEYS));
    }
    let pack = r.str()?;
    let format = r.u32()?;
    let n = r.u32()? as usize;
    if n > b.len() / 9 {
        return Err(StoreError::Corrupt(WHAT_KEYS));
    }
    let mut minted = BTreeMap::new();
    for _ in 0..n {
        let at = r.u32()?;
        let key = r.str()?;
        let weak = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(StoreError::Corrupt(WHAT_KEYS)),
        };
        minted.insert(at, (key, weak));
    }
    r.end()?;
    Ok(KeysRow {
        pack,
        format,
        minted,
    })
}

/// How many nodes hold each native ID in this row.
fn native_counts(row: &ReadyRow) -> HashMap<&str, u32> {
    let mut seen: HashMap<&str, u32> = HashMap::new();
    for n in &row.nodes {
        if let Some(k) = n.native.as_deref() {
            *seen.entry(k).or_default() += 1;
        }
    }
    seen
}

/// Whether any node needs a minted key: it has no native ID, or shares one.
pub(crate) fn needs_mint(row: &ReadyRow) -> bool {
    let seen = native_counts(row);
    row.nodes
        .iter()
        .any(|n| n.native.as_deref().map_or(true, |k| seen[k] > 1))
}

/// The nodes that need a minted key.
pub(crate) fn mint_needed(row: &ReadyRow) -> Vec<usize> {
    let seen = native_counts(row);
    row.nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.native.as_deref().map_or(true, |k| seen[k] > 1))
        .map(|(i, _)| i)
        .collect()
}

/// The join of a row and its keys, as readers see it. A node's key is its unique native ID, or
/// its minted key from `keys`.
pub(crate) fn view_of(
    version: VersionId,
    row: ReadyRow,
    keys: &KeysRow,
) -> Result<IndexView, StoreError> {
    let seen = native_counts(&row);
    let mut node_keys: Vec<(NodeKey, KeyClass)> = Vec::with_capacity(row.nodes.len());
    for (i, n) in row.nodes.iter().enumerate() {
        let native = n.native.as_deref().filter(|k| seen[*k] == 1);
        node_keys.push(match native {
            Some(k) => (NodeKey(k.to_owned()), KeyClass::Native),
            None => {
                let (k, weak) = keys
                    .minted
                    .get(&(i as u32))
                    .ok_or(StoreError::Corrupt(WHAT_KEYS))?;
                (NodeKey(format!("@k:{k}")), KeyClass::Minted { weak: *weak })
            }
        });
    }
    drop(seen);
    let parents: Vec<Option<NodeKey>> = row
        .nodes
        .iter()
        .map(|n| n.parent.map(|p| node_keys[p as usize].0.clone()))
        .collect();
    let mut nodes = Vec::with_capacity(row.nodes.len());
    let mut key_iter = node_keys.into_iter();
    let mut parent_iter = parents.into_iter();
    for n in row.nodes {
        let (key, key_class) = key_iter.next().expect("one key per node");
        let parent = parent_iter.next().expect("one parent per node");
        let fingerprint = Fingerprint {
            kind: n.kind.clone(),
            name: n.name.clone(),
            bounds: n.bounds,
            content: n.content_hash,
            weak: n.weak,
        };
        nodes.push(IndexNode {
            key,
            key_class,
            native: n.native.map(NodeKey),
            kind: n.kind,
            name: n.name,
            parent,
            order: n.order,
            depth: n.depth,
            bounds: n.bounds,
            visible: n.visible,
            visible_in_tree: n.visible_in_tree,
            text: n.text,
            attrs: n.attrs,
            flags: n.flags,
            record_hash: n.record_hash,
            content_hash: n.content_hash,
            fingerprint,
        });
    }
    Ok(IndexView {
        version,
        source: row.source,
        attrs: row.attrs,
        nodes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(parent: Option<u32>) -> RowNode {
        RowNode {
            native: None,
            kind: "k".into(),
            name: "n".into(),
            parent,
            order: 0,
            depth: 0,
            bounds: None,
            visible: true,
            visible_in_tree: true,
            text: None,
            attrs: BTreeMap::new(),
            flags: BTreeSet::new(),
            record_hash: NodeHash::from_raw([1; 32]),
            content_hash: None,
            weak: BTreeMap::new(),
        }
    }

    /// A stored row is not trusted: a bad parent is `Corrupt` at decode, so `view_of` always
    /// returns.
    #[test]
    fn a_stored_row_with_a_bad_parent_is_corrupt() {
        crate::differential::under_env_index_mutant(|| {
            for parent in [0, 1, 5, u32::MAX - 1] {
                let row = StructRow::Ready(ReadyRow {
                    source: "f".into(),
                    attrs: BTreeMap::new(),
                    nodes: vec![node(Some(parent))],
                });
                let b = enc_struct_row(&row);
                assert!(
                    matches!(dec_struct_row(&b), Err(StoreError::Corrupt(_))),
                    "a stored parent that isn't an earlier node is corrupt: {parent}"
                );
            }
        });
    }
}
