//! A purge of one text doc in the store: one transaction over every kit table that holds
//! its content (K-PURGE-2, -3, -7, -13). The session decides what is targeted and supplies
//! the re-encoded state; [`super::DocStore::rewrite_file`] then leaves no bytes behind.
//!
//! - **Log** (`updates`): every entry up to the purge's head becomes `redacted {byte
//!   length}`; its author, client, time, surface and `on_behalf_of` stay. A new snapshot at
//!   the head (the state re-encoded from the live doc, so no deleted content rides along as
//!   CRDT tombstones) replaces what they held.
//! - **Snapshot**: that new snapshot.
//! - **Versions**: the targeted ones lose all their content, the others the spans the
//!   needles cover; entries (and names) stay, with their [`Redaction`]s.
//! - **Revert records**: the targeted changes' hunks, and every other hunk whose inserted or
//!   deleted text a needle matches, are blanked and marked `redacted` (a revert over them
//!   reports `erased`). Change ids, authors, times and lengths stay (`changes` keeps each
//!   change's hunk positions and lengths).
//! - **Sync table**: the replica and its hash as the session gives them; any write-back in
//!   flight is dropped.
//! - **Hash key**: replaced, so every earlier keyed hash of this doc means nothing.
//! - **Participant erasure**: the pid is replaced by the pseudonym in the log, changes,
//!   revert records, approvals, versions' authors and the client table.

use std::collections::HashSet;
use std::ops::Range;

use redb::{ReadableTable, WriteTransaction};

use super::history::{
    dec_change, dec_pair, dec_record, enc_change, enc_pair, enc_record, enc_stats, stats_in,
    CHANGES, REVERTS, REVERT_INDEX, REVERT_STATS,
};
use super::{
    dec_approval, dec_entry_full, dec_meta, dec_version_full, enc_approval, enc_doc_key,
    enc_entry_as, enc_meta, enc_snapshot, enc_sync, enc_version_redacted, ApprovalState, DocId,
    DocStore, Needles, Neutral, Redaction, Seq, StoreError, SyncState, APPROVALS, CLIENTS, DOCS,
    DOC_KEYS, PENDING, SNAPSHOTS, SYNC, SYNCED, UPDATES, VERSIONS,
};

/// Which records a purge takes whole (by change seq, or by version id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pick {
    None,
    All,
    Range(u64, u64),
    Set(HashSet<u64>),
}

impl Pick {
    pub(crate) fn has(&self, x: u64) -> bool {
        match self {
            Pick::None => false,
            Pick::All => true,
            Pick::Range(a, b) => (*a..=*b).contains(&x),
            Pick::Set(s) => s.contains(&x),
        }
    }
}

/// What to purge from one text doc.
pub(crate) struct DocPurge<'a> {
    /// The doc's version now: entries up to it are redacted, the snapshot is taken at it.
    pub head: Seq,
    /// The live doc's state, re-encoded (GC on: no deleted content).
    pub state: &'a [u8],
    /// Revert records redacted whole.
    pub changes: Pick,
    /// Versions whose content is redacted whole.
    pub versions: Pick,
    /// Matched in every record not taken whole.
    pub needles: &'a Needles,
    /// The new synced state, when the doc has one.
    pub sync: Option<SyncState>,
    /// The doc's new hash key.
    pub key: [u8; 32],
    /// Participant erasure: `(pid, pseudonym)`.
    pub erase: Option<(&'a str, &'a str)>,
}

/// What a purge did (metadata only: counts and sizes, never content).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PurgeCounts {
    /// Log entries whose update became `redacted {byte length}`.
    pub log_entries: u64,
    /// Versions whose content lost spans (their entries stay).
    pub versions: u64,
    /// Revert-record hunks blanked.
    pub revert_hunks: u64,
    /// Content bytes taken out of versions and revert records.
    pub bytes: u64,
    /// Rows where participant erasure replaced the pid.
    pub attributions: u64,
    /// Undo and redo batches cleared (every stack of the doc, spec U19).
    pub undo_batches: u64,
    /// Cached read bases dropped.
    pub read_bases: u64,
    /// Tier 0: blob versions purged, blobs removed, conflict copies removed.
    pub blob_versions: u64,
    pub blobs_removed: u64,
    pub conflict_copies: u64,
    /// Tier 0: approval records of the purged blob versions (on-behalf writes). Reached and
    /// kept: they are metadata (agent, person, state, settler, time) and hold no content
    /// or blob id, like the version's entry that stays.
    pub approvals: u64,
}

/// Replace `pid` by the pseudonym in `s`; returns whether it did.
fn rename(s: &mut String, erase: Option<(&str, &str)>) -> bool {
    match erase {
        Some((pid, ps)) if s == pid => {
            *s = ps.to_owned();
            true
        }
        _ => false,
    }
}

fn rename_opt(s: &mut Option<String>, erase: Option<(&str, &str)>) -> bool {
    s.as_mut().is_some_and(|s| rename(s, erase))
}

/// Cut the `spans` (in `text`'s own coordinates, sorted, disjoint) out of `text`; add them,
/// in original coordinates, to `red` (this root's earlier redactions, original coordinates,
/// sorted). Returns the new text.
fn cut(text: &str, spans: &[Range<usize>], root: &str, red: &mut Vec<Redaction>) -> String {
    let mut gaps: Vec<(u64, u64)> = red
        .iter()
        .filter(|r| r.root == root)
        .map(|r| (r.start, r.len))
        .collect();
    gaps.sort_unstable();
    // a position of the cut text in original coordinates: past every gap at or before it
    // (`incl`), or strictly before it (an end)
    let orig = |x: usize, incl: bool| -> u64 {
        let mut o = x as u64;
        for &(s, l) in &gaps {
            if s < o || (incl && s == o) {
                o += l;
            } else {
                break;
            }
        }
        o
    };
    let mut new: Vec<(u64, u64)> = gaps.clone();
    for sp in spans {
        let (s, e) = (orig(sp.start, true), orig(sp.end, false));
        new.push((s, e.saturating_sub(s)));
    }
    new.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (s, l) in new {
        match merged.last_mut() {
            Some((ms, ml)) if s <= *ms + *ml => *ml = (*ml).max(s + l - *ms),
            _ => merged.push((s, l)),
        }
    }
    red.retain(|r| r.root != root);
    red.extend(merged.into_iter().map(|(start, len)| Redaction {
        root: root.to_owned(),
        start,
        len,
    }));
    red.sort_by(|a, b| (&a.root, a.start).cmp(&(&b.root, b.start)));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for sp in spans {
        out.push_str(&text[at..sp.start]);
        at = sp.end;
    }
    out.push_str(&text[at..]);
    out
}

impl DocStore {
    /// Purge doc `id` as `p` says, in one durable transaction (module docs).
    pub(crate) fn purge_doc(&self, id: DocId, p: &DocPurge<'_>) -> Result<PurgeCounts, StoreError> {
        let w = self.begin_write()?;
        let c = purge_in(&w, id, p)?;
        w.commit()?;
        Ok(c)
    }
}

fn purge_in(w: &WriteTransaction, id: DocId, p: &DocPurge<'_>) -> Result<PurgeCounts, StoreError> {
    let mut c = PurgeCounts::default();
    let er = p.erase;
    let mut meta = {
        let docs = w.open_table(DOCS)?;
        let m = docs.get(id)?.ok_or(StoreError::NoDoc(id))?;
        dec_meta(m.value())?
    };
    // the log: metadata only from here on
    {
        let mut t = w.open_table(UPDATES)?;
        let rows: Vec<(Seq, Vec<u8>)> = t
            .range((id, 0)..=(id, p.head))?
            .map(|kv| kv.map(|(k, v)| (k.value().1, v.value().to_vec())))
            .collect::<Result<_, _>>()?;
        for (seq, b) in rows {
            let (mut e, red) = dec_entry_full(&b)?;
            let mut changed = red.is_none();
            if changed {
                c.log_entries += 1;
            }
            let n = red.unwrap_or(e.update.len() as u64);
            if rename(&mut e.author, er) | rename_opt(&mut e.on_behalf_of, er) {
                c.attributions += 1;
                changed = true;
            }
            if changed {
                t.insert((id, seq), enc_entry_as(&e, Some(n)).as_slice())?;
            }
        }
    }
    w.open_table(SNAPSHOTS)?
        .insert(id, enc_snapshot(p.head, p.state).as_slice())?;
    // versions
    {
        let mut t = w.open_table(VERSIONS)?;
        let rows: Vec<(u64, Vec<u8>)> = t
            .range((id, 0)..=(id, u64::MAX))?
            .map(|kv| kv.map(|(k, v)| (k.value().1, v.value().to_vec())))
            .collect::<Result<_, _>>()?;
        for (vid, b) in rows {
            let (mut v, mut red) = dec_version_full(&b)?;
            let whole = p.versions.has(vid);
            let mut cut_any = false;
            for (root, val) in &mut v.content {
                let Neutral::Text(text) = val;
                let spans: Vec<Range<usize>> = if whole {
                    if text.is_empty() {
                        Vec::new()
                    } else {
                        std::iter::once(0..text.len()).collect()
                    }
                } else {
                    p.needles.text_spans(text)
                };
                if spans.is_empty() {
                    continue;
                }
                c.bytes += spans.iter().map(|s| (s.end - s.start) as u64).sum::<u64>();
                *text = cut(text, &spans, root, &mut red);
                cut_any = true;
            }
            let mut renamed = false;
            for a in &mut v.authors {
                renamed |= rename(a, er);
            }
            if renamed {
                // two authors may now be one pseudonym: keep the list a set
                let mut seen = HashSet::new();
                v.authors.retain(|a| seen.insert(a.clone()));
                c.attributions += 1;
            }
            if cut_any {
                c.versions += 1;
            }
            if cut_any || renamed {
                t.insert((id, vid), enc_version_redacted(&v, &red).as_slice())?;
            }
        }
    }
    // revert records, their index sizes and the doc's totals
    {
        let pending: HashSet<Seq> = {
            let t = w.open_table(APPROVALS)?;
            let mut s = HashSet::new();
            for kv in t.range((id, 0)..=(id, u64::MAX))? {
                let (k, v) = kv?;
                if dec_approval(v.value())?.state == ApprovalState::Pending {
                    s.insert(k.value().1);
                }
            }
            s
        };
        let mut rt = w.open_table(REVERTS)?;
        let mut it = w.open_table(REVERT_INDEX)?;
        let mut st = w.open_table(REVERT_STATS)?;
        let mut stats = stats_in(&st, id)?;
        let rows: Vec<(Seq, Vec<u8>)> = rt
            .range((id, 0)..=(id, u64::MAX))?
            .map(|kv| kv.map(|(k, v)| (k.value().1, v.value().to_vec())))
            .collect::<Result<_, _>>()?;
        for (seq, b) in rows {
            let mut rec = dec_record(&b)?;
            let whole = p.changes.has(seq);
            let mut changed = false;
            for h in rec.roots.iter_mut().flat_map(|r| r.hunks.iter_mut()) {
                if h.redacted {
                    continue;
                }
                let hit = whole
                    || p.needles.hits(h.insert.as_bytes())
                    || p.needles.hits(h.deleted.as_bytes());
                if hit {
                    c.bytes += (h.insert.len() + h.deleted.len()) as u64;
                    c.revert_hunks += 1;
                    h.insert.clear();
                    h.deleted.clear();
                    h.redacted = true;
                    changed = true;
                }
            }
            if rename(&mut rec.author, er) | rename_opt(&mut rec.on_behalf_of, er) {
                c.attributions += 1;
                changed = true;
            }
            if !changed {
                continue;
            }
            let nb = enc_record(&rec);
            let (old, new) = (b.len() as u64, nb.len() as u64);
            rt.insert((id, seq), nb.as_slice())?;
            let at = it
                .get((id, seq))?
                .map(|v| dec_pair(v.value()))
                .transpose()?
                .map_or(rec.at_ms, |x| x.0);
            it.insert((id, seq), enc_pair(at, new).as_slice())?;
            stats.bytes = stats.bytes.saturating_sub(old) + new;
            if pending.contains(&seq) {
                stats.pending_bytes = stats.pending_bytes.saturating_sub(old) + new;
            }
        }
        st.insert(id, enc_stats(&stats).as_slice())?;
    }
    if er.is_some() {
        c.attributions += rename_rows(w, id, er)?;
    }
    // the sync table
    {
        let mut t = w.open_table(SYNC)?;
        t.remove((id, PENDING))?;
        match &p.sync {
            Some(s) => {
                t.insert((id, SYNCED), enc_sync(s).as_slice())?;
            }
            None => {
                t.remove((id, SYNCED))?;
            }
        }
    }
    meta.synced_hash = p
        .sync
        .as_ref()
        .map(|s| s.hash.clone())
        .filter(|h| !h.is_empty());
    if let Some(s) = &p.sync {
        meta.synced_seq = s.seq;
    }
    w.open_table(DOCS)?.insert(id, enc_meta(&meta).as_slice())?;
    w.open_table(DOC_KEYS)?
        .insert(id, enc_doc_key(&p.key).as_slice())?;
    Ok(c)
}

/// Participant erasure in the tables without content: change summaries, approvals and the
/// client table. Returns the rows changed.
fn rename_rows(
    w: &WriteTransaction,
    id: DocId,
    er: Option<(&str, &str)>,
) -> Result<u64, StoreError> {
    let mut n = 0;
    {
        let mut t = w.open_table(CHANGES)?;
        let rows: Vec<(Seq, Vec<u8>)> = t
            .range((id, 0)..=(id, u64::MAX))?
            .map(|kv| kv.map(|(k, v)| (k.value().1, v.value().to_vec())))
            .collect::<Result<_, _>>()?;
        for (seq, b) in rows {
            let (mut info, shape) = dec_change(seq, &b, true)?;
            if rename(&mut info.author, er) | rename_opt(&mut info.on_behalf_of, er) {
                t.insert((id, seq), enc_change(&info, &shape).as_slice())?;
                n += 1;
            }
        }
    }
    {
        let mut t = w.open_table(APPROVALS)?;
        let rows: Vec<(Seq, Vec<u8>)> = t
            .range((id, 0)..=(id, u64::MAX))?
            .map(|kv| kv.map(|(k, v)| (k.value().1, v.value().to_vec())))
            .collect::<Result<_, _>>()?;
        for (seq, b) in rows {
            let mut a = dec_approval(&b)?;
            if rename(&mut a.author, er)
                | rename(&mut a.on_behalf_of, er)
                | rename_opt(&mut a.by, er)
            {
                t.insert((id, seq), enc_approval(&a).as_slice())?;
                n += 1;
            }
        }
    }
    {
        let mut t = w.open_table(CLIENTS)?;
        let rows: Vec<(u64, String)> = t
            .range((id, 0)..=(id, u64::MAX))?
            .map(|kv| kv.map(|(k, v)| (k.value().1, v.value().to_owned())))
            .collect::<Result<_, _>>()?;
        for (client, mut pid) in rows {
            if rename(&mut pid, er) {
                t.insert((id, client), pid.as_str())?;
                n += 1;
            }
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::single_range_in_vec_init)] // one span each
    fn cut_keeps_original_coordinates_across_purges() {
        let mut red = Vec::new();
        let t0 = "aaaSECRETbbbOTHERccc";
        // first purge: SECRET (3..9)
        let t1 = cut(t0, &[3..9], "r", &mut red);
        assert_eq!(t1, "aaabbbOTHERccc");
        assert_eq!((red[0].start, red[0].len), (3, 6));
        // second purge: OTHER, at 6..11 in the cut text, 12..17 originally
        let t2 = cut(&t1, &[6..11], "r", &mut red);
        assert_eq!(t2, "aaabbbccc");
        assert_eq!(
            red.iter().map(|r| (r.start, r.len)).collect::<Vec<_>>(),
            vec![(3, 6), (12, 5)]
        );
        // a span across an earlier gap merges with it
        let t3 = cut(&t2, &[2..4], "r", &mut red);
        assert_eq!(t3, "aabbccc");
        assert_eq!(
            red.iter().map(|r| (r.start, r.len)).collect::<Vec<_>>(),
            vec![(2, 8), (12, 5)]
        );
    }
}
