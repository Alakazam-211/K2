//! The App refusal registry is complete and its exits are the CLI's
//! (prd-tauri-core-0467-v1 TC-18, vs-live TA-19).
//!
//! 1. Every refusal code an App route module can send has a row in
//!    `k2_core::refusals`: the scan reads `apps/`, `app_pairing/` and
//!    `app_push/` (production code only: test files and everything after
//!    `#[cfg(test)]` are skipped) for `no("code", …)`, `no(CONST, …)`,
//!    `refusal("4xx …", "code", …)`, `refusal_with(…)`, `"code": "…"` and
//!    `codes::CONST`, with each `CONST` resolved in its own file first,
//!    then in `deploy::codes`, then anywhere in those modules.
//! 2. A row's exit is what `cli/k2` prints: `_compute_fail` maps 400 and
//!    413 to 2 and the rest to 3; the listed overrides are pinned to the
//!    CLI lines that print them.
//! 3. The generated `cli/k2` block holds every row (contract-gen fresh).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use k2_core::refusals;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Production code only.
fn prod(src: &str) -> &str {
    src.split("\n#[cfg(test)]").next().expect("split")
}

fn is_code(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') && s.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
}

fn is_const(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') && s.bytes().next().is_some_and(|b| b.is_ascii_uppercase())
}

/// The string literal starting at `s` (after optional whitespace), if any.
fn literal(s: &str) -> Option<&str> {
    let s = s.trim_start();
    let rest = s.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// `NAME = "value"` for every `const NAME: &str = "value";`.
fn consts(src: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (i, _) in src.match_indices("const ") {
        let rest = &src[i + 6..];
        let Some((name, after)) = rest.split_once(": &str = ") else { continue };
        if !is_const(name) || name.contains(char::is_whitespace) {
            continue;
        }
        if let Some(v) = literal(after).filter(|v| is_code(v)) {
            if after.trim_start()[v.len() + 2..].starts_with(';') {
                out.insert(name.to_string(), v.to_string());
            }
        }
    }
    out
}

/// The identifier (with `::` paths) at the start of `s`; its last segment.
fn ident_tail(s: &str) -> &str {
    let s = s.trim_start();
    let end = s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':')).unwrap_or(s.len());
    s[..end].rsplit("::").next().unwrap_or("")
}

#[derive(Default)]
struct Found {
    literals: BTreeSet<String>,
    consts: BTreeSet<String>,
}

fn scan(src: &str) -> Found {
    let mut f = Found::default();
    for (i, _) in src.match_indices("no(") {
        // `no(` as a call, not `info(` or `do_no(`.
        if src[..i].chars().last().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let rest = &src[i + 3..];
        if let Some(code) = literal(rest) {
            if is_code(code) {
                f.literals.insert(code.to_string());
            }
        } else {
            let id = ident_tail(rest);
            if is_const(id) {
                f.consts.insert(id.to_string());
            }
        }
    }
    for call in ["refusal(", "refusal_with("] {
        for (i, _) in src.match_indices(call) {
            if src[..i].chars().last().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            let rest = &src[i + call.len()..];
            let Some(status) = literal(rest) else { continue };
            if !status.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let after = &rest.trim_start()[status.len() + 2..];
            let Some(after) = after.trim_start().strip_prefix(',') else { continue };
            if let Some(code) = literal(after).filter(|c| is_code(c)) {
                f.literals.insert(code.to_string());
            }
        }
    }
    for (i, _) in src.match_indices("\"code\"") {
        let rest = src[i + 6..].trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            if let Some(code) = literal(rest).filter(|c| is_code(c)) {
                f.literals.insert(code.to_string());
            }
        }
    }
    for (i, _) in src.match_indices("codes::") {
        let id = ident_tail(&src[i..]);
        if is_const(id) {
            f.consts.insert(id.to_string());
        }
    }
    f
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = e.expect("entry").path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with("_tests.rs") && name != "tests.rs" {
                out.push(p);
            }
        }
    }
}

/// Every code the App route modules can send, with the files sending it.
fn route_codes() -> BTreeMap<String, BTreeSet<String>> {
    let src = root().join("crates/k2-daemon/src");
    let mut files = Vec::new();
    for d in ["apps", "app_pairing", "app_push"] {
        rs_files(&src.join(d), &mut files);
    }
    assert!(files.len() > 40, "the scan found the route modules: {}", files.len());
    let deploy_src = read(&root().join("crates/k2-core/src/deploy/mod.rs"));
    let codes_mod = deploy_src.split("pub mod codes {").nth(1).expect("deploy::codes").split("\n}").next().expect("its end");
    let deploy_codes = consts(codes_mod);
    assert!(deploy_codes.len() >= 30, "deploy::codes: {deploy_codes:?}");
    let per_file: Vec<(PathBuf, String)> = files.iter().map(|p| (p.clone(), prod(&read(p)).to_string())).collect();
    let mut global: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut local: BTreeMap<PathBuf, BTreeMap<String, String>> = BTreeMap::new();
    for (p, s) in &per_file {
        let c = consts(s);
        for (k, v) in &c {
            global.entry(k.clone()).or_default().insert(v.clone());
        }
        local.insert(p.clone(), c);
    }
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut add = |code: String, file: &Path| {
        let rel = file.strip_prefix(&src).unwrap_or(file).display().to_string();
        out.entry(code).or_default().insert(rel);
    };
    for (p, s) in &per_file {
        let f = scan(s);
        for c in f.literals {
            add(c, p);
        }
        for name in f.consts {
            let vals: BTreeSet<String> = if let Some(v) = local[p].get(&name) {
                BTreeSet::from([v.clone()])
            } else if let Some(v) = deploy_codes.get(&name) {
                BTreeSet::from([v.clone()])
            } else {
                global.get(&name).cloned().unwrap_or_default()
            };
            for v in vals {
                add(v, p);
            }
        }
    }
    for v in deploy_codes.values() {
        out.entry(v.clone()).or_default().insert("k2-core deploy::codes".into());
    }
    out
}

#[test]
fn every_app_route_refusal_code_has_a_registry_row() {
    let codes = route_codes();
    assert!(codes.len() >= 120, "the scan finds the App codes ({})", codes.len());
    for known in ["apps_manage_off", "app_not_found", "token_in_use", "pairing_off", "pass_invalid", "egress_self", "box_service_invalid", "handover_vault_closed"] {
        assert!(codes.contains_key(known), "the scan sees {known}: {:?}", codes.keys().collect::<Vec<_>>());
    }
    let missing: Vec<String> = codes
        .iter()
        .filter(|(c, _)| refusals::lookup(c).is_none())
        .map(|(c, files)| format!("{c} (sent by {})", files.iter().cloned().collect::<Vec<_>>().join(", ")))
        .collect();
    assert!(
        missing.is_empty(),
        "App refusal codes with no row in crates/k2-core/src/refusals/rows.rs (add one: what happened, the fix, who fixes it):\n  {}",
        missing.join("\n  ")
    );
}

/// The scanner itself: each call shape it must see, and what it must not.
#[test]
fn the_scan_reads_every_call_shape() {
    let src = r#"
        const MINE: &str = "my_code";
        fn a() { no("lit_code", 409, "x"); no(codes::APP_EXISTS, 409, "y"); no(MINE, 400, "z"); }
        fn b() { refusal("403 Forbidden", "ref_code", "h"); refusal_with("429 Too Many Requests", "rate_x", "h", m); }
        fn c() { json!({"code": "json_code"}); info("not_a_code", 1); do_no("nope", 1); }
    "#;
    let f = scan(src);
    for c in ["lit_code", "ref_code", "rate_x", "json_code"] {
        assert!(f.literals.contains(c), "{c}: {:?}", f.literals);
    }
    assert!(!f.literals.contains("not_a_code") && !f.literals.contains("nope"), "{:?}", f.literals);
    assert!(f.consts.contains("APP_EXISTS") && f.consts.contains("MINE"), "{:?}", f.consts);
    assert_eq!(consts(src).get("MINE").map(String::as_str), Some("my_code"));
}

/// `_compute_fail`'s case block in `cli/k2`.
fn compute_fail_block(cli: &str) -> &str {
    let start = cli.find("\n_compute_fail() {").expect("_compute_fail in cli/k2");
    let rest = &cli[start..];
    &rest[..rest.find("\n}\n").expect("its end")]
}

#[test]
fn every_rows_exit_is_what_the_cli_prints() {
    let cli = read(&root().join("cli/k2"));
    let fail = compute_fail_block(&cli);
    assert!(fail.contains("400|413) exit 2 ;;"), "_compute_fail maps 400 and 413 to 2:\n{fail}");
    assert!(fail.contains("*) exit 3 ;;"), "_compute_fail maps the rest to 3:\n{fail}");
    for s in [400u16, 413] {
        assert_eq!(refusals::cli_exit(s), 2);
    }
    // The overrides, each against the CLI line that prints it.
    let dep_exit = cli.split("\n_app_rt_deployment_exit() {").nth(1).expect("_app_rt_deployment_exit");
    let dep_exit = &dep_exit[..dep_exit.find("\n}\n").expect("end")];
    assert!(dep_exit.contains("*) exit 1 ;;"), "a failed deployment exits 1:\n{dep_exit}");
    let build = cli.split("\ncmd_app_build() {").nth(1).expect("cmd_app_build");
    let build = &build[..build.find("\n}\n").expect("end")];
    assert!(build.contains("if [ \"$_CP_STATUS\" = \"422\" ]") && build.contains("exit 1"), "a 422 build exits 1");
    for (id, exit) in refusals::EXIT_OVERRIDES {
        let row = refusals::lookup(id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(row.exit, *exit, "{id}");
        match row.http {
            422 => assert_eq!(*exit, 1, "{id}: a 422 from build/deploy"),
            _ => assert!(["health_failed", "crashlooping"].contains(id), "{id}: a deployment result exits 1"),
        }
    }
    for row in refusals::all() {
        if !refusals::EXIT_OVERRIDES.iter().any(|(id, _)| *id == row.id) {
            assert_eq!(row.exit, refusals::cli_exit(row.http), "{}: HTTP {} exits {} in the CLI", row.id, row.http, refusals::cli_exit(row.http));
        }
    }
}

#[test]
fn the_cli_block_holds_every_row() {
    let cli = read(&root().join("cli/k2"));
    let start = cli.find(refusals::CLI_BEGIN).unwrap_or_else(|| panic!("cli/k2 has no '{}' block; run contract-gen", refusals::CLI_BEGIN));
    let block = &cli[start..];
    let json_line = block.lines().nth(3).expect("the rows line");
    let v: serde_json::Value = serde_json::from_str(json_line).unwrap_or_else(|e| panic!("the rows line is JSON ({e}); run contract-gen"));
    let ids: BTreeSet<&str> = v["rows"].as_array().expect("rows").iter().filter_map(|r| r["id"].as_str()).collect();
    for row in refusals::all() {
        assert!(ids.contains(row.id), "{} is missing from cli/k2's block: run `cargo run -p k2-core --bin contract-gen`", row.id);
    }
}
