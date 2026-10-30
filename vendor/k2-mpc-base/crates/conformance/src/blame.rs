//! Rules over [`Blame`].
//!
//! - `text.blame_char` (E-TEXT-11; undo spec U18a, U23): every live character maps to its
//!   **provenance-root insert** (the event, agent and ID, that inserted the element it is or
//!   restores), plus the **restoring event** (the undo, redo or revert that put it back)
//!   when it is a restoration; IDs are unique per character and identical on every
//!   replica; a character keeps its ID through others' edits; text deleted and typed again
//!   blames to whoever typed it again (typing is never a restoration). A replica has no
//!   undo: every element here is its own root, and the rule checks the insert; restored
//!   elements are checked on the kit's sessions (`hist.undo_own_only`, which reads each
//!   element's root, and the kit's blame tests).
//! - `hist.blame_grain` (E-HIST-4), over [`BlameAt`]: blame at a past version is per
//!   character, the same on every replica, and equal to the live blame when that version
//!   was current (by root, as `text.blame_char`); a purge removes its span from the blame
//!   of every version (it reads as removed: no character of it blames to anyone) and
//!   changes no other character's.

use std::collections::BTreeSet;

use crate::caps::{Blame, BlameAt, Blamed, Events};
use crate::erasure::secret;
use crate::purge::{blame_without, purge_live};
use crate::{del, ins, rep, Net, SEED_AGENT};

/// Every rule over [`Blame`], by id.
pub const RULES: &[&str] = &["text.blame_char"];

/// Every rule over [`BlameAt`], by id.
pub const AT_RULES: &[&str] = &["hist.blame_grain"];

/// Run rule `id` on engine `E`. An unknown id panics.
pub fn run_at<E: BlameAt>(id: &str) {
    match id {
        "hist.blame_grain" => blame_grain::<E>(),
        _ => panic!("no blame-at rule {id:?}"),
    }
}

/// Run rule `id` on engine `E`. An unknown id panics.
pub fn run<E: Blame>(id: &str) {
    match id {
        "text.blame_char" => blame_char::<E>(),
        _ => panic!("no blame rule {id:?}"),
    }
}

/// Check one replica's blame against its text and `who` (the agent each character must
/// blame to); returns it.
fn check<E: Blame>(r: &E, who: impl Fn(char) -> u64, at: &str) -> Vec<Blamed<E::CharId>> {
    let b = r.blame();
    let chars: String = b.iter().map(|x| x.ch).collect();
    assert_eq!(
        chars,
        r.text(),
        "{}: {at}: blame doesn't cover the text",
        E::ENGINE
    );
    for x in &b {
        assert_eq!(
            x.agent,
            who(x.ch),
            "{}: {at}: {:?} blames to the wrong agent ({:?})",
            E::ENGINE,
            x.ch,
            x.id
        );
    }
    let ids: BTreeSet<&E::CharId> = b.iter().map(|x| &x.id).collect();
    assert_eq!(
        ids.len(),
        b.len(),
        "{}: {at}: two characters share an ID",
        E::ENGINE
    );
    b
}

/// `text.blame_char`
pub fn blame_char<E: Blame>() {
    let seed = "--------\n";
    let mut n = Net::<E>::new(&[101, 102, 103], seed);
    let seed_blame = check(&n.r[0], |_| SEED_AGENT, "the seed");

    n.insert(0, 2, "aé");
    n.insert(1, 6, "b🎉");
    n.sync(0, 2);
    // 103 types `c` at the start and deletes 101's `a`, in one batch
    n.edit(2, vec![ins(0, "c"), del(2, 3)]);
    n.sync_all();
    let who = |c: char| match c {
        '-' | '\n' => SEED_AGENT,
        'a' | 'é' => 101,
        'b' | '🎉' => 102,
        'c' => 103,
        other => panic!("unexpected character {other:?}"),
    };
    let first = check(&n.r[0], who, "after concurrent edits");
    for i in 1..n.r.len() {
        assert_eq!(
            check(&n.r[i], who, "after concurrent edits"),
            first,
            "{}: replica {i}'s blame differs",
            E::ENGINE
        );
    }
    assert!(!n.text(0).contains('a'), "{}", E::ENGINE);

    // the seed's surviving characters keep their IDs, in order
    let kept: Vec<&E::CharId> = first
        .iter()
        .filter(|x| x.agent == SEED_AGENT)
        .map(|x| &x.id)
        .collect();
    let mut it = seed_blame.iter().map(|x| &x.id);
    for id in &kept {
        assert!(
            it.any(|s| s == *id),
            "{}: seed character {id:?} changed ID or order",
            E::ENGINE
        );
    }
    assert_eq!(
        kept.len(),
        seed.chars().count(),
        "{}: the seed lost characters",
        E::ENGINE
    );

    // 102 deletes `é` and types it again: it now blames to 102
    let t = n.text(1);
    let at = t.find('é').expect("é is live");
    n.edit(1, vec![rep(at, at + 'é'.len_utf8(), "é")]);
    n.sync_all();
    let who2 = |c: char| if c == 'é' { 102 } else { who(c) };
    let last = check(&n.r[0], who2, "after typing é again");
    for i in 1..n.r.len() {
        assert_eq!(
            check(&n.r[i], who2, "after typing é again"),
            last,
            "{}",
            E::ENGINE
        );
    }
}

fn exchange<E: Events>(a: &mut E, b: &mut E) {
    let (to_a, to_b) = (b.missing_for(a), a.missing_for(b));
    if let Err(e) = a.deliver(&to_a).and_then(|_| b.deliver(&to_b)) {
        panic!("{}: exchange refused: {e:?}", E::ENGINE);
    }
}

fn blame_at<E: BlameAt>(r: &E, v: &E::Version) -> Vec<Blamed<E::CharId>> {
    r.blame_at(v)
        .unwrap_or_else(|e| panic!("{}: blame at {v:?}: {e:?}", E::ENGINE))
}

/// A version, its blame and its text, as they were live.
type Seen<E> = (
    <E as crate::TextReplica>::Version,
    Vec<Blamed<<E as Blame>::CharId>>,
    String,
);

/// `hist.blame_grain`
pub fn blame_grain<E: BlameAt>() {
    let seed = "--------\n";
    let mut a = E::replica(101, seed);
    let mut b = E::replica(102, seed);
    let mut at: Vec<Seen<E>> = Vec::new();
    let cut = |r: &E, at: &mut Vec<_>| at.push((r.version(), r.blame(), r.text()));
    cut(&a, &mut at);

    let edit = |r: &mut E, h: Vec<crate::Hunk>| {
        if let Err(e) = r.apply(&h) {
            panic!("{}: refused {h:?}: {e:?}", E::ENGINE);
        }
    };
    edit(&mut a, vec![ins(2, "aé")]);
    edit(&mut b, vec![ins(6, "b🎉")]);
    exchange(&mut a, &mut b);
    cut(&a, &mut at);
    // a deletes `a`; b types `é` again (it then blames to b)
    let t = a.text();
    let x = t.find('a').expect("a");
    edit(&mut a, vec![del(x, x + 1)]);
    let t = b.text();
    let x = t.find('é').expect("é");
    edit(&mut b, vec![rep(x, x + 'é'.len_utf8(), "é")]);
    exchange(&mut a, &mut b);
    cut(&a, &mut at);
    // a secret, typed by b, live at the last version
    let s = secret();
    let t = b.text();
    edit(&mut b, vec![ins(t.len(), &format!("{s}\n"))]);
    exchange(&mut a, &mut b);
    cut(&a, &mut at);

    // per character, on every replica, as it was live
    for (i, (v, live, text)) in at.iter().enumerate() {
        for (n, r) in [("a", &a), ("b", &b)] {
            let got = blame_at(r, v);
            assert_eq!(&got, live, "{}: version {i} on {n}", E::ENGINE);
            let chars: String = got.iter().map(|x| x.ch).collect();
            assert_eq!(
                &chars,
                text,
                "{}: version {i} on {n}: not the text",
                E::ENGINE
            );
        }
    }

    // a purge reads as removed in every version's blame; nothing else moves
    let (p, x) = purge_live(&mut a, &s);
    if let Err(e) = b.deliver(&p) {
        panic!("{}: the purge to b: {e:?}", E::ENGINE);
    }
    let (start, end) = (x, x + s.len());
    let last = at.len() - 1;
    for (i, (v, live, text)) in at.iter().enumerate() {
        let want = if i == last {
            blame_without(live, start, end)
        } else {
            live.clone()
        };
        for (n, r) in [("a", &a), ("b", &b)] {
            let got = blame_at(r, v);
            assert_eq!(
                got,
                want,
                "{}: version {i} on {n} after the purge",
                E::ENGINE
            );
            let chars: String = got.iter().map(|x| x.ch).collect();
            assert!(
                !chars.contains(&s[..8]),
                "{}: version {i} blames purged text",
                E::ENGINE
            );
            if i < last {
                assert_eq!(&chars, text, "{}", E::ENGINE);
            }
        }
    }
    assert_eq!(
        a.blame(),
        blame_without(&at[last].1, start, end),
        "{}",
        E::ENGINE
    );
    assert_eq!(b.blame(), a.blame(), "{}", E::ENGINE);
}
