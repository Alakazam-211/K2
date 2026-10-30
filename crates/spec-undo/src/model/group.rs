//! Person undo grouping (U3; editor clients): consecutive edits from one connection carrying
//! one group token are **one undo step** until a closer fires. Written from the spec's U3 and
//! its kit note (`docs/SPEC-UNDO.md`), not from the kit's code.
//!
//! **Closers.** An edit starts a new step when any of these holds; otherwise it joins the
//! open group:
//! - (a) **idle:** the group's last edit is [`Bounds::group_idle_ms`] old or more (500 ms;
//!   time is the caller's, as the host's clock);
//! - (b) **non-contiguous:** the edit is not one hunk overlapping or touching the group's
//!   span, on its root (an edit of several hunks, or on several roots, is never contiguous);
//! - (c) **word boundary:** it types a space, tab or punctuation after a word character
//!   (**a boundary starts the next step**: the boundary character opens it);
//! - (d) **newline:** it types a newline: a step of its own (it opens no group);
//! - (e) **another token**, another connection of the same person, an edit without a token
//!   or any other batch of the person (a verb, a write, a revert, a purge);
//! - (f) **an undo or redo** by the person (whatever it finds);
//! - (g) **anyone else's change inside the group's span or touching it**, in apply order
//!   (the span as carried; touching: the change's range meets the span's, ends included).
//!
//! **Agents never group:** one call is one batch whatever token it carries.
//!
//! **Joining (R12, reading):** the group is one region, read off the text: its window is the
//! union of the group's span and the range the edit replaced, in the text just before the
//! edit; `before` is that window with the group's span put back to what the group replaced
//! (its own `before`, roots included), `after` is the window as the edit left it (its
//! elements' roots and ids). Ours is one edit over the whole window, as a fresh change's.
//! So undoing a group is undoing its edits, element for element. A group whose `before`
//! and `after` are equal, roots included (typed, then deleted), **undid itself**: it leaves
//! no step, and closes.
//!
//! **Word and boundary characters (R12, reading):** a word character is alphanumeric or `_`;
//! a boundary is whitespace or punctuation, punctuation being ASCII's and Unicode's
//! punctuation blocks (Latin-1's `¡`–`¿`, General Punctuation's `‐`–`‧` and `‰`–`⁞`, CJK's
//! `　`–`〿`, the fullwidth `！`–`／`). The spec names the kinds, not a table: this is the
//! table the kit documents.
//!
//! Revert records are per change, as without grouping (U22): grouping changes only what the
//! person's undo stack holds.

use super::*;

/// A person's open group: the top of their undo stack while it is set.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OpenGroup {
    /// The connection that opened it (the caller's numbering).
    pub conn: usize,
    pub token: u64,
    /// The host time of its last edit.
    pub last_ms: u64,
    /// Someone's change landed inside or touching its span since its last edit (U3 (g)).
    pub touched: bool,
}

/// Why an edit with a token didn't join the person's open group (U3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Closer {
    /// (a)
    Idle,
    /// (b)
    NonContiguous,
    /// (c)
    Boundary,
    /// (d)
    Newline,
    /// (e): another token or connection
    Token,
    /// (g)
    Foreign,
}

/// What [`Model::edit_grouped`] did with the person's undo stack (the trace runner's
/// classes and its mirror of the stacks; not part of the state).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GroupNote {
    /// The edit changed nothing: no batch, the open group as it was.
    #[default]
    Nothing,
    /// An agent's edit: a batch of its own (agents never group).
    Agent,
    /// A new step, with every closer that fired (empty: there was no open group).
    New(Vec<Closer>),
    /// It joined the open group.
    Joined,
    /// It joined the open group, which then undid itself: the group's step is gone.
    SelfUndone,
}

/// A word character (U3 (c)).
pub fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A boundary character, typed after a word character (U3 (c)).
pub fn boundary(c: char) -> bool {
    c.is_whitespace()
        || c.is_ascii_punctuation()
        || matches!(c,
            '\u{00a1}'..='\u{00bf}'
            | '\u{2010}'..='\u{2027}'
            | '\u{2030}'..='\u{205e}'
            | '\u{3000}'..='\u{303f}'
            | '\u{ff01}'..='\u{ff0f}')
}

/// U3 (g), on a change to `root` (`es` in the coordinates before it): it lands inside the
/// open group's span or touches it. Called before the regions are carried.
pub(super) fn touch(st: &mut Stacks, root: usize, es: &[Edit]) {
    let (Some(o), Some(top)) = (st.open.as_mut(), st.undo.last()) else {
        return;
    };
    let hit = top
        .regions
        .iter()
        .filter(|r| r.root == root)
        .any(|r| es.iter().any(|e| e.start <= r.end && e.end >= r.start));
    if hit {
        o.touched = true;
    }
}

impl Model {
    /// A grouped edit (U3; an editor client's keystroke): `edits` (as [`Model::edit`]) by
    /// `pid` on connection `conn` with group token `token` at host time `now_ms`; `agent`:
    /// the pid is an agent (it never groups). One change, one revert record, as any edit;
    /// on the person's undo stack it joins their open group or starts a new step.
    pub fn edit_grouped(
        &mut self,
        pid: &str,
        agent: bool,
        conn: usize,
        token: u64,
        now_ms: u64,
        edits: Vec<(usize, Vec<Edit>)>,
    ) -> (Outcome, GroupNote) {
        let authored: Vec<(usize, Vec<Edit>)> = edits
            .into_iter()
            .map(|(r, es)| (r, hunk::canonical(es)))
            .filter(|(_, es)| !es.is_empty())
            .collect();
        // D1 (v1.10): the change as made is widened to the tokens it lies in, as a fresh
        // change's is ([`Model::edit`]); the closers (c) and (d) read what the person typed
        // (the authored hunks), contiguity (b) and joining the change as made (the kit's
        // `group_join` and `compose_group` likewise)
        let edits: Vec<(usize, Vec<Edit>)> = authored
            .iter()
            .map(|(r, es)| (*r, self.d1(pid, *r, es.clone(), &[])))
            .collect();
        if agent {
            self.authored_next = Some(authored);
            let out = self.new_batch(pid, None, edits);
            let note = match out {
                Outcome::Changed(Some(_)) => GroupNote::Agent,
                _ => GroupNote::Nothing,
            };
            return (out, note);
        }
        // (c), (d): what it types, each character after the one before it (the text's
        // character before the hunk, for its first)
        let (mut newline, mut bounded) = (false, false);
        for (root, es) in &authored {
            let t = &self.texts[*root];
            for e in es {
                let mut prev = t[..e.start].chars().next_back();
                for c in e.insert.chars() {
                    newline |= c == '\n';
                    bounded |= boundary(c) && prev.is_some_and(word);
                    prev = Some(c);
                }
            }
        }
        // the one hunk it is, if it is one
        let single = match edits.as_slice() {
            [(root, es)] if es.len() == 1 => Some((*root, es[0].clone())),
            _ => None,
        };
        // the open group and every closer that fires
        let mut why = Vec::new();
        let mut join: Option<(usize, usize)> = None;
        if let Some(st) = self.stacks.get(pid) {
            if let Some(o) = &st.open {
                let top = st
                    .undo
                    .last()
                    .expect("an open group is the top of the stack");
                if now_ms.saturating_sub(o.last_ms) >= self.bounds.group_idle_ms {
                    why.push(Closer::Idle);
                }
                let span = match top.regions.as_slice() {
                    [r] => Some(r),
                    _ => None,
                };
                let contiguous = match (&single, span) {
                    (Some((root, e)), Some(r)) => {
                        *root == r.root && e.start <= r.end && r.start <= e.end
                    }
                    _ => false,
                };
                if !contiguous {
                    why.push(Closer::NonContiguous);
                }
                if bounded {
                    why.push(Closer::Boundary);
                }
                if newline {
                    why.push(Closer::Newline);
                }
                if o.conn != conn || o.token != token {
                    why.push(Closer::Token);
                }
                if o.touched {
                    why.push(Closer::Foreign);
                }
                if why.is_empty() {
                    let r = span.expect("contiguous with one region");
                    join = Some((r.start, r.end));
                }
            }
        }
        // the root's text and elements before the edit (what a joined group's `before`
        // reads)
        let snap = join.map(|_| {
            let (root, _) = single.as_ref().expect("a joining edit is one hunk");
            (self.texts[*root].clone(), self.elems[*root].clone())
        });
        let change = edits.into_iter().map(|(r, es)| (r, es, None)).collect();
        self.authored_next = Some(authored);
        let Some((id, regions, _)) = self.commit(pid, None, change) else {
            // no change: no batch, and the group stays as it was
            return (Outcome::Changed(None), GroupNote::Nothing);
        };
        let out = Outcome::Changed(Some(id));
        let st = self.stacks.entry(pid.to_owned()).or_default();
        st.redo.clear();
        st.cleared = false;
        st.open = None;
        let note = match (join, snap, single) {
            (Some((a, b)), Some((t0, e0)), Some((root, e))) => {
                let prev = st.undo.pop().expect("the open group");
                let g = self.joined(&prev.regions[0], a, b, root, &e, &t0, &e0);
                if g.before == g.after && g.before_roots == g.after_roots {
                    // the group undid itself: no step, and it is closed
                    return (out, GroupNote::SelfUndone);
                }
                self.stacks.get_mut(pid).expect("a stack").undo.push(Batch {
                    on_behalf_of: None,
                    regions: vec![g],
                    after_partial: false,
                });
                GroupNote::Joined
            }
            _ => {
                st.undo.push(Batch {
                    on_behalf_of: None,
                    regions,
                    after_partial: false,
                });
                GroupNote::New(why)
            }
        };
        self.bound(pid);
        // it is the open group unless it typed a newline (d), or the bound took it (U6)
        let st = self.stacks.get_mut(pid).expect("a stack");
        if !newline && !st.undo.is_empty() {
            st.open = Some(OpenGroup {
                conn,
                token,
                last_ms: now_ms,
                touched: false,
            });
        }
        (out, note)
    }

    /// R12: the open group's region `prev` (span `a..b` in `t0`, the root's text before the
    /// edit, whose elements were `e0`) joined with the edit `e` (on `t0`), read off the text
    /// as the edit left it.
    #[allow(clippy::too_many_arguments)]
    fn joined(
        &self,
        prev: &Region,
        a: usize,
        b: usize,
        root: usize,
        e: &Edit,
        t0: &str,
        e0: &[Elem],
    ) -> Region {
        debug_assert_eq!(
            &t0[a..b],
            prev.after,
            "an untouched group's span is its after"
        );
        let (s, end0) = (a.min(e.start), b.max(e.end));
        let end1 = (end0 as isize + e.growth()) as usize;
        let t1 = &self.texts[root];
        let after = t1[s..end1].to_owned();
        let before = format!("{}{}{}", &t0[s..a], prev.before, &t0[b..end0]);
        let roots0 = |x: usize, y: usize| -> Vec<Prov> {
            e0[hunk::chars_in(t0, x, y)]
                .iter()
                .map(|el| el.prov)
                .collect()
        };
        let mut before_roots = roots0(s, a);
        before_roots.extend(&prev.before_roots);
        before_roots.extend(roots0(b, end0));
        let els = &self.elems[root][hunk::chars_in(t1, s, end1)];
        Region {
            root,
            start: s,
            end: end1,
            ours: vec![Edit {
                start: 0,
                end: after.len(),
                insert: before.clone(),
            }],
            theirs: Vec::new(),
            before_roots,
            after_roots: els.iter().map(|x| x.prov).collect(),
            after_ids: els.iter().map(|x| x.id).collect(),
            before,
            after,
            tainted: false,
            base_doc_nonempty: !t1.is_empty(),
            erased: false,
        }
    }
}
