//! The live-edit rebase (E-LIVE, `edit_at_base`; kit v0.9.7): carry a text change made at an
//! older version through the changes applied since, positionally, with deterministic
//! tie-breaks. The server ([`crate::session::Session::edit_at_base`]) and the JS client
//! (`client/`, K1) apply the same rules, so both compute the same text. Pure functions over
//! [`Hunk`]s (canonical, UTF-8 byte offsets); shared test vectors pin them for the client.
//!
//! **Roles.** A change is carried through another one that was applied first:
//! - [`rebase_ours`] (the server, and the client for its own pending change): ours was made
//!   later, so at a tie ours goes **after** theirs (rule 3: the change the server applied
//!   first stays on the left). A conflict (rule 4 (b)) refuses ours.
//! - [`rebase_incoming`] (the client mirror): an incoming server change carried through the
//!   client's own pending change. The incoming one was applied first on the server, so at a
//!   tie it goes **before** (Bias::Before, "left"). Never refuses (a pending change that
//!   conflicts is refused by the server, and the client rolls it back).
//!
//! **The rules** (amendment E-LIVE r2):
//! - two inserts at one point: the first applied stays left (rule 3);
//! - deletion vs deletion: no conflict; what the other already deleted is a no-op (4 (a));
//! - a conflict only when ours' **replacement** (a delete plus an insert) intersects a range
//!   the other deleted (or replaced), or ours **inserts strictly inside** a range the other
//!   deleted (4 (b)), under **both** tie-break placements (r2.3 S3: wherever a tie put it
//!   among others' insertions at its point; an insertion at either edge never conflicts);
//! - others' inserted text is never deleted: a deletion over it is split around it, and a
//!   replacement's text goes at the **left end** of its original range, before the
//!   surviving insertions (rule 5).
//!
//! Positions of surviving characters move as [`super::position::transform`] moves them; the
//! points and ranges of a change move by [`map_point`] and [`map_range`], defined from it.
//!
//! **Each hunk is carried as sent (r2.3; r2.2's rule (1) withdrawn).** A change keeps its
//! hunks' identities through every step ([`Carried`]): an insertion that comes to touch a
//! deletion of the same change stays an insertion, and the pieces of a replacement that
//! rule 5 split stay that replacement's. Rule 4 judges each hunk by what it was when sent,
//! so a deletion of text someone else replaced never bounces because ours' own insertion came
//! to touch it, and a split replacement still competes with a replacement of its split-off
//! part. Only the result is made canonical, to be applied. The server and the client mirror
//! must both carry this way: re-canonicalizing on one side only makes them disagree.

use crate::text::diff::{self, Hunk};

/// Where a rebase found competing text (rule 4 (b)): our hunk's range (in the coordinates
/// of the text before the change it met), and the other change's hunk it met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveConflict {
    /// Which of ours: its index among our hunks as sent (canonical; r2.3: hunks keep their
    /// identities through every step).
    pub ours: usize,
    /// Our hunk's deleted range (or insertion point, `start == end`) where it met theirs, in
    /// the coordinates of the text before theirs, and the text it inserts.
    pub start: usize,
    pub end: usize,
    pub insert: String,
    /// Theirs: the step (index of the change, from 0) and the hunk's range.
    pub step: usize,
    pub theirs_start: usize,
    pub theirs_end: usize,
    /// `replacement` (4 (b) i) or `insert_inside_deleted` (4 (b) ii).
    pub why: &'static str,
}

/// One of our hunks while it is carried: its insertion point (the left end of its original
/// range) and text, and what is left of its deleted range, as pieces.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Piece {
    at: usize,
    /// Where its insertion would be with the other tie-break (`Bias::Left`, before others'
    /// insertions at its point), carried the same way (r2.3 S3); `at` is its placement.
    lo: usize,
    insert: String,
    dels: Vec<(usize, usize)>,
    /// Both a deletion and an insertion originally (rule 4 (b) i).
    replacement: bool,
    /// Its index in the hunks given.
    index: usize,
}

/// The planted faults (`crate::differential::LiveMutant`): never compiled into a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mutant {
    TieLeftForOurs,
    DeleteSwallowsInsert,
    O5ForLive,
    ReplacementTextPlacedRight,
    DeleteDeleteConflicts,
    RecanonServerOnly,
    RecanonBothSides,
    InsideByTieBreak,
}

#[cfg(any(test, feature = "differential"))]
fn mutant() -> Option<Mutant> {
    use crate::differential::LiveMutant as L;
    crate::differential::live_mutant().and_then(|m| match m {
        L::TieLeftForOurs => Some(Mutant::TieLeftForOurs),
        L::DeleteSwallowsInsert => Some(Mutant::DeleteSwallowsInsert),
        L::O5ForLive => Some(Mutant::O5ForLive),
        L::ReplacementTextPlacedRight => Some(Mutant::ReplacementTextPlacedRight),
        L::DeleteDeleteConflicts => Some(Mutant::DeleteDeleteConflicts),
        L::RecanonServerOnly => Some(Mutant::RecanonServerOnly),
        L::RecanonBothSides => Some(Mutant::RecanonBothSides),
        L::InsideByTieBreak => Some(Mutant::InsideByTieBreak),
        // the client model's and the window's, not the rebase's
        L::OwnChangeInWindow | L::WindowHoldsExpandedHunks => None,
    })
}

#[cfg(not(any(test, feature = "differential")))]
#[inline(always)]
fn mutant() -> Option<Mutant> {
    None
}

/// Where an insertion point `at` (of the text before `theirs`) goes when `theirs` (canonical)
/// is applied. `after`: at a tie (theirs inserts at `at`, or replaces a range starting at
/// `at`), ours goes after theirs' inserted text; else before it. A point strictly inside a
/// range theirs deleted goes after theirs' inserted text there (the server puts a
/// replacement's text at the left end, before what others inserted: rule 5).
pub fn map_point(at: usize, theirs: &[Hunk], after: bool) -> usize {
    let mut delta: isize = 0;
    let shift = |p: usize, d: isize| (p as isize + d) as usize;
    for t in theirs {
        let (ts, te, j) = (t.start, t.end, t.insert.len());
        if at < ts {
            break;
        }
        if ts == te {
            // a pure insertion at `at`: the tie
            if at == ts && !after {
                break;
            }
            delta += j as isize;
            continue;
        }
        if at == ts {
            return if after {
                shift(ts, delta) + j
            } else {
                shift(ts, delta)
            };
        }
        if at < te {
            return shift(ts, delta) + j;
        }
        delta += j as isize - (te - ts) as isize;
    }
    shift(at, delta)
}

/// What survives of the deleted range `[a, b)` (of the text before `theirs`) once `theirs`
/// is applied: the characters theirs didn't delete, as ranges of the text after it, split
/// around every insertion of theirs, which they never cover (rule 5; 4 (a): a part theirs
/// already deleted is gone).
pub fn map_range(a: usize, b: usize, theirs: &[Hunk]) -> Vec<(usize, usize)> {
    let shift = |p: usize, d: isize| (p as isize + d) as usize;
    let swallow = mutant() == Some(Mutant::DeleteSwallowsInsert);
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut delta: isize = 0;
    // the next character of [a, b) not yet placed, and (planted only) the start of a piece
    // kept open across an insertion
    let mut cur = a;
    let mut open: Option<usize> = None;
    for t in theirs {
        let (ts, te) = (t.start, t.end);
        let grow = t.insert.len() as isize - (te - ts) as isize;
        if te <= cur && (ts < cur || ts == te) {
            // wholly before what's left (an insertion at `cur` lands before it)
            delta += grow;
            continue;
        }
        if ts >= b {
            break;
        }
        if swallow && ts == te {
            // planted: the deletion runs on over the insertion
            open.get_or_insert(shift(cur, delta));
            delta += grow;
            continue;
        }
        // theirs meets [cur, b): the part before it survives; its deletion is gone, its
        // insertion stays out of ours
        let start = open.take().unwrap_or(shift(cur, delta));
        if ts > cur || start < shift(cur, delta) {
            out.push((start, shift(ts.max(cur), delta)));
        }
        delta += grow;
        cur = cur.max(te);
    }
    if cur < b || open.is_some() {
        let start = open.take().unwrap_or(shift(cur, delta));
        out.push((start, shift(b.max(cur), delta)));
    }
    out.retain(|(s, e)| s < e);
    // adjacent pieces (no insertion between them) are one
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in out {
        match merged.last_mut() {
            Some(l) if l.1 == s => l.1 = e,
            _ => merged.push((s, e)),
        }
    }
    merged
}

fn pieces(hunks: &[Hunk]) -> Vec<Piece> {
    hunks
        .iter()
        .enumerate()
        .map(|(index, h)| Piece {
            at: h.start,
            lo: h.start,
            insert: h.insert.clone(),
            dels: if h.start < h.end {
                vec![(h.start, h.end)]
            } else {
                Vec::new()
            },
            replacement: h.start < h.end && !h.insert.is_empty(),
            index,
        })
        .collect()
}

/// Back to canonical hunks: each piece's insertion at its point (joined with a deletion
/// starting there), its other deletions as their own hunks; sorted, touching hunks joined.
fn hunks_of(ps: Vec<Piece>) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::new();
    for p in ps {
        let mut dels = p.dels.into_iter().peekable();
        if !p.insert.is_empty() {
            let end = match dels.peek() {
                Some(&(s, e)) if s == p.at => {
                    dels.next();
                    e
                }
                _ => p.at,
            };
            out.push(Hunk {
                start: p.at,
                end,
                insert: p.insert,
            });
        }
        for (s, e) in dels {
            out.push(Hunk {
                start: s,
                end: e,
                insert: String::new(),
            });
        }
    }
    // stable: pieces are in order, and a piece's own hunks are; ties keep that order
    out.sort_by_key(|h| (h.start, h.end > h.start));
    diff::coalesce(out)
}

/// Point `x` (of the text before `theirs`) is strictly inside a range `theirs` deletes: the
/// characters on both sides of it are deleted.
fn inside_deleted(x: usize, theirs: &[Hunk]) -> bool {
    theirs.iter().any(|t| t.start < x && x < t.end)
}

/// One step: carry `ps` (in the coordinates before `theirs`) through `theirs`. `after`: the
/// server's role (ours later); `check`: refuse per rule 4 (b).
fn carry(
    ps: Vec<Piece>,
    theirs: &[Hunk],
    after: bool,
    check: Option<usize>,
) -> Result<Vec<Piece>, LiveConflict> {
    let m = mutant();
    let mut out = Vec::with_capacity(ps.len());
    for p in ps {
        if let Some(step) = check {
            for t in theirs {
                let (ts, te) = (t.start, t.end);
                let conflict = |start, end, why| LiveConflict {
                    ours: p.index,
                    start,
                    end,
                    insert: p.insert.clone(),
                    step,
                    theirs_start: ts,
                    theirs_end: te,
                    why,
                };
                let deleting = p.dels.iter().find(|(s, e)| s.max(&ts) < e.min(&te));
                if ts < te {
                    if p.replacement {
                        if let Some(&(s, e)) = deleting {
                            return Err(conflict(s, e, "replacement"));
                        }
                    } else if p.dels.is_empty()
                        && !p.insert.is_empty()
                        && ts < p.at
                        && p.at < te
                        && (inside_deleted(p.lo, theirs)
                            || mutant() == Some(Mutant::InsideByTieBreak))
                    {
                        // r2.3 S3: strictly inside under both tie-break placements
                        return Err(conflict(p.at, p.at, "insert_inside_deleted"));
                    }
                    if m == Some(Mutant::DeleteDeleteConflicts) && t.insert.is_empty() {
                        if let Some(&(s, e)) = deleting {
                            return Err(conflict(s, e, "planted: deletion vs deletion"));
                        }
                    }
                }
                if m == Some(Mutant::O5ForLive)
                    && ts == te
                    && !t.insert.is_empty()
                    && !p.insert.is_empty()
                    && p.at == ts
                {
                    return Err(conflict(p.at, p.at, "planted: inserts at one point"));
                }
            }
        }
        let at_after = if m == Some(Mutant::TieLeftForOurs) && check.is_some() {
            false
        } else {
            after
        };
        let mut at = map_point(p.at, theirs, at_after);
        let lo = map_point(p.lo, theirs, false).min(at);
        let mut dels: Vec<(usize, usize)> = Vec::new();
        for &(s, e) in &p.dels {
            dels.extend(map_range(s, e, theirs));
        }
        if m == Some(Mutant::ReplacementTextPlacedRight)
            && p.replacement
            && check.is_some()
            && dels.len() > 1
        {
            // planted: the replacement's text after the surviving insertions
            at = dels.last().expect("two or more").0;
        }
        out.push(Piece { at, lo, dels, ..p });
    }
    Ok(out)
}

/// The gap form: each piece's insertion at its point and its deletions, as hunks in order,
/// not joined (an insertion at the end of another piece's deletion stays after it).
fn gaps_of(ps: &[Piece]) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::new();
    for p in ps {
        if !p.insert.is_empty() {
            out.push(Hunk {
                start: p.at,
                end: p.at,
                insert: p.insert.clone(),
            });
        }
        for &(s, e) in &p.dels {
            out.push(Hunk {
                start: s,
                end: e,
                insert: String::new(),
            });
        }
    }
    // stable: at one point, insertions in the pieces' order, before a deletion from there
    out.sort_by_key(|h| (h.start, h.end > h.start));
    // insertions at one point are one (a valid hunk set; the same for carrying through)
    let mut joined: Vec<Hunk> = Vec::with_capacity(out.len());
    for h in out {
        match joined.last_mut() {
            Some(l) if l.start == l.end && h.start == h.end && l.start == h.start => {
                l.insert.push_str(&h.insert)
            }
            _ => joined.push(h),
        }
    }
    joined
}

/// **A change while it is carried** (E-LIVE r2.3): its hunks as sent, each keeping its
/// identity through every step (module docs). The server carries ours through the changes
/// since its base ([`rebase_ours`]); the client carries its own pending change through each
/// incoming one as it arrives ([`Carried::through`], the same steps, so the same result) and
/// an incoming change through its pending ones ([`Carried::mirrored`], against their
/// [`Carried::gaps`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Carried {
    ps: Vec<Piece>,
}

impl Carried {
    /// A change as sent: `hunks` made canonical (sorted, touching hunks joined). Hunk `k` of
    /// that is "ours `k`" in a [`LiveConflict`].
    pub fn new(hunks: &[Hunk]) -> Self {
        Carried {
            ps: pieces(&diff::coalesce(hunks.to_vec())),
        }
    }

    /// One step in the server's role (ours was made later): carried through `theirs` (a
    /// change applied first, in the coordinates of the text before it), at a tie after it;
    /// competing text refuses (rule 4 (b)), reported at `step`.
    pub fn through(&self, theirs: &[Hunk], step: usize) -> Result<Carried, LiveConflict> {
        let ps = carry(self.ps.clone(), theirs, true, Some(step))?;
        if mutant() == Some(Mutant::RecanonBothSides) {
            // planted: r2.2's rule (1), on every side
            return Ok(Carried::new(&hunks_of(ps)));
        }
        Ok(Carried { ps })
    }

    /// The client mirror's role (rule 3): this incoming change carried through the client's
    /// own `pending` change (the server applies it later; its [`Carried::gaps`]), at a tie
    /// before it. Never refuses.
    pub fn mirrored(&self, pending: &[Hunk]) -> Carried {
        Carried {
            ps: carry(self.ps.clone(), pending, false, None).expect("the mirror never refuses"),
        }
    }

    /// The change as applied: canonical hunks, in the coordinates of the text after the
    /// last step.
    pub fn hunks(&self) -> Vec<Hunk> {
        hunks_of(self.ps.clone())
    }

    /// The change in gap form, for carrying other changes through it: the same text as
    /// [`Carried::hunks`], but each piece's insertion at its own point, not joined with a
    /// deletion it touches (a valid hunk set: sorted, insertions at one point joined).
    pub fn gaps(&self) -> Vec<Hunk> {
        gaps_of(&self.ps)
    }

    /// It changes nothing (every deletion already done, nothing to insert).
    pub fn is_empty(&self) -> bool {
        self.ps
            .iter()
            .all(|p| p.insert.is_empty() && p.dels.is_empty())
    }
}

/// **The server's rebase** (rules 2–5): carry `ours` (canonical hunks in the coordinates of
/// the base version's text) through `since` (each change applied after the base, in server
/// order, each canonical in its own pre-change coordinates). At a tie ours goes after;
/// competing text refuses (rule 4 (b)), with where. Each hunk is carried as sent (r2.3), and
/// overlap is checked stepwise (rule 4); the result is canonical, in the coordinates of the
/// text after the last change. A client carrying its pending change one step at a time
/// ([`Carried::through`]) computes the same.
pub fn rebase_ours(ours: &[Hunk], since: &[Vec<Hunk>]) -> Result<Vec<Hunk>, LiveConflict> {
    let server_only = mutant() == Some(Mutant::RecanonServerOnly);
    let mut c = Carried::new(ours);
    for (step, theirs) in since.iter().enumerate() {
        c = c.through(theirs, step)?;
        if server_only {
            // planted: r2.2's rule (1) on the server only
            c = Carried::new(&c.hunks());
        }
    }
    Ok(c.hunks())
}

/// **The client mirror** (rule 3): carry an `incoming` change (canonical, in the coordinates
/// before the client's own `pending` change) through `pending`, which the client applied
/// locally first but the server will apply after: at a tie the incoming one goes before
/// (left). Never refuses. The result is canonical, in the coordinates of the client's text
/// with `pending` applied. A pending change being carried is passed in gap form
/// ([`Carried::gaps`]).
pub fn rebase_incoming(incoming: &[Hunk], pending: &[Hunk]) -> Vec<Hunk> {
    Carried::new(incoming).mirrored(pending).hunks()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(start: usize, end: usize, insert: &str) -> Hunk {
        Hunk {
            start,
            end,
            insert: insert.into(),
        }
    }

    fn apply(t: &str, hs: &[Hunk]) -> String {
        diff::validate(t, hs).unwrap();
        diff::apply(t, hs)
    }

    /// Server: theirs applied first, ours rebased. Client (who sent ours): its text with ours
    /// applied, theirs carried through ours. Both must read the same.
    fn both(base: &str, ours: &[Hunk], theirs: &[Hunk]) -> String {
        let server = apply(
            &apply(base, theirs),
            &rebase_ours(ours, &[theirs.to_vec()]).unwrap(),
        );
        let client = apply(&apply(base, ours), &rebase_incoming(theirs, ours));
        assert_eq!(server, client, "server and client mirror differ");
        server
    }

    /// Rule 3: two inserts at one point: the one applied first (theirs) stays left.
    #[test]
    fn tie_server_first_stays_left() {
        assert_eq!(both("ab", &[h(1, 1, "X")], &[h(1, 1, "Y")]), "aYXb");
        // the same at the start and the end of the text
        assert_eq!(both("ab", &[h(0, 0, "X")], &[h(0, 0, "Y")]), "YXab");
        assert_eq!(both("ab", &[h(2, 2, "X")], &[h(2, 2, "Y")]), "abYX");
        // an insert at the start of a range theirs replaced goes after their text
        assert_eq!(both("abcd", &[h(1, 1, "X")], &[h(1, 3, "Y")]), "aYXd");
        // ... and at its end, after it too
        assert_eq!(both("abcd", &[h(3, 3, "X")], &[h(1, 3, "Y")]), "aYXd");
    }

    /// Rule 4 (a): deletion vs deletion never conflicts; the overlap is a no-op.
    #[test]
    fn delete_delete_is_idempotent() {
        assert_eq!(both("abcdef", &[h(1, 4, "")], &[h(2, 5, "")]), "af");
        // the same character twice (two backspaces)
        assert_eq!(both("abc", &[h(1, 2, "")], &[h(1, 2, "")]), "ac");
        // a shared select-all-and-delete
        assert_eq!(both("abc", &[h(0, 3, "")], &[h(0, 3, "")]), "");
        // ours' pure deletion over theirs' replacement: what theirs deleted is gone, its
        // text stays
        assert_eq!(both("abcdef", &[h(1, 5, "")], &[h(2, 4, "XY")]), "aXYf");
    }

    /// Rule 4 (b) i: ours' replacement meeting a range theirs deleted or replaced refuses.
    #[test]
    fn competing_replacement_conflicts() {
        let e = rebase_ours(&[h(1, 4, "X")], &[vec![h(2, 5, "")]]).unwrap_err();
        assert_eq!((e.why, e.start, e.end), ("replacement", 1, 4));
        let e = rebase_ours(&[h(1, 4, "X")], &[vec![h(3, 3, ""), h(3, 6, "Y")]]);
        assert!(e.is_err());
        // touching is not intersecting
        assert_eq!(both("abcdef", &[h(1, 3, "X")], &[h(3, 5, "Y")]), "aXYf");
        assert_eq!(both("abcdef", &[h(3, 5, "X")], &[h(1, 3, "Y")]), "aYXf");
    }

    /// Rule 4 (b) ii: ours inserting strictly inside a range theirs deleted refuses; at its
    /// edges it doesn't.
    #[test]
    fn insert_into_deleted_context_conflicts() {
        let e = rebase_ours(&[h(2, 2, "X")], &[vec![h(1, 4, "")]]).unwrap_err();
        assert_eq!((e.why, e.start, e.step), ("insert_inside_deleted", 2, 0));
        assert_eq!(both("abcde", &[h(1, 1, "X")], &[h(1, 4, "")]), "aXe");
        assert_eq!(both("abcde", &[h(4, 4, "X")], &[h(1, 4, "")]), "aXe");
    }

    /// Rule 5: others' inserted text is never deleted: a deletion over it is split around it.
    #[test]
    fn deletion_splits_around_others_inserts() {
        assert_eq!(both("abcdef", &[h(1, 5, "")], &[h(3, 3, "XY")]), "aXYf");
        let r = rebase_ours(&[h(1, 5, "")], &[vec![h(3, 3, "XY")]]).unwrap();
        assert_eq!(r, vec![h(1, 3, ""), h(5, 7, "")]);
    }

    /// Rule 5: a replacement over others' insertion puts its text at the left end of its
    /// original range, before the surviving insertion.
    #[test]
    fn replacement_text_at_the_left_end() {
        assert_eq!(both("abcdef", &[h(1, 5, "Z")], &[h(3, 3, "XY")]), "aZXYf");
        let r = rebase_ours(&[h(1, 5, "Z")], &[vec![h(3, 3, "XY")]]).unwrap();
        assert_eq!(r, vec![h(1, 3, "Z"), h(5, 7, "")]);
    }

    /// Rule 2: ours is carried through every change since its base, in order, and each
    /// step's conflict is judged in that step's coordinates.
    #[test]
    fn stepwise_through_several_changes() {
        let base = "hello world";
        let c1 = vec![h(0, 0, ">> ")];
        let c2 = vec![h(9, 14, "there")]; // "world" -> "there", in c1's after-coordinates
        let ours = vec![h(5, 5, ",")];
        let r = rebase_ours(&ours, &[c1.clone(), c2.clone()]).unwrap();
        let t = apply(&apply(&apply(base, &c1), &c2), &r);
        assert_eq!(t, ">> hello, there");
        // a replacement that met step 2's replaced range: refused at step 1 (0-based)
        let e = rebase_ours(&[h(6, 11, "all")], &[c1, c2]).unwrap_err();
        assert_eq!((e.step, e.why), (1, "replacement"));
    }

    /// Multi-byte text: positions are UTF-8 byte offsets on character boundaries.
    #[test]
    fn utf8_offsets() {
        // "é" is 2 bytes, "日" 3
        assert_eq!(both("aé日b", &[h(1, 1, "X")], &[h(3, 6, "Y")]), "aXéYb");
        assert_eq!(both("aé日b", &[h(1, 6, "")], &[h(3, 3, "Z")]), "aZb");
    }

    /// r2.3, K2 Engine's P2 counterexample to r2.2's rule (1): ours types `C` after `a` and
    /// deletes the separator, in one change; others replace `b`, then the separator. Carried
    /// past the first, `C` touches ours' own deletion; as sent it is still an insertion, so
    /// only the deletion meets the second replacement, and deleting what another replaced is
    /// no conflict (rule 4 (a), S5).
    #[test]
    fn r23_insertion_touching_own_deletion_stays_an_insertion() {
        let ours = [h(1, 1, "C"), h(2, 3, "")];
        let since = [vec![h(1, 2, "A")], vec![h(2, 3, "B")]];
        let r = rebase_ours(&ours, &since).unwrap();
        let t = apply(&apply(&apply("ab ", &since[0]), &since[1]), &r);
        assert_eq!(t, "aABC");
        // the server as it was (rule (1)) refused it
        let recanon = crate::differential::with_live_mutant(
            crate::differential::LiveMutant::RecanonBothSides,
            || rebase_ours(&ours, &since),
        );
        assert!(recanon.is_err(), "{recanon:?}");
    }

    /// r2.3, K2 Engine's P2c counterexample: ours replaces `ab`; another types `C` inside it,
    /// then replaces `b`. Rule 5 splits ours around `C`; the piece over `b` is still ours'
    /// replacement, so it competes with theirs (rule 4 (b) i) and is refused at step 1.
    #[test]
    fn r23_split_replacement_still_competes() {
        let ours = [h(0, 2, "B")];
        let since = [vec![h(1, 1, "C")], vec![h(2, 3, "D")]];
        let e = rebase_ours(&ours, &since).unwrap_err();
        assert_eq!((e.ours, e.step, e.why), (0, 1, "replacement"));
        let recanon = crate::differential::with_live_mutant(
            crate::differential::LiveMutant::RecanonBothSides,
            || rebase_ours(&ours, &since),
        );
        assert!(recanon.is_ok(), "{recanon:?}");
    }

    /// S3: an insertion exactly at either edge of a range theirs deleted is no conflict; one
    /// strictly inside is, on a single step whichever edge it starts nearer.
    #[test]
    fn s3_edges_are_symmetric() {
        for (at, refused) in [(1, false), (2, true), (3, true), (4, false)] {
            let r = rebase_ours(&[h(at, at, "X")], &[vec![h(1, 4, "")]]);
            assert_eq!(r.is_err(), refused, "at {at}: {r:?}");
        }
        // and with a replacement's deleted range
        for (at, refused) in [(1, false), (2, true), (4, false)] {
            let r = rebase_ours(&[h(at, at, "X")], &[vec![h(1, 4, "Y")]]);
            assert_eq!(r.is_err(), refused, "at {at}: {r:?}");
        }
    }

    /// S3 (r2.3), across steps: an insertion tied with another at its point is strictly
    /// inside a later deletion only if it is under both placements (before and after the
    /// other's text).
    #[test]
    fn s3_inside_under_both_placements() {
        // K2 Engine's case: ours `A` at 0; `B` at 0 first (ours goes after it); then `Ba`
        // deleted: ours' left placement (before `B`) is at the deleted range's edge
        let r = rebase_ours(&[h(0, 0, "A")], &[vec![h(0, 0, "B")], vec![h(0, 2, "")]]);
        assert_eq!(r, Ok(vec![h(0, 0, "A")]));
        // both sides of both placements deleted: refused
        let e = rebase_ours(&[h(1, 1, "X")], &[vec![h(1, 1, "Y")], vec![h(0, 3, "")]]);
        assert_eq!(
            e.map_err(|e| (e.why, e.step)),
            Err(("insert_inside_deleted", 1))
        );
        // the other side: the left neighbour and Y deleted, X's right one kept
        let r = rebase_ours(&[h(1, 1, "X")], &[vec![h(1, 1, "Y")], vec![h(0, 2, "")]]);
        assert!(r.is_ok(), "{r:?}");
        // Y and the right neighbour deleted, the left one kept: X's left placement is at the
        // edge
        let r = rebase_ours(&[h(1, 1, "X")], &[vec![h(1, 1, "Y")], vec![h(1, 3, "")]]);
        assert!(r.is_ok(), "{r:?}");
        // planted: the right placement alone (r2.3's withdrawn reading)
        let planted = crate::differential::with_live_mutant(
            crate::differential::LiveMutant::InsideByTieBreak,
            || rebase_ours(&[h(1, 1, "X")], &[vec![h(1, 1, "Y")], vec![h(1, 3, "")]]),
        );
        assert!(planted.is_err());
    }

    /// S4: a change fully absorbed (it deletes only what theirs already deleted) rebases to
    /// nothing; the session acks it with no new version.
    #[test]
    fn s4_absorbed_change_is_empty() {
        let c = Carried::new(&[h(1, 3, "")])
            .through(&[h(0, 4, "")], 0)
            .unwrap();
        assert!(c.is_empty() && c.hunks().is_empty());
        assert_eq!(
            rebase_ours(&[h(1, 3, "")], &[vec![h(0, 4, "")]]).unwrap(),
            vec![]
        );
    }

    /// Random hunks on `t` (canonical).
    pub(super) fn random_hunks(rng: &mut impl rand::Rng, t: &str, max: usize) -> Vec<Hunk> {
        let bits = ["a", "b", "\n", "é", "日", "XY", ""];
        let b = |rng: &mut dyn rand::RngCore, t: &str| {
            let mut x = rand::Rng::gen_range(rng, 0..=t.len());
            while !t.is_char_boundary(x) {
                x -= 1;
            }
            x
        };
        let n = rng.gen_range(1..=max);
        let mut cuts: Vec<usize> = (0..2 * n).map(|_| b(rng, t)).collect();
        cuts.sort_unstable();
        let mut out: Vec<Hunk> = Vec::new();
        for c in cuts.chunks(2) {
            let (s, mut e) = (c[0], c[1]);
            if rng.gen_bool(0.4) {
                e = s;
            }
            if out.last().is_some_and(|l: &Hunk| s < l.end) {
                continue;
            }
            let ins: String = (0..rng.gen_range(0..3))
                .map(|_| bits[rng.gen_range(0..bits.len())])
                .collect();
            if s == e && ins.is_empty() {
                continue;
            }
            out.push(h(s, e, &ins));
        }
        diff::coalesce(out)
    }

    /// TP1 along a history (r2.3): ours carried step by step as the client carries its
    /// pending change ([`Carried::through`]) and each incoming change carried through ours'
    /// gap form ([`Carried::mirrored`]): after every step the client's text (base, ours,
    /// every incoming as mirrored) is the server's (base, the changes since, ours rebased),
    /// and the client's carried ours is the server's [`rebase_ours`].
    #[test]
    fn carried_client_and_server_agree_along_histories() {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::SmallRng::seed_from_u64(11);
        let (mut steps, mut refused) = (0, 0);
        for _ in 0..20_000 {
            let base: String = (0..rng.gen_range(0..8))
                .map(|_| ["a", "b", "c", "\n", "é", "日"][rng.gen_range(0..6)])
                .collect();
            let ours = random_hunks(&mut rng, &base, 3);
            let mut c = Carried::new(&ours);
            let (mut server, mut client) = (base.clone(), apply(&base, &ours));
            let mut since: Vec<Vec<Hunk>> = Vec::new();
            for step in 0..rng.gen_range(1..4) {
                let theirs = random_hunks(&mut rng, &server, 3);
                let shown = Carried::new(&theirs).mirrored(&c.gaps()).hunks();
                let next = c.through(&theirs, step);
                server = apply(&server, &theirs);
                since.push(theirs);
                match next {
                    Ok(n) => {
                        client = apply(&client, &shown);
                        c = n;
                        steps += 1;
                        assert_eq!(
                            apply(&server, &c.hunks()),
                            client,
                            "base {base:?} ours {ours:?} since {since:?}"
                        );
                        assert_eq!(apply(&server, &c.gaps()), client, "the gap form");
                        assert_eq!(rebase_ours(&ours, &since).as_ref(), Ok(&c.hunks()));
                    }
                    Err(e) => {
                        refused += 1;
                        assert_eq!(rebase_ours(&ours, &since), Err(e));
                        break;
                    }
                }
            }
        }
        assert!(
            steps > 20_000 && refused > 100,
            "{steps} steps, {refused} refused"
        );
    }

    /// TP1 on random pairs: wherever the server accepts ours, the server's text (theirs, then
    /// ours rebased) is the client's (ours, then theirs through ours).
    #[test]
    fn server_and_client_mirror_agree_on_random_pairs() {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::SmallRng::seed_from_u64(7);
        let (mut accepted, mut refused) = (0, 0);
        for _ in 0..20_000 {
            let base: String = (0..rng.gen_range(0..8))
                .map(|_| ["a", "b", "c", "\n", "é", "日"][rng.gen_range(0..6)])
                .collect();
            let ours = random_hunks(&mut rng, &base, 3);
            let theirs = random_hunks(&mut rng, &base, 3);
            match rebase_ours(&ours, std::slice::from_ref(&theirs)) {
                Ok(r) => {
                    accepted += 1;
                    let server = apply(&apply(&base, &theirs), &r);
                    let client = apply(&apply(&base, &ours), &rebase_incoming(&theirs, &ours));
                    assert_eq!(
                        server, client,
                        "base {base:?} ours {ours:?} theirs {theirs:?} rebased {r:?}"
                    );
                }
                Err(_) => refused += 1,
            }
        }
        assert!(
            accepted > 10_000 && refused > 100,
            "{accepted} accepted, {refused} refused"
        );
    }
}
