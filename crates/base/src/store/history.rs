//! History records next to the log: a **change summary** per change (the `log`, kept for
//! the doc's life), its **clock span** (which engine clocks its inserted text has: blame's
//! index from a character to its change, kept for the doc's life), a **revert record** per change (its hunks with the deleted text, author,
//! `on_behalf_of`; kept inside the revert window), the **carried bounds** of pending
//! records older than the window's cut, and per-doc **stats** (record count and bytes,
//! pending on-behalf count and bytes). All are written in the transaction of the change
//! itself, so they never disagree with the log, and none of them depends on the log: a
//! compaction can drop log entries and revert still works (K-HIST-20, -22).
//!
//! **Purge reach (K-PURGE-3).** Content lives only in `REVERTS` rows (`RecordHunk::insert`
//! / `deleted`). A purge redacts those (sets `redacted`, blanks the text; a revert then
//! reports the region as `erased`); `CHANGES` (which keeps every change's id, author, time
//! and hunk lengths), `CHANGE_CLOCKS` (client IDs, clocks and change ids), `REVERT_INDEX`,
//! `REVERT_CARRY` and `REVERT_STATS` hold positions, sizes and metadata only.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use redb::{ReadableTable, TableDefinition};

use super::enc::{R, W};
use super::{
    dec_redactions, dec_version, DocId, DocStore, Seq, StoreError, Version, VersionId,
    VersionReason,
};

pub(super) const CHANGES: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("changes");
pub(super) const REVERTS: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("reverts");
/// `(doc, seq)` → `(at_ms, record bytes)`: the window's index without reading content.
pub(super) const REVERT_INDEX: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("revert_index");
pub(super) const REVERT_CARRY: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("revert_carry");
pub(super) const REVERT_STATS: TableDefinition<u64, &[u8]> = TableDefinition::new("revert_stats");
/// `(doc, client, first clock)` → `(end clock, seq)`: the engine clocks each change's inserted
/// text occupies ([`ClockSpan`]), so blame can map a live character to its change. Added in
/// v0.3.0 (store format 3, additive: changes made before have no row).
pub(super) const CHANGE_CLOCKS: TableDefinition<(u64, u64, u64), &[u8]> =
    TableDefinition::new("change_clocks");

pub(super) fn create_tables(w: &redb::WriteTransaction) -> Result<(), StoreError> {
    w.open_table(CHANGES)?;
    w.open_table(REVERTS)?;
    w.open_table(REVERT_INDEX)?;
    w.open_table(REVERT_CARRY)?;
    w.open_table(REVERT_STATS)?;
    w.open_table(CHANGE_CLOCKS)?;
    Ok(())
}

/// What kind of change a log position is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChangeKind {
    /// An edit call (`edit`, `edit_with`, `replace_text`, `write_base`, a tool verb).
    Edit,
    Undo,
    Redo,
    /// A restore of version [`ChangeInfo::of`].
    Restore,
    /// A revert of change [`ChangeInfo::of`].
    Revert,
    /// An outside edit of the file (`kit:external`).
    External,
    /// The doc's import (`kit:import`).
    Import,
}

impl ChangeKind {
    fn to_u8(self) -> u8 {
        match self {
            Self::Edit => 1,
            Self::Undo => 2,
            Self::Redo => 3,
            Self::Restore => 4,
            Self::Revert => 5,
            Self::External => 6,
            Self::Import => 7,
        }
    }
    fn from_u8(v: u8) -> Result<Self, StoreError> {
        Ok(match v {
            1 => Self::Edit,
            2 => Self::Undo,
            3 => Self::Redo,
            4 => Self::Restore,
            5 => Self::Revert,
            6 => Self::External,
            7 => Self::Import,
            _ => return Err(StoreError::Corrupt("change kind")),
        })
    }
}

/// One change as `log` lists it. Metadata only.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChangeInfo {
    /// Its log position: the stable change id.
    pub seq: Seq,
    pub kind: ChangeKind,
    /// For a restore, the version restored; for a revert, the change reverted.
    pub of: Option<u64>,
    pub author: String,
    pub via: String,
    pub at_ms: u64,
    pub on_behalf_of: Option<String>,
    pub lines_added: u64,
    pub lines_removed: u64,
}

impl ChangeInfo {
    pub(crate) fn new(kind: ChangeKind, author: &str, via: &str, at_ms: u64) -> Self {
        Self {
            seq: 0,
            kind,
            of: None,
            author: author.to_owned(),
            via: via.to_owned(),
            at_ms,
            on_behalf_of: None,
            lines_added: 0,
            lines_removed: 0,
        }
    }
}

/// A change's hunks on one root, as positions and lengths (what carrying needs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RootShape {
    pub root: String,
    /// `(start, end, inserted length)`, canonical, in the coordinates before the change.
    pub hunks: Vec<(u64, u64, u64)>,
}

/// One canonical hunk of a change with the text it deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordHunk {
    pub start: u64,
    pub end: u64,
    pub insert: String,
    pub deleted: String,
    /// A purge blanked this hunk's text (revert reports it `erased`).
    pub redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordRoot {
    pub root: String,
    /// The root was empty right after the change (undo spec U10, m11).
    pub empty_after: bool,
    pub hunks: Vec<RecordHunk>,
}

/// The persisted revert record of one change (undo spec U22, K-HIST-22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevertRecord {
    pub author: String,
    pub on_behalf_of: Option<String>,
    pub at_ms: u64,
    pub roots: Vec<RecordRoot>,
}

impl RevertRecord {
    pub fn shape(&self) -> Vec<RootShape> {
        self.roots
            .iter()
            .map(|r| RootShape {
                root: r.root.clone(),
                hunks: r
                    .hunks
                    .iter()
                    .map(|h| (h.start, h.end, h.insert.len() as u64))
                    .collect(),
            })
            .collect()
    }
}

/// The engine clocks a change's inserted text occupies: client `client`, clocks
/// `start..end` (the engine's own units). Engine clocks per client only grow, so the spans
/// of one client's changes never overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClockSpan {
    pub client: u64,
    pub start: u32,
    pub end: u32,
}

/// `(start, end, seq)`: change `seq` inserted the text at clocks `start..end` of one client.
pub(crate) type SpanOf = (u32, u32, Seq);

/// A change as appended: its summary, its revert record and, when it inserted anything, its
/// clock span.
#[derive(Debug, Clone)]
pub(crate) struct NewChange {
    pub info: ChangeInfo,
    pub record: RevertRecord,
    pub span: Option<ClockSpan>,
}

/// The carried bounds of one record, as of log position `carried_to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Carry {
    pub carried_to: Seq,
    /// `(root, start, end, start tainted, end tainted)` per region, in record order: each
    /// edge's taint on its own (undo spec U22a), so a rebuilt index equals the live one.
    pub regions: Vec<(String, u64, u64, bool, bool)>,
}

/// A carried region's taint byte. Written by v0.8.1 and earlier: `0` (clean) or
/// [`TAINT_LEGACY`] (one flag for the region, read as both edges tainted: the conservative
/// reading those versions also made). Written since: [`TAINT_START`] | [`TAINT_END`], each
/// edge apart. Same carry record version (1) and store format: an older build reads any
/// non-zero byte as "tainted", its own conservative reading.
const TAINT_LEGACY: u8 = 1;
const TAINT_START: u8 = 2;
const TAINT_END: u8 = 4;

fn enc_taint(start: bool, end: bool) -> u8 {
    (if start { TAINT_START } else { 0 }) | (if end { TAINT_END } else { 0 })
}

fn dec_taint(b: u8) -> Result<(bool, bool), StoreError> {
    match b {
        0 => Ok((false, false)),
        TAINT_LEGACY => Ok((true, true)),
        b if b & !(TAINT_START | TAINT_END) == 0 => Ok((b & TAINT_START != 0, b & TAINT_END != 0)),
        _ => Err(StoreError::Corrupt("revert carry taint")),
    }
}

/// Per-doc revert record totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RevertStats {
    pub count: u64,
    pub bytes: u64,
    pub pending_count: u64,
    pub pending_bytes: u64,
}

/// A span of a version a purge removed (K-PURGE-2, -4): `len` bytes of root `root` from
/// byte `start`, in the coordinates of the version's text before any purge. The version
/// keeps its entry; its text is stored, shown, diffed and restored without the span.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Redaction {
    pub root: String,
    pub start: u64,
    pub len: u64,
}

/// A version without its content (`log`, `versions`, retention).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct VersionInfo {
    pub id: VersionId,
    /// The log position it reflects.
    pub seq: Seq,
    pub at_ms: u64,
    pub reason: VersionReason,
    pub name: Option<String>,
    pub authors: Vec<String>,
    /// Stored size (content included).
    pub bytes: u64,
    /// The spans a purge removed from its content (empty when none): `show`, `diff` and
    /// `restore` see the text without them, and it is no `write --base` (format 3).
    pub redacted: Vec<Redaction>,
}

/// Version retention (K-HIST-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Retention {
    pub keep_all_ms: u64,
    pub hourly_until_ms: u64,
    pub max_count: usize,
    pub max_bytes: u64,
}

/// The revert window (K-HIST-23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RevertWindow {
    pub keep_ms: u64,
    pub keep_changes: u64,
    pub max_bytes: u64,
}

// ----- encodings -----

pub(super) fn enc_change(c: &ChangeInfo, shape: &[RootShape]) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1)
        .u8(c.kind.to_u8())
        .u8(u8::from(c.of.is_some()))
        .u64(c.of.unwrap_or(0))
        .str(&c.author)
        .str(&c.via)
        .u64(c.at_ms)
        .opt_str(c.on_behalf_of.as_deref())
        .u64(c.lines_added)
        .u64(c.lines_removed)
        .u32(shape.len() as u32);
    for r in shape {
        w.str(&r.root).u32(r.hunks.len() as u32);
        for &(s, e, n) in &r.hunks {
            w.u64(s).u64(e).u64(n);
        }
    }
    w.0
}

pub(super) fn dec_change(
    seq: Seq,
    b: &[u8],
    with_shape: bool,
) -> Result<(ChangeInfo, Vec<RootShape>), StoreError> {
    let mut r = R::new(b, "change");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("change record version"));
    }
    let kind = ChangeKind::from_u8(r.u8()?)?;
    let has_of = r.u8()?;
    let of = r.u64()?;
    let info = ChangeInfo {
        seq,
        kind,
        of: match has_of {
            0 => None,
            1 => Some(of),
            _ => return Err(StoreError::Corrupt("change record")),
        },
        author: r.str()?,
        via: r.str()?,
        at_ms: r.u64()?,
        on_behalf_of: r.opt_str()?,
        lines_added: r.u64()?,
        lines_removed: r.u64()?,
    };
    let mut shape = Vec::new();
    if with_shape {
        let n = r.u32()? as usize;
        for _ in 0..n.min(b.len()) {
            let root = r.str()?;
            let k = r.u32()? as usize;
            if k > b.len() {
                return Err(StoreError::Corrupt("change shape"));
            }
            let mut hunks = Vec::with_capacity(k);
            for _ in 0..k {
                hunks.push((r.u64()?, r.u64()?, r.u64()?));
            }
            shape.push(RootShape { root, hunks });
        }
        r.end()?;
    }
    Ok((info, shape))
}

pub(super) fn enc_record(rec: &RevertRecord) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1)
        .str(&rec.author)
        .opt_str(rec.on_behalf_of.as_deref())
        .u64(rec.at_ms)
        .u32(rec.roots.len() as u32);
    for root in &rec.roots {
        w.str(&root.root)
            .u8(u8::from(root.empty_after))
            .u32(root.hunks.len() as u32);
        for h in &root.hunks {
            w.u64(h.start)
                .u64(h.end)
                .u8(u8::from(h.redacted))
                .str(&h.insert)
                .str(&h.deleted);
        }
    }
    w.0
}

pub(super) fn dec_record(b: &[u8]) -> Result<RevertRecord, StoreError> {
    let mut r = R::new(b, "revert record");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("revert record version"));
    }
    let (author, on_behalf_of, at_ms) = (r.str()?, r.opt_str()?, r.u64()?);
    let n = r.u32()? as usize;
    if n > b.len() {
        return Err(StoreError::Corrupt("revert record roots"));
    }
    let mut roots = Vec::with_capacity(n);
    for _ in 0..n {
        let root = r.str()?;
        let empty_after = r.u8()? != 0;
        let k = r.u32()? as usize;
        if k > b.len() {
            return Err(StoreError::Corrupt("revert record hunks"));
        }
        let mut hunks = Vec::with_capacity(k);
        for _ in 0..k {
            hunks.push(RecordHunk {
                start: r.u64()?,
                end: r.u64()?,
                redacted: r.u8()? != 0,
                insert: r.str()?,
                deleted: r.str()?,
            });
        }
        roots.push(RecordRoot {
            root,
            empty_after,
            hunks,
        });
    }
    r.end()?;
    Ok(RevertRecord {
        author,
        on_behalf_of,
        at_ms,
        roots,
    })
}

pub(super) fn enc_pair(a: u64, b: u64) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&a.to_le_bytes());
    out[8..].copy_from_slice(&b.to_le_bytes());
    out
}

pub(super) fn dec_pair(b: &[u8]) -> Result<(u64, u64), StoreError> {
    if b.len() != 16 {
        return Err(StoreError::Corrupt("revert index"));
    }
    let a = u64::from_le_bytes(b[..8].try_into().expect("8 bytes"));
    let c = u64::from_le_bytes(b[8..].try_into().expect("8 bytes"));
    Ok((a, c))
}

fn enc_carry(c: &Carry) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).u64(c.carried_to).u32(c.regions.len() as u32);
    for (root, s, e, ts, te) in &c.regions {
        w.str(root).u64(*s).u64(*e).u8(enc_taint(*ts, *te));
    }
    w.0
}

fn dec_carry(b: &[u8]) -> Result<Carry, StoreError> {
    let mut r = R::new(b, "revert carry");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("revert carry version"));
    }
    let carried_to = r.u64()?;
    let n = r.u32()? as usize;
    if n > b.len() {
        return Err(StoreError::Corrupt("revert carry"));
    }
    let mut regions = Vec::with_capacity(n);
    for _ in 0..n {
        let (root, s, e) = (r.str()?, r.u64()?, r.u64()?);
        let (ts, te) = dec_taint(r.u8()?)?;
        regions.push((root, s, e, ts, te));
    }
    r.end()?;
    Ok(Carry {
        carried_to,
        regions,
    })
}

pub(super) fn enc_stats(s: &RevertStats) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1)
        .u64(s.count)
        .u64(s.bytes)
        .u64(s.pending_count)
        .u64(s.pending_bytes);
    w.0
}

fn dec_stats(b: &[u8]) -> Result<RevertStats, StoreError> {
    let mut r = R::new(b, "revert stats");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("revert stats version"));
    }
    let s = RevertStats {
        count: r.u64()?,
        bytes: r.u64()?,
        pending_count: r.u64()?,
        pending_bytes: r.u64()?,
    };
    r.end()?;
    Ok(s)
}

/// The header of a stored version (everything before its content).
fn dec_version_head(id: VersionId, b: &[u8]) -> Result<VersionInfo, StoreError> {
    let mut r = R::new(b, "version");
    let ver = r.u8()?;
    if ver != 1 && ver != 2 {
        return Err(StoreError::Corrupt("version record version"));
    }
    let (seq, at_ms, reason, name, authors) = (
        r.u64()?,
        r.u64()?,
        VersionReason::from_u8(r.u8()?)?,
        r.opt_str()?,
        r.strs()?,
    );
    let redacted = dec_redactions(&mut r, ver, b.len())?;
    Ok(VersionInfo {
        id,
        seq,
        at_ms,
        reason,
        name,
        authors,
        bytes: b.len() as u64,
        redacted,
    })
}

pub(super) fn stats_in(
    t: &impl ReadableTable<u64, &'static [u8]>,
    id: DocId,
) -> Result<RevertStats, StoreError> {
    t.get(id)?
        .map(|v| dec_stats(v.value()))
        .transpose()
        .map(Option::unwrap_or_default)
}

/// Write `changes` (positions `seqs`) and their revert records in `w`, updating the stats.
pub(super) fn put_changes(
    w: &redb::WriteTransaction,
    id: DocId,
    seqs: &[Seq],
    changes: &[NewChange],
) -> Result<(), StoreError> {
    if changes.is_empty() {
        return Ok(());
    }
    let mut ct = w.open_table(CHANGES)?;
    let mut rt = w.open_table(REVERTS)?;
    let mut it = w.open_table(REVERT_INDEX)?;
    let mut st = w.open_table(REVERT_STATS)?;
    let mut kt = w.open_table(CHANGE_CLOCKS)?;
    let mut stats = stats_in(&st, id)?;
    for (&seq, c) in seqs.iter().zip(changes) {
        let shape = c.record.shape();
        ct.insert((id, seq), enc_change(&c.info, &shape).as_slice())?;
        if let Some(k) = c.span.filter(|k| k.end > k.start) {
            kt.insert(
                (id, k.client, u64::from(k.start)),
                enc_pair(u64::from(k.end), seq).as_slice(),
            )?;
        }
        let rec = enc_record(&c.record);
        let n = rec.len() as u64;
        rt.insert((id, seq), rec.as_slice())?;
        it.insert((id, seq), enc_pair(c.info.at_ms, n).as_slice())?;
        stats.count += 1;
        stats.bytes += n;
        if c.info.on_behalf_of.is_some() {
            stats.pending_count += 1;
            stats.pending_bytes += n;
        }
    }
    st.insert(id, enc_stats(&stats).as_slice())?;
    Ok(())
}

/// Erase every history row of doc `id` in `w` (deleting the doc).
pub(super) fn delete_rows(w: &redb::WriteTransaction, id: DocId) -> Result<(), StoreError> {
    for t in [CHANGES, REVERTS, REVERT_INDEX, REVERT_CARRY] {
        w.open_table(t)?
            .retain_in((id, 0)..=(id, u64::MAX), |_, _| false)?;
    }
    w.open_table(REVERT_STATS)?.remove(id)?;
    w.open_table(CHANGE_CLOCKS)?
        .retain_in((id, 0, 0)..=(id, u64::MAX, u64::MAX), |_, _| false)?;
    Ok(())
}

/// An on-behalf change was settled (approved or declined): it no longer counts as pending.
pub(super) fn settle_pending(
    w: &redb::WriteTransaction,
    id: DocId,
    seq: Seq,
) -> Result<(), StoreError> {
    let Some((_, n)) = w
        .open_table(REVERT_INDEX)?
        .get((id, seq))?
        .map(|v| dec_pair(v.value()))
        .transpose()?
    else {
        return Ok(()); // no record (older than this format): never counted
    };
    let mut st = w.open_table(REVERT_STATS)?;
    let mut stats = stats_in(&st, id)?;
    stats.pending_count = stats.pending_count.saturating_sub(1);
    stats.pending_bytes = stats.pending_bytes.saturating_sub(n);
    st.insert(id, enc_stats(&stats).as_slice())?;
    Ok(())
}

/// Which unnamed versions retention removes (K-HIST-7): every version younger than
/// `keep_all_ms`; then the newest per hour up to `hourly_until_ms`; then the newest per day;
/// then, while over `max_count` or `max_bytes`, the oldest unnamed. Named versions are
/// never removed.
pub(crate) fn thin_plan(heads: &[VersionInfo], now: u64, r: &Retention) -> Vec<VersionId> {
    const HOUR: u64 = 60 * 60_000;
    const DAY: u64 = 24 * HOUR;
    let mut drop: HashSet<VersionId> = HashSet::new();
    let mut seen: HashSet<(u8, u64)> = HashSet::new();
    // newest first, so the newest in each bucket is the one kept
    for v in heads.iter().rev() {
        if v.name.is_some() {
            continue;
        }
        let age = now.saturating_sub(v.at_ms);
        let bucket = if age <= r.keep_all_ms {
            continue;
        } else if age <= r.hourly_until_ms {
            (0, v.at_ms / HOUR)
        } else {
            (1, v.at_ms / DAY)
        };
        if !seen.insert(bucket) {
            drop.insert(v.id);
        }
    }
    let mut count = heads.len() - drop.len();
    let mut bytes: u64 = heads
        .iter()
        .filter(|v| !drop.contains(&v.id))
        .map(|v| v.bytes)
        .sum();
    for v in heads {
        if count <= r.max_count && bytes <= r.max_bytes {
            break;
        }
        if v.name.is_some() || drop.contains(&v.id) {
            continue;
        }
        drop.insert(v.id);
        count -= 1;
        bytes -= v.bytes;
    }
    let mut out: Vec<VersionId> = drop.into_iter().collect();
    out.sort_unstable();
    out
}

/// Which revert records leave the window (K-HIST-23): records outside it (older than
/// `keep_ms` **and** not among the last `keep_changes` changes) that aren't pending, oldest
/// first; then, while over `max_bytes`, the oldest non-pending ones. `index` is
/// `(seq, at_ms, bytes)` oldest first; `head` the doc's last log position.
pub(crate) fn window_plan(
    index: &[(Seq, u64, u64)],
    pending: &HashSet<Seq>,
    head: Seq,
    now: u64,
    w: &RevertWindow,
) -> Vec<Seq> {
    let mut total: u64 = index.iter().map(|x| x.2).sum();
    let mut out = Vec::new();
    for &(seq, at, n) in index {
        if pending.contains(&seq) {
            continue;
        }
        let in_window =
            seq > head.saturating_sub(w.keep_changes) || at.saturating_add(w.keep_ms) >= now;
        if in_window && total <= w.max_bytes {
            break; // records are oldest first: everything after is in the window too
        }
        out.push(seq);
        total -= n;
    }
    out
}

impl DocStore {
    /// Append `entries` with their change summaries and revert records (`changes`, one per
    /// entry), and store `versions`, in **one** durable transaction. Returns the positions.
    pub(crate) fn commit_changes(
        &self,
        id: DocId,
        entries: &[super::LogEntry],
        changes: &[NewChange],
        versions: &[Version],
    ) -> Result<Vec<Seq>, StoreError> {
        debug_assert_eq!(entries.len(), changes.len());
        let w = self.begin_write()?;
        let seqs = self.append_in(&w, id, entries)?;
        put_changes(&w, id, &seqs, changes)?;
        super::put_versions(&w, id, versions)?;
        w.commit()?;
        Ok(seqs)
    }

    /// The summary of change `seq`, if this store recorded one.
    pub fn change(&self, id: DocId, seq: Seq) -> Result<Option<ChangeInfo>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(CHANGES)?;
        let v = t.get((id, seq))?;
        v.map(|v| dec_change(seq, v.value(), false).map(|x| x.0))
            .transpose()
    }

    /// Blame's lookups, in one read: for each `(client, from, to)`, the clock spans of
    /// `client`'s changes that meet clocks `from..to`, in clock order, as
    /// `(start, end, seq)`. Clocks no recorded change covers (changes made before v0.3.0)
    /// are simply not covered.
    pub(crate) fn clock_spans(
        &self,
        id: DocId,
        runs: &[(u64, u32, u32)],
    ) -> Result<Vec<Vec<SpanOf>>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(CHANGE_CLOCKS)?;
        let mut out = Vec::with_capacity(runs.len());
        for &(client, from, to) in runs {
            let mut spans = Vec::new();
            if to > from {
                // the span starting at or before `from` may reach into it
                let first = t
                    .range((id, client, 0)..=(id, client, u64::from(from)))?
                    .next_back();
                let later =
                    t.range((id, client, u64::from(from) + 1)..(id, client, u64::from(to)))?;
                for kv in first.into_iter().chain(later) {
                    let (k, v) = kv?;
                    let (end, seq) = dec_pair(v.value())?;
                    let start = u32::try_from(k.value().2)
                        .map_err(|_| StoreError::Corrupt("change clocks"))?;
                    let end =
                        u32::try_from(end).map_err(|_| StoreError::Corrupt("change clocks"))?;
                    if end > from {
                        spans.push((start, end, seq));
                    }
                }
            }
            out.push(spans);
        }
        Ok(out)
    }

    /// The summaries of changes `seqs` this store recorded, in one read.
    pub(crate) fn change_infos(
        &self,
        id: DocId,
        seqs: &BTreeSet<Seq>,
    ) -> Result<HashMap<Seq, ChangeInfo>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(CHANGES)?;
        let mut out = HashMap::with_capacity(seqs.len());
        for &seq in seqs {
            if let Some(v) = t.get((id, seq))? {
                out.insert(seq, dec_change(seq, v.value(), false)?.0);
            }
        }
        Ok(out)
    }

    /// Which of changes `seqs` a purge reached: their revert record (while in the window)
    /// has a redacted hunk (blame marks their text `erased`).
    pub(crate) fn purged_changes(
        &self,
        id: DocId,
        seqs: &BTreeSet<Seq>,
    ) -> Result<HashSet<Seq>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(REVERTS)?;
        let mut out = HashSet::new();
        for &seq in seqs {
            if let Some(v) = t.get((id, seq))? {
                let rec = dec_record(v.value())?;
                if rec.roots.iter().flat_map(|x| &x.hunks).any(|h| h.redacted) {
                    out.insert(seq);
                }
            }
        }
        Ok(out)
    }

    /// Change summaries, newest first, while `f` returns true.
    pub fn scan_changes(
        &self,
        id: DocId,
        mut f: impl FnMut(ChangeInfo) -> bool,
    ) -> Result<(), StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(CHANGES)?;
        for kv in t.range((id, 0)..=(id, u64::MAX))?.rev() {
            let (k, v) = kv?;
            if !f(dec_change(k.value().1, v.value(), false)?.0) {
                break;
            }
        }
        Ok(())
    }

    /// The shapes of every change after `after`, in order (carrying revert bounds).
    pub(crate) fn change_shapes(
        &self,
        id: DocId,
        after: Seq,
    ) -> Result<Vec<(Seq, Vec<RootShape>)>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(CHANGES)?;
        let mut out = Vec::new();
        for kv in t.range((id, after.saturating_add(1))..=(id, u64::MAX))? {
            let (k, v) = kv?;
            let seq = k.value().1;
            out.push((seq, dec_change(seq, v.value(), true)?.1));
        }
        Ok(out)
    }

    /// The revert record of change `seq`, while it is in the window.
    pub(crate) fn revert_record(
        &self,
        id: DocId,
        seq: Seq,
    ) -> Result<Option<RevertRecord>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(REVERTS)?;
        let v = t.get((id, seq))?;
        v.map(|v| dec_record(v.value())).transpose()
    }

    /// Every revert record of a doc, oldest first (purge reads the targeted ones' text).
    pub(crate) fn revert_records(&self, id: DocId) -> Result<Vec<(Seq, RevertRecord)>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(REVERTS)?;
        let mut out = Vec::new();
        for kv in t.range((id, 0)..=(id, u64::MAX))? {
            let (k, v) = kv?;
            out.push((k.value().1, dec_record(v.value())?));
        }
        Ok(out)
    }

    /// The window's index: `(seq, at_ms, bytes)` per record, oldest first.
    pub(crate) fn revert_index(&self, id: DocId) -> Result<Vec<(Seq, u64, u64)>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(REVERT_INDEX)?;
        let mut out = Vec::new();
        for kv in t.range((id, 0)..=(id, u64::MAX))? {
            let (k, v) = kv?;
            let (at, n) = dec_pair(v.value())?;
            out.push((k.value().1, at, n));
        }
        Ok(out)
    }

    /// Carried bounds persisted for records older than the window's cut.
    pub(crate) fn revert_carries(&self, id: DocId) -> Result<BTreeMap<Seq, Carry>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(REVERT_CARRY)?;
        let mut out = BTreeMap::new();
        for kv in t.range((id, 0)..=(id, u64::MAX))? {
            let (k, v) = kv?;
            out.insert(k.value().1, dec_carry(v.value())?);
        }
        Ok(out)
    }

    pub(crate) fn revert_stats(&self, id: DocId) -> Result<RevertStats, StoreError> {
        let r = self.begin_read()?;
        stats_in(&r.open_table(REVERT_STATS)?, id)
    }

    /// Drop the records `drop` (outside the window) and persist `carries` (the bounds of the
    /// older records that stay, as of now), in one transaction. Returns the records and bytes
    /// dropped.
    pub(crate) fn prune_reverts(
        &self,
        id: DocId,
        drop: &[Seq],
        carries: &[(Seq, Carry)],
    ) -> Result<(u64, u64), StoreError> {
        let w = self.begin_write()?;
        let (mut n, mut bytes) = (0, 0);
        {
            let mut rt = w.open_table(REVERTS)?;
            let mut it = w.open_table(REVERT_INDEX)?;
            let mut ct = w.open_table(REVERT_CARRY)?;
            for &seq in drop {
                if let Some(v) = it.remove((id, seq))? {
                    bytes += dec_pair(v.value())?.1;
                    n += 1;
                }
                rt.remove((id, seq))?;
                ct.remove((id, seq))?;
            }
            for (seq, c) in carries {
                ct.insert((id, *seq), enc_carry(c).as_slice())?;
            }
            let mut st = w.open_table(REVERT_STATS)?;
            let mut stats = stats_in(&st, id)?;
            stats.count = stats.count.saturating_sub(n);
            stats.bytes = stats.bytes.saturating_sub(bytes);
            st.insert(id, enc_stats(&stats).as_slice())?;
        }
        w.commit()?;
        Ok((n, bytes))
    }

    /// Every version's header and size, oldest first (no content is decoded).
    pub fn version_infos(&self, id: DocId) -> Result<Vec<VersionInfo>, StoreError> {
        let r = self.begin_read()?;
        let mut out = Vec::new();
        for kv in r
            .open_table(super::VERSIONS)?
            .range((id, 0)..=(id, u64::MAX))?
        {
            let (k, v) = kv?;
            out.push(dec_version_head(k.value().1, v.value())?);
        }
        Ok(out)
    }

    /// Apply version retention (K-HIST-7) to a doc in one transaction; returns the ids
    /// removed. Named versions are never removed.
    pub(crate) fn thin_versions(
        &self,
        id: DocId,
        now: u64,
        r: &Retention,
    ) -> Result<Vec<VersionId>, StoreError> {
        let heads = self.version_infos(id)?;
        let plan = thin_plan(&heads, now, r);
        if plan.is_empty() {
            return Ok(plan);
        }
        let w = self.begin_write()?;
        {
            let mut t = w.open_table(super::VERSIONS)?;
            for &vid in &plan {
                // re-checked inside the transaction: a version named since is kept
                let named = match t.get((id, vid))? {
                    Some(v) => dec_version(v.value())?.name.is_some(),
                    None => continue,
                };
                if !named {
                    t.remove((id, vid))?;
                }
            }
        }
        w.commit()?;
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A carry written by v0.8.1 or earlier holds one taint byte per region, `0` or `1`;
    /// `1` reads as both edges tainted (their own conservative reading). New writes keep
    /// each edge apart, in the same record version; any other byte is corrupt.
    #[test]
    fn carry_taint_per_edge_reads_old_single_flags() {
        let old = |t: u8| {
            let mut w = W::default();
            w.u8(1).u64(7).u32(1).str("source").u64(3).u64(5).u8(t);
            w.0
        };
        let regions = |b: &[u8]| dec_carry(b).map(|c| c.regions);
        assert_eq!(
            regions(&old(0)).unwrap(),
            vec![("source".into(), 3, 5, false, false)]
        );
        assert_eq!(
            regions(&old(1)).unwrap(),
            vec![("source".into(), 3, 5, true, true)]
        );
        for (ts, te) in [(false, false), (true, false), (false, true), (true, true)] {
            let c = Carry {
                carried_to: 7,
                regions: vec![("source".into(), 3, 5, ts, te)],
            };
            assert_eq!(dec_carry(&enc_carry(&c)).unwrap(), c);
        }
        for bad in [3u8, 8, 0xff] {
            assert!(matches!(
                regions(&old(bad)),
                Err(StoreError::Corrupt("revert carry taint"))
            ));
        }
    }

    fn head(id: VersionId, at_ms: u64, named: bool) -> VersionInfo {
        VersionInfo {
            id,
            seq: id,
            at_ms,
            reason: VersionReason::Idle,
            name: named.then(|| format!("v{id}")),
            authors: vec![],
            bytes: 10,
            redacted: vec![],
        }
    }

    const H: u64 = 60 * 60_000;
    const D: u64 = 24 * H;

    fn retention() -> Retention {
        Retention {
            keep_all_ms: 30 * D,
            hourly_until_ms: 90 * D,
            max_count: 500,
            max_bytes: 64 << 20,
        }
    }

    #[test]
    fn thinning_keeps_recent_hourly_daily_and_named() {
        let now = 200 * D;
        let mut heads = Vec::new();
        let mut id = 0;
        // every 10 minutes over 200 days, with a named one every 50 days
        let mut at = 0;
        while at < now {
            id += 1;
            heads.push(head(id, at, at % (50 * D) == 0));
            at += 10 * 60_000;
        }
        let r = Retention {
            max_count: usize::MAX,
            ..retention()
        };
        let drop: HashSet<VersionId> = thin_plan(&heads, now, &r).into_iter().collect();
        let kept: Vec<&VersionInfo> = heads.iter().filter(|v| !drop.contains(&v.id)).collect();
        // named versions all stay
        assert!(heads
            .iter()
            .filter(|v| v.name.is_some())
            .all(|v| !drop.contains(&v.id)));
        // the last 30 days are all kept
        let recent = heads.iter().filter(|v| now - v.at_ms <= 30 * D).count();
        assert_eq!(
            kept.iter().filter(|v| now - v.at_ms <= 30 * D).count(),
            recent
        );
        // 30–90 days: one per hour; older: one per day (plus named)
        let mid = kept
            .iter()
            .filter(|v| (30 * D + 1..=90 * D).contains(&(now - v.at_ms)) && v.name.is_none())
            .count() as u64;
        assert!((60 * 24 - 1..=60 * 24 + 1).contains(&mid), "{mid}");
        let old = kept
            .iter()
            .filter(|v| now - v.at_ms > 90 * D && v.name.is_none())
            .count() as u64;
        assert!((109..=111).contains(&old), "{old}");
    }

    #[test]
    fn thinning_caps_count_oldest_unnamed_first() {
        let heads: Vec<VersionInfo> = (1..=10).map(|i| head(i, 1000 + i, i == 1)).collect();
        let r = Retention {
            max_count: 4,
            ..retention()
        };
        assert_eq!(thin_plan(&heads, 2000, &r), vec![2, 3, 4, 5, 6, 7]);
    }

    /// The records land with the change, carry the text purge must reach, and leave the
    /// window with their stats; pending ones are counted until settled.
    #[test]
    fn records_stats_prune_and_purge_reach() {
        use crate::store::{ApprovalState, DocKey, DocMeta, LogEntry};
        let dir = std::env::temp_dir().join(format!("k2mb-hist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = DocStore::open(dir.join("d.redb")).unwrap();
        let id = store
            .create(&DocMeta {
                key: DocKey {
                    space: "s".into(),
                    path: "p".into(),
                },
                pack: "t".into(),
                engine: "e".into(),
                engine_format: 1,
                codec: vec![],
                created_ms: 0,
                synced_hash: None,
                synced_seq: 0,
                declined: None,
            })
            .unwrap();
        let change = |obo: Option<&str>, secret: &str| {
            let entry = LogEntry {
                author: "a".into(),
                client: 7,
                at_ms: 5,
                via: "cli".into(),
                update: vec![1],
                on_behalf_of: obo.map(Into::into),
            };
            let mut info = ChangeInfo::new(ChangeKind::Edit, "a", "cli", 5);
            info.on_behalf_of = obo.map(Into::into);
            let record = RevertRecord {
                author: "a".into(),
                on_behalf_of: obo.map(Into::into),
                at_ms: 5,
                roots: vec![RecordRoot {
                    root: "source".into(),
                    empty_after: false,
                    hunks: vec![RecordHunk {
                        start: 0,
                        end: 3,
                        insert: secret.into(),
                        deleted: "old".into(),
                        redacted: false,
                    }],
                }],
            };
            let start = if obo.is_some() { 8 } else { 0 };
            let span = Some(ClockSpan {
                client: 7,
                start,
                end: start + 8,
            });
            (entry, NewChange { info, record, span })
        };
        let (e1, c1) = change(None, "SECRET-1");
        let (e2, c2) = change(Some("p"), "SECRET-2");
        let seqs = store.commit_changes(id, &[e1, e2], &[c1, c2], &[]).unwrap();
        assert_eq!(seqs, vec![1, 2]);
        // blame's index: clocks to changes, across span edges, other clients uncovered
        assert_eq!(
            store
                .clock_spans(
                    id,
                    &[(7, 0, 3), (7, 6, 10), (7, 16, 20), (8, 0, 9), (7, 5, 5)]
                )
                .unwrap(),
            vec![
                vec![(0, 8, 1)],
                vec![(0, 8, 1), (8, 16, 2)],
                vec![],
                vec![],
                vec![],
            ]
        );
        let both: BTreeSet<Seq> = [1, 2, 9].into();
        let infos = store.change_infos(id, &both).unwrap();
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[&2].on_behalf_of.as_deref(), Some("p"));
        assert!(store.purged_changes(id, &both).unwrap().is_empty());
        let st = store.revert_stats(id).unwrap();
        assert_eq!((st.count, st.pending_count), (2, 1));
        assert!(st.pending_bytes > 0 && st.bytes > st.pending_bytes);
        // the purge hook sees every record's text
        let all = store.revert_records(id).unwrap();
        assert!(all
            .iter()
            .any(|(_, r)| r.roots[0].hunks[0].insert == "SECRET-2"));
        assert_eq!(
            store
                .change(id, 2)
                .unwrap()
                .unwrap()
                .on_behalf_of
                .as_deref(),
            Some("p")
        );
        assert_eq!(
            store.change_shapes(id, 1).unwrap()[0].1[0].hunks,
            vec![(0, 3, 8)]
        );
        // settling the approval stops counting it as pending
        let a = store.approval(id, 2).unwrap().unwrap();
        store
            .set_approval(id, 2, &a.settled(ApprovalState::Approved, "p", 9))
            .unwrap();
        assert_eq!(store.revert_stats(id).unwrap().pending_count, 0);
        // pruning drops the record (not its summary) and persists carries
        let carry = Carry {
            carried_to: 2,
            regions: vec![
                ("source".into(), 0, 8, false, false),
                ("source".into(), 9, 9, true, false),
                ("source".into(), 10, 12, false, true),
            ],
        };
        assert_eq!(
            store
                .prune_reverts(id, &[1], &[(2, carry.clone())])
                .unwrap()
                .0,
            1
        );
        assert!(store.revert_record(id, 1).unwrap().is_none());
        assert!(store.change(id, 1).unwrap().is_some());
        assert_eq!(store.revert_carries(id).unwrap().get(&2), Some(&carry));
        assert_eq!(store.revert_stats(id).unwrap().count, 1);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn window_keeps_pending_recent_and_last_changes() {
        let index: Vec<(Seq, u64, u64)> = (1..=20).map(|s| (s, s * 1000, 100)).collect();
        let pending: HashSet<Seq> = [2, 5].into_iter().collect();
        let w = RevertWindow {
            keep_ms: 5_000,
            keep_changes: 10,
            max_bytes: 1 << 20,
        };
        // head 20, now 20_000: the last 10 changes (11..=20) reach further than 5 s
        assert_eq!(
            window_plan(&index, &pending, 20, 20_000, &w),
            vec![1, 3, 4, 6, 7, 8, 9, 10]
        );
        // a byte cap drops the oldest non-pending even inside the window
        let w2 = RevertWindow {
            max_bytes: 1_000,
            ..w
        };
        let d = window_plan(&index, &pending, 20, 20_000, &w2);
        assert_eq!(d.len(), 20 - 2 - 8);
        assert!(!d.contains(&2) && !d.contains(&5));
    }
}
