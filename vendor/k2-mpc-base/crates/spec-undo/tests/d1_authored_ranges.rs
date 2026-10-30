//! The undo spec's D1 amendment (v1.10, `.k2/prds/amendment-D1-authored-ranges.md`): a change
//! is never built from someone else's text. Scaffolding, on the oracle, ahead of the build
//! (after v0.9.5 is tagged, as v0.9.7).
//!
//! - **Case classes:** (a) a line replace over another's recent edit, then the earlier author
//!   undoes; (b) two participants edit different words of one line concurrently, an agent's
//!   `write --base` merges both, and each can still undo their own word; (c) a one-word edit
//!   in a long one-line paragraph leaves the co-authors' blame and undo intact.
//! - (b) and (c) held before the amendment and must keep holding: they guard it against
//!   rewriting more than the authored ranges. (a) failed before it (the D1 spec gap).
//! - **Mutants that must fail** (the `mutants` feature; outside `Mutant::ALL` until the
//!   Quint model has D1):
//!   `reuse-foreign-elements-in-changed-token` (today's D1) fails (a); `rewrite-whole-line`
//!   fails (c); `rewrite-merged-ranges` fails (b).

use std::sync::Mutex;

use spec_undo::model::{Bounds, Mode, Model, Outcome, Request};

/// The mutant switch is process-wide: every test here holds this lock.
static LOCK: Mutex<()> = Mutex::new(());

fn locked(f: fn()) {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    f()
}

fn one(text: &str) -> Model {
    Model::new(vec![text.into()], 1, Bounds::default())
}

fn undo1(m: &mut Model, pid: &str) -> Outcome {
    m.undo(pid, None, &Request::new(1, Mode::All)).0
}

fn applied(o: &Outcome) -> bool {
    matches!(o, Outcome::Applied { steps: 1, .. })
}

/// The owner of every live character, as the oracle records it (blame by inserter).
fn owners(m: &Model) -> Vec<u16> {
    m.elems[0].iter().map(|e| e.owner).collect()
}

/// (a) the soak's failure (seed 1, call 241,426; released v0.9.4 too) in four calls.
fn class_a_a_line_replace_over_another_s_edit_then_their_undo() {
    let mut m = one("seed\n");
    m.replace("p16", None, 0, "seed\np16:3027\n");
    m.replace("p16", None, 0, "seed\np16:3029\n");
    m.replace("p14", None, 0, "seed\np14:3169\n");
    let o = undo1(&mut m, "p16");
    assert_eq!(m.texts[0], "seed\np14:3169\n", "p14's text is untouched");
    assert!(matches!(o, Outcome::Conflicts { step: 1, .. }), "{o:?}");
}

/// (b) concurrent words merged by an agent's `write --base`: every author undoes their own.
fn class_b_concurrent_words_merged_by_write_base_each_undoes_their_own() {
    let base = "alpha beta gamma\n";
    let mut m = one(base);
    m.replace("p1", None, 0, "alpha BETA gamma\n");
    m.replace("p2", None, 0, "alpha BETA GAMMA\n");
    // the agent read `base` and changed only its own word
    let (o, _) = m.write_base("ag", None, 0, base, "ALPHA beta gamma\n");
    assert!(matches!(o, Outcome::Changed(Some(_))), "{o:?}");
    assert_eq!(m.texts[0], "ALPHA BETA GAMMA\n");
    assert!(applied(&undo1(&mut m, "p1")), "p1 undoes BETA");
    assert_eq!(m.texts[0], "ALPHA beta GAMMA\n");
    assert!(applied(&undo1(&mut m, "p2")), "p2 undoes GAMMA");
    assert_eq!(m.texts[0], "ALPHA beta gamma\n");
    assert!(applied(&undo1(&mut m, "ag")), "the agent undoes ALPHA");
    assert_eq!(m.texts[0], base);
}

/// (c) prose: one word changed in a long paragraph three people wrote; their blame and
/// undo are intact.
fn class_c_a_one_word_edit_in_prose_keeps_the_co_authors() {
    let mut m = one("");
    m.replace("p1", None, 0, "The quick brown fox");
    m.replace("p2", None, 0, "The quick brown fox jumps over");
    m.replace(
        "p3",
        None,
        0,
        "The quick brown fox jumps over the lazy dog.\n",
    );
    let before = owners(&m);
    m.replace(
        "ag",
        None,
        0,
        "The quick brown fox leaps over the lazy dog.\n",
    );
    let after = owners(&m);
    // every character outside the changed word keeps its author
    let (w0, w1) = (20, 25); // "jumps" -> "leaps", same length
    assert_eq!(before[..w0], after[..w0], "blame before the word");
    assert_eq!(before[w1..], after[w1..], "blame after the word");
    // the co-authors' undo still applies (newest first, as each is their own)
    assert!(applied(&undo1(&mut m, "p3")), "p3 undoes its clause");
    assert!(
        applied(&undo1(&mut m, "p1")),
        "p1 undoes its sentence start"
    );
}

#[test]
fn class_a_a_line_replace_over_another_s_edit_then_their_undo_holds() {
    locked(class_a_a_line_replace_over_another_s_edit_then_their_undo);
}

#[test]
fn class_b_concurrent_words_merged_by_write_base_each_undoes_their_own_holds() {
    locked(class_b_concurrent_words_merged_by_write_base_each_undoes_their_own);
}

#[test]
fn class_c_a_one_word_edit_in_prose_keeps_the_co_authors_holds() {
    locked(class_c_a_one_word_edit_in_prose_keeps_the_co_authors);
}

/// The three mutants of the amendment, each against the class it must fail (the `mutants`
/// feature). Outside `Mutant::ALL` until the Quint model has D1.
#[cfg(feature = "mutants")]
mod mutants {
    use spec_undo::model::mutant::{self, Mutant};

    /// Run `f` with mutant `m` active; the oracle of record after.
    fn with<R>(m: Mutant, f: impl FnOnce() -> R) -> R {
        mutant::set(m);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        mutant::set(Mutant::None);
        r.unwrap_or_else(|e| std::panic::resume_unwind(e))
    }

    fn fails(m: Mutant, class: fn()) -> bool {
        with(m, || std::panic::catch_unwind(class).is_err())
    }

    #[test]
    fn each_d1_mutant_fails_its_class() {
        let _g = super::LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // one process-wide switch: the three run in turn in one test
        assert!(
            fails(
                Mutant::D1ReuseForeign,
                super::class_a_a_line_replace_over_another_s_edit_then_their_undo
            ),
            "reuse-foreign-elements-in-changed-token must fail class (a)"
        );
        assert!(
            fails(
                Mutant::D1RewriteWholeLine,
                super::class_c_a_one_word_edit_in_prose_keeps_the_co_authors
            ),
            "rewrite-whole-line must fail class (c)"
        );
        assert!(
            fails(
                Mutant::D1RewriteMergedRanges,
                super::class_b_concurrent_words_merged_by_write_base_each_undoes_their_own
            ),
            "rewrite-merged-ranges must fail class (b)"
        );
        // and the oracle passes all three
        super::class_a_a_line_replace_over_another_s_edit_then_their_undo();
        super::class_b_concurrent_words_merged_by_write_base_each_undoes_their_own();
        super::class_c_a_one_word_edit_in_prose_keeps_the_co_authors();
    }
}
