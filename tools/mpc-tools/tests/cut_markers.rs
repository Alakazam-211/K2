//! A cut trap, pinned (tools/public-cut-regressions.txt): no `public-cut: drop-` marker sits in
//! a `[features]` table of any of the repo's manifests. The public cut removes a feature it
//! drops together with the comment lines right above it, so a drop-begin marker there goes
//! with the feature and leaves a drop-end without one, and the cut stops.

use std::path::{Path, PathBuf};

/// Each `PATH:LINE` of a marker inside a `[features]` table of `text`.
fn markers_in_features(path: &str, text: &str) -> Vec<String> {
    let mut table = String::new();
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            table = t.to_string();
        }
        if table == "[features]" && t.contains("public-cut: drop-") {
            out.push(format!("{path}:{}", i + 1));
        }
    }
    out
}

fn manifests(dir: &Path, out: &mut Vec<PathBuf>) {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !matches!(name.as_str(), "vendor" | "target" | ".git" | "node_modules") {
                manifests(&p, out);
            }
        } else if name == "Cargo.toml" {
            out.push(p);
        }
    }
}

#[test]
fn a_cut_marker_in_a_features_table_is_found() {
    let planted = "[package]\nname = \"x\"\n\n[features]\n# public-cut: drop-begin\nx = []\n# public-cut: drop-end\n\n[dependencies]\n";
    assert_eq!(
        markers_in_features("p/Cargo.toml", planted),
        vec!["p/Cargo.toml:5", "p/Cargo.toml:7"]
    );
    let elsewhere =
        "[workspace]\n# public-cut: drop-begin\n# public-cut: drop-end\n[features]\nx = []\n";
    assert!(
        markers_in_features("q/Cargo.toml", elsewhere).is_empty(),
        "markers outside [features] are fine"
    );
}

#[test]
fn cargo_features_carry_no_cut_markers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut found = Vec::new();
    manifests(&root, &mut found);
    assert!(
        found.len() >= 3,
        "only {} manifests found under {}",
        found.len(),
        root.display()
    );
    let mut bad = Vec::new();
    for p in &found {
        let text = std::fs::read_to_string(p).unwrap();
        bad.extend(markers_in_features(&p.display().to_string(), &text));
    }
    assert!(
        bad.is_empty(),
        "cut markers in a [features] table (the cut stops on them):\n{}",
        bad.join("\n")
    );
    println!(
        "cut markers: {} manifests checked, none in a [features] table",
        found.len()
    );
}
