//! Structured (grid) docs in a session (C1c; CSV seam design §2 A6, §7.1–7.2, §7.5, §7.7,
//! §7.8). A child of `live`, so it works on the session's own state.
//!
//! - **Edits** ([`Session::edit_ops`], [`Session::edit_ops_with`]): list and map ops by key,
//!   one change, attributed as text edits are, under the same lock, version guard and rate
//!   limit. Before anything applies: an agent needs a base (K-BR-12); `--base V` refuses only
//!   when a facet the edit touches was written after V by someone else (judged by write
//!   identity from `ELEM_WRITES`, a write with no identity counting as someone else's,
//!   KC7); a table's size limits (K-CSV-7, KC4) refuse growth past them, never content.
//! - **Records**: each change's ops by key (revert record v3) with the live key before each
//!   list op, its counts, and its element writes (`ELEM_WRITES`, written in the change's own
//!   transaction).
//! - **History**: typed versions; restore by identity (revive, delete, the minimal moves as
//!   forward chains, then sets) as one forward change; revert from the v3 record by the
//!   element rules (seam §7.3, item 3); blame per element ([`Session::blame_cell`]).
//! - **Files**: outside edits are aligned to the doc as the file last held it
//!   ([`super::super::Pack::reconcile`]) and applied **by key** to the live doc, so changes
//!   the file never got are kept; `write --base` for grids is [`crate::table::merge3`]
//!   ([`Session::write_base_grid`]), with who wrote each conflicting region, where it is,
//!   and its rendered lines.
//! - **`rows drop-deleted`** ([`Session::drop_deleted`], owner only) and the **lost-edit
//!   audit's grid form** ([`Session::audit`]).
//!
//! Not here (later slices): undo and redo by element (C1e), erasure of values (C1f),
//! presence by element and the grid parity tier (C1g), the grid verbs (C1i).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::Range;

use super::*;
use crate::elem::{
    Anchor, Applied as ElemApplied, Effect, ElemState, Facet, Key, KeyedRoot, KeyedState, ListOp,
    ListOpKind, MapOp, MapOpKind, Value, WriteId,
};
use crate::session::awareness::{ClaimSpan, GridSpan, Overwrite};
use crate::store::{ElemFacet, ElemOpKind, ElemOpRecord, ElemWrite, StoredEffect};
use crate::table::{self, Table};
use crate::tools::VerbHistory;

/// A structured undo, redo or revert: per `(root, key, facet)`, the write it puts back (U23,
/// spec v1.10 S2).
pub(crate) type ElemRestores = HashMap<(String, String, ElemFacet), (Seq, Option<WriteId>)>;

/// The write a holder counts as (spec v1.10 S2): the one it restores, if it put one back.
pub(super) fn resolved(w: &ElemWrite) -> (Seq, Option<WriteId>) {
    w.restores.unwrap_or((w.seq, w.write))
}

/// One facet a `--base` edit touched that someone else wrote after the base (K-BR-12).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FacetConflict {
    pub root: String,
    pub key: String,
    pub facet: Facet,
    /// Who wrote it (`None`: a write the store can't attribute).
    pub by: Option<String>,
    /// The change that wrote it.
    pub change: Seq,
    /// What it holds now: `live at #N`, `deleted`, a value, or `absent`.
    pub now: String,
    /// It is a row a text re-paired since the base (an `edited_row`): the key may hold another
    /// row's content now. `change` is the text's change.
    pub rewritten_row: bool,
}

impl fmt::Display for FacetConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let facet = match self.facet {
            Facet::Value => "value",
            Facet::Position => "position",
            Facet::Existence => "existence",
        };
        if self.rewritten_row && self.root == table::COLS {
            return write!(
                f,
                "edited_row: a text remapped the columns in change {} by {}, after the version \
                 you named; read the grid again (cat) and retry with the version it reports",
                self.change,
                self.by.as_deref().unwrap_or("someone"),
            );
        }
        if self.rewritten_row {
            return write!(
                f,
                "edited_row at row {}: a text rewrote it in change {} by {}, after the version \
                 you named; read the grid again (cat) and retry with the version it reports",
                self.key,
                self.change,
                self.by.as_deref().unwrap_or("someone"),
            );
        }
        write!(
            f,
            "{} {} {facet} written by {} in change {} (now {})",
            self.root,
            self.key.replace(table::CELL_SEP, "/"),
            self.by.as_deref().unwrap_or("someone"),
            self.change,
            self.now
        )
    }
}

/// A cell a change wrote into a deleted row or column: kept, hidden, comes back with a
/// restore (contract §1.6, K-CSV-4), with who deleted the row (or column).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HiddenCellWrite {
    pub row: Key,
    pub col: Key,
    /// The row is deleted (else only the column is).
    pub row_hidden: bool,
    pub col_hidden: bool,
    /// Who deleted the row (or, when only the column is deleted, the column).
    pub deleted_by: Option<String>,
    /// The change that deleted it.
    pub deleted_in: Option<Seq>,
    /// The sentence every host and agent shows for it (csv contract §4.3's template), the same
    /// text everywhere: `r2: c1 set by B in a row deleted by A; row restore --row r2 brings it
    /// back`. Keys and pids only, never a value or a column name (both are content, §6).
    pub message: String,
    /// What the message is built from (pids), so a viewer's lens can rebuild it.
    pub(crate) parts: HiddenParts,
}

/// The pids [`HiddenCellWrite::message`] names: the writer, and the row's and the column's
/// deleters (`Some(None)`: hidden, deleter not recorded).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct HiddenParts {
    writer: String,
    row_deleter: Option<Option<String>>,
    col_deleter: Option<Option<String>>,
}

impl HiddenCellWrite {
    /// Every pid it names through `f` (an App viewer's handles), the message rebuilt.
    pub(crate) fn relabel(&mut self, mut f: impl FnMut(&str) -> String) {
        self.deleted_by = self.deleted_by.take().map(|p| f(&p));
        let p = &mut self.parts;
        p.writer = f(&p.writer);
        for d in [&mut p.row_deleter, &mut p.col_deleter] {
            if let Some(Some(x)) = d {
                *x = f(x);
            }
        }
        self.message = hidden_write_message(
            &self.row,
            &self.col,
            &p.writer,
            p.row_deleter.as_ref().map(|d| d.as_deref()),
            p.col_deleter.as_ref().map(|d| d.as_deref()),
        );
    }
}

/// A structured change the doc has just made: by whom (its author's pid, or the external
/// participant), its update, and its applied stream.
struct Made<'a> {
    writer: &'a str,
    update: Vec<u8>,
    stream: Vec<ElemApplied>,
}

/// [`HiddenCellWrite::message`]: `writer` set `row`/`col`, hidden by a row and/or a column
/// deleted by the given deleters (`None`: not recorded).
pub(crate) fn hidden_write_message(
    row: &Key,
    col: &Key,
    writer: &str,
    row_deleter: Option<Option<&str>>,
    col_deleter: Option<Option<&str>>,
) -> String {
    let by = |d: Option<&str>| match d {
        Some(p) => format!("deleted by {p}"),
        None => "deleted earlier".to_owned(),
    };
    match (row_deleter, col_deleter) {
        (Some(r), Some(c)) => format!(
            "{row}: {col} set by {writer} in a row {} and a column {}; row restore --row {row} \
             and col restore --col {col} bring it back",
            by(r),
            by(c)
        ),
        (None, Some(c)) => format!(
            "{row}: {col} set by {writer} in a column {}; col restore --col {col} brings it back",
            by(c)
        ),
        (r, None) => format!(
            "{row}: {col} set by {writer} in a row {}; row restore --row {row} brings it back",
            by(r.flatten())
        ),
    }
}

/// Who holds one element facet ([`Session::blame_elem`], [`Session::blame_cell`]). A value an
/// undo, redo or revert put back blames its **original write** (U23, `restores_write`), with
/// the restoring change as `restored_by` / `restored_in`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ElemBlame {
    pub root: String,
    pub key: String,
    /// The current value (`None`: absent, or a list element's position or existence).
    pub value: Option<Value>,
    /// The last write was a delete (a map key, or a list element's existence).
    pub deleted: bool,
    pub author: String,
    pub change: Seq,
    pub kind: Option<ChangeKind>,
    pub via: Option<String>,
    pub at_ms: Option<u64>,
    pub on_behalf_of: Option<String>,
    pub restored_by: Option<String>,
    pub restored_in: Option<Seq>,
}

/// A cell's blame ([`Session::blame_cell`]).
pub type CellBlame = ElemBlame;

/// One row's (or column's) place in [`Session::blame_structure`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlaceBlame {
    pub key: Key,
    /// Its live position now, 1-based (`#N`).
    pub at: usize,
    /// Who inserted it (`None`: nothing recorded).
    pub inserted: Option<ElemBlame>,
    /// Who last moved it, when it moved since its insert.
    pub moved: Option<ElemBlame>,
}

/// `blame --structure` (A12, K-HIST-11): per live row and column, in order, who inserted it
/// and who last moved it ([`Session::blame_structure`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StructureBlame {
    /// The doc's version it describes.
    pub version: Seq,
    pub rows: Vec<PlaceBlame>,
    pub cols: Vec<PlaceBlame>,
}

/// The doc's history for a structured verb, under the call's lock ([`VerbHistory`]): the
/// version, and attributed last writes when the caller has the history right (as `blame`).
/// For an App viewer (K5) the attributions are by handle: its lens runs on a working copy of
/// the doc's handles, which the caller puts back after the verb ([`LockedHistory::handles`]),
/// so a handle a verb's output names is the one every later call gives.
struct LockedHistory<'s> {
    session: &'s Session,
    st: &'s State,
    allowed: bool,
    app: Option<(String, std::cell::RefCell<super::super::handles::Handles>)>,
}

impl<'s> LockedHistory<'s> {
    fn new(session: &'s Session, st: &'s State, i: usize) -> Self {
        LockedHistory {
            session,
            st,
            allowed: st.members[i].rights.history,
            app: Session::app_viewer(st, i)
                .map(|v| (v, std::cell::RefCell::new(st.handles.clone()))),
        }
    }

    /// The handles after the verb (an App viewer's lens may have added some), to put back.
    fn handles(self) -> Option<super::super::handles::Handles> {
        self.app.map(|(_, h)| h.into_inner())
    }
}

impl VerbHistory for LockedHistory<'_> {
    fn version(&self) -> Seq {
        self.st.version
    }

    fn last_write(&self, root: &str, key: &str, facet: Facet) -> Option<ElemBlame> {
        if !self.allowed {
            return None;
        }
        let store = self.session.store(self.st).ok()?;
        let w = store
            .elem_write(self.session.doc_id, root, key, ElemFacet::of(facet))
            .ok()??;
        let b = self
            .session
            .blame_of_write(store, root, key, None, &w)
            .ok()?;
        Some(match &self.app {
            Some((v, h)) => h.borrow_mut().lens(v).elem_blame(b),
            None => b,
        })
    }
}

/// A rendered file's lines, without their terminators: LF, CRLF or a lone CR ends a line (as
/// [`Table::line_map`] counts them); no empty line after a final terminator.
fn split_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\n' => {
                out.push(text[start..i].to_owned());
                start = i + 1;
            }
            b'\r' => {
                out.push(text[start..i].to_owned());
                if b.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < b.len() {
        out.push(text[start..].to_owned());
    }
    out
}

/// [`Session::write_base_grid`]'s choices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct GridWriteOptions {
    /// `--partial` (K-BR-9): apply the clean part, report the conflicts.
    pub partial: bool,
    /// What the merge does about others' claims (default [`ClaimPolicy::Ignore`]): under
    /// [`ClaimPolicy::Respect`] an agent's merged change touching another participant's grid
    /// claim (or a reservation handed to someone else) is refused, exit 5, nothing applied,
    /// as for every other edit (A10).
    pub policy: ClaimPolicy,
}

impl GridWriteOptions {
    pub fn partial(mut self) -> Self {
        self.partial = true;
        self
    }

    /// The claim policy ([`ClaimPolicy::Respect`]: `--respect-claims`).
    pub fn policy(mut self, policy: ClaimPolicy) -> Self {
        self.policy = policy;
        self
    }
}

/// One conflict of a grid `write --base`, with the session's context (contract §3.4): the
/// table's report plus who last wrote the region, where it is in the live table, and the
/// rendered lines it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GridConflict {
    pub conflict: table::Conflict,
    /// Who last wrote the region (`None`: nobody recorded, or not attributable).
    pub written_by: Option<String>,
    pub written_in: Option<Seq>,
    /// Who (other than the writer) holds a live claim covering it (A10): their pids, in
    /// the order they claimed.
    pub claimed_by: Vec<String>,
    /// The row's (or column's) 0-based index in the live table, when it is live.
    pub index: Option<usize>,
    /// The 1-based rendered lines (end exclusive) its row is on in the live file.
    pub lines: Option<Range<usize>>,
}

impl fmt::Display for GridConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.conflict)?;
        if let Some(w) = &self.written_by {
            write!(f, "; written by {w}")?;
            if let Some(c) = self.written_in {
                write!(f, " in change {c}")?;
            }
        }
        if let Some(l) = &self.lines {
            if l.end == l.start + 1 {
                write!(f, "; line {}", l.start)?;
            } else {
                write!(f, "; lines {}-{}", l.start, l.end - 1)?;
            }
        }
        if self.conflict.is_rewritten_row() {
            f.write_str(
                "; nothing of this row applied. Read the grid again (cat) and write --base with \
                 the version it reports, your change redone on the current row; or, to change \
                 one cell of it, cell set --base with that version",
            )?;
        }
        Ok(())
    }
}

/// [`Session::write_base_grid`]'s outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridWrite {
    Applied(EditResult),
    /// Overlaps with changes since the base; nothing applied.
    Conflicts(Vec<GridConflict>),
    /// The base version is unknown or no longer cached: re-read and retry.
    BaseExpired,
}

impl GridWrite {
    /// The exit code (K-EXIT-1): 0 applied, 1 conflict, 8 base expired.
    pub fn exit(&self) -> u8 {
        match self {
            GridWrite::Applied(_) => crate::tools::exit::OK,
            GridWrite::Conflicts(_) => crate::tools::exit::NO,
            GridWrite::BaseExpired => crate::tools::exit::TOO_OLD,
        }
    }
}

/// Returned by [`Session::write_base_grid`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GridWriteReport {
    pub outcome: GridWrite,
    /// With `--partial`: the conflicts left out of what applied.
    pub skipped: Vec<GridConflict>,
    /// The body's new keys the live table already used, and the keys they got.
    pub rekeyed: Vec<(Key, Key)>,
}

impl GridWriteReport {
    /// 0 when everything applied, 1 when anything conflicted or was skipped, 8 expired.
    pub fn exit(&self) -> u8 {
        match &self.outcome {
            GridWrite::Applied(_) if !self.skipped.is_empty() => crate::tools::exit::NO,
            o => o.exit(),
        }
    }
}

/// One finding of [`Session::audit`] (K-AUD-1, grid form).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuditViolation {
    /// (a) The stored log doesn't replay to the live doc.
    LogReplay,
    /// (b) An acknowledged write is gone: the doc holds the facet's earlier write (or
    /// nothing) though change `change` wrote it and nothing recorded since did.
    DroppedWrite {
        root: String,
        key: String,
        facet: Facet,
        change: Seq,
    },
    /// (b) The doc holds a write no recorded change made (a silent overwrite, or an element
    /// nobody acknowledged).
    SilentWrite {
        root: String,
        key: String,
        facet: Facet,
    },
    /// (b) An element the record says is live (inserted, restored, last moved by change
    /// `change`) isn't: lost, not deleted by any attributed change.
    LostElement {
        root: String,
        key: String,
        change: Seq,
    },
    /// (b) The index names a change the store has no summary of.
    Unattributed {
        root: String,
        key: String,
        change: Seq,
    },
    /// (c) The file, synced and clean, doesn't hold the doc's rendering.
    File,
    /// (d) The doc is clean, but the disk replica (what the next outside edit is aligned
    /// to) doesn't hold the doc by key: [`Session::audit_disk_sync`]'s finding.
    DiskReplica(super::SyncDrift),
}

/// Returned by [`Session::audit`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct AuditReport {
    pub violations: Vec<AuditViolation>,
    /// Element facets checked.
    pub checked: usize,
}

impl AuditReport {
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }
}

// ----- plain helpers -----

pub(super) use crate::session::view::import_edits;

fn stored(e: &Effect) -> StoredEffect {
    match e {
        Effect::Visible => StoredEffect::Visible,
        Effect::Hidden => StoredEffect::Hidden,
        Effect::None { .. } => StoredEffect::None,
    }
}

/// At most this many left neighbours are recorded for a move (spec v1.10 S4): the nearest
/// first, up to and including the nearest live one.
pub const LEFT_CHAIN: usize = 8;

/// Each list root's places (live and deleted, in place order) and live keys just before a
/// change: [`elem_records`] replays the change's ops on them to name each move's left
/// neighbours in place order.
pub(super) type Befores = HashMap<String, (Vec<Key>, HashSet<Key>)>;

/// [`Befores`] for the list roots `edits` touch.
pub(super) fn befores_of(doc: &Doc, edits: &[Edit]) -> Result<Befores> {
    let mut out = Befores::new();
    for e in edits {
        if let Edit::List { root, .. } = e {
            if !out.contains_key(root) {
                let live = doc.list(root)?.into_iter().map(|(k, _)| k).collect();
                out.insert(root.clone(), (doc.places(root)?, live));
            }
        }
    }
    Ok(out)
}

/// `key`'s left neighbours in `places`, nearest first, up to and including the nearest live
/// one, at most [`LEFT_CHAIN`].
fn left_chain(places: &[Key], live: &HashSet<Key>, key: &Key) -> Vec<String> {
    let Some(at) = places.iter().position(|k| k == key) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for k in places[..at].iter().rev() {
        out.push(k.as_str().to_owned());
        if live.contains(k) || out.len() == LEFT_CHAIN {
            break;
        }
    }
    out
}

/// Where an anchor puts a slot in `places` (the element itself already out of it): right
/// after (before) the anchor's slot, deleted or not; the start; the end.
fn anchor_place(places: &[Key], at: &Anchor) -> usize {
    let of = |k: &Key| places.iter().position(|x| x == k);
    match at {
        Anchor::Start => 0,
        Anchor::End => places.len(),
        Anchor::After(a) => of(a).map_or(places.len(), |i| i + 1),
        Anchor::Before(a) => of(a).unwrap_or(0),
    }
}

/// A change's stream as records by key (revert record v3) and counts. `edits` are the
/// change's edits (the stream has one entry per edit and one per op, in order); `befores`
/// each list root's places and live keys just before it (absent: empty). The ops are
/// replayed on them to name each move's left neighbours in place order (spec v1.10 S4).
pub(super) fn elem_records(
    stream: &[ElemApplied],
    edits: &[Edit],
    befores: &Befores,
) -> (Vec<ElemRoot>, Vec<ElemCounts>) {
    debug_assert_eq!(stream.len(), edits.len(), "one applied entry per edit");
    let mut roots: Vec<ElemRoot> = Vec::new();
    let mut states: HashMap<String, (Vec<Key>, HashSet<Key>)> = HashMap::new();
    for (a, e) in stream.iter().zip(edits) {
        match a {
            ElemApplied::Text { .. } => {}
            ElemApplied::List { root, ops } => {
                let (places, live) = states
                    .entry(root.clone())
                    .or_insert_with(|| befores.get(root).cloned().unwrap_or_default());
                let list_ops: &[ListOp] = match e {
                    Edit::List { ops, .. } => ops,
                    _ => &[],
                };
                debug_assert_eq!(list_ops.len(), ops.len(), "one applied op per op");
                let mut recs = Vec::with_capacity(ops.len());
                for (i, o) in ops.iter().enumerate() {
                    let op = list_ops.get(i);
                    let mut left = Vec::new();
                    let applied = !matches!(o.effect, Effect::None { .. });
                    match op {
                        Some(ListOp::Insert { key, at, .. }) if applied => {
                            let p = anchor_place(places, at);
                            places.insert(p, key.clone());
                            live.insert(key.clone());
                        }
                        Some(ListOp::Move { key, to }) => {
                            left = left_chain(places, live, key);
                            if applied {
                                if let Some(p) = places.iter().position(|k| k == key) {
                                    places.remove(p);
                                }
                                let p = anchor_place(places, to);
                                places.insert(p, key.clone());
                            }
                        }
                        Some(ListOp::Delete { key }) if applied => {
                            live.remove(key);
                        }
                        Some(ListOp::Restore { key }) if applied => {
                            live.insert(key.clone());
                        }
                        Some(ListOp::Drop { key }) if applied => {
                            places.retain(|k| k != key);
                            live.remove(key);
                        }
                        _ => {}
                    }
                    recs.push(ElemOpRecord {
                        kind: ElemOpKind::of_list(o.kind),
                        key: o.key.as_str().to_owned(),
                        write: Some(o.write),
                        effect: stored(&o.effect),
                        left,
                        value_before: o.value_before.clone(),
                        value_after: o.value_after.clone(),
                        redacted: false,
                        prev: None,
                        restores: None,
                    });
                }
                push_root(&mut roots, root, true, recs);
            }
            ElemApplied::Map { root, ops } => {
                let recs = ops
                    .iter()
                    .map(|o| ElemOpRecord {
                        kind: ElemOpKind::of_map(o.kind),
                        key: o.key.clone(),
                        write: Some(o.write),
                        effect: stored(&o.effect),
                        left: Vec::new(),
                        value_before: o.before.clone(),
                        value_after: o.after.clone(),
                        redacted: false,
                        prev: None,
                        restores: None,
                    })
                    .collect();
                push_root(&mut roots, root, false, recs);
            }
        }
    }
    roots.retain(|r| !r.ops.is_empty());
    let counts = roots
        .iter()
        .map(ElemCounts::of)
        .filter(|c| !c.is_empty())
        .collect();
    (roots, counts)
}

fn push_root(roots: &mut Vec<ElemRoot>, root: &str, list: bool, ops: Vec<ElemOpRecord>) {
    match roots.iter_mut().find(|r| r.root == root) {
        Some(r) => r.ops.extend(ops),
        None => roots.push(ElemRoot {
            root: root.to_owned(),
            list,
            ops,
        }),
    }
}

/// A table import over the size limits is declined `too_large` (K-CSV-7): the host falls
/// back to the next pack (live text).
pub(super) fn check_import_size(
    config: &Config,
    schema: &Schema,
    ordered: &[(String, Neutral)],
) -> Result<()> {
    if !table::is_table(schema) {
        return Ok(());
    }
    let count = |root: &str| {
        ordered
            .iter()
            .find(|(r, _)| r == root)
            .map_or(0, |(_, n)| match n {
                Neutral::List(l) => l.len(),
                Neutral::Map(m) => m.len(),
                Neutral::Text(_) => 0,
            })
    };
    let (cells, cols, rows) = (count(table::CELLS), count(table::COLS), count(table::ROWS));
    let over = |what: &str, n: usize, max: usize| {
        SessionError::Declined(Declined {
            code: "too_large",
            message: format!(
                "this table is larger than the grid limit ({max} {what}; it has {n}), so it \
                 opens as text"
            ),
        })
    };
    if cells > config.grid_max_cells {
        return Err(over("cells", cells, config.grid_max_cells));
    }
    if cols > config.grid_max_cols {
        return Err(over("columns", cols, config.grid_max_cols));
    }
    // blank rows hold no cell: rows are their own limit
    if rows > config.grid_max_rows {
        return Err(over("rows", rows, config.grid_max_rows));
    }
    // a value or name past `grid_max_value_len` (C1k)
    let longest = ordered
        .iter()
        .filter(|(r, _)| r == table::CELLS || r == table::NAMES)
        .filter_map(|(_, n)| match n {
            Neutral::Map(m) => m.values().filter_map(|v| v.as_str()).map(str::len).max(),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    if longest > config.grid_max_value_len {
        return Err(SessionError::Declined(Declined {
            code: "too_large",
            message: format!(
                "a value in this table is {longest} bytes, past the grid limit ({} bytes), so \
                 it opens as text",
                config.grid_max_value_len
            ),
        }));
    }
    Ok(())
}

/// The facets an edit touches (K-BR-12's table, per op): a map op its key's value; an
/// insert its anchor's position and existence; a move the element's position and
/// existence; a delete its existence and position; a restore its existence; a set its
/// value. A table row's (column's) delete also touches the cells it hides (added by the
/// caller, who knows the cells). A table's key counters are no facet: see [`facets_of`].
/// Every row key `edits` name, once: the row a list op acts on, its anchor, a cell's row, a
/// quote exception's row.
fn rows_named(edits: &[Edit]) -> Vec<Key> {
    let mut out: Vec<Key> = Vec::new();
    let mut add = |k: Key| {
        if !out.contains(&k) {
            out.push(k);
        }
    };
    for e in edits {
        match e {
            Edit::Map { root, ops } if root == table::CELLS || root == table::QUOTING => {
                for o in ops {
                    if let Some((r, _)) = table::parse_cell_key(o.key()) {
                        add(r);
                    }
                }
            }
            Edit::List { root, ops } if root == table::ROWS => {
                for o in ops {
                    match o {
                        ListOp::Insert { at, .. } => {
                            if let Anchor::After(a) | Anchor::Before(a) = at {
                                add(a.clone());
                            }
                        }
                        ListOp::Move { key, to } => {
                            add(key.clone());
                            if let Anchor::After(a) | Anchor::Before(a) = to {
                                add(a.clone());
                            }
                        }
                        ListOp::Set { key, .. }
                        | ListOp::Delete { key }
                        | ListOp::Restore { key }
                        | ListOp::Drop { key } => add(key.clone()),
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The row keys a change's record touches.
pub(super) fn rows_of_elems(elems: &[crate::store::ElemRoot]) -> Vec<Key> {
    let mut out: Vec<Key> = Vec::new();
    for r in elems {
        for o in r.ops.iter().filter(|o| o.effect != StoredEffect::None) {
            let k = if r.root == table::ROWS {
                Key::new(&o.key).ok()
            } else if r.root == table::CELLS {
                table::parse_cell_key(&o.key).map(|(r, _)| r)
            } else {
                None
            };
            if let Some(k) = k {
                if !out.contains(&k) {
                    out.push(k);
                }
            }
        }
    }
    out
}

/// The facets `edits` touch (the table above). In a table (`is_table`), `meta.next_row` and
/// `meta.next_col` are left out: they are bookkeeping, not content. Minting skips every key
/// the axis knows, whatever the counter says, and a merge takes the higher one, so a counter
/// written after the base can't make an edit wrong.
pub(super) fn facets_of(edits: &[Edit], is_table: bool) -> Vec<(String, String, Facet)> {
    let mut out = Vec::new();
    for e in edits {
        match e {
            Edit::Text { .. } => {}
            Edit::Map { root, ops } => {
                for o in ops {
                    if is_table && root == table::META && counter(o.key()) {
                        continue;
                    }
                    out.push((root.clone(), o.key().to_owned(), Facet::Value));
                }
            }
            Edit::List { root, ops } => {
                for o in ops {
                    let mut add = |k: &Key, f: Facet| out.push((root.clone(), k.to_string(), f));
                    match o {
                        ListOp::Insert { at, .. } => {
                            if let Anchor::After(a) | Anchor::Before(a) = at {
                                add(a, Facet::Position);
                                add(a, Facet::Existence);
                            }
                        }
                        ListOp::Move { key, .. } => {
                            add(key, Facet::Position);
                            add(key, Facet::Existence);
                        }
                        ListOp::Delete { key } => {
                            add(key, Facet::Existence);
                            add(key, Facet::Position);
                        }
                        ListOp::Restore { key } | ListOp::Drop { key } => {
                            add(key, Facet::Existence)
                        }
                        ListOp::Set { key, .. } => add(key, Facet::Value),
                    }
                }
            }
        }
    }
    let mut seen = HashSet::new();
    out.retain(|x| seen.insert(x.clone()));
    out
}

fn value_text(v: &Option<Value>) -> String {
    match v {
        None => "absent".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Str(s)) => format!("{s:?}"),
    }
}

/// The edits that make `live` hold `target` (a version's content) **by identity** (K-HIST-8,
/// contract §5.2): per list root, revive the version's keys that are deleted now, delete the
/// live keys it doesn't have, re-insert its keys the list no longer knows (dropped since),
/// then the minimal moves (every element outside a longest run already in the version's
/// order, placed right after its left neighbour, left to right: forward chains), then the
/// values; per map root, the sets and deletes that differ. A table's key counters never go
/// back (a dropped key is never minted again).
pub(super) fn restore_edits(
    live: &KeyedState,
    target: &[(String, Neutral)],
    is_table: bool,
) -> Vec<Edit> {
    let mut out = Vec::new();
    for (root, cur) in &live.roots {
        let Some((_, want)) = target.iter().find(|(r, _)| r == root) else {
            continue;
        };
        match (cur, want) {
            (
                KeyedRoot::List {
                    live: l,
                    hidden: h,
                    places,
                },
                Neutral::List(t),
            ) => {
                let mut ops = Vec::new();
                let tkeys: HashSet<&Key> = t.iter().map(|(k, _)| k).collect();
                let lkeys: HashSet<&Key> = l.iter().map(|(k, _)| k).collect();
                for (k, _) in t {
                    if h.contains_key(k) {
                        ops.push(ListOp::Restore { key: k.clone() });
                    }
                }
                for (k, _) in l {
                    if !tkeys.contains(k) {
                        ops.push(ListOp::Delete { key: k.clone() });
                    }
                }
                let mut inserted = Vec::new();
                for (k, v) in t {
                    if !lkeys.contains(k) && !h.contains_key(k) {
                        ops.push(ListOp::Insert {
                            key: k.clone(),
                            at: Anchor::End,
                            value: v.clone(),
                        });
                        inserted.push(k.clone());
                    }
                }
                // the live order now: places (deleted ones where they sit) kept to the
                // version's keys, then the re-inserted at the end
                let exact = places.len() == l.len() + h.len();
                let base: Vec<&Key> = if exact {
                    places.iter().collect()
                } else {
                    l.iter().map(|(k, _)| k).chain(h.keys()).collect()
                };
                let now: Vec<Key> = base
                    .into_iter()
                    .filter(|k| tkeys.contains(*k))
                    .cloned()
                    .chain(inserted.iter().cloned())
                    .collect();
                let t_index: HashMap<&Key, usize> =
                    t.iter().enumerate().map(|(i, (k, _))| (k, i)).collect();
                let seq: Vec<usize> = now.iter().map(|k| t_index[k]).collect();
                let stay: HashSet<&Key> = crate::elem::lis(&seq)
                    .into_iter()
                    .map(|i| &now[i])
                    .collect();
                for (i, (k, _)) in t.iter().enumerate() {
                    if stay.contains(k) {
                        continue;
                    }
                    let to = match i {
                        0 => Anchor::Start,
                        _ => Anchor::After(t[i - 1].0.clone()),
                    };
                    ops.push(ListOp::Move { key: k.clone(), to });
                }
                let vals: HashMap<&Key, &Value> = l.iter().map(|(k, v)| (k, v)).chain(h).collect();
                for (k, v) in t {
                    if let Some(old) = vals.get(k) {
                        if *old != v {
                            ops.push(ListOp::Set {
                                key: k.clone(),
                                value: v.clone(),
                            });
                        }
                    }
                }
                if !ops.is_empty() {
                    out.push(Edit::List {
                        root: root.clone(),
                        ops,
                    });
                }
            }
            (KeyedRoot::Map(m), Neutral::Map(t)) => {
                let mut ops = Vec::new();
                for k in m.keys() {
                    if !t.contains_key(k) {
                        if is_table && root == table::META && counter(k) {
                            continue;
                        }
                        ops.push(MapOp::Delete { key: k.clone() });
                    }
                }
                for (k, v) in t {
                    if m.get(k) == Some(v) {
                        continue;
                    }
                    if is_table && root == table::META && counter(k) {
                        let n = |x: Option<&Value>| {
                            x.and_then(Value::as_str)
                                .and_then(|s| s.parse::<u64>().ok())
                                .unwrap_or(0)
                        };
                        if n(Some(v)) <= n(m.get(k)) {
                            continue;
                        }
                    }
                    ops.push(MapOp::Set {
                        key: k.clone(),
                        value: v.clone(),
                    });
                }
                if !ops.is_empty() {
                    out.push(Edit::Map {
                        root: root.clone(),
                        ops,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// Spec v1.10 S12: a version's rows and columns the live table no longer knows (`drop-deleted`
/// removed them) come back under **fresh keys**, minted from the live counters, with their
/// cells, names and quote exceptions re-keyed and the counters written past them: a key is
/// never reused in its causal past (K-CSV-10c; native's per-op check refuses it). Returns the
/// mapping, old key to fresh key.
fn rekey_dropped(live: &KeyedState, content: &mut [(String, Neutral)]) -> Result<Vec<(Key, Key)>> {
    let mut t = Table::from_keyed(live).map_err(|e| SessionError::BadEdit(e.to_string()))?;
    let mut rows: HashMap<Key, Key> = HashMap::new();
    let mut cols: HashMap<Key, Key> = HashMap::new();
    for (r, n) in content.iter() {
        let Neutral::List(l) = n else { continue };
        for (k, _) in l {
            if r == table::ROWS && !t.rows.knows(k) && !rows.contains_key(k) {
                let f = t.mint_row();
                rows.insert(k.clone(), f);
            } else if r == table::COLS && !t.cols.knows(k) && !cols.contains_key(k) {
                let f = t.mint_col();
                cols.insert(k.clone(), f);
            }
        }
    }
    if rows.is_empty() && cols.is_empty() {
        return Ok(Vec::new());
    }
    let rk = |k: &Key| rows.get(k).cloned().unwrap_or_else(|| k.clone());
    let ck = |k: &Key| cols.get(k).cloned().unwrap_or_else(|| k.clone());
    for (r, n) in content.iter_mut() {
        match (r.as_str(), n) {
            (table::ROWS, Neutral::List(l)) => l.iter_mut().for_each(|(k, _)| *k = rk(k)),
            (table::COLS, Neutral::List(l)) => l.iter_mut().for_each(|(k, _)| *k = ck(k)),
            (table::CELLS, Neutral::Map(m)) => {
                *m = std::mem::take(m)
                    .into_iter()
                    .map(|(k, v)| match table::parse_cell_key(&k) {
                        Some((a, b)) => (table::cell_key(&rk(&a), &ck(&b)), v),
                        None => (k, v),
                    })
                    .collect();
            }
            (table::NAMES, Neutral::Map(m)) => {
                *m = std::mem::take(m)
                    .into_iter()
                    .map(|(k, v)| match Key::new(k.as_str()) {
                        Ok(c) => (ck(&c).to_string(), v),
                        Err(_) => (k, v),
                    })
                    .collect();
            }
            (table::QUOTING, Neutral::Map(m)) => {
                *m = std::mem::take(m)
                    .into_iter()
                    .map(|(k, v)| match table::QuoteAt::parse(&k) {
                        Some(at) => (at.renamed(&rk, &ck).map_key(), v),
                        None => (k, v),
                    })
                    .collect();
            }
            (table::META, Neutral::Map(m)) => {
                m.insert("next_row".into(), Value::str(t.next_row.to_string()));
                m.insert("next_col".into(), Value::str(t.next_col.to_string()));
            }
            _ => {}
        }
    }
    let mut out: Vec<(Key, Key)> = rows.into_iter().chain(cols).collect();
    out.sort();
    Ok(out)
}

/// A table's key counter (`meta.next_row`, `next_col`): never set backwards.
fn counter(k: &str) -> bool {
    k == "next_row" || k == "next_col"
}

impl Session {
    // ----- edits -----

    /// Structured edits (one change): text edits can't be mixed in, and dropping deleted
    /// elements is the owner's [`Session::drop_deleted`].
    pub(super) fn normalize_structured(&self, edits: Vec<Edit>) -> Result<Vec<Edit>> {
        let mut out = Vec::with_capacity(edits.len());
        for e in edits {
            match e {
                Edit::Text { root, .. } => {
                    return Err(SessionError::BadEdit(format!(
                        "root {root:?}: one change edits text roots or structured roots, not both"
                    )))
                }
                Edit::List { root, ops } => {
                    if ops.iter().any(|o| matches!(o, ListOp::Drop { .. })) {
                        return Err(SessionError::BadEdit(
                            "dropping deleted elements is the owner's `rows drop-deleted`".into(),
                        ));
                    }
                    if !ops.is_empty() {
                        out.push(Edit::List { root, ops });
                    }
                }
                Edit::Map { root, ops } => {
                    if !ops.is_empty() {
                        out.push(Edit::Map { root, ops });
                    }
                }
            }
        }
        Ok(out)
    }

    /// Apply a structured change to the doc (nothing logged here): the update, its records
    /// by key, its counts, the applied stream, and the cells it wrote into deleted rows.
    pub(super) fn apply_structured(
        &self,
        st: &mut State,
        i: usize,
        edits: Vec<Edit>,
        restores: ElemRestores,
    ) -> Result<Option<Applied>> {
        #[cfg(any(test, feature = "differential"))]
        {
            use crate::differential::ApplyFault;
            match crate::differential::apply_fault() {
                Some(ApplyFault::Refused) => {
                    return Err(SessionError::BadEdit("a planted refusal".into()))
                }
                Some(ApplyFault::Poisoned) => {
                    return Err(crate::doc::DocError::Pending.into());
                }
                Some(ApplyFault::NoChange) => return Ok(None),
                _ => {}
            }
        }
        let client = st.members[i].client;
        // the axes as they were, for what it touches; its owner's own change ends a grid claim
        // (A10), marked below, only once it has landed
        let axes = self.touch_axes(st)?;
        #[cfg(any(test, feature = "differential"))]
        if crate::differential::mark_mutant()
            == Some(crate::differential::MarkMutant::GridMarkBeforeLanding)
        {
            let named = self.grid_touched(st, i, &edits)?;
            self.mark_grid_claims(st, i, &named);
        }
        self.planted_early_elem_marks(st, i, &edits);
        let befores = befores_of(&st.doc, &edits)?;
        let made = st.doc.edit_applied(client, &edits)?;
        let Some(update) = made.update else {
            return Ok(None);
        };
        let touched = self.grid_landed(st, i, &made.applied, &edits, axes.as_ref());
        let landed = self.elems_landed(st, &made.applied);
        let writer = st.members[i].p.pid.clone();
        let applied = self.structured_applied(
            st,
            Made {
                writer: &writer,
                update,
                stream: made.applied,
            },
            &edits,
            &befores,
            &restores,
        );
        #[cfg(any(test, feature = "differential"))]
        let applied = if crate::differential::take_bookkeeping_fault() {
            Err(crate::store::StoreError::Db("a planted failure".into()).into())
        } else {
            applied
        };
        let applied = match applied {
            Ok(a) => a,
            // the doc holds the change and it will never reach the store: reload
            Err(e) => {
                self.recover(st, &e.to_string());
                return Err(e);
            }
        };
        self.mark_grid_claims(st, i, &touched);
        // the element claims it touched end once it is durable, marked only now it has landed; a
        // grid claim and an element claim are never the same claim, and a table takes no element
        // claims
        self.mark_elem_claims(st, i, &landed);
        Ok(Some(applied))
    }

    /// The bookkeeping of a structured change the doc has applied.
    fn structured_applied(
        &self,
        st: &State,
        made: Made<'_>,
        edits: &[Edit],
        befores: &Befores,
        restores: &ElemRestores,
    ) -> Result<Applied> {
        let Made {
            writer,
            update,
            stream,
        } = made;
        let (mut elems, counts) = elem_records(&stream, edits, befores);
        for r in &mut elems {
            for o in &mut r.ops {
                o.restores = restores
                    .get(&(r.root.clone(), o.key.clone(), o.kind.facet()))
                    .copied();
            }
        }
        let touched = stream
            .iter()
            .map(|a| match a {
                ElemApplied::List { ops, .. } => ops
                    .iter()
                    .filter(|o| !matches!(o.effect, Effect::None { .. }))
                    .count(),
                _ => 0,
            })
            .sum();
        let hidden = self.hidden_writes(st, &stream, writer)?;
        Ok(Applied {
            update,
            span: None,
            widened: false,
            roots: Vec::new(),
            regions: Vec::new(),
            record: Vec::new(),
            prov_rows: Vec::new(),
            kind: ChangeKind::Edit,
            of: None,
            changed: Vec::new(),
            added: 0,
            removed: 0,
            last_end: None,
            elems,
            counts,
            stream,
            touched,
            hidden,
        })
    }

    /// The cells `stream` wrote into deleted rows or columns (a table), each with who
    /// deleted the row (column) from `ELEM_WRITES`.
    fn hidden_writes(
        &self,
        st: &State,
        stream: &[ElemApplied],
        writer: &str,
    ) -> Result<Vec<HiddenCellWrite>> {
        if !table::is_table(&self.schema) {
            return Ok(Vec::new());
        }
        let found = table::hidden_cell_writes(stream, |root, k| {
            matches!(st.doc.elem(root, k), Ok(ElemState::Live(_)))
        });
        let store = self.store(st)?;
        let mut out = Vec::with_capacity(found.len());
        let deleter = |root: &str, key: &Key| -> Result<(Option<String>, Option<Seq>)> {
            let w = store.elem_write(self.doc_id, root, key.as_str(), ElemFacet::Existence)?;
            let by = match &w {
                Some(w) => store.change(self.doc_id, w.seq)?.map(|c| c.author),
                None => None,
            };
            Ok((by, w.map(|w| w.seq)))
        };
        for h in found {
            let row_del = if h.row_hidden {
                Some(deleter(table::ROWS, &h.row)?)
            } else {
                None
            };
            let col_del = if h.col_hidden {
                Some(deleter(table::COLS, &h.col)?)
            } else {
                None
            };
            let (by, seq) = row_del
                .clone()
                .or_else(|| col_del.clone())
                .unwrap_or((None, None));
            let message = hidden_write_message(
                &h.row,
                &h.col,
                writer,
                row_del.as_ref().map(|d| d.0.as_deref()),
                col_del.as_ref().map(|d| d.0.as_deref()),
            );
            out.push(HiddenCellWrite {
                row: h.row,
                col: h.col,
                row_hidden: h.row_hidden,
                col_hidden: h.col_hidden,
                deleted_by: by,
                deleted_in: seq,
                message,
                parts: HiddenParts {
                    writer: writer.to_owned(),
                    row_deleter: row_del.map(|d| d.0),
                    col_deleter: col_del.map(|d| d.0),
                },
            });
        }
        Ok(out)
    }

    /// A table's present cells, columns and rows (live or deleted), counted on first need.
    fn grid_size(&self, st: &mut State) -> Result<(usize, usize, usize)> {
        if let Some(n) = st.grid_size {
            return Ok(n);
        }
        let k = st.doc.keyed_state()?;
        let mut n = (0, 0, 0);
        for (r, x) in &k.roots {
            match x {
                KeyedRoot::Map(m) if r == table::CELLS => n.0 = m.len(),
                KeyedRoot::List { live, hidden, .. } if r == table::COLS => {
                    n.1 = live.len() + hidden.len()
                }
                KeyedRoot::List { live, hidden, .. } if r == table::ROWS => {
                    n.2 = live.len() + hidden.len()
                }
                _ => {}
            }
        }
        st.grid_size = Some(n);
        Ok(n)
    }

    /// Keep the counted size in step with a change's stream.
    pub(super) fn grid_size_after(&self, st: &mut State, stream: &[ElemApplied]) {
        let Some((mut cells, mut cols, mut rows)) = st.grid_size else {
            return;
        };
        for a in stream {
            match a {
                ElemApplied::Map { root, ops } if root == table::CELLS => {
                    for o in ops {
                        match (o.kind, &o.before, &o.after) {
                            (MapOpKind::Set, None, Some(_)) => cells += 1,
                            (MapOpKind::Delete, Some(_), None) => cells = cells.saturating_sub(1),
                            _ => {}
                        }
                    }
                }
                ElemApplied::List { root, ops } if root == table::COLS || root == table::ROWS => {
                    let n = if root == table::COLS {
                        &mut cols
                    } else {
                        &mut rows
                    };
                    for o in ops
                        .iter()
                        .filter(|o| !matches!(o.effect, Effect::None { .. }))
                    {
                        match o.kind {
                            ListOpKind::Insert => *n += 1,
                            ListOpKind::Drop => *n = n.saturating_sub(1),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        st.grid_size = Some((cells, cols, rows));
    }

    /// K-CSV-7 / KC4: refuse a change that takes a table past a limit (and grows it there);
    /// a change that doesn't grow it always passes, so nothing is ever trapped.
    fn check_limits(&self, st: &mut State, edits: &[Edit]) -> Result<()> {
        if !table::is_table(&self.schema) {
            return Ok(());
        }
        // a value or name past the limit (C1k: `Config::grid_max_value_len`)
        let max = self.config.grid_max_value_len;
        for e in edits {
            if let Edit::Map { root, ops } = e {
                if root != table::CELLS && root != table::NAMES {
                    continue;
                }
                for o in ops {
                    if let MapOp::Set { value, .. } = o {
                        let len = value.as_str().map_or(0, str::len);
                        if len > max {
                            return Err(SessionError::AtLimit {
                                what: "value bytes",
                                limit: max,
                                would_be: len,
                            });
                        }
                    }
                }
            }
        }
        let (cells, cols, rows) = self.grid_size(st)?;
        let mut present: HashMap<&str, bool> = HashMap::new();
        let (mut c, mut k, mut n) = (cells as i64, cols as i64, rows as i64);
        for e in edits {
            match e {
                Edit::Map { root, ops } if root == table::CELLS => {
                    for o in ops {
                        let key = o.key();
                        let was = match present.get(key) {
                            Some(p) => *p,
                            None => st.doc.map_get(table::CELLS, key)?.is_some(),
                        };
                        let now = matches!(o, MapOp::Set { .. });
                        c += i64::from(now) - i64::from(was);
                        present.insert(key, now);
                    }
                }
                Edit::List { root, ops } if root == table::COLS || root == table::ROWS => {
                    let at = if root == table::COLS { &mut k } else { &mut n };
                    for o in ops {
                        match o {
                            ListOp::Insert { .. } => *at += 1,
                            ListOp::Drop { .. } => *at -= 1,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let over = |what, now: i64, before: usize, limit: usize| {
            (now > limit as i64 && now > before as i64).then_some(SessionError::AtLimit {
                what,
                limit,
                would_be: now.max(0) as usize,
            })
        };
        if let Some(e) = over("cells", c, cells, self.config.grid_max_cells) {
            return Err(e);
        }
        if let Some(e) = over("columns", k, cols, self.config.grid_max_cols) {
            return Err(e);
        }
        if let Some(e) = over("rows", n, rows, self.config.grid_max_rows) {
            return Err(e);
        }
        Ok(())
    }

    /// K-BR-12: `--base V` refuses (nothing applied) when a facet the edit touches was
    /// written after V by anyone but the caller, judged by write identity: a write with no
    /// identity (a merged restore or delete on this engine) counts as someone else's (KC7).
    fn check_base(&self, st: &State, i: usize, edits: &[Edit], base: Seq) -> Result<()> {
        let mut facets = facets_of(edits, table::is_table(&self.schema));
        // a row's (column's) delete also touches the values of the cells it hides: a row's
        // cells from the index by key prefix (O(its cells)); a column's from the doc
        let store = self.store(st)?;
        if table::is_table(&self.schema) {
            let mut gone_cols: Vec<&Key> = Vec::new();
            for e in edits {
                let Edit::List { root, ops } = e else {
                    continue;
                };
                for o in ops {
                    let ListOp::Delete { key } = o else {
                        continue;
                    };
                    if root != table::ROWS {
                        gone_cols.push(key);
                        continue;
                    }
                    let prefix = format!("{key}{}", table::CELL_SEP);
                    for (k, f, _) in
                        store.elem_writes_prefixed(self.doc_id, table::CELLS, &prefix)?
                    {
                        if f == ElemFacet::Value {
                            facets.push((table::CELLS.to_owned(), k, Facet::Value));
                        }
                    }
                }
            }
            if !gone_cols.is_empty() {
                for k in st.doc.map(table::CELLS)?.keys() {
                    if table::parse_cell_key(k).is_some_and(|(_, c)| gone_cols.contains(&&c)) {
                        facets.push((table::CELLS.to_owned(), k.clone(), Facet::Value));
                    }
                }
            }
        }
        let pid = &st.members[i].p.pid;
        let mut authors: HashMap<Seq, Option<String>> = HashMap::new();
        let mut out = Vec::new();
        for (root, key, facet) in facets {
            let Some(w) = store.elem_write(self.doc_id, &root, &key, ElemFacet::of(facet))? else {
                continue;
            };
            if w.seq <= base {
                continue;
            }
            let by = match authors.get(&w.seq) {
                Some(a) => a.clone(),
                None => {
                    let a = store.change(self.doc_id, w.seq)?.map(|c| c.author);
                    authors.insert(w.seq, a.clone());
                    a
                }
            };
            let mine = w.write.is_some() && by.as_deref() == Some(pid.as_str());
            if mine {
                continue;
            }
            let now = self.facet_now(st, &root, &key, facet)?;
            out.push(FacetConflict {
                root,
                key,
                facet,
                by,
                change: w.seq,
                now,
                rewritten_row: false,
            });
        }
        if out.is_empty() {
            Ok(())
        } else {
            Err(SessionError::FacetConflict(out))
        }
    }

    /// A keyed write as of version `seen` names a row a text re-paired since (every key it
    /// names: the row, a cell's row, an anchor): refused as `edited_row`, whoever wrote the
    /// text. Pairing a text's lines with keys is a guess, so the key may hold another row's
    /// content now, whatever the facets say.
    fn check_marks(&self, st: &State, edits: &[Edit], seen: Seq) -> Result<()> {
        if !table::is_table(&self.schema) {
            return Ok(());
        }
        let store = self.store(st)?;
        let mut out = Vec::new();
        if let Some(change) = store.col_remap(self.doc_id)?.filter(|c| *c > seen) {
            return Err(SessionError::FacetConflict(vec![FacetConflict {
                root: table::COLS.to_owned(),
                key: String::new(),
                facet: Facet::Value,
                by: store.change(self.doc_id, change)?.map(|c| c.author),
                change,
                now: String::new(),
                rewritten_row: true,
            }]));
        }
        for k in rows_named(edits) {
            let Some(change) = store.row_mark(self.doc_id, k.as_str())? else {
                continue;
            };
            if change > seen {
                out.push(FacetConflict {
                    root: table::ROWS.to_owned(),
                    key: k.to_string(),
                    facet: Facet::Value,
                    by: store.change(self.doc_id, change)?.map(|c| c.author),
                    change,
                    now: String::new(),
                    rewritten_row: true,
                });
            }
        }
        if out.is_empty() {
            Ok(())
        } else {
            Err(SessionError::FacetConflict(out))
        }
    }

    /// What a facet holds now, in words (a conflict's report).
    fn facet_now(&self, st: &State, root: &str, key: &str, facet: Facet) -> Result<String> {
        Ok(match self.schema.kind_of(root) {
            Some(Kind::Map) => value_text(&st.doc.map_get(root, key)?),
            Some(Kind::MovableList) => {
                let k = Key::new(key).map_err(|e| SessionError::BadEdit(e.to_string()))?;
                match facet {
                    Facet::Value => value_text(&st.doc.elem_value(root, &k)?),
                    _ => match st.doc.elem(root, &k)? {
                        ElemState::Live(i) => format!("live at #{}", i + 1),
                        ElemState::Deleted => "deleted".to_owned(),
                        ElemState::Unknown => "unknown".to_owned(),
                    },
                }
            }
            _ => String::new(),
        })
    }

    /// The structured edit's checks, in order, before anything applies: an agent names a
    /// base (K-BR-12), inserts into a tagged root carry the writer's own tag
    /// ([`Pack::tagged_keys`]), the base per facet, the size limits (K-CSV-7), the claim policy on
    /// grid claims (A10) and element claims, and a value naming a blob names one the doc's space
    /// holds ([`Pack::blob_ref_roots`]).
    fn structured_guards(
        &self,
        st: &mut State,
        i: usize,
        edits: &[Edit],
        opts: &EditOptions,
        now: u64,
    ) -> Result<()> {
        if st.members[i].p.kind == ParticipantKind::Agent
            && opts.base.is_none()
            && opts.if_version.is_none()
        {
            return Err(SessionError::NeedsBase);
        }
        self.check_tagged_keys(st, i, edits)?;
        self.check_cell_keys(st, edits)?;
        if let Some(b) = opts.base {
            self.check_marks(st, edits, b)?;
            self.check_base(st, i, edits, b)?;
        }
        if let Some(s) = opts.seen {
            self.check_marks(st, edits, s)?;
        }
        self.check_limits(st, edits)?;
        self.check_grid_claims(st, i, edits, opts.policy, now)?;
        self.check_blob_refs(st, edits)
    }

    /// A table's cell writes name rows and columns it knows (live or deleted), or ones the
    /// edit itself inserts: a cell under a key the table doesn't know (dropped, or never in it)
    /// could never be seen, restored or swept, so the whole edit is refused
    /// ([`SessionError::UnknownElem`]), naming the drop when the store still holds its record.
    /// A deleted key the table knows stays a hidden write (csv contract §4.3, K-CSV-4).
    fn check_cell_keys(&self, st: &State, edits: &[Edit]) -> Result<()> {
        if !table::is_table(&self.schema) || planted_keyed_write_under_unknown_key() {
            return Ok(());
        }
        let inserted = |root: &str, k: &Key| {
            edits.iter().any(|e| match e {
                Edit::List { root: r, ops } if r == root => ops
                    .iter()
                    .any(|o| matches!(o, ListOp::Insert { key, .. } if key == k)),
                _ => false,
            })
        };
        let mut t: Option<Table> = None;
        for e in edits {
            let Edit::Map { root, ops } = e else {
                continue;
            };
            if root != table::CELLS {
                continue;
            }
            for o in ops {
                let MapOp::Set { key, .. } = o else {
                    continue;
                };
                let Some((r, c)) = table::parse_cell_key(key) else {
                    continue;
                };
                if t.is_none() {
                    t = Some(
                        Table::from_keyed(&st.doc.keyed_state()?)
                            .map_err(|e| SessionError::BadEdit(e.to_string()))?,
                    );
                }
                let Some(t) = t.as_ref() else { continue };
                for (axis, k, known) in [
                    (table::ROWS, &r, t.rows.knows(&r)),
                    (table::COLS, &c, t.cols.knows(&c)),
                ] {
                    if !known && !inserted(axis, k) {
                        return Err(SessionError::UnknownElem {
                            root: axis,
                            key: k.to_string(),
                            dropped: self.dropped_by(st, axis, k)?,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// The `rows drop-deleted` that dropped `root`/`k`, from the change records still in the
    /// revert window (`None`: never in the table, or dropped before the records kept).
    fn dropped_by(&self, st: &State, root: &str, k: &Key) -> Result<Option<DroppedBy>> {
        let store = self.store(st)?;
        for (seq, rec) in store.revert_records(self.doc_id)? {
            let hit = rec.elems.iter().any(|r| {
                r.root == root
                    && r.ops
                        .iter()
                        .any(|o| o.kind == ElemOpKind::Drop && o.key == k.as_str())
            });
            if hit {
                return Ok(Some(DroppedBy {
                    change: seq,
                    by: rec.author,
                    at_ms: rec.at_ms,
                }));
            }
        }
        Ok(None)
    }

    /// Apply `edits` (list and map ops by key, or text hunks) through `conn` as one change
    /// (A6). Needs edit. Checked under the lock, in order: the version guard (`if_version`,
    /// doc-wide), the approvals backlog, then for structured edits an agent's base
    /// ([`SessionError::NeedsBase`]), the base per facet ([`SessionError::FacetConflict`]) and
    /// the size limits ([`SessionError::AtLimit`]); for a table, a cell under a row or column
    /// it doesn't know ([`SessionError::UnknownElem`]); for text edits the claim policy. A
    /// refused op ([`DocError::Op`]) refuses the change. Nothing applies on any refusal (there
    /// is no partial mode).
    pub fn edit_ops(
        &self,
        conn: ConnId,
        edits: Vec<Edit>,
        opts: &EditOptions,
    ) -> Result<EditResult> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit_edit(st, conn, Action::Edit, now)?;
        Self::check_version(st, opts.if_version)?;
        self.check_backlog(st, i)?;
        let edits = self.normalize(edits)?;
        if edits.iter().any(|e| !matches!(e, Edit::Text { .. })) {
            self.structured_guards(st, i, &edits, opts, now)?;
        } else {
            for e in &edits {
                if let Edit::Text { root, hunks } = e {
                    self.check_claims(st, i, root, hunks, opts.policy, now)?;
                }
            }
        }
        self.commit_edit(st, i, edits, ChangeKind::Edit, None, now, Vec::new())
    }

    /// Read live, compute, apply, atomically (A6): `f` gets the doc as a [`View`] under the
    /// session lock and returns the edits (none: nothing applies, `None`). Checks as
    /// [`Session::edit_ops`]. `f` must be bounded and must not call back into the kit; its
    /// error is [`EditWithError::Compute`], a panic is passed on after the lock is released.
    pub fn edit_ops_with<E>(
        &self,
        conn: ConnId,
        opts: &EditOptions,
        f: impl FnOnce(&View) -> std::result::Result<Vec<Edit>, E>,
    ) -> std::result::Result<Option<EditResult>, EditWithError<E>> {
        self.edit_ops_impl(conn, opts, None, false, |v, _, _| f(v))
            .map(|(r, _)| r)
    }

    /// [`Session::edit_ops_with`] for a pack's structured verb (A9, through `run_tool`): the
    /// change is logged under `verb`, and the heads-up notes on the elements it touches
    /// (others' claims, carets, views, recent edits there) come back with it. `at_base`: the
    /// verb names positions (`#N`, `LN`), which count in the `--base` version (K-ADDR-3,
    /// R-1): `f` also gets that version's doc when it isn't the live one (the live doc, a
    /// recent read or a stored version; none of them: [`SessionError::BaseExpired`]).
    pub(crate) fn edit_ops_verb<E>(
        &self,
        conn: ConnId,
        opts: &EditOptions,
        verb: &str,
        at_base: bool,
        f: impl FnOnce(&View, Option<&View>, &dyn VerbHistory) -> std::result::Result<Vec<Edit>, E>,
    ) -> std::result::Result<(Option<EditResult>, Notes), EditWithError<E>> {
        self.edit_ops_impl(conn, opts, Some(verb), at_base, f)
    }

    fn edit_ops_impl<E>(
        &self,
        conn: ConnId,
        opts: &EditOptions,
        verb: Option<&str>,
        at_base: bool,
        f: impl FnOnce(&View, Option<&View>, &dyn VerbHistory) -> std::result::Result<Vec<Edit>, E>,
    ) -> std::result::Result<(Option<EditResult>, Notes), EditWithError<E>> {
        let now = self.now();
        let mut guard = self.lock()?;
        let st = &mut *guard;
        let i = self.admit_edit(st, conn, Action::Edit, now)?;
        Self::check_version(st, opts.if_version)?;
        self.check_backlog(st, i)?;
        let at = match opts.base {
            Some(b) if at_base && b != st.version => match self.base_view(st, b, now)? {
                Some(v) => Some(v),
                None => return Err(SessionError::BaseExpired { base: b }.into()),
            },
            _ => None,
        };
        let view = View::new(st.doc.keyed_state().map_err(SessionError::from)?)
            .with_codec(&st.codec)
            .with_max_value(self.config.grid_max_value_len);
        let hist = LockedHistory::new(self, st, i);
        let edits = match catch_unwind(AssertUnwindSafe(|| f(&view, at.as_deref(), &hist))) {
            Ok(r) => r,
            Err(payload) => {
                drop(guard);
                resume_unwind(payload);
            }
        };
        if let Some(h) = hist.handles() {
            st.handles = h;
        }
        let edits = edits.map_err(EditWithError::Compute)?;
        let edits = self.normalize(edits)?;
        if edits.is_empty() {
            return Ok((None, Notes::default()));
        }
        if edits.iter().any(|e| !matches!(e, Edit::Text { .. })) {
            self.structured_guards(st, i, &edits, opts, now)?;
        } else {
            for e in &edits {
                if let Edit::Text { root, hunks } = e {
                    self.check_claims(st, i, root, hunks, opts.policy, now)?;
                }
            }
        }
        let touched = verb.map(|_| edits.clone());
        if let Some(v) = verb {
            st.verb_hint = Some(v.to_owned());
        }
        let v = st.version;
        let res = self.commit_edit(st, i, edits, ChangeKind::Edit, None, now, Vec::new())?;
        let notes = match &touched {
            Some(e) => self.grid_notes_for_edits(st, i, e, now),
            None => Notes::default(),
        };
        Ok(((res.version != v).then_some(res), notes))
    }

    /// A pack's structured read verb (A9): `f` reads the live doc as a [`View`] under the
    /// lock, after the read is recorded (a read base, so `write --base` from its version
    /// works, and "contributors since last read"). Needs read.
    pub(crate) fn read_view_with<R>(
        &self,
        conn: ConnId,
        verb: &str,
        for_edit: Option<Option<u64>>,
        f: impl FnOnce(&View, &dyn VerbHistory) -> (R, Option<ClaimSpan>, bool),
    ) -> Result<(ReadResult, R, Notes, Option<awareness::ForEdit>)> {
        let now = self.now();
        let mut guard = self.lock()?;
        let st = &mut *guard;
        let i = self.admit(st, conn, Need::Read, Action::Read, now)?;
        let read = self.read_unmarked(st, i)?;
        let view = View::new(st.doc.keyed_state()?)
            .with_codec(&st.codec)
            .with_max_value(self.config.grid_max_value_len);
        let hist = LockedHistory::new(self, st, i);
        let (r, shown, refused) = match catch_unwind(AssertUnwindSafe(|| f(&view, &hist))) {
            Ok(r) => r,
            Err(payload) => {
                drop(guard);
                resume_unwind(payload);
            }
        };
        if let Some(h) = hist.handles() {
            st.handles = h;
        }
        if refused_read(refused) {
            return Ok((read, r, Notes::default(), None));
        }
        self.mark_read(st, i, read.version, now);
        self.expire_claims(st, now);
        // the rows or cells shown, by key: --for-edit's claim, and the notes about those rows
        let span = if self.is_grid() {
            shown.as_ref().and_then(GridSpan::of)
        } else {
            None
        };
        let for_edit = for_edit.map(|ttl| self.grid_for_edit(st, i, span.clone(), ttl, verb, now));
        let notes = match span.as_ref().and_then(|s| s.rows.as_ref()) {
            Some(r) => self.grid_notes_for_rows(st, i, r, now),
            None => Notes::default(),
        };
        self.log_activity(st, i, ActivityKind::Read, verb, None, now, None);
        Ok((read, r, notes, for_edit))
    }

    /// `rows drop-deleted` (K-CSV-7): the doc's **owner** removes every deleted row and
    /// column, with the cells, names and quote exceptions keyed by them, from the live sheet
    /// ([`Table::drop_deleted`]), as one attributed change, audited
    /// ([`AuditEvent::DroppedDeleted`]). They stop counting toward the size limit and can't
    /// be restored from the sheet; versions and change records keep them (not erasure:
    /// purge is). Anyone else: [`SessionError::Forbidden`] with [`Need::Owner`] (exit 3).
    pub fn drop_deleted(&self, conn: ConnId) -> Result<EditResult> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit_edit(st, conn, Action::DropDeleted, now)?;
        if !st.members[i].rights.owner {
            self.emit(AuditEvent::Refused {
                key: st.key.clone(),
                pid: st.members[i].p.pid.clone(),
                action: Action::DropDeleted,
                need: Need::Owner,
            });
            return Err(SessionError::Forbidden { need: Need::Owner });
        }
        if !table::is_table(&self.schema) {
            return Err(SessionError::BadEdit(
                "rows drop-deleted is for tables".into(),
            ));
        }
        let mut t = Table::from_keyed(&st.doc.keyed_state()?)
            .map_err(|e| SessionError::BadEdit(e.to_string()))?;
        let (rows, cols) = (t.rows.deleted().count(), t.cols.deleted().count());
        let before = t.cells.len();
        // what each dropped row held, for its cells' authors: it can't come back
        let mut dropped: Vec<table::Conflict> = Vec::new();
        for k in t.rows.deleted() {
            let mut held: Vec<String> = Vec::new();
            let mut writes = Vec::new();
            for ((r, c), v) in &t.cells {
                if r == k {
                    held.push(v.clone());
                    writes.push(table::Region::Cell {
                        row: r.clone(),
                        col: c.clone(),
                    });
                }
            }
            if held.is_empty() {
                continue;
            }
            dropped.push(table::Conflict {
                rule: table::ConflictRule::EditedRow,
                at: table::Region::Row(k.clone()),
                base: table::Held::Value(None),
                ours: table::Held::Value(None),
                theirs: table::Held::Value(Some(held.join(","))),
                theirs_writes: writes,
            });
        }
        let before_drop = t.clone();
        // and each dropped column's cells in rows that stay (a dropped row's are above)
        for c in t.cols.deleted() {
            let mut held: Vec<String> = Vec::new();
            let mut writes = Vec::new();
            for ((r, x), v) in &t.cells {
                if x == c && !t.rows.deleted().any(|d| d == r) {
                    held.push(v.clone());
                    writes.push(table::Region::Cell {
                        row: r.clone(),
                        col: x.clone(),
                    });
                }
            }
            if held.is_empty() {
                continue;
            }
            dropped.push(table::Conflict {
                rule: table::ConflictRule::EditedCol,
                at: table::Region::Col(c.clone()),
                base: table::Held::Value(None),
                ours: table::Held::Value(None),
                theirs: table::Held::Value(Some(held.join(","))),
                theirs_writes: writes,
            });
        }
        let edits = t
            .drop_deleted()
            .map_err(|e| SessionError::BadEdit(e.to_string()))?;
        if edits.is_empty() {
            return Ok(EditResult {
                version: st.version,
                ..EditResult::default()
            });
        }
        let cells = before - t.cells.len();
        let pid = st.members[i].p.pid.clone();
        // one entry per cell, so each author's note holds the values they wrote; then one per
        // element and author again
        let split: Vec<table::Conflict> = dropped
            .iter()
            .flat_map(|c| {
                c.theirs_writes.iter().map(move |w| table::Conflict {
                    theirs_writes: vec![w.clone()],
                    ..c.clone()
                })
            })
            .collect();
        let mut overridden: Vec<Overridden> = Vec::new();
        for o in self.overridden(st, &before_drop, &split)? {
            match overridden
                .iter_mut()
                .find(|x| x.region == o.region && x.by == o.by)
            {
                Some(x) => {
                    x.yours.push_str(", ");
                    x.yours.push_str(&o.yours);
                }
                None => overridden.push(o),
            }
        }
        for o in &mut overridden {
            o.rule = if o.rule == "edited_col" {
                "dropped_col"
            } else {
                "dropped_row"
            };
            o.by.retain(|b| *b != pid);
        }
        let before_seq = st.version;
        let plan = self.plan_overrides(
            st,
            &overridden,
            "rows drop-deleted",
            before_seq,
            before_seq + 1,
            now,
        )?;
        // the version before the drop holds the dropped elements' cells in its cell map (their
        // content stays in history; restoring an element from it arrives in v0.10.1)
        let snap = self.snapshot(st)?;
        let versions = vec![Self::version_of(
            st,
            st.version,
            VersionReason::BeforeDrop,
            None,
            snap,
            now,
        )];
        st.verb_hint = Some("rows drop-deleted".to_owned());
        // on no stack (spec v1.10 S4, S11): a dropped element can't be undone
        st.no_stack = true;
        st.next_note_rows = plan.note_rows();
        let res = self.commit_edit(st, i, edits, ChangeKind::Edit, None, now, versions)?;
        // (its audit event is DroppedDeleted, below: not an override per row)
        self.finish_overrides(st, Vec::new(), plan, before_seq, before_seq + 1)?;
        self.emit(AuditEvent::DroppedDeleted {
            key: st.key.clone(),
            by: st.members[i].p.pid.clone(),
            seq: res.version,
            rows,
            cols,
            cells,
        });
        Ok(res)
    }

    // ----- write --base for grids -----

    /// A structured doc's content at `base`: live, a cached read, or a stored version (no
    /// deleted rows: a version holds the live ones). `None`: expired.
    fn base_view(&self, st: &mut State, base: Seq, now: u64) -> Result<Option<Arc<View>>> {
        if base == st.version {
            return Ok(Some(Arc::new(View::new(st.doc.keyed_state()?))));
        }
        self.prune_read_bases(st, now);
        if let Some(b) = st.read_bases.iter().find(|b| b.version == base) {
            return Ok(b.view.clone());
        }
        let store = self.store(st)?;
        let Some(vid) = store
            .version_infos(self.doc_id)?
            .into_iter()
            .find(|v| v.seq == base && v.redacted.is_empty())
            .map(|v| v.id)
        else {
            return Ok(None);
        };
        let v = store.version(self.doc_id, vid)?;
        Ok(Some(Arc::new(View::of_content(&v.content))))
    }

    /// `write --base V` for a table (contract §3.4, K-BR-3, K7): `body` (the file's bytes as
    /// the caller wants them) is aligned to the table at V ([`super::super::Pack::reconcile`])
    /// and three-way merged by key onto the live table ([`table::merge3`]): only what the
    /// caller changed since V applies, element by element; overlaps are reported with who
    /// last wrote each region, where it is now and its rendered lines, and nothing applies
    /// (with `partial`, the clean part applies and the rest is reported). Valid bases: the
    /// live version, recently read versions, stored versions. Needs edit. Under
    /// [`ClaimPolicy::Respect`] ([`GridWriteOptions::policy`]) an agent's merged change that
    /// touches another participant's grid claim is [`SessionError::Claimed`] (exit 5), nothing
    /// applied.
    pub fn write_base_grid(
        &self,
        conn: ConnId,
        base: Seq,
        body: &[u8],
        opts: &GridWriteOptions,
    ) -> Result<GridWriteReport> {
        timed!(GridWriteBase);
        let out = self.write_base_grid_inner(conn, base, body, opts);
        // an App viewer (K5) sees the others by handle
        self.for_viewer(conn, out, |l, r| l.grid_write(r))
    }

    fn write_base_grid_inner(
        &self,
        conn: ConnId,
        base: Seq,
        body: &[u8],
        opts: &GridWriteOptions,
    ) -> Result<GridWriteReport> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit_edit(st, conn, Action::WriteBase, now)?;
        self.check_backlog(st, i)?;
        if !table::is_table(&self.schema) {
            return Err(SessionError::BadEdit(
                "write --base by key is for tables".into(),
            ));
        }
        let report = |outcome| GridWriteReport {
            outcome,
            skipped: Vec::new(),
            rekeyed: Vec::new(),
        };
        let Some(base_view) = self.base_view(st, base, now)? else {
            return Ok(report(GridWrite::BaseExpired));
        };
        let bad = |e: table::TableError| SessionError::BadEdit(e.to_string());
        let base_t = base_view.table().map_err(bad)?;
        let ours = self
            .pack
            .reconcile(&base_view, body, &st.codec)
            .map_err(SessionError::Declined)?;
        let mut ours_t = base_t.clone();
        ours_t.apply(&ours.edits).map_err(bad)?;
        let live = View::new(st.doc.keyed_state()?);
        let theirs_t = live.table().map_err(bad)?;
        let exact = ours.exact_rows.iter().cloned().collect();
        let m = table::merge3_text(&base_t, &ours_t, &theirs_t, &exact).map_err(bad)?;
        let conflicts =
            self.grid_conflicts(st, i, &live, &theirs_t, (&base_t, &ours_t), m.conflicts)?;
        // a column layout change on either side that conflicts with the text (any conflict at a
        // column, or a `deleted_col` at a cell or name): the layout every row of the text assumes
        // changed, so `--partial` applies nothing of the text either
        let layout_held = conflicts.iter().any(|c| {
            matches!(c.conflict.at, table::Region::Col(_))
                || c.conflict.rule == table::ConflictRule::DeletedCol
        });
        if !conflicts.is_empty() && (!opts.partial || layout_held) {
            return Ok(GridWriteReport {
                outcome: GridWrite::Conflicts(conflicts),
                skipped: Vec::new(),
                rekeyed: m.rekeyed,
            });
        }
        let edits = self.normalize(m.edits)?;
        if edits.is_empty() {
            return Ok(GridWriteReport {
                outcome: GridWrite::Applied(EditResult {
                    version: st.version,
                    ..EditResult::default()
                }),
                skipped: conflicts,
                rekeyed: m.rekeyed,
            });
        }
        self.check_limits(st, &edits)?;
        // the merged change honours claims as every edit does (only what applies is judged)
        self.check_grid_claims(st, i, &edits, opts.policy, now)?;
        st.verb_hint = Some("write --base".to_owned());
        st.next_row_marks = {
            let held: Vec<&Key> = conflicts
                .iter()
                .filter_map(|c| match &c.conflict.at {
                    table::Region::Row(r) | table::Region::Cell { row: r, .. } => Some(r),
                    _ => None,
                })
                .collect();
            st.next_col_remap = !ours.cols_exact;
            kept_not_exact(&base_t, &ours_t, &ours.exact_rows, ours.cols_exact)
                .into_iter()
                .filter(|k| !held.contains(&k))
                .collect()
        };
        let res = self.commit_edit(st, i, edits, ChangeKind::Edit, None, now, Vec::new())?;
        Ok(GridWriteReport {
            outcome: GridWrite::Applied(res),
            skipped: conflicts,
            rekeyed: m.rekeyed,
        })
    }

    /// `write --base live` on a table (contract §3.2): the agent's or person's whole CSV
    /// **replaces** the grid. Reconciled into grid ops by key against the live table
    /// ([`super::Pack::reconcile`]: rows and columns matched by content keep their keys) and
    /// applied as one change, with the size limits and the claim policy of every edit. No
    /// merge, so nothing conflicts: what the body leaves out is deleted (restorable), what it
    /// changes is set. For people (K-BR-5): an agent derives its body from a read and uses
    /// [`Session::write_base_grid`] with that version, so others' changes since are kept.
    ///
    /// `seen` is the version the body was made from: the live doc's, or the write is **stale**
    /// and takes the outside-save path (contract §2.9.1): merged by key on what it saw, the
    /// body's row winning whole over a live change, with the note, the audit event and a
    /// version holding what it overrode. A `seen` no longer available: [`GridWrite::BaseExpired`].
    pub fn write_live_grid(
        &self,
        conn: ConnId,
        seen: Seq,
        body: &[u8],
        opts: &GridWriteOptions,
    ) -> Result<GridWriteReport> {
        let out = self.write_live_grid_inner(conn, seen, body, opts);
        // an App viewer (K5) sees the others by handle
        self.for_viewer(conn, out, |l, r| l.grid_write(r))
    }

    fn write_live_grid_inner(
        &self,
        conn: ConnId,
        seen: Seq,
        body: &[u8],
        opts: &GridWriteOptions,
    ) -> Result<GridWriteReport> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit_edit(st, conn, Action::WriteBase, now)?;
        self.check_backlog(st, i)?;
        if !table::is_table(&self.schema) {
            return Err(SessionError::BadEdit(
                "write --base live by key is for tables".into(),
            ));
        }
        if seen != st.version {
            return self.write_live_stale(st, i, seen, body, opts, now);
        }
        let live = View::new(st.doc.keyed_state()?);
        let rec = self
            .pack
            .reconcile(&live, body, &st.codec)
            .map_err(SessionError::Declined)?;
        let marks = {
            let bad = |e: table::TableError| SessionError::BadEdit(e.to_string());
            let base = live.table().map_err(bad)?;
            let mut ours = base.clone();
            ours.apply(&rec.edits).map_err(bad)?;
            kept_not_exact(&base, &ours, &rec.exact_rows, rec.cols_exact)
        };
        let edits = self.normalize(rec.edits)?;
        let report = |outcome| GridWriteReport {
            outcome,
            skipped: Vec::new(),
            rekeyed: Vec::new(),
        };
        if edits.is_empty() {
            return Ok(report(GridWrite::Applied(EditResult {
                version: st.version,
                ..EditResult::default()
            })));
        }
        self.check_limits(st, &edits)?;
        self.check_grid_claims(st, i, &edits, opts.policy, now)?;
        st.verb_hint = Some("write --base live".to_owned());
        st.next_row_marks = marks;
        st.next_col_remap = !rec.cols_exact;
        let res = self.commit_edit(st, i, edits, ChangeKind::Edit, None, now, Vec::new())?;
        Ok(report(GridWrite::Applied(res)))
    }

    /// A live write from an older version: the outside-save merge, so no live edit goes with no
    /// note.
    fn write_live_stale(
        &self,
        st: &mut State,
        i: usize,
        seen: Seq,
        body: &[u8],
        opts: &GridWriteOptions,
        now: u64,
    ) -> Result<GridWriteReport> {
        let bad = |e: table::TableError| SessionError::BadEdit(e.to_string());
        let Some(base_view) = self.base_view(st, seen, now)? else {
            return Ok(GridWriteReport {
                outcome: GridWrite::BaseExpired,
                skipped: Vec::new(),
                rekeyed: Vec::new(),
            });
        };
        let base_t = base_view.table().map_err(bad)?;
        let rec = self
            .pack
            .reconcile(&base_view, body, &st.codec)
            .map_err(SessionError::Declined)?;
        let mut ours_t = base_t.clone();
        ours_t.apply(&rec.edits).map_err(bad)?;
        let live_t = View::new(st.doc.keyed_state()?).table().map_err(bad)?;
        let ours_t = rekey_dropped_in_file(&base_t, ours_t, &live_t);
        let exact = rec.exact_rows.iter().cloned().collect();
        let m = table::merge3_outside_text(&base_t, &ours_t, &live_t, &exact).map_err(bad)?;
        let edits = self.normalize(m.edits)?;
        if edits.is_empty() {
            return Ok(GridWriteReport {
                outcome: GridWrite::Applied(EditResult {
                    version: st.version,
                    ..EditResult::default()
                }),
                skipped: Vec::new(),
                rekeyed: m.rekeyed,
            });
        }
        self.check_limits(st, &edits)?;
        self.check_grid_claims(st, i, &edits, opts.policy, now)?;
        let pid = st.members[i].p.pid.clone();
        let mut overridden = self.overridden(st, &live_t, &m.conflicts)?;
        drop_own(&mut overridden, &pid);
        let before = st.version;
        let plan = self.plan_overrides(st, &overridden, "a write", before, before + 1, now)?;
        let mut versions = Vec::new();
        if !overridden.is_empty() && st.last_version_seq != st.version {
            let snap = self.snapshot(st)?;
            versions.push(Self::version_of(
                st,
                st.version,
                VersionReason::BeforeExternal,
                None,
                snap,
                now,
            ));
        }
        st.verb_hint = Some("write --base live".to_owned());
        st.next_note_rows = plan.note_rows();
        st.next_row_marks = kept_not_exact(&base_t, &ours_t, &rec.exact_rows, rec.cols_exact);
        st.next_col_remap = !rec.cols_exact;
        let res = self.commit_edit(st, i, edits, ChangeKind::Edit, None, now, versions)?;
        self.finish_overrides(st, overridden, plan, before, before + 1)?;
        Ok(GridWriteReport {
            outcome: GridWrite::Applied(res),
            skipped: Vec::new(),
            rekeyed: m.rekeyed,
        })
    }

    /// The line map a conflict report names lines by, or none unless the two line counts agree:
    /// on a file holding a lone CR its rendered lines aren't the lines `cat -n` numbers, and given
    /// to `--lines` one would hit another row; on a file that can't be rendered, nobody can tell.
    /// A report without lines still names the row by key. It returns no error, so a report can't
    /// fail a save.
    fn conflict_lines(&self, view: &View, codec: &[u8]) -> Option<table::LineMap> {
        let counts = table::LineCounts::of(&self.pack.encode(&view.content(), codec));
        #[cfg(any(test, feature = "differential"))]
        let counts = {
            use crate::differential::{line_mutant, LineMutant};
            match (line_mutant(), counts) {
                (Some(LineMutant::ConflictKeepsLines), table::LineCounts::Differ)
                | (Some(LineMutant::ConflictLinesFallBack), table::LineCounts::Unknown) => {
                    table::LineCounts::Agree
                }
                _ => counts,
            }
        };
        match counts {
            table::LineCounts::Agree => self.pack.line_map(view, codec),
            table::LineCounts::Differ | table::LineCounts::Unknown => None,
        }
    }

    /// The session's context for each merge conflict: who last wrote the region, its index
    /// in the live table, the rendered lines of its row, who claims it. `(base, ours)`: the
    /// table at V and the text's, for the cells the text changed in a row's conflict.
    fn grid_conflicts(
        &self,
        st: &State,
        i: usize,
        live: &View,
        t: &Table,
        (base_t, ours_t): (&Table, &Table),
        conflicts: Vec<table::Conflict>,
    ) -> Result<Vec<GridConflict>> {
        if conflicts.is_empty() {
            return Ok(Vec::new());
        }
        let store = self.store(st)?;
        let lines = self.conflict_lines(live, &st.codec);
        let now_g = self.grid_now(st)?;
        // read once for every conflict: each live row's and column's index, each row's lines,
        // and the live columns' positions in the base and the text (a row's conflict lists
        // the cells changed in it from the row's own cells), so each conflict costs O(its
        // row), however many there are (the planted ConflictWalk walks them per conflict)
        let walk = table::conflict_walk();
        let (rows_at, cols_at) = (t.rows.live_index(), t.cols.live_index());
        let spans = lines.as_ref().map(|m| m.row_spans());
        let (base_cols_at, ours_cols_at) = (base_t.cols.live_index(), ours_t.cols.live_index());
        let mut out = Vec::with_capacity(conflicts.len());
        for c in conflicts {
            let (facets, row): (Vec<(&str, String, ElemFacet)>, Option<&Key>) = match &c.at {
                table::Region::Cell { row, col } => (
                    vec![(table::CELLS, table::cell_key(row, col), ElemFacet::Value)],
                    Some(row),
                ),
                // a column the text deleted that someone wrote into: the cells' writers
                table::Region::Col(_)
                    if c.rule == table::ConflictRule::EditedCol
                        && c.theirs_writes
                            .iter()
                            .any(|w| matches!(w, table::Region::Cell { .. })) =>
                {
                    (
                        c.theirs_writes
                            .iter()
                            .filter_map(|w| match w {
                                table::Region::Cell { row, col } => Some((
                                    table::CELLS,
                                    table::cell_key(row, col),
                                    ElemFacet::Value,
                                )),
                                _ => None,
                            })
                            .collect(),
                        None,
                    )
                }
                // a row the text and someone else both changed: whoever changed it last
                table::Region::Row(r) if c.is_rewritten_row() => (
                    c.theirs_writes
                        .iter()
                        .filter_map(|w| match w {
                            table::Region::Cell { row, col } => {
                                Some((table::CELLS, table::cell_key(row, col), ElemFacet::Value))
                            }
                            table::Region::Quote(at) => {
                                Some((table::QUOTING, at.map_key(), ElemFacet::Value))
                            }
                            _ => None,
                        })
                        .collect(),
                    Some(r),
                ),
                table::Region::Row(r) => (
                    vec![
                        (table::ROWS, r.to_string(), ElemFacet::Position),
                        (table::ROWS, r.to_string(), ElemFacet::Existence),
                    ],
                    Some(r),
                ),
                table::Region::Col(k) => (
                    vec![
                        (table::COLS, k.to_string(), ElemFacet::Position),
                        (table::COLS, k.to_string(), ElemFacet::Existence),
                    ],
                    None,
                ),
                table::Region::Name(k) => {
                    (vec![(table::NAMES, k.to_string(), ElemFacet::Value)], None)
                }
                table::Region::Quote(at) => (
                    vec![(table::QUOTING, at.map_key(), ElemFacet::Value)],
                    match at {
                        table::QuoteAt::Cell(r, _) => Some(r),
                        table::QuoteAt::Header(_) => None,
                    },
                ),
            };
            let mut last: Option<ElemWrite> = None;
            for (root, key, f) in &facets {
                if let Some(w) = store.elem_write(self.doc_id, root, key, *f)? {
                    if last.as_ref().map_or(true, |l| w.seq > l.seq) {
                        last = Some(w);
                    }
                }
            }
            let written_by = match &last {
                Some(w) => store.change(self.doc_id, w.seq)?.map(|i| i.author),
                None => None,
            };
            let index = match &c.at {
                table::Region::Col(k) | table::Region::Name(k) if walk => match t.cols.state(k) {
                    ElemState::Live(i) => Some(i),
                    _ => None,
                },
                _ if walk => row.and_then(|r| match t.rows.state(r) {
                    ElemState::Live(i) => Some(i),
                    _ => None,
                }),
                table::Region::Col(k) | table::Region::Name(k) => cols_at.get(k).copied(),
                _ => row.and_then(|r| rows_at.get(r).copied()),
            };
            let lines = match (&lines, &spans, row) {
                (Some(m), _, Some(r)) if walk => m.lines_of(r),
                (_, Some(sp), Some(r)) => sp.get(r).cloned(),
                _ => None,
            };
            // a row's conflict: the cells in it either side changed since V
            let row_cells: Vec<(Key, Key)> = match &c.at {
                table::Region::Row(r) => {
                    let mut cells: Vec<(Key, Key)> = c
                        .theirs_writes
                        .iter()
                        .filter_map(|w| match w {
                            table::Region::Cell { row, col }
                            | table::Region::Quote(table::QuoteAt::Cell(row, col))
                                if row == r =>
                            {
                                Some((row.clone(), col.clone()))
                            }
                            _ => None,
                        })
                        .collect();
                    if walk {
                        for col in base_t.cols.live().chain(ours_t.cols.live()) {
                            let at = (r.clone(), col.clone());
                            if base_t.cell(r, col) != ours_t.cell(r, col) && !cells.contains(&at) {
                                cells.push(at);
                            }
                        }
                        cells
                    } else {
                        // the columns live in the base or the text where the row's cell differs:
                        // only where either holds a cell (absent on both is the same), in the
                        // base's column order, then the text's
                        let mut seen: HashSet<(Key, Key)> = cells.iter().cloned().collect();
                        let mut changed: Vec<((usize, usize), &Key)> = base_t
                            .row_cells(r)
                            .chain(ours_t.row_cells(r))
                            .map(|(c, _)| c)
                            .filter_map(|c| {
                                let at = match (base_cols_at.get(c), ours_cols_at.get(c)) {
                                    (Some(&i), _) => (0, i),
                                    (None, Some(&i)) => (1, i),
                                    (None, None) => return None,
                                };
                                Some((at, c))
                            })
                            .filter(|(_, c)| base_t.cell(r, c) != ours_t.cell(r, c))
                            .collect();
                        changed.sort_unstable_by_key(|x| x.0);
                        changed.dedup_by_key(|x| x.0);
                        for (_, c) in changed {
                            let at = (r.clone(), c.clone());
                            if seen.insert(at.clone()) {
                                cells.push(at);
                            }
                        }
                        cells
                    }
                }
                _ => Vec::new(),
            };
            let claimed_by = self.claimed_region(st, &now_g, i, &c.at, &row_cells);
            out.push(GridConflict {
                conflict: c,
                written_by,
                written_in: last.map(|w| w.seq),
                claimed_by,
                index,
                lines,
            });
        }
        Ok(out)
    }

    // ----- history -----

    /// `restore --version V` on a structured doc: by identity (see [`restore_edits`]), one
    /// forward change by the caller, after cutting a version of the doc as it is.
    pub(super) fn restore_structured(
        &self,
        st: &mut State,
        i: usize,
        target: Version,
        vid: crate::store::VersionId,
        now: u64,
    ) -> Result<EditResult> {
        let live = st.doc.keyed_state()?;
        let mut content = target.content;
        if table::is_table(&self.schema) {
            // S12: a key `drop-deleted` removed is never reused
            rekey_dropped(&live, &mut content)?;
        }
        let edits = restore_edits(&live, &content, table::is_table(&self.schema));
        if edits.is_empty() {
            return Ok(EditResult {
                version: st.version,
                ..EditResult::default()
            });
        }
        self.check_limits(st, &edits)?;
        let mut versions = Vec::new();
        if st.last_version_seq != st.version {
            let snap = self.snapshot(st)?;
            versions.push(Self::version_of(
                st,
                st.version,
                VersionReason::BeforeRestore,
                None,
                snap,
                now,
            ));
        }
        // a restore is a whole rewrite by identity: each live cell of someone else's that it
        // takes gets the note (contract §2.9.1); the version just cut holds it
        let mut finish = None;
        if table::is_table(&self.schema) {
            let live_t =
                Table::from_keyed(&live).map_err(|e| SessionError::BadEdit(e.to_string()))?;
            let taken = restore_overrides(&live_t, &edits);
            let pid = st.members[i].p.pid.clone();
            let mut overridden = self.overridden(st, &live_t, &taken)?;
            drop_own(&mut overridden, &pid);
            let before = st.version;
            let plan =
                self.plan_overrides(st, &overridden, "a restore", before, before + 1, now)?;
            st.next_note_rows = plan.note_rows();
            finish = Some((overridden, plan, before));
        }
        let res = self.commit_edit(st, i, edits, ChangeKind::Restore, Some(vid), now, versions)?;
        if let Some((overridden, plan, before)) = finish {
            self.finish_overrides(st, overridden, plan, before, before + 1)?;
        }
        Ok(res)
    }

    /// `revert <change>` of a structured change (spec v1.10 S10): its v3 record reversed by
    /// the element rules ([`super::grid_undo`]), "own" being the change's writes. The revert
    /// is the caller's batch, on their undo stack (U5).
    pub(super) fn revert_structured(
        &self,
        st: &mut State,
        i: usize,
        change: Seq,
        rec: RevertRecord,
        partial: bool,
        now: u64,
    ) -> Result<UndoOutcome> {
        let mut taken = Vec::new();
        let out = self.reverse_records(
            st,
            i,
            vec![(change, rec, None)],
            partial,
            ChangeKind::Revert,
            Some(change),
            Some(&mut taken),
            now,
        )?;
        if let Some(Some(seq)) = taken.first() {
            let pid = st.members[i].p.pid.clone();
            let batch = Batch {
                seq: *seq,
                on_behalf_of: st.members[i].p.on_behalf_of.clone(),
                regions: Vec::new(),
                touched: false,
            };
            self.push_batch(st, &pid, batch, Origin::Edit);
        }
        Ok(out)
    }

    /// Element conflicts as the undo report's regions (no byte range: `range` empty; the
    /// rendered lines of the element's row when the pack maps lines).
    pub(super) fn elem_conflict_regions(
        &self,
        st: &State,
        live: &KeyedState,
        conflicts: Vec<(String, String, String, Option<ElemWrite>)>,
    ) -> Result<Vec<ConflictRegion>> {
        if conflicts.is_empty() {
            return Ok(Vec::new());
        }
        let store = self.store(st)?;
        let view = View::new(live.clone());
        let lines = self.conflict_lines(&view, &st.codec);
        // each row's lines read once (O(1) per conflict after)
        let spans = lines.as_ref().map(|m| m.row_spans());
        let mut out = Vec::new();
        for (root, key, why, w) in conflicts {
            let row = match root.as_str() {
                table::ROWS => Key::new(key.as_str()).ok(),
                table::CELLS => table::parse_cell_key(&key).map(|(r, _)| r),
                _ => None,
            };
            let l = match (&spans, &row) {
                (Some(sp), Some(r)) => sp.get(r).map_or((0, 0), |x| (x.start, x.end - 1)),
                _ => (0, 0),
            };
            let by = match &w {
                Some(w) => store.change(self.doc_id, w.seq)?.map(|c| c.author),
                None => None,
            };
            let erased = why == "erased";
            let mut c = ConflictRegion::element(root, key, l, why);
            if erased {
                // a purge took the value (C1f), as a text revert over purged text says
                c.kind = crate::session::ConflictKind::Erased;
            }
            c.theirs = match (&by, &w) {
                (Some(b), Some(w)) => format!("{b} in change {}", w.seq),
                _ => String::new(),
            };
            out.push(c);
        }
        Ok(out)
    }

    /// `show --version V` with every root: text, a list's elements, a map. Needs history.
    pub fn show_content(
        &self,
        conn: ConnId,
        vid: crate::store::VersionId,
    ) -> Result<Vec<(String, Neutral)>> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::History, now)?;
        self.admit(&mut st, conn, Need::History, Action::History, now)?;
        let v = self
            .store(&st)?
            .version(self.doc_id, vid)
            .map_err(|e| match e {
                crate::store::StoreError::NoVersion(..) => SessionError::NoSuchChange(vid),
                e => e.into(),
            })?;
        Ok(v.content)
    }

    /// Blame one element facet (A12): who holds `root`/`key`'s `facet` now, from
    /// `ELEM_WRITES` (the engine's winner agrees: the audit checks it). A value an undo, redo
    /// or revert put back blames its original write, with the restoring change second
    /// (U23). `None`: nothing recorded (never written). Needs history.
    pub fn blame_elem(
        &self,
        conn: ConnId,
        root: &str,
        key: &str,
        facet: Facet,
    ) -> Result<Option<ElemBlame>> {
        let out = self.blame_elem_inner(conn, root, key, facet);
        // an App viewer (K5) sees the others by handle
        self.for_viewer(conn, out, |l, b| b.map(|b| l.elem_blame(b)))
    }

    fn blame_elem_inner(
        &self,
        conn: ConnId,
        root: &str,
        key: &str,
        facet: Facet,
    ) -> Result<Option<ElemBlame>> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Blame, now)?;
        self.admit(&mut st, conn, Need::History, Action::Blame, now)?;
        let st = &*st;
        let kind = self.schema.kind_of(root).ok_or_else(|| DocError::NoRoot {
            schema: self.schema.name,
            root: root.to_owned(),
        })?;
        if kind == Kind::Text {
            return Err(DocError::WrongKind {
                root: root.to_owned(),
                wanted: Kind::Map,
                actual: Kind::Text,
            }
            .into());
        }
        let store = self.store(st)?;
        let Some(w) = store.elem_write(self.doc_id, root, key, ElemFacet::of(facet))? else {
            return Ok(None);
        };
        let value = match (kind, facet) {
            (Kind::Map, _) => st.doc.map_get(root, key)?,
            (_, Facet::Value) => match Key::new(key) {
                Ok(k) => st.doc.elem_value(root, &k)?,
                Err(_) => None,
            },
            _ => None,
        };
        Ok(Some(self.blame_of_write(store, root, key, value, &w)?))
    }

    /// One facet's blame from its `ELEM_WRITES` row: the original write's change (U23: a
    /// value put back blames the write it restores), with the restoring change second.
    fn blame_of_write(
        &self,
        store: &DocStore,
        root: &str,
        key: &str,
        value: Option<Value>,
        w: &ElemWrite,
    ) -> Result<ElemBlame> {
        let (orig, restoring) = match w.restores {
            Some((seq, _)) => (seq, Some(w.seq)),
            None => (w.seq, None),
        };
        let info = store.change(self.doc_id, orig)?;
        let restorer = match restoring {
            Some(s) => store.change(self.doc_id, s)?.map(|c| c.author),
            None => None,
        };
        Ok(ElemBlame {
            root: root.to_owned(),
            key: key.to_owned(),
            value,
            deleted: matches!(w.kind, ElemOpKind::MapDelete | ElemOpKind::Delete),
            author: info
                .as_ref()
                .map_or_else(|| UNKNOWN_PID.to_owned(), |c| c.author.clone()),
            change: orig,
            kind: info.as_ref().map(|c| c.kind),
            via: info.as_ref().map(|c| c.via.clone()),
            at_ms: info.as_ref().map(|c| c.at_ms),
            on_behalf_of: info.and_then(|c| c.on_behalf_of),
            restored_by: restorer,
            restored_in: restoring,
        })
    }

    /// `blame --structure` (A12, K-HIST-11): per live row and column, in order, who
    /// inserted it and who last moved it (moves don't author content, so line blame never
    /// goes to a sort; this is where moves show). A move an undo, redo or revert put back
    /// blames the position it restores (`moved: None` when that is the insert's own).
    /// Needs history. A table only.
    pub fn blame_structure(&self, conn: ConnId) -> Result<StructureBlame> {
        let out = self.blame_structure_inner(conn);
        // an App viewer (K5) sees the others by handle
        self.for_viewer(conn, out, |l, b| l.structure_blame(b))
    }

    fn blame_structure_inner(&self, conn: ConnId) -> Result<StructureBlame> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Blame, now)?;
        self.admit(&mut st, conn, Need::History, Action::Blame, now)?;
        let st = &*st;
        if !table::is_table(&self.schema) {
            return Err(SessionError::BadEdit(
                "blame --structure is for tables".into(),
            ));
        }
        let store = self.store(st)?.clone();
        let mut out = StructureBlame {
            version: st.version,
            rows: Vec::new(),
            cols: Vec::new(),
        };
        for (root, into) in [(table::ROWS, &mut out.rows), (table::COLS, &mut out.cols)] {
            for (n, (k, _)) in st.doc.list(root)?.into_iter().enumerate() {
                let ins = store.elem_write(self.doc_id, root, k.as_str(), ElemFacet::Insert)?;
                let pos = store.elem_write(self.doc_id, root, k.as_str(), ElemFacet::Position)?;
                let inserted = match &ins {
                    Some(w) => Some(self.blame_of_write(&store, root, k.as_str(), None, w)?),
                    None => None,
                };
                let moved = match &pos {
                    Some(w)
                        if resolved(w).0 != ins.as_ref().map_or(0, |i| i.seq)
                            && (w.kind == ElemOpKind::Move || w.restores.is_some()) =>
                    {
                        Some(self.blame_of_write(&store, root, k.as_str(), None, w)?)
                    }
                    _ => None,
                };
                into.push(PlaceBlame {
                    key: k,
                    at: n + 1,
                    inserted,
                    moved,
                });
            }
        }
        Ok(out)
    }

    /// [`Session::blame`] on a table (A12, contract §5.5): the rendered file's lines; a data
    /// row's lines go to its most recent cell write (the greatest change id among its present
    /// cells in live columns, a value put back blaming the write it restores), the header's
    /// to the most recent write of a live column's name, and a row with no present cell to
    /// its insert. Moves author nothing: a sorted table blames as before the sort.
    pub(super) fn blame_grid(
        &self,
        st: &State,
        root: &str,
        lines: Option<LineRange>,
    ) -> Result<Blame> {
        let content = self.content(st)?;
        let text = String::from_utf8_lossy(&self.pack.encode(&content, &st.codec)?).into_owned();
        let view = View::new(st.doc.keyed_state()?);
        let t = view
            .table()
            .map_err(|e| SessionError::BadEdit(e.to_string()))?;
        let map = self
            .pack
            .line_map(&view, &st.codec)
            .unwrap_or_else(|| t.line_map());
        let texts = split_lines(&text);
        let count = map.lines.len();
        let (first, last) = match &lines {
            None => (1, count),
            Some(r) => {
                if r.root != root {
                    return Err(SessionError::BadPosition(format!(
                        "lines of root {:?}, blaming {root:?}",
                        r.root
                    )));
                }
                if r.first == 0 || r.first > r.last || r.first > count {
                    return Err(SessionError::BadPosition(format!(
                        "lines {}..{} of a {count}-line file",
                        r.first, r.last
                    )));
                }
                (r.first, r.last.min(count))
            }
        };
        let mut out = Blame {
            root: root.to_owned(),
            version: st.version,
            lines: Vec::new(),
        };
        if count == 0 {
            return Ok(out);
        }
        let store = self.store(st)?.clone();
        // every value write of the table, by root and key
        let mut writes: HashMap<(String, String), ElemWrite> = HashMap::new();
        let mut inserts: HashMap<String, ElemWrite> = HashMap::new();
        for ((r, k, f), w) in store.elem_writes(self.doc_id)? {
            match (r.as_str(), f) {
                (table::CELLS | table::NAMES, ElemFacet::Value) => {
                    writes.insert((r, k), w);
                }
                (table::ROWS, ElemFacet::Insert) => {
                    inserts.insert(k, w);
                }
                _ => {}
            }
        }
        // the write a line goes to: (original change, the write)
        let rank = |w: &ElemWrite| (resolved(w).0, w.restores.map(|_| w.seq));
        let best = |cands: Vec<&ElemWrite>| -> Option<ElemWrite> {
            cands.into_iter().max_by_key(|w| rank(w)).cloned()
        };
        // each live column's position: a line's candidates are its row's present cells (or
        // the names), in column order (the order breaks ties between equal ranks), so a line
        // costs O(its row's cells), whatever the width; a row's lines share one answer
        let cols_at = t.col_positions();
        let owner_write = |owner: Option<&table::LineOwner>| -> Option<ElemWrite> {
            if table::per_column() {
                // the planted per-column walk
                let live_cols = t.live_cols();
                return match owner {
                    Some(table::LineOwner::Header) => best(
                        live_cols
                            .iter()
                            .filter_map(|c| writes.get(&(table::NAMES.to_owned(), c.to_string())))
                            .collect(),
                    ),
                    Some(table::LineOwner::Row(r)) => best(
                        live_cols
                            .iter()
                            .filter(|c| t.cell(r, c).is_some())
                            .filter_map(|c| {
                                writes.get(&(table::CELLS.to_owned(), table::cell_key(r, c)))
                            })
                            .collect(),
                    )
                    .or_else(|| inserts.get(r.as_str()).cloned()),
                    Some(table::LineOwner::Preamble) | None => None,
                };
            }
            let in_order = |mut at: Vec<(usize, (String, String))>| {
                at.sort_unstable_by_key(|(i, _)| *i);
                best(at.iter().filter_map(|(_, k)| writes.get(k)).collect())
            };
            match owner {
                Some(table::LineOwner::Header) => in_order(
                    t.names
                        .keys()
                        .filter_map(|c| cols_at.get(c).map(|&i| (i, c)))
                        .map(|(i, c)| (i, (table::NAMES.to_owned(), c.to_string())))
                        .collect(),
                ),
                Some(table::LineOwner::Row(r)) => in_order(
                    t.row_cells(r)
                        .filter_map(|(c, _)| cols_at.get(c).map(|&i| (i, c)))
                        .map(|(i, c)| (i, (table::CELLS.to_owned(), table::cell_key(r, c))))
                        .collect(),
                )
                .or_else(|| inserts.get(r.as_str()).cloned()),
                // a codec's own line (`sep=;`): no element wrote it
                Some(table::LineOwner::Preamble) | None => None,
            }
        };
        let mut chosen: Vec<(usize, Option<ElemWrite>)> = Vec::new();
        let mut last_owner: Option<(&table::LineOwner, Option<ElemWrite>)> = None;
        for n in first..=last {
            let owner = map.owner(n);
            let same = matches!((&last_owner, owner), (Some((o, _)), Some(x)) if *o == x);
            let w = if same {
                last_owner.as_ref().and_then(|(_, w)| w.clone())
            } else {
                let w = owner_write(owner);
                last_owner = owner.map(|o| (o, w.clone()));
                w
            };
            chosen.push((n, w));
        }
        let seqs: std::collections::BTreeSet<Seq> = chosen
            .iter()
            .filter_map(|(_, w)| w.as_ref())
            .flat_map(|w| [Some(resolved(w).0), w.restores.map(|_| w.seq)])
            .flatten()
            .collect();
        let infos = store.change_infos(self.doc_id, &seqs)?;
        let purged = store.purged_changes(self.doc_id, &seqs)?;
        let approvals: HashMap<Seq, ApprovalState> = store
            .approvals(self.doc_id)?
            .into_iter()
            .filter(|(s, _)| seqs.contains(s))
            .map(|(s, a)| (s, a.state))
            .collect();
        for (n, w) in chosen {
            let orig = w.as_ref().map(|w| resolved(w).0);
            let restoring = w.as_ref().and_then(|w| w.restores.map(|_| w.seq));
            let info = orig.and_then(|s| infos.get(&s));
            out.lines.push(BlameLine {
                line: n,
                text: texts.get(n - 1).cloned().unwrap_or_default(),
                author: info.map_or_else(|| UNKNOWN_PID.to_owned(), |i| i.author.clone()),
                change: orig,
                kind: info.map(|i| i.kind),
                via: info.map(|i| i.via.clone()),
                at_ms: info.map(|i| i.at_ms),
                on_behalf_of: info.and_then(|i| i.on_behalf_of.clone()),
                approval: orig.and_then(|s| approvals.get(&s).copied()),
                erased: orig.is_some_and(|s| purged.contains(&s)),
                restored_by: restoring
                    .and_then(|s| infos.get(&s))
                    .map(|i| i.author.clone()),
                restored_in: restoring,
            });
        }
        Ok(out)
    }

    /// Blame one cell of a table (A12, K-HIST-11): who wrote its value (or deleted it).
    /// `None`: no write recorded (an absent cell never written, or a row or column the table
    /// doesn't have). The `blame --cell` verb refuses that as the kit refuses any address
    /// outside the doc, as `blame -L` past the end does: [`SessionError::BadPosition`]
    /// (`bad_position`, exit 2).
    pub fn blame_cell(&self, conn: ConnId, row: &Key, col: &Key) -> Result<Option<CellBlame>> {
        if !table::is_table(&self.schema) {
            return Err(SessionError::BadEdit("blame_cell is for tables".into()));
        }
        self.blame_elem(conn, table::CELLS, &table::cell_key(row, col), Facet::Value)
    }

    // ----- the lost-edit audit (K-AUD-1, grid form) -----

    /// The lost-edit audit for a structured doc (K-AUD-1, seam §7.8): (a) the stored log
    /// replays to the live doc; (b) per element facet, every acknowledged write is live or
    /// superseded by an attributed write or delete (a cell in a deleted row stays, hidden):
    /// the index (`ELEM_WRITES`) is checked against the doc's registers (their winning write
    /// identities) and against the change records in the revert window, and every element
    /// and key the doc holds was acknowledged; (c) a synced, clean file holds the doc's
    /// rendering; (d) a clean doc's disk replica is the doc, by key
    /// ([`Session::audit_disk_sync`]: else the next outside edit is aligned to a stale
    /// structure and lost). For a host's integrity check and the tests; reads the whole doc.
    pub fn audit(&self) -> Result<AuditReport> {
        let mut guard = self.lock()?;
        // `&mut`: the sync check (d) decodes the disk replica when it isn't yet (v0.9.8)
        let st = &mut *guard;
        let store = self.store(st)?.clone();
        let mut rep = AuditReport::default();
        // (a)
        let replayed = Doc::from_store(self.schema, &store.load(self.doc_id)?)?;
        let live = st.doc.keyed_state()?;
        if replayed.keyed_state()? != live {
            rep.violations.push(AuditViolation::LogReplay);
        }
        // (b) the index against the doc
        let rows = store.elem_writes(self.doc_id)?;
        let mut indexed: HashSet<(String, String, ElemFacet)> = HashSet::new();
        let records: HashMap<Seq, RevertRecord> = store
            .revert_records(self.doc_id)?
            .into_iter()
            .filter(|(_, r)| !r.elems.is_empty())
            .collect();
        let mut summaries: HashMap<Seq, bool> = HashMap::new();
        for ((root, key, f), w) in &rows {
            indexed.insert((root.clone(), key.clone(), *f));
            if *f == ElemFacet::Insert {
                continue;
            }
            rep.checked += 1;
            let known = match summaries.get(&w.seq) {
                Some(k) => *k,
                None => {
                    let k = store.change(self.doc_id, w.seq)?.is_some();
                    summaries.insert(w.seq, k);
                    k
                }
            };
            if !known {
                rep.violations.push(AuditViolation::Unattributed {
                    root: root.clone(),
                    key: key.clone(),
                    change: w.seq,
                });
                continue;
            }
            let facet = match f {
                ElemFacet::Value => Facet::Value,
                ElemFacet::Position => Facet::Position,
                _ => Facet::Existence,
            };
            let list = self.schema.kind_of(root) == Some(Kind::MovableList);
            let k = Key::new(key.as_str()).ok();
            let state = match (&k, list) {
                (Some(k), true) => Some(st.doc.elem(root, k)?),
                _ => None,
            };
            let doc_w = st.doc.winner(root, key, facet)?;
            // the write the record says the facet held before this one (a dropped write
            // leaves the doc holding it)
            let prev = records.get(&w.seq).and_then(|r| {
                r.elems
                    .iter()
                    .filter(|x| x.root == *root)
                    .flat_map(|x| x.ops.iter())
                    .rfind(|o| o.key == *key && o.kind.facet() == *f)
                    .and_then(|o| o.prev.clone())
            });
            let dropped = |doc_w: Option<WriteId>| {
                prev.as_ref().is_some_and(|p| p.write == doc_w) || doc_w.is_none()
            };
            let mut v = |x| rep.violations.push(x);
            match (f, w.kind) {
                (ElemFacet::Existence, ElemOpKind::Insert | ElemOpKind::Restore) => {
                    if !matches!(state, Some(ElemState::Live(_))) {
                        v(AuditViolation::LostElement {
                            root: root.clone(),
                            key: key.clone(),
                            change: w.seq,
                        });
                    }
                }
                (ElemFacet::Existence, _) => {
                    if doc_w != w.write {
                        v(match state {
                            Some(ElemState::Live(_)) if dropped(doc_w) => {
                                AuditViolation::DroppedWrite {
                                    root: root.clone(),
                                    key: key.clone(),
                                    facet,
                                    change: w.seq,
                                }
                            }
                            _ => AuditViolation::SilentWrite {
                                root: root.clone(),
                                key: key.clone(),
                                facet,
                            },
                        });
                    }
                }
                (ElemFacet::Value, ElemOpKind::MapDelete) => {
                    if st.doc.map_get(root, key)?.is_some() {
                        v(AuditViolation::SilentWrite {
                            root: root.clone(),
                            key: key.clone(),
                            facet,
                        });
                    }
                }
                _ => {
                    if doc_w != w.write {
                        v(if dropped(doc_w) {
                            AuditViolation::DroppedWrite {
                                root: root.clone(),
                                key: key.clone(),
                                facet,
                                change: w.seq,
                            }
                        } else {
                            AuditViolation::SilentWrite {
                                root: root.clone(),
                                key: key.clone(),
                                facet,
                            }
                        });
                    }
                }
            }
        }
        // ... every element and key the doc holds was acknowledged
        for (root, x) in &live.roots {
            match x {
                KeyedRoot::List { live, hidden, .. } => {
                    for k in live.iter().map(|(k, _)| k).chain(hidden.keys()) {
                        if !indexed.contains(&(root.clone(), k.to_string(), ElemFacet::Existence)) {
                            rep.violations.push(AuditViolation::SilentWrite {
                                root: root.clone(),
                                key: k.to_string(),
                                facet: Facet::Existence,
                            });
                        }
                    }
                }
                KeyedRoot::Map(m) => {
                    for k in m.keys() {
                        if !indexed.contains(&(root.clone(), k.clone(), ElemFacet::Value)) {
                            rep.violations.push(AuditViolation::SilentWrite {
                                root: root.clone(),
                                key: k.clone(),
                                facet: Facet::Value,
                            });
                        }
                    }
                }
                KeyedRoot::Text(_) => {}
            }
        }
        // ... and the records in the window against the index: every acknowledged op is
        // there, or superseded by a later recorded write (a dropped element leaves it)
        let index: HashMap<(String, String, ElemFacet), Seq> = rows
            .iter()
            .map(|((r, k, f), w)| ((r.clone(), k.clone(), *f), w.seq))
            .collect();
        let dropped_later: HashSet<(String, String)> = records
            .values()
            .flat_map(|r| r.elems.iter())
            .flat_map(|x| {
                x.ops
                    .iter()
                    .filter(|o| o.kind == ElemOpKind::Drop)
                    .map(move |o| (x.root.clone(), o.key.clone()))
            })
            .collect();
        let mut seqs: Vec<&Seq> = records.keys().collect();
        seqs.sort();
        for seq in seqs {
            for x in &records[seq].elems {
                for o in x.ops.iter().filter(|o| o.effect != StoredEffect::None) {
                    if o.kind == ElemOpKind::Drop
                        || dropped_later.contains(&(x.root.clone(), o.key.clone()))
                    {
                        continue;
                    }
                    let f = o.kind.facet();
                    let at = index.get(&(x.root.clone(), o.key.clone(), f)).copied();
                    if at.map_or(true, |s| s < *seq) {
                        rep.violations.push(AuditViolation::DroppedWrite {
                            root: x.root.clone(),
                            key: o.key.clone(),
                            facet: match f {
                                ElemFacet::Value => Facet::Value,
                                ElemFacet::Position => Facet::Position,
                                _ => Facet::Existence,
                            },
                            change: *seq,
                        });
                    }
                }
            }
        }
        // (c)
        if let Some(d) = &st.disk {
            if d.dirty_since.is_none()
                && !d.missing
                && st.declined.is_none()
                && !d.synced_hash.is_empty()
                && file_hash(&st.hash_key, &self.render(st)?) != d.synced_hash
            {
                rep.violations.push(AuditViolation::File);
            }
        }
        // (d)
        for drift in self.disk_drift(st)? {
            rep.violations.push(AuditViolation::DiskReplica(drift));
        }
        rep.violations.dedup();
        Ok(rep)
    }

    // ----- outside edits -----

    /// Conformance only: while `on`, outside saves record their writes with **no identity**
    /// (`ELEM_WRITES.write` empty), as a write merged in from elsewhere whose writer the
    /// engine doesn't report: KC7's case, which only Yrs has (spec v1.10 S2). The grid trace
    /// class `write_with_no_identity` uses it; the kit has no such merge path yet.
    #[cfg(feature = "conformance")]
    pub fn set_outside_without_identity_for_tests(&self, on: bool) -> Result<()> {
        self.lock()?.outside_without_identity = on;
        Ok(())
    }

    /// An outside save of a structured doc's file (contract §2.9, §2.9.1): a **keyed
    /// three-way merge** applied to the live doc, as v0.9.5 does for text. Base = the grid as
    /// the file held it at the last sync (the disk replica's keyed state; the replica is the
    /// doc itself whenever the doc renders the synced bytes, by the change-counter marker);
    /// ours = the file's new grid, aligned on the base ([`super::super::Pack::reconcile`],
    /// with the live doc's key counters, so new keys are new to the live doc too); theirs =
    /// the live grid, dirty or not. Merged by row and column keys ([`table::merge3_outside`]):
    /// changes on other elements both apply; where both sides changed one element the file
    /// wins (it applies after the live change, as the external participant: the later
    /// write), and the live value is never lost: a version is cut before the save when the
    /// live doc has changed since the last one, its author gets a heads-up note
    /// ([`Session::take_notes`]) and an [`AuditEvent::OutsideOverride`] records it. Never
    /// refused, never dropped. Then the replica takes the file's grid
    /// ([`Session::grid_replica_to_file`]). The replica's elements are never merged into the
    /// doc, so a change that moved no byte (a header switch, an empty cell unset,
    /// `drop-deleted`), written back or not, can't misplace the save.
    pub(super) fn ingest_structured(
        &self,
        st: &mut State,
        bytes: &[u8],
        h: Vec<u8>,
        now: u64,
    ) -> Result<DiskCheck> {
        timed!(GridIngest);
        let is_table = table::is_table(&self.schema);
        // when the doc renders the synced bytes, the replica is the doc (counter marker)
        self.resync_if_synced(st)?;
        let bad = |e: table::TableError| SessionError::BadEdit(e.to_string());
        if is_table {
            // the live doc may have re-identified rows the file shows (a header switch, with
            // edits pending since): the replica takes the live keys for them first
            let live_t = Table::from_keyed(&st.doc.keyed_state()?).map_err(bad)?;
            let d = st.disk.as_mut().expect("has disk");
            let mut rt = Table::from_keyed(&d.replica.get()?.keyed_state()?).map_err(bad)?;
            let edits = rt.reidentify_onto(&live_t).map_err(bad)?;
            if !edits.is_empty() {
                let own = loop {
                    let c = mint_client_id();
                    if st.doc.clock_of(c) == 0 && d.replica.get()?.clock_of(c) == 0 {
                        break c;
                    }
                };
                d.replica.get()?.edit_applied(own, &edits)?;
                // as in `grid_replica_to_file`: the replica now holds ops of its own, so the next
                // write-back reloads it from the doc instead of comparing state vectors
                d.catch_up = None;
            }
        }
        let mut synced = st
            .disk
            .as_mut()
            .expect("has disk")
            .replica
            .get()?
            .keyed_state()?;
        let live_t = if is_table {
            let t = Table::from_keyed(&st.doc.keyed_state()?).map_err(bad)?;
            // keys the file's new rows get must be new to the live doc too
            bump_counters(&mut synced, &t);
            Some(t)
        } else {
            None
        };
        // the host's value limit holds for the file's values too (C1k)
        let synced = View::new(synced).with_max_value(self.config.grid_max_value_len);
        let rec = match self.pack.reconcile(&synced, bytes, &st.codec) {
            Ok(r) => r,
            Err(why) => return self.decline(st, why),
        };
        let store = self.store(st)?.clone();
        let mut meta = store.meta(self.doc_id)?;
        meta.codec = rec.codec.clone();
        meta.declined = None;
        meta.synced_hash = Some(h.clone());
        let was_declined = st.declined.is_some();
        // the save on the synced grid (for the replica) and the merge on the live grid (for
        // the doc)
        let file_edits = self.normalize(rec.edits)?;
        let mut row_marks: Vec<Key> = Vec::new();
        let mut col_remap = false;
        let mut moved_then_deleted: Vec<Key> = Vec::new();
        let (edits, overrides) = match &live_t {
            Some(live_t) if !file_edits.is_empty() => {
                let base_t = synced.table().map_err(bad)?;
                let mut ours_t = base_t.clone();
                ours_t.apply(&file_edits).map_err(bad)?;
                // what the file changed of an element the live doc dropped since comes back
                // under a fresh key (a dropped key is never reused, and can't be restored)
                let ours_t = rekey_dropped_in_file(&base_t, ours_t, live_t);
                let exact = rec.exact_rows.iter().cloned().collect();
                row_marks = kept_not_exact(&base_t, &ours_t, &rec.exact_rows, rec.cols_exact);
                col_remap = !rec.cols_exact;
                let m =
                    table::merge3_outside_text(&base_t, &ours_t, live_t, &exact).map_err(bad)?;
                moved_then_deleted = m.moved_then_deleted;
                debug_assert!(
                    m.rekeyed.is_empty(),
                    "the base's counters cover the live doc's: no key is re-minted"
                );
                (self.normalize(m.edits)?, m.conflicts)
            }
            _ => (file_edits.clone(), Vec::new()),
        };
        if edits.is_empty() {
            // the same content in other bytes, or a save the doc already holds: adopt the
            // file's format; the doc doesn't change
            let held = self.holds_file(st, bytes, &rec.codec)?;
            let state = match self.grid_replica_to_file(st, held, &file_edits) {
                Ok(s) => s,
                Err(e) => {
                    self.recover(st, &e.to_string());
                    return Err(e);
                }
            };
            let d = st.disk.as_mut().expect("has disk");
            if held {
                d.synced_seq = st.version;
            }
            meta.synced_seq = d.synced_seq;
            let sync = SyncState {
                hash: h.clone(),
                seq: d.synced_seq,
                state,
            };
            d.replica_bytes = sync.state.len() as u64;
            // a re-save that changed only the codec settings (a BOM, CRLF, another
            // delimiter) is an attributed outside change, never silent (C1k): a version by
            // the external participant holding the new settings; the doc's log doesn't move
            // (no change record without an engine update)
            let old_codec = std::mem::replace(&mut st.codec, rec.codec.clone());
            let settings = (old_codec != rec.codec)
                .then(|| self.pack.describe_codec_change(&old_codec, &rec.codec));
            let versions = match &settings {
                Some(_) => {
                    let (content, codec) = self.snapshot(st)?;
                    vec![Version {
                        seq: st.version,
                        at_ms: now,
                        reason: VersionReason::External,
                        name: None,
                        authors: vec![EXTERNAL_PID.to_owned()],
                        content,
                        codec,
                    }]
                }
                None => Vec::new(),
            };
            if let Err(e) = store.commit_disk(
                self.doc_id,
                st.version + 1,
                &[],
                &[],
                &versions,
                &meta,
                &sync,
            ) {
                st.codec = old_codec;
                return Err(e.into());
            }
            let d = st.disk.as_mut().expect("has disk");
            d.synced_hash = h;
            // a person saved since the kit's recent writes: one of them coming back now is
            // their save, not an echo (v0.9.5, as for text)
            d.own.clear();
            d.echoed = None;
            if was_declined {
                st.declined = None;
                self.emit(AuditEvent::FileRestored {
                    key: st.key.clone(),
                });
            }
            if let Some(description) = settings {
                st.last_version_seq = st.version;
                let version = store
                    .version_infos(self.doc_id)?
                    .into_iter()
                    .filter(|v| v.seq == st.version && v.reason == VersionReason::External)
                    .map(|v| v.id)
                    .max()
                    .unwrap_or_default();
                self.emit(AuditEvent::OutsideSettings {
                    key: st.key.clone(),
                    version,
                    description: description.clone(),
                });
                Self::broadcast_settings(
                    st,
                    &SettingsChanged {
                        version: st.version,
                        settings_version: version,
                        pid: EXTERNAL_PID.to_owned(),
                        via: "disk".to_owned(),
                        at_ms: now,
                        codec: st.codec.clone(),
                        description,
                    },
                );
            }
            self.settle_saved(st, now)?;
            return Ok(DiskCheck::Unchanged);
        }
        if self.check_limits(st, &edits).is_err() {
            return self.decline(
                st,
                Declined {
                    code: "too_large",
                    message: format!(
                        "the file now holds more than the grid limit ({} cells, {} columns, {} \
                         rows)",
                        self.config.grid_max_cells,
                        self.config.grid_max_cols,
                        self.config.grid_max_rows
                    ),
                },
            );
        }
        // who held each live value the save overrides, and where it was (before it applies)
        let overridden = match &live_t {
            Some(t) => self.overridden(st, t, &overrides)?,
            None => Vec::new(),
        };
        let client = self.external_client(st)?;
        let before_seq = st.version;
        let before_snap = match st.last_version_seq != st.version {
            true => Some(self.snapshot(st)?),
            false => None,
        };
        let befores = befores_of(&st.doc, &edits)?;
        // the merge, by key, on the live doc as the external participant; then the replica
        // takes the file's grid
        let made = match st.doc.edit_applied(client, &edits) {
            Ok(m) => m,
            Err(e) => {
                self.recover(st, &e.to_string());
                return Err(e.into());
            }
        };
        // from here to the store commit the doc holds a save the store hasn't got: every failure
        // reloads it, so the save stays in the file for the next check
        let held = self.holds_file(st, bytes, &rec.codec);
        let held = self.reload_on_failure(st, held)?;
        let replica_state = match self.grid_replica_to_file(st, held, &file_edits) {
            Ok(s) => s,
            Err(e) => {
                self.recover(st, &e.to_string());
                return Err(e);
            }
        };
        let Some(update) = made.update else {
            return Ok(DiskCheck::Unchanged);
        };
        let seq = st.version + 1;
        let a = self.structured_applied(
            st,
            Made {
                writer: EXTERNAL_PID,
                update,
                stream: made.applied,
            },
            &edits,
            &befores,
            &ElemRestores::new(),
        );
        #[cfg(any(test, feature = "differential"))]
        let a = if crate::differential::take_bookkeeping_fault() {
            Err(crate::store::StoreError::Db("a planted failure".into()).into())
        } else {
            a
        };
        #[allow(unused_mut)]
        let mut a = self.reload_on_failure(st, a)?;
        #[cfg(feature = "conformance")]
        if st.outside_without_identity {
            for o in a.elems.iter_mut().flat_map(|r| r.ops.iter_mut()) {
                o.write = None;
            }
        }
        let mut versions = Vec::new();
        if let Some(snap) = before_snap {
            versions.push(Self::version_of(
                st,
                st.version,
                VersionReason::BeforeExternal,
                None,
                snap,
                now,
            ));
        }
        let mut info = ChangeInfo::new(ChangeKind::External, EXTERNAL_PID, "disk", now);
        info.elems = a.counts.clone();
        let note_plan =
            self.plan_overrides(st, &overridden, "an outside save", before_seq, seq, now);
        let note_plan = self.reload_on_failure(st, note_plan)?;
        let change = NewChange {
            info,
            record: RevertRecord {
                author: EXTERNAL_PID.to_owned(),
                on_behalf_of: None,
                at_ms: now,
                roots: Vec::new(),
                elems: a.elems.clone(),
            },
            span: None,
            provenance: Vec::new(),
            authored: None,
            row_marks,
            col_remap,
            note_rows: note_plan.rows.clone(),
            blob_refs: BlobRefScan::of(self.pack.blob_ref_roots()),
        };
        st.codec = rec.codec;
        let snap = self.snapshot(st);
        let (content, _) = self.reload_on_failure(st, snap)?;
        versions.push(Version {
            seq,
            at_ms: now,
            reason: VersionReason::External,
            name: None,
            authors: vec![EXTERNAL_PID.to_owned()],
            content,
            codec: Some(st.codec.clone()),
        });
        meta.synced_seq = seq;
        let sync = SyncState {
            hash: h.clone(),
            seq,
            state: replica_state,
        };
        let entry = LogEntry {
            author: EXTERNAL_PID.to_owned(),
            client,
            at_ms: now,
            via: "disk".to_owned(),
            body: EntryBody::Yrs(a.update.clone()),
            on_behalf_of: None,
        };
        if let Err(e) = store.commit_disk(
            self.doc_id,
            seq,
            std::slice::from_ref(&entry),
            std::slice::from_ref(&change),
            &versions,
            &meta,
            &sync,
        ) {
            self.recover(st, &e.to_string());
            return Err(e.into());
        }
        for x in &a.stream {
            Self::broadcast(
                st,
                &ChangeEvent {
                    version: seq,
                    pid: EXTERNAL_PID.to_owned(),
                    client,
                    via: "disk".to_owned(),
                    root: x.root().to_owned(),
                    hunks: Vec::new(),
                    at_ms: now,
                    on_behalf_of: None,
                    applied: x.clone(),
                    hidden: if x.root() == table::CELLS {
                        a.hidden.clone()
                    } else {
                        Vec::new()
                    },
                    kind: change.info.kind,
                },
            );
        }
        self.grid_size_after(st, &a.stream);
        st.last_version_seq = seq;
        st.last_change_ms = now;
        st.version = seq;
        self.grid_presence_after(st, None, &a.stream, now);
        Self::elem_presence_after(st);
        st.log_entries += 1;
        st.log_bytes += entry.body.yrs_len();
        st.state_bytes += entry.body.yrs_len();
        st.authors.push_back((seq, EXTERNAL_PID.to_owned()));
        if st.authors.len() > AUTHORS_KEPT {
            st.authors.pop_front();
        }
        let d = st.disk.as_mut().expect("has disk");
        d.synced_hash = h;
        d.synced_seq = seq;
        d.replica_bytes = sync.state.len() as u64;
        // as above (v0.9.5)
        d.own.clear();
        d.echoed = None;
        if was_declined {
            st.declined = None;
            self.emit(AuditEvent::FileRestored {
                key: st.key.clone(),
            });
        }
        self.emit(AuditEvent::ExternalEdit {
            key: st.key.clone(),
            seq,
        });
        if !overridden.is_empty() {
            self.finish_overrides(st, overridden, note_plan, before_seq, seq)?;
        }
        // a file's delete of a row the live side only moved: no note, but on the audit record
        let store = self.store(st)?.clone();
        for k in moved_then_deleted {
            let by = match store.elem_write(
                self.doc_id,
                table::ROWS,
                k.as_str(),
                ElemFacet::Position,
            )? {
                Some(w) => store.change(self.doc_id, w.seq)?.map(|c| c.author),
                None => None,
            };
            self.emit(AuditEvent::OutsideOverride {
                key: st.key.clone(),
                change: seq,
                rule: table::ConflictRule::MovedRow.as_str(),
                region: format!("row {k}"),
                by,
                version: None,
            });
        }
        self.settle_saved(st, now)?;
        self.maybe_compact(st);
        Ok(DiskCheck::Ingested { version: seq })
    }

    /// `r`, reloading the doc first if it failed: the doc took an outside save that won't reach
    /// the store, so it can't keep it.
    fn reload_on_failure<T>(&self, st: &mut State, r: Result<T>) -> Result<T> {
        if let Err(e) = &r {
            #[cfg(any(test, feature = "differential"))]
            if crate::differential::resync_mutant()
                == Some(crate::differential::ResyncMutant::IngestKeepsTheDoc)
            {
                return r;
            }
            self.recover(st, &e.to_string());
        }
        r
    }

    /// Whether the live doc, rendered with `codec`, is the file's `bytes`.
    fn holds_file(&self, st: &State, bytes: &[u8], codec: &[u8]) -> Result<bool> {
        Ok(self.pack.encode(&self.content(st)?, codec)? == bytes)
    }

    /// After an outside save is merged into a structured doc: the disk replica takes the
    /// file's grid (`file_edits`: the save on the replica's keyed state), as
    /// `replica_to_file` does for text in v0.9.5. Returns its state, to store as the synced
    /// state. When the doc now holds the file (`held`), the replica is the doc: the same
    /// elements. Otherwise (the doc holds changes the file doesn't have yet; the write-back
    /// follows) the save is made on the replica as a client of its own, one the doc never
    /// holds; nothing is ever merged from it into the doc.
    pub(super) fn grid_replica_to_file(
        &self,
        st: &mut State,
        held: bool,
        file_edits: &[Edit],
    ) -> Result<Vec<u8>> {
        if held {
            let state = st.doc.encode_state()?;
            let replica = self.checked(Doc::load(self.schema, &state)?);
            st.disk.as_mut().expect("has disk").replica = Replica::Doc(replica);
            return Ok(state);
        }
        let d = st.disk.as_mut().expect("has disk");
        if !file_edits.is_empty() {
            let own = loop {
                let c = mint_client_id();
                if st.doc.clock_of(c) == 0 && d.replica.get()?.clock_of(c) == 0 {
                    break c;
                }
            };
            d.replica.get()?.edit_applied(own, file_edits)?;
            // while the doc is dirty the replica holds the file's state as its own ops, so its
            // state vector can't match the doc's until it reloads from the doc: no catch-up this
            // cycle, so the next write-back reloads it instead of comparing (a compare would
            // report a mismatch that isn't one)
            d.catch_up = None;
        }
        Ok(d.replica.get()?.encode_state()?)
    }

    /// Each overridden live element of an outside save (contract §2.9.1), before the save
    /// applies: who held it (the live write's author: the cell's or name's writer, the row's
    /// deleter, mover, or the writers of the cells a file delete takes), and its address in
    /// the live file.
    pub(super) fn overridden(
        &self,
        st: &State,
        live: &Table,
        overrides: &[table::Conflict],
    ) -> Result<Vec<Overridden>> {
        let store = self.store(st)?;
        // nothing to report asks nothing of the pack: an outside save's check renders no more
        if overrides.is_empty() {
            return Ok(Vec::new());
        }
        let lines = self.conflict_lines(&View::new(st.doc.keyed_state()?), &st.codec);
        let author = |root: &str, key: &str, f: ElemFacet| -> Result<Option<String>> {
            Ok(match store.elem_write(self.doc_id, root, key, f)? {
                Some(w) => store.change(self.doc_id, resolved(&w).0)?.map(|c| c.author),
                None => None,
            })
        };
        // each row's lines and each live column's index read once (O(1) per override after)
        let spans = lines.as_ref().map(|m| m.row_spans());
        let cols_at = live.cols.live_index();
        let line_of = |r: &Key| spans.as_ref().and_then(|m| m.get(r)).map(|l| l.start);
        let col_of = |c: &Key| cols_at.get(c).map(|i| i + 1);
        let mut out = Vec::with_capacity(overrides.len());
        for c in overrides {
            let mut by: Vec<String> = Vec::new();
            let mut add = |a: Option<String>| {
                if let Some(a) = a {
                    if !by.contains(&a) {
                        by.push(a);
                    }
                }
            };
            let (line, col) = match &c.at {
                table::Region::Cell { row, col } => {
                    match c.rule {
                        // the file wrote into a row (column) the live side deleted
                        table::ConflictRule::DeletedRow => {
                            add(author(table::ROWS, row.as_str(), ElemFacet::Existence)?)
                        }
                        table::ConflictRule::DeletedCol => {
                            add(author(table::COLS, col.as_str(), ElemFacet::Existence)?)
                        }
                        _ => add(author(
                            table::CELLS,
                            &table::cell_key(row, col),
                            ElemFacet::Value,
                        )?),
                    }
                    (line_of(row), col_of(col))
                }
                table::Region::Row(r) | table::Region::Col(r) => {
                    let root = match c.at {
                        table::Region::Row(_) => table::ROWS,
                        _ => table::COLS,
                    };
                    match c.rule {
                        table::ConflictRule::EditedRow | table::ConflictRule::EditedCol => {
                            for w in &c.theirs_writes {
                                match w {
                                    table::Region::Cell { row, col } => add(author(
                                        table::CELLS,
                                        &table::cell_key(row, col),
                                        ElemFacet::Value,
                                    )?),
                                    table::Region::Name(k) => {
                                        add(author(table::NAMES, k.as_str(), ElemFacet::Value)?)
                                    }
                                    table::Region::Row(k) => {
                                        add(author(table::ROWS, k.as_str(), ElemFacet::Position)?)
                                    }
                                    table::Region::Col(k) => {
                                        add(author(table::COLS, k.as_str(), ElemFacet::Position)?)
                                    }
                                    table::Region::Quote(_) => {}
                                }
                            }
                        }
                        table::ConflictRule::DeletedRow | table::ConflictRule::DeletedCol => {
                            add(author(root, r.as_str(), ElemFacet::Existence)?)
                        }
                        _ => add(author(root, r.as_str(), ElemFacet::Position)?),
                    }
                    if root == table::ROWS {
                        (line_of(r), None)
                    } else {
                        (None, col_of(r))
                    }
                }
                table::Region::Name(k) => {
                    add(author(table::NAMES, k.as_str(), ElemFacet::Value)?);
                    (Some(1), col_of(k))
                }
                table::Region::Quote(at) => {
                    add(author(table::QUOTING, &at.map_key(), ElemFacet::Value)?);
                    match at {
                        table::QuoteAt::Cell(r, k) => (line_of(r), col_of(k)),
                        table::QuoteAt::Header(k) => (Some(1), col_of(k)),
                    }
                }
            };
            let row = match &c.at {
                table::Region::Cell { row, .. } | table::Region::Row(row) => Some(row.to_string()),
                table::Region::Quote(table::QuoteAt::Cell(row, _)) => Some(row.to_string()),
                _ => None,
            };
            // a file's delete over live edits (or a drop): the note carries the live values the
            // element held, not its place
            let yours = match (&c.rule, &c.at) {
                (
                    table::ConflictRule::EditedRow | table::ConflictRule::EditedCol,
                    table::Region::Row(_) | table::Region::Col(_),
                ) if !c.is_rewritten_row() => {
                    let vals: Vec<String> = c
                        .theirs_writes
                        .iter()
                        .filter_map(|w| match w {
                            table::Region::Cell { row, col } => {
                                live.cell(row, col).map(|v| format!("{v:?}"))
                            }
                            _ => None,
                        })
                        .collect();
                    if vals.is_empty() {
                        c.theirs.to_string()
                    } else {
                        vals.join(", ")
                    }
                }
                _ => c.theirs.to_string(),
            };
            out.push(Overridden {
                rule: c.rule.as_str(),
                row,
                region: c.at.to_string().replace(table::CELL_SEP, "/"),
                line,
                col,
                yours,
                file: c.ours.to_string(),
                by,
            });
        }
        Ok(out)
    }

    /// The notes a change that overrides live values leaves (contract §2.9.1): one per row per
    /// author, values capped, the waiting queues as they will stand (bounded per author, rolled
    /// up past it). Written in the change's transaction ([`NewChange::note_rows`]), so a crash
    /// can't lose one and keep the change; the mirror and the push follow the commit.
    pub(super) fn plan_overrides(
        &self,
        st: &mut State,
        overridden: &[Overridden],
        source: &'static str,
        before: Seq,
        change: Seq,
        now: u64,
    ) -> Result<OverridePlan> {
        self.load_notes(st, now)?;
        let mut queues: HashMap<String, Vec<awareness::Note>> = HashMap::new();
        for o in overridden {
            let (yours, c1) = awareness::cap_value(&o.yours);
            let (file, c2) = awareness::cap_value(&o.file);
            let ow = Overwrite {
                rule: o.rule,
                source,
                region: o.region.clone(),
                row: o.row.clone(),
                line: o.line,
                col: o.col,
                yours,
                file,
                cut: c1 || c2,
                change,
                version: None,
                restore_from: None,
                also: Vec::new(),
                more: 0,
                not_shown: 0,
                withheld: false,
                no_history: false,
                before,
                at_ms: now,
                text: false,
            };
            for pid in o.by.iter().filter(|p| !p.starts_with(RESERVED_PID_PREFIX)) {
                let q = queues
                    .entry(pid.clone())
                    .or_insert_with(|| st.aware.pending.get(pid).cloned().unwrap_or_default());
                Self::queue_grid_overwrite(q, ow.clone());
            }
        }
        let rows = queues
            .iter()
            .map(|(pid, q)| (pid.clone(), Self::encode_notes(q)))
            .collect();
        Ok(OverridePlan { queues, rows })
    }

    /// After the change committed: the waiting queues as planned, the roster push (each note
    /// this change made or grew, to its author's own subscriptions), one audit event per
    /// overridden element, with no content.
    pub(super) fn finish_overrides(
        &self,
        st: &mut State,
        overridden: Vec<Overridden>,
        plan: OverridePlan,
        before: Seq,
        change: Seq,
    ) -> Result<()> {
        // notes for a change that didn't land: a commit that changed nothing. Reported only.
        if st.version < change && !plan.rows.is_empty() {
            self.emit(AuditEvent::NotesWithoutChange {
                key: st.key.clone(),
                change,
                version: st.version,
                authors: plan.rows.len(),
            });
        }
        let version = self.version_before(self.store(st)?, before);
        for (pid, q) in &plan.queues {
            for o in q.iter().filter_map(|n| n.overwrite.as_ref()) {
                if o.change == change {
                    let mut o = o.clone();
                    o.version = o.version_of(version);
                    st.roster.record(
                        RosterEvent::Overwritten {
                            to: Who::of(pid),
                            overwrite: o,
                        },
                        self.config.roster_queue,
                    );
                }
            }
        }
        st.aware.pending.extend(plan.queues);
        for o in overridden {
            self.emit(AuditEvent::OutsideOverride {
                key: st.key.clone(),
                change,
                rule: o.rule,
                region: o.region,
                by: o.by.first().cloned(),
                version,
            });
        }
        Ok(())
    }
}

/// What [`Session::plan_overrides`] made.
pub(super) struct OverridePlan {
    queues: HashMap<String, Vec<awareness::Note>>,
    rows: Vec<(String, Vec<u8>)>,
}

impl OverridePlan {
    pub(super) fn note_rows(&self) -> Vec<(String, Vec<u8>)> {
        self.rows.clone()
    }
}

/// One live element an outside save overrode (before it applied).
pub(super) struct Overridden {
    pub(super) rule: &'static str,
    pub(super) region: String,
    /// The row it is on, for a cell or a row.
    pub(super) row: Option<String>,
    pub(super) line: Option<usize>,
    pub(super) col: Option<usize>,
    pub(super) yours: String,
    pub(super) file: String,
    /// Who held it: the live write's author(s), first the one the audit event names.
    pub(super) by: Vec<String>,
}

/// Your own value isn't overridden by you: an entry only you held goes. One with **no recorded
/// author stays**: nobody can be told, but its audit event is the record that it happened.
pub(super) fn drop_own(overridden: &mut Vec<Overridden>, pid: &str) {
    overridden.retain_mut(|o| {
        if o.by.is_empty() {
            return true;
        }
        o.by.retain(|b| b != pid);
        !o.by.is_empty()
    });
}

/// The live cells a restore's `edits` take, as overrides: a cell set or removed to something
/// else, or a live row deleted with its cells.
fn restore_overrides(live: &Table, edits: &[Edit]) -> Vec<table::Conflict> {
    let mut out = Vec::new();
    let cell = |r: &Key, c: &Key, new: Option<String>| table::Conflict {
        rule: table::ConflictRule::Cell,
        at: table::Region::Cell {
            row: r.clone(),
            col: c.clone(),
        },
        base: table::Held::Value(new.clone()),
        ours: table::Held::Value(new),
        theirs: table::Held::Value(live.cell(r, c).map(str::to_owned)),
        theirs_writes: Vec::new(),
    };
    for e in edits {
        match e {
            Edit::Map { root, ops } if root == table::CELLS => {
                for o in ops {
                    let Some((r, c)) = table::parse_cell_key(o.key()) else {
                        continue;
                    };
                    // a cell under a deleted row or column counts too: its value is its
                    // author's, hidden or not (the restore takes it all the same)
                    if live.cell(&r, &c).is_none() {
                        continue;
                    }
                    let new = match o {
                        MapOp::Set { value, .. } => value.as_str().map(str::to_owned),
                        _ => None,
                    };
                    if new.as_deref() != live.cell(&r, &c) {
                        out.push(cell(&r, &c, new));
                    }
                }
            }
            Edit::List { root, ops } if root == table::ROWS => {
                for o in ops {
                    if let ListOp::Delete { key } = o {
                        for ((r, c), _) in live.cells.iter().filter(|((r, _), _)| r == key) {
                            out.push(cell(r, c, None));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // one entry per cell (a cell both set and in a deleted row is taken once)
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.at.clone()));
    out
}

/// The rows a text kept on their keys (live before and after it) that it didn't leave exact: a
/// guess about identity, marked (the store's `row_marks`). Under a non-exact column mapping
/// the doc-level column remap version does it instead, for every key.
fn kept_not_exact(base: &Table, ours: &Table, exact: &[Key], cols_exact: bool) -> Vec<Key> {
    if !cols_exact {
        return Vec::new();
    }
    base.rows
        .live()
        .filter(|k| ours.rows.is_live(k) && !exact.contains(k))
        .cloned()
        .collect()
}

/// Where a move goes back to (spec v1.10 S4): right after the first of `left` (its left
/// neighbours in place order before the change, nearest first, deleted ones included) that
/// the list still knows (a dropped key isn't), or the start.
pub(super) fn back_after(live: &KeyedState, root: &str, left: &[String], moving: &Key) -> Anchor {
    let Some((_, KeyedRoot::List { places, .. })) = live.roots.iter().find(|(r, _)| r == root)
    else {
        return Anchor::Start;
    };
    left.iter()
        .filter(|l| l.as_str() != moving.as_str())
        .find_map(|l| places.iter().find(|k| k.as_str() == l.as_str()))
        .map_or(Anchor::Start, |k| Anchor::After(k.clone()))
}

/// A table state's key counters raised to `live`'s (so keys minted against it are new to the
/// live doc).
/// The file's version of an element the live doc dropped since the sync (`rows
/// drop-deleted`): if the file changed it (a cell, its name, its place), it is renamed in
/// `ours` to a key none of the three tables knows, so the merge inserts it as the file has it
/// (contract §2.9.1: never dropped); unchanged, it is left alone (the live drop stands).
fn rekey_dropped_in_file(base: &Table, ours: Table, live: &Table) -> Table {
    if table::rekey_walk() {
        return rekey_dropped_walk(base, ours, live);
    }
    // the candidates: live in the base, unknown to the live side, still in the text; each
    // checked against indexes read once (each version's live places, the candidate columns'
    // cells), and a row's cells by its own range: O(the tables) for any number dropped
    let drop_rows: Vec<&Key> = base
        .rows
        .live()
        .filter(|r| !live.rows.knows(r) && ours.rows.knows(r))
        .collect();
    let drop_cols: Vec<&Key> = base
        .cols
        .live()
        .filter(|c| !live.cols.knows(c) && ours.cols.knows(c))
        .collect();
    if drop_rows.is_empty() && drop_cols.is_empty() {
        return ours;
    }
    let changed: (HashSet<Key>, HashSet<Key>) = {
        let ours = &ours;
        let mut rows_changed = HashSet::new();
        if !drop_rows.is_empty() {
            let (b, o) = (
                table::merge::ElemIndex::new(&base.rows),
                table::merge::ElemIndex::new(&ours.rows),
            );
            for r in &drop_rows {
                if b.of(r) != o.of(r) || base.row_cells(r).ne(ours.row_cells(r)) {
                    rows_changed.insert((*r).clone());
                }
            }
        }
        let mut cols_changed = HashSet::new();
        if !drop_cols.is_empty() {
            let (b, o) = (
                table::merge::ElemIndex::new(&base.cols),
                table::merge::ElemIndex::new(&ours.cols),
            );
            let cand: HashSet<&Key> = drop_cols.iter().copied().collect();
            let (bc, oc) = (cells_by_col(base, &cand), cells_by_col(ours, &cand));
            for c in &drop_cols {
                if b.of(c) != o.of(c) || base.name(c) != ours.name(c) || bc.get(c) != oc.get(c) {
                    cols_changed.insert((*c).clone());
                }
            }
        }
        (rows_changed, cols_changed)
    };
    let mut next = (
        base.next_row.max(ours.next_row).max(live.next_row),
        base.next_col.max(ours.next_col).max(live.next_col),
    );
    let fresh = |prefix: char, n: &mut u64| -> Key {
        loop {
            let k = Key::new(format!("{prefix}{}", *n)).expect("a minted key");
            *n += 1;
            let known = |t: &Table| {
                if prefix == 'r' {
                    t.rows.knows(&k)
                } else {
                    t.cols.knows(&k)
                }
            };
            if !known(base) && !known(&ours) && !known(live) {
                return k;
            }
        }
    };
    // in the base's order, as the walk mints them
    let mut rows: HashMap<Key, Key> = HashMap::new();
    for r in drop_rows.iter().filter(|r| changed.0.contains(**r)) {
        rows.insert((*r).clone(), fresh('r', &mut next.0));
    }
    let mut cols: HashMap<Key, Key> = HashMap::new();
    for c in drop_cols.iter().filter(|c| changed.1.contains(**c)) {
        cols.insert((*c).clone(), fresh('c', &mut next.1));
    }
    if rows.is_empty() && cols.is_empty() {
        return ours;
    }
    let mut t = ours.renamed(
        &|k| rows.get(k).cloned().unwrap_or_else(|| k.clone()),
        &|k| cols.get(k).cloned().unwrap_or_else(|| k.clone()),
    );
    t.next_row = t.next_row.max(next.0);
    t.next_col = t.next_col.max(next.1);
    t
}

/// The cells of the columns in `cand`, each column's in row order: one pass over `t`'s cells.
fn cells_by_col<'a>(
    t: &'a Table,
    cand: &HashSet<&Key>,
) -> HashMap<&'a Key, Vec<(&'a Key, &'a String)>> {
    table::visited(t.cells.len());
    let mut m: HashMap<&Key, Vec<(&Key, &String)>> = HashMap::new();
    for ((r, c), v) in &t.cells {
        if cand.contains(c) {
            m.entry(c).or_default().push((r, v));
        }
    }
    m
}

/// [`rekey_dropped_in_file`] by the walk: each candidate's cells found by a scan of both
/// versions' cells, its place by a walk of each axis (the planted
/// [`crate::differential::GridCostMutant::RekeyWalk`] only).
fn rekey_dropped_walk(base: &Table, ours: Table, live: &Table) -> Table {
    let changed_row = |r: &Key| {
        table::visited(base.cells.len() + ours.cells.len());
        let at = |t: &Table| table::ElemAt::of(&t.rows, r);
        at(base) != at(&ours)
            || base
                .cells
                .iter()
                .filter(|((x, _), _)| x == r)
                .map(|((_, c), v)| (c, v))
                .ne(ours
                    .cells
                    .iter()
                    .filter(|((x, _), _)| x == r)
                    .map(|((_, c), v)| (c, v)))
    };
    let changed_col = |c: &Key| {
        table::visited(base.cells.len() + ours.cells.len());
        let at = |t: &Table| table::ElemAt::of(&t.cols, c);
        at(base) != at(&ours)
            || base.name(c) != ours.name(c)
            || base
                .cells
                .iter()
                .filter(|((_, y), _)| y == c)
                .map(|((r, _), v)| (r, v))
                .ne(ours
                    .cells
                    .iter()
                    .filter(|((_, y), _)| y == c)
                    .map(|((r, _), v)| (r, v)))
    };
    let mut next = (
        base.next_row.max(ours.next_row).max(live.next_row),
        base.next_col.max(ours.next_col).max(live.next_col),
    );
    let fresh = |prefix: char, n: &mut u64| -> Key {
        loop {
            let k = Key::new(format!("{prefix}{}", *n)).expect("a minted key");
            *n += 1;
            let known = |t: &Table| {
                if prefix == 'r' {
                    t.rows.knows(&k)
                } else {
                    t.cols.knows(&k)
                }
            };
            if !known(base) && !known(&ours) && !known(live) {
                return k;
            }
        }
    };
    let mut rows: HashMap<Key, Key> = HashMap::new();
    for r in base.rows.live() {
        if !live.rows.knows(r) && ours.rows.knows(r) && changed_row(r) {
            rows.insert(r.clone(), fresh('r', &mut next.0));
        }
    }
    let mut cols: HashMap<Key, Key> = HashMap::new();
    for c in base.cols.live() {
        if !live.cols.knows(c) && ours.cols.knows(c) && changed_col(c) {
            cols.insert(c.clone(), fresh('c', &mut next.1));
        }
    }
    if rows.is_empty() && cols.is_empty() {
        return ours;
    }
    let mut t = ours.renamed(
        &|k| rows.get(k).cloned().unwrap_or_else(|| k.clone()),
        &|k| cols.get(k).cloned().unwrap_or_else(|| k.clone()),
    );
    t.next_row = t.next_row.max(next.0);
    t.next_col = t.next_col.max(next.1);
    t
}

fn bump_counters(state: &mut KeyedState, live: &Table) {
    for (r, x) in &mut state.roots {
        if r != table::META {
            continue;
        }
        if let KeyedRoot::Map(m) = x {
            for (k, n) in [("next_row", live.next_row), ("next_col", live.next_col)] {
                let cur = m
                    .get(k)
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                if n > cur {
                    m.insert(k.to_owned(), Value::str(n.to_string()));
                }
            }
        }
    }
}

/// (xxxvii) planted ([`crate::differential::MergeMutant::KeyedWriteUnderUnknownKey`]): the cell
/// key check is skipped. Without the `differential` feature there is no hook: `false`.
#[cfg(any(test, feature = "differential"))]
fn planted_keyed_write_under_unknown_key() -> bool {
    crate::differential::merge_mutant()
        == Some(crate::differential::MergeMutant::KeyedWriteUnderUnknownKey)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_keyed_write_under_unknown_key() -> bool {
    false
}
