//! A cargo-deny-style licence check of what `k2-deckrender` ships (normal
//! and build dependencies, the vendored deckcraft crates included), for each
//! target K2 builds it for: every licence on the allowlist, and no git source
//! anywhere in the tree. `deny.toml` (cargo deny) checks the same with the
//! advisories; this runs with every `cargo test`. The listing it prints
//! (`[licence] …`) is the shipped tree.
//!
//! Liberation Sans (OFL-1.1) is data compiled in through deckcraft-fonts, not
//! a crate: NOTICE.md and vendor/deckcraft/VENDORED.md cover it.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::process::Command;

const ALLOWED: &[&str] = &["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0"];

const TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

/// SPDX expression: `a OR b`, `a AND b`, `(...)`, `id WITH exception`, and
/// the legacy `a/b` (= OR). An exception only adds permissions, so
/// `X WITH Y` is allowed when `X` is.
fn spdx_allowed(expr: &str) -> Result<bool, String> {
    let spaced = expr.replace('(', " ( ").replace(')', " ) ").replace('/', " OR ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    let mut pos = 0;
    let v = or_expr(&tokens, &mut pos)?;
    if pos != tokens.len() {
        return Err(format!("trailing tokens in {expr:?}"));
    }
    Ok(v)
}

fn or_expr(t: &[&str], pos: &mut usize) -> Result<bool, String> {
    let mut v = and_expr(t, pos)?;
    while t.get(*pos) == Some(&"OR") {
        *pos += 1;
        let r = and_expr(t, pos)?;
        v = v || r;
    }
    Ok(v)
}

fn and_expr(t: &[&str], pos: &mut usize) -> Result<bool, String> {
    let mut v = atom(t, pos)?;
    while t.get(*pos) == Some(&"AND") {
        *pos += 1;
        let r = atom(t, pos)?;
        v = v && r;
    }
    Ok(v)
}

fn atom(t: &[&str], pos: &mut usize) -> Result<bool, String> {
    match t.get(*pos) {
        Some(&"(") => {
            *pos += 1;
            let v = or_expr(t, pos)?;
            if t.get(*pos) != Some(&")") {
                return Err("unbalanced parentheses".into());
            }
            *pos += 1;
            Ok(v)
        }
        Some(id) => {
            *pos += 1;
            let ok = ALLOWED.contains(id);
            if t.get(*pos) == Some(&"WITH") {
                *pos += 2;
            }
            Ok(ok)
        }
        None => Err("empty expression".into()),
    }
}

fn metadata(target: &str) -> Value {
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--locked", "--filter-platform", target, "--manifest-path", manifest])
        .output()
        .expect("run cargo metadata");
    assert!(out.status.success(), "cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("metadata json")
}

#[test]
fn spdx_parser() {
    assert_eq!(spdx_allowed("MIT OR Apache-2.0"), Ok(true));
    assert_eq!(spdx_allowed("MIT/Apache-2.0"), Ok(true));
    assert_eq!(spdx_allowed("(MIT OR Apache-2.0) AND Unicode-3.0"), Ok(true));
    assert_eq!(spdx_allowed("CC0-1.0 OR MIT-0 OR Apache-2.0"), Ok(true));
    assert_eq!(spdx_allowed("Apache-2.0 WITH LLVM-exception"), Ok(true));
    assert_eq!(spdx_allowed("MPL-2.0"), Ok(false));
    assert_eq!(spdx_allowed("MIT AND MPL-2.0"), Ok(false));
    assert_eq!(spdx_allowed("GPL-3.0-only"), Ok(false));
    assert_eq!(spdx_allowed("OFL-1.1"), Ok(false));
    assert!(spdx_allowed("(MIT").is_err());
}

#[test]
fn the_shipped_tree_is_permissive_with_no_git_source() {
    let mut all = BTreeSet::new();
    for target in TARGETS {
        let md = metadata(target);
        let packages: BTreeMap<String, &Value> =
            md["packages"].as_array().expect("packages").iter().map(|p| (p["id"].as_str().expect("id").to_string(), p)).collect();
        let nodes: BTreeMap<String, &Value> =
            md["resolve"]["nodes"].as_array().expect("resolve nodes").iter().map(|n| (n["id"].as_str().expect("id").to_string(), n)).collect();
        let root = packages.values().find(|p| p["name"] == "k2-deckrender").expect("k2-deckrender in the workspace");
        let root_id = root["id"].as_str().expect("id").to_string();

        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([root_id.clone()]);
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let node = nodes.get(&id).unwrap_or_else(|| panic!("no resolve node for {id}"));
            for dep in node["deps"].as_array().expect("deps") {
                let kinds = dep["dep_kinds"].as_array().expect("dep_kinds");
                if kinds.iter().any(|k| k["kind"].is_null() || k["kind"] == "build") {
                    queue.push_back(dep["pkg"].as_str().expect("pkg").to_string());
                }
            }
        }
        for id in &seen {
            let p = packages[id];
            let name = p["name"].as_str().expect("name");
            let version = p["version"].as_str().expect("version");
            let source = p["source"].as_str().unwrap_or("path");
            assert!(!source.starts_with("git+"), "{name} {version} comes from git ({source}) ({target})");
            let licence = p["license"].as_str().unwrap_or_else(|| panic!("{name} {version} has no SPDX licence expression ({target})"));
            match spdx_allowed(licence) {
                Ok(true) => {}
                Ok(false) => panic!("{name} {version}: {licence} is not on the allowlist ({target})"),
                Err(e) => panic!("{name} {version}: can't parse {licence:?}: {e}"),
            }
            let from = if source == "path" { "path" } else { "crates.io" };
            all.insert(format!("{name} {version}: {licence} ({from})"));
        }
    }
    for line in &all {
        eprintln!("[licence] {line}");
    }
    eprintln!("[licence] {} packages across {} targets", all.len(), TARGETS.len());
    for name in ["deckcraft-pptx", "deckcraft-render", "deckcraft-fonts", "zip"] {
        assert!(all.iter().any(|l| l.starts_with(&format!("{name} "))), "{name} is in the shipped tree");
    }
    for banned in ["epaint_default_fonts", "symphonia", "deckcraft-engine", "deckcraft-media", "craft-media", "craft-audio", "egui"] {
        assert!(!all.iter().any(|l| l.starts_with(&format!("{banned} "))), "{banned} must not be in the shipped tree");
    }
}
