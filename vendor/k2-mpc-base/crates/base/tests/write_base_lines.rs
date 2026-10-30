//! `write --base` merges whole lines (v0.9.7.1): an independent judge of `merge::merge3`, the
//! merge an agent's `write --base` makes (base = the version the agent read, ours = its new
//! text, theirs = the live text). Public API only; nothing here calls the kit's diff.
//!
//! The merge is diff3 by line: two changes touching one line (or overlapping lines) are a
//! conflict, and the write is refused; nothing is merged inside a line. So a clean merge
//! must:
//!
//! 1. **hold only whole lines:** every line of the result is a whole line of base, ours or
//!    theirs (no line spliced from pieces of two lines, no two lines joined into one, no
//!    line merged from two changes);
//! 2. **pass the line-LCS rule** over every longest-common-subsequence alignment of base
//!    with each side: no deleted line back, no line lost (the rule of
//!    `fuzz/src/keepboth.rs`' `check_lcs`, ported with its own LCS tables).
//!
//! A conflict (the write refused) always passes: refusing is safe. The cases are random
//! lines that share their start ("p02:17", "p01:3 w4 w2"): each base line kept, deleted or
//! rewritten by one side, rewritten by both, or both changing different words of it; whole
//! new lines in the gaps, from one side or both. Up to v0.9.7 the sides were diffed to the
//! character (`diff::hunks`), which pairs lines sharing a start one line off: two different
//! lines could merge "cleanly" into one garbled line (soak_32 seed 1, call 7836). This test
//! fails there and passes with whole-line sides (`merge::write_base_sides`).

use std::collections::{HashMap, HashSet};

use k2_mpc_base::text::merge::{merge3, Merge};
use proptest::prelude::*;

fn lines(s: &str) -> Vec<&str> {
    s.split_inclusive('\n').collect()
}

/// A line as compared: without its line break.
fn key(l: &str) -> &str {
    l.strip_suffix('\n').unwrap_or(l)
}

fn count<'a>(ls: impl IntoIterator<Item = &'a str>) -> HashMap<&'a str, usize> {
    let mut m = HashMap::new();
    for l in ls {
        *m.entry(key(l)).or_insert(0) += 1;
    }
    m
}

fn keys(t: &str) -> Vec<&str> {
    lines(t).into_iter().map(key).collect()
}

/// Check 1: every line of `result` is a whole line of `base`, `ours` or `theirs`. `Err`
/// names the line: a join when it is two known lines end to end, a splice otherwise.
fn whole_lines(base: &str, ours: &str, theirs: &str, result: &str) -> Result<(), String> {
    let known: HashSet<&str> = [base, ours, theirs].into_iter().flat_map(keys).collect();
    for r in keys(result) {
        if known.contains(r) {
            continue;
        }
        let join = r
            .char_indices()
            .map(|(i, _)| i)
            .skip(1)
            .any(|i| known.contains(&r[..i]) && known.contains(&r[i..]));
        return Err(if join {
            format!("two lines joined into one: {r:?}")
        } else {
            format!("a line no one wrote (spliced or merged inside the line): {r:?}")
        });
    }
    Ok(())
}

/// Checks 1 and 2 on a clean merge's text.
fn judge_clean(base: &str, ours: &str, theirs: &str, result: &str) -> Result<(), String> {
    whole_lines(base, ours, theirs, result)?;
    check_lcs(base, ours, theirs, result)
}

/// Over every longest common subsequence of `base` and `side` (line texts): the fewest and
/// the most copies of each text of `base` it keeps, by plain O(nm) tables (one for the LCS
/// lengths, then per text one counting its matches along optimal paths, min and max).
fn lcs_keeps<'t>(base: &[&'t str], side: &[&str]) -> HashMap<&'t str, (usize, usize)> {
    let (n, m) = (base.len(), side.len());
    let mut texts: Vec<&str> = base.to_vec();
    texts.sort_unstable();
    texts.dedup();
    let w = m + 1;
    let mut len = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            len[i * w + j] = if base[i] == side[j] {
                len[(i + 1) * w + j + 1] + 1
            } else {
                len[(i + 1) * w + j].max(len[i * w + j + 1])
            };
        }
    }
    let mut out = HashMap::new();
    let (mut lo, mut hi) = (vec![0u32; (n + 1) * w], vec![0u32; (n + 1) * w]);
    for x in texts {
        for i in (0..=n).rev() {
            for j in (0..=m).rev() {
                let k = i * w + j;
                if i == n || j == m {
                    (lo[k], hi[k]) = (0, 0);
                    continue;
                }
                let (mut l, mut h) = (u32::MAX, 0u32);
                let here = len[k];
                if base[i] == side[j] && len[(i + 1) * w + j + 1] + 1 == here {
                    let add = u32::from(base[i] == x);
                    l = l.min(lo[(i + 1) * w + j + 1] + add);
                    h = h.max(hi[(i + 1) * w + j + 1] + add);
                }
                if len[(i + 1) * w + j] == here {
                    l = l.min(lo[(i + 1) * w + j]);
                    h = h.max(hi[(i + 1) * w + j]);
                }
                if len[i * w + j + 1] == here {
                    l = l.min(lo[i * w + j + 1]);
                    h = h.max(hi[i * w + j + 1]);
                }
                (lo[k], hi[k]) = (l, h);
            }
        }
        out.insert(x, (lo[0] as usize, hi[0] as usize));
    }
    out
}

/// Check 2, the line-LCS rule over all alignments: per line text `x`, with `d_b`, `d_o`,
/// `d_t` its copies in base, ours and theirs and `[kmin_s, kmax_s]` the copies an LCS of base
/// and side `s` keeps, `result` holds `x` at most
/// `min(d_b, kmax_o, kmax_t) + (d_o - kmin_o) + (d_t - kmin_t)` times (no deleted line back)
/// and at least `max(0, kmin_o + kmin_t - d_b) + max(d_o - kmax_o, d_t - kmax_t)` times
/// (nothing lost).
fn check_lcs(base: &str, ours: &str, theirs: &str, result: &str) -> Result<(), String> {
    let split = |t| -> Vec<&str> { lines(t).into_iter().map(key).collect() };
    let (bl, ol, tl) = (split(base), split(ours), split(theirs));
    let (ko, kt) = (lcs_keeps(&bl, &ol), lcs_keeps(&bl, &tl));
    let (db, d_o, dt) = (count(lines(base)), count(lines(ours)), count(lines(theirs)));
    let got = count(lines(result));
    let mut keys: Vec<&str> = db
        .keys()
        .chain(d_o.keys())
        .chain(dt.keys())
        .chain(got.keys())
        .copied()
        .collect();
    keys.sort_unstable();
    keys.dedup();
    let c = |m: &HashMap<&str, usize>, x: &str| m.get(x).copied().unwrap_or(0);
    for x in keys {
        let (b, o, t, g) = (c(&db, x), c(&d_o, x), c(&dt, x), c(&got, x));
        let (o_lo, o_hi) = ko.get(x).copied().unwrap_or((0, 0));
        let (t_lo, t_hi) = kt.get(x).copied().unwrap_or((0, 0));
        let upper = b.min(o_hi).min(t_hi) + (o - o_lo) + (t - t_lo);
        let lower = (o_lo + t_lo).saturating_sub(b) + (o - o_hi).max(t - t_hi);
        if g > upper {
            return Err(format!(
                "(lcs) a deleted line back (or a line too often): {x:?} {g} times, at most {upper}"
            ));
        }
        if g < lower {
            return Err(format!(
                "(lcs) a line lost: {x:?} {g} times, at least {lower}"
            ));
        }
    }
    Ok(())
}

/// A case: base, ours and theirs.
#[derive(Debug, Clone)]
struct Case {
    base: String,
    ours: String,
    theirs: String,
}

/// Lines sharing their start, as the soak's ("p" + a participant + ":" + a counter), few
/// enough values that starts collide often; one in four with two words after it.
fn line_pool() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec((0u8..4, 0u8..40, 0u8..4, 0u8..10, 0u8..10), 64).prop_map(|v| {
        let mut seen = HashSet::new();
        v.into_iter()
            .map(|(p, n, words, a, b)| {
                if words == 0 {
                    format!("p{p:02}:{n} w{a} w{b}")
                } else {
                    format!("p{p:02}:{n}")
                }
            })
            .filter(|l| seen.insert(l.clone()))
            .collect()
    })
}

fn case() -> impl Strategy<Value = Case> {
    (1usize..=7).prop_flat_map(|n| {
        (
            line_pool(),
            // per base line: 0 keep, 1/2 ours deletes/rewrites, 3/4 theirs deletes/rewrites,
            // 5 both rewrite, 6 both change one word (different words where it has two)
            prop::collection::vec(0u8..7, n),
            // per gap (before each line, and at the end): 0 nobody, 1 ours, 2 theirs, 3 both
            prop::collection::vec((0u8..4, 1usize..=2), n + 1),
        )
            .prop_filter_map("too few distinct lines", |(pool, ops, gaps)| {
                build(&pool, &ops, &gaps)
            })
    })
}

/// `line` with its word `k` (counting "pNN:N" as word 0) replaced by `w`.
fn with_word(line: &str, k: usize, w: &str) -> String {
    let mut ws: Vec<&str> = line.split(' ').collect();
    let k = k.min(ws.len() - 1);
    ws[k] = w;
    ws.join(" ")
}

fn build(pool: &[String], ops: &[u8], gaps: &[(u8, usize)]) -> Option<Case> {
    let mut fresh = pool.iter();
    let mut take = || fresh.next().cloned();
    let n = ops.len();
    let base_lines: Vec<String> = (0..n).map(|_| take()).collect::<Option<_>>()?;
    let (mut base, mut ours, mut theirs) = (String::new(), String::new(), String::new());
    let push = |s: &mut String, l: &str| {
        s.push_str(l);
        s.push('\n');
    };
    for i in 0..=n {
        let (who, k) = gaps[i];
        for _ in 0..k {
            if who & 1 != 0 {
                push(&mut ours, &take()?);
            }
            if who & 2 != 0 {
                push(&mut theirs, &take()?);
            }
        }
        if i == n {
            break;
        }
        let b = base_lines[i].clone();
        push(&mut base, &b);
        let (o, t): (Option<String>, Option<String>) = match ops[i] {
            0 => (Some(b.clone()), Some(b)),
            1 => (None, Some(b)),
            2 => (Some(take()?), Some(b)),
            3 => (Some(b), None),
            4 => (Some(b), Some(take()?)),
            5 => (Some(take()?), Some(take()?)),
            _ => {
                let (x, y) = (take()?, take()?);
                let x = x.split(' ').next().unwrap_or("x").to_owned();
                let y = y.split(' ').next().unwrap_or("y").to_owned();
                let words = b.split(' ').count();
                if words > 1 {
                    (
                        Some(with_word(&b, 1, &x)),
                        Some(with_word(&b, words - 1, &y)),
                    )
                } else {
                    (Some(with_word(&b, 0, &x)), Some(with_word(&b, 0, &y)))
                }
            }
        };
        if let Some(o) = o {
            push(&mut ours, &o);
        }
        if let Some(t) = t {
            push(&mut theirs, &t);
        }
    }
    Some(Case { base, ours, theirs })
}

/// The judge on one case; a conflict passes. `Ok(true)` when the merge was clean.
fn judge(c: &Case) -> Result<bool, String> {
    match merge3(&c.base, &c.ours, &c.theirs) {
        Merge::Conflicts(_) => Ok(false),
        Merge::Clean(m) => judge_clean(&c.base, &c.ours, &c.theirs, &m.text)
            .map(|()| true)
            .map_err(|e| {
                format!(
                    "{e}\n base   {:?}\n ours   {:?}\n theirs {:?}\n merged {:?}",
                    c.base, c.ours, c.theirs, m.text
                )
            }),
    }
}

fn cases() -> u32 {
    std::env::var("WRITE_BASE_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2048)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: cases(),
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// The property, both ways round (write --base reads the agent's text as ours and the
    /// live text as theirs; the rule must not depend on which side made which edit).
    #[test]
    fn a_clean_write_base_merge_never_splices_lines(c in case()) {
        for (what, c) in [
            ("", c.clone()),
            (" (sides swapped)", Case { ours: c.theirs.clone(), theirs: c.ours.clone(), ..c }),
        ] {
            if let Err(e) = judge(&c) {
                eprintln!("WRITE_BASE_LINES failing case{what}: {e}");
                prop_assert!(false, "{}", e);
            }
        }
    }
}

/// Most generated cases must merge clean, and none may hold a line no one wrote, or the
/// property judges little.
#[test]
fn the_cases_merge_clean_and_never_inside_a_line() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let (mut clean, mut total) = (0, 0);
    for _ in 0..1024 {
        let c = case().new_tree(&mut runner).unwrap().current();
        total += 1;
        if let Merge::Clean(m) = merge3(&c.base, &c.ours, &c.theirs) {
            clean += 1;
            whole_lines(&c.base, &c.ours, &c.theirs, &m.text).unwrap();
        }
    }
    eprintln!("WRITE_BASE_LINES clean {clean} of {total}");
    assert!(
        clean * 5 > total,
        "only {clean} of {total} cases merged clean"
    );
}

/// The soak's case (soak_32 seed 1, call 7836, trimmed): ours inserts a line between two
/// lines theirs rewrites into one. Before v0.9.7.1 this merged "cleanly" into "p213:102" and
/// "p0:90", lines nobody wrote; now it is a conflict (the write is refused).
#[test]
fn an_insertion_between_two_lines_the_other_side_rewrote_into_one_is_never_spliced() {
    let base = "p26:95\np05:90\n";
    let ours = "p26:95\np13:102\np05:90\n";
    let theirs = "p20:90\n";
    for (o, t) in [(ours, theirs), (theirs, ours)] {
        match merge3(base, o, t) {
            Merge::Clean(m) => judge_clean(base, o, t, &m.text).unwrap(),
            Merge::Conflicts(c) => assert_eq!(c[0].lines, (1, 2)),
        }
    }
}

/// Two sides rewriting one line into several lines each (the case the line-then-character
/// sides of the first v0.9.7.1 candidate merged into "p03 w0 w0" and "p00:3 w0 w3:3"): a
/// conflict.
#[test]
fn both_sides_rewriting_one_line_conflict() {
    let base = "p01:2\n";
    let ours = "p01:3 w0 w0\np00:3 w0 w3\n";
    let theirs = "p00:0 w1 w0\np00:0\np02:3\n";
    assert!(matches!(merge3(base, ours, theirs), Merge::Conflicts(_)));
    assert!(matches!(merge3(base, theirs, ours), Merge::Conflicts(_)));
    // the line-LCS failure of the same candidate ("a line lost"), shrunk
    let (base, ours, theirs) = (
        "p02:28\n",
        "p00:30\np00:0 w0 w0\np03:28\n",
        "p02:10 w0 w0\np00:0 w0 w1\n",
    );
    assert!(matches!(merge3(base, ours, theirs), Merge::Conflicts(_)));
    assert!(matches!(merge3(base, theirs, ours), Merge::Conflicts(_)));
}

/// Different words of one line conflict now (v0.9.7.1: no merge inside a line); different
/// lines still merge.
#[test]
fn different_words_of_one_line_conflict_and_different_lines_merge() {
    let base = "p01:3 w4 w2\np01:5\n";
    assert!(matches!(
        merge3(base, "p01:3 w7 w2\np01:5\n", "p01:3 w4 w9\np01:5\n"),
        Merge::Conflicts(_)
    ));
    let (ours, theirs) = ("p01:3 w7 w2\np01:5\n", "p01:3 w4 w2\np01:6\n");
    let Merge::Clean(m) = merge3(base, ours, theirs) else {
        panic!("different lines merge")
    };
    assert_eq!(m.text, "p01:3 w7 w2\np01:6\n");
    judge_clean(base, ours, theirs, &m.text).unwrap();
}

/// The judge itself: the soak's splice, a join and a line merged inside fail it; the LCS
/// rule's two directions behave.
#[test]
fn the_judge_catches_a_splice_and_a_join() {
    let (base, ours, theirs) = ("p26:95\np05:90\n", "p26:95\np13:102\np05:90\n", "p20:90\n");
    // v0.9.7's merge of these texts
    let e = judge_clean(base, ours, theirs, "p213:102\np0:90\n").unwrap_err();
    assert!(e.contains("no one wrote"), "{e}");
    let e = judge_clean(base, ours, theirs, "p26:95p05:90\n").unwrap_err();
    assert!(e.contains("joined"), "{e}");
    assert!(judge_clean("abc\n", "aXc\n", "abY\n", "aXY\n").is_err());
    judge_clean("ab\ncd\n", "aX\ncd\n", "ab\nYd\n", "aX\nYd\n").unwrap();
    // a deleted line back, and a line lost
    assert!(check_lcs("a\nb\n", "a\n", "a\nb\n", "a\nb\n").is_err());
    assert!(check_lcs("a\nb\n", "a\nb\nc\n", "a\nb\n", "a\nb\n").is_err());
    check_lcs("a\nb\n", "a\nb\nc\n", "x\na\nb\n", "x\na\nb\nc\n").unwrap();
}
