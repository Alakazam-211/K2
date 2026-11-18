//! The committed DWG fixture (`crates/dwg/tests/data/draw_sampler.dwg`: the sampler's own
//! drawing, its manifest beside it) read by the DWG reader and compared with the sampler's DXF,
//! which the pack's own test generator writes here: `cad-measure dwg-draw-compare`, every space's
//! rows by handle and the header, with no difference outside the compare's named known items.

use std::path::Path;
use std::process::Command;

use k2_mpc_dxf::testgen::{draw_sampler, Ver};

/// The fixture's size, as its manifest states it.
const FIXTURE_BYTES: u64 = 11_852;

/// dwg_fixture_draws_as_its_dxf: the fixture draws as `draw_sampler`'s 2018 DXF: every row
/// matched, none differing or on one side only, no header difference, no gap; the named known
/// items a viewport's on flag (its status bits don't carry it) and every row's `read-only` edit
/// (DWG has no edit verbs).
#[test]
fn dwg_fixture_draws_as_its_dxf() {
    let dwg = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dwg/tests/data/draw_sampler.dwg");
    let size = std::fs::metadata(&dwg)
        .unwrap_or_else(|e| panic!("dwg_fixture_draws_as_its_dxf: {}: {e}", dwg.display()))
        .len();
    assert_eq!(
        size, FIXTURE_BYTES,
        "dwg_fixture_draws_as_its_dxf: the fixture's size (its manifest gives its sha256)"
    );
    let dir = std::env::temp_dir().join(format!("cad-dwg-fixture-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dwg_fixture_draws_as_its_dxf: a temporary folder");
    let dxf = dir.join("draw_sampler.dxf");
    std::fs::write(&dxf, draw_sampler(Ver::R2018).build().bytes)
        .expect("dwg_fixture_draws_as_its_dxf: write the sampler's DXF");
    let out = Command::new(env!("CARGO_BIN_EXE_cad-measure"))
        .arg("dwg-draw-compare")
        .arg(&dwg)
        .arg(&dxf)
        .output()
        .expect("dwg_fixture_draws_as_its_dxf: run cad-measure");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    for line in stdout.lines() {
        println!("dwg_fixture_draws_as_its_dxf: {line}");
    }
    assert!(
        out.status.success(),
        "dwg_fixture_draws_as_its_dxf: the compare failed ({}): {stderr}",
        out.status
    );
    // the summary: every row matched (some, never none), nothing else
    let summary = stdout
        .lines()
        .find(|l| l.starts_with("draw-compare "))
        .expect("dwg_fixture_draws_as_its_dxf: the compare's summary line");
    let field = |name: &str| -> u64 {
        let words: Vec<&str> = summary.split_whitespace().collect();
        let at = words
            .iter()
            .position(|w| *w == name)
            .unwrap_or_else(|| panic!("dwg_fixture_draws_as_its_dxf: no {name} in {summary}"));
        words[at + 1]
            .parse()
            .unwrap_or_else(|_| panic!("dwg_fixture_draws_as_its_dxf: {name} in {summary}"))
    };
    let ours = summary
        .split_whitespace()
        .skip_while(|w| *w != "ours")
        .nth(1)
        .and_then(|n| n.parse::<u64>().ok())
        .expect("dwg_fixture_draws_as_its_dxf: the rows read from the DWG");
    assert!(ours > 0, "dwg_fixture_draws_as_its_dxf: no rows: {summary}");
    assert_eq!(
        field("matched"),
        ours,
        "dwg_fixture_draws_as_its_dxf: {summary}"
    );
    for name in ["differ", "only-ours", "only-theirs", "header-diffs", "gaps"] {
        assert_eq!(
            field(name),
            0,
            "dwg_fixture_draws_as_its_dxf: {name}: {summary}"
        );
    }
    // the named known items: the viewport's on flag once, every row's read-only edit, every
    // other one never
    let read_only = format!("draw-known a DWG row's edit (read-only: no DWG edit verbs) {ours}");
    let known: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with("draw-known ") || l.starts_with("draw-gap "))
        .collect();
    assert!(
        known.contains(&"draw-known a viewport's on flag 1"),
        "dwg_fixture_draws_as_its_dxf: the viewport's on flag: {known:?}"
    );
    assert!(
        known.contains(&read_only.as_str()),
        "dwg_fixture_draws_as_its_dxf: every row's read-only edit: {known:?}"
    );
    for l in known {
        assert!(
            l == "draw-known a viewport's on flag 1" || l == read_only || l.ends_with(" 0"),
            "dwg_fixture_draws_as_its_dxf: a known item reached: {l}"
        );
    }
    assert!(
        !stdout.lines().any(|l| l.starts_with("draw-diff ")),
        "dwg_fixture_draws_as_its_dxf: a difference listed"
    );
}

/// dwg_strict_names_files_by_index: `cad-measure dwg-strict` names each file by its place in
/// the arguments (`F1`, `F2`, ...), never by its name: run twice over the fixture, every line
/// is a count line of `F1` or `F2`, each with its total, and no line holds the file's name.
#[test]
fn dwg_strict_names_files_by_index() {
    let dwg = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dwg/tests/data/draw_sampler.dwg");
    let out = Command::new(env!("CARGO_BIN_EXE_cad-measure"))
        .arg("dwg-strict")
        .arg(&dwg)
        .arg(&dwg)
        .output()
        .expect("dwg_strict_names_files_by_index: run cad-measure");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "dwg_strict_names_files_by_index: the strict check failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        !lines.is_empty(),
        "dwg_strict_names_files_by_index: no output"
    );
    for line in &lines {
        assert!(
            !line.contains("draw_sampler"),
            "dwg_strict_names_files_by_index: a line names the file: {line}"
        );
        assert!(
            line.starts_with("strict F1 ") || line.starts_with("strict F2 "),
            "dwg_strict_names_files_by_index: a line not by index: {line}"
        );
    }
    for f in ["F1", "F2"] {
        let total = format!("strict {f} total objects ");
        assert!(
            lines.iter().any(|l| l.starts_with(&total)),
            "dwg_strict_names_files_by_index: no total for {f}"
        );
    }
}
