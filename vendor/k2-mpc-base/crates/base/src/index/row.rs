//! A version's index as stored: the content-keyed row (a pure function of the bytes, with
//! digests keyed) and the per-doc key overlay (minted keys), and the join of the two that
//! readers see.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::draft::{IndexDraft, Items, Shape};
use super::{
    digest_key, get_view_with, key_digest, put_fonts, put_view, Fingerprint, IndexNode, IndexView,
    KeyClass, NodeHash, NodeKey, NodeRect, NodeView, NotIndexedWhy, INDEX_FONTS_MAX,
    INDEX_ROW_FORMAT,
};
use crate::store::enc::{R, W};
use crate::store::{StoreError, VersionId};
use crate::view_index::FontUse;

/// Row format 1: the nodes. A row with no view item is still written so, byte for byte what a
/// kit before the view items wrote.
const ROW_VERSION: u8 = 1;
/// Row format 2: the view items too, after the doc's attributes (the fonts) and after each
/// node's fields (its view and render hash). A row with no own digest is still written so.
const ROW_VERSION_VIEW: u8 = 2;
/// Row format 3 ([`INDEX_ROW_FORMAT`]): format 2's, and after each node's render hash its own
/// record's hash ([`super::DraftNode::own`]). Written only for a row where some node has one.
const ROW_VERSION_OWN: u8 = INDEX_ROW_FORMAT;
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
    pub view: Option<NodeView>,
    pub render_hash: Option<NodeHash>,
    pub own_hash: Option<NodeHash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadyRow {
    pub source: String,
    pub attrs: BTreeMap<String, String>,
    pub nodes: Vec<RowNode>,
    pub fonts: Vec<FontUse>,
}

impl ReadyRow {
    /// Whether the row carries a view item: then it is stored as format 2 (or 3).
    pub(crate) fn has_view_items(&self) -> bool {
        !self.fonts.is_empty()
            || self
                .nodes
                .iter()
                .any(|n| n.view.is_some() || n.render_hash.is_some())
    }

    /// Whether a node of the row has its own record's hash: then it is stored as format 3.
    pub(crate) fn has_own(&self) -> bool {
        self.nodes.iter().any(|n| n.own_hash.is_some())
    }

    /// The lowest format that holds the row.
    fn items(&self) -> Items {
        Items::of(self.has_view_items(), self.has_own())
    }
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
            view: n.view.clone(),
            render_hash: n.render.as_ref().map(|d| key_digest(&key, d)),
            own_hash: n.own.as_ref().map(|d| key_digest(&key, d)),
        })
        .collect();
    ReadyRow {
        source: draft.source.clone(),
        attrs: draft.attrs.clone(),
        nodes,
        fonts: draft.fonts.clone(),
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

fn put_node(w: &mut W, n: &RowNode, items: Items) {
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
    if items.view {
        #[cfg(any(test, feature = "differential"))]
        let dropped = (crate::differential::index_mutant()
            == Some(crate::differential::IndexMutant::IndexV2FieldDropped))
        .then(|| {
            n.view.as_ref().map(|v| NodeView {
                geom: None,
                ..v.clone()
            })
        });
        #[cfg(not(any(test, feature = "differential")))]
        let dropped: Option<Option<NodeView>> = None;
        match &dropped {
            Some(d) => put_view(w, d.as_ref()),
            None => put_view(w, n.view.as_ref()),
        }
        put_opt_hash(w, n.render_hash.as_ref());
    }
    if items.own {
        put_opt_hash(w, n.own_hash.as_ref());
    }
}

fn put_opt_hash(w: &mut W, h: Option<&NodeHash>) {
    match h {
        None => {
            w.u8(0);
        }
        Some(h) => {
            w.u8(1);
            put_hash(w, h);
        }
    }
}

fn get_opt_hash(r: &mut R<'_>) -> Result<Option<NodeHash>, StoreError> {
    match r.u8()? {
        0 => Ok(None),
        1 => Ok(Some(get_hash(r.bytes()?)?)),
        _ => Err(StoreError::Corrupt(WHAT)),
    }
}

fn get_fonts(r: &mut R<'_>) -> Result<Vec<FontUse>, StoreError> {
    let n = r.u32()? as usize;
    if n > INDEX_FONTS_MAX {
        return Err(StoreError::Corrupt(WHAT));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let family = r.str()?;
        let script = r.str()?;
        let from = r.str()?;
        let embedded = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(StoreError::Corrupt(WHAT)),
        };
        out.push(FontUse {
            family,
            script,
            from,
            embedded,
        });
    }
    Ok(out)
}

fn get_node(r: &mut R<'_>, items: Items) -> Result<RowNode, StoreError> {
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
    let (view, render_hash) = if items.view {
        let view = get_view_with(r, u32::MAX as usize, &mut |_| {})?;
        (view, get_opt_hash(r)?)
    } else {
        (None, None)
    };
    let own_hash = if items.own { get_opt_hash(r)? } else { None };
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
        view,
        render_hash,
        own_hash,
    })
}

/// A row's stored bytes: format 3 when a node has its own record's hash, else format 2 when it
/// carries a view item, else format 1. The node count sits at a fixed place, for [`peek_row`].
pub(crate) fn enc_struct_row(row: &StructRow) -> Vec<u8> {
    let mut w = W::default();
    match row {
        StructRow::Ready(r) => {
            let items = r.items();
            w.u8(if items.own {
                ROW_VERSION_OWN
            } else if items.view {
                ROW_VERSION_VIEW
            } else {
                ROW_VERSION
            });
            w.u8(TAG_READY).u32(r.nodes.len() as u32);
            w.str(&r.source);
            put_map(&mut w, &r.attrs);
            if items.view {
                put_fonts(&mut w, &r.fonts);
            }
            for n in &r.nodes {
                put_node(&mut w, n, items);
            }
        }
        StructRow::NotIndexed(why) => {
            w.u8(ROW_VERSION).u8(TAG_NOT_INDEXED);
            put_why(&mut w, why);
        }
    }
    w.0
}

/// A row's format byte: what it carries past the nodes' first fields. Any other is `Corrupt`.
fn row_items(r: &mut R<'_>) -> Result<Items, StoreError> {
    match r.u8()? {
        ROW_VERSION => Ok(Items::NONE),
        ROW_VERSION_VIEW => Ok(Items::of(true, false)),
        ROW_VERSION_OWN => Ok(Items::of(true, true)),
        _ => Err(StoreError::Corrupt(WHAT)),
    }
}

/// A row's status from its stored bytes, without decoding its nodes.
pub(crate) fn peek_row(b: &[u8]) -> Result<RowStatus, StoreError> {
    let mut r = R::new(b, WHAT);
    row_items(&mut r)?;
    match r.u8()? {
        TAG_READY => Ok(RowStatus::Ready { nodes: r.u32()? }),
        TAG_NOT_INDEXED => Ok(RowStatus::NotIndexed(get_why(&mut r)?)),
        _ => Err(StoreError::Corrupt(WHAT)),
    }
}

/// A row, decoded in full.
pub(crate) fn dec_struct_row(b: &[u8]) -> Result<StructRow, StoreError> {
    let mut r = R::new(b, WHAT);
    let items = row_items(&mut r)?;
    let row = match r.u8()? {
        TAG_READY => {
            let n = r.u32()? as usize;
            if n > b.len() / MIN_NODE_BYTES {
                return Err(StoreError::Corrupt(WHAT));
            }
            let source = r.str()?;
            let attrs = get_map(&mut r)?;
            let fonts = if items.view {
                get_fonts(&mut r)?
            } else {
                Vec::new()
            };
            // reserved from the length a damaged row can't lie about, and grown as nodes decode
            let mut nodes = Vec::with_capacity(n.min(RESERVE_NODES));
            #[cfg(any(test, feature = "differential"))]
            let check_parents = crate::differential::index_mutant()
                != Some(crate::differential::IndexMutant::RowParentsUnchecked);
            #[cfg(not(any(test, feature = "differential")))]
            let check_parents = true;
            for i in 0..n {
                let node = get_node(&mut r, items)?;
                if check_parents && node.parent.is_some_and(|p| p as usize >= i) {
                    return Err(StoreError::Corrupt(WHAT));
                }
                nodes.push(node);
            }
            let row = ReadyRow {
                source,
                attrs,
                nodes,
                fonts,
            };
            // each row is written as the lowest format that holds it: a format-3 row has an own
            // hash, and a format-2 row a view item (one without is the lower format's)
            if row.items() != items {
                return Err(StoreError::Corrupt(WHAT));
            }
            StructRow::Ready(row)
        }
        TAG_NOT_INDEXED if items == Items::NONE => StructRow::NotIndexed(get_why(&mut r)?),
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
            view: n.view,
            render_hash: n.render_hash,
            own_hash: n.own_hash,
        });
    }
    Ok(IndexView {
        version,
        source: row.source,
        attrs: row.attrs,
        nodes,
        fonts: row.fonts,
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
            view: None,
            render_hash: None,
            own_hash: None,
        }
    }

    /// A row of format 1 as a kit before the view items stored it (one node, `@n:1`, a group
    /// named `Layers` with text `hi`): hand-written, so it doesn't follow this encoder.
    fn v1_row_bytes() -> Vec<u8> {
        let mut w = W::default();
        w.u8(1).u8(TAG_READY).u32(1).str("fmt-1");
        w.u32(1).str("seed").str("7");
        w.opt_str(Some("@n:1")).str("group").str("Layers");
        w.u32(0).u32(0).u32(0);
        w.u8(1).u64(1).u64(2).u64(3).u64(4);
        w.u8(1).u8(1);
        w.opt_str(Some("hi"));
        w.u32(0);
        w.u32(1).str("styles");
        w.bytes(&[7; 32]);
        w.u8(0);
        w.u32(0);
        w.0
    }

    /// A row of format 1 still reads, with every view item absent; one with no view item is
    /// still written as format 1, byte for byte.
    #[test]
    fn a_format_1_row_reads_with_the_view_items_absent() {
        let b = v1_row_bytes();
        let StructRow::Ready(row) = dec_struct_row(&b).expect("format 1 decodes") else {
            panic!("ready");
        };
        assert_eq!(row.fonts, Vec::new());
        assert_eq!(row.nodes.len(), 1);
        assert_eq!(row.nodes[0].text.as_deref(), Some("hi"));
        assert_eq!(row.nodes[0].view, None);
        assert_eq!(row.nodes[0].render_hash, None);
        assert_eq!(
            enc_struct_row(&StructRow::Ready(row.clone())),
            b,
            "no view item: written as format 1, the same bytes"
        );
        let v = view_of(3, row, &KeysRow::default()).unwrap();
        assert!(v.fonts.is_empty() && v.nodes[0].view.is_none());
        assert!(v.nodes[0].render_hash.is_none());
    }

    /// A row with view items is format 2 and round-trips; a format-2 row with none, or with a
    /// format byte past 2, is corrupt.
    #[test]
    fn a_format_2_row_round_trips_its_view_items() {
        use crate::index::ViewGeom;
        use crate::view_index::{Bbox, EditableText, Flip};
        crate::differential::under_env_index_mutant(|| {
            let mut n = node(None);
            let mut v = NodeView::new();
            v.geom = Some(ViewGeom {
                bbox: Bbox {
                    x: 1,
                    y: -2,
                    w: 3,
                    h: 4,
                },
                rot: 60_000,
                flip: Flip { h: false, v: true },
                inherited: true,
                approx: false,
            });
            v.editable_text = Some(EditableText::Never);
            v.fields = vec!["slidenum".into()];
            v.text = Some("1\t2".into());
            v.fonts = vec![0];
            n.view = Some(v);
            n.render_hash = Some(NodeHash::from_raw([9; 32]));
            let row = ReadyRow {
                source: "f".into(),
                attrs: BTreeMap::new(),
                nodes: vec![n],
                fonts: vec![FontUse {
                    family: "Aptos".into(),
                    script: "latin".into(),
                    from: "explicit".into(),
                    embedded: true,
                }],
            };
            let b = enc_struct_row(&StructRow::Ready(row.clone()));
            assert_eq!(b[0], ROW_VERSION_VIEW, "view items: format 2");
            assert_eq!(peek_row(&b).unwrap(), RowStatus::Ready { nodes: 1 });
            assert_eq!(
                dec_struct_row(&b).unwrap(),
                StructRow::Ready(row.clone()),
                "every view item reads back"
            );
            let mut newer = b.clone();
            newer[0] = INDEX_ROW_FORMAT + 1;
            assert!(matches!(
                dec_struct_row(&newer),
                Err(StoreError::Corrupt(_))
            ));
            assert!(matches!(peek_row(&newer), Err(StoreError::Corrupt(_))));
            let mut bare = enc_struct_row(&StructRow::Ready(ReadyRow {
                fonts: Vec::new(),
                nodes: vec![node(None)],
                ..row
            }));
            bare[0] = ROW_VERSION_VIEW;
            assert!(
                dec_struct_row(&bare).is_err(),
                "a format-2 row is never one without the items' bytes"
            );
        });
    }

    /// Row format 3 (own record hashes): a row with one round-trips as format 3, with or
    /// without view items; a format-2 row (view items, no own hash) is written byte for byte as
    /// before it (a hand-written format-2 row decodes and encodes back to the same bytes); a
    /// format-3 row with no own hash, or one cut short, is corrupt; a format past 3 is corrupt.
    #[test]
    fn a_format_3_row_round_trips_its_own_hashes() {
        let ready = |nodes: Vec<RowNode>, fonts: Vec<FontUse>| ReadyRow {
            source: "f".into(),
            attrs: BTreeMap::new(),
            nodes,
            fonts,
        };
        // with an own hash, no view item: format 3
        let mut p = node(None);
        p.own_hash = Some(NodeHash::from_raw([5; 32]));
        let kid = node(Some(0));
        let row = ready(vec![p.clone(), kid.clone()], Vec::new());
        let b = enc_struct_row(&StructRow::Ready(row.clone()));
        assert_eq!(b[0], ROW_VERSION_OWN, "an own hash: format 3");
        assert_eq!(peek_row(&b).unwrap(), RowStatus::Ready { nodes: 2 });
        assert_eq!(
            dec_struct_row(&b).unwrap(),
            StructRow::Ready(row.clone()),
            "format 3 without view items reads back"
        );
        // read back as a view (keyed nodes: no minted keys needed)
        let mut keyed = row.clone();
        for (i, n) in keyed.nodes.iter_mut().enumerate() {
            n.native = Some(format!("@n:{}", i + 1));
        }
        let v = view_of(4, keyed, &KeysRow::default()).unwrap();
        assert_eq!(v.nodes[0].own_hash, Some(NodeHash::from_raw([5; 32])));
        assert_eq!(v.nodes[1].own_hash, None);
        // with view items too
        let mut k2 = kid.clone();
        k2.render_hash = Some(NodeHash::from_raw([9; 32]));
        let both = ready(vec![p.clone(), k2.clone()], Vec::new());
        let b3 = enc_struct_row(&StructRow::Ready(both.clone()));
        assert_eq!(
            b3[0], ROW_VERSION_OWN,
            "an own hash and a view item: format 3"
        );
        assert_eq!(
            dec_struct_row(&b3).unwrap(),
            StructRow::Ready(both.clone()),
            "format 3 with view items reads back"
        );
        // the same row without its own hash: format 2, the bytes a kit before format 3 wrote
        let mut p2 = p.clone();
        p2.own_hash = None;
        let v2 = ready(vec![p2, k2], Vec::new());
        let b2 = enc_struct_row(&StructRow::Ready(v2.clone()));
        assert_eq!(b2[0], ROW_VERSION_VIEW, "no own hash: format 2");
        let mut hand = W::default();
        hand.u8(2).u8(TAG_READY).u32(2).str("f");
        hand.u32(0); // attrs
        hand.u32(0); // fonts
        for (parent, render) in [(0u32, None), (1u32, Some([9u8; 32]))] {
            hand.opt_str(None).str("k").str("n");
            hand.u32(parent).u32(0).u32(0);
            hand.u8(0).u8(1).u8(1);
            hand.opt_str(None);
            hand.u32(0);
            hand.u32(0);
            hand.bytes(&[1; 32]);
            hand.u8(0);
            hand.u32(0);
            hand.u8(0); // no view
            match render {
                None => {
                    hand.u8(0);
                }
                Some(r) => {
                    hand.u8(1).bytes(&r);
                }
            }
        }
        assert_eq!(b2, hand.0, "format 2, byte for byte as before format 3");
        assert_eq!(dec_struct_row(&b2).unwrap(), StructRow::Ready(v2));
        // a format-3 byte on a row without an own hash is corrupt
        let mut no_own = b2.clone();
        no_own[0] = ROW_VERSION_OWN;
        assert!(
            dec_struct_row(&no_own).is_err(),
            "a format-3 byte on format-2 bytes"
        );
        let mut none_set = ready(vec![node(None)], Vec::new());
        none_set.nodes[0].render_hash = Some(NodeHash::from_raw([3; 32]));
        let mut b = enc_struct_row(&StructRow::Ready(none_set));
        b[0] = ROW_VERSION_OWN;
        b.push(0); // an own marker of "none" for the one node: still no own hash anywhere
        assert!(matches!(dec_struct_row(&b), Err(StoreError::Corrupt(_))));
        // cut short, or past format 3
        let short = &b3[..b3.len() - 1];
        assert!(dec_struct_row(short).is_err(), "a format-3 row cut short");
        let mut newer = b3.clone();
        newer[0] = ROW_VERSION_OWN + 1;
        assert!(matches!(
            dec_struct_row(&newer),
            Err(StoreError::Corrupt(_))
        ));
        assert!(matches!(peek_row(&newer), Err(StoreError::Corrupt(_))));
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
                    fonts: Vec::new(),
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
