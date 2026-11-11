//! The annotations in the store (`docs/SESSIONS.md`, "Annotations"): five per-doc tables,
//! added without a format bump and **created lazily** by a doc's first comment, so a store
//! no comment reached keeps none (and a host that hasn't adopted comments can still purge and
//! rewrite after rolling this kit back). Every reader takes a missing table as empty.
//!
//! - [`ANNOT_THREADS`]: `(doc, thread)` → the thread row ([`ThreadRec`]).
//! - [`ANNOT_COMMENTS`]: `(doc, thread, n)` → one comment: the **only** copy of its body.
//! - [`ANNOT_LOG`]: `(doc, seq)` → one history row per op. Never a body.
//! - [`ANNOT_THREAD_LOG`]: `(doc, thread, seq)` → nothing: the history rows of one thread,
//!   so a thread's history is read without scanning the doc's.
//! - [`ANNOT_CHANGED`]: `(doc, change)` → thread: each thread's last change, one row a thread
//!   (its previous one goes). `comments --since` reads it, so every change a reader sees
//!   (an erasure's renames, a version purge's clears, an anchor's new state) is in it, history
//!   row or not.
//!
//! Rows are never removed but by a doc delete (they are in `DOC_TABLES`), so a thread id and a
//! history seq are never reused: the next is the last plus one.
//!
//! **A node's content is only ever written from a live version.** Every transaction that
//! writes an anchor's copy of a node (a new comment, a re-anchor, an index record) re-checks
//! that the version it came from still exists and isn't purged; a version purge clears the
//! copies in its own transaction. So a purge racing a comment can't be undone by it.

use std::collections::{BTreeMap, BTreeSet};

use redb::{ReadableTable, TableDefinition, TableHandle, WriteTransaction};

use super::blob::{dec_version, enc_version, BLOB_APPROVALS, BLOB_VERSIONS};
use super::{dec_approval, enc_approval, history, DocId, DocStore, Needles, StoreError, VersionId};
use crate::anchor::{planted, AnchorTarget};
use crate::annot::rec::{
    dec_comment, dec_event, dec_thread, enc_comment, enc_event, enc_thread, ThreadRec,
};
use crate::annot::{AnnotEvent, AnnotOp, AnnotPurge, Attribution, Comment, ThreadId, ThreadStatus};

/// `(doc, thread)` → the thread row.
pub(super) const ANNOT_THREADS: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("annot_threads");
/// `(doc, thread, n)` → one comment.
pub(super) const ANNOT_COMMENTS: TableDefinition<(u64, u64, u64), &[u8]> =
    TableDefinition::new("annot_comments");
/// `(doc, seq)` → one history row.
pub(super) const ANNOT_LOG: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("annot_log");
/// `(doc, thread, seq)` → nothing: one thread's history rows.
pub(super) const ANNOT_THREAD_LOG: TableDefinition<(u64, u64, u64), &[u8]> =
    TableDefinition::new("annot_thread_log");
/// `(doc, change)` → the thread id (8 bytes, little-endian): each thread's last change.
pub(super) const ANNOT_CHANGED: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("annot_changed");

/// A refusal decided inside an annotation transaction (nothing was written).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AnnotFail {
    NoThread(ThreadId),
    NoComment(ThreadId, u32),
    /// Past a cap: `what` and its limit.
    Cap {
        what: &'static str,
        limit: u64,
    },
    /// The caller may not delete comment `n` of the thread (`AuthorOrOwner`).
    NotYours(ThreadId, u32),
    /// The version an anchor's node came from was purged (or is gone) since it was read.
    VersionGone(VersionId),
    /// An erasure the doc refuses (why).
    Erasure(&'static str),
}

/// A store result holding a refusal decided in the transaction.
pub(crate) type Out<T> = Result<Result<T, AnnotFail>, StoreError>;

/// A thread as one transaction left it: what a call reports, read in that transaction, so a
/// committed change never reads back as a failure.
#[derive(Debug, Clone)]
pub(crate) struct Committed {
    pub id: ThreadId,
    /// The history row it added (`None`: nothing changed).
    pub seq: Option<u64>,
    /// The comment it added or changed.
    pub n: Option<u32>,
    pub rec: ThreadRec,
    pub comments: Vec<Comment>,
}

/// What an annotation purge or an erasure changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AnnotCounts {
    /// Bodies cleared.
    pub comments: u64,
    /// Anchors that lost their hint, fingerprint or last-seen name.
    pub anchors: u64,
    /// Rows where an erasure replaced the pid.
    pub attributions: u64,
}

impl std::ops::AddAssign for AnnotCounts {
    fn add_assign(&mut self, o: Self) {
        self.comments += o.comments;
        self.anchors += o.anchors;
        self.attributions += o.attributions;
    }
}

/// An index record's update of one thread: applied only if the thread hasn't changed since
/// `expected` (its change seq when it was read).
pub(crate) struct Observed {
    pub id: ThreadId,
    pub expected: u64,
    pub rec: ThreadRec,
}

fn last_key(t: &redb::Table<'_, (u64, u64), &[u8]>, doc: DocId) -> Result<u64, StoreError> {
    Ok(match t.range((doc, 0)..=(doc, u64::MAX))?.next_back() {
        Some(kv) => kv?.0.value().1,
        None => 0,
    })
}

/// Add history row `ev` (for thread `rec`, `id`), indexed by thread. Returns its seq.
fn append(
    w: &WriteTransaction,
    doc: DocId,
    id: u64,
    rec: &mut ThreadRec,
    ev: &AnnotEvent,
) -> Result<u64, StoreError> {
    let mut ev = ev.clone();
    ev.thread = ThreadId(id);
    let seq = {
        let mut t = w.open_table(ANNOT_LOG)?;
        let seq = last_key(&t, doc)? + 1;
        t.insert((doc, seq), enc_event(&ev).as_slice())?;
        seq
    };
    w.open_table(ANNOT_THREAD_LOG)?
        .insert((doc, id, seq), b"".as_slice())?;
    rec.log_rows = rec.log_rows.saturating_add(1);
    Ok(seq)
}

/// Mark thread `id` changed now: a new change seq, its previous one gone.
fn touch(w: &WriteTransaction, doc: DocId, id: u64, rec: &mut ThreadRec) -> Result<(), StoreError> {
    let mut t = w.open_table(ANNOT_CHANGED)?;
    let seq = last_key(&t, doc)? + 1;
    if rec.changed != 0 {
        t.remove((doc, rec.changed))?;
    }
    t.insert((doc, seq), id.to_le_bytes().as_slice())?;
    rec.changed = seq;
    Ok(())
}

/// Touch thread `id` and write its row.
fn put_thread(
    w: &WriteTransaction,
    doc: DocId,
    id: u64,
    rec: &mut ThreadRec,
) -> Result<(), StoreError> {
    touch(w, doc, id, rec)?;
    w.open_table(ANNOT_THREADS)?
        .insert((doc, id), enc_thread(rec).as_slice())?;
    Ok(())
}

fn get_thread(w: &WriteTransaction, doc: DocId, id: u64) -> Result<Option<ThreadRec>, StoreError> {
    if !history::has_table(w, ANNOT_THREADS.name())? {
        return Ok(None);
    }
    let t = w.open_table(ANNOT_THREADS)?;
    let v = t.get((doc, id))?;
    v.map(|v| dec_thread(v.value())).transpose()
}

fn comments_in(w: &WriteTransaction, doc: DocId, t: u64) -> Result<Vec<Comment>, StoreError> {
    if !history::has_table(w, ANNOT_COMMENTS.name())? {
        return Ok(Vec::new());
    }
    let tab = w.open_table(ANNOT_COMMENTS)?;
    let mut out = Vec::new();
    for kv in tab.range((doc, t, 0)..=(doc, t, u64::MAX))? {
        let (k, v) = kv?;
        let n =
            u32::try_from(k.value().2).map_err(|_| StoreError::Corrupt("annotation comment"))?;
        out.push(dec_comment(n, v.value())?);
    }
    Ok(out)
}

/// Whether version `v` of `doc` exists and isn't purged, in `w`.
fn version_live(w: &WriteTransaction, doc: DocId, v: VersionId) -> Result<bool, StoreError> {
    if planted!(AnchorFromPurgedVersion) {
        return Ok(true);
    }
    if !history::has_table(w, BLOB_VERSIONS.name())? {
        return Ok(false);
    }
    let t = w.open_table(BLOB_VERSIONS)?;
    let found = match t.get((doc, v))? {
        Some(g) => !dec_version(g.value())?.redacted,
        None => false,
    };
    Ok(found)
}

/// Whether `rec`'s anchor holds a copy of a node (its fingerprint, last-seen record or hint).
fn holds_node_copy(rec: &ThreadRec) -> bool {
    let a = &rec.anchor;
    a.fingerprint.is_some() || a.last_seen.is_some() || a.hint.is_some()
}

/// A comment with nothing left of its content: no body, mentions or attachments.
fn clear(c: &mut Comment) -> bool {
    let had = c.body.is_some() || !c.mentions.is_empty() || !c.attachments.is_empty();
    c.body = None;
    c.mentions.clear();
    c.attachments.clear();
    had
}

/// Clear an anchor's copies of node content; sticky until a re-anchor.
fn clear_anchor(rec: &mut ThreadRec) -> bool {
    let had = holds_node_copy(rec);
    rec.anchor.hint = None;
    rec.anchor.fingerprint = None;
    rec.anchor.last_seen = None;
    rec.purged = true;
    had
}

/// Replace `pid` by `ps` in `s`.
fn rename(s: &mut String, pid: &str, ps: &str) -> bool {
    if s == pid {
        *s = ps.to_owned();
        true
    } else {
        false
    }
}

fn rename_attr(a: &mut Attribution, pid: &str, ps: &str) -> bool {
    let mine = rename(&mut a.pid, pid, ps);
    if mine {
        a.name = ps.to_owned();
    }
    let obo = a.on_behalf_of.as_mut().is_some_and(|o| rename(o, pid, ps));
    mine | obo
}

/// Every thread row of `doc`, in id order.
fn all_threads(w: &WriteTransaction, doc: DocId) -> Result<BTreeMap<u64, ThreadRec>, StoreError> {
    let mut out = BTreeMap::new();
    if !history::has_table(w, ANNOT_THREADS.name())? {
        return Ok(out);
    }
    let t = w.open_table(ANNOT_THREADS)?;
    for kv in t.range((doc, 0)..=(doc, u64::MAX))? {
        let (k, v) = kv?;
        out.insert(k.value().1, dec_thread(v.value())?);
    }
    Ok(out)
}

/// A version purge (`blob_purge_versions`, in its transaction): every anchor of `doc` made on
/// one of `vids` loses its hint and fingerprint, and one last seen on one its last-seen record
/// (a node's name is that version's content); a cleared anchor stays cleared until a
/// re-anchor. Returns the anchors changed.
pub(super) fn forget_versions(
    w: &WriteTransaction,
    doc: DocId,
    vids: &[VersionId],
) -> Result<u64, StoreError> {
    if vids.is_empty() || !history::has_table(w, ANNOT_THREADS.name())? {
        return Ok(0);
    }
    if planted!(VersionPurgeKeepsHints) {
        return Ok(0);
    }
    let mut n = 0;
    for (id, mut rec) in all_threads(w, doc)? {
        let mut hit = false;
        if vids.contains(&rec.anchor.created_version)
            && (rec.anchor.hint.is_some() || rec.anchor.fingerprint.is_some())
        {
            rec.anchor.hint = None;
            rec.anchor.fingerprint = None;
            hit = true;
        }
        if rec
            .anchor
            .last_seen
            .as_ref()
            .is_some_and(|l| vids.contains(&l.version))
        {
            rec.anchor.last_seen = None;
            hit = true;
        }
        if hit {
            rec.purged = true;
            n += 1;
            put_changed(w, doc, id, &mut rec)?;
        }
    }
    Ok(n)
}

/// Write a thread a kit-side change touched (no history row): touched, so `since` sees it.
fn put_changed(
    w: &WriteTransaction,
    doc: DocId,
    id: u64,
    rec: &mut ThreadRec,
) -> Result<(), StoreError> {
    if planted!(SinceMissesKitChanges) {
        w.open_table(ANNOT_THREADS)?
            .insert((doc, id), enc_thread(rec).as_slice())?;
        return Ok(());
    }
    put_thread(w, doc, id, rec)
}

/// A text doc's purge (`purge_doc`, in its transaction; a doc promoted from tier 0 keeps its
/// threads): bodies and anchor copies any needle matches are cleared, an erasure's renames and
/// clears apply, and a purge of the whole history (`all_history`) clears every anchor's copies
/// of tier-0 node content. No history row is added (the text purge's audit is its own).
pub(super) fn text_doc_purge(
    w: &WriteTransaction,
    doc: DocId,
    needles: &Needles,
    erase: Option<(&str, &str)>,
    all_history: bool,
) -> Result<AnnotCounts, StoreError> {
    let mut c = AnnotCounts::default();
    if planted!(TextPurgeSkipsAnnotations) || !history::has_table(w, ANNOT_THREADS.name())? {
        return Ok(c);
    }
    if let Some((pid, ps)) = erase {
        c += erase_in(w, doc, pid, ps)?;
    }
    for (id, mut rec) in all_threads(w, doc)? {
        let mut hit = false;
        let mut comments = comments_in(w, doc, id)?;
        for cm in &mut comments {
            let take = cm
                .body
                .as_deref()
                .is_some_and(|b| needles.hits(b.as_bytes()))
                || cm
                    .attachments
                    .iter()
                    .any(|a| needles.hits(a.target.as_bytes()));
            if take && clear(cm) {
                c.comments += 1;
                hit = true;
                w.open_table(ANNOT_COMMENTS)?
                    .insert((doc, id, u64::from(cm.n)), enc_comment(cm).as_slice())?;
            }
        }
        if (all_history || anchor_hits(&rec, |s| needles.hits(s.as_bytes())))
            && clear_anchor(&mut rec)
        {
            c.anchors += 1;
            hit = true;
        }
        if hit {
            put_changed(w, doc, id, &mut rec)?;
        }
    }
    Ok(c)
}

/// Whether `hits` matches any of `rec`'s anchor copies of node content.
fn anchor_hits(rec: &ThreadRec, hits: impl Fn(&str) -> bool) -> bool {
    let a = &rec.anchor;
    a.hint.as_deref().is_some_and(&hits)
        || a.fingerprint
            .as_ref()
            .is_some_and(|f| hits(&f.name) || f.weak.values().any(|v| hits(v)))
        || a.last_seen.as_ref().is_some_and(|l| hits(&l.name))
}

/// Participant erasure on `doc`'s annotations, in `w`: every attribution and mention of `pid`
/// becomes `ps`, and every body the erased participant wrote, or an agent wrote on their
/// behalf, is cleared.
fn erase_in(
    w: &WriteTransaction,
    doc: DocId,
    pid: &str,
    ps: &str,
) -> Result<AnnotCounts, StoreError> {
    let mut c = AnnotCounts::default();
    let mut threads = all_threads(w, doc)?;
    let mut changed: BTreeSet<u64> = BTreeSet::new();
    for (id, rec) in &mut threads {
        let mut hit = rename_attr(&mut rec.created, pid, ps);
        if let ThreadStatus::Resolved { by } = &mut rec.status {
            hit |= rename_attr(by, pid, ps);
        }
        if let Some(p) = &mut rec.pending {
            hit |= rename_attr(&mut p.by, pid, ps);
        }
        if hit {
            c.attributions += 1;
            changed.insert(*id);
        }
    }
    if history::has_table(w, ANNOT_COMMENTS.name())? {
        let mut rows: Vec<((u64, u64), Vec<u8>)> = Vec::new();
        {
            let t = w.open_table(ANNOT_COMMENTS)?;
            for kv in t.range((doc, 0, 0)..=(doc, u64::MAX, u64::MAX))? {
                let (k, v) = kv?;
                let (_, id, n) = k.value();
                let n32 =
                    u32::try_from(n).map_err(|_| StoreError::Corrupt("annotation comment"))?;
                let mut cm = dec_comment(n32, v.value())?;
                let behalf =
                    cm.by.on_behalf_of.as_deref() == Some(pid) && !planted!(ErasureSkipsBehalf);
                let theirs = cm.by.pid == pid || behalf;
                let mut hit = rename_attr(&mut cm.by, pid, ps);
                if let Some(d) = &mut cm.deleted {
                    hit |= rename_attr(d, pid, ps);
                }
                for m in &mut cm.mentions {
                    hit |= rename(m, pid, ps);
                }
                if theirs && !planted!(ErasureKeepsBodies) && clear(&mut cm) {
                    c.comments += 1;
                    hit = true;
                }
                if hit {
                    rows.push(((id, n), enc_comment(&cm)));
                }
            }
        }
        c.attributions += rows.len() as u64;
        let mut t = w.open_table(ANNOT_COMMENTS)?;
        for ((id, n), b) in rows {
            t.insert((doc, id, n), b.as_slice())?;
            changed.insert(id);
        }
    }
    if history::has_table(w, ANNOT_LOG.name())? {
        let mut rows: Vec<(u64, Vec<u8>)> = Vec::new();
        {
            let t = w.open_table(ANNOT_LOG)?;
            for kv in t.range((doc, 0)..=(doc, u64::MAX))? {
                let (k, v) = kv?;
                let seq = k.value().1;
                let mut e = dec_event(seq, v.value())?;
                if rename_attr(&mut e.by, pid, ps) {
                    rows.push((seq, enc_event(&e)));
                }
            }
        }
        c.attributions += rows.len() as u64;
        let mut t = w.open_table(ANNOT_LOG)?;
        for (seq, b) in rows {
            t.insert((doc, seq), b.as_slice())?;
        }
    }
    for id in changed {
        if let Some(rec) = threads.get_mut(&id) {
            put_changed(w, doc, id, rec)?;
        }
    }
    Ok(c)
}

/// Whether `pid` is a participant's in `doc`'s records (an author of a version or approval,
/// or of a thread, comment or history row).
fn pid_in_doc(w: &WriteTransaction, doc: DocId, pid: &str) -> Result<bool, StoreError> {
    if history::has_table(w, BLOB_VERSIONS.name())? {
        let t = w.open_table(BLOB_VERSIONS)?;
        for kv in t.range((doc, 0)..=(doc, u64::MAX))? {
            let v = dec_version(kv?.1.value())?;
            if v.author == pid || v.on_behalf_of.as_deref() == Some(pid) {
                return Ok(true);
            }
        }
    }
    let attr = |a: &Attribution| a.pid == pid || a.on_behalf_of.as_deref() == Some(pid);
    for rec in all_threads(w, doc)?.values() {
        if attr(&rec.created) {
            return Ok(true);
        }
    }
    if history::has_table(w, ANNOT_COMMENTS.name())? {
        let t = w.open_table(ANNOT_COMMENTS)?;
        for kv in t.range((doc, 0, 0)..=(doc, u64::MAX, u64::MAX))? {
            let cm = dec_comment(0, kv?.1.value())?;
            if attr(&cm.by) || cm.mentions.iter().any(|m| m == pid) {
                return Ok(true);
            }
        }
    }
    if history::has_table(w, ANNOT_LOG.name())? {
        let t = w.open_table(ANNOT_LOG)?;
        for kv in t.range((doc, 0)..=(doc, u64::MAX))? {
            let (k, v) = kv?;
            if attr(&dec_event(k.value().1, v.value())?.by) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// What a purge of `what` takes from one thread.
fn purge_thread_in(
    w: &WriteTransaction,
    doc: DocId,
    id: u64,
    what: &AnnotPurge,
    needles: Option<&Needles>,
    by: &Attribution,
    counts: &mut AnnotCounts,
) -> Result<std::result::Result<(), AnnotFail>, StoreError> {
    let t = ThreadId(id);
    let Some(mut rec) = get_thread(w, doc, id)? else {
        return Ok(Err(AnnotFail::NoThread(t)));
    };
    let hits = |s: &str| needles.is_some_and(|n| n.hits(s.as_bytes()));
    let keep = planted!(PurgeKeepsBodies);
    let mut reached = false;
    let mut one: Option<u32> = None;
    let comments = comments_in(w, doc, id)?;
    if let AnnotPurge::Comment(_, n) = what {
        if !comments.iter().any(|c| c.n == *n) {
            return Ok(Err(AnnotFail::NoComment(t, *n)));
        }
        one = Some(*n);
    }
    for mut c in comments {
        let take = match what {
            AnnotPurge::Comment(_, n) => c.n == *n,
            AnnotPurge::Thread(_) | AnnotPurge::All => true,
            AnnotPurge::Text(_) => {
                c.body.as_deref().is_some_and(hits) || c.attachments.iter().any(|a| hits(&a.target))
            }
        };
        if !take {
            continue;
        }
        reached = true;
        if !keep && clear(&mut c) {
            counts.comments += 1;
            w.open_table(ANNOT_COMMENTS)?
                .insert((doc, id, u64::from(c.n)), enc_comment(&c).as_slice())?;
        }
    }
    if !matches!(what, AnnotPurge::Comment(..)) {
        let take = match what {
            AnnotPurge::Text(_) => anchor_hits(&rec, hits),
            _ => true,
        };
        if take && holds_node_copy(&rec) {
            reached = true;
            if !keep {
                clear_anchor(&mut rec);
                counts.anchors += 1;
            }
        }
    }
    if reached {
        let ev = AnnotEvent {
            seq: 0,
            thread: t,
            comment: one,
            op: AnnotOp::Purged,
            by: by.clone(),
            to: None,
            version: None,
        };
        append(w, doc, id, &mut rec, &ev)?;
        put_thread(w, doc, id, &mut rec)?;
    }
    Ok(Ok(()))
}

impl DocStore {
    /// A new thread with its first comment and its `Created` row, in one transaction. Refused
    /// past `max_threads` threads in the doc, and when a node anchor's version was purged
    /// since it was read.
    pub(crate) fn annot_create(
        &self,
        doc: DocId,
        rec: &ThreadRec,
        first: &Comment,
        ev: &AnnotEvent,
        max_threads: u64,
    ) -> Out<Committed> {
        let w = self.begin_write()?;
        if holds_node_copy(rec) && !version_live(&w, doc, rec.anchor.created_version)? {
            return Ok(Err(AnnotFail::VersionGone(rec.anchor.created_version)));
        }
        let id = {
            let t = w.open_table(ANNOT_THREADS)?;
            let last = last_key(&t, doc)?;
            if last >= max_threads {
                return Ok(Err(AnnotFail::Cap {
                    what: "threads",
                    limit: max_threads,
                }));
            }
            last + 1
        };
        w.open_table(ANNOT_COMMENTS)?
            .insert((doc, id, 0), enc_comment(first).as_slice())?;
        let mut rec = rec.clone();
        rec.changed = 0;
        rec.log_rows = 0;
        let mut ev = ev.clone();
        ev.comment = Some(0);
        let seq = append(&w, doc, id, &mut rec, &ev)?;
        put_thread(&w, doc, id, &mut rec)?;
        let comments = comments_in(&w, doc, id)?;
        w.commit()?;
        Ok(Ok(Committed {
            id: ThreadId(id),
            seq: Some(seq),
            n: Some(0),
            rec,
            comments,
        }))
    }

    /// A reply on thread `t` and its row. Refused past `max_comments` comments on it.
    pub(crate) fn annot_reply(
        &self,
        doc: DocId,
        t: ThreadId,
        c: &Comment,
        ev: &AnnotEvent,
        max_comments: u64,
    ) -> Out<Committed> {
        let w = self.begin_write()?;
        let Some(mut rec) = get_thread(&w, doc, t.0)? else {
            return Ok(Err(AnnotFail::NoThread(t)));
        };
        let n = {
            let mut tab = w.open_table(ANNOT_COMMENTS)?;
            let last = match tab.range((doc, t.0, 0)..=(doc, t.0, u64::MAX))?.next_back() {
                Some(kv) => kv?.0.value().2,
                None => return Err(StoreError::Corrupt("annotation thread with no comment")),
            };
            let n = last + 1;
            if n >= max_comments {
                return Ok(Err(AnnotFail::Cap {
                    what: "comments",
                    limit: max_comments,
                }));
            }
            let n = u32::try_from(n).map_err(|_| StoreError::Corrupt("annotation comment"))?;
            tab.insert((doc, t.0, u64::from(n)), enc_comment(c).as_slice())?;
            n
        };
        let mut ev = ev.clone();
        ev.comment = Some(n);
        let seq = append(&w, doc, t.0, &mut rec, &ev)?;
        put_thread(&w, doc, t.0, &mut rec)?;
        let comments = comments_in(&w, doc, t.0)?;
        w.commit()?;
        Ok(Ok(Committed {
            id: t,
            seq: Some(seq),
            n: Some(n),
            rec,
            comments,
        }))
    }

    /// Change thread `t`'s row with `f`, in one transaction: `f` returns the history row to
    /// add (`None`: nothing changed, nothing written). `capped`: the op is one the thread's
    /// history cap refuses past `log_max` rows (a toggle or a move). When `f` leaves the
    /// anchor holding a node copy, its version is re-checked here.
    pub(crate) fn annot_update(
        &self,
        doc: DocId,
        t: ThreadId,
        log_max: Option<u32>,
        f: &mut dyn FnMut(&mut ThreadRec) -> Result<Option<AnnotEvent>, AnnotFail>,
    ) -> Out<Committed> {
        let w = self.begin_write()?;
        let Some(mut rec) = get_thread(&w, doc, t.0)? else {
            return Ok(Err(AnnotFail::NoThread(t)));
        };
        let before = rec.anchor.clone();
        let ev = match f(&mut rec) {
            Ok(Some(ev)) => ev,
            Ok(None) => {
                let comments = comments_in(&w, doc, t.0)?;
                return Ok(Ok(Committed {
                    id: t,
                    seq: None,
                    n: None,
                    rec,
                    comments,
                }));
            }
            Err(e) => return Ok(Err(e)),
        };
        if let Some(max) = log_max {
            if rec.log_rows >= max {
                return Ok(Err(AnnotFail::Cap {
                    what: "history rows",
                    limit: u64::from(max),
                }));
            }
        }
        if rec.anchor != before
            && holds_node_copy(&rec)
            && !version_live(&w, doc, rec.anchor.created_version)?
        {
            return Ok(Err(AnnotFail::VersionGone(rec.anchor.created_version)));
        }
        let seq = append(&w, doc, t.0, &mut rec, &ev)?;
        put_thread(&w, doc, t.0, &mut rec)?;
        let comments = comments_in(&w, doc, t.0)?;
        w.commit()?;
        Ok(Ok(Committed {
            id: t,
            seq: Some(seq),
            n: None,
            rec,
            comments,
        }))
    }

    /// Delete comment `n` of thread `t`: its body, mentions and attachments are cleared at
    /// once, and `by` recorded. `may` says whether the caller may (the author, the person it
    /// was for, an owner). An already deleted comment is left as it is (`seq: None`).
    pub(crate) fn annot_delete(
        &self,
        doc: DocId,
        t: ThreadId,
        n: u32,
        by: &Attribution,
        may: &dyn Fn(&Comment) -> bool,
    ) -> Out<Committed> {
        let w = self.begin_write()?;
        let Some(mut rec) = get_thread(&w, doc, t.0)? else {
            return Ok(Err(AnnotFail::NoThread(t)));
        };
        let mut c = {
            let tab = w.open_table(ANNOT_COMMENTS)?;
            let Some(v) = tab.get((doc, t.0, u64::from(n)))? else {
                return Ok(Err(AnnotFail::NoComment(t, n)));
            };
            dec_comment(n, v.value())?
        };
        if !may(&c) {
            return Ok(Err(AnnotFail::NotYours(t, n)));
        }
        if c.deleted.is_some() {
            let comments = comments_in(&w, doc, t.0)?;
            return Ok(Ok(Committed {
                id: t,
                seq: None,
                n: Some(n),
                rec,
                comments,
            }));
        }
        if !planted!(DeleteKeepsBody) {
            clear(&mut c);
        }
        c.deleted = Some(by.clone());
        w.open_table(ANNOT_COMMENTS)?
            .insert((doc, t.0, u64::from(n)), enc_comment(&c).as_slice())?;
        let ev = AnnotEvent {
            seq: 0,
            thread: t,
            comment: Some(n),
            op: AnnotOp::Deleted,
            by: by.clone(),
            to: None,
            version: None,
        };
        let seq = append(&w, doc, t.0, &mut rec, &ev)?;
        put_thread(&w, doc, t.0, &mut rec)?;
        let comments = comments_in(&w, doc, t.0)?;
        w.commit()?;
        Ok(Ok(Committed {
            id: t,
            seq: Some(seq),
            n: Some(n),
            rec,
            comments,
        }))
    }

    /// Thread `t`'s row and comments.
    pub(crate) fn annot_thread(
        &self,
        doc: DocId,
        t: ThreadId,
    ) -> Result<Option<(ThreadRec, Vec<Comment>)>, StoreError> {
        let r = self.begin_read()?;
        let threads = match r.open_table(ANNOT_THREADS) {
            Ok(x) => x,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let Some(v) = threads.get((doc, t.0))? else {
            return Ok(None);
        };
        let rec = dec_thread(v.value())?;
        let tab = r.open_table(ANNOT_COMMENTS)?;
        let mut comments = Vec::new();
        for kv in tab.range((doc, t.0, 0)..=(doc, t.0, u64::MAX))? {
            let (k, v) = kv?;
            let n = u32::try_from(k.value().2)
                .map_err(|_| StoreError::Corrupt("annotation comment"))?;
            comments.push(dec_comment(n, v.value())?);
        }
        Ok(Some((rec, comments)))
    }

    /// Thread rows of `doc` after `after`, in id order, at most `limit`.
    pub(crate) fn annot_threads(
        &self,
        doc: DocId,
        after: u64,
        limit: usize,
    ) -> Result<Vec<(ThreadId, ThreadRec)>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(ANNOT_THREADS) {
            Ok(x) => x,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        let Some(from) = after.checked_add(1) else {
            return Ok(out);
        };
        for kv in t.range((doc, from)..=(doc, u64::MAX))? {
            if out.len() >= limit {
                break;
            }
            let (k, v) = kv?;
            out.push((ThreadId(k.value().1), dec_thread(v.value())?));
        }
        Ok(out)
    }

    /// The node-anchored threads of `doc` (an index record resolves them).
    pub(crate) fn annot_node_threads(
        &self,
        doc: DocId,
    ) -> Result<Vec<(ThreadId, ThreadRec)>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(ANNOT_THREADS) {
            Ok(x) => x,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for kv in t.range((doc, 0)..=(doc, u64::MAX))? {
            let (k, v) = kv?;
            let rec = dec_thread(v.value())?;
            if matches!(rec.anchor.target, AnchorTarget::Node { .. }) {
                out.push((ThreadId(k.value().1), rec));
            }
        }
        Ok(out)
    }

    /// The doc's newest change seq (0: none): a page's `head`.
    pub(crate) fn annot_head(&self, doc: DocId) -> Result<u64, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(ANNOT_CHANGED) {
            Ok(x) => x,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
            Err(e) => return Err(e.into()),
        };
        Ok(match t.range((doc, 0)..=(doc, u64::MAX))?.next_back() {
            Some(kv) => kv?.0.value().1,
            None => 0,
        })
    }

    /// The threads that changed after change seq `since`: one row a thread, so at most the
    /// doc's thread count is read.
    pub(crate) fn annot_changed_since(
        &self,
        doc: DocId,
        since: u64,
    ) -> Result<BTreeSet<ThreadId>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(ANNOT_CHANGED) {
            Ok(x) => x,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(BTreeSet::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = BTreeSet::new();
        let Some(from) = since.checked_add(1) else {
            return Ok(out);
        };
        for kv in t.range((doc, from)..=(doc, u64::MAX))? {
            let (_, v) = kv?;
            let b: [u8; 8] = v
                .value()
                .try_into()
                .map_err(|_| StoreError::Corrupt("annotation change"))?;
            out.insert(ThreadId(u64::from_le_bytes(b)));
        }
        Ok(out)
    }

    /// History rows of `doc` after `since` (of thread `thread` only, through its index, when
    /// named), oldest first, at most `limit`.
    pub(crate) fn annot_events(
        &self,
        doc: DocId,
        thread: Option<ThreadId>,
        since: u64,
        limit: usize,
    ) -> Result<Vec<AnnotEvent>, StoreError> {
        let r = self.begin_read()?;
        let log = match r.open_table(ANNOT_LOG) {
            Ok(x) => x,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        let Some(from) = since.checked_add(1) else {
            return Ok(out);
        };
        match thread {
            None => {
                for kv in log.range((doc, from)..=(doc, u64::MAX))? {
                    if out.len() >= limit {
                        break;
                    }
                    let (k, v) = kv?;
                    out.push(dec_event(k.value().1, v.value())?);
                }
            }
            Some(t) => {
                let ix = r.open_table(ANNOT_THREAD_LOG)?;
                for kv in ix.range((doc, t.0, from)..=(doc, t.0, u64::MAX))? {
                    if out.len() >= limit {
                        break;
                    }
                    let seq = kv?.0.value().2;
                    let v = log
                        .get((doc, seq))?
                        .ok_or(StoreError::Corrupt("annotation history index"))?;
                    out.push(dec_event(seq, v.value())?);
                }
            }
        }
        Ok(out)
    }

    /// After an index of `version` was recorded: apply each update whose thread hasn't
    /// changed since it was read (compare and set on its change seq), in one short
    /// transaction that re-checks the version is still live. Returns the threads updated.
    pub(crate) fn annot_observe_apply(
        &self,
        doc: DocId,
        version: VersionId,
        updates: &[Observed],
    ) -> Result<Vec<ThreadId>, StoreError> {
        if updates.is_empty() {
            return Ok(Vec::new());
        }
        let w = self.begin_write()?;
        if !version_live(&w, doc, version)? {
            return Ok(Vec::new());
        }
        let mut done = Vec::new();
        for u in updates {
            let Some(cur) = get_thread(&w, doc, u.id.0)? else {
                continue;
            };
            if cur.changed != u.expected {
                continue;
            }
            let mut rec = u.rec.clone();
            put_changed(&w, doc, u.id.0, &mut rec)?;
            done.push(u.id);
        }
        w.commit()?;
        Ok(done)
    }

    /// An owner's annotation purge: clear what `what` names (bodies; with `Thread`, `Text`
    /// and `All` the anchors' copies of node content too, cleared for good until a re-anchor),
    /// with a `Purged` history row per thread reached, in one transaction. `needles` is
    /// `Text`'s.
    pub(crate) fn annot_purge(
        &self,
        doc: DocId,
        what: &AnnotPurge,
        needles: Option<&Needles>,
        by: &Attribution,
    ) -> Out<AnnotCounts> {
        let w = self.begin_write()?;
        let mut counts = AnnotCounts::default();
        let ids: Vec<u64> = match what {
            AnnotPurge::Comment(t, _) | AnnotPurge::Thread(t) => vec![t.0],
            AnnotPurge::Text(_) | AnnotPurge::All => all_threads(&w, doc)?.into_keys().collect(),
        };
        for id in ids {
            if let Err(e) = purge_thread_in(&w, doc, id, what, needles, by, &mut counts)? {
                return Ok(Err(e));
            }
        }
        w.commit()?;
        Ok(Ok(counts))
    }

    /// Participant erasure on a blob doc, in one transaction: `pid` becomes `ps` in every
    /// attribution of its annotations (threads, comments, history, mentions) and of its
    /// versions and approvals; the bodies they wrote, or an agent wrote on their behalf, are
    /// cleared. Refused when `ps` is already a participant's pid in the doc's records.
    pub(crate) fn blob_erase_participant(
        &self,
        doc: DocId,
        pid: &str,
        ps: &str,
    ) -> Out<AnnotCounts> {
        let w = self.begin_write()?;
        if !planted!(EraseSkipsChecks) && pid_in_doc(&w, doc, ps)? {
            return Ok(Err(AnnotFail::Erasure(
                "the pseudonym is already a participant's id in this doc",
            )));
        }
        let mut c = erase_in(&w, doc, pid, ps)?;
        {
            let mut t = w.open_table(BLOB_VERSIONS)?;
            let mut rows: Vec<(u64, Vec<u8>)> = Vec::new();
            for kv in t.range((doc, 0)..=(doc, u64::MAX))? {
                let (k, v) = kv?;
                let mut ver = dec_version(v.value())?;
                let mine = rename(&mut ver.author, pid, ps);
                if mine {
                    ver.author_name = ps.to_owned();
                }
                let obo = ver
                    .on_behalf_of
                    .as_mut()
                    .is_some_and(|o| rename(o, pid, ps));
                if mine | obo {
                    rows.push((k.value().1, enc_version(&ver)));
                }
            }
            c.attributions += rows.len() as u64;
            for (vid, b) in rows {
                t.insert((doc, vid), b.as_slice())?;
            }
        }
        {
            let mut t = w.open_table(BLOB_APPROVALS)?;
            let mut rows: Vec<(u64, Vec<u8>)> = Vec::new();
            for kv in t.range((doc, 0)..=(doc, u64::MAX))? {
                let (k, v) = kv?;
                let mut a = dec_approval(v.value())?;
                let hit = rename(&mut a.author, pid, ps)
                    | rename(&mut a.on_behalf_of, pid, ps)
                    | a.by.as_mut().is_some_and(|b| rename(b, pid, ps));
                if hit {
                    rows.push((k.value().1, enc_approval(&a)));
                }
            }
            c.attributions += rows.len() as u64;
            for (vid, b) in rows {
                t.insert((doc, vid), b.as_slice())?;
            }
        }
        w.commit()?;
        Ok(Ok(c))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::anchor::{Anchor, LastSeen};
    use crate::blob::BlobId;
    use crate::index::{Fingerprint, NodeKey};
    use crate::store::{BlobReason, BlobVersion, DocKey, DocMeta};

    fn m<R>(f: impl FnOnce() -> R) -> R {
        crate::differential::under_env_annot_mutant(f)
    }

    fn store(tag: &str) -> (DocStore, std::path::PathBuf) {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2mb-annot-unit-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (DocStore::open(dir.join("docs.redb")).unwrap(), dir)
    }

    fn id(b: u8) -> BlobId {
        BlobId::from_slice(&[b; 32]).unwrap()
    }

    /// A blob doc: version 1 (blob 1), version 2 (current, blob 2, by `a`).
    fn blob_doc(s: &DocStore) -> DocId {
        let k = s.blob_space_or_create("ws", [7; 32]).unwrap();
        s.blob_index(k.no, &id(1), 3, 1).unwrap();
        s.blob_index(k.no, &id(2), 3, 1).unwrap();
        let meta = DocMeta {
            key: DocKey {
                space: "ws".into(),
                path: "p".into(),
            },
            pack: crate::store::BLOB_DOC_PACK.into(),
            engine: crate::engine::NO_ENGINE.to_owned(),
            engine_format: 0,
            codec: Vec::new(),
            created_ms: 1,
            synced_hash: None,
            synced_seq: 0,
            declined: None,
        };
        let v = BlobVersion::new(
            id(1),
            3,
            1,
            "kit:import",
            "import",
            "disk",
            BlobReason::Import,
        );
        let (doc, _) = s.create_blob_doc(&meta, &v, None).unwrap();
        let v2 = BlobVersion::new(id(2), 3, 2, "a", "a", "cli", BlobReason::Write);
        s.commit_blob_versions(doc, "ws", 1, &[v2], None).unwrap();
        doc
    }

    fn tables(s: &DocStore) -> Vec<String> {
        let r = s.begin_read().unwrap();
        let mut names: Vec<String> = r
            .list_tables()
            .unwrap()
            .map(|t| t.name().to_owned())
            .filter(|n| n.starts_with("annot_"))
            .collect();
        names.sort();
        names
    }

    fn by(pid: &str) -> Attribution {
        Attribution {
            pid: pid.into(),
            name: pid.into(),
            via: "tab".into(),
            on_behalf_of: None,
            at_ms: 1,
        }
    }

    fn rec(anchor: Anchor) -> ThreadRec {
        ThreadRec {
            anchor,
            status: ThreadStatus::Open,
            created: by("a"),
            pending: None,
            last_state: None,
            changed: 0,
            log_rows: 0,
            purged: false,
        }
    }

    fn first() -> Comment {
        Comment {
            n: 0,
            by: by("a"),
            body: Some("hi".into()),
            mentions: Vec::new(),
            attachments: Vec::new(),
            deleted: None,
        }
    }

    fn created() -> AnnotEvent {
        AnnotEvent {
            seq: 0,
            thread: ThreadId(0),
            comment: None,
            op: AnnotOp::Created,
            by: by("a"),
            to: None,
            version: None,
        }
    }

    /// A node anchor made on `version`, holding that version's node name.
    fn node_anchor(version: u64) -> Anchor {
        Anchor {
            created_version: version,
            target: AnchorTarget::Node {
                key: NodeKey("@n:1".into()),
            },
            hint: None,
            fingerprint: Some(Fingerprint {
                kind: "raster".into(),
                name: "Secret layer".into(),
                bounds: None,
                content: None,
                weak: Default::default(),
            }),
            last_seen: Some(LastSeen::new(version, "Secret layer", None)),
        }
    }

    /// A store no comment reached keeps no annotation table: a version purge and an erasure
    /// (the paths every blob doc can take) don't create one, so a host that hasn't adopted
    /// comments can roll the kit back and still purge and rewrite.
    #[test]
    fn no_comment_no_annotation_table() {
        let (s, dir) = store("lazy");
        let doc = blob_doc(&s);
        s.blob_purge_versions(doc, &[1], 3).unwrap();
        s.blob_erase_participant(doc, "a", "erased-1")
            .unwrap()
            .unwrap();
        assert!(s.annot_threads(doc, 0, 10).unwrap().is_empty());
        assert_eq!(s.annot_head(doc).unwrap(), 0);
        assert!(
            tables(&s).is_empty(),
            "no annotation table: {:?}",
            tables(&s)
        );
        s.check_rewritable().unwrap();
        // the first comment creates all five
        let r = rec(Anchor::doc(2));
        let c1 = s
            .annot_create(doc, &r, &first(), &created(), 2)
            .unwrap()
            .unwrap();
        assert_eq!((c1.id, c1.seq, c1.rec.changed), (ThreadId(1), Some(1), 1));
        assert_eq!(
            tables(&s),
            [
                "annot_changed",
                "annot_comments",
                "annot_log",
                "annot_thread_log",
                "annot_threads"
            ]
        );
        // ids are never reused, and the cap counts threads
        let c2 = s
            .annot_create(doc, &r, &first(), &created(), 2)
            .unwrap()
            .unwrap();
        assert_eq!(c2.id, ThreadId(2));
        assert!(matches!(
            s.annot_create(doc, &r, &first(), &created(), 2).unwrap(),
            Err(AnnotFail::Cap {
                what: "threads",
                limit: 2
            })
        ));
        // `since` reads one row a thread
        assert_eq!(s.annot_head(doc).unwrap(), 2);
        let since: Vec<ThreadId> = s.annot_changed_since(doc, 1).unwrap().into_iter().collect();
        assert_eq!(since, [ThreadId(2)]);
        // a thread's history through its own index
        let evs = s.annot_events(doc, Some(ThreadId(2)), 0, 10).unwrap();
        let threads: Vec<ThreadId> = evs.iter().map(|e| e.thread).collect();
        assert_eq!(threads, [ThreadId(2)]);
        // a doc delete's tables erase all five tables' rows
        let w = s.begin_write().unwrap();
        for (rows, _) in super::super::DOC_TABLES {
            rows.erase(&w, doc).unwrap();
        }
        w.commit().unwrap();
        assert!(s.annot_thread(doc, ThreadId(1)).unwrap().is_none());
        assert_eq!(s.annot_head(doc).unwrap(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A node's content is only written from a live version: a comment, a re-anchor or an
    /// index record whose version a purge took between its read and its commit is refused,
    /// and writes nothing.
    #[test]
    fn an_anchor_is_never_written_from_a_purged_version() {
        m(|| {
            let (s, dir) = store("purged-version");
            let doc = blob_doc(&s);
            s.blob_purge_versions(doc, &[1], 3).unwrap();
            let got = s
                .annot_create(doc, &rec(node_anchor(1)), &first(), &created(), 10)
                .unwrap();
            assert!(
                matches!(got, Err(AnnotFail::VersionGone(1))),
                "an anchor is never written from a purged version"
            );
            assert!(s.annot_threads(doc, 0, 10).unwrap().is_empty());
            // a re-anchor to it
            let t = s
                .annot_create(doc, &rec(Anchor::doc(2)), &first(), &created(), 10)
                .unwrap()
                .unwrap()
                .id;
            let got = s
                .annot_update(doc, t, None, &mut |r| {
                    r.anchor = node_anchor(1);
                    Ok(Some(created()))
                })
                .unwrap();
            assert!(
                matches!(got, Err(AnnotFail::VersionGone(1))),
                "an anchor is never written from a purged version"
            );
            // an index record of it
            let cur = s.annot_thread(doc, t).unwrap().unwrap().0;
            let mut new = cur.clone();
            new.anchor.last_seen = Some(LastSeen::new(1, "Secret layer", None));
            let done = s
                .annot_observe_apply(
                    doc,
                    1,
                    &[Observed {
                        id: t,
                        expected: cur.changed,
                        rec: new,
                    }],
                )
                .unwrap();
            assert!(
                done.is_empty(),
                "an anchor is never written from a purged version"
            );
            let _ = std::fs::remove_dir_all(dir);
        });
    }
}
