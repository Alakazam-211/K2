//! Person undo grouping (undo spec U3, kit v0.9.7, K4): consecutive edits from one connection
//! with the same [`EditOptions::group`] token merge into **one undo step**. A child of `live`,
//! so it works on the session's own state; it touches only the undo stack push.
//!
//! The editor proposes the token; the session closes the group (the next edit is a new
//! step) on what it can see: idle on the host clock (a), a non-contiguous edit (b), a word
//! boundary (c), a newline (d), another token, no token or any other push by the person
//! (e), an undo or redo (f), and anyone else's change inside the group's span or touching
//! it, in host apply order (g). Agents never group.
//!
//! **How a step joins:** the open group is the top of the person's undo stack. Before the
//! edit lands, [`Session::group_join`] checks the closers that read the state before it and
//! takes the group's bounds; after it lands, [`Session::push_grouped`] composes the group's
//! region with the edit's ([`undo::compose_group`], which closes it on a non-contiguous
//! edit) into one region whose roots compose exactly, so undoing the group is undoing its
//! edits, element for element. A group that undid itself (typed, then deleted) leaves
//! nothing on the stack.

use super::*;

/// What an edit may join, read before it lands.
pub(super) struct Join {
    /// The open group's region bounds just before the edit (`None`: it starts a new step).
    prev: Option<(usize, usize)>,
    /// It typed a newline: a step of its own (U3 (d)), so nothing joins it.
    closes_after: bool,
}

/// A word character (U3 (c)).
fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A word boundary when typed after a word character (U3 (c)): a space, tab or punctuation.
fn boundary(c: char) -> bool {
    c.is_whitespace()
        || c.is_ascii_punctuation()
        || matches!(c,
            '\u{00a1}'..='\u{00bf}'
            | '\u{2010}'..='\u{2027}'
            | '\u{2030}'..='\u{205e}'
            | '\u{3000}'..='\u{303f}'
            | '\u{ff01}'..='\u{ff0f}')
}

/// The planted faults ([`crate::differential::GroupMutant`]): (never closes on a foreign
/// change, never closes on idle, never closes on a word boundary).
#[cfg(any(test, feature = "differential"))]
fn planted() -> (bool, bool, bool) {
    use crate::differential::{group_mutant, GroupMutant};
    let m = group_mutant();
    (
        m == Some(GroupMutant::NeverClosesOnForeign),
        m == Some(GroupMutant::NeverClosesOnIdle),
        m == Some(GroupMutant::NeverClosesOnBoundary),
    )
}

#[cfg(not(any(test, feature = "differential")))]
fn planted() -> (bool, bool, bool) {
    (false, false, false)
}

impl Session {
    /// What member `i`'s edit `edits` with group `token` may join (U3), read on the state
    /// before it lands: the open group's bounds when the group is this connection's, has
    /// this token, isn't idle, is still the top of the undo stack untouched by anyone
    /// since, and the edit types no word boundary or newline.
    pub(super) fn group_join(
        &self,
        st: &State,
        i: usize,
        token: u64,
        edits: &[Edit],
        now: u64,
    ) -> Join {
        let (foreign_mutant, idle_mutant, boundary_mutant) = planted();
        // (c), (d): what it types, after the character before each insertion
        let (mut newline, mut bounded) = (false, false);
        for e in edits {
            let Edit::Text { root, hunks } = e;
            let Ok((_, text)) = self.text_of(st, root) else {
                bounded = true;
                continue;
            };
            for h in hunks {
                let mut prev = text.get(..h.start).and_then(|t| t.chars().next_back());
                for c in h.insert.chars() {
                    newline |= c == '\n';
                    bounded |= boundary(c) && prev.is_some_and(word);
                    prev = Some(c);
                }
            }
        }
        let closes_after = newline;
        let starts_new = newline || (bounded && !boundary_mutant);
        let m = &st.members[i];
        let prev = (!starts_new)
            .then(|| st.hist.get(&m.p.pid))
            .flatten()
            .and_then(|h| {
                let o = h.open.as_ref()?;
                // (e): this connection, this token
                if o.conn != m.conn || o.token != token {
                    return None;
                }
                // (a): idle on the host clock
                if !idle_mutant && now.saturating_sub(o.last_ms) >= self.config.undo_group_idle_ms {
                    return None;
                }
                let top = h.undo.back().filter(|b| b.seq == o.seq)?;
                // (g): anyone's change inside the span or touching it since
                if !foreign_mutant && top.touched {
                    return None;
                }
                let [r] = top.regions.as_slice() else {
                    return None;
                };
                if !foreign_mutant && (r.tainted || !r.theirs.is_empty()) {
                    return None;
                }
                Some((r.start, r.end))
            });
        Join { prev, closes_after }
    }

    /// Put member `i`'s grouped edit `batch` on their undo stack: merged into the open group
    /// when [`Session::group_join`] allowed it and it is contiguous with it (U3 (b)), else a
    /// new step; then it is the open group, unless it typed a newline.
    pub(super) fn push_grouped(
        &self,
        st: &mut State,
        i: usize,
        token: u64,
        join: Join,
        batch: Batch,
        now: u64,
    ) {
        let (pid, conn) = (st.members[i].p.pid.clone(), st.members[i].conn);
        let seq = batch.seq;
        let h = st.hist.entry(pid.clone()).or_default();
        let merged = match (join.prev, h.undo.back()) {
            (Some((a, b)), Some(top))
                if top.on_behalf_of == batch.on_behalf_of
                    && top.regions.len() == 1
                    && batch.regions.len() == 1 =>
            {
                undo::compose_group(&top.regions[0], a, b, &batch.regions[0])
            }
            _ => None,
        };
        let batch = match merged {
            Some(region) => {
                h.undo.pop_back();
                let noop = region.before == region.after
                    && region.roots.as_ref().is_some_and(|r| r.before == r.after);
                if noop {
                    // the group undid itself: nothing to undo, and it is closed
                    h.bytes = h.held_bytes();
                    h.open = None;
                    return;
                }
                Batch {
                    seq,
                    on_behalf_of: batch.on_behalf_of,
                    regions: vec![region],
                    touched: false,
                }
            }
            None => batch,
        };
        self.push_batch(st, &pid, batch, Origin::Edit);
        if join.closes_after {
            return;
        }
        if let Some(h) = st.hist.get_mut(&pid) {
            if h.undo.back().is_some_and(|b| b.seq == seq) {
                h.open = Some(OpenGroup {
                    conn,
                    token,
                    last_ms: now,
                    seq,
                });
            }
        }
    }
}
