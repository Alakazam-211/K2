//! `k5 changes diff`: the snapshot picks the files, then each file's two
//! sides (`/cli/git/diff-file?sides=1`) become a unified diff as Python's
//! `difflib` writes it (`_changes_diff` and `_CH_PY_RENDER`,
//! `cli/src/changes.sh`).

use super::{name, q, quote, refusal, trim_amp, usage, Die, Run, Words};
use crate::exit::Exit;
use crate::families::{py_get, py_iter, raised, Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::difflib::unified_diff;
use crate::out::pylib::json::loads;
use crate::out::pylib::ops::{index, py_eq, py_in};
use crate::out::pylib::path::{join, realpath, relpath, split};
use crate::out::{self, py};

/// `changes diff`'s words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opts {
    pub ws: String,
    pub base: String,
    /// `worktree`, `staged`, `unstaged` or `base`.
    pub mode: &'static str,
    /// `-U`: digits.
    pub ctx: String,
    pub paths: Vec<String>,
}

/// `_changes_diff`'s loop and its checks.
pub fn parse(w: &mut Words) -> Result<Opts, Die> {
    let n = name();
    let (mut ws, mut base, mut mode) = (String::new(), String::new(), "worktree");
    let (mut ctx, mut paths) = ("3".to_string(), Vec::new());
    while let Some(a) = w.word() {
        if w.common(a, &mut ws)? {
            continue;
        }
        match a {
            "--base" => base = w.value("--base needs a ref")?,
            "--staged" | "--cached" => mode = "staged",
            "--unstaged" => mode = "unstaged",
            "-U" | "--unified" => ctx = w.value("-U needs a number")?,
            "--" => paths.extend(w.rest()),
            _ => {
                if let Some(v) = a.strip_prefix("--base=") {
                    base = v.to_string();
                } else if let Some(v) = a.strip_prefix("-U") {
                    ctx = v.to_string();
                } else if let Some(v) = a.strip_prefix("--unified=") {
                    ctx = v.to_string();
                } else if a.starts_with('-') {
                    return Err(w.die(usage(format!("unknown flag for {n} changes diff: {a}"))));
                } else {
                    paths.push(a.to_string());
                }
            }
        }
    }
    if ctx.is_empty() || !ctx.bytes().all(|b| b.is_ascii_digit()) {
        return Err(w.die(usage("-U needs a number")));
    }
    if !base.is_empty() && mode != "worktree" {
        return Err(w.die(usage("--base can't be mixed with --staged/--unstaged")));
    }
    if !base.is_empty() {
        mode = "base";
    }
    Ok(Opts {
        ws,
        base,
        mode,
        ctx,
        paths,
    })
}

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).expect("a value from JSON always prints")
}

/// What the snapshot's reader printed, its stderr, and how it ended:
/// `None` for a clean end, else its traceback or `sys.exit(4)`.
pub struct Rows {
    pub stdout: String,
    pub stderr: String,
    pub ok: bool,
}

/// The snapshot's reader: the meta line, then one `path\x1fold\x1fdir`
/// line per file.
pub fn rows(body: &[u8], mode: &str, want: &[String]) -> Rows {
    let mut out = String::new();
    let mut err = String::new();
    match rows_of(body, mode, want, &mut out, &mut err) {
        Ok(true) => Rows {
            stdout: out,
            stderr: err,
            ok: true,
        },
        Ok(false) => Rows {
            stdout: out,
            stderr: err,
            ok: false,
        },
        Err(e) => {
            let shown = raised(out, err, &e);
            Rows {
                stdout: shown.stdout,
                stderr: shown.stderr,
                ok: false,
            }
        }
    }
}

fn str_arg(v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(py::str_of(v)),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: expected str, bytes or os.PathLike object, not {}",
                py::type_name(other)
            ),
        )),
    }
}

fn rows_of(
    body: &[u8],
    mode: &str,
    want: &[String],
    out: &mut String,
    err: &mut String,
) -> Result<bool, Raise> {
    let st = loads(&String::from_utf8_lossy(body)).map_err(|e| Raise::at(3, e.last_line()))?;
    if !py_get(&st, "isRepo", 5)?.is_some_and(py::truthy) {
        return Ok(false);
    }
    let meta = Value::Object(
        ["workspacePath", "repoRoot", "rev"]
            .iter()
            .map(|k| -> Result<(Text, Value), Raise> {
                Ok((
                    Text::from(*k),
                    py_get(&st, k, 7)?.cloned().unwrap_or(Value::Null),
                ))
            })
            .collect::<Result<_, Raise>>()?,
    );
    out.push_str(&dumps(&meta));
    out.push('\n');
    let groups: &[&str] = match mode {
        "worktree" => &["conflicted", "staged", "unstaged", "untracked"],
        "staged" => &["staged"],
        "unstaged" => &["conflicted", "unstaged"],
        _ => &["conflicted", "base", "untracked"],
    };
    let groups = Value::Array(groups.iter().map(|g| s(g)).collect());
    let all = match py_get(&st, "files", 12)? {
        Some(v) if py::truthy(v) => v.clone(),
        _ => Value::Array(Vec::new()),
    };
    let mut files = Vec::new();
    for f in py_iter(&all, 12)? {
        let g = py_get(&f, "group", 12)?.cloned().unwrap_or(Value::Null);
        if py_in(&g, &groups, 12)? {
            files.push(f);
        }
    }
    let mut seen: Vec<Value> = Vec::new();
    let mut rows: Vec<Value> = Vec::new();
    for f in files {
        let p = index(&f, "path", 15)?.clone();
        if matches!(p, Value::Array(_) | Value::Object(_)) {
            return Err(Raise::at(
                15,
                format!("TypeError: unhashable type: '{}'", py::type_name(&p)),
            ));
        }
        if seen.iter().any(|x| py_eq(x, &p)) {
            continue;
        }
        seen.push(p);
        rows.push(f);
    }
    let root_v = match py_get(&st, "repoRoot", 19)? {
        Some(v) if py::truthy(v) => v.clone(),
        _ => s(""),
    };
    let root = realpath(&str_arg(&root_v, 19)?);
    let rel = |p: &str| -> String {
        if p.starts_with('/') {
            let (d, b) = split(p.trim_end_matches('/'));
            let full = join(&realpath(&d), &b);
            if full == root || full.starts_with(&format!("{root}/")) {
                let r = relpath(&full, &root);
                return if p.ends_with('/') { r + "/" } else { r };
            }
        }
        p.strip_prefix("./").unwrap_or(p).to_string()
    };
    let mut picked: Vec<Value> = Vec::new();
    if want.is_empty() {
        picked = rows.clone();
    } else {
        for w in want {
            let r = rel(w);
            let key = r.trim_end_matches('/').to_string();
            let mut hit = Vec::new();
            for f in &rows {
                let p = index(f, "path", 33)?;
                let Value::Str(_) = p else {
                    return Err(Raise::no_attribute(33, p, "rstrip"));
                };
                let p = py::str_of(p);
                if p.trim_end_matches('/') == key
                    || p.starts_with(&format!("{key}/"))
                    || key.is_empty()
                    || key == "."
                {
                    hit.push(f.clone());
                }
            }
            if hit.is_empty() {
                picked.push(Value::Object(vec![
                    (Text::from("path"), s(&r)),
                    (Text::from("oldPath"), Value::Null),
                    (Text::from("dir"), Value::Bool(false)),
                    (Text::from("explicit"), Value::Bool(true)),
                ]));
            } else {
                for h in hit {
                    if !picked.iter().any(|x| py_eq(x, &h)) {
                        picked.push(h);
                    }
                }
            }
        }
    }
    if want.is_empty() && py_get(&st, "truncated", 40)?.is_some_and(py::truthy) {
        let limit = py_get(&st, "limit", 41)?
            .cloned()
            .unwrap_or_else(|| Value::Int("1000".into()));
        err.push_str(&format!(
            "note: the change list is capped at {} files; name paths to see the rest\n",
            crate::families::py_d(&limit, 41)?
        ));
    }
    for f in &picked {
        let path = py::str_of(index(f, "path", 44)?);
        let old = match py_get(f, "oldPath", 44)? {
            Some(v) if py::truthy(v) => py::str_of(v),
            _ => String::new(),
        };
        let dir = if py_get(f, "dir", 44)?.is_some_and(py::truthy) {
            1
        } else {
            0
        };
        out.push_str(&format!("{path}\x1f{old}\x1f{dir}\n"));
    }
    Ok(true)
}

/// `lines(t)` in `_CH_PY_RENDER`: the text cut after each newline.
fn lines(t: &str) -> Vec<String> {
    if t.is_empty() {
        return Vec::new();
    }
    let parts: Vec<&str> = t.split('\n').collect();
    let mut out: Vec<String> = parts[..parts.len() - 1]
        .iter()
        .map(|p| format!("{p}\n"))
        .collect();
    let last = parts[parts.len() - 1];
    if !last.is_empty() {
        out.push(last.to_string());
    }
    out
}

/// `_CH_PY_RENDER` on one `diff-file?sides=1` answer: the patch (or, with
/// `as_json`, the one JSON line), or its traceback.
pub fn render(body: &[u8], ctx: usize, as_json: bool) -> Shown {
    match render_of(body, ctx, as_json) {
        Ok(text) => Shown::out(text),
        Err(e) => raised(String::new(), String::new(), &e),
    }
}

fn str_or_empty(x: Option<&Value>) -> Value {
    match x {
        Some(v) if py::truthy(v) => v.clone(),
        _ => s(""),
    }
}

fn render_of(body: &[u8], ctx: usize, as_json: bool) -> Result<String, Raise> {
    let d = loads(&String::from_utf8_lossy(body)).map_err(|e| Raise::at(4, e.last_line()))?;
    let new_path = str_or_empty(py_get(&d, "path", 5)?);
    let old_path = match py_get(&d, "oldPath", 6)? {
        Some(v) if py::truthy(v) => v.clone(),
        _ => new_path.clone(),
    };
    let obj = |x: Option<&Value>| match x {
        Some(v) if py::truthy(v) => v.clone(),
        _ => Value::Object(Vec::new()),
    };
    let old = obj(py_get(&d, "old", 7)?);
    let new = obj(py_get(&d, "new", 7)?);
    let concat = |prefix: &str, v: &Value, line: u32| -> Result<String, Raise> {
        match v {
            Value::Str(_) => Ok(format!("{prefix}{}", py::str_of(v))),
            other => Err(Raise::at(
                line,
                format!(
                    "TypeError: can only concatenate str (not \"{}\") to str",
                    py::type_name(other)
                ),
            )),
        }
    };
    let old_exists = py_get(&old, "exists", 8)?.cloned().unwrap_or(Value::Null);
    let a = if py::truthy(&old_exists) {
        concat("a/", &old_path, 8)?
    } else {
        "/dev/null".into()
    };
    let new_exists = py_get(&new, "exists", 9)?.cloned().unwrap_or(Value::Null);
    let b = if py::truthy(&new_exists) {
        concat("b/", &new_path, 9)?
    } else {
        "/dev/null".into()
    };
    let (op, np) = (py::str_of(&old_path), py::str_of(&new_path));
    let mut head = Vec::new();
    if py::truthy(&old_exists) && py::truthy(&new_exists) {
        head.push(format!("diff --git a/{op} b/{np}\n"));
        if !py_eq(&old_path, &new_path) {
            head.push(format!("rename from {op}\n"));
            head.push(format!("rename to {np}\n"));
        }
    }
    let mut patch = Vec::new();
    if py_get(&d, "binary", 29)?.is_some_and(py::truthy) {
        patch = head;
        patch.push(format!("Binary files {a} and {b} differ\n"));
    } else if py_get(&d, "tooLarge", 31)?.is_some_and(py::truthy) {
        patch = head;
        patch.push(format!(
            "Large files {a} and {b} differ (over 10 MB or 120,000 lines; not shown)\n"
        ));
    } else {
        let ot = str_or_empty(py_get(&old, "text", 34)?);
        let nt = str_or_empty(py_get(&new, "text", 34)?);
        if !py_eq(&ot, &nt) || !py_eq(&old_exists, &new_exists) || !py_eq(&old_path, &new_path) {
            let text = |v: &Value| -> Result<String, Raise> {
                match v {
                    Value::Str(_) => Ok(py::str_of(v)),
                    other if !py::truthy(other) => Ok(String::new()),
                    other => Err(Raise::inside(
                        37,
                        14,
                        "lines",
                        format!(
                            "AttributeError: '{}' object has no attribute 'split'",
                            py::type_name(other)
                        ),
                    )),
                }
            };
            let (ol, nl) = (lines(&text(&ot)?), lines(&text(&nt)?));
            let mut body = Vec::new();
            for ln in unified_diff(&ol, &nl, &a, &b, ctx) {
                if ln.ends_with('\n') {
                    body.push(ln);
                } else {
                    body.push(format!("{ln}\n"));
                    body.push("\\ No newline at end of file\n".to_string());
                }
            }
            patch = head;
            patch.extend(body);
        }
    }
    let text: String = patch.concat();
    if as_json {
        let mut meta: Vec<(Text, Value)> = Vec::new();
        for k in [
            "path",
            "oldPath",
            "mode",
            "baseRef",
            "rev",
            "binary",
            "tooLarge",
            "additions",
            "deletions",
        ] {
            meta.push((
                Text::from(k),
                py_get(&d, k, 46)?.cloned().unwrap_or(Value::Null),
            ));
        }
        meta.push((Text::from("patch"), s(&text)));
        return Ok(dumps(&Value::Object(meta)) + "\n");
    }
    Ok(text)
}

/// `_changes_diff` after its words.
pub fn run(r: &mut Run, o: Opts) -> Result<(), Exit> {
    let t = r.target(&o.ws);
    let want = if o.paths.is_empty() {
        Vec::new()
    } else {
        super::files(&super::physical_cwd(), &t.dir, &o.paths)
    };
    let mut query = q(&t.path);
    if !o.base.is_empty() {
        query.push_str(&format!("base={}&", quote(&o.base)));
    }
    let snap = r.call(false, "/cli/git/snapshot", trim_amp(&query))?;
    let rws = rows(&snap, o.mode, &want);
    if !rws.stderr.is_empty() {
        let _ = out::write_flushed(&mut std::io::stderr().lock(), rws.stderr.as_bytes());
    }
    if !rws.ok {
        return Err(Die {
            json: r.json,
            exit: Exit::Refused,
            ..super::die("not_a_repo", "not a git repository")
        }
        .print());
    }
    let mut text = rws.stdout;
    while text.ends_with('\n') {
        text.pop();
    }
    let (meta, rows_text) = match text.split_once('\n') {
        Some((m, rest)) => (m.to_string(), rest.to_string()),
        None => (text.clone(), String::new()),
    };
    let modeq = if o.mode == "base" {
        format!("base={}", quote(&o.base))
    } else {
        format!("mode={}", o.mode)
    };
    let ctx = o.ctx.trim_start_matches('0').parse::<usize>().unwrap_or(
        if o.ctx.trim_start_matches('0').is_empty() {
            0
        } else {
            usize::MAX / 4
        },
    );
    let mut failed = Exit::Ok;
    let mut jl: Vec<String> = Vec::new();
    for line in rows_text.split('\n') {
        let mut parts = line.splitn(3, '\x1f');
        let fpath = parts.next().unwrap_or("");
        let fold = parts.next().unwrap_or("");
        let fdir = parts.next().unwrap_or("");
        if fpath.is_empty() {
            continue;
        }
        if fdir == "1" {
            out::stderr_line(&format!(
                "note: {fpath} is an untracked folder (not expanded); name a file inside it"
            ));
            continue;
        }
        let mut fq = format!("{}file_path={}&sides=1&{modeq}", q(&t.path), quote(fpath));
        if !fold.is_empty() {
            fq.push_str(&format!("&old_path={}", quote(fold)));
        }
        let sent = r.send(false, "/cli/git/diff-file", &fq)?;
        if sent.status != "200" {
            if !r.json {
                out::stderr_line(&format!("({fpath})"));
            }
            failed = refusal(&sent.status, &sent.body, r.json).print();
            if sent.status == "000" {
                break;
            }
            continue;
        }
        let shown = render(&sent.body, ctx, r.json);
        if !shown.stderr.is_empty() {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), shown.stderr.as_bytes());
        }
        let rendered = shown.stdout.replace('\0', "");
        if r.json {
            jl.push(rendered);
        } else if let Err(e) = out::stdout(&rendered) {
            return Err(out::after_stdout_error(&e));
        }
    }
    if r.json {
        let shown = assemble(&meta, &jl, o.mode, &o.base);
        if shown.print() != Exit::Ok {
            return Err(Exit::Error);
        }
    }
    if failed != Exit::Ok {
        return Err(failed);
    }
    Ok(())
}

/// The `--json` line: the snapshot's meta, the mode, the base, and each
/// file's line.
pub fn assemble(meta: &str, jl: &[String], mode: &str, base: &str) -> Shown {
    let r = (|| -> Result<String, Raise> {
        let m = if meta.is_empty() { "{}" } else { meta };
        let st = loads(m).map_err(|e| Raise::at(3, e.last_line()))?;
        let mut files = Vec::new();
        let joined: String = if jl.is_empty() {
            "\n".to_string()
        } else {
            jl.iter().map(|l| format!("{l}\n")).collect()
        };
        for l in joined.split_inclusive('\n') {
            if l.trim_matches(crate::families::py_space).is_empty() {
                continue;
            }
            files.push(loads(l).map_err(|e| Raise::at(4, e.last_line()))?);
        }
        let v = Value::Object(vec![
            (
                Text::from("workspacePath"),
                py_get(&st, "workspacePath", 5)?
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            (
                Text::from("repoRoot"),
                py_get(&st, "repoRoot", 5)?.cloned().unwrap_or(Value::Null),
            ),
            (
                Text::from("rev"),
                py_get(&st, "rev", 5)?.cloned().unwrap_or(Value::Null),
            ),
            (Text::from("mode"), s(mode)),
            (
                Text::from("base"),
                if base.is_empty() {
                    Value::Null
                } else {
                    s(base)
                },
            ),
            (Text::from("files"), Value::Array(files)),
        ]);
        Ok(dumps(&v) + "\n")
    })();
    match r {
        Ok(t) => Shown::out(t),
        Err(e) => raised(String::new(), String::new(), &e),
    }
}
