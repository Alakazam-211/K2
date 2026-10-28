//! v0.3.0 API: versions (`version`, `text_at`, `list_at`, `blame_at`), restore forward
//! (`restore_as`), branches (`fork_at`, `merge_from`), purge (`purge_as`, `find_content`); and
//! seeded property tests for the kit's rules `hist.version_stable`, `hist.restore_forward`,
//! `hist.branch_merge`, `hist.merge_any_base`, `hist.blame_grain`, `erase.purge`,
//! `erase.no_readmit` and `erase.versions`, with a byte scan for a purged secret and its
//! planted negative (the `PurgeRule::KeepContent` mutant must be caught).

use mpce_model::check::{def4, strong_list_spec};
use mpce_model::text::{Doc, DocError, Event, Hunk, Id, Op, PurgeRule, Version};
use std::collections::{BTreeMap, BTreeSet};

fn h(start: usize, end: usize, s: &str) -> Hunk {
    Hunk { start, end, insert: s.to_string() }
}

fn snapshot(d: &Doc) -> (String, Vec<Id>, BTreeSet<Id>, usize) {
    (d.text(), d.list_with_tombstones(), d.frontier().clone(), d.log().len())
}

fn purged(d: &Doc) -> BTreeSet<Id> {
    d.list_with_tombstones().into_iter().filter(|x| d.is_purged(*x)).collect()
}

fn deleted(d: &Doc) -> BTreeSet<Id> {
    d.list_with_tombstones().into_iter().filter(|x| d.is_deleted(*x)).collect()
}

/// Same state: text, list with tombstones, frontier, blame, deleted and purged sets.
fn same(a: &Doc, b: &Doc, what: &str) {
    assert_eq!(a.text(), b.text(), "{what}: text");
    assert_eq!(a.list_with_tombstones(), b.list_with_tombstones(), "{what}: list");
    assert_eq!(a.frontier(), b.frontier(), "{what}: frontier");
    assert_eq!(a.blame(), b.blame(), "{what}: blame");
    assert_eq!(deleted(a), deleted(b), "{what}: deleted");
    assert_eq!(purged(a), purged(b), "{what}: purged");
}

fn insert_ids(evs: &[Event]) -> Vec<Id> {
    evs.iter().filter(|e| matches!(e.op, Op::Insert { .. })).map(|e| e.id).collect()
}

fn value_of(d: &Doc, id: Id) -> Option<char> {
    match d.event(id).map(|e| &e.op) {
        Some(Op::Insert { value, .. }) => *value,
        _ => panic!("{id:?} is not an insert here"),
    }
}

/// Every char literal (`'X'`) in a Debug string, concatenated: storage keeps one character
/// per event, so this is how a leaked string shows up in Debug output.
fn char_stream(debug: &str) -> String {
    let v: Vec<char> = debug.chars().collect();
    let mut out = String::new();
    for i in 0..v.len().saturating_sub(2) {
        if v[i] == '\'' && v[i + 2] == '\'' && v[i + 1] != '\\' && v[i + 1] != '\'' {
            out.push(v[i + 1]);
        }
    }
    out
}

fn has_uppercase_literal(debug: &str) -> bool {
    char_stream(debug).chars().any(|c| c.is_ascii_uppercase())
}

// ---------------------------------------------------------------------------------------
// Versions

#[test]
fn versions_reproduce_their_state() {
    let mut d = Doc::replica();
    let v0 = d.version();
    d.edit_as(101, &[h(0, 0, "héllo 🎉\nworld")]).unwrap();
    let v1 = d.version();
    // "é" is bytes 1..3, "🎉" 7..11
    d.edit_as(102, &[h(1, 3, "e"), h(7, 11, "")]).unwrap();
    let v2 = d.version();
    assert_eq!(d.text(), "hello \nworld");
    d.edit_as(101, &[h(0, 0, "> ")]).unwrap();
    assert_eq!(d.text_at(&v0).unwrap(), "");
    assert_eq!(d.text_at(&v1).unwrap(), "héllo 🎉\nworld");
    assert_eq!(d.text_at(&v2).unwrap(), "hello \nworld");
    assert_eq!(d.text_at(&d.version()).unwrap(), d.text());
    assert_eq!(d.list_at(&d.version()).unwrap(), d.visible());
    // a non-antichain version stands for its closure too
    let both: Version = v1.union(&v2).copied().collect();
    assert_eq!(d.text_at(&both).unwrap(), "hello \nworld");
    // fork_at replays the same state
    for v in [&v0, &v1, &v2] {
        let f = d.fork_at(v).unwrap();
        assert_eq!(f.text(), d.text_at(v).unwrap());
        assert_eq!(f.visible(), d.list_at(v).unwrap());
        assert_eq!(&f.version(), v);
    }
    // blame at a version (E-HIST-4: per character)
    assert!(d.blame_at(&v1).unwrap().iter().all(|(_, id)| id.agent == 101));
    let b2 = d.blame_at(&v2).unwrap();
    assert_eq!(b2.iter().map(|(c, _)| *c).collect::<String>(), "hello \nworld");
    for (c, id) in &b2 {
        assert_eq!(id.agent, if *c == 'e' { 102 } else { 101 }, "{c}");
    }
    // a version naming an unknown event is refused
    let bad: Version = BTreeSet::from([Id::new(9, 9)]);
    assert_eq!(d.text_at(&bad), Err(DocError::UnknownVersion { id: Id::new(9, 9) }));
    assert_eq!(d.list_at(&bad), Err(DocError::UnknownVersion { id: Id::new(9, 9) }));
    assert!(d.fork_at(&bad).is_err());
    assert!(d.blame_at(&bad).is_err());
}

#[test]
fn versions_survive_concurrent_edits_and_syncs() {
    let mut a = Doc::replica();
    a.edit_as(101, &[h(0, 0, "base")]).unwrap();
    let mut b = a.clone();
    a.edit_as(101, &[h(4, 4, " é")]).unwrap();
    b.edit_as(102, &[h(0, 0, "🎉 ")]).unwrap();
    let (va, vb) = (a.version(), b.version());
    let (ta, tb) = (a.text(), b.text());
    let a0 = a.clone();
    a.merge_from(&b).unwrap();
    b.merge_from(&a0).unwrap();
    a.edit_as(101, &[h(0, a.text().len(), "")]).unwrap();
    b.merge_from(&a).unwrap();
    for d in [&a, &b] {
        assert_eq!(d.text_at(&va).unwrap(), ta);
        assert_eq!(d.text_at(&vb).unwrap(), tb);
        assert_eq!(d.text(), "");
    }
    let merged: Version = va.union(&vb).copied().collect();
    assert_eq!(a.text_at(&merged).unwrap(), "🎉 base é");
}

// ---------------------------------------------------------------------------------------
// Restore forward

#[test]
fn restore_moves_forward_with_new_events() {
    let mut d = Doc::replica();
    d.edit_as(101, &[h(0, 0, "héllo 🎉\nworld")]).unwrap();
    let v1 = d.version();
    // h(0..1) é(1..3) l l o ' '(6) 🎉(7..11) \n(11) world(12..17)
    d.edit_as(102, &[h(0, 1, "J"), h(7, 11, "!"), h(12, 17, "")]).unwrap();
    assert_eq!(d.text(), "Jéllo !\n");
    let before = d.log().to_vec();
    let u = d.version();
    let ev = d.restore_as(103, &v1).unwrap();
    assert_eq!(d.text(), "héllo 🎉\nworld");
    assert_eq!(d.text(), d.text_at(&v1).unwrap());
    // history is never rewritten: the old log is a prefix, the restore is new events by 103
    assert_eq!(&d.log()[..before.len()], &before[..]);
    assert!(!ev.is_empty());
    assert!(ev.iter().all(|e| e.id.agent == 103 && !before.iter().any(|b| b.id == e.id)));
    // re-inserted text is new content attributed to the restorer (U18a); kept text keeps its blame
    for (c, id) in d.blame() {
        match c {
            'h' | '🎉' | 'w' | 'r' | 'd' => assert_eq!(id.agent, 103, "{c}"),
            'é' | ' ' | '\n' => assert_eq!(id.agent, 101, "{c}"),
            _ => {}
        }
    }
    // round trip: restore the pre-restore version
    d.restore_as(103, &u).unwrap();
    assert_eq!(d.text(), "Jéllo !\n");
    assert_eq!(d.text(), d.text_at(&u).unwrap());
    // restoring the current version does nothing
    assert_eq!(d.restore_as(103, &d.version()), Ok(vec![]));
    // an unknown version is refused, atomically
    let s = snapshot(&d);
    assert!(d.restore_as(103, &BTreeSet::from([Id::new(9, 9)])).is_err());
    assert_eq!(snapshot(&d), s);
}

#[test]
fn restore_brings_purged_content_back_as_nothing() {
    let mut d = Doc::replica();
    d.edit_as(101, &[h(0, 0, "keep SECRET keep")]).unwrap();
    let v1 = d.version();
    d.edit_as(101, &[h(5, 12, "")]).unwrap();
    assert_eq!(d.text(), "keep keep");
    let occ = d.find_content("SECRET");
    assert_eq!(occ.len(), 1);
    d.purge_as(102, &occ[0]).unwrap();
    assert_eq!(d.text_at(&v1).unwrap(), "keep  keep");
    d.restore_as(103, &v1).unwrap();
    assert_eq!(d.text(), "keep  keep");
    assert!(d.find_content("SECRET").is_empty());
}

// ---------------------------------------------------------------------------------------
// Branches

#[test]
fn branches_fork_at_any_version_and_merge() {
    let mut main = Doc::new(101);
    main.edit_as(101, &[h(0, 0, "abc")]).unwrap();
    let v1 = main.version();
    main.edit_as(101, &[h(3, 3, "déf")]).unwrap();
    let mut br = main.fork_at(&v1).unwrap();
    assert_eq!(br.text(), "abc");
    assert_eq!(br.agent(), Some(101));
    // the same agent edits the branch: its seqs continue after the origin's, no reuse
    let ev = br.edit_as(101, &[h(1, 1, "🎉")]).unwrap();
    assert_eq!(ev[0].id, Id::new(101, 6));
    main.edit_as(102, &[h(0, 1, "")]).unwrap();
    let mut direct = main.clone();
    direct.deliver_all(&br.diff_events(main.frontier())).unwrap();
    assert_eq!(main.merge_from(&br), Ok(1));
    same(&main, &direct, "merge = direct delivery");
    assert_eq!(main.text(), "🎉bcdéf");
    let s = snapshot(&main);
    assert_eq!(main.merge_from(&br), Ok(0));
    assert_eq!(snapshot(&main), s);
    br.merge_from(&main).unwrap();
    same(&br, &main, "branch catches up");
    // a branch from the empty version merges too
    let mut root = main.fork_at(&Version::new()).unwrap();
    assert_eq!(root.text(), "");
    root.edit_as(201, &[h(0, 0, "\n")]).unwrap();
    main.merge_from(&root).unwrap();
    assert!(main.text().contains('\n'));
}

#[test]
fn a_fork_keeps_content_purged_before_the_fork() {
    let mut d = Doc::replica();
    d.edit_as(101, &[h(0, 0, "a SECRET b")]).unwrap();
    let v1 = d.version();
    let occ = d.find_content("SECRET");
    d.purge_as(101, &occ[0]).unwrap();
    let f = d.fork_at(&v1).unwrap();
    assert_eq!(f.text(), "a  b");
    assert!(occ[0].iter().all(|x| f.is_purged(*x) && !f.is_deleted(*x)));
    assert!(!has_uppercase_literal(&format!("{f:?}")));
}

// ---------------------------------------------------------------------------------------
// Purge

#[test]
fn purge_deletes_erases_and_propagates_without_readmission() {
    let mut a = Doc::replica();
    a.edit_as(101, &[h(0, 0, "pré SECRET post")]).unwrap();
    let mut b = Doc::replica();
    b.merge_from(&a).unwrap();
    let v1 = a.version();
    let originals: Vec<Event> = a.log().to_vec();
    let occ = a.find_content("SECRET");
    assert_eq!(occ.len(), 1);
    assert_eq!(occ[0].len(), 6);
    let p = a.purge_as(102, &occ[0]).unwrap();
    assert_eq!(p.op, Op::Purge { targets: occ[0].iter().copied().collect() });
    assert_eq!(p.parents, v1);
    assert_eq!(p.id, Id::new(102, 0));
    assert_eq!(a.text(), "pré  post");
    assert_eq!(a.text_at(&v1).unwrap(), "pré  post");
    for x in &occ[0] {
        assert!(a.is_purged(*x) && a.is_deleted(*x));
        assert_eq!(value_of(&a, *x), None);
    }
    assert!(a.find_content("SECRET").is_empty());
    assert!(!has_uppercase_literal(&format!("{a:?}")));
    // it propagates like any event; the receiver erases too
    assert_eq!(b.merge_from(&a), Ok(1));
    same(&a, &b, "after purge sync");
    assert!(!has_uppercase_literal(&format!("{b:?}")));
    // a fresh replica receives content-less inserts
    let mut c = Doc::replica();
    let all = a.diff_events(&BTreeSet::new());
    assert!(!has_uppercase_literal(&format!("{all:?}")));
    c.deliver_all(&all).unwrap();
    same(&a, &c, "fresh replica");
    // no re-admission: the original content-bearing events are known, their content dropped
    for d in [&mut a, &mut b, &mut c] {
        let s = snapshot(d);
        assert_eq!(d.deliver_all(&originals), Ok(0));
        for o in &originals {
            assert_eq!(d.deliver(o), Ok(false));
        }
        assert_eq!(d.sync_from(&Doc::replica()), Ok(0));
        assert_eq!(snapshot(d), s);
        assert!(!has_uppercase_literal(&format!("{d:?}")));
    }
    // any other difference under a known ID is still a conflict
    let mut forged = originals[0].clone(); // 'p', not purged
    if let Op::Insert { value, .. } = &mut forged.op {
        *value = Some('Z');
    }
    assert_eq!(a.deliver(&forged), Err(DocError::Conflict));
    let mut forged = originals.iter().find(|e| e.id == occ[0][1]).unwrap().clone();
    forged.parents.clear();
    assert_eq!(a.deliver(&forged), Err(DocError::Conflict));
}

#[test]
fn a_content_less_insert_before_its_purge_and_content_arriving_late() {
    let mut a = Doc::replica();
    let ins = a.edit_as(101, &[h(0, 0, "xSECRETy")]).unwrap();
    let ids = insert_ids(&ins[1..7]);
    let t = a.clone(); // a replica that has the content and never sees the purge
    a.purge_as(101, &ids).unwrap();
    let diff = a.diff_events(&BTreeSet::new());
    assert_eq!(diff.len(), 9);
    // b receives the content-less inserts but not (yet) the purge event
    let mut b = Doc::replica();
    b.deliver_all(&diff[..8]).unwrap();
    assert_eq!(b.text(), "xy");
    assert!(ids.iter().all(|x| b.is_purged(*x) && !b.is_deleted(*x)));
    // the content arrives later from t: accepted as known and dropped
    assert_eq!(b.merge_from(&t), Ok(0));
    assert_eq!(b.deliver_all(t.log()), Ok(0));
    assert!(ids.iter().all(|x| b.is_purged(*x)));
    assert!(!has_uppercase_literal(&format!("{b:?}")));
    // a content-less copy reaching t erases there too (purge knowledge only grows)
    let mut t2 = t.clone();
    assert_eq!(t2.deliver_all(&diff[..8]), Ok(0));
    assert!(ids.iter().all(|x| t2.is_purged(*x)));
    assert_eq!(t2.text(), "xy");
    // the purge event completes it everywhere
    b.merge_from(&a).unwrap();
    t2.merge_from(&a).unwrap();
    let mut t3 = t.clone();
    t3.merge_from(&a).unwrap();
    same(&a, &b, "b");
    same(&a, &t2, "t2");
    same(&a, &t3, "t3");
}

#[test]
fn concurrent_inserts_beside_and_inside_a_purged_span_survive() {
    let mut a = Doc::replica();
    a.edit_as(101, &[h(0, 0, "xxSECRETyy")]).unwrap();
    let mut b = a.clone();
    b.edit_as(102, &[h(2, 2, "<"), h(5, 5, "é🎉"), h(8, 8, ">")]).unwrap();
    assert_eq!(b.text(), "xx<SECé🎉RET>yy");
    let occ = a.find_content("SECRET");
    a.purge_as(101, &occ[0]).unwrap();
    // purge then merge, merge then purge, both directions
    let mut a1 = a.clone();
    a1.merge_from(&b).unwrap();
    let mut b1 = b.clone();
    b1.merge_from(&a).unwrap();
    same(&a1, &b1, "purge/merge");
    assert_eq!(a1.text(), "xx<é🎉>yy");
}

#[test]
fn purge_refusals_change_nothing() {
    let mut d = Doc::replica();
    let ev = d.edit_as(101, &[h(0, 0, "ab")]).unwrap();
    let del = d.edit_as(101, &[h(0, 1, "")]).unwrap();
    let s = snapshot(&d);
    assert_eq!(d.purge_as(102, &[]), Err(DocError::EmptyPurge));
    assert_eq!(d.purge_as(102, &[Id::new(9, 9)]), Err(DocError::UnknownTarget));
    assert_eq!(d.purge_as(102, &[ev[1].id, del[0].id]), Err(DocError::UnknownTarget));
    assert_eq!(snapshot(&d), s);
    // deleted and already purged targets are fine; the agent's seq did not advance on refusal
    let p = d.purge_as(102, &[ev[0].id]).unwrap();
    assert_eq!(p.id, Id::new(102, 0));
    let p2 = d.purge_as(102, &[ev[0].id]).unwrap();
    assert_eq!(p2.id, Id::new(102, 1));
    assert_eq!(d.text(), "b");
    // a received empty purge is refused
    let mut bad = p.clone();
    bad.id = Id::new(103, 0);
    bad.op = Op::Purge { targets: BTreeSet::new() };
    assert_eq!(d.deliver(&bad), Err(DocError::EmptyPurge));
}

#[test]
fn find_content_finds_whole_split_typo_fixed_and_deleted_text() {
    let mut d = Doc::replica();
    d.edit_as(101, &[h(0, 0, "a SECRET b")]).unwrap();
    // split across two edits
    let n = d.text().len();
    d.edit_as(102, &[h(n, n, " HAL")]).unwrap();
    let n = d.text().len();
    d.edit_as(102, &[h(n, n, "VES")]).unwrap();
    // a typo fixed mid-word: the tombstone 'q' sits between TY and PO in the list
    let n = d.text().len();
    d.edit_as(103, &[h(n, n, " TYqPO")]).unwrap();
    d.edit_as(103, &[h(n + 3, n + 4, "")]).unwrap();
    // typed, then deleted
    let n = d.text().len();
    d.edit_as(104, &[h(n, n, " GONE")]).unwrap();
    d.edit_as(104, &[h(n, n + 5, "")]).unwrap();
    // multi-byte
    let n = d.text().len();
    d.edit_as(105, &[h(n, n, " é🎉\n")]).unwrap();
    assert_eq!(d.text(), "a SECRET b HALVES TYPO é🎉\n");
    for needle in ["SECRET", "HALVES", "TYPO", "GONE", "é🎉\n"] {
        let occ = d.find_content(needle);
        assert_eq!(occ.len(), 1, "{needle}: {occ:?}");
        let s: String = occ[0].iter().map(|x| value_of(&d, *x).unwrap()).collect();
        assert_eq!(s, needle);
    }
    assert!(d.find_content("").is_empty());
    assert!(d.find_content("absent").is_empty());
    // occurrences are sorted by where they start
    let occ = d.find_content("E");
    assert!(occ.len() >= 3);
    let order = d.list_with_tombstones();
    let starts: Vec<usize> = occ.iter().map(|o| order.iter().position(|x| *x == o[0]).unwrap()).collect();
    assert!(starts.windows(2).all(|w| w[0] < w[1]));
    for needle in ["SECRET", "HALVES", "TYPO", "GONE"] {
        let ids: Vec<Id> = d.find_content(needle).concat();
        d.purge_as(106, &ids).unwrap();
        assert!(d.find_content(needle).is_empty());
    }
    assert_eq!(d.text(), "a  b   é🎉\n");
    assert!(!has_uppercase_literal(&format!("{d:?}")));
}

// ---------------------------------------------------------------------------------------
// Property tests

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

fn seeds(default: u64) -> u64 {
    std::env::var("MPCE_PROP_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

/// Lowercase and multi-byte only: an uppercase secret cannot occur by chance.
const ALPHABET: [&str; 6] = ["a", "b", "é", "🎉", "\n", "z"];

fn bounds(text: &str) -> Vec<usize> {
    text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())).collect()
}

fn random_hunks(rng: &mut Rng, text: &str) -> Vec<Hunk> {
    let bounds = bounds(text);
    let mut out = Vec::new();
    let mut min = 0;
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

/// A random run of 1..=4 elements of the list (tombstones included), or none if empty.
fn random_run(rng: &mut Rng, d: &Doc) -> Vec<Id> {
    let l = d.list_with_tombstones();
    if l.is_empty() {
        return vec![];
    }
    let s = rng.below(l.len());
    l[s..(s + 1 + rng.below(4)).min(l.len())].to_vec()
}

fn full_sync(reps: &mut [Doc]) {
    for _ in 0..3 {
        for i in 0..reps.len() {
            for j in 0..reps.len() {
                if i != j {
                    let from = reps[i].clone();
                    reps[j].merge_from(&from).unwrap();
                }
            }
        }
    }
}

/// The byte scan: every 8-byte window of `secret` (so every substring of >= 8 bytes) is
/// looked for in each replica's text, its text at every version seen, the Debug of every
/// event in `log()` and in `diff_events(&empty)`, and the Debug of the whole replica; each
/// Debug both raw and as its char-literal stream. Returns where it was found.
fn byte_scan(secret: &str, reps: &[Doc], versions: &[(Version, Vec<(char, Id)>)]) -> Vec<String> {
    let windows: Vec<&str> = (0..=secret.len() - 8).map(|i| &secret[i..i + 8]).collect();
    let hit = |hay: &str| windows.iter().any(|w| hay.contains(w));
    let mut leaks = Vec::new();
    for (r, d) in reps.iter().enumerate() {
        let mut places: Vec<(String, String)> = vec![("text()".into(), d.text())];
        for (k, (v, _)) in versions.iter().enumerate() {
            if let Ok(t) = d.text_at(v) {
                places.push((format!("text_at(version {k})"), t));
            }
        }
        let log: String = d.log().iter().map(|e| format!("{e:?}")).collect();
        let diff: String = d.diff_events(&BTreeSet::new()).iter().map(|e| format!("{e:?}")).collect();
        let dbg = format!("{d:?}");
        for (name, s) in [("log()", log), ("diff_events(empty)", diff), ("Debug(doc)", dbg)] {
            places.push((format!("{name} char stream"), char_stream(&s)));
            places.push((name.into(), s));
        }
        for (name, s) in places {
            if hit(&s) {
                leaks.push(format!("replica {r}: {name}"));
            }
        }
    }
    leaks
}

/// erase.purge / erase.no_readmit / erase.versions / hist.version_stable / convergence
/// through purge: 3 replicas, random edits, partial syncs, random purges, re-delivery of old
/// content-bearing events, versions, restores, purge∘merge = merge∘purge checks; a 32-letter
/// secret (whole, split across two edits, or typo-fixed; maybe deleted) is purged by its
/// author. Returns the byte scan's findings (empty unless the purge rule is the mutant).
fn run_purge_seed(seed: u64, rule: PurgeRule) -> Vec<String> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let agents = [101u64, 102, 103];
    let mut reps: Vec<Doc> = (0..3)
        .map(|_| {
            let mut d = Doc::replica();
            d.set_purge_rule(rule);
            d
        })
        .collect();
    let secret: String = (0..32).map(|_| (b'A' + rng.below(26) as u8) as char).collect();
    let mut secret_ids: Vec<Id> = Vec::new();
    let mut pool: Vec<Event> = Vec::new(); // every event as created (content-bearing)
    let mut versions: Vec<(Version, Vec<(char, Id)>)> = vec![(Version::new(), vec![])];
    let mut snapshots: Vec<Vec<Id>> = Vec::new();
    let plant_at = rng.below(15);
    let purge_at = plant_at + rng.below(30);
    let owner = rng.below(3);
    for step in 0..50 {
        if step == plant_at {
            let d = &mut reps[owner];
            let a = agents[owner];
            let b = bounds(&d.text());
            let off = b[rng.below(b.len())];
            let k = 1 + rng.below(31);
            let mut evs = Vec::new();
            match rng.below(3) {
                0 => evs.extend(d.edit_as(a, &[h(off, off, &secret)]).unwrap()),
                1 => {
                    evs.extend(d.edit_as(a, &[h(off, off, &secret[..k])]).unwrap());
                    evs.extend(d.edit_as(a, &[h(off + k, off + k, &secret[k..])]).unwrap());
                }
                _ => {
                    let typo = format!("{}q{}", &secret[..k], &secret[k..]);
                    evs.extend(d.edit_as(a, &[h(off, off, &typo)]).unwrap());
                    evs.extend(d.edit_as(a, &[h(off + k, off + k + 1, "")]).unwrap());
                }
            }
            assert!(d.text().contains(&secret), "seed {seed}: planted");
            if rng.below(3) == 0 {
                evs.extend(d.edit_as(a, &[h(off, off + 32, "")]).unwrap());
            }
            secret_ids = evs.iter().filter(|e| matches!(e.op, Op::Insert { value: Some(c), .. } if c.is_ascii_uppercase())).map(|e| e.id).collect();
            assert_eq!(secret_ids.len(), 32);
            pool.extend(evs);
        }
        if step == purge_at {
            let d = &mut reps[owner];
            let occ = d.find_content(&secret);
            let want: BTreeSet<Id> = secret_ids.iter().copied().collect();
            if !secret_ids.iter().any(|x| d.is_purged(*x)) {
                assert!(occ.iter().any(|o| o.iter().copied().collect::<BTreeSet<_>>() == want), "seed {seed}: find_content {occ:?}");
            }
            let mut ids: BTreeSet<Id> = occ.into_iter().flatten().collect();
            ids.extend(want);
            let ids: Vec<Id> = ids.into_iter().collect();
            pool.push(d.purge_as(agents[owner], &ids).unwrap());
            if rule == PurgeRule::Erase {
                assert!(d.find_content(&secret).is_empty(), "seed {seed}");
                let one = std::slice::from_ref(&reps[owner]);
                assert_eq!(byte_scan(&secret, one, &versions), Vec::<String>::new(), "seed {seed}: right after the purge");
            }
        }
        let i = rng.below(3);
        let j = (i + 1 + rng.below(2)) % 3;
        match rng.below(16) {
            0..=5 => {
                let hunks = random_hunks(&mut rng, &reps[i].text());
                let evs = reps[i].edit_as(agents[i], &hunks).unwrap_or_else(|e| panic!("seed {seed}: {e:?} for {hunks:?}"));
                pool.extend(evs);
            }
            6..=8 => {
                let diff = reps[i].diff_events(reps[j].frontier());
                let k = rng.below(diff.len() + 1);
                reps[j].deliver_all(&diff[..k]).unwrap_or_else(|e| panic!("seed {seed}: partial sync {e:?}"));
            }
            9 => {
                let ids = random_run(&mut rng, &reps[i]);
                if !ids.is_empty() {
                    pool.push(reps[i].purge_as(agents[i], &ids).unwrap());
                }
            }
            10 | 11 => {
                // re-deliver old content-bearing events (known ones and deliverable new ones)
                let mut have: BTreeSet<Id> = reps[i].log().iter().map(|e| e.id).collect();
                let mut chosen = Vec::new();
                for e in &pool {
                    if rng.below(3) == 0 && (have.contains(&e.id) || e.parents.iter().all(|p| have.contains(p))) {
                        have.insert(e.id);
                        chosen.push(e.clone());
                    }
                }
                let before = purged(&reps[i]);
                reps[i].deliver_all(&chosen).unwrap_or_else(|e| panic!("seed {seed}: re-delivery {e:?}"));
                assert!(before.is_subset(&purged(&reps[i])), "seed {seed}: re-admitted purged content");
            }
            12 => {
                let v = reps[i].version();
                assert_eq!(reps[i].text_at(&v).unwrap(), reps[i].text());
                versions.push((v.clone(), reps[i].blame_at(&v).unwrap()));
            }
            13 => {
                // purge∘merge = merge∘purge, the purge at any point among the merged events
                let ids = random_run(&mut rng, &reps[i]);
                if ids.is_empty() {
                    continue;
                }
                // (the same event copies in every order: `diff` is fixed before the purge, as
                // a frontier naming the unknown purge would make `j` send its whole log)
                let diff = reps[j].diff_events(reps[i].frontier());
                let mut a1 = reps[i].clone();
                let p = a1.purge_as(agents[i], &ids).unwrap();
                a1.deliver_all(&diff).unwrap();
                let mut a2 = reps[i].clone();
                a2.deliver_all(&diff).unwrap();
                a2.deliver_all(std::slice::from_ref(&p)).unwrap();
                let k = rng.below(diff.len() + 1);
                let mut a3 = reps[i].clone();
                a3.deliver_all(&diff[..k]).unwrap();
                a3.deliver_all(std::slice::from_ref(&p)).unwrap();
                a3.deliver_all(&diff[k..]).unwrap();
                same(&a1, &a2, &format!("seed {seed}: purge then merge vs merge then purge"));
                same(&a1, &a3, &format!("seed {seed}: purge in the middle of a merge"));
                if rng.below(2) == 0 {
                    reps[i] = a1;
                    pool.push(p);
                }
            }
            14 => {
                // restore to a version this replica knows; only where no unpurged secret
                // content is held (a restore there would re-type the secret as new content,
                // which the secret's purge rightly does not cover)
                if secret_ids.iter().any(|x| reps[i].has(*x) && !reps[i].is_purged(*x)) {
                    continue;
                }
                let known: Vec<&Version> = versions.iter().map(|(v, _)| v).filter(|v| v.iter().all(|x| reps[i].has(*x))).collect();
                let v = known[rng.below(known.len())].clone();
                let evs = reps[i].restore_as(agents[i], &v).unwrap();
                assert_eq!(reps[i].text(), reps[i].text_at(&v).unwrap(), "seed {seed}: restore");
                pool.extend(evs);
            }
            _ => {
                let from = reps[i].clone();
                reps[j].merge_from(&from).unwrap();
            }
        }
        for d in &reps {
            snapshots.push(d.list_with_tombstones());
        }
    }
    full_sync(&mut reps);
    for d in &reps[1..] {
        same(&reps[0], d, &format!("seed {seed}: convergence"));
    }
    if rule == PurgeRule::Erase {
        for d in &reps {
            assert!(secret_ids.iter().all(|x| d.is_purged(*x) && d.is_deleted(*x)), "seed {seed}: secret purged everywhere");
        }
    }
    let l = reps[0].list_with_tombstones();
    let origins: BTreeMap<Id, _> = reps[0].origins();
    assert_eq!(def4(&l, &origins), Ok(()), "seed {seed}: Def. 4");
    snapshots.push(l);
    assert_eq!(strong_list_spec(&snapshots, &origins), Ok(()), "seed {seed}: strong list spec");
    // hist.version_stable / erase.versions: each version reads as it did, minus purged content
    for (k, (v, blame)) in versions.iter().enumerate() {
        for d in &reps {
            let want: Vec<(char, Id)> = blame.iter().copied().filter(|(_, x)| !d.is_purged(*x)).collect();
            assert_eq!(d.blame_at(v).unwrap(), want, "seed {seed}: version {k}");
        }
        let f = reps[0].fork_at(v).unwrap();
        assert_eq!(f.text(), reps[0].text_at(v).unwrap(), "seed {seed}: text_at = fork_at(v).text()");
        assert_eq!(f.list_with_tombstones().len(), reps[0].list_with_tombstones().iter().filter(|x| f.has(**x)).count());
    }
    let mut leaks = byte_scan(&secret, &reps, &versions);
    if rule == PurgeRule::Erase {
        for (r, d) in reps.iter().enumerate() {
            if !d.find_content(&secret).is_empty() {
                leaks.push(format!("replica {r}: find_content"));
            }
            if has_uppercase_literal(&format!("{d:?}")) {
                leaks.push(format!("replica {r}: a secret character in Debug(doc)"));
            }
        }
    }
    leaks
}

#[test]
fn property_purge_converges_and_leaves_no_trace() {
    for seed in 0..seeds(200) {
        let leaks = run_purge_seed(seed, PurgeRule::Erase);
        assert!(leaks.is_empty(), "seed {seed}: the purged secret is still in {leaks:?}");
    }
}

#[test]
fn planted_negative_the_byte_scan_catches_a_purge_that_keeps_content() {
    let n = seeds(200);
    let mut places: BTreeMap<String, usize> = BTreeMap::new();
    for seed in 0..n {
        let leaks = run_purge_seed(seed, PurgeRule::KeepContent);
        assert!(!leaks.is_empty(), "seed {seed}: the scan missed a purge that kept the content");
        let mut here = BTreeSet::new();
        for l in leaks {
            let place = l.split_once(": ").map_or(l.clone(), |(_, p)| p.to_string());
            here.insert(if place.starts_with("text_at") { "text_at(some version)".to_string() } else { place });
        }
        for place in here {
            *places.entry(place).or_default() += 1;
        }
    }
    eprintln!("planted negative: {n}/{n} seeds caught; seeds caught per place: {places:?}");
}

/// hist.branch_merge / hist.merge_any_base / hist.restore_forward: two main replicas edit and
/// sync; branches fork at random past versions (of main or of another branch) and edit as
/// their own agents; restores and purges mix in. Every merge equals direct delivery and is
/// idempotent; everyone converges; versions stay stable.
fn run_branch_seed(seed: u64) {
    let mut rng = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
    let mut reps: Vec<(u64, Doc)> = vec![(101, Doc::replica()), (102, Doc::replica())];
    let mut versions: Vec<(Version, Vec<(char, Id)>)> = vec![(Version::new(), vec![])];
    let mut snapshots: Vec<Vec<Id>> = Vec::new();
    for _ in 0..40 {
        let i = rng.below(reps.len());
        let (agent, _) = reps[i];
        match rng.below(12) {
            0..=4 => {
                let hunks = random_hunks(&mut rng, &reps[i].1.text());
                reps[i].1.edit_as(agent, &hunks).unwrap_or_else(|e| panic!("seed {seed}: {e:?}"));
            }
            5 | 6 => {
                let j = rng.below(reps.len());
                let diff = reps[i].1.diff_events(reps[j].1.frontier());
                let k = rng.below(diff.len() + 1);
                reps[j].1.deliver_all(&diff[..k]).unwrap();
            }
            7 => {
                let v = reps[i].1.version();
                versions.push((v.clone(), reps[i].1.blame_at(&v).unwrap()));
            }
            8 | 9 => {
                // fork at any past version this replica knows
                let known: Vec<&Version> = versions.iter().map(|(v, _)| v).filter(|v| v.iter().all(|x| reps[i].1.has(*x))).collect();
                let v = known[rng.below(known.len())].clone();
                let f = reps[i].1.fork_at(&v).unwrap();
                assert_eq!(f.text(), reps[i].1.text_at(&v).unwrap(), "seed {seed}: fork text");
                assert_eq!(f.version(), v, "seed {seed}: fork version");
                assert!(f.log().iter().all(|e| reps[i].1.event(e.id) == Some(e)), "seed {seed}: fork events");
                reps.push((200 + reps.len() as u64, f));
            }
            10 => {
                let known: Vec<&Version> = versions.iter().map(|(v, _)| v).filter(|v| v.iter().all(|x| reps[i].1.has(*x))).collect();
                let v = known[rng.below(known.len())].clone();
                let d = &mut reps[i].1;
                let u = d.version();
                let before = d.log().to_vec();
                let evs = d.restore_as(agent, &v).unwrap();
                assert_eq!(d.text(), d.text_at(&v).unwrap(), "seed {seed}: restore_forward");
                assert_eq!(&d.log()[..before.len()], &before[..], "seed {seed}: history kept");
                assert!(evs.iter().all(|e| e.id.agent == agent && !before.iter().any(|b| b.id == e.id)));
                if rng.below(2) == 0 {
                    d.restore_as(agent, &u).unwrap();
                    assert_eq!(d.text(), d.text_at(&u).unwrap(), "seed {seed}: restore round trip");
                }
            }
            _ => {
                let ids = random_run(&mut rng, &reps[i].1);
                if !ids.is_empty() {
                    reps[i].1.purge_as(agent, &ids).unwrap();
                }
            }
        }
        for (_, d) in &reps {
            snapshots.push(d.list_with_tombstones());
        }
    }
    // merge every branch into main 0, in random order
    let mut order: Vec<usize> = (1..reps.len()).collect();
    for k in (1..order.len()).rev() {
        order.swap(k, rng.below(k + 1));
    }
    for &b in &order {
        let br = reps[b].1.clone();
        let main = &mut reps[0].1;
        let mut direct = main.clone();
        direct.deliver_all(&br.diff_events(main.frontier())).unwrap();
        main.merge_from(&br).unwrap_or_else(|e| panic!("seed {seed}: merge {e:?}"));
        same(main, &direct, &format!("seed {seed}: merge = direct delivery"));
        let s = snapshot(main);
        assert_eq!(main.merge_from(&br), Ok(0), "seed {seed}: idempotent");
        assert_eq!(snapshot(main), s);
    }
    let main = reps[0].1.clone();
    for (_, d) in reps.iter_mut() {
        d.merge_from(&main).unwrap();
        same(d, &main, &format!("seed {seed}: convergence"));
    }
    let l = main.list_with_tombstones();
    let origins: BTreeMap<Id, _> = main.origins();
    assert_eq!(def4(&l, &origins), Ok(()), "seed {seed}: Def. 4");
    snapshots.push(l);
    assert_eq!(strong_list_spec(&snapshots, &origins), Ok(()), "seed {seed}: strong list spec");
    for (k, (v, blame)) in versions.iter().enumerate() {
        let want: Vec<(char, Id)> = blame.iter().copied().filter(|(_, x)| !main.is_purged(*x)).collect();
        assert_eq!(main.blame_at(v).unwrap(), want, "seed {seed}: version {k} stable");
        assert_eq!(main.fork_at(v).unwrap().text(), main.text_at(v).unwrap(), "seed {seed}: version {k} replay");
    }
}

#[test]
fn property_branches_restore_and_merge_any_base() {
    for seed in 0..seeds(200) {
        run_branch_seed(seed);
    }
}
