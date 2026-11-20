//! The shell's Python readers for `k5 changes`: what each prints of a 200's
//! body, with Python's exception where the body is not what it expects
//! (`json.load` at line 3 of each, as `python3 -c` numbers it).

use super::IdsVerb;
use crate::exit::Exit;
use crate::families::{get, py_d, py_get, py_iter, py_len, raised, Raise, Shown};
use crate::out::json::{self, Text, Value};
use crate::out::py;
use crate::out::pylib::json::loads;
use crate::out::pylib::ops::{index, join, py_eq, splitlines};

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

fn text_of(body: &[u8]) -> String {
    String::from_utf8_lossy(body).into_owned()
}

/// `json.load(sys.stdin)` at line 3.
fn load(body: &[u8]) -> Result<Value, Raise> {
    loads(&text_of(body)).map_err(|e| Raise::at(3, e.last_line()))
}

/// Lines printed so far, then the end.
#[derive(Default)]
struct Out {
    out: String,
    err: String,
}

impl Out {
    fn p(&mut self, line: impl AsRef<str>) {
        self.out.push_str(line.as_ref());
        self.out.push('\n');
    }

    fn e(&mut self, line: impl AsRef<str>) {
        self.err.push_str(line.as_ref());
        self.err.push('\n');
    }

    fn end(self, r: Result<Exit, Raise>) -> Shown {
        match r {
            Ok(exit) => Shown {
                stdout: self.out,
                stderr: self.err,
                exit,
            },
            Err(e) => raised(self.out, self.err, &e),
        }
    }
}

fn or_default(x: Option<&Value>, default: Value) -> Value {
    match x {
        Some(v) if py::truthy(v) => v.clone(),
        _ => default,
    }
}

fn get_or(v: &Value, key: &str, default: Value, line: u32) -> Result<Value, Raise> {
    Ok(py_get(v, key, line)?.cloned().unwrap_or(default))
}

fn get_s(v: &Value, key: &str, line: u32) -> Result<String, Raise> {
    Ok(py_get(v, key, line)?.map_or_else(|| "None".to_string(), py::str_of))
}

fn truthy(v: &Value, key: &str, line: u32) -> Result<bool, Raise> {
    Ok(py_get(v, key, line)?.is_some_and(py::truthy))
}

/// `x[:n]` on a string or a list; Python's error otherwise.
pub fn head(x: &Value, n: usize, line: u32) -> Result<Value, Raise> {
    match x {
        Value::Str(t) => Ok(Value::Str(Text::from(
            py::str_of(&Value::Str(t.clone()))
                .chars()
                .take(n)
                .collect::<String>(),
        ))),
        Value::Array(items) => Ok(Value::Array(items.iter().take(n).cloned().collect())),
        Value::Object(_) => Err(Raise::at(line, format!("KeyError: slice(None, {n}, None)"))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `a + b` where `a` is a string.
fn concat(a: &str, b: &Value, line: u32) -> Result<String, Raise> {
    match b {
        Value::Str(_) => Ok(format!("{a}{}", py::str_of(b))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// `"%-<width>s" % name`.
fn pad(name: &str, width: usize) -> String {
    crate::families::ljust(name, width)
}

/// `_changes_target`'s Python on what `/cli/workspace/resolve` answered:
/// `path`, `projectPath` or `project_path`, as `$(…)` holds it.
pub fn resolved(body: &[u8]) -> String {
    let raw = text_of(body);
    let raw = if raw.is_empty() {
        "{}".to_string()
    } else {
        raw
    };
    let Ok(Value::Object(p)) = json::parse(&raw) else {
        return String::new();
    };
    let pick = ["path", "projectPath", "project_path"]
        .iter()
        .find_map(|k| get(&p, k).filter(|v| py::truthy(v)));
    let mut out = pick.map_or_else(String::new, py::str_of);
    while out.ends_with('\n') {
        out.pop();
    }
    out.replace('\0', "")
}

/// `_changes_list`'s reader. Exit 4 (`sys.exit(4)`) is no repository.
pub fn list(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = list_lines(body, &mut o);
    o.end(r)
}

fn list_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let st = load(body)?;
    if !truthy(&st, "isRepo", 4)? {
        o.p(format!(
            "Not a git repository: {}",
            py::str_of(&get_or(&st, "workspacePath", s(""), 5)?)
        ));
        return Ok(Exit::NotFound);
    }
    let mut head_v = or_default(py_get(&st, "branch", 7)?, s("(no branch)"));
    if truthy(&st, "detached", 8)? {
        let oid = or_default(py_get(&st, "headOid", 9)?, s(""));
        head_v = s(&format!(
            "HEAD detached at {}",
            py::str_of(&head(&oid, 7, 9)?)
        ));
    } else if truthy(&st, "unborn", 10)? {
        head_v = s(&format!("{} (no commits yet)", py::str_of(&head_v)));
    }
    let mut line = format!("On {}", py::str_of(&head_v));
    if truthy(&st, "upstream", 13)? {
        line.push_str(&format!(
            " · {} ahead, {} behind {}",
            py::str_of(&get_or(&st, "ahead", Value::Int("0".into()), 14)?),
            py::str_of(&get_or(&st, "behind", Value::Int("0".into()), 14)?),
            py::str_of(index(&st, "upstream", 14)?)
        ));
    }
    o.p(line);
    if truthy(&st, "state", 16)? {
        let state = index(&st, "state", 16)?;
        if !py_eq(state, &s("clean")) {
            let Value::Str(t) = state else {
                return Err(Raise::no_attribute(17, state, "replace"));
            };
            o.p(format!(
                "In progress: {}",
                py::str_of(&Value::Str(t.clone())).replace('_', " ")
            ));
        }
    }
    let b = py_get(&st, "base", 18)?.cloned().unwrap_or(Value::Null);
    if py::truthy(&b) {
        let mb = or_default(py_get(&b, "mergeBase", 20)?, s(""));
        let mb = head(&mb, 7, 20)?;
        let mb = if py::truthy(&mb) {
            py::str_of(&mb)
        } else {
            "none".to_string()
        };
        o.p(format!(
            "Compared with {} (merge-base {mb} · {} ahead, {} behind)",
            get_s(&b, "ref", 21)?,
            py::str_of(&get_or(&b, "ahead", Value::Int("0".into()), 21)?),
            py::str_of(&get_or(&b, "behind", Value::Int("0".into()), 21)?)
        ));
    }
    let files = or_default(py_get(&st, "files", 22)?, Value::Array(Vec::new()));
    let c = or_default(py_get(&st, "counts", 23)?, Value::Object(Vec::new()));
    if !py::truthy(&files) {
        o.p("No changes.");
        return Ok(Exit::Ok);
    }
    let files = py_iter(&files, 29)?;
    let mut width = 0usize;
    for f in &files {
        let path = py_len(index(f, "path", 29)?, 29)?;
        let old = if truthy(f, "oldPath", 29)? {
            py_len(index(f, "oldPath", 29)?, 29)? + 3
        } else {
            0
        };
        width = width.max(path + old);
    }
    let width = width.min(60);
    let titles = [
        ("conflicted", "Conflicted"),
        ("staged", "Staged"),
        ("unstaged", "Unstaged"),
        ("base", "Changed vs base"),
        ("untracked", "Untracked"),
    ];
    for (g, title) in titles {
        let mut rows = Vec::new();
        for f in &files {
            if py_get(f, "group", 31)?.is_some_and(|x| py_eq(x, &s(g))) {
                rows.push(f.clone());
            }
        }
        if rows.is_empty() && !truthy(&c, g, 32)? {
            continue;
        }
        o.p("");
        let count = get_or(&c, g, Value::Int(rows.len().to_string()), 35)?;
        o.p(format!("{title} ({})", py_d(&count, 35)?));
        for f in &rows {
            let path = index(f, "path", 37)?;
            let mut name = match path {
                Value::Str(_) => py::str_of(path),
                other => {
                    return Err(Raise::at(
                        37,
                        format!(
                            "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                            py::type_name(other)
                        ),
                    ))
                }
            };
            if truthy(f, "oldPath", 37)? {
                name = concat(&format!("{name} <- "), index(f, "oldPath", 37)?, 37)?;
            }
            let stat = if truthy(f, "dir", 38)? {
                "(folder)".to_string()
            } else if truthy(f, "binary", 40)? {
                "(binary)".to_string()
            } else if py_get(f, "additions", 42)?.is_none_or(|a| *a == Value::Null) {
                String::new()
            } else {
                let del = or_default(py_get(f, "deletions", 45)?, Value::Int("0".into()));
                format!(
                    "+{} -{}",
                    py_d(index(f, "additions", 45)?, 45)?,
                    py_d(&del, 45)?
                )
            };
            let status = get_or(f, "status", s("?"), 46)?;
            o.p(format!(
                "  {}  {}  {stat}",
                py::str_of(&status),
                pad(&name, width)
            ));
        }
    }
    o.p("");
    if truthy(&st, "truncated", 48)? {
        let limit = get_or(&st, "limit", Value::Int(files.len().to_string()), 49)?;
        let total = get_or(&c, "total", Value::Int("0".into()), 49)?;
        o.p(format!(
            "Showing the first {} of {} changes (too many to list).",
            py_d(&limit, 49)?,
            py_d(&total, 49)?
        ));
    } else {
        let total = get_or(&c, "total", Value::Int(files.len().to_string()), 51)?;
        let plural = if py_eq(&total, &Value::Int("1".into())) {
            ""
        } else {
            "s"
        };
        o.p(format!("{} change{plural}", py_d(&total, 51)?));
    }
    Ok(Exit::Ok)
}

/// `_changes_discard`'s `--json` check: exit 1 when `failed` is there.
pub fn discard_failed(body: &[u8]) -> Shown {
    let o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let r = loads(&text_of(body)).map_err(|e| Raise::at(1, e.last_line()))?;
        Ok(if truthy(&r, "failed", 1)? {
            Exit::Error
        } else {
            Exit::Ok
        })
    })();
    o.end(r)
}

/// `_changes_discard`'s reader: exit 1 when something failed.
pub fn discard(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let r = load(body)?;
        let list = |k: &str, line: u32| -> Result<Value, Raise> {
            Ok(or_default(py_get(&r, k, line)?, Value::Array(Vec::new())))
        };
        let (restored, trashed, unstaged) = (
            list("restored", 4)?,
            list("trashed", 4)?,
            list("unstaged", 4)?,
        );
        let (skipped, failed) = (list("skipped", 5)?, list("failed", 5)?);
        if !(py::truthy(&restored) || py::truthy(&trashed) || py::truthy(&unstaged)) {
            o.p("Nothing discarded.");
        }
        for p in py_iter(&restored, 8)? {
            o.p(format!("  restored  {}", py::str_of(&p)));
        }
        for p in py_iter(&unstaged, 10)? {
            o.p(format!("  unstaged  {}", py::str_of(&p)));
        }
        for p in py_iter(&trashed, 12)? {
            o.p(format!(
                "  trashed   {}  (moved to the Trash)",
                py::str_of(&p)
            ));
        }
        for x in py_iter(&skipped, 14)? {
            o.p(format!(
                "  skipped   {}: {}",
                get_s(&x, "path", 15)?,
                get_s(&x, "reason", 15)?
            ));
        }
        for x in py_iter(&failed, 16)? {
            o.e(format!(
                "  FAILED    {}: {} (left untouched)",
                get_s(&x, "path", 17)?,
                get_s(&x, "error", 17)?
            ));
        }
        Ok(if py::truthy(&failed) {
            Exit::Error
        } else {
            Exit::Ok
        })
    })();
    o.end(r)
}

/// `_changes_commit`'s count of staged files: `-1` for no repository,
/// else `str()` of the count. `Err`: Python's traceback, and `set -e`
/// ends the command with 1.
pub fn staged(body: &[u8]) -> Result<String, Shown> {
    let r = (|| -> Result<String, Raise> {
        let st = load(body)?;
        if !truthy(&st, "isRepo", 4)? {
            return Ok("-1".into());
        }
        let c = or_default(py_get(&st, "counts", 4)?, Value::Object(Vec::new()));
        let n = get_or(&c, "staged", Value::Int("0".into()), 4)?;
        Ok(py::str_of(&n))
    })();
    r.map_err(|e| raised(String::new(), String::new(), &e))
}

/// `_changes_commit`'s reader.
pub fn committed(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let r = load(body)?;
        let oid = or_default(py_get(&r, "oid", 4)?, s(""));
        let oid = head(&oid, 7, 4)?;
        let first = if truthy(&r, "message", 4)? {
            let m = index(&r, "message", 4)?;
            let Value::Str(t) = m else {
                return Err(Raise::no_attribute(4, m, "splitlines"));
            };
            let lines = splitlines(&py::str_of(&Value::Str(t.clone())));
            match lines.first() {
                Some(l) => l.clone(),
                None => {
                    return Err(Raise::at(4, "IndexError: list index out of range"));
                }
            }
        } else {
            String::new()
        };
        o.p(format!("Committed {}: {first}", py::str_of(&oid)));
        Ok(Exit::Ok)
    })();
    o.end(r)
}

/// `_changes_notes_list`'s reader.
pub fn notes(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let r = load(body)?;
        let notes = or_default(py_get(&r, "notes", 4)?, Value::Array(Vec::new()));
        let c = or_default(py_get(&r, "counts", 4)?, Value::Object(Vec::new()));
        let zero = || Value::Int("0".into());
        o.p(format!(
            "Review notes on {}: {} open ({} not sent yet), {} resolved",
            py::str_of(&get_or(&r, "workspacePath", s(""), 5)?),
            py_d(&get_or(&c, "open", zero(), 6)?, 5)?,
            py_d(&get_or(&c, "unsent", zero(), 6)?, 5)?,
            py_d(&get_or(&c, "resolved", zero(), 6)?, 5)?
        ));
        if !py::truthy(&notes) {
            o.p("No notes to show.");
            return Ok(Exit::Ok);
        }
        for n in py_iter(&notes, 10)? {
            let start = or_default(py_get(&n, "currentStartLine", 11)?, Value::Null);
            let start = if py::truthy(&start) {
                start
            } else {
                get_or(&n, "startLine", Value::Null, 11)?
            };
            let end = or_default(py_get(&n, "currentEndLine", 12)?, Value::Null);
            let end = if py::truthy(&end) {
                end
            } else {
                get_or(&n, "endLine", Value::Null, 12)?
            };
            let span = if py_eq(&start, &end) {
                py::str_of(&start)
            } else {
                format!("{}-{}", py::str_of(&start), py::str_of(&end))
            };
            let mut place = format!("{}:{span}", get_s(&n, "path", 13)?);
            if py_get(&n, "side", 14)?.is_some_and(|x| py_eq(x, &s("old"))) {
                place.push_str(" (before the change)");
            }
            let mut tags = Vec::new();
            if truthy(&n, "resolvedAt", 17)? {
                let by = or_default(py_get(&n, "resolvedBy", 18)?, s("?"));
                tags.push(format!("resolved by {}", py::str_of(&by)));
            } else if truthy(&n, "sentAt", 19)? {
                tags.push("sent".to_string());
            }
            if truthy(&n, "outdated", 21)? && !truthy(&n, "resolvedAt", 21)? {
                tags.push("outdated: the lines changed".to_string());
            }
            o.p("");
            let author = or_default(py_get(&n, "authorName", 24)?, Value::Null);
            let author = if py::truthy(&author) {
                author
            } else {
                get_or(&n, "author", Value::Null, 24)?
            };
            let tags = if tags.is_empty() {
                String::new()
            } else {
                format!("  [{}]", tags.join(", "))
            };
            o.p(format!(
                "{}  {place}  by {}{tags}",
                get_s(&n, "id", 24)?,
                py::str_of(&author)
            ));
            let body = or_default(py_get(&n, "body", 25)?, s(""));
            let Value::Str(t) = &body else {
                return Err(Raise::no_attribute(25, &body, "splitlines"));
            };
            let mut lines = splitlines(&py::str_of(&Value::Str(t.clone())));
            if lines.is_empty() {
                lines.push(String::new());
            }
            for l in lines {
                o.p(format!("    {l}"));
            }
        }
        Ok(Exit::Ok)
    })();
    o.end(r)
}

/// `_changes_notes_ids`'s reader.
pub fn ids(body: &[u8], verb: IdsVerb) -> Shown {
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let r = load(body)?;
        if verb == IdsVerb::Delete {
            let d = or_default(py_get(&r, "deleted", 5)?, Value::Array(Vec::new()));
            o.p(format!("Deleted {}", join(&d, ", ", 5)?));
            return Ok(Exit::Ok);
        }
        let done = or_default(py_get(&r, "changed", 7)?, Value::Array(Vec::new()));
        let same = or_default(py_get(&r, "unchanged", 8)?, Value::Array(Vec::new()));
        let (word, state) = if verb == IdsVerb::Resolve {
            ("Resolved", "resolved")
        } else {
            ("Reopened", "open")
        };
        if py::truthy(&done) {
            o.p(format!("{word} {}", join(&done, ", ", 11)?));
        }
        if py::truthy(&same) {
            o.p(format!("Already {state}: {}", join(&same, ", ", 13)?));
        }
        Ok(Exit::Ok)
    })();
    o.end(r)
}

/// `_changes_notes_add`'s reader.
pub fn added(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let v = load(body)?;
        let n = or_default(py_get(&v, "note", 3)?, Value::Object(Vec::new()));
        let (st, en) = (
            get_or(&n, "startLine", Value::Null, 4)?,
            get_or(&n, "endLine", Value::Null, 4)?,
        );
        let span = if py_eq(&st, &en) {
            py::str_of(&st)
        } else {
            format!("{}-{}", py::str_of(&st), py::str_of(&en))
        };
        o.p(format!(
            "Added note {} on {}:{span}",
            get_s(&n, "id", 5)?,
            get_s(&n, "path", 5)?
        ));
        Ok(Exit::Ok)
    })();
    o.end(r)
}

/// `_changes_notes_send`'s reader.
pub fn sent(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let r = load(body)?;
        let n = get_or(&r, "sent", Value::Int("0".into()), 4)?;
        let plural = if py_eq(&n, &Value::Int("1".into())) {
            ""
        } else {
            "s"
        };
        let mut line = format!(
            "Sent {} note{plural} to the agent of {}",
            py_d(&n, 5)?,
            py::str_of(&get_or(&r, "target", s(""), 5)?)
        );
        if truthy(&r, "deliveryId", 6)? {
            line.push_str(&format!(
                " (delivery {})",
                py::str_of(index(&r, "deliveryId", 7)?)
            ));
        }
        o.p(line);
        if truthy(&r, "remaining", 9)? {
            o.p(format!(
                "{} more did not fit in one message; send again.",
                py_d(index(&r, "remaining", 10)?, 10)?
            ));
        }
        Ok(Exit::Ok)
    })();
    o.end(r)
}

/// `_changes_open`'s reader.
pub fn opened(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let r = load(body)?;
        o.p(format!(
            "Asked {} to open the Changes tab for {} (any window showing that workspace opens it).",
            k2_client::product_name!(),
            py::str_of(&get_or(&r, "workspacePath", s(""), 4)?)
        ));
        Ok(Exit::Ok)
    })();
    o.end(r)
}

/// `_changes_init`'s reader.
pub fn initialized(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let r = load(body)?;
        o.p(format!(
            "Initialized a git repository in {} (no commits yet).",
            py::str_of(&get_or(&r, "repoRoot", s(""), 4)?)
        ));
        Ok(Exit::Ok)
    })();
    o.end(r)
}
