//! Agent parity for Markdown (K-PAR-1…3): every row of `docs/PARITY.md` through the kit's
//! harness. Each person action runs the person's way (a tab, the session API) and the
//! agent's way (registered verbs only), and the effects must match. Fails loudly on a
//! missing verb, an unmapped row or any difference.

use std::sync::Arc;
use std::time::Instant;

use k2_mpc_base::parity::{
    self, edit_summary, Agent, Case, Failure, Output, ParityRow, Person, PersonAction, Registry,
    RowStatus, Step, VerbValue, DOC,
};
use k2_mpc_base::session::Pack;
use k2_mpc_markdown::parity::{parity, parity_row};
use k2_mpc_markdown::PACK;

type R = Result<Step, String>;
const P13: PersonAction = PersonAction::Paste;
const VERB: &str = "replace-section";

fn pack() -> Arc<dyn Pack> {
    Arc::new(PACK)
}

/// The person types exactly the change that turns the live text into `expected` (an
/// independent oracle: `expected` is written out by hand in each case).
fn person_types(p: &mut Person<'_>, expected: &str) -> R {
    let live = p.text()?;
    let hunks = k2_mpc_base::text::diff::hunks(&live, expected);
    match p.edit(hunks) {
        Ok(r) => Ok(Step::ok(Output::Value(edit_summary(&r)))),
        Err(e) => Ok(Step::exit(
            k2_mpc_base::tools::session_exit(&e),
            Output::None,
        )),
    }
}

fn agent_runs(a: &mut Agent<'_>, argv: &[&str], body: &str) -> R {
    let out = a.verb(argv, body.as_bytes())?;
    if let Some(why) = out.refused {
        return Ok(Step::exit(out.exit, Output::Text(why)));
    }
    let VerbValue::Tool(t) = &out.value else {
        return Err(format!("{VERB} gave {:?}", out.value));
    };
    if !t.stdout.is_empty() && out.exit == 0 {
        return Err(format!(
            "unexpected report: {:?}",
            String::from_utf8_lossy(&t.stdout)
        ));
    }
    Ok(Step::exit(
        out.exit,
        Output::Value(t.changed.as_ref().map_or("no change".into(), edit_summary)),
    ))
}

// ---- 1. the body under an ATX heading ----

fn atx_person(p: &mut Person<'_>) -> R {
    person_types(
        p,
        &DOC.replace("- ship the core\n- keep files plain\n", "- one\n- two\n"),
    )
}
fn atx_agent(a: &mut Agent<'_>) -> R {
    agent_runs(a, &[VERB, "--heading", "## Goals"], "- one\n- two\n")
}

// ---- 2. --include-heading ----

fn include_person(p: &mut Person<'_>) -> R {
    person_types(
        p,
        &DOC.replace(
            "## Notes\n\nTODO: write more.\nEnd.\n",
            "## Later\n\nNothing yet.\n",
        ),
    )
}
fn include_agent(a: &mut Agent<'_>) -> R {
    agent_runs(
        a,
        &[VERB, "--heading", "## Notes", "--include-heading"],
        "## Later\n\nNothing yet.\n",
    )
}

// ---- 3. a setext heading ----

const SETEXT: &str = "Title\n=====\n\nIntro.\n\nPart\n----\n\nOld body.\n\nNext\n----\n\nTail.\n";

fn setext_person(p: &mut Person<'_>) -> R {
    person_types(p, &SETEXT.replace("Old body.", "New body."))
}
fn setext_agent(a: &mut Agent<'_>) -> R {
    agent_runs(a, &[VERB, "--heading", "## Part"], "New body.\n")
}

// ---- 4. --nth, with a look-alike heading in a fence ----

const FENCED: &str = "# Log\n\n```md\n## Notes\n```\n\n## Notes\n\nfirst\n\n## Notes\n\nsecond\n";

fn nth_person(p: &mut Person<'_>) -> R {
    person_types(p, &FENCED.replace("second", "updated"))
}
fn nth_agent(a: &mut Agent<'_>) -> R {
    agent_runs(
        a,
        &[VERB, "--heading", "## Notes", "--nth", "2"],
        "updated\n",
    )
}

// ---- 5. ambiguous without --nth: exit 1 with the lines, nothing applied ----

fn ambiguous_person(p: &mut Person<'_>) -> R {
    // the tab's outline shows two "## Notes" sections (the fenced one is code)
    p.text()?;
    Ok(Step::exit(1, Output::Lines(vec![7, 11])))
}
fn ambiguous_agent(a: &mut Agent<'_>) -> R {
    let out = a.verb(&[VERB, "--heading", "## Notes"], b"x\n")?;
    let VerbValue::Tool(t) = &out.value else {
        return Err(format!("{:?}", out.value));
    };
    if t.changed.is_some() {
        return Err("an ambiguous heading applied something".into());
    }
    let report = String::from_utf8_lossy(&t.stdout).into_owned();
    let lines = report
        .split_once("at lines ")
        .and_then(|(_, r)| r.split_once(';'))
        .map(|(l, _)| l.split(", ").filter_map(|n| n.parse().ok()).collect())
        .ok_or_else(|| format!("no line list in {report:?}"))?;
    Ok(Step::exit(out.exit, Output::Lines(lines)))
}

// ---- 6. not found: exit 1, nothing applied ----

fn missing_person(p: &mut Person<'_>) -> R {
    p.text()?;
    Ok(Step::exit(1, Output::None))
}
fn missing_agent(a: &mut Agent<'_>) -> R {
    let out = a.verb(&[VERB, "--heading", "## Missing"], b"x\n")?;
    if out.stdout() != b"replace-section: notes.md: no section '## Missing'\n" {
        return Err(format!(
            "report {:?}",
            String::from_utf8_lossy(out.stdout())
        ));
    }
    Ok(Step::exit(out.exit, Output::None))
}

fn cases() -> Vec<Case> {
    vec![
        Case::new(
            P13,
            VERB,
            "the body under an ATX heading",
            atx_person,
            atx_agent,
        ),
        Case::new(
            P13,
            VERB,
            "--include-heading",
            include_person,
            include_agent,
        ),
        Case::new(P13, VERB, "a setext heading", setext_person, setext_agent).initial(SETEXT),
        Case::new(
            P13,
            VERB,
            "--nth; a fenced look-alike",
            nth_person,
            nth_agent,
        )
        .initial(FENCED),
        Case::new(
            P13,
            VERB,
            "ambiguous: exit 1, lines",
            ambiguous_person,
            ambiguous_agent,
        )
        .initial(FENCED),
        Case::new(
            P13,
            VERB,
            "not found: exit 1",
            missing_person,
            missing_agent,
        ),
    ]
}

/// K-PAR-3: every row, the person's way and the agent's way.
#[test]
fn agent_parity() {
    let t = Instant::now();
    let registry = Registry::for_pack(&PACK).expect("registry");
    let report = parity::check(pack(), &parity(), &registry, &cases())
        .unwrap_or_else(|f| panic!("{}", parity::render(&f)));
    let rows = parity();
    let live = rows.iter().filter(|r| r.status == RowStatus::Live).count();
    assert_eq!(live, 28, "P24 Export is live since kit v0.9.7");
    assert!(report.not_applicable.is_empty());
    let deferred: Vec<PersonAction> = report.deferred.iter().map(|d| d.0).collect();
    assert_eq!(
        deferred,
        [
            PersonAction::Annotate,
            PersonAction::Mention,
            PersonAction::Purge
        ]
    );
    // every live row was exercised, and every pack case ran
    for r in rows.iter().filter(|r| r.status == RowStatus::Live) {
        for v in r.verbs {
            assert!(
                report.matched.iter().any(|m| m.0 == r.action && m.1 == *v),
                "{} {v} not exercised",
                r.action
            );
        }
    }
    assert_eq!(report.matched.iter().filter(|m| m.1 == VERB).count(), 6);
    // P22 (live since base v0.3.0): the gutter and the verb, -L, and the history right
    assert_eq!(
        report
            .matched
            .iter()
            .filter(|m| m.0 == PersonAction::Blame)
            .count(),
        3
    );
    eprintln!(
        "agent parity (markdown): {} cases matched, {live} live rows, 1 N/A, {} deferred, in {:?}",
        report.matched.len(),
        report.deferred.len(),
        t.elapsed()
    );
}

/// K-PAR-2: the pack's registry has its own verb, serving P13, and covers every live row.
#[test]
fn registry_covers_actions() {
    let r = Registry::for_pack(&PACK).unwrap();
    let v = r.get(VERB).expect("replace-section is registered");
    assert_eq!(v.serves, &[P13]);
    assert_eq!(v.source, parity::Source::Pack);
    for a in PersonAction::ALL {
        if parity_row(*a).status == RowStatus::Live {
            assert!(r.serving(*a).count() > 0, "{a}");
        }
    }
}

/// Negative control: with `replace-section` taken out of the registry, the harness reports
/// it missing, from the list and from the agent path, and nothing else.
#[test]
fn unmapping_replace_section_fails() {
    let without = Registry::for_pack(&PACK).unwrap().without(VERB);
    let f = parity::check_list(&parity(), &without, &cases());
    assert_eq!(
        f,
        vec![Failure::MissingVerb {
            action: P13,
            verb: VERB.into(),
            at: "parity list",
        }],
        "{}",
        parity::render(&f)
    );
    let f = parity::run_case(&pack(), &without, &cases()[0]);
    assert_eq!(
        f,
        vec![Failure::MissingVerb {
            action: P13,
            verb: VERB.into(),
            at: "agent path",
        }],
        "{}",
        parity::render(&f)
    );
    // and a row with no case for the verb is caught
    let f = parity::check_list(&parity(), &Registry::for_pack(&PACK).unwrap(), &[]);
    assert_eq!(
        f,
        vec![Failure::NoCase {
            action: P13,
            verb: VERB.into()
        }],
        "{}",
        parity::render(&f)
    );
}

/// Negative control: v0.1.0's P22 row (deferred, `blame` absent) fails now that the base
/// registers `blame` and has cases for it; and with `blame` out of the registry the live row
/// is reported missing.
#[test]
fn the_deferred_blame_row_now_fails() {
    let registry = Registry::for_pack(&PACK).unwrap();
    let mut rows = parity();
    let i = rows
        .iter()
        .position(|r| r.action == PersonAction::Blame)
        .unwrap();
    rows[i] = ParityRow::deferred(
        PersonAction::Blame,
        &["blame"],
        "C7b (the release after MD-v1)",
        "blame lands with its UI",
    );
    let f = parity::check_list(&rows, &registry, &cases());
    let s = parity::render(&f);
    assert!(
        f.iter().any(|x| matches!(x, Failure::UnexpectedVerb { action: PersonAction::Blame, verb, .. } if verb == "blame")),
        "{s}"
    );
    assert!(
        f.contains(&Failure::StaleDeferral(PersonAction::Blame)),
        "{s}"
    );
    let f = parity::check_list(&parity(), &registry.without("blame"), &cases());
    assert_eq!(
        f,
        vec![Failure::MissingVerb {
            action: PersonAction::Blame,
            verb: "blame".into(),
            at: "parity list",
        }],
        "{}",
        parity::render(&f)
    );
}

/// Negative control: a replace-section that does something else than the person did fails.
#[test]
fn a_wrong_section_is_a_mismatch() {
    fn wrong(a: &mut Agent<'_>) -> R {
        agent_runs(a, &[VERB, "--heading", "## Notes"], "- one\n- two\n")
    }
    let bad = Case::new(P13, VERB, "planted: wrong section", atx_person, wrong);
    let f = parity::run_case(&pack(), &Registry::for_pack(&PACK).unwrap(), &bad);
    assert!(
        matches!(&f[..], [Failure::Mismatch { diffs, .. }] if diffs.iter().any(|d| d.starts_with("text:"))),
        "{}",
        parity::render(&f)
    );
}

/// K-PAR-1: `docs/PARITY.md` has a row for every action, in order, naming every verb the
/// data lists (and the data has a row for every action, by its exhaustive match).
#[test]
fn every_pack_has_a_table() {
    let md = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/PARITY.md"))
        .expect("docs/PARITY.md");
    let rows: Vec<(u8, &str)> = md
        .lines()
        .filter_map(|l| {
            let cells: Vec<&str> = l.split('|').map(str::trim).collect();
            let n = cells
                .get(1)?
                .trim_matches('*')
                .strip_prefix('P')?
                .parse()
                .ok()?;
            Some((n, *cells.get(3)?))
        })
        .collect();
    let numbers: Vec<u8> = rows.iter().map(|r| r.0).collect();
    assert_eq!(
        numbers,
        (1..=31).collect::<Vec<u8>>(),
        "PARITY.md rows P1..P31 in order"
    );
    for row in parity() {
        let ParityRow { action, verbs, .. } = row;
        let (_, cell) = rows[usize::from(action.number()) - 1];
        let spans: Vec<&str> = cell.split('`').skip(1).step_by(2).collect();
        for v in verbs {
            assert!(
                spans
                    .iter()
                    .any(|s| *s == *v || s.starts_with(&format!("{v} "))),
                "{action}: PARITY.md's verb column ({cell}) doesn't name `{v}`"
            );
        }
    }
}
