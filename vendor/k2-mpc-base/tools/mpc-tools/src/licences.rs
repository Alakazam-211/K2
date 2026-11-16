//! `licences`: every package in every tracked Cargo.lock carries a licence the allow-list
//! satisfies.
//!
//! The allow-list: MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception,
//! BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0; NCSA too, only in a fuzz lock (a lock
//! under a `fuzz` folder: libFuzzer, test tooling). Anything else refuses; Unlicense passes only
//! as one side of an OR whose other side is allowed. The expression rule: `A OR B` passes when
//! either side does, `A AND B` when both do, parentheses group, the old `/` reads as OR, `X WITH
//! E` passes only as `Apache-2.0 WITH LLVM-exception`; an expression that doesn't parse, or no
//! licence at all, refuses.
//!
//! Every package is judged but our own path crates (`k2-mpc-*`, `mpce-*` with no registry
//! source), which are MIT OR Apache-2.0 by the repos' licence and listed apart; a vendored copy
//! of another project (a path crate by any other name) is judged by its declared licence.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use crate::{Ctx, Fail, Report, ERROR, OK};

const ALLOWED: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
];

#[derive(clap::Args)]
pub struct Args {
    /// The repo whose tracked Cargo.lock files are all checked (default: the current folder);
    /// each through `cargo metadata --locked --all-features`
    #[arg(long, value_name = "DIR", conflicts_with_all = ["metadata", "expr"])]
    root: Option<PathBuf>,
    /// Judge one `cargo metadata` output instead (a file, or `-` for stdin)
    #[arg(long, value_name = "FILE", conflicts_with = "expr")]
    metadata: Option<PathBuf>,
    /// Judge these licence expressions only (e.g. a crate's own manifest before it is vendored)
    #[arg(long, num_args = 1.., value_name = "NAME=EXPR")]
    expr: Vec<String>,
    /// With --metadata or --expr: judge as in a fuzz lock (NCSA allowed)
    #[arg(long)]
    fuzz: bool,
}

/// Whether `expr` passes the allow-list; `fuzz`: in a fuzz lock.
pub fn allowed(expr: &str, fuzz: bool) -> bool {
    let e = expr.replace('/', " OR ");
    let tokens = tokenize(&e);
    let mut p = Parser {
        tokens: &tokens,
        pos: 0,
        fuzz,
    };
    match p.or() {
        Some(v) => p.pos == tokens.len() && v,
        None => false,
    }
}

fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c == '(' || c == ')' || crate::py::is_space(c) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            if c == '(' || c == ')' {
                out.push(c.to_string());
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The expression parser: OR over AND over atoms; `None` when the expression ends early or a
/// parenthesis doesn't close.
struct Parser<'a> {
    tokens: &'a [String],
    pos: usize,
    fuzz: bool,
}

impl Parser<'_> {
    fn or(&mut self) -> Option<bool> {
        let mut v = self.and()?;
        while self.pos < self.tokens.len() && self.tokens[self.pos] == "OR" {
            self.pos += 1;
            let w = self.and()?;
            v = v || w;
        }
        Some(v)
    }

    fn and(&mut self) -> Option<bool> {
        let mut v = self.atom()?;
        while self.pos < self.tokens.len() && self.tokens[self.pos] == "AND" {
            self.pos += 1;
            let w = self.atom()?;
            v = v && w;
        }
        Some(v)
    }

    fn atom(&mut self) -> Option<bool> {
        let t = self.tokens.get(self.pos)?.clone();
        self.pos += 1;
        if t == "(" {
            let v = self.or()?;
            if self.tokens.get(self.pos).map(String::as_str) != Some(")") {
                return None;
            }
            self.pos += 1;
            return Some(v);
        }
        if self.tokens.get(self.pos).map(String::as_str) == Some("WITH") {
            let exception = self.tokens.get(self.pos + 1)?;
            self.pos += 2;
            return Some(t == "Apache-2.0" && exception == "LLVM-exception");
        }
        Some(ALLOWED.contains(&t.as_str()) || (self.fuzz && t == "NCSA"))
    }
}

/// One package outside the allow-list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub name: String,
    pub version: String,
    pub licence: String,
}

impl Refused {
    pub fn line(&self) -> String {
        format!("{} {}: {}", self.name, self.version, self.licence)
    }
}

/// Judges one `cargo metadata` document: the number of packages, and those refused.
pub fn judge(meta: &Value, fuzz: bool) -> Result<(usize, Vec<Refused>), Fail> {
    let packages = meta
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| Fail::error("not `cargo metadata` output: no `packages` list"))?;
    let mut refused = Vec::new();
    for p in packages {
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let ours = s("name").starts_with("k2-mpc-") || s("name").starts_with("mpce-");
        if ours && p.get("source").map_or(true, Value::is_null) {
            continue;
        }
        let licence = match p.get("license").and_then(Value::as_str) {
            Some(l) if !l.is_empty() => l.to_string(),
            _ => "NONE".to_string(),
        };
        if !allowed(&licence, fuzz) {
            refused.push(Refused {
                name: s("name"),
                version: s("version"),
                licence,
            });
        }
    }
    Ok((packages.len(), refused))
}

/// A lock is a fuzz lock when a folder on its path is named `fuzz`.
pub fn is_fuzz_lock(lock: &str) -> bool {
    lock.split('/').rev().skip(1).any(|c| c == "fuzz")
}

/// The tracked locks this check covers: every `Cargo.lock` but those of copied projects
/// (under a `vendor` folder) and of test inputs (under `tests/corpus`).
pub fn covered(locks: &[String]) -> Vec<String> {
    locks
        .iter()
        .filter(|l| l.as_str() == "Cargo.lock" || l.ends_with("/Cargo.lock"))
        .filter(|l| !l.split('/').any(|c| c == "vendor"))
        .filter(|l| !l.contains("tests/corpus/"))
        .cloned()
        .collect()
}

/// The tracked `Cargo.lock` paths under `root`, as git lists them.
fn tracked_locks(root: &Path) -> Result<Vec<String>, String> {
    // a job's checkout belongs to another user, and git refuses it unless told it's safe
    let listed = Command::new("git")
        .args(["-c", "safe.directory=*"])
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--", "*Cargo.lock"])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !listed.status.success() {
        return Err(format!(
            "git ls-files: {}",
            String::from_utf8_lossy(&listed.stderr).trim()
        ));
    }
    Ok(listed
        .stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect())
}

/// Every `Cargo.lock` on disk under `dir` (`rel` its path from the root), build output and
/// dependency folders aside.
fn locks_on_disk(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<(), Fail> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| Fail::io(dir, e))?
        .collect::<Result<_, _>>()
        .map_err(|e| Fail::io(dir, e))?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let ft = e.file_type().map_err(|err| Fail::io(&e.path(), err))?;
        if ft.is_dir() {
            if !matches!(name.as_str(), "target" | "node_modules" | ".git") {
                locks_on_disk(&e.path(), &path, out)?;
            }
        } else if name == "Cargo.lock" {
            out.push(path);
        }
    }
    Ok(())
}

pub fn run(a: &Args, ctx: &Ctx) -> Result<Report, Fail> {
    if !a.expr.is_empty() {
        return Ok(exprs(&a.expr, a.fuzz));
    }
    if let Some(m) = &a.metadata {
        let bytes = if m.as_os_str() == "-" {
            ctx.read_stdin()?
        } else {
            let p = ctx.path(m);
            std::fs::read(&p).map_err(|e| Fail::io(&p, e))?
        };
        let meta: Value = serde_json::from_slice(&bytes)
            .map_err(|e| Fail::error(format!("{}: not JSON: {e}", m.display())))?;
        let (n, refused) = judge(&meta, a.fuzz)?;
        let label = if m.as_os_str() == "-" {
            "stdin".to_string()
        } else {
            m.display().to_string()
        };
        return Ok(summary(vec![(label, a.fuzz, n, refused)]));
    }
    let root = ctx.path(a.root.as_deref().unwrap_or(Path::new(".")));
    if !root.is_dir() {
        return Err(Fail::not_found(format!(
            "licences: no folder {}",
            root.display()
        )));
    }
    let mut notices = Vec::new();
    let all = match tracked_locks(&root) {
        Ok(all) => all,
        Err(why) => {
            // a tree that isn't a git checkout (a bundle): every Cargo.lock on disk instead
            notices.push(format!(
                "licences: {why}; checking every Cargo.lock under {} instead",
                root.display()
            ));
            let mut found = Vec::new();
            locks_on_disk(&root, "", &mut found)?;
            found
        }
    };
    let locks = covered(&all);
    if locks.is_empty() {
        return Err(Fail::not_found(format!(
            "licences: no tracked Cargo.lock in {}",
            root.display()
        )));
    }
    let cargo = ctx.var("CARGO").unwrap_or("cargo").to_string();
    let mut results = Vec::new();
    for lock in &locks {
        let dir = Path::new(lock).parent().unwrap_or(Path::new(""));
        let manifest = root.join(dir).join("Cargo.toml");
        let out = Command::new(&cargo)
            .current_dir(&root)
            .args([
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--all-features",
                "--manifest-path",
            ])
            .arg(&manifest)
            .output()
            .map_err(|e| Fail::error(format!("licences: {cargo}: {e}")))?;
        if !out.status.success() {
            return Err(Fail::error(format!(
                "licences: cargo metadata for {lock} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let meta: Value = serde_json::from_slice(&out.stdout)
            .map_err(|e| Fail::error(format!("licences: cargo metadata for {lock}: {e}")))?;
        let fuzz = is_fuzz_lock(lock);
        let (n, refused) = judge(&meta, fuzz)?;
        results.push((lock.clone(), fuzz, n, refused));
    }
    let mut report = summary(results);
    report.notices = notices;
    Ok(report)
}

fn summary(results: Vec<(String, bool, usize, Vec<Refused>)>) -> Report {
    let mut text = String::new();
    let mut locks = Vec::new();
    let (mut total, mut bad) = (0, 0);
    for (lock, fuzz, n, refused) in &results {
        total += n;
        bad += refused.len();
        let kind = if *fuzz { " (fuzz lock)" } else { "" };
        if refused.is_empty() {
            text += &format!("{lock}{kind}: {n} packages, ok\n");
        } else {
            text += &format!(
                "{lock}{kind}: {n} packages, {} outside the allow-list:\n",
                refused.len()
            );
            for r in refused {
                text += &format!("  {}\n", r.line());
            }
        }
        locks.push(json!({
            "lock": lock,
            "fuzz_lock": fuzz,
            "packages": n,
            "refused": refused.iter().map(|r| json!({
                "name": r.name, "version": r.version, "licence": r.licence
            })).collect::<Vec<_>>(),
        }));
    }
    let code = if bad == 0 { OK } else { ERROR };
    if bad == 0 {
        text += &format!("licences ok ({} locks, {total} packages)\n", results.len());
    } else {
        text += &format!(
            "licences: {bad} packages outside the allow-list ({} locks, {total} packages)\n",
            results.len()
        );
    }
    Report::new(code, text, json!({ "ok": bad == 0, "locks": locks }))
}

fn exprs(args: &[String], fuzz: bool) -> Report {
    let pairs: Vec<(String, String)> = args
        .iter()
        .map(|a| match a.split_once('=') {
            Some((n, e)) if !e.is_empty() => (n.to_string(), e.to_string()),
            Some((n, _)) => (n.to_string(), "NONE".to_string()),
            None => (a.clone(), "NONE".to_string()),
        })
        .collect();
    let bad: Vec<&(String, String)> = pairs.iter().filter(|(_, e)| !allowed(e, fuzz)).collect();
    let doc = json!({
        "ok": bad.is_empty(),
        "exprs": pairs.iter().map(|(n, e)| json!({"name": n, "expr": e, "ok": allowed(e, fuzz)})).collect::<Vec<_>>(),
    });
    if bad.is_empty() {
        let list: Vec<String> = pairs.iter().map(|(n, e)| format!("{n} ({e})")).collect();
        let text = format!("licences ok ({}: {})\n", pairs.len(), list.join(", "));
        Report::new(OK, text, doc)
    } else {
        let mut text = "Licences outside the allow-list:\n".to_string();
        for (n, e) in bad {
            text += &format!("{n}: {e}\n");
        }
        Report::new(ERROR, text, doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fuzz crate's folder, by name only: a public cut drops that folder and any path into it.
    const F: &str = "fuzz";

    #[test]
    fn locks_covered_and_fuzz() {
        let all: Vec<String> = vec![
            "Cargo.lock".to_string(),
            format!("{F}/Cargo.lock"),
            "tools/mpc-tools/Cargo.lock".to_string(),
            "vendor/yrs/Cargo.lock".to_string(),
            "crates/x/tests/corpus/a/Cargo.lock".to_string(),
            "NotCargo.lock".to_string(),
        ];
        let want = vec![
            "Cargo.lock".to_string(),
            format!("{F}/Cargo.lock"),
            "tools/mpc-tools/Cargo.lock".to_string(),
        ];
        assert_eq!(covered(&all), want);
        assert!(is_fuzz_lock(&format!("{F}/Cargo.lock")));
        assert!(is_fuzz_lock(&format!("bridge/{F}/Cargo.lock")));
        assert!(!is_fuzz_lock("Cargo.lock"));
        assert!(!is_fuzz_lock(&format!("{F}y/Cargo.lock")));
    }

    #[test]
    fn the_ruled_list() {
        for e in [
            "MIT",
            "Apache-2.0 WITH LLVM-exception",
            "Unlicense OR MIT",
            "(MIT OR Apache-2.0) AND Unicode-3.0",
        ] {
            assert!(allowed(e, false), "{e} passes");
        }
        for e in [
            "Unlicense",
            "NCSA",
            "MPL-2.0",
            "",
            "NONE",
            "Apache-2.0 WITH",
            "MIT AND GPL-3.0",
            "(MIT",
        ] {
            assert!(!allowed(e, false), "{e} refuses");
        }
        assert!(
            allowed("(MIT OR Apache-2.0) AND NCSA", true),
            "NCSA in a fuzz lock"
        );
        assert!(
            !allowed("(MIT OR Apache-2.0) AND NCSA", false),
            "NCSA outside a fuzz lock"
        );
    }
}
