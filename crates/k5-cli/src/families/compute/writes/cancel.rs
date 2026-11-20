//! `k5 compute cancel`, as `cmd_compute`'s `cancel)` arm answers it
//! (`cli/src/compute-command.sh`):
//!
//! - `cancel <job> [--force|-f]`: one POST to `/cli/compute/jobs/cancel`
//!   with `{"job": …, "ifState": "queued"}` (no `ifState` with `--force`),
//!   then what happened to the job, one line.
//! - `cancel <job> --if-queued`: the same with `ifState` queued, where a
//!   409 whose body names `job_started` or `already_ended` is "not
//!   cancelled", exit 0.
//! - `cancel --all-mine|--workspace-all [--tag T] [--state S] [--node N]
//!   [--workspace WS] [--force]`: one POST with the filter, then the count
//!   and each job skipped.
//!
//! `--json` anywhere prints the answer as it came. The words are read as
//! the shell's loop reads them ([`parse`]); a flag that takes a value and
//! comes last ends the command with 1 and nothing said (`shift 2` under
//! `set -e`).

use super::{
    body_of, divmod, dumps, head, int_of, is_str, load, object, or_str, print_body, stopped,
    str_at, str_value, subscript, Cp, Words,
};
use crate::exit::Exit;
use crate::families::{get, py_get, py_iter, py_len, Raise, Shown};
use crate::out::json::Value;
use crate::out::py;

/// The route.
pub const ENDPOINT: &str = "/cli/compute/jobs/cancel";
/// `$cusage`.
pub const USAGE: &str = concat!(
    "usage: ",
    k2_client::cli_name!(),
    " compute cancel <job> [--force|--if-queued]  |  ",
    k2_client::cli_name!(),
    " compute cancel --all-mine|--workspace-all [--tag T] [--state queued|running|any] [--node N] [--workspace WS] [--force]"
);

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    /// The filter's reader (`--all-mine`, `--workspace-all`).
    pub mod many {
        pub const LOAD: u32 = 3;
        pub const C: u32 = 4;
        pub const PRINT: u32 = 5;
        pub const FOR: u32 = 6;
        pub const SKIPPED: u32 = 7;
    }
    /// One job's reader.
    pub mod one {
        pub const LOAD: u32 = 3;
        pub const NODE: u32 = 5;
        pub const INT: u32 = 8;
        pub const STATE: u32 = 10;
        pub const STOPPING: u32 = 15;
    }
    /// `--if-queued`'s reader of a 409.
    pub mod kept {
        pub const LOAD: u32 = 3;
        pub const CODE: u32 = 5;
    }
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub job: String,
    pub force: bool,
    pub if_queued: bool,
    /// `session` (`--all-mine`) or `workspace` (`--workspace-all`).
    pub scope: Option<&'static str>,
    pub tag: String,
    pub state: String,
    pub node: String,
    pub workspace: String,
}

fn usage(why: &str) -> Words<Ask> {
    Words::Usage(if why.is_empty() {
        USAGE.to_string()
    } else {
        format!("{USAGE} ({why})")
    })
}

/// The words after `cancel`, as the shell's loop and the checks after it
/// read them.
pub fn parse(words: &[&str]) -> Words<Ask> {
    let mut a = Ask::default();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let value = || words.get(i + 1).copied().unwrap_or("").to_string();
        match w {
            "--force" | "-f" => a.force = true,
            "--if-queued" => a.if_queued = true,
            "--all-mine" => a.scope = Some("session"),
            "--workspace-all" => a.scope = Some("workspace"),
            "--tag" | "--state" | "--node" | "--workspace" => {
                let v = value();
                match w {
                    "--tag" => a.tag = v,
                    "--state" => a.state = v,
                    "--node" => a.node = v,
                    _ => a.workspace = v,
                }
                if i + 2 > words.len() {
                    return Words::ShiftFailed;
                }
                i += 2;
                continue;
            }
            "--json" => {}
            w if w.starts_with('-') => return usage(&format!("unknown flag '{w}'")),
            w => {
                if !a.job.is_empty() {
                    return usage("one job");
                }
                a.job = w.to_string();
            }
        }
        i += 1;
    }
    if a.scope.is_some() {
        if !a.job.is_empty() {
            return usage("a job id or --all-mine, not both");
        }
        if !matches!(state_or_queued(&a.state), "queued" | "running" | "any") {
            return usage("--state is queued, running or any");
        }
        return Words::Ask(a);
    }
    if a.job.is_empty() {
        return usage("");
    }
    if a.if_queued && a.force {
        return usage("--if-queued or --force, not both");
    }
    Words::Ask(a)
}

fn state_or_queued(state: &str) -> &str {
    if state.is_empty() {
        "queued"
    } else {
        state
    }
}

/// The requests and what they print.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    match a.scope {
        Some(scope) => many(cp, a, scope),
        None => one(cp, a),
    }
    .unwrap_or_else(|e| e)
}

/// The filter's body: `{"filter": {...}, "force": …}`.
pub fn filter_body(a: &Ask, scope: &str, workspace: &str) -> String {
    let mut filter = vec![("scope", str_value(scope))];
    for (k, v) in [
        ("tag", a.tag.as_str()),
        ("state", a.state.as_str()),
        ("node", a.node.as_str()),
        ("workspace", workspace),
    ] {
        if !v.is_empty() {
            filter.push((k, str_value(v)));
        }
    }
    dumps(&object(vec![
        ("filter", object(filter)),
        ("force", Value::Bool(a.force)),
    ]))
}

fn many(cp: &mut Cp<'_>, a: &Ask, scope: &str) -> Result<Exit, Exit> {
    let state = state_or_queued(&a.state);
    if state != "queued" && !a.force {
        crate::out::stderr_line(&format!(
            concat!(
                "[",
                k2_client::cli_name!(),
                " compute] --state {} stops started jobs only with --force; without it they are listed as skipped"
            ),
            a.state
        ));
    }
    let ws = cp.ws(&a.workspace);
    let body = cp.call(true, ENDPOINT, &filter_body(a, scope, &ws), false)?;
    if cp.as_json {
        return Ok(print_body(&body));
    }
    Ok(many_reader(&body).print())
}

/// The filter's reader on the answer.
pub fn many_reader(body: &[u8]) -> Shown {
    use line::many as l;
    let mut out = String::new();
    let r = (|| -> Result<(), Raise> {
        let v = load(body, l::LOAD)?;
        let c = match py_get(&v, "cancelled", l::C)? {
            Some(x) if py::truthy(x) => x.clone(),
            _ => Value::Array(Vec::new()),
        };
        let n = py_len(&c, l::PRINT)?;
        let matched = py_get(&v, "matched", l::PRINT)?
            .cloned()
            .unwrap_or_else(|| Value::Int("0".into()));
        let tail = if py::truthy(&c) {
            let items = py_iter(&c, l::PRINT)?;
            let mut jobs = Vec::new();
            for x in &items {
                jobs.push(
                    subscript(x, "job")
                        .map_err(|e| Raise::inside(l::PRINT, l::PRINT, "<genexpr>", e))?,
                );
            }
            let mut words = Vec::new();
            for (i, j) in jobs.iter().enumerate() {
                match j {
                    Value::Str(_) => words.push(py::str_of(j)),
                    other => {
                        return Err(Raise::at(
                            l::PRINT,
                            format!(
                                "TypeError: sequence item {i}: expected str instance, {} found",
                                py::type_name(other)
                            ),
                        ))
                    }
                }
            }
            format!(": {}", words.join(" "))
        } else {
            String::new()
        };
        let m = py::percent_d(&matched).map_err(|e| Raise::at(l::PRINT, e))?;
        out.push_str(&format!("cancelled {n} of {m} matching job(s){tail}\n"));
        let skipped = match py_get(&v, "skipped", l::FOR)? {
            Some(x) if py::truthy(x) => x.clone(),
            _ => Value::Array(Vec::new()),
        };
        for s in py_iter(&skipped, l::FOR)? {
            let Value::Object(pairs) = &s else {
                return Err(Raise::no_attribute(l::SKIPPED, &s, "get"));
            };
            out.push_str(&format!(
                "  skipped {}: {} ({})\n",
                str_at(get(pairs, "job")),
                str_at(get(pairs, "code")),
                str_at(get(pairs, "hint"))
            ));
        }
        Ok(())
    })();
    match r {
        Ok(()) => Shown::out(out),
        Err(e) => stopped(out, &e),
    }
}

fn one(cp: &mut Cp<'_>, a: &Ask) -> Result<Exit, Exit> {
    if a.if_queued {
        let s = cp.send(
            true,
            ENDPOINT,
            &body_of(&[("job", &a.job), ("ifState", "queued")]),
            false,
        )?;
        if s.status == "409" && names_started_or_ended(&s.body) {
            if cp.as_json {
                return Ok(print_body(&s.body));
            }
            return Ok(kept_reader(&s.body, &a.job).print());
        }
        if s.status != "200" {
            return Err(super::fail(&s.status, &s.body));
        }
        if cp.as_json {
            return Ok(print_body(&s.body));
        }
        return Ok(one_reader(&s.body, &a.job).print());
    }
    let if_state = if a.force { "" } else { "queued" };
    let body = cp.call(
        true,
        ENDPOINT,
        &body_of(&[("job", &a.job), ("ifState", if_state)]),
        false,
    )?;
    if cp.as_json {
        return Ok(print_body(&body));
    }
    Ok(one_reader(&body, &a.job).print())
}

/// `grep -Eq '"code":"(job_started|already_ended)"'` on the body.
pub fn names_started_or_ended(body: &[u8]) -> bool {
    [
        &b"\"code\":\"job_started\""[..],
        b"\"code\":\"already_ended\"",
    ]
    .iter()
    .any(|n| body.windows(n.len()).any(|w| w == *n))
}

/// `--if-queued`'s reader of a 409 that names a started or ended job.
pub fn kept_reader(body: &[u8], job: &str) -> Shown {
    use line::kept as l;
    let r = (|| -> Result<String, Raise> {
        let v = load(body, l::LOAD)?;
        let e = match py_get(&v, "error", l::LOAD)? {
            Some(x) if py::truthy(x) => x.clone(),
            _ => Value::Object(Vec::new()),
        };
        let Value::Object(pairs) = &e else {
            return Err(Raise::no_attribute(l::CODE, &e, "get"));
        };
        let job = head(job, 8);
        Ok(if is_str(get(pairs, "code"), "already_ended") {
            format!(
                "not cancelled: {job} already ended ({})\n",
                str_at(get(pairs, "hint"))
            )
        } else {
            format!(
                concat!(
                    "not cancelled: {job} already started; left running (",
                    k2_client::cli_name!(),
                    " compute cancel {job} --force stops it)\n"
                ),
                job = job
            )
        })
    })();
    match r {
        Ok(text) => Shown::out(text),
        Err(e) => stopped(String::new(), &e),
    }
}

/// One job's reader on the answer.
pub fn one_reader(body: &[u8], job: &str) -> Shown {
    use line::one as l;
    let r = (|| -> Result<String, Raise> {
        let v = load(body, l::LOAD)?;
        let Value::Object(pairs) = &v else {
            return Err(Raise::no_attribute(l::NODE, &v, "get"));
        };
        let job = head(job, 8);
        let node = or_str(get(pairs, "node"), "?");
        let state = get(pairs, "state");
        if is_str(state, "cancelled") && is_str(get(pairs, "was"), "queued") {
            return Ok(format!("cancelled {job} (was queued on {node})\n"));
        }
        if is_str(state, "cancel_requested") && get(pairs, "onNode").is_some_and(py::truthy) {
            return Ok(format!(
                "cancel asked for {job} (queued on {node}; {}): {}\n",
                str_at(get(pairs, "cancel")),
                or_str(get(pairs, "note"), "")
            ));
        }
        if let Some(cancel) = get(pairs, "cancel") {
            let was = match get(pairs, "was") {
                Some(w) if py::truthy(w) => py::str_of(w),
                _ => str_at(state),
            };
            let took = match get(pairs, "ranSecs") {
                None | Some(Value::Null) => String::new(),
                Some(s) => {
                    let n = int_of(s).map_err(|e| Raise::inside(l::STOPPING, l::INT, "took", e))?;
                    let (m, s) = divmod(&n, 60);
                    format!(" for {m}m{s:02}s")
                }
            };
            return Ok(format!(
                "stopping {job} (was {was} on {node}{took}; cancel {})\n",
                py::str_of(cancel)
            ));
        }
        Ok(format!(
            "{}\n",
            state.map_or_else(|| "ok".to_string(), py::str_of)
        ))
    })();
    match r {
        Ok(text) => Shown::out(text),
        Err(e) => stopped(String::new(), &e),
    }
}
