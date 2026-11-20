//! `k5 compute move <job>... --auto [--avoid <node>]... [--os O|any]
//! [--arch A|any] [--label k=v] | --to <node>`, and `move
//! --all-mine|--workspace-all [--tag T] [--node N] [--workspace WS]`, as
//! `cmd_compute_move` answers it (`cli/src/compute-access.sh`): one POST
//! to `/cli/compute/jobs/move` per job (or one for the filter), then each
//! move in a line (or the answer as it came, `--json`).
//!
//! Several jobs: the shell means each to be tried and the worst exit to
//! win, but each runs in a `( … )` under `set -e`, so the first that
//! fails ends the command with its exit and the rest are not sent. The
//! binary does the same (a bash bug, reported).

use super::super::writes::{dumps, load, object, print_body, str_value, Cp, Words};
use crate::exit::Exit;
use crate::families::{py_d, py_iter, py_len, raised, Raise, Shown};
use crate::out::json::{Text, Value};
use crate::out::py;
use crate::out::pylib::ops::{contains, get_in, index, join, or};
use k2_client::cli_name;

pub const ENDPOINT: &str = "/cli/compute/jobs/move";

pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute move <job>... --auto [--avoid <node>]... [--os O|any] [--arch A|any] [--label k=v]  |  ",
    cli_name!(),
    " compute move <job>... --to <node>  |  ",
    cli_name!(),
    " compute move --all-mine|--workspace-all [--tag T] [--node N] [--workspace WS] (--auto … | --to <node>)"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub jobs: Vec<String>,
    pub auto: bool,
    pub to: String,
    pub avoids: Vec<String>,
    pub os: String,
    pub arch: String,
    /// `--label`'s pairs, as `json.dumps` of the dict writes them.
    pub labels: Vec<(String, String)>,
    /// `session` (`--all-mine`) or `workspace` (`--workspace-all`).
    pub scope: String,
    pub tag: String,
    pub node: String,
    pub workspace: String,
    pub json: bool,
}

fn usage(why: &str) -> Words<Ask> {
    Words::Usage(format!("{USAGE} ({why})"))
}

/// The words after `move`.
pub fn parse(words: &[&str]) -> Words<Ask> {
    let mut a = Ask::default();
    let mut labels: Vec<String> = vec![];
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let v = words.get(i + 1).copied().unwrap_or("").to_string();
        let short = i + 2 > words.len();
        let takes = matches!(
            w,
            "--to" | "--avoid" | "--os" | "--arch" | "--label" | "--tag" | "--node" | "--workspace"
        );
        if takes {
            match w {
                "--to" => a.to = v.clone(),
                "--avoid" => a.avoids.push(v.clone()),
                "--os" => a.os = v,
                "--arch" => a.arch = v,
                "--label" => labels.push(v),
                "--tag" => a.tag = v,
                "--node" => a.node = v,
                _ => a.workspace = v,
            }
            if short {
                return Words::ShiftFailed;
            }
            if w == "--to" && a.to.is_empty() {
                return usage("--to needs a node");
            }
            if w == "--avoid" && a.avoids.last().is_some_and(String::is_empty) {
                return usage("--avoid needs a node");
            }
            i += 2;
            continue;
        }
        match w {
            "--auto" => a.auto = true,
            "--all-mine" => a.scope = "session".into(),
            "--workspace-all" => a.scope = "workspace".into(),
            "--json" => a.json = true,
            w if w.starts_with('-') => return usage(&format!("unknown flag '{w}'")),
            w => a.jobs.push(w.to_string()),
        }
        i += 1;
    }
    if a.auto && !a.to.is_empty() {
        return usage("--auto or --to, not both");
    }
    if !a.auto && a.to.is_empty() {
        return usage("say where: --auto or --to <node>");
    }
    if !a.scope.is_empty() && !a.jobs.is_empty() {
        return usage("job ids or --all-mine, not both");
    }
    if a.scope.is_empty() && a.jobs.is_empty() {
        return usage("name a job, or --all-mine");
    }
    if a.scope.is_empty() && (!a.tag.is_empty() || !a.node.is_empty()) {
        return usage("--tag and --node pick jobs with --all-mine or --workspace-all");
    }
    for l in labels {
        let Some((k, v)) = l.split_once('=') else {
            return Words::Usage("usage: --label key=value".into());
        };
        match a.labels.iter_mut().find(|(have, _)| have == k) {
            Some(pair) => pair.1 = v.to_string(),
            None => a.labels.push((k.to_string(), v.to_string())),
        }
    }
    Words::Ask(a)
}

/// The body: `_compute_json` with the job (or the filter) first.
fn body(a: &Ask, first: (&str, Value)) -> String {
    let mut pairs = vec![first];
    if a.auto {
        pairs.push(("auto", Value::Bool(true)));
    }
    if !a.to.is_empty() {
        pairs.push(("to", str_value(&a.to)));
    }
    if !a.avoids.is_empty() {
        pairs.push((
            "avoid",
            Value::Array(a.avoids.iter().map(|s| str_value(s)).collect()),
        ));
    }
    if !a.os.is_empty() {
        pairs.push(("os", str_value(&a.os)));
    }
    if !a.arch.is_empty() {
        pairs.push(("arch", str_value(&a.arch)));
    }
    if !a.labels.is_empty() {
        pairs.push((
            "labels",
            Value::Object(
                a.labels
                    .iter()
                    .map(|(k, v)| (Text::from(k.as_str()), str_value(v)))
                    .collect(),
            ),
        ));
    }
    dumps(&object(pairs))
}

/// The requests and what they print.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    if !a.scope.is_empty() {
        let ws = cp.ws(&a.workspace);
        let mut f = vec![("scope", str_value(&a.scope))];
        for (k, v) in [("tag", &a.tag), ("node", &a.node), ("workspace", &ws)] {
            if !v.is_empty() {
                f.push((k, str_value(v)));
            }
        }
        let b = body(a, ("filter", object(f)));
        return match cp.call(true, ENDPOINT, &b, false) {
            Ok(answer) if a.json => print_body(&answer),
            Ok(answer) => reader(&answer).print(),
            Err(e) => e,
        };
    }
    for job in &a.jobs {
        let b = body(a, ("job", str_value(job)));
        let exit = match cp.call(true, ENDPOINT, &b, false) {
            Ok(answer) if a.json => print_body(&answer),
            Ok(answer) => reader(&answer).print(),
            Err(e) => e,
        };
        if exit != Exit::Ok {
            return exit;
        }
    }
    Exit::Ok
}

/// `one(r)`, called from the module's line `from`.
fn one(r: &Value, from: u32) -> Result<String, Raise> {
    let inside = |line: u32| move |e: Raise| Raise::inside(from, line, "one", e.exception);
    let empty = Value::Object(vec![]);
    let f = or(get_in(r, "filter", 5).map_err(inside(5))?, &empty).clone();
    let os = get_in(&f, "os", 7).map_err(inside(7))?.cloned();
    let arch_ = get_in(&f, "arch", 7).map_err(inside(7))?.cloned();
    let mut extra = vec![];
    let truthy = |x: &Option<Value>| x.as_ref().is_some_and(py::truthy);
    if truthy(&os) || truthy(&arch_) {
        let any = str_value("any");
        extra.push(format!(
            "{}-{}",
            py::str_of(or(os.as_ref(), &any)),
            py::str_of(or(arch_.as_ref(), &any))
        ));
    }
    if get_in(&f, "avoid", 9).map_err(inside(9))?.is_some_and(py::truthy) {
        let avoid = index(&f, "avoid", 10).map_err(inside(10))?;
        extra.push(format!(
            "avoids {}",
            join(avoid, ", ", 10).map_err(inside(10))?
        ));
    }
    let tail = if extra.is_empty() {
        String::new()
    } else {
        format!(" ({})", extra.join("; "))
    };
    let note = |line: u32| -> Result<String, Raise> {
        let empty = str_value("");
        Ok(py::str_of(or(
            get_in(r, "note", line).map_err(inside(line))?,
            &empty,
        )))
    };
    if get_in(r, "moved", 12).map_err(inside(12))?.is_some_and(py::truthy) {
        let s = |k: &str| index(r, k, 13).map(py::str_of).map_err(inside(13));
        let (job, from_, node, mode) = (s("job")?, s("from")?, s("node")?, s("mode")?);
        return Ok(format!(
            "job {job}: moved {from_} -> {node} ({mode}){tail}. {}",
            note(13)?
        ));
    }
    let mode = get_in(r, "mode", 14).map_err(inside(14))?;
    if mode.is_some_and(|m| matches!(m, Value::Str(t) if *t == Text::from("auto"))) {
        let job = py::str_of(index(r, "job", 15).map_err(inside(15))?);
        return Ok(format!("job {job}: auto-placed now{tail}. {}", note(15)?));
    }
    let job = py::str_of(index(r, "job", 16).map_err(inside(16))?);
    let n = get_in(r, "note", 16).map_err(inside(16))?;
    let text = match n {
        Some(x) if py::truthy(x) => py::str_of(x),
        _ => {
            let node = get_in(r, "node", 16).map_err(inside(16))?;
            format!("pinned to {}", node.map_or_else(|| "None".into(), py::str_of))
        }
    };
    Ok(format!("job {job}: {text}"))
}

/// The reader on an answer.
pub fn reader(body: &[u8]) -> Shown {
    let (mut out, mut err) = (String::new(), String::new());
    let r = (|| -> Result<(), Raise> {
        let v = load(body, 3)?;
        let none = Value::Array(vec![]);
        let warnings = or(get_in(&v, "warnings", 18)?, &none).clone();
        for w in py_iter(&warnings, 18)? {
            err.push_str(&format!(
                "[{} compute] note: {}\n",
                cli_name!(),
                py::str_of(&w)
            ));
        }
        if contains(&v, "matched", 20)? {
            let m = or(get_in(&v, "moved", 21)?, &none).clone();
            let zero = Value::Int("0".into());
            let n = py_len(&m, 22)?;
            let matched = get_in(&v, "matched", 22)?.unwrap_or(&zero);
            out.push_str(&format!(
                "moved {n} of {} matching queued job(s)\n",
                py_d(matched, 22)?
            ));
            for r in py_iter(&m, 23)? {
                out.push_str(&format!("  {}\n", one(&r, 24)?));
            }
            let skipped = or(get_in(&v, "skipped", 25)?, &none).clone();
            for s in py_iter(&skipped, 25)? {
                let g = |k: &str| -> Result<String, Raise> {
                    Ok(get_in(&s, k, 26)?.map_or_else(|| "None".into(), py::str_of))
                };
                out.push_str(&format!(
                    "  skipped {}: {} ({})\n",
                    g("job")?,
                    g("code")?,
                    g("hint")?
                ));
            }
        } else {
            out.push_str(&one(&v, 28)?);
            out.push('\n');
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
