//! Agent parity for the text pack (K-PAR-1…3): every row of `docs/PARITY.md` through the
//! kit's harness. Each person action runs the person's way (a tab, the session API) and the
//! agent's way (registered verbs only), and the effects must match. The pack adds no verbs,
//! so every case is the kit's; this proves the kit's text tier is whole on this pack's
//! codec. Fails loudly on a missing verb, an unmapped row or any difference.

use std::sync::Arc;
use std::time::Instant;

use k2_mpc_base::parity::{self, Failure, ParityRow, PersonAction, Registry, RowStatus, Source};
use k2_mpc_base::session::Pack;
use k2_mpc_text::parity::{parity, parity_row};
use k2_mpc_text::{TextPack, PACK};

fn packs() -> Vec<(&'static str, TextPack)> {
    vec![
        ("unknown type", PACK),
        ("code", TextPack::for_path("main.rs").unwrap()),
        ("windows code", TextPack::for_path("run.bat").unwrap()),
        ("config", TextPack::for_path("Cargo.toml").unwrap()),
        ("logs", TextPack::for_path("app.log").unwrap()),
    ]
}

/// K-PAR-3: every row, the person's way and the agent's way, on the pack for each kind of
/// type (they differ only in how a file is read in, so all must pass).
#[test]
fn agent_parity() {
    for (what, pack) in packs() {
        let t = Instant::now();
        let registry = Registry::for_pack(&pack).expect("registry");
        let p: Arc<dyn Pack> = Arc::new(pack);
        let report = parity::check(p, &parity(), &registry, &[])
            .unwrap_or_else(|f| panic!("{what}: {}", parity::render(&f)));
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
        // every live row's every verb was exercised
        for r in rows.iter().filter(|r| r.status == RowStatus::Live) {
            for v in r.verbs {
                assert!(
                    report.matched.iter().any(|m| m.0 == r.action && m.1 == *v),
                    "{what}: {} {v} not exercised",
                    r.action
                );
            }
        }
        // P22: the gutter and the verb, -L, and the history right
        assert_eq!(
            report
                .matched
                .iter()
                .filter(|m| m.0 == PersonAction::Blame)
                .count(),
            3
        );
        eprintln!(
            "agent parity (text, {what}): {} cases matched, {live} live rows, 1 N/A, {} deferred, in {:?}",
            report.matched.len(),
            report.deferred.len(),
            t.elapsed()
        );
    }
}

/// K-PAR-2: the registry is the kit's alone (no pack verbs) and covers every live row.
#[test]
fn registry_covers_actions_with_kit_verbs_only() {
    let r = Registry::for_pack(&PACK).unwrap();
    assert!(PACK.verbs().is_empty());
    for a in PersonAction::ALL {
        if parity_row(*a).status == RowStatus::Live {
            assert!(r.serving(*a).count() > 0, "{a}");
            assert!(
                r.serving(*a).all(|v| v.source != Source::Pack),
                "{a}: a pack verb"
            );
        }
    }
}

/// Negative control: with `patch` taken out of the registry, the harness reports P13's verb
/// missing from the list.
#[test]
fn unmapping_a_verb_fails() {
    let without = Registry::for_pack(&PACK).unwrap().without("patch");
    let f = parity::check_list(&parity(), &without, &[]);
    assert!(
        f.contains(&Failure::MissingVerb {
            action: PersonAction::Paste,
            verb: "patch".into(),
            at: "parity list",
        }),
        "{}",
        parity::render(&f)
    );
}

/// Negative control: a row naming a verb nobody registered (a `format` verb for P15) fails.
#[test]
fn an_invented_verb_fails() {
    let mut rows = parity();
    let i = rows
        .iter()
        .position(|r| r.action == PersonAction::Format)
        .unwrap();
    rows[i] = ParityRow::live(PersonAction::Format, &["format"]);
    let f = parity::check_list(&rows, &Registry::for_pack(&PACK).unwrap(), &[]);
    assert!(
        f.iter().any(|x| matches!(x, Failure::MissingVerb { action: PersonAction::Format, verb, .. } if verb == "format")),
        "{}",
        parity::render(&f)
    );
}

/// Negative control: P22 deferred (blame absent) fails, since the kit registers `blame`.
#[test]
fn a_stale_deferral_fails() {
    let registry = Registry::for_pack(&PACK).unwrap();
    let mut rows = parity();
    let i = rows
        .iter()
        .position(|r| r.action == PersonAction::Blame)
        .unwrap();
    rows[i] = ParityRow::deferred(PersonAction::Blame, &["blame"], "later", "planted");
    let f = parity::check_list(&rows, &registry, &[]);
    let s = parity::render(&f);
    assert!(
        f.contains(&Failure::StaleDeferral(PersonAction::Blame)),
        "{s}"
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
    assert!(
        !md.contains("replace-section"),
        "the text pack has no replace-section"
    );
}
