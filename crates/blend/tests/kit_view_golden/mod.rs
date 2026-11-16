//! The golden snapshots of the kit's view (`tests/kit_view.rs`), one file per sample beside this
//! module, kept exactly as the kit prints them (`Value::to_json_pretty`, then a newline), and
//! the check that holds a value to its file ([`check`]). The files are plain JSON, so an editor's
//! mocks can read them as they are.
//!
//! **Re-bless** (after a change to the view made on purpose): run the test with
//! `MPC_BLESS_GOLDEN=1`. Each sample's file is written from what the kit gives now, and the test
//! passes saying `blessed <file>`. On a build node, keep `crates/blend/tests/kit_view_golden/*.json`
//! as the job's artifacts and fetch them; read the diff, and commit it with the change that made
//! it. Without the variable the test never writes, and any difference fails it, by path.

use std::fs;
use std::path::PathBuf;

use k2_mpc_base::errors::Value as Json;

/// The variable that re-blesses the goldens.
pub const BLESS: &str = "MPC_BLESS_GOLDEN";

/// The most differences one failure lists.
const SHOWN: usize = 40;

fn path_of(dir: &str, name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(dir)
        .join(format!("{name}.json"))
}

/// `got` against the golden `name`: `Ok` when its pretty JSON is the file's, byte for byte (or
/// when re-blessing); otherwise each path that differs (at most [`SHOWN`]) and how to re-bless.
pub fn check(name: &str, got: &Json) -> Result<(), String> {
    check_in("kit_view_golden", name, got)
}

/// [`check`] for a golden in another folder under `tests/` (`watch_turn_golden`), the same way.
pub fn check_in(dir: &str, name: &str, got: &Json) -> Result<(), String> {
    let path = path_of(dir, name);
    let text = format!("{}\n", got.to_json_pretty());
    if std::env::var_os(BLESS).is_some_and(|v| !v.is_empty()) {
        if let Some(d) = path.parent() {
            fs::create_dir_all(d).map_err(|e| format!("{name}: {}: {e}", d.display()))?;
        }
        fs::write(&path, &text).map_err(|e| format!("{name}: writing {}: {e}", path.display()))?;
        println!("blessed {}", path.display());
        return Ok(());
    }
    let how =
        format!("if the change is meant, re-bless with {BLESS}=1 (tests/kit_view_golden/mod.rs)");
    let want = fs::read_to_string(&path)
        .map_err(|e| format!("{name}: no golden at {} ({e}); {how}", path.display()))?;
    if want == text {
        return Ok(());
    }
    let tree = parse(&want).map_err(|e| format!("{name}: the golden doesn't parse: {e}"))?;
    let mut out = Vec::new();
    diff("$", got, &tree, &mut out);
    if out.is_empty() {
        out.push("$: the same values, in another layout or key order".to_owned());
    }
    let n = out.len();
    out.truncate(SHOWN);
    Err(format!(
        "{name}: {n} difference(s) from the golden {}:\n{}\n{how}",
        path.display(),
        out.join("\n")
    ))
}

/// A golden's JSON, its scalars and keys kept as written (quotes and escapes too), so they
/// compare with what the kit prints.
enum Raw {
    Scalar(String),
    List(Vec<Raw>),
    Obj(Vec<(String, Raw)>),
}

fn parse(s: &str) -> Result<Raw, String> {
    let b = s.as_bytes();
    let mut i = 0;
    let v = value(b, &mut i)?;
    ws(b, &mut i);
    if i != b.len() {
        return Err(format!("text after the value, at byte {i}"));
    }
    Ok(v)
}

fn ws(b: &[u8], i: &mut usize) {
    while b.get(*i).is_some_and(u8::is_ascii_whitespace) {
        *i += 1;
    }
}

fn value(b: &[u8], i: &mut usize) -> Result<Raw, String> {
    ws(b, i);
    match b.get(*i) {
        Some(b'{') => {
            *i += 1;
            let mut fields = Vec::new();
            ws(b, i);
            if b.get(*i) == Some(&b'}') {
                *i += 1;
                return Ok(Raw::Obj(fields));
            }
            loop {
                ws(b, i);
                let k = string(b, i)?;
                ws(b, i);
                if b.get(*i) != Some(&b':') {
                    return Err(format!("no ':' at byte {}", *i));
                }
                *i += 1;
                fields.push((k, value(b, i)?));
                ws(b, i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b'}') => {
                        *i += 1;
                        return Ok(Raw::Obj(fields));
                    }
                    _ => return Err(format!("no ',' or '}}' at byte {}", *i)),
                }
            }
        }
        Some(b'[') => {
            *i += 1;
            let mut items = Vec::new();
            ws(b, i);
            if b.get(*i) == Some(&b']') {
                *i += 1;
                return Ok(Raw::List(items));
            }
            loop {
                items.push(value(b, i)?);
                ws(b, i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b']') => {
                        *i += 1;
                        return Ok(Raw::List(items));
                    }
                    _ => return Err(format!("no ',' or ']' at byte {}", *i)),
                }
            }
        }
        Some(b'"') => string(b, i).map(Raw::Scalar),
        Some(_) => {
            let start = *i;
            while b
                .get(*i)
                .is_some_and(|c| !matches!(c, b',' | b']' | b'}') && !c.is_ascii_whitespace())
            {
                *i += 1;
            }
            Ok(Raw::Scalar(
                String::from_utf8_lossy(&b[start..*i]).into_owned(),
            ))
        }
        None => Err("the text ends where a value should be".to_owned()),
    }
}

/// A string as written, quotes and escapes kept.
fn string(b: &[u8], i: &mut usize) -> Result<String, String> {
    if b.get(*i) != Some(&b'"') {
        return Err(format!("no string at byte {}", *i));
    }
    let start = *i;
    *i += 1;
    while let Some(c) = b.get(*i) {
        match c {
            b'\\' => *i += 2,
            b'"' => {
                *i += 1;
                return Ok(String::from_utf8_lossy(&b[start..*i]).into_owned());
            }
            _ => *i += 1,
        }
    }
    Err("the text ends inside a string".to_owned())
}

/// A key as the kit prints it.
fn quoted(k: &str) -> String {
    Json::from(k).to_json()
}

fn compact(v: &Raw) -> String {
    match v {
        Raw::Scalar(s) => s.clone(),
        Raw::List(l) => format!("[{}]", l.iter().map(compact).collect::<Vec<_>>().join(",")),
        Raw::Obj(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{k}:{}", compact(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// A list item's label in a path: its index, and its `key` when it has one.
fn label(n: usize, v: &Json) -> String {
    match v.get("key").and_then(Json::as_str) {
        Some(k) => format!("{n} {k}"),
        None => n.to_string(),
    }
}

/// Every place `got` and the golden differ, by path: a field either side lacks, a list's
/// length, a scalar as printed.
fn diff(path: &str, got: &Json, want: &Raw, out: &mut Vec<String>) {
    match (got, want) {
        (Json::Obj(g), Raw::Obj(w)) => {
            for (k, wv) in w {
                let p = format!("{path}.{}", k.trim_matches('"'));
                match g.iter().find(|(gk, _)| quoted(gk) == *k) {
                    Some((_, gv)) => diff(&p, gv, wv, out),
                    None => out.push(format!("{p}: missing, want {}", compact(wv))),
                }
            }
            for (gk, gv) in g {
                if !w.iter().any(|(k, _)| *k == quoted(gk)) {
                    out.push(format!(
                        "{path}.{gk}: not in the golden, got {}",
                        gv.to_json()
                    ));
                }
            }
        }
        (Json::List(g), Raw::List(w)) => {
            if g.len() != w.len() {
                out.push(format!("{path}: {} items, want {}", g.len(), w.len()));
            }
            for (n, (gv, wv)) in g.iter().zip(w).enumerate() {
                diff(&format!("{path}[{}]", label(n, gv)), gv, wv, out);
            }
        }
        (_, Raw::Scalar(s)) if got.to_json() == *s => {}
        _ => out.push(format!(
            "{path}: got {}, want {}",
            got.to_json(),
            compact(want)
        )),
    }
}
