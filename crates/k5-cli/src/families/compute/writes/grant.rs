//! `k5 compute grant <node> --workspace WS [--max-time 2h] [--max-disk
//! 60G] [--max-parallel 1] [--max-queued 10] [--submitter-max-running N]
//! [--ci|--no-ci] [--gpu|--no-gpu] [--userns|--no-userns]
//! [--no-addr-filter|--addr-filter]`, as `cmd_compute_grant` answers it
//! (`cli/src/compute-access.sh`): one POST to `/cli/compute/grants/set`
//! over TCP, then the grant in one line (and the queued jobs sent again
//! with the new caps). A refusal's permission card (`compute.grant`) is
//! the closing line, as for every compute call.

use super::{dumps, load, object, stopped, str_value, Cp, Words};
use crate::exit::Exit;
use crate::families::{get, Raise, Shown};
use crate::out::json::{self, Value};
use crate::out::py;

/// The route.
pub const ENDPOINT: &str = "/cli/compute/grants/set";
/// The usage line.
pub const USAGE: &str = concat!(
    "usage: ",
    k2_client::cli_name!(),
    " compute grant <node> --workspace WS [--max-time 2h] [--max-disk 60G] [--max-parallel 1] [--max-queued 10] [--submitter-max-running N] [--ci|--no-ci] [--gpu|--no-gpu] [--userns|--no-userns] [--no-addr-filter|--addr-filter]"
);

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    pub const LOAD: u32 = 3;
    pub const GRANT: u32 = 4;
    pub const SUB: u32 = 5;
    pub const PRINT: u32 = 6;
    pub const RESENT_PRINT: u32 = 8;
    /// `_compute_json`'s number, in its Python.
    pub const NUMBER: u32 = 15;
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub node: String,
    pub workspace: String,
    /// `--max-time`, in seconds.
    pub max_time: String,
    pub max_disk: String,
    pub max_parallel: String,
    pub max_queued: String,
    pub max_sub: String,
    pub ci: Option<bool>,
    pub gpu: Option<bool>,
    /// `--userns|--no-userns` (0.46.18).
    pub userns: Option<bool>,
    /// `--no-addr-filter|--addr-filter` (0.46.18).
    pub no_addr_filter: Option<bool>,
}

/// The words after `grant`. `in_session`: `_compute_in_session`.
pub fn parse(words: &[&str], in_session: bool) -> Words<Ask> {
    let mut a = Ask::default();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let v = words.get(i + 1).copied().unwrap_or("").to_string();
        // No word after a value flag: `_need_value`'s line.
        let short = i + 2 > words.len();
        match w {
            "--workspace" | "--max-disk" | "--max-parallel" | "--max-queued" => {
                match w {
                    "--workspace" => a.workspace = v,
                    "--max-disk" => {
                        a.max_disk = v.strip_suffix(['G', 'g']).map_or(v.clone(), str::to_string)
                    }
                    "--max-parallel" => a.max_parallel = v,
                    _ => a.max_queued = v,
                }
                if short {
                    return Words::ShiftFailed(words[i].to_string());
                }
                i += 2;
                continue;
            }
            "--submitter-max-running" => {
                a.max_sub = v;
                if short {
                    return Words::ShiftFailed(words[i].to_string());
                }
                if a.max_sub.is_empty() || !a.max_sub.bytes().all(|b| b.is_ascii_digit()) {
                    return Words::Usage("usage: --submitter-max-running N (0 = off)".into());
                }
                i += 2;
                continue;
            }
            "--max-time" => {
                // `maxt="$(_compute_secs "${2:-}")"` before the shift.
                let Some(s) = secs(&v) else {
                    return Words::Usage(format!(
                        "usage: bad duration '{v}' (use 2h, 90m, 1h30m, 300s)"
                    ));
                };
                a.max_time = s;
                if short {
                    return Words::ShiftFailed(words[i].to_string());
                }
                i += 2;
                continue;
            }
            "--ci" => a.ci = Some(true),
            "--no-ci" => a.ci = Some(false),
            "--gpu" => a.gpu = Some(true),
            "--no-gpu" => a.gpu = Some(false),
            "--userns" => a.userns = Some(true),
            "--no-userns" => a.userns = Some(false),
            "--no-addr-filter" => a.no_addr_filter = Some(true),
            "--addr-filter" => a.no_addr_filter = Some(false),
            w if w.starts_with('-') => return Words::Usage(format!("usage: unknown flag '{w}'")),
            w => a.node = w.to_string(),
        }
        i += 1;
    }
    if a.node.is_empty() || (a.workspace.is_empty() && !in_session) {
        return Words::Usage(USAGE.to_string());
    }
    Words::Ask(a)
}

/// `_compute_secs` as text: its digits (`10#` drops the leading zeros).
fn secs(v: &str) -> Option<String> {
    super::wait::secs(v).map(|n| n.to_string())
}

/// `_compute_json`'s `_n` value: `float(v)` with a `.`, else `int(v)`.
pub fn number(v: &str) -> Result<Value, String> {
    if v.contains('.') {
        let t = v.trim_matches(crate::families::py_space);
        let ok = !t.is_empty()
            && t.trim_start_matches(['+', '-'])
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'.' || b == b'e' || b == b'E' || b == b'_');
        let parsed = if ok {
            json::parse(&normal_float(t)).ok()
        } else {
            None
        };
        return match parsed {
            Some(Value::Float(f)) => Ok(Value::Float(f)),
            Some(Value::Int(d)) => Ok(Value::Float(d.parse::<f64>().unwrap_or(f64::NAN))),
            _ => Err(format!(
                "ValueError: could not convert string to float: {}",
                py::repr(&str_value(v))
            )),
        };
    }
    super::int_of(&str_value(v)).map(Value::Int)
}

/// A float literal as JSON reads it: `.5` is `0.5`, `5.` is `5.0`, no
/// `+` and no `_`.
fn normal_float(t: &str) -> String {
    let (sign, rest) = match t.as_bytes().first() {
        Some(b'-') => ("-", &t[1..]),
        Some(b'+') => ("", &t[1..]),
        _ => ("", t),
    };
    if rest.contains('_') {
        return String::from("x");
    }
    let (mant, exp) = match rest.find(['e', 'E']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let mant = if mant.starts_with('.') {
        format!("0{mant}")
    } else {
        mant.to_string()
    };
    let mant = if mant.ends_with('.') {
        format!("{mant}0")
    } else {
        mant
    };
    format!("{sign}{mant}{exp}")
}

/// The body `_compute_json` and the two edits after it write.
pub fn body(a: &Ask) -> Result<String, String> {
    let mut pairs = vec![];
    for (k, v) in [("node", &a.node), ("workspace", &a.workspace)] {
        if !v.is_empty() {
            pairs.push((k, str_value(v)));
        }
    }
    for (k, v) in [
        ("maxJobSecs", &a.max_time),
        ("maxDiskGb", &a.max_disk),
        ("maxParallel", &a.max_parallel),
        ("maxQueued", &a.max_queued),
        ("submitterMaxRunning", &a.max_sub),
    ] {
        if !v.is_empty() {
            pairs.push((k, number(v)?));
        }
    }
    if let Some(ci) = a.ci {
        pairs.push(("allowCi", Value::Bool(ci)));
    }
    if let Some(gpu) = a.gpu {
        pairs.push(("allowGpu", Value::Bool(gpu)));
    }
    if let Some(userns) = a.userns {
        pairs.push(("allowUserns", Value::Bool(userns)));
    }
    if let Some(naf) = a.no_addr_filter {
        pairs.push(("allowNoAddrFilter", Value::Bool(naf)));
    }
    Ok(dumps(&object(pairs)))
}

/// The request and what it prints.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let body = match body(a) {
        Ok(b) => b,
        // `body="$(… python3 …)"` under `set -e`: the traceback, exit 1.
        Err(e) => return stopped(String::new(), &Raise::at(line::NUMBER, e)).print(),
    };
    match cp.call(true, ENDPOINT, &body, true) {
        Ok(answer) => reader(&answer).print(),
        Err(e) => e,
    }
}

fn key<'a>(v: &'a Value, k: &str, line: u32) -> Result<&'a Value, Raise> {
    if let Value::Object(p) = v {
        return get(p, k).ok_or_else(|| Raise::at(line, format!("KeyError: '{k}'")));
    }
    Err(Raise::at(
        line,
        super::subscript(v, k).err().unwrap_or_default(),
    ))
}

/// The reader on the answer.
pub fn reader(body: &[u8]) -> Shown {
    use line as l;
    let mut out = String::new();
    let r = (|| -> Result<(), Raise> {
        let v = load(body, l::LOAD)?;
        let g = key(&v, "grant", l::GRANT)?;
        let Value::Object(gp) = g else {
            return Err(Raise::no_attribute(l::SUB, g, "get"));
        };
        let sub = match get(gp, "submitterMaxRunning") {
            Some(x) if py::truthy(x) => format!(", {} running per submitter", py::str_of(x)),
            _ => String::new(),
        };
        let s = |k: &str| key(g, k, l::PRINT).map(py::str_of);
        let vs = |k: &str| key(&v, k, l::PRINT).map(py::str_of);
        let (node, ws) = (vs("node")?, vs("workspace")?);
        let (secs, par, queued, disk) = (
            s("maxJobSecs")?,
            s("maxParallel")?,
            s("maxQueued")?,
            s("maxDiskGb")?,
        );
        let ci = if get(gp, "allowCi").is_some_and(py::truthy) {
            ", CI allowed"
        } else {
            ""
        };
        let gpu = if get(gp, "allowGpu").is_some_and(py::truthy) {
            ", GPU jobs allowed"
        } else {
            ""
        };
        let userns = if get(gp, "allowUserns").is_some_and(py::truthy) {
            ", --userns jobs allowed"
        } else {
            ""
        };
        let naf = if get(gp, "allowNoAddrFilter").is_some_and(py::truthy) {
            ", --no-addr-filter allowed"
        } else {
            ""
        };
        let note = vs("note")?;
        out.push_str(&format!(
            "granted {node} to {ws} (max {secs}s/job, {par} parallel{sub}, {queued} queued, {disk} GB disk{ci}{gpu}{userns}{naf}). {note}\n"
        ));
        let resent = match &v {
            Value::Object(vp) => get(vp, "resent").filter(|x| py::truthy(x)),
            _ => None,
        };
        if let Some(resent) = resent {
            let n = py::percent_d(resent).map_err(|e| Raise::at(l::RESENT_PRINT, e))?;
            out.push_str(&format!(
                "re-sent {n} queued job(s) of {} on {} with the new caps (they keep their order)\n",
                vs("workspace")?,
                vs("node")?
            ));
        }
        Ok(())
    })();
    match r {
        Ok(()) => Shown::out(out),
        Err(e) => stopped(out, &e),
    }
}
