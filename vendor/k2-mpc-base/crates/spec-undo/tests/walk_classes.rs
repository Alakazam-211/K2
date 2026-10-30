//! A short walk run of the enumerator binary (`undo_enum walk`): its SUMMARY line reports the
//! case class `concurrent_restore` and each of its parts (v0.9.2: O4 took a restoration once
//! by root, O4a fired, the base was counted by root only), and each is hit.

use std::process::Command;

#[test]
fn a_short_walk_reports_each_concurrent_restore_part() {
    let out = Command::new(env!("CARGO_BIN_EXE_undo_enum"))
        .args([
            "walk",
            "--pids",
            "2",
            "--events",
            "1",
            "--undos",
            "2",
            "--reverts",
            "1",
            "--maxlen",
            "3",
            "--init",
            "ab",
            "--probes",
            "3000",
            "--threads",
            "2",
            "--rng",
            "7",
        ])
        .output()
        .expect("undo_enum runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "undo_enum walk failed:\n{stdout}");
    let summary = stdout
        .lines()
        .find(|l| l.starts_with("SUMMARY: mode=walk"))
        .unwrap_or_else(|| panic!("no SUMMARY line:\n{stdout}"));
    let classes = summary
        .split("classes=[")
        .nth(1)
        .and_then(|s| s.split(']').next())
        .expect("a classes list");
    let count = |name: &str| -> u64 {
        classes
            .split(',')
            .find_map(|kv| kv.strip_prefix(&format!("{name}:")))
            .unwrap_or_else(|| panic!("{name} not in the SUMMARY: {summary}"))
            .parse()
            .expect("a count")
    };
    let total = count("concurrent_restore");
    for part in [
        "concurrent_restore_o4",
        "concurrent_restore_o4a",
        "concurrent_restore_base_by_root",
    ] {
        let n = count(part);
        assert!(n > 0, "{part} never hit in a short walk: {summary}");
        assert!(n <= total, "{part} ({n}) over the class ({total})");
    }
}
