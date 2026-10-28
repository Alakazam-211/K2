//! The parity framework on the kit's own verbs, with a plain-text pack (K-PAR-2, -3): every
//! row through the harness, and negative controls proving it fails loudly.

use std::sync::Arc;
use std::time::Instant;

use k2_mpc_base::parity::{
    self, Case, Failure, Output, ParityRow, PersonAction, Registry, RowStatus, Step,
};
use k2_mpc_base::session::{Pack, TextFilePack};

fn pack() -> Arc<dyn Pack> {
    Arc::new(TextFilePack::new("text", 4 << 20))
}

/// A plain-text pack's parity list: the kit's verbs only. An exhaustive `match`, as every
/// pack writes it.
fn row(a: PersonAction) -> ParityRow {
    use PersonAction as P;
    match a {
        P::Open => ParityRow::live(a, &["open"]),
        P::Close => ParityRow::live(a, &["leave"]),
        P::ReadAll => ParityRow::live(a, &["cat", "wc"]),
        P::ViewRange => ParityRow::live(a, &["head", "tail", "sed", "view"]),
        P::Find => ParityRow::live(a, &["grep"]),
        P::Who => ParityRow::live(a, &["who"]),
        P::Follow => ParityRow::live(a, &["follow"]),
        P::Caret => ParityRow::live_but(a, &["cursor"], &["intent"]),
        P::Type => ParityRow::live(a, &["insert"]),
        P::TypeOver => ParityRow::live(a, &["replace"]),
        P::Delete => ParityRow::live(a, &["delete", "sed -i"]),
        P::FindReplace => ParityRow::live(a, &["sed -i"]),
        P::Paste => ParityRow::live(a, &["patch", "apply", "append"]),
        P::Rewrite => ParityRow::live(a, &["write"]),
        P::Format => ParityRow::live(a, &["insert", "replace", "sed -i"]),
        P::Undo => ParityRow::live(a, &["undo", "redo", "undo --drop"]),
        P::Saved => ParityRow::live(a, &["status", "flush"]),
        P::Watch => ParityRow::live(a, &["watch", "tail -f"]),
        P::History => ParityRow::live(a, &["log", "diff --version", "show --version", "diff"]),
        P::Restore => ParityRow::live(a, &["restore", "version --name"]),
        P::Rename => ParityRow::live(a, &["mv"]),
        P::Blame => ParityRow::live(a, &["blame"]),
        P::Annotate => ParityRow::deferred(
            a,
            &[
                "comment", "reply", "resolve", "suggest", "accept", "reject", "comments",
            ],
            "M5",
            "the annotation layer",
        ),
        P::Export => ParityRow::not_applicable(
            a,
            &["cat"],
            &["export"],
            "the file is the export; cat gives its exact bytes",
        ),
        P::WhyOff => ParityRow::live(a, &["status"]),
        P::UndoPartial => ParityRow::live(a, &["undo --partial"]),
        P::Mention => ParityRow::deferred(a, &["comment"], "M5", "the annotation layer"),
        P::Purge => ParityRow::deferred(a, &["purge"], "C7a purge", "redaction is not built"),
        P::Approve => ParityRow::live(a, &["approve", "decline"]),
        P::Pending => ParityRow::live(a, &["log --pending"]),
        P::Revert => ParityRow::live(a, &["revert"]),
    }
}

fn rows() -> Vec<ParityRow> {
    PersonAction::ALL.iter().map(|a| row(*a)).collect()
}

#[test]
fn agent_parity_text_pack() {
    let t = Instant::now();
    let report = parity::check(pack(), &rows(), &Registry::kit(), &[])
        .unwrap_or_else(|f| panic!("{}", parity::render(&f)));
    let live = rows()
        .iter()
        .filter(|r| r.status == RowStatus::Live)
        .count();
    assert_eq!(live, 27, "rows live");
    assert_eq!(report.not_applicable.len(), 1);
    assert_eq!(report.deferred.len(), 3);
    assert_eq!(
        report
            .matched
            .iter()
            .filter(|m| m.0 == PersonAction::Blame)
            .count(),
        3,
        "P22's cases"
    );
    assert!(report.matched.len() >= 45, "{} cases", report.matched.len());
    eprintln!(
        "agent parity: {} cases matched over {live} live rows in {:?}",
        report.matched.len(),
        t.elapsed()
    );
}

/// K-PAR-2: every kit verb serves an action, and the list and the registry agree.
#[test]
fn registry_covers_actions() {
    let r = Registry::kit();
    for spec in r.verbs() {
        assert!(!spec.serves.is_empty(), "{}", spec.name);
    }
    for a in PersonAction::ALL {
        let row = row(*a);
        if row.status == RowStatus::Live {
            assert!(r.serving(*a).count() > 0, "{a}: no verb serves it");
        }
    }
}

fn list_failures(rows: &[ParityRow], registry: &Registry) -> Vec<Failure> {
    let f = parity::check_list(rows, registry, &[]);
    assert!(
        !f.is_empty(),
        "the negative control passed: the harness can't catch its failure"
    );
    f
}

/// Negative control: a verb removed from the registry is reported missing, from the list
/// and from the agent path.
#[test]
fn a_removed_verb_is_reported_missing() {
    let reg = Registry::kit().without("undo --partial");
    let f = list_failures(&rows(), &reg);
    assert_eq!(
        f,
        vec![Failure::MissingVerb {
            action: PersonAction::UndoPartial,
            verb: "undo --partial".into(),
            at: "parity list",
        }],
        "{}",
        parity::render(&f)
    );
    let case = parity::kit_cases(PersonAction::UndoPartial)[0];
    let f = parity::run_case(&pack(), &reg, &case);
    assert_eq!(
        f,
        vec![Failure::MissingVerb {
            action: PersonAction::UndoPartial,
            verb: "undo --partial".into(),
            at: "agent path",
        }],
        "{}",
        parity::render(&f)
    );
    // and the same case passes with the full registry
    let ok = parity::run_case(&pack(), &Registry::kit(), &case);
    assert!(ok.is_empty(), "{}", parity::render(&ok));
}

/// Negative control: an agent path that behaves differently is a mismatch, naming the
/// field.
#[test]
fn a_different_behaviour_is_a_mismatch() {
    fn person(p: &mut parity::Person<'_>) -> Result<Step, String> {
        let at = p.text()?.len();
        p.type_at(at, at, "x\n").map_err(|e| e.to_string())?;
        Ok(Step::done())
    }
    fn agent(a: &mut parity::Agent<'_>) -> Result<Step, String> {
        // appends something else
        a.verb(&["append"], b"y\n")?;
        Ok(Step::done())
    }
    let bad = Case::new(
        PersonAction::Paste,
        "append",
        "planted difference",
        person,
        agent,
    );
    let f = parity::run_case(&pack(), &Registry::kit(), &bad);
    assert_eq!(f.len(), 1, "{}", parity::render(&f));
    let Failure::Mismatch { case, diffs } = &f[0] else {
        panic!("{}", parity::render(&f));
    };
    assert!(case.contains("planted difference"));
    assert!(diffs.iter().any(|d| d.starts_with("text:")), "{diffs:?}");
}

/// Negative control: an unlisted action, an N/A verb that exists, and a listed verb with no
/// case each fail.
#[test]
fn the_list_is_checked_both_ways() {
    let mut r = rows();
    r.retain(|x| x.action != PersonAction::Find);
    r.iter_mut()
        .find(|x| x.action == PersonAction::Export)
        .unwrap()
        .absent = &["cat"];
    let f = list_failures(&r, &Registry::kit());
    let s = parity::render(&f);
    assert!(f.contains(&Failure::MissingRow(PersonAction::Find)), "{s}");
    assert!(
        f.iter().any(|x| matches!(x, Failure::UnexpectedVerb { action: PersonAction::Export, verb, .. } if verb == "cat")),
        "{s}"
    );
    assert!(
        f.iter().any(|x| matches!(x, Failure::VerbNotListed { action: PersonAction::Find, verb, .. } if verb == "grep")),
        "{s}"
    );

    // a pack verb listed with no case
    let mut r = rows();
    r.iter_mut()
        .find(|x| x.action == PersonAction::Follow)
        .unwrap()
        .verbs = &["follow", "who"];
    let f = list_failures(&r, &Registry::kit());
    assert!(
        f.iter().any(|x| matches!(x, Failure::VerbDoesNotServe { action: PersonAction::Follow, verb } if verb == "who")),
        "{}",
        parity::render(&f)
    );
}

/// Negative control: a deferred row whose verb now exists must be made live. P22 is the
/// row that flipped (v0.3.0): kept deferred now that `blame` is registered and has cases,
/// the list fails both ways.
#[test]
fn a_landed_deferred_verb_fails() {
    let mut r = rows();
    let i = r
        .iter()
        .position(|x| x.action == PersonAction::Blame)
        .unwrap();
    r[i] = ParityRow::deferred(
        PersonAction::Blame,
        &["blame"],
        "C7b",
        "blame lands with its UI",
    );
    let f = list_failures(&r, &Registry::kit());
    let s = parity::render(&f);
    assert!(
        f.iter().any(|x| matches!(x, Failure::UnexpectedVerb { action: PersonAction::Blame, verb, .. } if verb == "blame")),
        "{s}"
    );
    assert!(
        f.contains(&Failure::StaleDeferral(PersonAction::Blame)),
        "{s}"
    );
    assert!(
        f.iter().any(|x| matches!(x, Failure::VerbNotListed { action: PersonAction::Blame, verb } if verb == "blame")),
        "{s}"
    );
    // a still-deferred row that names a registered verb fails too
    let mut r = rows();
    r.iter_mut()
        .find(|x| x.action == PersonAction::Purge)
        .unwrap()
        .absent = &["log"];
    let f = list_failures(&r, &Registry::kit());
    assert!(
        f.iter().any(|x| matches!(x, Failure::UnexpectedVerb { action: PersonAction::Purge, verb, .. } if verb == "log")),
        "{}",
        parity::render(&f)
    );
    // and a row deferred while the kit has cases for it
    let mut r = rows();
    let i = r
        .iter()
        .position(|x| x.action == PersonAction::Find)
        .unwrap();
    r[i] = ParityRow::deferred(PersonAction::Find, &[], "later", "test");
    let f = list_failures(&r, &Registry::kit());
    assert!(
        f.contains(&Failure::StaleDeferral(PersonAction::Find)),
        "{}",
        parity::render(&f)
    );
    // without the verb in the registry, the deferred P22 row is consistent again
    let mut r = rows();
    let i = r
        .iter()
        .position(|x| x.action == PersonAction::Blame)
        .unwrap();
    r[i] = ParityRow::deferred(PersonAction::Blame, &["blame"], "C7b", "test");
    let f = parity::check_list(&r, &Registry::kit().without("blame"), &[]);
    assert_eq!(
        f,
        vec![Failure::StaleDeferral(PersonAction::Blame)],
        "{}",
        parity::render(&f)
    );
}

#[test]
fn output_value_is_debug() {
    assert_eq!(Output::value(3), Output::Value("3".into()));
}
