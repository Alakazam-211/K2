//! When an outside save removes someone's text (v0.9.7.2; `docs/SESSIONS.md`, "When an outside
//! save removes someone's text"): the save is taken in as before, and each person whose text it
//! may have removed is told what went and which version holds it. Nothing here changes the
//! merge or the doc: [`Session::outside_removals`] only reads, before the save applies.
//!
//! **Whose text.** Authors who wrote through the kit: the elements the merge's authored hunks
//! delete, by provenance (each character's own author, not a line's most recent one), and the
//! person an assistant acted for. Text from an earlier outside save has no author to tell
//! (`kit:external`): an audit row, no note. Whoever put text back by an undo or revert is told
//! where, without the text: it isn't theirs.
//!
//! **Which text.** A non-whitespace character written since the last outside save, or within
//! [`Config::outside_note_window_ms`]. "May have": the kit can't tell a removal from a move, so
//! a moved line is reported too, and the note's words say so.
//!
//! **What the note carries.** That person's own characters and no one else's, so they can paste
//! them back, up to [`OVERWRITE_TEXT_MAX`] a note. With the history right it also names the
//! version; without it, it says who can restore what was cut.
//!
//! **Where the notes live.** In memory, per pid: [`Aware::pending`]. An unload hands them, text
//! included, to the manager for the doc's next session (agents mostly run one verb at a time,
//! so that's their normal case). A restart loses them; the version, the log and the audit row
//! remain. Bounded per note, per person ([`OVERWRITE_PENDING_MAX`], with a roll-up), per doc
//! ([`OVERWRITE_DOC_TEXT_MAX`]) and in time ([`Config::outside_note_text_keep_ms`]). Past the last
//! two the note keeps the fact and says the text is no longer held.

use std::collections::{BTreeMap, BTreeSet};

use crate::store::{VersionId, VersionInfo};

use super::blame::{pieces, Piece};
use super::*;
use crate::session::awareness::{
    cap_value, Note, NoteKind, Overwrite, OverwriteCell, OVERWRITE_CELLS_MAX,
    OVERWRITE_DOC_TEXT_MAX, OVERWRITE_PENDING_MAX, OVERWRITE_TEXT_MAX, OVERWRITE_VALUE_MAX,
};

/// One run of lines one person may have lost.
struct Place {
    root: String,
    first: usize,
    last: usize,
    yours: String,
    file: String,
    cut: bool,
    replaced: bool,
}

/// What a save may take from one person (read before it applies).
pub(super) struct Removed {
    pid: String,
    places: Vec<Place>,
    more: usize,
    /// Characters of their own text the note can't carry.
    not_shown: usize,
    /// The note's text budget left.
    left: usize,
    multi_root: bool,
}

impl Place {
    fn region(&self, multi_root: bool) -> String {
        let at = if self.first == self.last {
            format!("L{}", self.first)
        } else {
            format!("L{}–L{}", self.first, self.last)
        };
        if multi_root {
            format!("{}:{at}", self.root)
        } else {
            at
        }
    }
}

/// The byte range of the first and last non-whitespace characters of `s`, offset by `at`.
fn inked(s: &str, at: usize) -> Option<(usize, usize)> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let lead = s.len() - s.trim_start().len();
    Some((at + lead, at + lead + t.len()))
}

/// The 1-based line of byte `at`, given each line's first byte.
fn line_at(starts: &[usize], at: usize) -> usize {
    starts.partition_point(|&s| s <= at)
}

/// `s` cut to `max` bytes at a character boundary.
fn cut_to(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// One of a person's runs in the removed text: its lines, its bytes, whether it's their own
/// text (not someone else's they put back) and whether it has anything but whitespace.
struct Run {
    first: usize,
    last: usize,
    start: usize,
    end: usize,
    own: bool,
    ink: bool,
}

/// Lines `first..=last` of `text` as they'd read after `hunks`, capped, and whether a hunk there
/// inserted anything.
fn after_of(
    text: &str,
    starts: &[usize],
    hunks: &[Hunk],
    (first, last): (usize, usize),
) -> (String, bool, bool) {
    let ls = starts[first - 1];
    let le = starts.get(last).copied().unwrap_or(text.len());
    let (mut after, mut at, mut replaced) = (String::new(), ls, false);
    // the hunks are in order and don't overlap: those touching these lines
    let from = hunks.partition_point(|h| h.end <= ls && h.start < ls);
    for h in hunks[from..].iter().take_while(|h| h.start < le) {
        if h.start > at {
            after.push_str(&text[at..h.start]);
        }
        after.push_str(&h.insert);
        replaced |= h.insert.chars().any(|c| !c.is_whitespace());
        at = at.max(h.end.min(le));
    }
    if at < le {
        after.push_str(&text[at..le]);
    }
    let after = after.trim_end_matches('\n');
    let after = if after.trim().is_empty() { "" } else { after };
    let (file, cut) = cap_value(after);
    (file, cut, replaced)
}

impl Session {
    /// Who may lose what to this save, before it applies. Only reads, and never stops the
    /// save: if the store can't answer, that is an audit row (`not_computed`), and the save
    /// goes on.
    pub(super) fn outside_removals(
        &self,
        st: &State,
        edits: &[Edit],
        authored: &[(String, Vec<Hunk>)],
        sides: &[(String, Vec<Hunk>)],
        now: u64,
    ) -> Vec<Removed> {
        #[cfg(any(test, feature = "differential"))]
        let started = std::time::Instant::now();
        let out = self.outside_removals_in(st, edits, authored, sides, now);
        #[cfg(any(test, feature = "differential"))]
        crate::differential::note_removal_cost(started.elapsed());
        match out {
            Ok(v) => v,
            Err(_) => {
                self.emit(AuditEvent::OutsideOverride {
                    key: st.key.clone(),
                    change: st.version + 1,
                    rule: "not_computed",
                    region: String::new(),
                    by: None,
                    version: None,
                });
                Vec::new()
            }
        }
    }

    fn outside_removals_in(
        &self,
        st: &State,
        edits: &[Edit],
        authored: &[(String, Vec<Hunk>)],
        sides: &[(String, Vec<Hunk>)],
        now: u64,
    ) -> Result<Vec<Removed>> {
        let store = self.store(st)?.clone();
        let Some(prov) = st.prov.as_ref() else {
            return Err(SessionError::Disk("no provenance map".into()));
        };
        // per root: whose text the merge's whole lines take. Whole lines, not the characters the
        // change was trimmed to: a line deleted between two that start alike trims to pieces of
        // both, but the person lost that line, and can only paste back a whole one. Only where
        // the trimmed change takes something other than whitespace: an editor stripping trailing
        // spaces, or adding to a line, removes nobody's text.
        let mut rooted: Vec<(&str, &str, Vec<Hunk>, Vec<Piece>)> = Vec::new();
        for e in edits {
            let Edit::Text { root, hunks } = e;
            let told = authored
                .iter()
                .find(|(r, _)| r == root)
                .map_or_else(|| diff::coalesce(hunks.clone()), |(_, h)| h.clone());
            let lines = sides
                .iter()
                .find(|(r, _)| r == root)
                .map_or_else(|| told.clone(), |(_, h)| diff::coalesce(h.clone()));
            let text = self.text_of(st, root)?.1;
            let takes_ink = |h: &Hunk| {
                told.iter().any(|t| {
                    let (s, e) = (t.start.max(h.start), t.end.min(h.end));
                    s < e && !text[s..e].trim().is_empty()
                })
            };
            let mut ps = Vec::new();
            for h in lines.iter().filter(|h| h.end > h.start && takes_ink(h)) {
                let runs = prov.query(root, text, h.start, h.end);
                ps.extend(pieces(&store, self.doc_id, text, &runs, h.start)?);
            }
            rooted.push((root.as_str(), text, lines, ps));
        }
        if rooted.iter().all(|r| r.3.is_empty()) {
            return Ok(Vec::new());
        }
        let last_outside = store
            .version_infos(self.doc_id)?
            .into_iter()
            .filter(|v| v.reason == VersionReason::External)
            .map(|v| v.seq)
            .max()
            .unwrap_or(0);
        let seqs: BTreeSet<Seq> = rooted
            .iter()
            .flat_map(|r| r.3.iter())
            .flat_map(|p| [p.seq, p.restorer.and_then(|x| x.1)])
            .flatten()
            .collect();
        let infos = store.change_infos(self.doc_id, &seqs)?;
        let owners = st.owners.as_ref();
        let owner = |c: ClientId, seq: Option<Seq>| -> String {
            owners
                .and_then(|o| o.get(&c).cloned())
                .or_else(|| seq.and_then(|s| infos.get(&s)).map(|i| i.author.clone()))
                .unwrap_or_else(|| UNKNOWN_PID.to_owned())
        };
        let window = self.config.outside_note_window_ms;
        let eligible = |p: &Piece| {
            p.seq
                .into_iter()
                .chain(p.restorer.and_then(|x| x.1))
                .max()
                .is_some_and(|s| {
                    s > last_outside
                        || (window > 0
                            && infos
                                .get(&s)
                                .is_some_and(|i| now.saturating_sub(i.at_ms) <= window))
                })
        };
        let multi_root = rooted.len() > 1 || self.schema.roots.len() > 1;
        let mut out: BTreeMap<String, Removed> = BTreeMap::new();
        for (root, text, told, ps) in &rooted {
            let starts: Vec<usize> = std::iter::once(0)
                .chain(text.match_indices('\n').map(|(i, _)| i + 1))
                .collect();
            // per person, their runs in the text this save may take, in order
            let mut runs: BTreeMap<String, Vec<Run>> = BTreeMap::new();
            for p in ps.iter().filter(|p| eligible(p)) {
                let slice = &text[p.start..p.end];
                let ink = inked(slice, p.start);
                let (a, b) = ink.unwrap_or((p.start, p.end));
                let run = |own| Run {
                    first: line_at(&starts, a),
                    last: line_at(&starts, b - 1),
                    start: p.start,
                    end: p.end,
                    own,
                    ink: ink.is_some(),
                };
                let mut own = vec![owner(p.client, p.seq)];
                own.extend(p.seq.and_then(|s| infos.get(&s)?.on_behalf_of.clone()));
                let mut back = Vec::new();
                if let Some((c, seq)) = p.restorer {
                    back.push(owner(c, seq));
                    back.extend(seq.and_then(|s| infos.get(&s)?.on_behalf_of.clone()));
                }
                back.retain(|w| !own.contains(w));
                for who in own {
                    runs.entry(who).or_default().push(run(true));
                }
                for who in back {
                    runs.entry(who).or_default().push(run(false));
                }
            }
            for (pid, list) in runs {
                let e = out.entry(pid.clone()).or_insert_with(|| Removed {
                    pid,
                    places: Vec::new(),
                    more: 0,
                    not_shown: 0,
                    left: OVERWRITE_TEXT_MAX,
                    multi_root,
                });
                // places: runs on touching lines; whitespace alone never makes one
                let mut k = 0;
                while k < list.len() {
                    let (first, mut last) = (list[k].first, list[k].last);
                    let mut j = k + 1;
                    while j < list.len() && list[j].first <= last + 1 {
                        last = last.max(list[j].last);
                        j += 1;
                    }
                    let group = &list[k..j];
                    k = j;
                    if !group.iter().any(|r| r.ink) {
                        continue;
                    }
                    let own: Vec<&str> = group
                        .iter()
                        .filter(|r| r.own)
                        .map(|r| &text[r.start..r.end])
                        .collect();
                    let chars: usize = own.iter().map(|s| s.chars().count()).sum();
                    let others: usize = group
                        .iter()
                        .filter(|r| !r.own)
                        .map(|r| text[r.start..r.end].chars().count())
                        .sum();
                    if e.places.len() >= OVERWRITE_CELLS_MAX {
                        e.more += 1;
                        e.not_shown += chars + others;
                        continue;
                    }
                    // their own text first, as much as the note can carry
                    let mut yours = String::new();
                    let mut cut = false;
                    for s in own {
                        let piece = cut_to(s, e.left);
                        yours.push_str(piece);
                        e.left -= piece.len();
                        if piece.len() < s.len() {
                            cut = true;
                            e.not_shown += s[piece.len()..].chars().count();
                        }
                    }
                    e.not_shown += others;
                    let (file, fc, replaced) = after_of(text, &starts, told, (first, last));
                    let file = cut_to(&file, e.left.min(OVERWRITE_VALUE_MAX)).to_owned();
                    e.left -= file.len();
                    e.places.push(Place {
                        root: (*root).to_owned(),
                        first,
                        last,
                        yours,
                        file,
                        cut: cut || fc,
                        replaced,
                    });
                }
            }
        }
        Ok(out.into_values().collect())
    }

    /// After the save committed: the audit rows, the waiting notes, the push to each author's
    /// own subscriptions. `before` is the doc's version just before the save (a version was
    /// cut there, or already was).
    pub(super) fn finish_removals(
        &self,
        st: &mut State,
        removed: Vec<Removed>,
        before: Seq,
        change: Seq,
        now: u64,
    ) {
        if removed.is_empty() {
            return;
        }
        let version = self.store(st).ok().and_then(|s| self.version_at(s, before));
        #[cfg(any(test, feature = "differential"))]
        let mutant = crate::differential::note_mutant();
        #[cfg(any(test, feature = "differential"))]
        let first_named = removed
            .iter()
            .find(|r| !r.pid.starts_with(RESERVED_PID_PREFIX))
            .map(|r| r.pid.clone());
        for r in removed {
            let Some(first) = r.places.first() else {
                continue;
            };
            let region = first.region(r.multi_root);
            let others = r.places.len() - 1 + r.more;
            let rule = if r.places.iter().all(|p| p.replaced) {
                "replaced_text"
            } else {
                "removed_text"
            };
            self.emit(AuditEvent::OutsideOverride {
                key: st.key.clone(),
                change,
                rule,
                region: if others == 0 {
                    region.clone()
                } else {
                    format!("{region} and {others} more")
                },
                by: Some(r.pid.clone()),
                version,
            });
            if r.pid.starts_with(RESERVED_PID_PREFIX) {
                continue;
            }
            let o = Overwrite {
                rule,
                source: "an outside save",
                region,
                row: None,
                line: Some(first.first),
                col: None,
                yours: first.yours.clone(),
                file: first.file.clone(),
                cut: first.cut,
                change,
                version,
                restore_from: version,
                also: r
                    .places
                    .iter()
                    .skip(1)
                    .map(|p| OverwriteCell {
                        region: p.region(r.multi_root),
                        col: None,
                        yours: p.yours.clone(),
                        file: p.file.clone(),
                        cut: p.cut,
                    })
                    .collect(),
                more: r.more,
                not_shown: r.not_shown,
                withheld: false,
                no_history: false,
                before,
                at_ms: now,
            };
            let lines = LineRange {
                root: first.root.clone(),
                first: first.first,
                last: first.last,
            };
            #[allow(unused_mut)]
            let mut pid = r.pid;
            #[cfg(any(test, feature = "differential"))]
            match mutant {
                Some(crate::differential::NoteMutant::DropAll) => continue,
                Some(crate::differential::NoteMutant::WrongAuthor) => {
                    pid = first_named.clone().unwrap_or(pid);
                }
                _ => {}
            }
            let q = st.aware.pending.entry(pid.clone()).or_default();
            q.retain(|n| {
                n.overwrite.as_ref().map_or(true, |x| {
                    now.saturating_sub(x.at_ms) <= self.config.revert_keep_ms
                })
            });
            queue_overwrite(q, o, lines);
            if let Some(o) = q.last().and_then(|n| n.overwrite.clone()) {
                st.roster.record(
                    RosterEvent::Overwritten {
                        to: Who::of(&pid),
                        overwrite: o,
                    },
                    self.config.roster_queue,
                );
            }
        }
        bound_doc_text(&mut st.aware.pending);
    }

    /// The version holding the text as it was at `before`: exactly that version, never an
    /// older one (which might not hold it).
    fn version_at(&self, store: &DocStore, before: Seq) -> Option<VersionId> {
        store
            .version_infos(self.doc_id)
            .ok()?
            .into_iter()
            .filter(|v| v.seq == before)
            .map(|v| v.id)
            .max()
    }

    /// `o` as `rights` may have it at `now`: the version named only if they can open it and it
    /// is still there; the text only inside its hold.
    fn note_for_reader(
        &self,
        o: &mut Overwrite,
        rights: &Rights,
        versions: &[VersionInfo],
        now: u64,
    ) {
        let before = o.before;
        if !o
            .restore_from
            .is_some_and(|v| versions.iter().any(|x| x.id == v && x.seq == before))
        {
            o.restore_from = None;
        }
        o.no_history = !rights.history;
        o.version = if rights.history { o.restore_from } else { None };
        if !o.withheld && now.saturating_sub(o.at_ms) > self.config.outside_note_text_keep_ms {
            o.strip();
        }
    }

    /// The notes waiting for member `i`, delivered: gone from the queue. Someone who can't read
    /// the doc now gets nothing, not even a count; theirs stay and arrive if they can read
    /// again.
    pub(super) fn drain_notes(&self, st: &mut State, i: usize, now: u64) -> Vec<Note> {
        let pid = st.members[i].p.pid.clone();
        if !st.aware.pending.contains_key(&pid) {
            return Vec::new();
        }
        let rights = self.host.rights(&st.members[i].p, &st.key);
        if !rights.read {
            return Vec::new();
        }
        let mut q = st.aware.pending.remove(&pid).unwrap_or_default();
        let keep = self.config.revert_keep_ms;
        q.retain(|n| {
            n.overwrite
                .as_ref()
                .map_or(true, |o| now.saturating_sub(o.at_ms) <= keep)
        });
        let versions = self
            .store(st)
            .ok()
            .and_then(|s| s.version_infos(self.doc_id).ok())
            .unwrap_or_default();
        for o in q.iter_mut().filter_map(|n| n.overwrite.as_mut()) {
            self.note_for_reader(o, &rights, &versions, now);
        }
        q
    }

    /// The notes waiting for member `conn`, delivered: a host's way to collect them for someone
    /// it showed the push to, or who runs no verb. Needs read.
    pub fn take_notes(&self, conn: ConnId) -> Result<Notes> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::Read, now)?;
        let waiting = self.drain_notes(st, i, now);
        let notes = match Self::app_viewer(st, i) {
            Some(v) => {
                let mut l = st.handles.lens(&v);
                waiting.into_iter().map(|n| l.note(n)).collect()
            }
            None => waiting,
        };
        Ok(Notes {
            notes,
            ..Notes::default()
        })
    }

    /// Each member's waiting notes for a snapshot, as they may read them. Someone who can't
    /// read the doc now has none here.
    pub(super) fn waiting_for_subscribers(
        &self,
        st: &State,
        now: u64,
    ) -> HashMap<String, Vec<Overwrite>> {
        if st.aware.pending.is_empty() {
            return HashMap::new();
        }
        let versions = self
            .store(st)
            .ok()
            .and_then(|s| s.version_infos(self.doc_id).ok())
            .unwrap_or_default();
        let keep = self.config.revert_keep_ms;
        let mut out = HashMap::new();
        for (pid, q) in &st.aware.pending {
            let Some(m) = st.members.iter().find(|m| m.p.pid == *pid) else {
                continue;
            };
            let rights = self.host.rights(&m.p, &st.key);
            if !rights.read {
                continue;
            }
            let list: Vec<Overwrite> = q
                .iter()
                .filter_map(|n| n.overwrite.clone())
                .filter(|o| now.saturating_sub(o.at_ms) <= keep)
                .map(|mut o| {
                    self.note_for_reader(&mut o, &rights, &versions, now);
                    o
                })
                .collect();
            out.insert(pid.clone(), list);
        }
        out
    }

    /// Before a flush: the recorded pushes as their readers may have them. Nothing for someone
    /// who can't read the doc now, no version for someone who can't read history.
    pub(super) fn notes_at_flush(&self, st: &mut State) {
        let st = &mut *st;
        let any = st
            .roster
            .pending_mut()
            .iter()
            .any(|e| matches!(e, RosterEvent::Overwritten { .. }));
        if !any {
            return;
        }
        let rights: Vec<(Who, Rights)> = st
            .members
            .iter()
            .map(|m| (Who::of(&m.p.pid), self.host.rights(&m.p, &st.key)))
            .collect();
        st.roster.pending_mut().retain_mut(|e| {
            let RosterEvent::Overwritten { to, overwrite } = e else {
                return true;
            };
            match rights.iter().find(|(w, _)| w == to) {
                Some((_, r)) if !r.read => false,
                Some((_, r)) if !r.history => {
                    overwrite.version = None;
                    overwrite.no_history = true;
                    true
                }
                _ => true,
            }
        });
    }

    /// A purge: every waiting note, and any push not yet sent, loses its values and its version.
    pub(super) fn withhold_notes(&self, st: &mut State) {
        for o in st
            .aware
            .pending
            .values_mut()
            .flatten()
            .filter_map(|n| n.overwrite.as_mut())
        {
            o.withhold();
        }
        for e in st.roster.pending_mut().iter_mut() {
            if let RosterEvent::Overwritten { overwrite, .. } = e {
                overwrite.withhold();
            }
        }
    }

    /// The notes the previous session of this doc left waiting (the manager's hand-off).
    pub(crate) fn adopt_notes(&self, notes: HashMap<String, Vec<Note>>) {
        let now = self.host.now_ms();
        let keep = self.config.revert_keep_ms;
        let Ok(mut st) = self.lock_any() else {
            return;
        };
        for (pid, mut q) in notes {
            q.retain(|n| {
                n.overwrite
                    .as_ref()
                    .map_or(true, |o| now.saturating_sub(o.at_ms) <= keep)
            });
            if !q.is_empty() {
                st.aware.pending.insert(pid, q);
            }
        }
    }
}

/// The doc's total text cap: past [`OVERWRITE_DOC_TEXT_MAX`], the oldest notes with text lose it
/// (the fact stays, and says the text is no longer held).
pub(crate) fn bound_doc_text(pending: &mut HashMap<String, Vec<Note>>) {
    let mut held: Vec<(u64, usize)> = Vec::new();
    let mut total = 0usize;
    for o in pending
        .values()
        .flatten()
        .filter_map(|n| n.overwrite.as_ref())
    {
        let b = o.text_bytes();
        total += b;
        if b > 0 {
            held.push((o.at_ms, b));
        }
    }
    if total <= OVERWRITE_DOC_TEXT_MAX {
        return;
    }
    // the at_ms below which text goes, oldest first
    held.sort_unstable();
    let mut cutoff = None;
    for (at, b) in held {
        if total <= OVERWRITE_DOC_TEXT_MAX {
            break;
        }
        total -= b;
        cutoff = Some(at);
    }
    let Some(cutoff) = cutoff else {
        return;
    };
    for o in pending
        .values_mut()
        .flatten()
        .filter_map(|n| n.overwrite.as_mut())
        .filter(|o| o.at_ms <= cutoff)
    {
        o.strip();
    }
}

/// `o` queued in `q`, at most [`OVERWRITE_PENDING_MAX`] notes: past it the last is a roll-up
/// counting the rest. Never dropped silently.
fn queue_overwrite(q: &mut Vec<Note>, o: Overwrite, lines: LineRange) {
    let note = |o: Overwrite, lines: LineRange| Note {
        pid: EXTERNAL_PID.to_owned(),
        name: EXTERNAL_PID.to_owned(),
        kind: NoteKind::Overwritten,
        participant_kind: ParticipantKind::Person,
        lines,
        age_ms: 0,
        claim: None,
        claim_note: None,
        remaining_ms: None,
        overwrite: Some(o),
    };
    if q.len() < OVERWRITE_PENDING_MAX {
        q.push(note(o, lines));
        return;
    }
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::note_mutant() == Some(crate::differential::NoteMutant::DropRollup) {
        return;
    }
    let cells = o.cells();
    if let Some(r) = q
        .last_mut()
        .and_then(|n| n.overwrite.as_mut())
        .filter(|r| r.is_rollup())
    {
        r.more += cells;
        r.change = o.change;
        r.before = o.before;
        r.version = o.version;
        r.restore_from = o.restore_from;
        r.at_ms = o.at_ms;
        return;
    }
    // the last note becomes part of the roll-up too, to keep the bound
    let old = q.pop().and_then(|n| n.overwrite);
    let more = cells + old.as_ref().map_or(0, Overwrite::cells);
    let mut r = o;
    r.rule = "rollup";
    r.region.clear();
    r.row = None;
    r.line = None;
    r.col = None;
    r.yours.clear();
    r.file.clear();
    r.cut = false;
    r.also.clear();
    r.more = more;
    r.not_shown = 0;
    q.push(note(r, lines));
}
