//! The edit path's runtime check (the canary, v0.9.3), at the doc: a divergence planted in
//! the fast path (test builds only: hunks applied front to back, the same text with other
//! bytes) is caught on a sampled edit, **never served** (the doc holds the throwaway
//! replica's result, byte for byte), and the doc stays on the throwaway path. Without the
//! check, the planted divergence is served: the control that shows it is what catches it.
//! The session's events and the text cache: `session/engine_check_tests.rs`.

use k2_mpc_base::differential::{with_side, Path, Side};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::{Doc, Edit, Kind, Schema};

const S: Schema = Schema {
    name: "t",
    roots: &[("source", Kind::Text)],
};

/// A change inserting at two places (where front to back and back to front differ).
fn two_inserts(text: &str, n: usize) -> Vec<Edit> {
    let mid = text.len() / 2;
    let mid = (0..=mid).rev().find(|&k| text.is_char_boundary(k)).unwrap();
    vec![Edit::Text {
        root: "source".into(),
        hunks: vec![
            Hunk {
                start: 0,
                end: 0,
                insert: format!("<{n}>"),
            },
            Hunk {
                start: mid,
                end: mid,
                insert: format!("[{n}]"),
            },
        ],
    }]
}

/// Ten two-place edits on a doc through `side`, checking one edit in `every`; the updates.
fn run(side: &mut Side, every: u32) -> (Vec<Vec<u8>>, Vec<u8>, bool) {
    let mut doc = Doc::new(S).unwrap();
    doc.set_edit_check_every(every);
    doc.replace_text_as(5, "source", "seed text, a line\nand another\n")
        .unwrap();
    let mut updates = Vec::new();
    for n in 0..10 {
        let t = doc.text("source").unwrap();
        let edits = two_inserts(&t, n);
        let u = with_side(side, || doc.edit_as(100 + n as u64 % 3, &edits))
            .unwrap()
            .unwrap();
        updates.push(u);
    }
    (
        updates,
        doc.encode_state().unwrap(),
        doc.edit_path_fallback(),
    )
}

#[test]
fn a_planted_divergence_is_caught_and_never_served() {
    let (want, want_state, _) = run(&mut Side::new(Path::Throwaway), 0);
    // checked on every edit: the first two-place edit is caught; every update served is the
    // throwaway's, and the doc ends on the throwaway path
    let (got, got_state, fallback) = run(&mut Side::new(Path::MainDoc).planting_divergence(), 1);
    assert_eq!(got, want, "a served update differs from the throwaway's");
    assert_eq!(got_state, want_state);
    assert!(
        fallback,
        "the doc stays on the throwaway path after a mismatch"
    );
    // the control: not checked, the planted divergence is served
    let (served, _, fallback) = run(&mut Side::new(Path::MainDoc).planting_divergence(), 0);
    assert_ne!(served, want, "the planted divergence must change the bytes");
    assert!(!fallback);
    // and unplanted, checked on every edit: identical, no fallback
    let (clean, clean_state, fallback) = run(&mut Side::new(Path::MainDoc), 1);
    assert_eq!(clean, want);
    assert_eq!(clean_state, want_state);
    assert!(!fallback);
}
