//! The model check's mutants (spec §8 Mutants; `model::mutant`) at a tiny bound: each must
//! violate the rule it breaks, at the depth given, on the enumerator; the oracle of record
//! violates none of the rules the runs stop at. The same set fails TLC on `undo.qnt`
//! (`tools/undo/counts.sh`, `PART=m`). Needs `--features mutants`.
//!
//! One test per mutant (and per oracle bound), so a runner with a process per test (nextest)
//! runs them in parallel. The active mutant is process-wide, so each test holds [`one_at_a_time`]
//! while it runs: under `cargo test` (one process, several threads) they take turns. The
//! enumerator test runs the binary and holds nothing.

// the trace runner (shared with tests/traces.rs), for the D1 mutants' trace check
#[allow(dead_code)]
mod trace;
use spec_undo::bounded::{bfs, Bound, STOP};
use spec_undo::model::mutant::{self, Mutant};
use spec_undo::model::{Bounds, Edit, Mode, Model, Outcome, Request};
use std::sync::{Mutex, MutexGuard};

/// The active mutant is process-wide: a test that sets one holds this for its whole run, and
/// the guard puts the oracle of record back when the test ends (panicked or not).
struct OneAtATime(#[allow(dead_code)] MutexGuard<'static, ()>);

impl Drop for OneAtATime {
    fn drop(&mut self) {
        mutant::set(Mutant::None);
    }
}

fn one_at_a_time() -> OneAtATime {
    static LOCK: Mutex<()> = Mutex::new(());
    // a test that failed while holding it leaves it poisoned; the others still run
    let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    mutant::set(Mutant::None);
    OneAtATime(g)
}

fn tiny() -> Bound {
    Bound {
        pids: 2,
        events: 1,
        undos: 1,
        reverts: 0,
        maxlen: 4,
        // three letters, so a two-region edit exists from the start (mutant (iv))
        init: "abb".into(),
        ..Bound::small()
    }
}

/// The restart mutant's bound: one pid, three events, a revert and a restart, from `abba`
/// (its least counterexample: a two-region edit, deletes making the regions touch with one
/// tainted at its start only, then the restart).
fn restart_bound() -> Bound {
    Bound {
        pids: 1,
        events: 3,
        undos: 0,
        reverts: 1,
        restarts: 1,
        maxlen: 4,
        init: "abba".into(),
        ..Bound::small()
    }
}

/// Mutant (x)'s bound: one pid, one event, one undo and one revert, from `ab` (its least
/// counterexample: p1 deletes `b` at the doc's end, undoes, and reverts the delete; by
/// element identity the revert's `b` and the undo's are different insertions at the doc's
/// end, so O5 lands both: two live `b` with one root).
fn provenance_bound() -> Bound {
    Bound {
        pids: 1,
        events: 1,
        undos: 1,
        reverts: 1,
        maxlen: 4,
        init: "ab".into(),
        ..Bound::small()
    }
}

/// Mutant (xi)'s bound: two pids, one event each, one undo and one revert, from `ab` (its
/// least counterexample, at the doc's end: p1 deletes `b`, undoes it, p2 types `a` before
/// the restored `b`, and p1 reverts its delete; theirs inserts `a` and the restored `b`,
/// ours `b`, not identical by roots (O4), so O5 lands both: two live `b` with one root).
fn no_o4a_bound() -> Bound {
    Bound {
        pids: 2,
        events: 1,
        undos: 1,
        reverts: 1,
        maxlen: 4,
        init: "ab".into(),
        ..Bound::small()
    }
}

/// The oracle of record violates none of the rules the runs stop at, at each mutant's bound.
fn the_oracle_passes_at(b: Bound) {
    let _one = one_at_a_time();
    let clean = bfs(&b, false, STOP, false, |_, _| {});
    assert!(
        clean.violations.is_empty(),
        "the oracle: {:?}",
        clean.violations.keys().collect::<Vec<_>>()
    );
}

#[test]
fn the_oracle_passes_at_the_tiny_bound() {
    the_oracle_passes_at(tiny());
}

#[test]
fn the_oracle_passes_at_the_restart_bound() {
    the_oracle_passes_at(restart_bound());
}

#[test]
fn the_oracle_passes_at_the_provenance_bound() {
    the_oracle_passes_at(provenance_bound());
}

#[test]
fn the_oracle_passes_at_the_no_o4a_bound() {
    the_oracle_passes_at(no_o4a_bound());
}

/// Mutant `m` violates `rule` at `depth` on the enumerator at bound `b`.
fn fails_its_rule(m: Mutant, rule: &str, depth: usize, b: Bound) {
    let _one = one_at_a_time();
    mutant::set(m);
    let c = bfs(&b, false, STOP, true, |_, _| {});
    let got: Vec<_> = c
        .violations
        .iter()
        .filter(|(r, _)| STOP.contains(r))
        .map(|(r, (d, _))| (*r, *d))
        .collect();
    eprintln!("{}: {got:?}", m.name());
    assert!(
        got.contains(&(rule, depth)),
        "{}: wanted {rule} at depth {depth}, got {got:?}",
        m.name()
    );
}

#[test]
fn no_taint_fails_u9() {
    fails_its_rule(Mutant::NoTaint, "U9", 3, tiny());
}

#[test]
fn partial_hides_taint_fails_u15() {
    fails_its_rule(Mutant::PartialHidesTaint, "U15", 3, tiny());
}

#[test]
fn redo_retries_skipped_fails_u17() {
    fails_its_rule(Mutant::RedoRetriesSkipped, "U17", 3, tiny());
}

#[test]
fn o5_every_window_end_fails_o5() {
    fails_its_rule(Mutant::O5EveryWindowEnd, "O5", 3, tiny());
}

#[test]
fn o5_region_base_fails_o5() {
    fails_its_rule(Mutant::O5RegionBase, "O5", 3, tiny());
}

#[test]
fn skip_u19_fails_u19() {
    fails_its_rule(Mutant::SkipU19, "U19", 1, tiny());
}

/// F3: the merge's sides found by diffing strings.
#[test]
fn rediff_fails_own_only() {
    fails_its_rule(Mutant::Rediff, "own_only", 3, tiny());
}

/// v0.8.1's persisted carry: one taint flag per region.
#[test]
fn restart_single_taint_fails_rt() {
    fails_its_rule(Mutant::RestartSingleTaint, "RT", 4, restart_bound());
}

/// v1.8: O4 on element identity, no O4a.
#[test]
fn provenance_ignored_fails_i1() {
    fails_its_rule(Mutant::ProvenanceIgnored, "I1", 3, provenance_bound());
}

/// v1.9: O4 on roots, no O4a.
#[test]
fn no_o4a_fails_i1() {
    fails_its_rule(Mutant::NoO4a, "I1", 4, no_o4a_bound());
}

/// Every mutant of the model check has its test above (a new one fails this until it has).
#[test]
fn every_mutant_has_its_test() {
    let tested = [
        Mutant::NoTaint,
        Mutant::PartialHidesTaint,
        Mutant::RedoRetriesSkipped,
        Mutant::O5EveryWindowEnd,
        Mutant::O5RegionBase,
        Mutant::SkipU19,
        Mutant::Rediff,
        Mutant::RestartSingleTaint,
        Mutant::ProvenanceIgnored,
        Mutant::NoO4a,
    ];
    for (name, m) in Mutant::ALL {
        assert!(
            tested.contains(&m),
            "mutant {name} has no test in tests/mutants.rs"
        );
    }
    assert_eq!(Mutant::ALL.len(), tested.len());
}

fn e(start: usize, end: usize, insert: &str) -> Edit {
    Edit {
        start,
        end,
        insert: insert.into(),
    }
}

/// Mutant (xi) on O4a's mid-text witness (the oracle's `o4a_mid_text`; depth 5, two undos):
/// p1 inserts `a` at 1; p2 deletes the seed's `a` and replaces `b` (two regions); p1 undoes;
/// p1 reverts p2's change, which puts the seed's `a` back outside the window of p2's first
/// region. p2's `undo --partial` then puts the seed's `a` root back a second time: the oracle
/// refuses it (O4a, blocked); without O4a it applies and I1 fails.
#[test]
fn no_o4a_fails_i1_in_mid_text() {
    let _one = one_at_a_time();
    let run = |m: Mutant| {
        mutant::set(m);
        // letters, one token each, as the model check runs (D1 the identity, v1.10)
        let chars = Bounds {
            tokens: k2_mpc_base::text::lines::Tokens::Chars,
            ..Bounds::default()
        };
        let mut o = Model::new(vec!["ab".into()], 1, chars);
        o.edit("p1", None, vec![(0, vec![e(1, 1, "a")])]); // "aab"
        o.edit("p2", None, vec![(0, vec![e(0, 1, ""), e(2, 3, "a")])]); // change 3: "aa"
        o.undo("p1", None, &Request::new(1, Mode::All)); // "a"
        let (r, _) = o.revert("p1", None, 3, false); // "ab", the seed's roots back
        assert!(matches!(r, Outcome::Applied { .. }), "{}: {r:?}", m.name());
        let (u, h) = o.undo("p2", None, &Request::new(1, Mode::Partial));
        mutant::set(Mutant::None);
        (u, h.o4a, h.i1_violated, o.i1(), o.texts[0].clone())
    };
    let (u, o4a, viol, i1, text) = run(Mutant::None);
    assert!(
        matches!(u, Outcome::Blocked { .. }) && o4a && !viol && i1,
        "{u:?}"
    );
    assert_eq!(text, "ab");
    let (u, o4a, viol, i1, _) = run(Mutant::NoO4a);
    assert!(matches!(u, Outcome::Applied { .. }) && !o4a, "{u:?}");
    assert!(viol && !i1, "no O4a: two live elements with one root");
}

/// A panic in the oracle (the harness mutant: it panics on an undo with something to undo)
/// ends every mode of `undo_enum` promptly, as a violation `panic@<depth>` with its message,
/// exit 1 (v0.9.3: a walker's panic used to leave the process running until the job's
/// timeout). The dfs run puts the panic below its seeds, in a worker.
#[test]
fn an_oracle_panic_ends_every_enumerator_mode_as_a_violation() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let bound = [
        "--pids",
        "2",
        "--events",
        "1",
        "--undos",
        "1",
        "--reverts",
        "1",
        "--maxlen",
        "3",
        "--init",
        "ab",
        "--mutant",
        "harness-oracle-panics",
        "--threads",
        "4",
    ];
    for (mode, extra) in [
        ("walk", &["--probes", "1000000"][..]),
        ("bfs", &[][..]),
        ("dfs", &["--seed-depth", "1"][..]),
        ("knuth", &["--probes", "100000"][..]),
    ] {
        let t0 = Instant::now();
        let mut child = Command::new(env!("CARGO_BIN_EXE_undo_enum"))
            .arg(mode)
            .args(bound)
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("undo_enum runs");
        // it must end on its own, promptly
        let status = loop {
            if let Some(st) = child.try_wait().expect("wait") {
                break st;
            }
            if t0.elapsed() > Duration::from_secs(60) {
                let _ = child.kill();
                panic!("undo_enum {mode} still running after 60 s with a panicking oracle");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let out = child.wait_with_output().expect("output");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let summary = stdout
            .lines()
            .find(|l| l.starts_with("SUMMARY"))
            .unwrap_or_else(|| panic!("{mode}: no SUMMARY line:\n{stdout}"));
        assert_eq!(status.code(), Some(1), "{mode}: {summary}");
        assert!(summary.contains("panic@"), "{mode}: {summary}");
        assert!(
            summary.contains("panic=\"harness mutant: the oracle panics on an undo\""),
            "{mode}: {summary}"
        );
        let depth: usize = summary
            .split("panic@")
            .nth(1)
            .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|d| d.parse().ok())
            .expect("a depth");
        assert!(
            depth >= 2,
            "{mode}: an undo with something to undo: {summary}"
        );
    }
}

/// D1's mutants (undo spec v1.10, outside `Mutant::ALL`) make the oracle disagree with the
/// kit on some trace: each must be caught within 256 traces.
#[test]
fn d1_mutants_are_caught_by_the_traces() {
    let _one = one_at_a_time();
    for (name, m) in Mutant::D1 {
        mutant::set(m);
        let caught = (0..256u64).find(|&seed| trace::run(seed).mismatch.is_some());
        mutant::set(Mutant::None);
        let seed = caught.unwrap_or_else(|| panic!("{name}: no trace disagreed in 256"));
        eprintln!("[mutants] {name}: caught by trace seed {seed}");
    }
}
