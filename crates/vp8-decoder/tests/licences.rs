//! A cargo-deny-style licence check of this crate's dependency tree
//! (normal and build dependencies, default features), for each desktop
//! target. The crate's own licence must be exactly what PROVENANCE.md and
//! NOTICE describe, and every dependency must carry an SPDX expression
//! satisfiable with the allowlist below.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::process::Command;

const ALLOWED: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
];

const TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
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
        .args([
            "metadata",
            "--format-version",
            "1",
            "--filter-platform",
            target,
            "--manifest-path",
            manifest,
        ])
        .output()
        .expect("run cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("metadata json")
}

#[test]
fn spdx_parser() {
    assert_eq!(spdx_allowed("MIT OR Apache-2.0"), Ok(true));
    assert_eq!(spdx_allowed("(MIT OR Apache-2.0) AND Unicode-3.0"), Ok(true));
    assert_eq!(spdx_allowed("MIT/Apache-2.0"), Ok(true));
    assert_eq!(spdx_allowed("MPL-2.0"), Ok(false));
    assert_eq!(spdx_allowed("MIT AND MPL-2.0"), Ok(false));
    assert_eq!(spdx_allowed("MPL-2.0 OR MIT"), Ok(true));
    assert_eq!(spdx_allowed("Apache-2.0 WITH LLVM-exception"), Ok(true));
    assert_eq!(spdx_allowed("GPL-3.0-only"), Ok(false));
    assert!(spdx_allowed("(MIT").is_err());
}

#[test]
fn dependency_licences_are_on_the_allowlist() {
    let mut report: BTreeMap<String, String> = BTreeMap::new();
    for target in TARGETS {
        let md = metadata(target);
        let packages: BTreeMap<String, &Value> = md["packages"]
            .as_array()
            .expect("packages")
            .iter()
            .map(|p| (p["id"].as_str().expect("id").to_string(), p))
            .collect();
        let nodes: BTreeMap<String, &Value> = md["resolve"]["nodes"]
            .as_array()
            .expect("resolve nodes")
            .iter()
            .map(|n| (n["id"].as_str().expect("id").to_string(), n))
            .collect();
        let root = packages
            .values()
            .find(|p| p["name"] == "vp8-decoder")
            .expect("vp8-decoder in the workspace");
        let root_id = root["id"].as_str().expect("id").to_string();
        assert_eq!(root["license"], "(MIT OR Apache-2.0) AND BSD-3-Clause");
        assert_eq!(spdx_allowed(root["license"].as_str().expect("licence")), Ok(true));

        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([root_id.clone()]);
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let node = nodes.get(&id).unwrap_or_else(|| panic!("no resolve node for {id}"));
            for dep in node["deps"].as_array().expect("deps") {
                let kinds = dep["dep_kinds"].as_array().expect("dep_kinds");
                let shipped = kinds.iter().any(|k| k["kind"].is_null() || k["kind"] == "build");
                if shipped {
                    queue.push_back(dep["pkg"].as_str().expect("pkg").to_string());
                }
            }
        }
        seen.remove(&root_id);

        for id in &seen {
            let p = packages[id];
            let name = p["name"].as_str().expect("name");
            let version = p["version"].as_str().expect("version");
            let licence = p["license"].as_str().unwrap_or_else(|| {
                panic!("{name} {version} has no SPDX licence expression ({target})")
            });
            match spdx_allowed(licence) {
                Ok(true) => {}
                Ok(false) => panic!("{name} {version}: {licence} is not on the allowlist ({target})"),
                Err(e) => panic!("{name} {version}: can't parse {licence:?}: {e}"),
            }
            report.insert(format!("{name} {version}"), licence.to_string());
        }
    }
    for (pkg, lic) in &report {
        eprintln!("[licence] {pkg}: {lic}");
    }
    // Pure Rust with no dependencies: anything new here needs a licence review.
    assert!(report.is_empty(), "unexpected dependencies: {report:?}");
}
