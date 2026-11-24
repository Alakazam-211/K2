//! Versions after a lost group (grouped commits, `group.rs`): a version a lost change had is
//! never issued again, and a writer that may have held a lost change never writes again.
//!
//! A doc's version ([`super::Seq`], its log position) is what an agent passes as a base, and
//! what `events_since` and the live window start from. Its next one is the newest log row's
//! plus one, so after an abrupt stop that lost a group, the versions the lost changes had would
//! be issued again for other changes, and "equal version, equal content" would break.
//!
//! - **The floor.** A grouped store keeps, per doc, a **seq floor** in its `meta` rows
//!   (`seq_floor/{doc}`): no version at or past it was ever issued. Before a commit issues a
//!   version at or past the floor, it raises the floor to that version plus `max_changes` plus
//!   1, and that commit waits for the disk ([`super::DocStore`]'s `append_in`): so the floor on
//!   disk is always past every version issued, and it costs at most one durable commit per doc
//!   per `max_changes` changes, folded into the group's.
//! - **A recovery** is any open of a store whose last grouped opener didn't close it cleanly
//!   (the mark [`GROUPED_OPEN`] still set: a stop, a power loss; nothing on disk says whether a
//!   group was waiting, so every such open counts), and the in-process rollback after a commit
//!   failed at the disk. In one transaction it
//!   - records, per doc whose floor is past its newest version plus one, the versions between
//!     as **lost** (`seq_lost/{doc}/{from}` = to): a base or an `events_since` naming one is
//!     refused `base_expired`, and the doc's next version is past them;
//!   - **retires every reusable writer** (an agent's, a person's on a device): its kind row
//!     says "never reused", so every writer after the recovery is fresh, and no (writer,
//!     clock) pair a lost change held is made again;
//!   - and, when it recorded a lost range, stamps the store's features word with
//!     [`super::FEATURE_SEQ_FLOOR`], so a kit that doesn't keep the floor refuses the store at
//!     open (`store_newer`) instead of issuing a lost version again. A store that never lost
//!     a version stays readable by those kits; one closed cleanly keeps its floor rows, which
//!     such a kit ignores.
//!
//! - **Tier-0 versions** ([`super::VersionId`], `blob.rs`) are a second number space, kept the
//!   same way under their own keys (`blob_floor/{doc}`, `blob_lost/{doc}/{from}`): a blob
//!   doc's next version is its last one's plus one, past its lost ones, and a commit issuing
//!   one at or past the blob floor raises it and waits for the disk, as a text commit does. A
//!   recovery walks both spaces; the newest tier-0 version is the doc's last one (or its newest
//!   lost one). A demoted doc has rows in both.
//!
//! Keys are zero-padded, so their string order is their number order. At
//! [`super::CommitDurability::Immediate`] (the default) none of this is written: such a store
//! never gets a floor, a mark or a lost range, unless a grouped opener, or the store while it
//! grouped ([`super::DocStore::set_commit_durability`]), left them.

use redb::{ReadableTable, TableHandle, WriteTransaction};

use super::{
    dec_person_row, dec_snapshot_ref, DocId, Seq, StoreError, CLIENT_KINDS, DOCS, KIND_AGENT,
    KIND_OWN_REPLICA, META, SNAPSHOTS, UPDATES,
};

/// `meta`: 1 while a grouped opener holds the store; its clean close removes it.
pub(super) const GROUPED_OPEN: &str = "grouped_open";

/// A doc's two number spaces: its text versions ([`Seq`], the log's positions) and its tier-0
/// versions ([`super::VersionId`], `blob.rs`). Each keeps its own floor and lost ranges, under
/// its own keys: a demoted doc has both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Space {
    /// `seq_floor/{doc}`, `seq_lost/{doc}/{from}`.
    Text,
    /// `blob_floor/{doc}`, `blob_lost/{doc}/{from}`.
    Blob,
}

impl Space {
    /// The prefix of every floor key of the space.
    fn floors(self) -> &'static str {
        match self {
            Space::Text => "seq_floor/",
            Space::Blob => "blob_floor/",
        }
    }

    /// The prefix of every lost-range key of the space.
    fn losts(self) -> &'static str {
        match self {
            Space::Text => "seq_lost/",
            Space::Blob => "blob_lost/",
        }
    }
}

/// The end of the key range of every key starting with `prefix` (which ends in `/`): `0` sorts
/// right after `/`.
fn range_end(prefix: &str) -> String {
    format!("{}0", &prefix[..prefix.len() - 1])
}

fn floor_key(s: Space, id: DocId) -> String {
    format!("{}{id:020}", s.floors())
}

fn lost_prefix(s: Space, id: DocId) -> String {
    format!("{}{id:020}/", s.losts())
}

fn lost_key(s: Space, id: DocId, from: Seq) -> String {
    format!("{}{id:020}/{from:020}", s.losts())
}

/// The doc's floor in space `s` in `w` (0: none).
pub(super) fn floor_in(w: &WriteTransaction, s: Space, id: DocId) -> Result<Seq, StoreError> {
    Ok(w.open_table(META)?
        .get(floor_key(s, id).as_str())?
        .map_or(0, |v| v.value()))
}

/// Set the doc's floor in space `s` in `w`.
pub(super) fn set_floor_in(
    w: &WriteTransaction,
    s: Space,
    id: DocId,
    floor: Seq,
) -> Result<(), StoreError> {
    w.open_table(META)?
        .insert(floor_key(s, id).as_str(), floor)?;
    Ok(())
}

/// The last version of the doc's newest lost range in space `s` (0: none), from any `meta`
/// table.
pub(super) fn lost_through(
    meta: &impl ReadableTable<&'static str, u64>,
    s: Space,
    id: DocId,
) -> Result<Seq, StoreError> {
    let p = lost_prefix(s, id);
    let end = format!("{p}~");
    Ok(meta
        .range(p.as_str()..end.as_str())?
        .next_back()
        .transpose()?
        .map_or(0, |(_, to)| to.value()))
}

/// The lost range of space `s` holding version `seq` of the doc (its first and last), if any.
pub(super) fn lost_range_at(
    meta: &impl ReadableTable<&'static str, u64>,
    s: Space,
    id: DocId,
    seq: Seq,
) -> Result<Option<(Seq, Seq)>, StoreError> {
    let p = lost_prefix(s, id);
    let at = lost_key(s, id, seq);
    let Some((k, to)) = meta
        .range(p.as_str()..=at.as_str())?
        .next_back()
        .transpose()?
    else {
        return Ok(None);
    };
    let to = to.value();
    if seq > to {
        return Ok(None);
    }
    let from = k.value()[p.len()..]
        .parse()
        .map_err(|_| StoreError::Corrupt("a lost range's key"))?;
    Ok(Some((from, to)))
}

/// Whether version `seq` of the doc is in one of its lost ranges in space `s`.
pub(super) fn is_lost(
    meta: &impl ReadableTable<&'static str, u64>,
    s: Space,
    id: DocId,
    seq: Seq,
) -> Result<bool, StoreError> {
    Ok(lost_range_at(meta, s, id, seq)?.is_some())
}

/// The doc's floors and lost ranges, in both spaces, erased in `w` (its delete).
pub(super) fn erase_in(w: &WriteTransaction, id: DocId) -> Result<(), StoreError> {
    erase_space_in(w, Space::Text, id)?;
    erase_space_in(w, Space::Blob, id)
}

fn erase_space_in(w: &WriteTransaction, s: Space, id: DocId) -> Result<(), StoreError> {
    let mut meta = w.open_table(META)?;
    meta.remove(floor_key(s, id).as_str())?;
    let p = lost_prefix(s, id);
    let end = format!("{p}~");
    let keys: Vec<String> = meta
        .range(p.as_str()..end.as_str())?
        .map(|kv| kv.map(|(k, _)| k.value().to_owned()))
        .collect::<Result<_, _>>()?;
    for k in keys {
        meta.remove(k.as_str())?;
    }
    Ok(())
}

/// Every floor of space `s` in `w`: (doc, floor).
fn floors_in(w: &WriteTransaction, s: Space) -> Result<Vec<(DocId, Seq)>, StoreError> {
    let meta = w.open_table(META)?;
    let p = s.floors();
    let end = range_end(p);
    let mut v = Vec::new();
    for kv in meta.range(p..end.as_str())? {
        let (k, f) = kv?;
        let id = k.value()[p.len()..]
            .parse()
            .map_err(|_| StoreError::Corrupt("a version floor's key"))?;
        v.push((id, f.value()));
    }
    Ok(v)
}

/// The newest version the store holds of the doc in space `s`, or lost in it (0: none).
fn newest_in(w: &WriteTransaction, s: Space, id: DocId) -> Result<Seq, StoreError> {
    let mut newest = match s {
        Space::Text => {
            let mut n = w
                .open_table(UPDATES)?
                .range((id, 0)..=(id, u64::MAX))?
                .next_back()
                .transpose()?
                .map_or(0, |(k, _)| k.value().1);
            if let Some(v) = w.open_table(SNAPSHOTS)?.get(id)? {
                n = n.max(dec_snapshot_ref(v.value())?.0);
            }
            n
        }
        Space::Blob => super::blob::last_vid(w, id)?,
    };
    newest = newest.max(lost_through(&w.open_table(META)?, s, id)?);
    Ok(newest)
}

/// Retire, in `w`, every writer of every doc that a later join could reuse, so every writer
/// after this is fresh and no `(writer, clock)` the store's line may already have given out is
/// made again: each agent's ([`KIND_AGENT`]) gets the kind "never reused" (an own replica's);
/// each person's on a device (a person-writer row, the host's device or the store's) is marked
/// retired, the rest of its row kept, so a late reclaim of its last lease is still told when
/// that lease ended. Owner rows stay, so authorship still names the same participant. The one
/// place this is done: a recovery ([`recover_in`]) and an open whose writer incarnation rotated
/// (the store was copied, moved or restored, or not closed cleanly). Returns how many it retired
/// (a row retired before isn't counted again).
pub(super) fn retire_reusable_writers(w: &WriteTransaction) -> Result<u32, StoreError> {
    if !w.list_tables()?.any(|t| t.name() == CLIENT_KINDS.name()) {
        return Ok(0);
    }
    let mut retire: Vec<((u64, u64), Vec<u8>)> = Vec::new();
    for kv in w.open_table(CLIENT_KINDS)?.iter()? {
        let (k, row) = kv?;
        let row = row.value();
        if row == [KIND_AGENT].as_slice() {
            retire.push((k.value(), vec![KIND_OWN_REPLICA]));
        } else if let Some(mut p) = dec_person_row(row).filter(|p| !p.retired) {
            p.retired = true;
            retire.push((k.value(), super::enc_person_row(&p).to_vec()));
        }
    }
    let mut t = w.open_table(CLIENT_KINDS)?;
    let mut n = 0;
    for (k, row) in retire {
        t.insert(k, row.as_slice())?;
        n += 1;
    }
    Ok(n)
}

/// What a recovery did ([`recover_in`]).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Recovered {
    /// Lost ranges recorded.
    pub lost_ranges: u32,
    /// Writers retired.
    pub retired: u32,
}

/// The recovery, in `w` (module docs): lost ranges from the floors, every reusable writer
/// retired, and the feature stamped when a range was recorded.
pub(super) fn recover_in(w: &WriteTransaction) -> Result<Recovered, StoreError> {
    let mut out = Recovered::default();
    for sp in [Space::Text, Space::Blob] {
        for (id, floor) in floors_in(w, sp)? {
            if w.open_table(DOCS)?.get(id)?.is_none() {
                erase_in(w, id)?;
                continue;
            }
            let newest = newest_in(w, sp, id)?;
            if floor > newest + 1 {
                w.open_table(META)?
                    .insert(lost_key(sp, id, newest + 1).as_str(), floor - 1)?;
                out.lost_ranges += 1;
            }
        }
    }
    // every writer a later join could reuse is retired: never reused again (the plant: a
    // recovery leaves the reusable writers reusable)
    if !crate::beta_plant::planted("no-rotation-after-recovery") {
        out.retired = retire_reusable_writers(w)?;
    }
    if out.lost_ranges > 0 {
        super::features::stamp_feature(w, super::FEATURE_SEQ_FLOOR)?;
    }
    Ok(out)
}

/// What an open found ([`at_open`]).
pub(super) struct AtOpen {
    /// The recovery it ran, if the last grouped opener didn't close the store cleanly.
    pub done: Option<Recovered>,
    /// The store holds lost versions (its features word says so).
    pub lost_known: bool,
}

/// An open's part, in its write transaction `w`: a recovery when the mark is set (the last
/// grouped opener didn't close cleanly), then the mark set for a grouped open (removed for
/// any other). An open at [`super::CommitDurability::Immediate`] of a store no grouped opener
/// left a mark in writes nothing.
pub(super) fn at_open(
    w: &WriteTransaction,
    settings: &super::StoreSettings,
) -> Result<AtOpen, StoreError> {
    let mark = w.open_table(META)?.get(GROUPED_OPEN)?.map(|v| v.value());
    let done = match mark {
        Some(v) if v != 0 => Some(recover_in(w)?),
        _ => None,
    };
    let grouped = matches!(
        settings.commit_durability(),
        super::CommitDurability::Grouped { .. }
    );
    {
        let mut meta = w.open_table(META)?;
        if grouped {
            meta.insert(GROUPED_OPEN, 1)?;
        } else if mark.is_some() {
            meta.remove(GROUPED_OPEN)?;
        }
    }
    let word = w
        .open_table(META)?
        .get(super::features::REQUIRED_FEATURES)?
        .map_or(0, |v| v.value());
    Ok(AtOpen {
        done,
        lost_known: word & super::FEATURE_SEQ_FLOOR != 0,
    })
}

/// A grouped store's clean close, in `w`: the mark removed.
pub(super) fn close(w: &WriteTransaction) -> Result<(), StoreError> {
    w.open_table(META)?.remove(GROUPED_OPEN)?;
    Ok(())
}

/// A change of the store's durability from `was` to `now`, in its own commit `w`, which waits
/// for the disk ([`super::DocStore::set_commit_durability`]): the mark set when the store
/// starts grouping, so an abrupt stop after it is a recovery, as after a grouped open; removed
/// when it stops, as a clean close removes it (every commit before is durable with `w`). A
/// change of a group's bounds alone writes nothing: the floors already on disk are past every
/// version given out, and the next one past a floor raises it under the new bound.
pub(super) fn switched(
    w: &WriteTransaction,
    was: super::CommitDurability,
    now: super::CommitDurability,
) -> Result<(), StoreError> {
    let grouped = |c| matches!(c, super::CommitDurability::Grouped { .. });
    match (grouped(was), grouped(now)) {
        (false, true) => {
            w.open_table(META)?.insert(GROUPED_OPEN, 1)?;
        }
        (true, false) => close(w)?,
        _ => {}
    }
    Ok(())
}
