//! `k5 compute cache [list] [--node N] [--workspace WS] [--name NAME]
//! [--lineage trusted|scratch|release|all] [--json] | clear (--workspace
//! WS | --all) [--name NAME] [--node N]`, as `cmd_compute_cache` answers
//! it (`cli/src/compute-run.sh`):
//!
//! - list: one GET of `/cli/compute/cache` over TCP, then a table;
//! - `--lineage`: one GET of `/cli/compute/cache/lineages`, then each
//!   node's cache generations;
//! - clear: one POST to `/cli/compute/cache/clear` over TCP, then what
//!   was cleared and what a running job holds.
//!
//! `--json` prints the answer as it came.

use super::super::writes::{dumps, load, object, print_body, str_value, Cp, Words};
use crate::exit::Exit;
use crate::families::{py_iter, raised, Raise, Shown};
use crate::out::json::Value;
use crate::out::py;
use crate::out::pylib::ops::{get_in, index, or};
use crate::transport::encode::quote;
use k2_client::cli_name;

pub const LIST: &str = "/cli/compute/cache";
pub const LINEAGES: &str = "/cli/compute/cache/lineages";
pub const CLEAR: &str = "/cli/compute/cache/clear";

pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute cache [list] [--node N] [--workspace WS] [--name NAME] [--lineage trusted|scratch|release|all] [--json] | clear (--workspace WS | --all) [--name NAME] [--node N]"
);
pub const CLEAR_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute cache clear --workspace WS [--name NAME] (or --all for every workspace's caches)"
);
pub const LINEAGE_USAGE: &str = "usage: --lineage trusted|scratch|release|all";
pub const LINEAGE_LIST_USAGE: &str = concat!(
    "usage: --lineage goes with ",
    cli_name!(),
    " compute cache list"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub clear: bool,
    pub node: String,
    pub workspace: String,
    pub name: String,
    pub all: bool,
    pub lineage: String,
    pub json: bool,
}

/// The words after `cache`. `in_session`: `_compute_in_session`.
pub fn parse(words: &[&str], in_session: bool) -> Words<Ask> {
    let mut a = Ask::default();
    let mut words = words;
    match words.first() {
        Some(&("list" | "ls")) => words = &words[1..],
        Some(&("clear" | "rm")) => {
            a.clear = true;
            words = &words[1..];
        }
        _ => {}
    }
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let v = words.get(i + 1).copied().unwrap_or("").to_string();
        match w {
            "--lineage" => {
                if !matches!(v.as_str(), "trusted" | "scratch" | "release" | "all") {
                    return Words::Usage(LINEAGE_USAGE.into());
                }
                a.lineage = v;
                i += 2;
            }
            "--node" | "--workspace" | "--name" => {
                match w {
                    "--node" => a.node = v,
                    "--workspace" => a.workspace = v,
                    _ => a.name = v,
                }
                if i + 2 > words.len() {
                    return Words::ShiftFailed(words[i].to_string());
                }
                i += 2;
            }
            "--all" => {
                a.all = true;
                i += 1;
            }
            "--json" => {
                a.json = true;
                i += 1;
            }
            _ => return Words::Usage(USAGE.into()),
        }
    }
    if !a.lineage.is_empty() {
        if a.clear {
            return Words::Usage(LINEAGE_LIST_USAGE.into());
        }
    } else if a.clear && a.workspace.is_empty() && !a.all && !in_session {
        return Words::Usage(CLEAR_USAGE.into());
    }
    Words::Ask(a)
}

/// `q="$q&key=$(urlencode "$v")"` for each value set, the first `&` gone.
fn query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}={}", quote(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The request and what it prints.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let answer = if !a.lineage.is_empty() {
        let mut q = query(&[("node", &a.node), ("workspace", &a.workspace)]);
        if a.lineage != "all" {
            if !q.is_empty() {
                q.push('&');
            }
            q.push_str(&format!("lineage={}", a.lineage));
        }
        cp.call(false, LINEAGES, &q, false)
    } else if a.clear {
        let mut pairs = vec![];
        for (k, v) in [
            ("node", &a.node),
            ("workspace", &a.workspace),
            ("name", &a.name),
        ] {
            if !v.is_empty() {
                pairs.push((k, str_value(v)));
            }
        }
        // `K2_CJ_all_b="$all"`: 0 is a value too, so `all` is always there.
        pairs.push(("all", Value::Bool(a.all)));
        cp.call(true, CLEAR, &dumps(&object(pairs)), true)
    } else {
        let q = query(&[
            ("node", &a.node),
            ("workspace", &a.workspace),
            ("name", &a.name),
        ]);
        cp.call(false, LIST, &q, true)
    };
    let answer = match answer {
        Ok(b) => b,
        Err(e) => return e,
    };
    if a.json {
        return print_body(&answer);
    }
    if !a.lineage.is_empty() {
        lineage_reader(&answer).print()
    } else {
        reader(&answer, a.clear, now()).print()
    }
}

/// `int(time.time())`.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A number `json.load` gave, as Python compares and divides it.
fn num(v: &Value, line: u32, what: &str) -> Result<f64, Raise> {
    match v {
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        Value::Int(d) => Ok(d.parse::<f64>().unwrap_or(f64::NAN)),
        Value::Float(f) => Ok(*f),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '<' not supported between instances of '{}' and '{what}'",
                py::type_name(other)
            ),
        )),
    }
}

/// `size(b)`: bytes as B, KB, MB, GB or TB.
fn size(b: &Value, from: u32) -> Result<String, Raise> {
    let n = num(b, 7, "int").map_err(|e| Raise::inside(from, 7, "size", e.exception))?;
    if n < 1024.0 {
        return py::percent_d(b)
            .map(|d| format!("{d} B"))
            .map_err(|e| Raise::inside(from, 8, "size", e));
    }
    let mut x = n;
    for u in ["KB", "MB", "GB", "TB"] {
        x /= 1024.0;
        if x < 1024.0 || u == "TB" {
            return Ok(format!("{x:.1} {u}"));
        }
    }
    unreachable!("TB always returns")
}

/// `ago(t)`.
fn ago(t: Option<&Value>, now: i64, from: u32) -> Result<String, Raise> {
    let Some(t) = t.filter(|t| py::truthy(t)) else {
        return Ok("-".into());
    };
    let secs = match t {
        Value::Int(d) => d.parse::<i64>().unwrap_or(0),
        Value::Float(f) => f.trunc() as i64,
        Value::Bool(b) => i64::from(*b),
        Value::Str(_) => super::super::writes::int_of(t)
            .map_err(|e| Raise::inside(from, 12, "ago", e))?
            .parse::<i64>()
            .unwrap_or(0),
        other => {
            return Err(Raise::inside(
                from,
                12,
                "ago",
                format!(
                    "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
                    py::type_name(other)
                ),
            ))
        }
    };
    let d = (now - secs).max(0);
    Ok(if d < 3600 {
        format!("{}m ago", d / 60)
    } else if d < 86400 {
        format!("{}h ago", d / 3600)
    } else {
        format!("{}d ago", d / 86400)
    })
}

/// `x[:n]`: a string's first `n` code points, a list's first `n` items.
fn head_of(x: &Value, n: usize, line: u32) -> Result<String, Raise> {
    match x {
        Value::Str(_) => Ok(py::str_of(x).chars().take(n).collect()),
        Value::Array(items) => Ok(py::str_of(&Value::Array(
            items.iter().take(n).cloned().collect(),
        ))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `e.get("workspace") or "(<what> %s)" % e["wsKey"][:8]`.
fn ws_of(e: &Value, what: &str, line: u32) -> Result<String, Raise> {
    match get_in(e, "workspace", line)? {
        Some(w) if py::truthy(w) => Ok(py::str_of(w)),
        _ => Ok(format!(
            "({what} {})",
            head_of(index(e, "wsKey", line)?, 8, line)?
        )),
    }
}

fn ljust(s: &str, n: usize) -> String {
    crate::families::ljust(s, n)
}

fn rjust(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len < n {
        format!("{}{s}", " ".repeat(n - len))
    } else {
        s.to_string()
    }
}

/// The list or clear reader (`K2_CP_VERB`), at `now`.
pub fn reader(body: &[u8], clear: bool, now: i64) -> Shown {
    let (mut out, mut err) = (String::new(), String::new());
    let tag = concat!("[", cli_name!(), " compute]");
    let r = (|| -> Result<(), Raise> {
        let v = load(body, 3)?;
        let none = Value::Array(vec![]);
        let mut rows: Vec<[String; 6]> = vec![];
        for n in py_iter(or(get_in(&v, "nodes", 17)?, &none), 17)? {
            let error = get_in(&n, "error", 18)?.is_some_and(py::truthy);
            let quiet = || -> Result<bool, Raise> {
                Ok(get_in(&n, "note", 18)?.is_some_and(py::truthy)
                    && !get_in(&n, "caches", 18)?.is_some_and(py::truthy))
            };
            if error || quiet()? {
                let what = match get_in(&n, "note", 19)? {
                    Some(x) if py::truthy(x) => x.clone(),
                    _ => get_in(&n, "error", 19)?.cloned().unwrap_or(Value::Null),
                };
                let node = py::str_of(index(&n, "node", 19)?);
                err.push_str(&format!("{tag} {node}: {}\n", py::str_of(&what)));
            }
            if clear {
                for e in py_iter(or(get_in(&n, "cleared", 20)?, &none), 20)? {
                    let node = py::str_of(index(&n, "node", 21)?);
                    let ws = ws_of(&e, "another:", 21)?;
                    let name = py::str_of(index(&e, "name", 21)?);
                    let sz = size(index(&e, "bytes", 21)?, 21)?;
                    out.push_str(&format!(
                        "cleared  {} {} {} {}\n",
                        ljust(&node, 14),
                        ljust(&ws, 20),
                        ljust(&name, 24),
                        rjust(&sz, 10)
                    ));
                }
            }
            for e in py_iter(or(get_in(&n, "caches", 22)?, &none), 22)? {
                let node = py::str_of(index(&n, "node", 23)?);
                let ws = ws_of(&e, "unknown", 23)?;
                let name = py::str_of(index(&e, "name", 23)?);
                let sz = size(index(&e, "bytes", 23)?, 23)?;
                let used = ago(get_in(&e, "usedAt", 23)?, now, 23)?;
                let busy = if get_in(&e, "busy", 23)?.is_some_and(py::truthy) {
                    "busy"
                } else {
                    ""
                };
                rows.push([node, ws, name, sz, used, busy.to_string()]);
            }
        }
        let line = |r: &[String; 6]| {
            format!(
                "{} {} {} {}  {} {}\n",
                ljust(&r[0], 14),
                ljust(&r[1], 20),
                ljust(&r[2], 24),
                rjust(&r[3], 10),
                ljust(&r[4], 9),
                r[5]
            )
        };
        if !rows.is_empty() && !clear {
            let head = ["NODE", "WORKSPACE", "CACHE", "SIZE", "USED", ""].map(String::from);
            out.push_str(&line(&head));
            for r in &rows {
                out.push_str(&line(r));
            }
        } else if clear {
            for r in rows.iter().filter(|r| r[5] == "busy") {
                err.push_str(&format!(
                    "{tag} {}: {}/{} is held by a running job; not cleared\n",
                    r[0], r[1], r[2]
                ));
            }
        } else {
            err.push_str(&format!("{tag} no job caches\n"));
        }
        Ok(())
    })();
    match r {
        Ok(()) => Shown {
            stdout: out,
            stderr: err,
            exit: Exit::Ok,
        },
        Err(e) => raised(out, err, &e),
    }
}

/// The `--lineage` reader.
pub fn lineage_reader(body: &[u8]) -> Shown {
    let mut out = String::new();
    let r = (|| -> Result<(), Raise> {
        let v = load(body, 3)?;
        let none = Value::Array(vec![]);
        let nodes = or(get_in(&v, "nodes", 4)?, &none).clone();
        if !py::truthy(&nodes) {
            let ws = get_in(&v, "workspace", 6)?.map_or_else(|| "None".into(), py::str_of);
            out.push_str(&format!(
                "no cache generations of {ws} on any connected node (a node keeps them once its owner turns them on: sudo k2-node admin cache-gens on)\n"
            ));
        }
        for n in py_iter(&nodes, 7)? {
            out.push_str(&format!("{}:\n", py::str_of(index(&n, "node", 8)?)));
            for l in py_iter(index(&n, "lineages", 9)?, 9)? {
                let zero = Value::Int("0".into());
                let age = or(get_in(&l, "ageSecs", 10)?, &zero).clone();
                let a = num(&age, 11, "int")?;
                let unit = |d: f64| -> Result<String, Raise> {
                    let q = super::super::writes::int_of(&Value::Float((a / d).floor()))
                        .map_err(|e| Raise::at(11, e))?;
                    Ok(q)
                };
                let ago = if a < 7200.0 {
                    format!("{}m", unit(60.0)?)
                } else if a < 172_800.0 {
                    format!("{}h", unit(3600.0)?)
                } else {
                    format!("{}d", unit(86400.0)?)
                };
                let lineage = py::str_of(index(&l, "lineage", 12)?);
                let seq = py::str_of(index(&l, "headSeq", 12)?);
                let commit = head_of(index(&l, "headCommit", 12)?, 10, 12)?;
                let gb = num(index(&l, "bytes", 12)?, 12, "float")? / 1_073_741_824.0;
                let forks = match get_in(&l, "forks", 12)? {
                    Some(f) if py::truthy(f) => format!(
                        "  ({} forks running)",
                        py::percent_d(f).map_err(|e| Raise::at(12, e))?
                    ),
                    _ => String::new(),
                };
                out.push_str(&format!(
                    "  {} #{} {commit}  {:>6} GB  used {ago} ago{forks}\n",
                    ljust(&lineage, 28),
                    ljust(&seq, 4),
                    format!("{gb:.1}")
                ));
            }
            if get_in(&n, "truncated", 13)?.is_some_and(py::truthy) {
                out.push_str("  (more lineages than the node reports)\n");
            }
        }
        Ok(())
    })();
    match r {
        Ok(()) => Shown::out(out),
        Err(e) => raised(out, String::new(), &e),
    }
}
