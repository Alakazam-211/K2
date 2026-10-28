//! The coverage matrix: it holds (every rule runs somewhere or is awaiting, every gap is
//! explained), it is printed, and `docs/CONFORMANCE.md` is equal to it.

use k2_mpc_conformance::matrix::{
    self, Awaiting, Cap, Engine, Matrix, Never, Rule, Status, CURRENT, REQUIRED,
};

#[test]
fn matrix_holds_and_prints() {
    let doc = CURRENT.render();
    println!("{doc}");
    let problems = CURRENT.problems();
    assert!(
        problems.is_empty(),
        "coverage matrix:\n{}",
        problems.join("\n")
    );
}

#[test]
fn every_new_rule_is_declared() {
    assert_eq!(
        REQUIRED.len(),
        20,
        "the 17 (new) rules plus hist.undo_atomic, hist.undo_partial and hist.redo"
    );
    for id in REQUIRED {
        assert!(matrix::rule(id).is_some(), "{id} is not in the suite");
    }
}

/// G1: every rule runs on some engine; what still awaits an engine is the native decoder's
/// (`sec.*`, G2). (`erase.span_includes_tombstones` runs on the model since mpce-model
/// v0.4.1, `Doc::span_ids`.)
#[test]
fn only_the_listed_rules_await() {
    const LATER: &[&str] = &[];
    let waiting = |r: &str| r.starts_with("sec.") || LATER.contains(&r);
    let others: Vec<&str> = matrix::AWAITING
        .iter()
        .map(|a| a.rule)
        .filter(|r| !waiting(r))
        .collect();
    assert_eq!(
        others,
        Vec::<&str>::new(),
        "awaiting beyond sec.* and {LATER:?}"
    );
    for r in matrix::RULES.iter().filter(|r| !waiting(r.id)) {
        assert!(
            matrix::ENGINES.iter().any(|e| matrix::runs(r, e)),
            "{} runs on no engine",
            r.id
        );
    }
}

#[test]
fn conformance_doc_is_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/CONFORMANCE.md");
    let want = CURRENT.render();
    if std::env::var_os("CONFORMANCE_BLESS").is_some() {
        std::fs::write(&path, &want).expect("write docs/CONFORMANCE.md");
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "docs/CONFORMANCE.md is out of date; regenerate it with \
         CONFORMANCE_BLESS=1 cargo test -p k2-mpc-conformance --test matrix"
    );
}

// ---------- negative controls: the checks catch what they should ----------

const ENGINE: Engine = Engine {
    name: "e",
    caps: &[Cap::Text],
    deviations: &[],
};

fn problems(rules: &[Rule], awaiting: &[Awaiting], never: &[Never]) -> Vec<String> {
    Matrix {
        rules,
        engines: &[ENGINE],
        awaiting,
        never,
        required: &[],
    }
    .problems()
    .into_iter()
    // the fake table doesn't list every implemented rule; only the checks under test count
    .filter(|p| !p.contains("but not listed under it"))
    .collect()
}

#[test]
fn a_rule_on_no_engine_must_be_awaiting() {
    let rules = [Rule {
        id: "hist.version_stable",
        req: "",
        needs: &[Cap::Versions],
    }];
    let p = problems(&rules, &[], &[]);
    assert!(
        p.iter().any(|x| x.contains("runs on no engine")),
        "not caught: {p:?}"
    );
    // NEVER alone isn't enough for a rule that runs nowhere
    let never = [Never {
        rule: "hist.version_stable",
        engine: "e",
        reason: "r",
    }];
    let p = problems(&rules, &[], &never);
    assert!(p.iter().any(|x| x.contains("runs on no engine")), "{p:?}");
    let awaiting = [Awaiting {
        rule: "hist.version_stable",
        engine: "e",
        reason: "r",
        target: "t",
    }];
    assert_eq!(problems(&rules, &awaiting, &[]), Vec::<String>::new());
    let m = Matrix {
        rules: &rules,
        engines: &[ENGINE],
        awaiting: &awaiting,
        never: &[],
        required: &[],
    };
    assert_eq!(
        m.status(&rules[0], &ENGINE),
        Status::Awaiting {
            reason: "r",
            target: "t"
        }
    );
}

#[test]
fn stale_and_silent_entries_are_caught() {
    let rules = [Rule {
        id: "text.converge",
        req: "",
        needs: &[Cap::Text],
    }];
    // awaiting a rule that runs now
    let awaiting = [Awaiting {
        rule: "text.converge",
        engine: "e",
        reason: "r",
        target: "t",
    }];
    let p = problems(&rules, &awaiting, &[]);
    assert!(p.iter().any(|x| x.contains("runs now")), "{p:?}");
    // a declared rule with an engine gap nobody explained
    let two = Engine {
        name: "f",
        caps: &[Cap::Text, Cap::Blame],
        deviations: &[],
    };
    let rules = [Rule {
        id: "text.blame_char",
        req: "",
        needs: &[Cap::Blame],
    }];
    let p = Matrix {
        rules: &rules,
        engines: &[ENGINE, two],
        awaiting: &[],
        never: &[],
        required: &["core.idempotent"],
    }
    .problems();
    assert!(
        p.iter().any(|x| x.contains("text.blame_char on e")),
        "{p:?}"
    );
    assert!(p.iter().any(|x| x.contains("core.idempotent")), "{p:?}");
    // a misspelled awaiting entry
    let awaiting = [Awaiting {
        rule: "text.convrge",
        engine: "e",
        reason: "r",
        target: "t",
    }];
    let rules = [Rule {
        id: "text.converge",
        req: "",
        needs: &[Cap::Text],
    }];
    let p = problems(&rules, &awaiting, &[]);
    assert!(p.iter().any(|x| x.contains("unknown rule")), "{p:?}");
}
