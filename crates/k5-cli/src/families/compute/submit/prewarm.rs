//! `compute prewarm [--preset NAME | --needs …] [--os …] [--distro …]
//! [--arch …] [--label k=v] [--node N]… [--tag T] [--plan] [--wait]
//! [--workspace WS] [--json]`, as `cmd_compute_prewarm` answers it
//! (`cli/src/compute-run.sh`): one POST of `/cli/compute/prewarm`, then
//! the answer as it came (`--json`) or a line per node on stderr; with
//! `--wait`, the jobs it started waited for (`cmd_compute_wait`); a
//! package request held for the owner is its lines and exit 2.

use super::super::more::packages;
use super::super::more::py_line_q;
use super::super::pyrt::{self as p, s};
use super::super::render::{self, Cx};
use super::super::writes::{dumps, object, print_body, str_value, wait, Cp, Words};
use super::run::{pairs, split_needs};
use super::{distro, SRC};
use crate::exit::Exit;
use crate::families::{raised, Raise};
use crate::out;
use crate::out::json::Value;
use k2_client::{brand::CLI_NAME, cli_name};

pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute prewarm [--preset NAME | --needs rust@stable,cmake@3] [--os linux|macos] [--distro ubuntu@24.04] [--arch A] [--label k=v] [--node N]... [--tag T] [--plan] [--wait] [--workspace WS] [--json] (unknown '%s')"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ask {
    pub preset: String,
    pub os: String,
    pub distro: String,
    pub arch: String,
    pub tag: String,
    pub workspace: String,
    pub plan: bool,
    pub wait: bool,
    pub as_json: bool,
    pub needs: Option<Value>,
    pub nodes: Option<Value>,
    pub labels: Option<Value>,
}

/// The words after `prewarm`. `None`: the shell's.
pub fn parse(words: &[&str]) -> Option<Words<Ask>> {
    let mut a = Ask::default();
    let (mut needs, mut nodes, mut labels) = (vec![], vec![], vec![]);
    let mut i = 0;
    let n = words.len();
    while i < n {
        let w = words[i];
        let v = words.get(i + 1).copied().unwrap_or("").to_string();
        match w {
            "--preset" => a.preset = v,
            "--needs" | "--need" => needs.push(v),
            "--os" => a.os = v,
            "--distro" => {
                a.distro = match distro(&v)? {
                    Ok(d) => d,
                    Err(stop) => return Some(stop),
                }
            }
            "--arch" => a.arch = v,
            "--label" => labels.push(v),
            "--node" => nodes.push(v),
            "--tag" => a.tag = v,
            "--workspace" => a.workspace = v,
            "--plan" | "--dry-run" | "--wait" | "--json" => {
                match w {
                    "--wait" => a.wait = true,
                    "--json" => a.as_json = true,
                    _ => a.plan = true,
                }
                i += 1;
                continue;
            }
            other => return Some(Words::Usage(USAGE.replacen("%s", other, 1))),
        }
        if i + 2 > n {
            return Some(Words::ShiftFailed(words[i].to_string()));
        }
        i += 2;
    }
    if a.plan && a.wait {
        return Some(Words::Usage(
            "usage: --plan submits nothing, so there is nothing to --wait for".into(),
        ));
    }
    if !needs.is_empty() {
        a.needs = Some(split_needs(&needs));
    }
    if !nodes.is_empty() {
        a.nodes = Some(Value::Array(nodes.iter().map(|x| str_value(x)).collect()));
    }
    if !labels.is_empty() {
        a.labels = Some(match pairs(&labels, "usage: --label key=value") {
            Ok(v) => v,
            Err(line) => return Some(Words::Usage(line.into())),
        });
    }
    // `cmd_compute_wait` makes its folder with `mktemp -d`.
    if a.wait && !wait::temp_usable() {
        return None;
    }
    Some(Words::Ask(a))
}

/// The body, as `_compute_json` writes it.
pub fn body(a: &Ask, repo_dir: &str, ws: &str) -> String {
    let st = |x: &str| (!x.is_empty()).then(|| str_value(x));
    let fields: Vec<(&str, Option<Value>)> = vec![
        ("preset", st(&a.preset)),
        ("needs", a.needs.clone()),
        ("os", st(&a.os)),
        ("distro", st(&a.distro)),
        ("arch", st(&a.arch)),
        ("labels", a.labels.clone()),
        ("nodes", a.nodes.clone()),
        ("tag", st(&a.tag)),
        ("plan", a.plan.then_some(Value::Bool(true))),
        ("repoDir", st(repo_dir)),
        ("workspace", st(ws)),
    ];
    dumps(&object(
        fields
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| (k, v)))
            .collect(),
    ))
}

/// The request and what follows it.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let repo_dir = if crate::daemon::usable_git().is_some() {
        super::git_rev_parse("--show-toplevel")
    } else {
        String::new()
    };
    let ws = cp.ws(&a.workspace);
    let body = body(a, &repo_dir, &ws);
    let answer = match cp.call(true, "/cli/compute/prewarm", &body, false) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if a.as_json && !a.wait {
        return print_body(&answer);
    }
    let ids = match ids(&answer) {
        Ok(ids) => ids,
        Err(e) => return e,
    };
    if a.wait && !ids.is_empty() {
        // `read -r -a wargs <<< "$ids"`: the first line's words.
        let first = ids.split('\n').next().unwrap_or("");
        let mut jobs: Vec<String> = first
            .split([' ', '\t'])
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect();
        if a.as_json {
            jobs.push("--json".into());
        }
        let refs: Vec<&str> = jobs.iter().map(String::as_str).collect();
        return match wait::parse(&refs) {
            Some(Words::Ask(w)) => wait::send(cp, &w),
            Some(Words::Usage(line)) => {
                out::stderr_line(&line);
                Exit::Usage
            }
            Some(Words::ShiftFailed(flag)) => out::stop_need_value(&flag),
            // A job id `--follow` or `--failures` from the daemon.
            None => {
                out::stderr_line(wait::USAGE);
                Exit::Usage
            }
        };
    }
    if a.as_json {
        let r = print_body(&answer);
        if r != Exit::Ok {
            return r;
        }
    }
    packages::held_exit(&answer).unwrap_or(Exit::Ok)
}

macro_rules! line {
    ($m:literal) => {
        const { py_line_q(SRC, "    local ids\n", $m) }
    };
}

/// The reader of the answer: a line per node (and the fit and the package
/// request) on stderr, then the jobs it started (`$(…)`).
pub fn ids(body: &[u8]) -> Result<String, Exit> {
    let cx = Cx::new(0);
    let mut err = String::new();
    let r = read(&cx, body, &mut err);
    let _ = out::write_flushed(&mut std::io::stderr().lock(), err.as_bytes());
    match r {
        Ok(text) => Ok(text.trim_end_matches('\n').to_string()),
        Err(e) => Err(raised(String::new(), String::new(), &e).print()),
    }
}

fn joined(cx: &Cx, l: u32, r: &Value, key: &str) -> Result<Value, Raise> {
    let v = p::or(cx.at(l, p::get(r, key))?, Value::Array(vec![]));
    Ok(s(&cx.at(l, p::join(", ", &v))?))
}

fn read(cx: &Cx, body: &[u8], err: &mut String) -> Result<String, Raise> {
    let v = render::load(cx, body, line!("v = json.load"))?;
    let l = line!("for r in v.get(\"nodes\")");
    let rows = p::or(cx.at(l, p::get(&v, "nodes"))?, Value::Array(vec![]));
    for r in cx.at(l, p::iter(&rows))? {
        let l = line!("st = r.get(\"state\")");
        let st = cx.at(l, p::get(&r, "state"))?;
        let mut text = if p::eq_s(&st, "ready") {
            s("ready already")
        } else if p::eq_s(&st, "installing") {
            let l = line!("s = \"installing %s (job %s)\"");
            let installs = joined(cx, l, &r, "installs")?;
            let short = cx.at(l, p::get(&r, "short"))?;
            s(&cx.at(l, p::pct("installing %s (job %s)", &[installs, short]))?)
        } else if p::eq_s(&st, "would_install") {
            let l = line!("s = \"would install %s\"");
            let installs = joined(cx, l, &r, "installs")?;
            s(&cx.at(l, p::pct("would install %s", &[installs]))?)
        } else if p::eq_s(&st, "asked") || p::eq_s(&st, "would_ask") {
            let l = line!("s = \"%s its owner for %s\"");
            let who = if p::eq_s(&st, "asked") {
                s("asked")
            } else {
                s("would ask")
            };
            let ask = joined(cx, l, &r, "ask")?;
            s(&cx.at(l, p::pct("%s its owner for %s", &[who, ask]))?)
        } else if p::eq_s(&st, "cant_install") {
            s("cannot install there")
        } else {
            let l = line!("e = r.get(\"error\") or {}");
            let e = p::or(cx.at(l, p::get(&r, "error"))?, Value::Object(vec![]));
            let l = line!("s = \"refused: %s: %s\"");
            let code = cx.at(l, p::get(&e, "code"))?;
            let hint = cx.at(l, p::get(&e, "hint"))?;
            s(&cx.at(l, p::pct("refused: %s: %s", &[code, hint]))?)
        };
        let l = line!("if r.get(\"note\"): s +=");
        if p::truthy(&cx.at(l, p::get(&r, "note"))?) {
            let note = cx.at(l, p::idx(&r, "note"))?;
            let more = cx.at(l, super::run::plus(" \u{b7} ", &note))?;
            text = cx.at(l, p::iadd(&text, &s(&more)))?;
        }
        let l = line!("sys.stderr.write(\"[k2 compute] %s: %s\\n\" % (r.get(\"node\")");
        let node = cx.at(l, p::get(&r, "node"))?;
        err.push_str(&cx.at(
            l,
            p::pct(&format!("[{CLI_NAME} compute] %s: %s\n"), &[node, text]),
        )?);
    }
    let l = line!("if v.get(\"fits\")");
    if p::truthy(&cx.at(l, p::get(&v, "fits"))?) {
        let fits = cx.at(l, p::idx(&v, "fits"))?;
        err.push_str(&cx.at(l, p::pct(&format!("[{CLI_NAME} compute] %s\n"), &[fits]))?);
    }
    let l = line!("rq = v.get(\"request\")");
    let rq = cx.at(l, p::get(&v, "request"))?;
    let l = line!("if isinstance(rq, dict) and rq.get(\"id\")");
    if p::is_dict(&rq) && p::truthy(&cx.at(l, p::get(&rq, "id"))?) {
        let l = line!("package request %s: %s.");
        let id = cx.at(l, p::idx(&rq, "id"))?;
        let summary = p::or(cx.at(l, p::get(&rq, "summary"))?, s(""));
        err.push_str(&cx.at(
            l,
            p::pct(
                &format!(
                    "[{CLI_NAME} compute] package request %s: %s. Prewarm again once it is installed ({CLI_NAME} compute node packages requests)\n"
                ),
                &[id, summary],
            ),
        )?);
    } else {
        let l = line!("elif isinstance(rq, dict) and isinstance(");
        if p::is_dict(&rq) && p::is_dict(&cx.at(l, p::get(&rq, "error"))?) {
            let l = line!("package request not filed");
            let e = cx.at(l, p::idx(&rq, "error"))?;
            let code = cx.at(l, p::get(&e, "code"))?;
            let e = cx.at(l, p::idx(&rq, "error"))?;
            let hint = cx.at(l, p::get(&e, "hint"))?;
            err.push_str(&cx.at(
                l,
                p::pct(
                    &format!("[{CLI_NAME} compute] package request not filed: %s: %s\n"),
                    &[code, hint],
                ),
            )?);
        }
    }
    let l = line!("print(\" \".join(v.get(\"jobs\")");
    let jobs = p::or(cx.at(l, p::get(&v, "jobs"))?, Value::Array(vec![]));
    Ok(cx.at(l, p::join(" ", &jobs))? + "\n")
}
