//! v0.2.0 API: `Doc::replica`, `edit_as` (UTF-8 byte offsets, atomic batches),
//! `diff_events`, `deliver_all`, `blame`, anchors; and a seeded property test.

use mpce_model::check::{def4, strong_list_spec};
use mpce_model::text::{Anchor, Doc, DocError, Hunk, Id};
use mpce_model::tree::Side;
use std::collections::{BTreeMap, BTreeSet};

fn h(start: usize, end: usize, s: &str) -> Hunk {
    Hunk { start, end, insert: s.to_string() }
}

fn snapshot(d: &Doc) -> (String, Vec<Id>, BTreeSet<Id>, usize) {
    (d.text(), d.list_with_tombstones(), d.frontier().clone(), d.log().len())
}

#[test]
fn byte_offset_edits_with_multibyte_text() {
    let mut d = Doc::replica();
    d.edit_as(101, &[h(0, 0, "héllo 🎉\nworld")]).unwrap();
    assert_eq!(d.text(), "héllo 🎉\nworld");
    // "é" is bytes 1..3
    d.edit_as(101, &[h(1, 3, "e")]).unwrap();
    assert_eq!(d.text(), "hello 🎉\nworld");
    // "🎉" is bytes 6..10, "\n" is 10..11; two hunks in one batch
    let ev = d.edit_as(102, &[h(6, 6, "!"), h(10, 11, "")]).unwrap();
    assert_eq!(d.text(), "hello !🎉world");
    assert_eq!(ev.len(), 2);
    // replace a multi-byte char by a multi-byte string, and append at the end
    let n = d.text().len();
    d.edit_as(103, &[h(7, 11, "é\n"), h(n, n, "🎉")]).unwrap();
    assert_eq!(d.text(), "hello !é\nworld🎉");
}

#[test]
fn offsets_inside_a_scalar_are_refused() {
    let mut d = Doc::replica();
    d.edit_as(101, &[h(0, 0, "héllo🎉")]).unwrap();
    let before = snapshot(&d);
    assert_eq!(d.edit_as(101, &[h(2, 2, "x")]), Err(DocError::NotCharBoundary { offset: 2 }));
    assert_eq!(d.edit_as(101, &[h(0, 7, "")]), Err(DocError::NotCharBoundary { offset: 7 }));
    assert_eq!(d.edit_as(101, &[h(0, 99, "")]), Err(DocError::OutOfRange));
    assert_eq!(d.anchor(2, Side::Right), Err(DocError::NotCharBoundary { offset: 2 }));
    assert_eq!(snapshot(&d), before);
}

#[test]
fn hunks_must_be_sorted_and_disjoint() {
    let mut d = Doc::replica();
    d.edit_as(101, &[h(0, 0, "abcdef")]).unwrap();
    let before = snapshot(&d);
    assert_eq!(d.edit_as(101, &[h(3, 4, "x"), h(1, 2, "y")]), Err(DocError::BadHunks));
    assert_eq!(d.edit_as(101, &[h(1, 4, "x"), h(3, 5, "y")]), Err(DocError::BadHunks));
    assert_eq!(d.edit_as(101, &[h(2, 2, "x"), h(2, 2, "y")]), Err(DocError::BadHunks));
    assert_eq!(d.edit_as(101, &[h(4, 2, "x")]), Err(DocError::BadHunks));
    assert_eq!(snapshot(&d), before);
    // adjacent hunks are fine
    d.edit_as(101, &[h(1, 2, "B"), h(2, 3, "C")]).unwrap();
    assert_eq!(d.text(), "aBCdef");
}

#[test]
fn a_bad_hunk_makes_the_whole_batch_change_nothing() {
    let mut d = Doc::replica();
    d.edit_as(101, &[h(0, 0, "aé")]).unwrap();
    let before = snapshot(&d);
    // first hunk fine, second inside "é"
    assert_eq!(d.edit_as(102, &[h(0, 0, "X"), h(2, 2, "Y")]), Err(DocError::NotCharBoundary { offset: 2 }));
    assert_eq!(snapshot(&d), before);
    // and the agent's seq did not advance
    let ev = d.edit_as(102, &[h(0, 0, "X")]).unwrap();
    assert_eq!(ev[0].id, Id::new(102, 0));
}

#[test]
fn seqs_are_consecutive_and_continue_for_a_returning_agent() {
    let mut a = Doc::replica();
    let ev = a.edit_as(101, &[h(0, 0, "ab")]).unwrap();
    assert_eq!(ev.iter().map(|e| e.id).collect::<Vec<_>>(), vec![Id::new(101, 0), Id::new(101, 1)]);
    let ev = a.edit_as(101, &[h(0, 1, "")]).unwrap();
    assert_eq!(ev[0].id, Id::new(101, 2));
    // the agent returns on another replica that has its events
    let mut b = Doc::replica();
    b.deliver_all(&a.diff_events(&BTreeSet::new())).unwrap();
    let ev = b.edit_as(101, &[h(1, 1, "c")]).unwrap();
    assert_eq!(ev[0].id, Id::new(101, 3));
    // a fixed-agent replica still works; a replica() refuses agentless edits
    let mut c = Doc::new(7);
    c.insert(0, 'x').unwrap();
    assert_eq!(c.agent(), Some(7));
    assert_eq!(Doc::replica().insert(0, 'x'), Err(DocError::NoAgent));
}

/// two replicas with concurrent edits on a common base
fn concurrent_pair() -> (Doc, Doc) {
    let mut x = Doc::replica();
    x.edit_as(101, &[h(0, 0, "base text")]).unwrap();
    let mut y = Doc::replica();
    y.sync_from(&x).unwrap();
    x.edit_as(101, &[h(0, 4, "BASE"), h(9, 9, "!")]).unwrap();
    y.edit_as(102, &[h(4, 5, "_"), h(5, 5, "é🎉")]).unwrap();
    (x, y)
}

#[test]
fn diff_events_and_deliver_all_reproduce_sync_from() {
    let (x, y) = concurrent_pair();
    let mut a = x.clone();
    a.sync_from(&y).unwrap();
    let mut b = x.clone();
    let n = b.deliver_all(&y.diff_events(x.frontier())).unwrap();
    assert!(n > 0);
    assert_eq!(snapshot(&a).0, snapshot(&b).0);
    assert_eq!(a.list_with_tombstones(), b.list_with_tombstones());
    assert_eq!(a.frontier(), b.frontier());
    // nothing left to send either way once synced
    assert!(b.diff_events(a.frontier()).is_empty());
    // the full log from an empty frontier; in reverse order, with duplicates: same result
    let mut all = y.diff_events(&BTreeSet::new());
    assert_eq!(all.len(), y.log().len());
    all.reverse();
    all.extend(all.clone());
    let mut c = x.clone();
    c.deliver_all(&all).unwrap();
    assert_eq!(c.list_with_tombstones(), a.list_with_tombstones());
    assert_eq!(c.deliver_all(&all), Ok(0));
}

#[test]
fn deliver_all_refuses_missing_dependencies_atomically() {
    let (x, y) = concurrent_pair();
    let diff = y.diff_events(x.frontier());
    let mut z = x.clone();
    let before = snapshot(&z);
    // the first event is deliverable, the last needs the ones in between
    let partial = vec![diff[0].clone(), diff[diff.len() - 1].clone()];
    assert_eq!(z.deliver_all(&partial), Err(DocError::MissingDeps));
    assert_eq!(snapshot(&z), before);
}

#[test]
fn blame_after_concurrent_edits() {
    let (mut x, mut y) = concurrent_pair();
    let xs = x.clone();
    x.sync_from(&y).unwrap();
    y.sync_from(&xs).unwrap();
    assert_eq!(x.text(), y.text());
    let b = x.blame();
    assert_eq!(b, y.blame());
    assert_eq!(b.iter().map(|(c, _)| *c).collect::<String>(), x.text());
    for (c, id) in &b {
        match c {
            'B' | 'A' | 'S' | 'E' | '!' => assert_eq!(id.agent, 101, "{c}"),
            '_' | 'é' | '🎉' => assert_eq!(id.agent, 102, "{c}"),
            _ => {}
        }
    }
}

#[test]
fn anchors_are_stable_across_concurrent_edits() {
    let mut a = Doc::replica();
    a.edit_as(101, &[h(0, 0, "héllo")]).unwrap();
    let mut b = Doc::replica();
    b.sync_from(&a).unwrap();
    // the gap between "é" (bytes 1..3) and "l" (3..4)
    let right = a.anchor(3, Side::Right).unwrap(); // sticks to the first "l"
    let left = a.anchor(3, Side::Left).unwrap(); // sticks to "é"
    let start = a.anchor(0, Side::Left).unwrap();
    let end = a.anchor(a.text().len(), Side::Right).unwrap();
    assert_eq!((start.elem, end.elem), (None, None));
    assert_eq!((a.resolve(&right), a.resolve(&left)), (Ok(3), Ok(3)));
    // concurrently: b inserts "🎉" into that gap; a inserts before "h" and deletes the last "o"
    b.edit_as(102, &[h(3, 3, "🎉")]).unwrap();
    a.edit_as(101, &[h(0, 0, "XY"), h(5, 6, "")]).unwrap();
    let a0 = a.clone();
    a.sync_from(&b).unwrap();
    b.sync_from(&a0).unwrap();
    assert_eq!(a.text(), "XYhé🎉ll");
    for d in [&a, &b] {
        assert_eq!(d.resolve(&left), Ok(5)); // just after "é"
        assert_eq!(d.resolve(&right), Ok(9)); // just before "l", after the inserted 🎉
        assert_eq!(d.resolve(&start), Ok(0));
        assert_eq!(d.resolve(&end), Ok(d.text().len()));
    }
    // deleted anchors resolve to the gap where the element was
    b.edit_as(102, &[h(3, 5, "")]).unwrap(); // delete "é"
    let b0 = b.clone();
    b.sync_from(&a).unwrap();
    a.sync_from(&b0).unwrap();
    assert_eq!(a.text(), "XYh🎉ll");
    for d in [&a, &b] {
        assert_eq!(d.resolve(&left), Ok(3)); // where "é" was: after "h"
        assert_eq!(d.resolve(&right), Ok(7));
    }
    // delete the anchor's own element and its neighbours concurrently
    let a1 = a.edit_as(101, &[h(7, 8, "")]); // a deletes the first "l" (right's element)
    a1.unwrap();
    b.edit_as(102, &[h(3, 7, "Z")]).unwrap(); // b replaces 🎉 by Z
    let a0 = a.clone();
    a.sync_from(&b).unwrap();
    b.sync_from(&a0).unwrap();
    assert_eq!(a.text(), b.text());
    assert_eq!(a.resolve(&right), b.resolve(&right));
    assert_eq!(a.resolve(&left), b.resolve(&left));
    // an anchor on an element a replica has not received
    let mut c = Doc::replica();
    let ev = c.edit_as(103, &[h(0, 0, "q")]).unwrap();
    let anc: Anchor = c.anchor(0, Side::Right).unwrap();
    assert_eq!(anc.elem, Some(ev[0].id));
    assert_eq!(a.resolve(&anc), Err(DocError::UnknownTarget));
}

// ---------------------------------------------------------------------------------------
// Property test: 3 replicas, random byte-offset edits (multi-byte), random partial syncs,
// random anchors; then full sync and every check.

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const ALPHABET: [&str; 6] = ["a", "b", "é", "🎉", "\n", "z"];

fn random_hunks(rng: &mut Rng, text: &str) -> Vec<Hunk> {
    let bounds: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())).collect();
    let mut out = Vec::new();
    let mut min = 0; // index into bounds of the smallest allowed start
    for _ in 0..1 + rng.below(3) {
        if min >= bounds.len() {
            break;
        }
        let s = min + rng.below((bounds.len() - min).min(4));
        let e = (s + rng.below(3)).min(bounds.len() - 1);
        let ins: String = (0..rng.below(4)).map(|_| ALPHABET[rng.below(ALPHABET.len())]).collect();
        out.push(Hunk { start: bounds[s], end: bounds[e], insert: ins });
        min = e.max(s + 1);
    }
    out
}

fn run_seed(seed: u64) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let agents = [101u64, 102, 103];
    let mut reps = vec![Doc::replica(), Doc::replica(), Doc::replica()];
    let mut anchors: Vec<Anchor> = Vec::new();
    let mut ranges: Vec<(Anchor, Anchor)> = Vec::new();
    let mut snapshots: Vec<Vec<Id>> = Vec::new();
    for _ in 0..40 {
        let i = rng.below(3);
        match rng.below(10) {
            0..=4 => {
                let hunks = random_hunks(&mut rng, &reps[i].text());
                reps[i].edit_as(agents[i], &hunks).unwrap_or_else(|e| panic!("seed {seed}: {e:?} for {hunks:?}"));
            }
            5..=7 => {
                let j = (i + 1 + rng.below(2)) % 3;
                let diff = reps[i].diff_events(reps[j].frontier());
                let k = rng.below(diff.len() + 1);
                reps[j].deliver_all(&diff[..k]).unwrap_or_else(|e| panic!("seed {seed}: partial sync {e:?}"));
            }
            _ => {
                let t = reps[i].text();
                let bounds: Vec<usize> = t.char_indices().map(|(b, _)| b).chain(std::iter::once(t.len())).collect();
                let off = bounds[rng.below(bounds.len())];
                let side = if rng.below(2) == 0 { Side::Left } else { Side::Right };
                let a = reps[i].anchor(off, side).unwrap();
                assert_eq!(reps[i].resolve(&a), Ok(off), "seed {seed}: anchor round trip");
                anchors.push(a);
                // an expanding range [s, e): start sticks to the char before s, end to the char after e
                let off2 = bounds[rng.below(bounds.len())];
                let (s0, e0) = if off <= off2 { (off, off2) } else { (off2, off) };
                let rs = reps[i].anchor(s0, Side::Left).unwrap();
                let re = reps[i].anchor(e0, Side::Right).unwrap();
                ranges.push((rs, re));
            }
        }
        for d in &reps {
            snapshots.push(d.list_with_tombstones());
            for (rs, re) in &ranges {
                if let (Ok(a), Ok(b)) = (d.resolve(rs), d.resolve(re)) {
                    assert!(a <= b, "seed {seed}: anchored range inverted ({a} > {b})");
                }
            }
        }
    }
    // full sync
    for _ in 0..2 {
        for i in 0..3 {
            for j in 0..3 {
                if i != j {
                    let diff = reps[i].diff_events(reps[j].frontier());
                    reps[j].deliver_all(&diff).unwrap();
                }
            }
        }
    }
    let t = reps[0].text();
    let l = reps[0].list_with_tombstones();
    let b = reps[0].blame();
    for d in &reps[1..] {
        assert_eq!(d.text(), t, "seed {seed}: text");
        assert_eq!(d.list_with_tombstones(), l, "seed {seed}: list");
        assert_eq!(d.blame(), b, "seed {seed}: blame");
        assert_eq!(d.frontier(), reps[0].frontier(), "seed {seed}: frontier");
    }
    assert_eq!(b.iter().map(|(c, _)| *c).collect::<String>(), t);
    for a in &anchors {
        let r0 = reps[0].resolve(a).unwrap();
        assert!(t.is_char_boundary(r0), "seed {seed}");
        for d in &reps[1..] {
            assert_eq!(d.resolve(a), Ok(r0), "seed {seed}: anchor {a:?}");
        }
    }
    for (rs, re) in &ranges {
        for d in &reps {
            let (a, b) = (d.resolve(rs).unwrap(), d.resolve(re).unwrap());
            assert!(a <= b, "seed {seed}: anchored range inverted after full sync ({a} > {b})");
        }
    }
    let origins: BTreeMap<Id, _> = reps[0].origins();
    assert_eq!(def4(&l, &origins), Ok(()), "seed {seed}: Def. 4");
    snapshots.push(l);
    assert_eq!(strong_list_spec(&snapshots, &origins), Ok(()), "seed {seed}: strong list spec");
}

#[test]
fn property_three_replicas_random_edits_and_partial_syncs() {
    let n: u64 = std::env::var("MPCE_PROP_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    for seed in 0..n {
        run_seed(seed);
    }
}

#[test]
fn touching_hunks_apply_like_one_coalesced_hunk() {
    // Matches the kit's diff::validate: touching (n.start == prev.end) is fine unless both are pure
    // inserts at one offset; the result equals the single coalesced hunk (spec D2).
    let cases: &[(&[Hunk], Hunk)] = &[
        (&[h(2, 5, "X"), h(5, 5, "Y")], h(2, 5, "XY")), // replace then insert at its end
        (&[h(2, 5, ""), h(5, 7, "Z")], h(2, 7, "Z")),   // delete then replace
        (&[h(5, 5, "Y"), h(5, 7, "Z")], h(5, 7, "YZ")), // insert then replace from the same offset
        (&[h(2, 5, ""), h(5, 5, "Q")], h(2, 5, "Q")),   // delete then insert at its end
    ];
    for (split, coalesced) in cases {
        let mut a = Doc::replica();
        a.edit_as(101, &[h(0, 0, "abcdefgh")]).unwrap();
        let mut b = a.clone();
        a.edit_as(102, split).unwrap();
        b.edit_as(102, std::slice::from_ref(coalesced)).unwrap();
        assert_eq!(a.text(), b.text(), "split {:?} vs coalesced {:?}", split, coalesced);
    }
    let mut d = Doc::replica();
    d.edit_as(101, &[h(0, 0, "abcdefgh")]).unwrap();
    assert_eq!(d.edit_as(102, &[h(5, 5, "Y"), h(5, 5, "Z")]), Err(DocError::BadHunks));
    assert_eq!(d.edit_as(102, &[h(2, 6, "Y"), h(5, 7, "Z")]), Err(DocError::BadHunks));
}

#[test]
fn contracting_ranges_can_invert_so_ranges_must_expand() {
    // Why ranges use (start: Left, end: Right): the opposite convention inverts when someone
    // inserts into an empty range. This is the teeth for the property above.
    let mut a = Doc::replica();
    a.edit_as(101, &[h(0, 0, "ab")]).unwrap();
    let mut b = a.clone();
    // empty range at offset 1, between "a" and "b"
    let exp = (a.anchor(1, Side::Left).unwrap(), a.anchor(1, Side::Right).unwrap());
    let con = (a.anchor(1, Side::Right).unwrap(), a.anchor(1, Side::Left).unwrap());
    b.edit_as(102, &[h(1, 1, "X")]).unwrap();
    a.deliver_all(&b.diff_events(a.frontier())).unwrap();
    assert_eq!(a.text(), "aXb");
    let (s, e) = (a.resolve(&exp.0).unwrap(), a.resolve(&exp.1).unwrap());
    assert!(s <= e && (s, e) == (1, 2), "expanding range grows to cover the insert");
    let (s, e) = (a.resolve(&con.0).unwrap(), a.resolve(&con.1).unwrap());
    assert!(s > e, "contracting range inverts: ({s}, {e})");
}
