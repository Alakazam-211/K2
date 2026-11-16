//! `pack-check`: the pack template's checks (docs/PACK_TEMPLATE.md), run from a pack's root.
//!
//! - `engine-free [ROOT]`: a type pack never depends on or imports a CRDT engine. Fails on a
//!   Cargo dependency (direct, any target) on an engine crate in any of the pack's crates; a JS
//!   dependency on an engine package in any package.json (outside node_modules); a source
//!   import or use of an engine in .rs/.ts/.tsx/.js/.mjs/.cjs files.
//! - `carrier-row [ROOT]`: a pack whose source (outside tests/, target/ and the fuzz crate)
//!   implements `Pack::wants_carrier` with a body other than a plain `false` keeps a carrier,
//!   and must call `k2_mpc_base::session::test_support::carrier_judge` over its corpus in a test
//!   under a `tests/` folder.
//!
//! Exit 0 clean, 1 findings. Paths print as `./<path>` from ROOT; folders are walked in name
//! order. Files are read as Python's text mode read them (bytes that aren't UTF-8 replaced,
//! `\r\n` and a lone `\r` as `\n`), as the scripts this replaces did.

use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;
use serde_json::{json, Value};

use crate::{Ctx, Fail, Report, ERROR, OK};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// A type pack never depends on or imports a CRDT engine
    EngineFree {
        /// The pack's root (default: the current folder)
        root: Option<PathBuf>,
        /// Read this `cargo metadata --no-deps` output instead of running cargo
        #[arg(long, value_name = "FILE")]
        metadata: Option<PathBuf>,
    },
    /// A pack that keeps a carrier runs the kit's byte-exact row
    CarrierRow {
        /// The pack's root (default: the current folder)
        root: Option<PathBuf>,
    },
}

const RUST: &[&str] = &[
    "yrs",
    "y-sync",
    "yrs-warp",
    "loro",
    "loro-internal",
    "automerge",
    "autosurgeon",
    "diamond-types",
];
const JS: &[&str] = &[
    "yjs",
    "y-protocols",
    "y-codemirror.next",
    "y-prosemirror",
    "y-monaco",
    "y-websocket",
    "lib0",
    "loro-crdt",
    "loro-codemirror",
    "@automerge/automerge",
    "@automerge/automerge-repo",
];
const SECTIONS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

pub fn run(cmd: &Cmd, ctx: &Ctx) -> Result<Report, Fail> {
    match cmd {
        Cmd::EngineFree { root, metadata } => {
            let root = folder(ctx, root.as_deref())?;
            let meta = match metadata {
                Some(m) => {
                    let p = ctx.path(m);
                    std::fs::read(&p).map_err(|e| Fail::io(&p, e))?
                }
                None => cargo_metadata(&root, ctx)?,
            };
            let meta: Value = serde_json::from_slice(&meta)
                .map_err(|e| Fail::error(format!("engine-free: cargo metadata: {e}")))?;
            engine_free(&root, &meta)
        }
        Cmd::CarrierRow { root } => carrier_row(&folder(ctx, root.as_deref())?),
    }
}

fn folder(ctx: &Ctx, root: Option<&Path>) -> Result<PathBuf, Fail> {
    let p = ctx.path(root.unwrap_or(Path::new(".")));
    if p.is_dir() {
        Ok(p)
    } else {
        Err(Fail::not_found(format!(
            "pack-check: no folder {}",
            p.display()
        )))
    }
}

fn cargo_metadata(root: &Path, ctx: &Ctx) -> Result<Vec<u8>, Fail> {
    let cargo = ctx.var("CARGO").unwrap_or("cargo");
    let out = Command::new(cargo)
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| Fail::error(format!("engine-free: {cargo}: {e}")))?;
    if !out.status.success() {
        return Err(Fail::error(format!(
            "engine-free: cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// Every file under `root` (as `./a/b`), in name order, skipping folders named in `skip`; with
/// each file, whether a folder on its way is named `tests`.
fn walk(root: &Path, skip: &[&str]) -> Result<Vec<(String, PathBuf, bool)>, Fail> {
    fn go(
        dir: &Path,
        shown: &str,
        in_tests: bool,
        skip: &[&str],
        out: &mut Vec<(String, PathBuf, bool)>,
    ) -> Result<(), Fail> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| Fail::io(dir, e))?
            .collect::<Result<_, _>>()
            .map_err(|e| Fail::io(dir, e))?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            let shown_p = format!("{shown}/{name}");
            if e.file_type().map_err(|err| Fail::io(&p, err))?.is_dir() {
                if !skip.contains(&name.as_str()) {
                    go(&p, &shown_p, in_tests || name == "tests", skip, out)?;
                }
            } else {
                out.push((shown_p, p, in_tests));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    go(root, ".", false, skip, &mut out)?;
    Ok(out)
}

fn read_text(p: &Path) -> Result<String, Fail> {
    let bytes = std::fs::read(p).map_err(|e| Fail::io(p, e))?;
    Ok(crate::py::read_text_lossy(&bytes))
}

/// The JS names a package.json section lists, as Python iterates the value; `Err` where Python
/// would stop with an error.
fn section_names(v: &Value) -> Result<Vec<String>, ()> {
    match v {
        Value::Object(m) => Ok(m.keys().cloned().collect()),
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => Ok(s.clone()),
                Value::Number(_) | Value::Bool(_) | Value::Null => Ok(String::new()),
                _ => Err(()),
            })
            .collect(),
        Value::String(s) => Ok(s.chars().map(|c| c.to_string()).collect()),
        _ => Err(()),
    }
}

pub fn engine_free(root: &Path, meta: &Value) -> Result<Report, Fail> {
    let mut bad = Vec::new();
    let packages = meta
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| Fail::error("engine-free: not `cargo metadata` output"))?;
    for p in packages {
        let pname = p.get("name").and_then(Value::as_str).unwrap_or("");
        for d in p
            .get("dependencies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let dname = d.get("name").and_then(Value::as_str).unwrap_or("");
            if RUST.contains(&dname) {
                bad.push(format!(
                    "{pname}: Cargo dependency on engine crate `{dname}`"
                ));
            }
        }
    }
    let src = Regex::new(
        r#"(?m)(?:^\s*(?:pub\s+)?use\s+|extern\s+crate\s+|\byrs::|\bloro::|\bautomerge::)(yrs|loro|automerge)\b|(?:from\s+|require\(\s*|import\s*\(\s*|import\s+)['"]((?:@automerge/)?[\w.\-]+)"#,
    )
    .expect("a fixed pattern");
    for (shown, path, _) in walk(root, &["node_modules", "target", ".git"])? {
        let file = shown.rsplit('/').next().unwrap_or("");
        if file == "package.json" {
            let text = read_text(&path)?;
            let pkg: Value = serde_json::from_str(&text)
                .map_err(|e| Fail::error(format!("engine-free: {shown}: {e}")))?;
            let obj = pkg
                .as_object()
                .ok_or_else(|| Fail::error(format!("engine-free: {shown}: not a JSON object")))?;
            for section in SECTIONS {
                let Some(v) = obj.get(*section) else { continue };
                let names = section_names(v).map_err(|_| {
                    Fail::error(format!(
                        "engine-free: {shown}: `{section}` is not a list of names"
                    ))
                })?;
                for name in names {
                    if JS.contains(&name.as_str()) {
                        bad.push(format!("{shown}: {section} on engine package `{name}`"));
                    }
                }
            }
        } else if [".rs", ".ts", ".tsx", ".js", ".mjs", ".cjs"]
            .iter()
            .any(|x| file.ends_with(x))
        {
            let text = read_text(&path)?;
            for c in src.captures_iter(&text) {
                let m = c.get(0).expect("the whole match");
                let name = c.get(1).or_else(|| c.get(2)).map_or("", |g| g.as_str());
                if RUST.contains(&name) || JS.contains(&name) {
                    let line = text[..m.start()].matches('\n').count() + 1;
                    bad.push(format!("{shown}:{line}: imports engine `{name}`"));
                }
            }
        }
    }
    if bad.is_empty() {
        return Ok(Report::new(
            OK,
            "engine-free: ok\n".into(),
            json!({ "ok": true, "findings": [] }),
        ));
    }
    let mut text =
        "A type pack must not touch the CRDT engine (use the kit's primitives and JS client):\n"
            .to_string();
    for b in &bad {
        text += &format!("  {b}\n");
    }
    Ok(Report::new(
        ERROR,
        text,
        json!({ "ok": false, "findings": bad }),
    ))
}

pub fn carrier_row(root: &Path) -> Result<Report, Fail> {
    let imp =
        Regex::new(r"fn\s+wants_carrier\s*\(\s*&\s*self\s*\)\s*->\s*bool\s*\{(?P<body>[^}]*)\}")
            .expect("a fixed pattern");
    let (mut keeps, mut judged) = (Vec::new(), Vec::new());
    for (shown, path, in_tests) in walk(root, &["node_modules", "target", ".git", "fuzz"])? {
        if !shown.ends_with(".rs") {
            continue;
        }
        let text = read_text(&path)?;
        if in_tests {
            if text.contains("carrier_judge(") {
                judged.push(shown);
            }
            continue;
        }
        for c in imp.captures_iter(&text) {
            if crate::py::strip(&c["body"]) != "false" {
                keeps.push(shown.clone());
            }
        }
    }
    keeps.sort();
    keeps.dedup();
    judged.sort();
    let doc = json!({ "carrier": !keeps.is_empty(), "keeps": keeps, "rows": judged });
    if !keeps.is_empty() && judged.is_empty() {
        let text = format!(
            "carrier-row: the pack keeps a carrier (wants_carrier in {}) but no test under tests/ calls test_support::carrier_judge over its corpus\n",
            keeps.join(", ")
        );
        return Ok(Report::new(ERROR, text, doc));
    }
    let text = if keeps.is_empty() {
        "carrier-row: ok (no carrier)\n".to_string()
    } else {
        format!(
            "carrier-row: ok (carrier pack; the row runs in {})\n",
            judged.join(", ")
        )
    };
    Ok(Report::new(OK, text, doc))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// carrier-row skips the fuzz crate's folder (its pin table's rows use neutral folder
    /// names, as a public cut drops that folder and paths into it): a carrier there is no carrier
    #[test]
    fn carrier_row_skips_the_fuzz_folder() {
        let root = std::env::temp_dir().join(format!("mpc-tools-carrier-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("fuzz").join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "fn wants_carrier(&self) -> bool { true }\n",
        )
        .unwrap();
        let r = carrier_row(&root).unwrap();
        assert_eq!(
            (r.code, r.text.as_str()),
            (OK, "carrier-row: ok (no carrier)\n")
        );
        let other = root.join("fz").join("src");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("lib.rs"),
            "fn wants_carrier(&self) -> bool { true }\n",
        )
        .unwrap();
        let r = carrier_row(&root).unwrap();
        assert_eq!(r.code, ERROR, "{}", r.text);
        let _ = std::fs::remove_dir_all(&root);
    }
}
