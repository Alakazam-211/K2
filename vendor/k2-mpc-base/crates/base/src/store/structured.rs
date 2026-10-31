//! Structured (list and map) docs in the store, format 6 (C1c, K-FMT-10 item 22):
//!
//! - **Typed versions** (version record 3): each structured root's content in the kit's
//!   typed neutral encoding (a list as `[(key, value)]` in order, a map as sorted
//!   `[(key, value)]`), and the codec settings when the version was cut.
//! - **Revert records version 3**: per structured root, the change's ops by key, each with
//!   its write, its effect, for a move its left neighbours in place order (nearest first,
//!   deleted ones included, up to the nearest live one: it goes back after the first still
//!   known, spec v1.10 S4), its values before and after, and the write its facet held before it (`prev`, filled in here from
//!   `ELEM_WRITES` in the change's own transaction). Content lives only in values.
//! - **`ELEM_WRITES`** `(doc, root, key, facet)` → the change (its log position) and write
//!   that hold that facet now, and the write it restores when an undo, redo or revert put it
//!   back (U23). Blame, the per-facet `--base` check (K-BR-12), undo and revert read it; the
//!   audit checks it against the doc. Kept for the doc's life; a dropped element leaves it.
//! - **Change summaries version 2**: per structured root, element counts (`log`).
//!
//! **Lazy upgrade.** A store is stamped 6 only in the transaction that first writes any of
//! these (a grid doc's import): a store with text docs only stays at format 5, byte for
//! byte, and keeps the downgrade guarantee in full. A build that reads only format 5
//! refuses a format-6 store with one line ([`super::StoreError::Newer`]).

use redb::{ReadableTable, TableDefinition};

use super::enc::{R, W};
use super::{DocId, DocStore, Seq, StoreError, META, STORE_FORMAT};
use crate::elem::{Value, WriteId};

/// `(doc, root, key, facet)` → [`ElemWrite`]. Format 6.
pub(super) const ELEM_WRITES: TableDefinition<(u64, &str, &str, u8), &[u8]> =
    TableDefinition::new("elem_writes");

/// `(doc, row key)` → the latest change (8 bytes, little-endian) that left the row on its key
/// by a non-exact pairing of a text, or changed a marked row by undo, redo, revert or restore
/// (`row_marks`, format 6). A keyed write whose base or seen version predates it is refused:
/// the key may hold another row's content since. Only ever moves forward. Keys, no content.
pub(super) const ROW_MARKS: TableDefinition<(u64, &str), &[u8]> = TableDefinition::new("row_marks");

/// In [`ROW_MARKS`], the doc-level entry (no row has an empty key): the latest change that
/// remapped the columns. Any keyed write whose base predates it is refused, whatever it names.
const COL_REMAP: &str = "";

/// `(doc, pid)` → the Overwritten notes waiting for that participant, encoded by the session
/// (`overwrite_notes`, format 6). They hold capped document values: a purge strips them, a doc
/// delete erases them, and they expire with the revert window. Bounded per participant.
pub(super) const OVERWRITE_NOTES: TableDefinition<(u64, &str), &[u8]> =
    TableDefinition::new("overwrite_notes");

/// A facet's code in `ELEM_WRITES`: a list element's value, position (its last insert or
/// move), existence (its last insert, delete or restore) and insert (who first put it
/// there: `blame --structure`); a map key's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ElemFacet {
    Value = 0,
    Position = 1,
    Existence = 2,
    Insert = 3,
}

impl ElemFacet {
    pub(crate) const ALL: [ElemFacet; 4] = [
        ElemFacet::Value,
        ElemFacet::Position,
        ElemFacet::Existence,
        ElemFacet::Insert,
    ];

    fn from_u8(v: u8) -> Result<Self, StoreError> {
        Ok(match v {
            0 => Self::Value,
            1 => Self::Position,
            2 => Self::Existence,
            3 => Self::Insert,
            _ => return Err(StoreError::Corrupt("elem write facet")),
        })
    }

    pub(crate) fn of(f: crate::elem::Facet) -> Self {
        match f {
            crate::elem::Facet::Value => Self::Value,
            crate::elem::Facet::Position => Self::Position,
            crate::elem::Facet::Existence => Self::Existence,
        }
    }
}

/// What an op was, in a record and in `ELEM_WRITES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ElemOpKind {
    Insert = 1,
    Move = 2,
    Delete = 3,
    Restore = 4,
    Set = 5,
    Drop = 6,
    MapSet = 7,
    MapDelete = 8,
}

impl ElemOpKind {
    fn from_u8(v: u8) -> Result<Self, StoreError> {
        Ok(match v {
            1 => Self::Insert,
            2 => Self::Move,
            3 => Self::Delete,
            4 => Self::Restore,
            5 => Self::Set,
            6 => Self::Drop,
            7 => Self::MapSet,
            8 => Self::MapDelete,
            _ => return Err(StoreError::Corrupt("elem op kind")),
        })
    }

    /// The facet the op writes (and whose earlier write it records as `prev`).
    pub(crate) fn facet(self) -> ElemFacet {
        match self {
            Self::Insert | Self::Move => ElemFacet::Position,
            Self::Delete | Self::Restore | Self::Drop => ElemFacet::Existence,
            Self::Set | Self::MapSet | Self::MapDelete => ElemFacet::Value,
        }
    }

    pub(crate) fn of_list(k: crate::elem::ListOpKind) -> Self {
        use crate::elem::ListOpKind as L;
        match k {
            L::Insert => Self::Insert,
            L::Move => Self::Move,
            L::Delete => Self::Delete,
            L::Restore => Self::Restore,
            L::Set => Self::Set,
            L::Drop => Self::Drop,
        }
    }

    pub(crate) fn of_map(k: crate::elem::MapOpKind) -> Self {
        match k {
            crate::elem::MapOpKind::Set => Self::MapSet,
            crate::elem::MapOpKind::Delete => Self::MapDelete,
        }
    }
}

/// Who holds one facet now: change `seq`'s write `write` (`None`: a write with no identity,
/// which the per-facet check counts as someone else's, KC7), and for a value an undo, redo or
/// revert put back, the write it restores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ElemWrite {
    pub seq: Seq,
    pub write: Option<WriteId>,
    pub kind: ElemOpKind,
    pub restores: Option<(Seq, Option<WriteId>)>,
}

/// An op's effect, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoredEffect {
    Visible = 0,
    Hidden = 1,
    None = 2,
}

/// One structured op of a change, by key (revert record version 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ElemOpRecord {
    pub kind: ElemOpKind,
    pub key: String,
    pub write: Option<WriteId>,
    pub effect: StoredEffect,
    /// A move: the element's left neighbours **in place order** just before the op, nearest
    /// first, deleted ones included (a deleted element keeps its place), up to and including
    /// the nearest live one, at most `LEFT_CHAIN` (8) keys (empty: it was first).
    /// Its undo goes back right after the first of them that the list still knows (spec v1.10
    /// S4; a dropped key is skipped), or to the start. Other ops: empty.
    pub left: Vec<String>,
    pub value_before: Option<Value>,
    pub value_after: Option<Value>,
    /// A purge redacted the values (C1f).
    pub redacted: bool,
    /// The facet's holder just before the op (filled in by the store, from `ELEM_WRITES`).
    pub prev: Option<ElemWrite>,
    /// An undo, redo or revert putting a facet back (a value, a place, an existence): the
    /// write it restores (U23, spec v1.10 S2), path-compressed.
    pub restores: Option<(Seq, Option<WriteId>)>,
}

/// A change's ops on one structured root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ElemRoot {
    pub root: String,
    /// A MovableList root (else a Map).
    pub list: bool,
    pub ops: Vec<ElemOpRecord>,
}

/// One structured root's element counts in a change summary (`log`: `+2 rows −1 row ~5
/// cells`). For a list: elements inserted, deleted, moved, restored, values set, dropped;
/// for a map: keys set (`set`) and deleted (`deleted`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ElemCounts {
    pub root: String,
    pub inserted: u64,
    pub deleted: u64,
    pub moved: u64,
    pub restored: u64,
    pub set: u64,
    pub dropped: u64,
}

impl ElemCounts {
    pub(crate) fn new(root: &str) -> Self {
        Self {
            root: root.to_owned(),
            ..Self::default()
        }
    }

    /// Every count zero.
    pub fn is_empty(&self) -> bool {
        self.inserted == 0
            && self.deleted == 0
            && self.moved == 0
            && self.restored == 0
            && self.set == 0
            && self.dropped == 0
    }

    /// The counts of one root's ops (the ops that changed something).
    pub(crate) fn of(root: &ElemRoot) -> Self {
        let mut c = Self::new(&root.root);
        for o in root.ops.iter().filter(|o| o.effect != StoredEffect::None) {
            match o.kind {
                ElemOpKind::Insert => c.inserted += 1,
                ElemOpKind::Move => c.moved += 1,
                ElemOpKind::Delete | ElemOpKind::MapDelete => c.deleted += 1,
                ElemOpKind::Restore => c.restored += 1,
                ElemOpKind::Set | ElemOpKind::MapSet => c.set += 1,
                ElemOpKind::Drop => c.dropped += 1,
            }
        }
        c
    }
}

// ----- encodings -----

pub(super) fn enc_value(w: &mut W, v: &Value) {
    match v {
        Value::Null => {
            w.u8(0);
        }
        Value::Str(s) => {
            w.u8(1).str(s);
        }
    }
}

pub(super) fn dec_value(r: &mut R<'_>) -> Result<Value, StoreError> {
    match r.u8()? {
        0 => Ok(Value::Null),
        1 => Ok(Value::Str(r.str()?)),
        _ => Err(StoreError::Corrupt("value kind")),
    }
}

fn enc_opt_value(w: &mut W, v: Option<&Value>) {
    match v {
        None => {
            w.u8(0);
        }
        Some(v) => {
            w.u8(1);
            enc_value(w, v);
        }
    }
}

fn dec_opt_value(r: &mut R<'_>) -> Result<Option<Value>, StoreError> {
    match r.u8()? {
        0 => Ok(None),
        1 => Ok(Some(dec_value(r)?)),
        _ => Err(StoreError::Corrupt("optional value")),
    }
}

fn enc_write(w: &mut W, x: Option<&WriteId>) {
    w.opt_str(x.map(WriteId::encode).as_deref());
}

fn dec_write(r: &mut R<'_>) -> Result<Option<WriteId>, StoreError> {
    r.opt_str()?
        .map(|s| WriteId::decode(&s).ok_or(StoreError::Corrupt("write id")))
        .transpose()
}

fn enc_restores(w: &mut W, x: Option<&(Seq, Option<WriteId>)>) {
    match x {
        None => {
            w.u8(0);
        }
        Some((s, wr)) => {
            w.u8(1).u64(*s);
            enc_write(w, wr.as_ref());
        }
    }
}

fn dec_restores(r: &mut R<'_>) -> Result<Option<(Seq, Option<WriteId>)>, StoreError> {
    match r.u8()? {
        0 => Ok(None),
        1 => Ok(Some((r.u64()?, dec_write(r)?))),
        _ => Err(StoreError::Corrupt("restores")),
    }
}

fn enc_elem_write_into(w: &mut W, e: &ElemWrite) {
    w.u8(1).u64(e.seq);
    enc_write(w, e.write.as_ref());
    w.u8(e.kind as u8);
    enc_restores(w, e.restores.as_ref());
}

fn dec_elem_write_from(r: &mut R<'_>) -> Result<ElemWrite, StoreError> {
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("elem write version"));
    }
    Ok(ElemWrite {
        seq: r.u64()?,
        write: dec_write(r)?,
        kind: ElemOpKind::from_u8(r.u8()?)?,
        restores: dec_restores(r)?,
    })
}

pub(super) fn enc_elem_write(e: &ElemWrite) -> Vec<u8> {
    let mut w = W::default();
    enc_elem_write_into(&mut w, e);
    w.0
}

pub(super) fn dec_elem_write(b: &[u8]) -> Result<ElemWrite, StoreError> {
    let mut r = R::new(b, "elem write");
    let e = dec_elem_write_from(&mut r)?;
    r.end()?;
    Ok(e)
}

/// The structured part of a revert record version 3 (after the text roots).
pub(super) fn enc_elem_roots(w: &mut W, roots: &[ElemRoot]) {
    w.u32(roots.len() as u32);
    for root in roots {
        w.str(&root.root)
            .u8(u8::from(root.list))
            .u32(root.ops.len() as u32);
        for o in &root.ops {
            w.u8(o.kind as u8).str(&o.key);
            enc_write(w, o.write.as_ref());
            w.u8(o.effect as u8)
                .opt_str((!o.left.is_empty()).then(|| o.left.join(" ")).as_deref())
                .u8(u8::from(o.redacted));
            enc_opt_value(w, o.value_before.as_ref());
            enc_opt_value(w, o.value_after.as_ref());
            match &o.prev {
                None => {
                    w.u8(0);
                }
                Some(p) => {
                    w.u8(1);
                    enc_elem_write_into(w, p);
                }
            }
            enc_restores(w, o.restores.as_ref());
        }
    }
}

pub(super) fn dec_elem_roots(r: &mut R<'_>, total: usize) -> Result<Vec<ElemRoot>, StoreError> {
    let n = r.u32()? as usize;
    if n > total {
        return Err(StoreError::Corrupt("revert record elem roots"));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let root = r.str()?;
        let list = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(StoreError::Corrupt("revert record elem root kind")),
        };
        let k = r.u32()? as usize;
        if k > total {
            return Err(StoreError::Corrupt("revert record elem ops"));
        }
        let mut ops = Vec::with_capacity(k);
        for _ in 0..k {
            let kind = ElemOpKind::from_u8(r.u8()?)?;
            let key = r.str()?;
            let write = dec_write(r)?;
            let effect = match r.u8()? {
                0 => StoredEffect::Visible,
                1 => StoredEffect::Hidden,
                2 => StoredEffect::None,
                _ => return Err(StoreError::Corrupt("elem op effect")),
            };
            // keys are [0-9a-z._-]: a space separates them
            let left = r
                .opt_str()?
                .map(|s| s.split(' ').map(str::to_owned).collect())
                .unwrap_or_default();
            let redacted = r.u8()? != 0;
            let value_before = dec_opt_value(r)?;
            let value_after = dec_opt_value(r)?;
            let prev = match r.u8()? {
                0 => None,
                1 => Some(dec_elem_write_from(r)?),
                _ => return Err(StoreError::Corrupt("elem op prev")),
            };
            let restores = dec_restores(r)?;
            ops.push(ElemOpRecord {
                kind,
                key,
                write,
                effect,
                left,
                value_before,
                value_after,
                redacted,
                prev,
                restores,
            });
        }
        out.push(ElemRoot { root, list, ops });
    }
    Ok(out)
}

pub(super) fn enc_counts(w: &mut W, counts: &[ElemCounts]) {
    w.u32(counts.len() as u32);
    for c in counts {
        w.str(&c.root)
            .u64(c.inserted)
            .u64(c.deleted)
            .u64(c.moved)
            .u64(c.restored)
            .u64(c.set)
            .u64(c.dropped);
    }
}

pub(super) fn dec_counts(r: &mut R<'_>, total: usize) -> Result<Vec<ElemCounts>, StoreError> {
    let n = r.u32()? as usize;
    if n > total {
        return Err(StoreError::Corrupt("change counts"));
    }
    (0..n)
        .map(|_| {
            Ok(ElemCounts {
                root: r.str()?,
                inserted: r.u64()?,
                deleted: r.u64()?,
                moved: r.u64()?,
                restored: r.u64()?,
                set: r.u64()?,
                dropped: r.u64()?,
            })
        })
        .collect()
}

// ----- the lazy upgrade and the index -----

/// Stamp the store format 6 and create its table, if it isn't already, in `w` (the
/// transaction about to write the store's first structured record).
pub(super) fn upgrade(w: &redb::WriteTransaction) -> Result<(), StoreError> {
    let mut meta = w.open_table(META)?;
    let found = meta.get("store_format")?.map(|v| v.value()).unwrap_or(0);
    if found < STORE_FORMAT {
        meta.insert("store_format", STORE_FORMAT)?;
    }
    drop(meta);
    w.open_table(ELEM_WRITES)?;
    w.open_table(ROW_MARKS)?;
    w.open_table(OVERWRITE_NOTES)?;
    Ok(())
}

/// Replace these participants' waiting notes, in the change's transaction.
pub(super) fn put_note_rows(
    w: &redb::WriteTransaction,
    id: DocId,
    rows: &[(String, Vec<u8>)],
) -> Result<(), StoreError> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut t = w.open_table(OVERWRITE_NOTES)?;
    for (pid, b) in rows {
        if b.is_empty() {
            t.remove((id, pid.as_str()))?;
        } else {
            t.insert((id, pid.as_str()), b.as_slice())?;
        }
    }
    Ok(())
}

/// Mark `rows` at change `seq`, in the change's own transaction.
pub(super) fn put_row_marks(
    w: &redb::WriteTransaction,
    id: DocId,
    seq: Seq,
    rows: &[crate::elem::Key],
    col_remap: bool,
) -> Result<(), StoreError> {
    if rows.is_empty() && !col_remap {
        return Ok(());
    }
    let mut t = w.open_table(ROW_MARKS)?;
    for k in rows {
        t.insert((id, k.as_str()), seq.to_le_bytes().as_slice())?;
    }
    if col_remap {
        t.insert((id, COL_REMAP), seq.to_le_bytes().as_slice())?;
    }
    Ok(())
}

/// Record change `seq`'s structured ops in `ELEM_WRITES`, in order, filling each op's
/// `prev` (the facet's holder before it) from the table as it stands.
pub(super) fn put_elem_writes(
    w: &redb::WriteTransaction,
    id: DocId,
    seq: Seq,
    roots: &mut [ElemRoot],
) -> Result<(), StoreError> {
    let mut t = w.open_table(ELEM_WRITES)?;
    for root in roots.iter_mut() {
        for o in root.ops.iter_mut() {
            let facet = o.kind.facet();
            o.prev = t
                .get((id, root.root.as_str(), o.key.as_str(), facet as u8))?
                .map(|v| dec_elem_write(v.value()))
                .transpose()?;
            if o.effect == StoredEffect::None {
                continue;
            }
            let row = ElemWrite {
                seq,
                write: o.write,
                kind: o.kind,
                restores: None,
            };
            let (r, k) = (root.root.as_str(), o.key.as_str());
            match o.kind {
                ElemOpKind::Insert => {
                    put_row(&mut t, id, r, k, ElemFacet::Insert, &row)?;
                    put_row(&mut t, id, r, k, ElemFacet::Position, &row)?;
                    put_row(&mut t, id, r, k, ElemFacet::Existence, &row)?;
                    if o.value_after.as_ref().is_some_and(|v| *v != Value::Null) {
                        put_row(&mut t, id, r, k, ElemFacet::Value, &row)?;
                    }
                }
                ElemOpKind::Drop => {
                    for f in ElemFacet::ALL {
                        t.remove((id, r, k, f as u8))?;
                    }
                }
                // a restoring write (an undo, redo or revert) holds its facet as the write
                // it restores, on every facet (spec v1.10 S2)
                kind => {
                    let row = ElemWrite {
                        restores: o.restores,
                        ..row
                    };
                    put_row(&mut t, id, r, k, kind.facet(), &row)?;
                }
            }
        }
    }
    Ok(())
}

fn put_row(
    t: &mut redb::Table<'_, (u64, &'static str, &'static str, u8), &'static [u8]>,
    id: DocId,
    root: &str,
    key: &str,
    f: ElemFacet,
    row: &ElemWrite,
) -> Result<(), StoreError> {
    t.insert((id, root, key, f as u8), enc_elem_write(row).as_slice())?;
    Ok(())
}

/// One row of `ELEM_WRITES`: `(root, key, facet)` and its holder.
pub(crate) type ElemRow = ((String, String, ElemFacet), ElemWrite);

impl DocStore {
    /// The latest change that marked row `key` ([`ROW_MARKS`]). `Err(MarksLost)`: the doc has
    /// structured writes but the store has no marks table, so nothing can be vouched for.
    pub(crate) fn row_mark(&self, id: DocId, key: &str) -> Result<Option<Seq>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(ROW_MARKS) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => {
                return match r.open_table(ELEM_WRITES) {
                    Ok(_) => Err(StoreError::MarksLost(id)),
                    Err(redb::TableError::TableDoesNotExist(_)) => Ok(None),
                    Err(e) => Err(e.into()),
                };
            }
            Err(e) => return Err(e.into()),
        };
        let v = t.get((id, key))?;
        Ok(v.map(|v| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&v.value()[..8]);
            Seq::from_le_bytes(b)
        }))
    }

    /// Every participant's waiting notes on doc `id`, encoded.
    pub(crate) fn overwrite_notes(&self, id: DocId) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(OVERWRITE_NOTES) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for row in t.range((id, "")..(id + 1, ""))? {
            let (k, v) = row?;
            out.push((k.value().1.to_owned(), v.value().to_vec()));
        }
        Ok(out)
    }

    /// Replace these participants' waiting notes, in their own transaction (delivered, purged).
    pub(crate) fn set_overwrite_notes(
        &self,
        id: DocId,
        rows: &[(String, Vec<u8>)],
    ) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        put_note_rows(&w, id, rows)?;
        w.commit()?;
        Ok(())
    }

    /// The latest change that remapped the columns ([`COL_REMAP`]).
    pub(crate) fn col_remap(&self, id: DocId) -> Result<Option<Seq>, StoreError> {
        self.row_mark(id, COL_REMAP)
    }

    /// The store's format stamp (5: text docs only; 6: it holds structured docs).
    pub fn format(&self) -> Result<u64, StoreError> {
        let r = self.begin_read()?;
        Ok(r.open_table(META)?
            .get("store_format")?
            .map(|v| v.value())
            .unwrap_or(0))
    }

    /// Who holds `root`/`key`'s `facet` now (`None`: nothing recorded: never written, or a
    /// store without structured docs).
    pub(crate) fn elem_write(
        &self,
        id: DocId,
        root: &str,
        key: &str,
        facet: ElemFacet,
    ) -> Result<Option<ElemWrite>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(ELEM_WRITES) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        t.get((id, root, key, facet as u8))?
            .map(|v| dec_elem_write(v.value()))
            .transpose()
    }

    /// The rows of `root` whose key starts with `prefix` (a table row's cells: `"<row>\x1f"`),
    /// in key order. `prefix` must end with a byte below `0xff`.
    pub(crate) fn elem_writes_prefixed(
        &self,
        id: DocId,
        root: &str,
        prefix: &str,
    ) -> Result<Vec<(String, ElemFacet, ElemWrite)>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(ELEM_WRITES) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut upper = prefix.as_bytes().to_vec();
        let last = upper.pop().expect("a non-empty prefix");
        upper.push(last + 1);
        let upper = String::from_utf8(upper).expect("an ASCII separator ends the prefix");
        let mut out = Vec::new();
        for kv in t.range((id, root, prefix, 0u8)..(id, root, upper.as_str(), 0u8))? {
            let (k, v) = kv?;
            let (_, _, key, f) = k.value();
            out.push((
                key.to_owned(),
                ElemFacet::from_u8(f)?,
                dec_elem_write(v.value())?,
            ));
        }
        Ok(out)
    }

    /// Every `ELEM_WRITES` row of doc `id` (the audit), in key order.
    pub(crate) fn elem_writes(&self, id: DocId) -> Result<Vec<ElemRow>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(ELEM_WRITES) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for kv in t.range((id, "", "", 0u8)..(id + 1, "", "", 0u8))? {
            let (k, v) = kv?;
            let (_, root, key, f) = k.value();
            out.push((
                (root.to_owned(), key.to_owned(), ElemFacet::from_u8(f)?),
                dec_elem_write(v.value())?,
            ));
        }
        Ok(out)
    }

    /// Tests only: overwrite one `ELEM_WRITES` row (a planted index state).
    #[cfg(test)]
    pub(crate) fn put_elem_write_for_tests(
        &self,
        id: DocId,
        root: &str,
        key: &str,
        facet: ElemFacet,
        w: &ElemWrite,
    ) -> Result<(), StoreError> {
        let tx = self.begin_write()?;
        tx.open_table(ELEM_WRITES)?
            .insert((id, root, key, facet as u8), enc_elem_write(w).as_slice())?;
        tx.commit()?;
        Ok(())
    }
}
