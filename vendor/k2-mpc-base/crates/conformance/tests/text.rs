//! The text rules on every engine, one test per rule and engine, plus the deviation lists'
//! checks and the adapters' contract.

use k2_mpc_conformance::model::{check_lists, ModelText};
use k2_mpc_conformance::text::{self, RULES};
use k2_mpc_conformance::yrs::YrsText;
use k2_mpc_conformance::{invalid_deviation_ids, Hunk, Net, TextReplica};

/// One `#[test]` per rule for engine `$engine`; `$after` runs on each rule's final replicas.
/// Also checks that this list and `text::RULES` agree, so a new rule can't go untested.
macro_rules! text_suite {
    ($engine:ty, $after:expr) => {
        text_suite!(@ $engine, $after;
            converge = "text.converge",
            no_interleave_forward = "text.no_interleave_forward",
            no_interleave_backward = "text.no_interleave_backward",
            no_interleave_backward_multi = "text.no_interleave_backward_multi",
            keep_concurrent_insert = "text.keep_concurrent_insert",
            delete_once = "text.delete_once",
            edit_atomic = "text.edit_atomic",
            yata_separating_case = "text.yata_separating_case",
            yata_paper_a19 = "text.yata_paper_a19",
            scalar_safe = "text.scalar_safe",
        );
    };
    (@ $engine:ty, $after:expr; $($name:ident = $id:literal,)*) => {
        $(
            #[test]
            fn $name() {
                let net = text::run::<$engine>($id);
                ($after)($id, &net);
            }
        )*

        #[test]
        fn every_rule_has_a_test() {
            let mut tested = vec![$($id),*];
            let mut all = RULES.to_vec();
            tested.sort_unstable();
            all.sort_unstable();
            assert_eq!(tested, all, "text::RULES and this suite's tests differ");
        }
    };
}

mod yrs {
    use super::*;
    text_suite!(YrsText, |_id: &str, _net: &Net<YrsText>| {});
}

mod model {
    use super::*;
    // The reference model also checks Def. 4 and the strong list specification on every
    // rule's final state (the interleaving rules, and the others too since they pass): an
    // extra assertion, never a deviation.
    text_suite!(ModelText, |id: &str, net: &Net<ModelText>| {
        if let Err(e) = check_lists(&net.r) {
            panic!("{id}: mpce-model's checkers reject the final state: {e}");
        }
    });
}

// ---------- deviation lists (per engine) ----------

#[test]
fn deviation_ids_valid() {
    assert_eq!(invalid_deviation_ids::<YrsText>(), Vec::<&str>::new());
    assert_eq!(invalid_deviation_ids::<ModelText>(), Vec::<&str>::new());
}

#[test]
fn model_list_is_empty() {
    assert!(
        ModelText::KNOWN_DEVIATIONS.is_empty(),
        "the reference model must conform with an empty list: {:?}",
        ModelText::KNOWN_DEVIATIONS
    );
}

#[test]
fn yrs_list_is_pinned() {
    let ids: Vec<&str> = YrsText::KNOWN_DEVIATIONS.iter().map(|(r, _)| *r).collect();
    assert_eq!(
        ids,
        [
            "text.yata_separating_case",
            "text.yata_paper_a19",
            "core.causal",
            "core.no_equivocation",
            "hist.undo_atomic",
        ],
        "the kit Doc's deviations changed: update this test only with the rule's evidence"
    );
}

/// A misspelled id is caught (negative control for `deviation_ids_valid`).
#[test]
fn a_misspelled_deviation_is_reported() {
    struct Typo;
    impl TextReplica for Typo {
        const ENGINE: &'static str = "typo";
        const KNOWN_DEVIATIONS: &'static [(&'static str, &'static str)] = &[
            ("text.delete_once", "fine"),
            ("text.yata_separting_case", "misspelled"),
        ];
        type Version = ();
        fn replica(_: u64, _: &str) -> Self {
            Typo
        }
        fn apply(&mut self, _: &[Hunk]) -> Result<(), k2_mpc_conformance::Refused> {
            Ok(())
        }
        fn sync_from(&mut self, _: &Self) {}
        fn text(&self) -> String {
            String::new()
        }
        fn version(&self) {}
    }
    assert_eq!(
        invalid_deviation_ids::<Typo>(),
        ["text.yata_separting_case"]
    );
}

// ---------- the adapters' contract ----------

fn contract<E: TextReplica>() {
    let seed = "a🎉é\n";
    let mut r = E::replica(101, seed);
    assert_eq!(r.text(), seed, "{}", E::ENGINE);
    // Replicas made separately share the seed: syncing adds nothing.
    let mut other = E::replica(102, seed);
    let before = other.version();
    other.sync_from(&r);
    assert_eq!(other.version(), before, "{}: the seed differs", E::ENGINE);

    // Off a scalar boundary (inside 🎉, inside é), out of range, out of order: refused,
    // nothing changes.
    let h = |start: usize, end: usize, s: &str| Hunk {
        start,
        end,
        insert: s.into(),
    };
    for bad in [
        vec![h(2, 2, "x")],
        vec![h(1, 3, "")],
        vec![h(6, 7, "")],
        vec![h(0, 0, "x"), h(9, 9, "y")],
        vec![h(5, 7, "x"), h(0, 1, "y")],
    ] {
        assert!(r.apply(&bad).is_err(), "{}: accepted {bad:?}", E::ENGINE);
        assert_eq!(
            r.text(),
            seed,
            "{}: a refused batch changed text",
            E::ENGINE
        );
    }

    // A multi-hunk batch with replace, delete and adjacent insert: the kit's diff::apply.
    let batch = vec![h(0, 1, "AB"), h(5, 7, ""), h(7, 7, "Z"), h(8, 8, "!")];
    let want = k2_mpc_base::text::diff::apply(seed, &batch);
    r.apply(&batch).unwrap();
    assert_eq!(r.text(), want, "{}", E::ENGINE);
    other.sync_from(&r);
    assert_eq!(other.text(), want, "{}", E::ENGINE);
    assert_eq!(other.version(), r.version(), "{}", E::ENGINE);

    // Touching hunks (the next starts where the previous ends) are accepted unless both are
    // pure inserts at one offset (ambiguous order): diff::validate's rule, on every engine.
    let seed = "0123456789\n";
    for batch in [
        vec![h(2, 5, "R"), h(5, 5, "I")],
        vec![h(2, 5, ""), h(5, 7, "R")],
        vec![h(2, 2, "I"), h(2, 4, "")],
        vec![h(5, 5, "Y"), h(5, 7, "R")],
    ] {
        let mut r = E::replica(101, seed);
        let mut other = E::replica(102, seed);
        let want = k2_mpc_base::text::diff::apply(seed, &batch);
        if let Err(e) = r.apply(&batch) {
            panic!("{}: refused touching hunks {batch:?}: {e:?}", E::ENGINE);
        }
        assert_eq!(r.text(), want, "{}: {batch:?}", E::ENGINE);
        other.sync_from(&r);
        assert_eq!(other.text(), want, "{}: {batch:?}", E::ENGINE);
    }
    let mut r = E::replica(101, seed);
    let two_inserts = vec![h(3, 3, "a"), h(3, 3, "b")];
    assert!(
        r.apply(&two_inserts).is_err(),
        "{}: two inserts at one offset",
        E::ENGINE
    );
    assert_eq!(r.text(), seed, "{}", E::ENGINE);
}

#[test]
fn yrs_adapter_contract() {
    contract::<YrsText>();
}

#[test]
fn model_adapter_contract() {
    contract::<ModelText>();
}
