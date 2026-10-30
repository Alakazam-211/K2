//! The multiplayer demo (`examples/multiplayer_demo.rs`) against its golden transcript,
//! byte for byte. The demo checks itself as it runs (exit codes, no lost text, the final
//! text); this pins everything it prints, so a change in the kit's behaviour or wording
//! shows up here as a diff.
//!
//! After an intended change, regenerate the golden file with
//! `DEMO_BLESS=1 cargo test -p k2-mpc-markdown --test demo_transcript` and review the diff.
//!
//! One test only in this binary: claim ids are numbered per process, so a second test
//! running beside it would change them.

#[path = "../examples/multiplayer_demo.rs"]
mod demo;

const GOLDEN: &str = "tests/golden/multiplayer_demo.txt";

#[test]
fn demo_matches_golden_transcript_and_is_deterministic() {
    let got = match demo::transcript() {
        Ok(t) => t,
        Err(e) => panic!("the demo went off script: {e}"),
    };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    // A second run in the same process gives the same transcript, apart from claim ids
    // (numbered per process): the demo depends on nothing but its script.
    let again = demo::transcript().expect("second run");
    assert_eq!(
        without_claim_ids(&got),
        without_claim_ids(&again),
        "two runs differ"
    );
    if std::env::var_os("DEMO_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().expect("golden dir")).expect("create golden dir");
        std::fs::write(&path, &got).expect("write golden transcript");
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {GOLDEN}: {e} (DEMO_BLESS=1 writes it)"));
    if got != want {
        let first = got
            .lines()
            .zip(want.lines())
            .position(|(g, w)| g != w)
            .unwrap_or_else(|| got.lines().count().min(want.lines().count()));
        let show = |s: &str| {
            s.lines()
                .skip(first.saturating_sub(3))
                .take(8)
                .collect::<Vec<_>>()
                .join("\n")
        };
        panic!(
            "the transcript differs from {GOLDEN} at line {}:\n--- got\n{}\n--- want\n{}\n\
             (if the change is intended: DEMO_BLESS=1 and review the diff)",
            first + 1,
            show(&got),
            show(&want)
        );
    }
}

fn without_claim_ids(s: &str) -> String {
    s.split("claim-")
        .enumerate()
        .map(|(i, p)| {
            if i == 0 {
                p
            } else {
                p.trim_start_matches(|c: char| c.is_ascii_digit())
            }
        })
        .collect::<Vec<_>>()
        .join("claim-N")
}
