//! v0.4.1 API: `Doc::span_ids` for the kit's span purge (`erase.span_includes_tombstones`):
//! `purge_as(agent, &span_ids(start, end)?)`. Fixtures (a typo-fixed secret inside the
//! selection, a deleted copy just outside it, boundary refusals, multi-byte text, tombstones
//! from another replica's concurrent edits) and a seeded property (`MPCE_PROP_SEEDS`).

use mpce_model::text::{Doc, DocError, Hunk, Id};
use std::collections::BTreeSet;

fn h(start: usize, end: usize, s: &str) -> Hunk {
    Hunk { start, end, insert: s.to_string() }
}

/// Every char literal (`'X'`) in a Debug string, concatenated (storage keeps one character per
/// event, so this is how a leaked string shows up in Debug output).
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

/// The byte scan of `tests/text_v3.rs`, for one replica: every 8-byte window of `secret` in
/// its text, the Debug of `log()`, of `diff_events(&empty)` and of the whole replica (raw and
/// as char-literal streams); and no uppercase char literal anywhere (the fixtures keep every
/// other character lowercase). Returns where something was found.
fn byte_scan(secret: &str, d: &Doc) -> Vec<String> {
    let windows: Vec<&str> = (0..=secret.len() - 8).map(|i| &secret[i..i + 8]).collect();
    let log: String = d.log().iter().map(|e| format!("{e:?}")).collect();
    let diff: String = d.diff_events(&BTreeSet::new()).iter().map(|e| format!("{e:?}")).collect();
    let dbg = format!("{d:?}");
    let mut places: Vec<(String, String)> = vec![("text()".into(), d.text())];
    for (name, s) in [("log()", log), ("diff_events(empty)", diff), ("Debug(doc)", dbg)] {
        places.push((format!("{name} char stream"), char_stream(&s)));
        places.push((name.into(), s));
    }
    let mut leaks = Vec::new();
    for (name, s) in places {
        if windows.iter().any(|w| s.contains(w)) {
            leaks.push(name.clone());
        }
        if name.ends_with("char stream") && s.chars().any(|c| c.is_ascii_uppercase()) {
            leaks.push(format!("{name} (uppercase)"));
        }
    }
    leaks
}

fn id_of(d: &Doc, ch: char) -> Id {
    let ids: Vec<Id> = d.blame().into_iter().filter(|(c, _)| *c == ch).map(|(_, id)| id).collect();
    assert_eq!(ids.len(), 1, "{ch}");
    ids[0]
}

/// The list order between (and including) two elements.
fn between(d: &Doc, a: Id, b: Id) -> Vec<Id> {
    let l = d.list_with_tombstones();
    let i = l.iter().position(|x| *x == a).unwrap();
    let j = l.iter().position(|x| *x == b).unwrap();
    l[i..=j].to_vec()
}

#[test]
fn a_typo_fixed_secret_inside_the_selection_is_erased_tombstones_included() {
    let mut a = Doc::replica();
    // the typo 'X' (uppercase, so a surviving copy shows in the scan) is typed, deleted, fixed
    a.edit_as(101, &[h(0, 0, "keep CORRECTHORSEBATTXRY keep")]).unwrap();
    let typo = id_of(&a, 'X');
    let at = "keep CORRECTHORSEBATT".len();
    a.edit_as(101, &[h(at, at + 1, "")]).unwrap();
    a.edit_as(101, &[h(at, at, "E")]).unwrap();
    let secret = "CORRECTHORSEBATTERY";
    assert_eq!(a.text(), format!("keep {secret} keep"));
    assert!(a.is_deleted(typo) && !a.is_purged(typo));
    let mut b = Doc::replica();
    b.merge_from(&a).unwrap();
    let (start, end) = (5, 5 + secret.len());
    let ids = a.span_ids(start, end).unwrap();
    // the span: C through the last Y, every tombstone between, the typo among them
    assert!(ids.contains(&typo));
    assert_eq!(ids.len(), secret.len() + 1);
    let vis = a.visible();
    assert_eq!(ids, between(&a, vis[5], vis[5 + secret.len() - 1]));
    let p = a.purge_as(102, &ids).unwrap();
    assert_eq!(a.text(), "keep  keep");
    b.deliver(&p).unwrap();
    for d in [&a, &b] {
        assert_eq!(d.text(), "keep  keep");
        assert!(ids.iter().all(|x| d.is_purged(*x) && d.is_deleted(*x)));
        assert_eq!(byte_scan(secret, d), Vec::<String>::new());
        assert!(d.find_content("X").is_empty() && d.find_content("CORRECT").is_empty());
    }
    assert_eq!(a.list_with_tombstones(), b.list_with_tombstones());
}

#[test]
fn a_deleted_copy_just_outside_the_selection_survives_and_purge_by_text_reaches_it() {
    let mut d = Doc::replica();
    d.edit_as(101, &[h(0, 0, "x SECRETPASS y")]).unwrap();
    let orig = d.visible()[2..12].to_vec();
    // a copy pasted right after it and deleted, and one pasted right before it and deleted
    d.edit_as(101, &[h(12, 12, "SECRETPASS")]).unwrap();
    d.edit_as(101, &[h(12, 22, "")]).unwrap();
    d.edit_as(101, &[h(2, 2, "SECRETPASS")]).unwrap();
    d.edit_as(101, &[h(2, 12, "")]).unwrap();
    assert_eq!(d.text(), "x SECRETPASS y");
    assert_eq!(d.visible()[2..12], orig[..]);
    let l = d.list_with_tombstones();
    let i = l.iter().position(|x| *x == orig[0]).unwrap();
    // the copies sit directly against the selection's ends, as tombstones
    assert!(d.is_deleted(l[i - 1]) && d.is_deleted(l[i + 10]));
    let ids = d.span_ids(2, 12).unwrap();
    assert_eq!(ids, orig, "tombstones just outside the ends are excluded");
    d.purge_as(102, &ids).unwrap();
    assert_eq!(d.text(), "x  y");
    // the copies survive the span purge, and purge by text reaches them
    let occ = d.find_content("SECRETPASS");
    assert_eq!(occ.len(), 2, "{occ:?}");
    assert!(occ.iter().flatten().all(|x| d.is_deleted(*x) && !d.is_purged(*x) && !orig.contains(x)));
    d.purge_as(102, &occ.concat()).unwrap();
    assert!(d.find_content("SECRETPASS").is_empty());
    assert_eq!(d.text(), "x  y");
    assert_eq!(byte_scan("SECRETPASS", &d), Vec::<String>::new());
}

#[test]
fn span_boundaries_are_refused_and_change_nothing() {
    let mut d = Doc::replica();
    assert_eq!(d.span_ids(0, 0), Err(DocError::BadHunks));
    assert_eq!(d.span_ids(0, 1), Err(DocError::OutOfRange));
    // a(0) é(1..3) 🎉(3..7) \n(7) b(8); len 9
    d.edit_as(101, &[h(0, 0, "aé🎉\nb")]).unwrap();
    let before = (d.text(), d.list_with_tombstones(), d.log().len());
    assert_eq!(d.span_ids(3, 3), Err(DocError::BadHunks), "empty span");
    assert_eq!(d.span_ids(7, 3), Err(DocError::BadHunks), "reversed span");
    assert_eq!(d.span_ids(0, 10), Err(DocError::OutOfRange));
    assert_eq!(d.span_ids(10, 11), Err(DocError::OutOfRange));
    assert_eq!(d.span_ids(10, 3), Err(DocError::OutOfRange), "offsets are checked first");
    for (s, e, bad) in [(2, 3, 2), (0, 2, 2), (1, 4, 4), (1, 5, 5), (6, 8, 6), (4, 4, 4)] {
        assert_eq!(d.span_ids(s, e), Err(DocError::NotCharBoundary { offset: bad }), "{s}..{e}");
    }
    assert_eq!((d.text(), d.list_with_tombstones(), d.log().len()), before);
}

#[test]
fn multi_byte_spans() {
    let mut d = Doc::replica();
    d.edit_as(101, &[h(0, 0, "aé🎉\nb")]).unwrap();
    let v = d.visible();
    assert_eq!(d.span_ids(1, 3).unwrap(), vec![v[1]]);
    assert_eq!(d.span_ids(3, 7).unwrap(), vec![v[2]]);
    assert_eq!(d.span_ids(7, 8).unwrap(), vec![v[3]]);
    assert_eq!(d.span_ids(1, 8).unwrap(), v[1..4].to_vec());
    assert_eq!(d.span_ids(0, 9).unwrap(), v);
    for (s, e, left) in [(1, 3, "a🎉\nb"), (3, 8, "aéb"), (0, 9, "")] {
        let mut x = d.clone();
        let ids = x.span_ids(s, e).unwrap();
        x.purge_as(102, &ids).unwrap();
        assert_eq!(x.text(), left, "{s}..{e}");
    }
    // a multi-byte tombstone inside the span is included
    d.edit_as(101, &[h(3, 7, "")]).unwrap();
    assert_eq!(d.text(), "aé\nb");
    assert_eq!(d.span_ids(1, 4).unwrap(), v[1..4].to_vec());
    assert_eq!(d.span_ids(0, 3).unwrap(), v[0..2].to_vec(), "the 🎉 tombstone after the end is excluded");
}

#[test]
fn a_span_over_tombstones_from_another_replicas_concurrent_edits() {
    let mut a = Doc::replica();
    a.edit_as(101, &[h(0, 0, "abcdefgh")]).unwrap();
    let mut b = Doc::replica();
    b.merge_from(&a).unwrap();
    // b types XYZ between d and e, then deletes it and f; concurrently a types q between b
    // and c and deletes g
    b.edit_as(102, &[h(4, 4, "XYZ")]).unwrap();
    b.edit_as(102, &[h(4, 7, ""), h(8, 9, "")]).unwrap();
    assert_eq!(b.text(), "abcdegh");
    a.edit_as(101, &[h(2, 2, "q"), h(6, 7, "")]).unwrap();
    let a0 = a.clone();
    a.merge_from(&b).unwrap();
    b.merge_from(&a0).unwrap();
    assert_eq!(a.text(), "abqcdeh");
    assert_eq!(a.list_with_tombstones(), b.list_with_tombstones());
    let tomb_b: Vec<Id> = a.list_with_tombstones().into_iter().filter(|x| x.agent == 102).collect();
    assert_eq!(tomb_b.len(), 3);
    // span "cde" (3..6) has X, Y, Z (b's) inside; f and g (deleted by b and by a) lie after
    // e, just outside the end
    let ids = a.span_ids(3, 6).unwrap();
    let v = a.visible();
    assert_eq!(ids, between(&a, v[3], v[5]));
    assert!(tomb_b.iter().all(|x| ids.contains(x)));
    assert_eq!(ids.len(), 6);
    // span "cdeh" (3..7) reaches f and g too
    let ids2 = b.span_ids(3, 7).unwrap();
    assert_eq!(ids2.len(), 9);
    // already-purged elements in range are included, and purge_as accepts them
    a.purge_as(101, &[tomb_b[1]]).unwrap();
    assert_eq!(a.span_ids(3, 6).unwrap(), ids);
    let p = a.purge_as(101, &ids).unwrap();
    assert_eq!(a.text(), "abqh");
    b.deliver_all(&a.diff_events(b.frontier())).unwrap();
    assert!(b.has(p.id));
    for d in [&a, &b] {
        assert_eq!(d.text(), "abqh");
        assert!(ids.iter().all(|x| d.is_purged(*x) && d.is_deleted(*x)));
        assert!(d.find_content("XYZ").is_empty());
    }
    assert_eq!(a.list_with_tombstones(), b.list_with_tombstones());
}

// ---------------------------------------------------------------------------------------
// Property

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

fn full_sync(reps: &mut [Doc]) {
    for _ in 0..2 {
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

fn edit(rng: &mut Rng, reps: &mut [Doc], agents: &[u64], r: usize) {
    let hs = random_hunks(rng, &reps[r].text());
    reps[r].edit_as(agents[r], &hs).unwrap();
}

/// Random edits and deletes on 3 replicas (each its own agent) with partial syncs and the odd
/// purge of a random run; a random span purge on one replica while the others make
/// concurrent edits; then full sync. Checks: span_ids is the list-order slice from the span's
/// first visible char to its last; the purger's visible text outside the span is unchanged;
/// every element of the span is purged on every replica after sync; between the span's ends
/// nothing content-bearing survives but inserts concurrent with the purge; replicas converge.
fn run_span_seed(seed: u64) -> (usize, usize) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let agents = [101u64, 102, 103];
    let mut reps: Vec<Doc> = (0..3).map(|_| Doc::replica()).collect();
    reps[0].edit_as(101, &[h(0, 0, "seed text é🎉\n")]).unwrap();
    for _ in 0..12 + rng.below(12) {
        let r = rng.below(3);
        match rng.below(10) {
            0..=5 => edit(&mut rng, &mut reps, &agents, r),
            6..=8 => {
                let f = rng.below(3);
                if f != r {
                    let from = reps[f].clone();
                    reps[r].merge_from(&from).unwrap();
                }
            }
            _ => {
                let l = reps[r].list_with_tombstones();
                if !l.is_empty() {
                    let s = rng.below(l.len());
                    let run = l[s..(s + 1 + rng.below(3)).min(l.len())].to_vec();
                    reps[r].purge_as(agents[r], &run).unwrap();
                }
            }
        }
    }
    full_sync(&mut reps);
    let r = rng.below(3);
    let text = reps[r].text();
    if text.is_empty() {
        reps[r].edit_as(agents[r], &[h(0, 0, "ab")]).unwrap();
        full_sync(&mut reps);
    }
    let text = reps[r].text();
    let b = bounds(&text);
    let si = rng.below(b.len() - 1);
    let ei = si + 1 + rng.below((b.len() - 1 - si).min(6));
    let (start, end) = (b[si], b[ei]);
    let ids = reps[r].span_ids(start, end).unwrap();
    let vis = reps[r].visible();
    let order = reps[r].list_with_tombstones();
    let i = order.iter().position(|x| *x == vis[si]).unwrap();
    let j = order.iter().position(|x| *x == vis[ei - 1]).unwrap();
    assert_eq!(ids, order[i..=j], "seed {seed}: span_ids is the slice");
    let tombs = ids.iter().filter(|x| !vis.contains(x)).count();
    let known: BTreeSet<Id> = order.iter().copied().collect();
    reps[r].purge_as(agents[r], &ids).unwrap();
    assert_eq!(reps[r].text(), format!("{}{}", &text[..start], &text[end..]), "seed {seed}: outside the span");
    // concurrent edits elsewhere, then sync
    for _ in 0..rng.below(4) {
        let o = (r + 1 + rng.below(2)) % 3;
        edit(&mut rng, &mut reps, &agents, o);
    }
    full_sync(&mut reps);
    let (first, last) = (ids[0], *ids.last().unwrap());
    for (k, d) in reps.iter().enumerate() {
        assert!(ids.iter().all(|x| d.is_purged(*x) && d.is_deleted(*x)), "seed {seed}: replica {k}");
        let l = d.list_with_tombstones();
        let a = l.iter().position(|x| *x == first).unwrap();
        let z = l.iter().position(|x| *x == last).unwrap();
        for x in &l[a..=z] {
            assert!(d.is_purged(*x) || !known.contains(x), "seed {seed}: replica {k}: {x:?} survives between the ends");
        }
        assert_eq!(d.text(), reps[0].text(), "seed {seed}: converge");
        assert_eq!(d.list_with_tombstones(), reps[0].list_with_tombstones(), "seed {seed}: converge");
        assert_eq!(d.frontier(), reps[0].frontier(), "seed {seed}: converge");
    }
    (ids.len(), tombs)
}

#[test]
fn property_span_purge() {
    let n = seeds(200);
    let (mut with_tombs, mut total) = (0, 0);
    for seed in 0..n {
        let (len, tombs) = run_span_seed(seed);
        total += len;
        with_tombs += usize::from(tombs > 0);
    }
    eprintln!("span purge property: {n} seeds, {with_tombs} with tombstones inside the span, {total} elements purged");
    assert!(n < 20 || with_tombs > n as usize / 10);
}
