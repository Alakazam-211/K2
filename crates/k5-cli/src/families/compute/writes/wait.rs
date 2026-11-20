//! `k5 compute wait <job>... [--any] [--max-wait D] [--quiet] [--json]`,
//! as `cmd_compute_wait` and `_compute_wait_any` answer it
//! (`cli/src/compute-jobs.sh`).
//!
//! - Every job, one after the other: one long poll of
//!   `/cli/compute/jobs/get` (`wait=20`, less when `--max-wait` is near)
//!   per round until the job ends or the time is up, a progress line on
//!   stderr each time its state, place in the queue or reason changes
//!   (none with `--quiet`). Then one line per job that ended, the error
//!   lines, `still_running` for the ones that didn't, and the exit: the
//!   first failed job's code (its exit code, 128 plus its signal, or 3),
//!   3 when one is still running. `--json` prints every job's answer.
//! - `--any`: one short poll (`wait=1`) per job per round until the first
//!   ends: its line, the others still running, its code.
//! - No answer (status 000): "lost the server" once, 10 s, and again; after
//!   60 in a row, `lost_contact` and exit 3. Any other status but 200 is
//!   `_compute_fail`'s line, 404 exit 4.
//!
//! What goes to the shell: `--follow` (it streams the job's output),
//! `--failures` (it prints `compute logs`), a `--max-wait` that is no
//! duration (the shell's arithmetic fails on it), and, without `--any`, a
//! temp folder `mktemp -d` can't make.

use super::{dumps, int_of, load, object, stopped, str_at, str_value, Cp, Words, NO_STATUS};
use crate::exit::Exit;
use crate::families::{get, py_get, Raise, Shown};
use crate::out::json::{Text, Value};
use crate::out::{self, py};
use crate::transport::encode::quote;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The route.
pub const ENDPOINT: &str = "/cli/compute/jobs/get";
/// The usage line.
pub const USAGE: &str = concat!(
    "usage: ",
    k2_client::cli_name!(),
    " compute wait <job>... [--any] [--follow] [--failures] [--max-wait 9m] [--quiet] [--json]"
);
/// The longest poll, in seconds.
pub const LONG_POLL: u64 = 20;
/// Between tries when no answer came.
pub const RETRY_PAUSE: Duration = Duration::from_secs(10);
/// Tries with no answer in a row before giving up.
pub const MAX_FAILS: u32 = 60;
/// The states a job ends in.
pub const TERM: [&str; 6] = [
    "done",
    "failed",
    "cancelled",
    "timeout",
    "interrupted",
    "unknown",
];

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    /// The progress reader of one answer.
    pub mod info {
        pub const LOAD: u32 = 3;
        pub const Q: u32 = 4;
        pub const KEY: u32 = 7;
        pub const ELSEWHERE: u32 = 9;
    }
    /// The end's reader of every answer.
    pub mod end {
        pub const FOR: u32 = 11;
        pub const TOOK: u32 = 17;
        pub const RL: u32 = 19;
        pub const LATE: u32 = 21;
        pub const EXIT: u32 = 23;
        pub const SIGNAL: u32 = 25;
        pub const ST: u32 = 27;
        pub const STILL: u32 = 37;
        pub const RC: u32 = 41;
    }
    /// `--any`'s reader of one answer.
    pub mod any {
        pub const LOAD: u32 = 3;
        pub const ST: u32 = 4;
        pub const TOOK: u32 = 8;
        pub const EXIT: u32 = 10;
        pub const SIGNAL: u32 = 12;
        pub const ST_PLUS: u32 = 14;
    }
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub jobs: Vec<String>,
    pub quiet: bool,
    pub json: bool,
    pub any: bool,
    /// `--max-wait`, in seconds.
    pub max_wait: Option<u64>,
}

/// The words after `wait`. `None`: the shell answers (see the module's
/// notes).
pub fn parse(words: &[&str]) -> Option<Words<Ask>> {
    let mut a = Ask::default();
    let (mut failures, mut follow) = (false, false);
    let mut max_wait = String::new();
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--failures" => failures = true,
            "--quiet" | "-q" => a.quiet = true,
            "--json" => a.json = true,
            "--follow" | "-f" => follow = true,
            "--any" => a.any = true,
            "--max-wait" => {
                max_wait = words.get(i + 1).copied().unwrap_or("").to_string();
                if i + 2 > words.len() {
                    return Some(Words::ShiftFailed);
                }
                i += 2;
                continue;
            }
            w if w.starts_with('-') => return Some(Words::Usage(USAGE.to_string())),
            w => a.jobs.push(w.to_string()),
        }
        i += 1;
    }
    if a.jobs.is_empty() {
        return Some(Words::Usage(USAGE.to_string()));
    }
    if !max_wait.is_empty() {
        a.max_wait = Some(secs(&max_wait)?);
    }
    if follow || failures {
        return None;
    }
    Some(Words::Ask(a))
}

/// `_compute_secs`: plain seconds, or `<n>h`, `<n>m` and `<n>s` parts in
/// any mix. `None` for anything else (its usage line, inside `$(…)`,
/// then the shell's own arithmetic error), and for a number the shell's
/// 64-bit arithmetic would wrap.
pub fn secs(v: &str) -> Option<u64> {
    let num = |d: &str| -> Option<u64> {
        let d = d.trim_start_matches('0');
        if d.len() > 15 {
            return None;
        }
        Some(if d.is_empty() { 0 } else { d.parse().ok()? })
    };
    if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
        return num(v);
    }
    let mut total: u64 = 0;
    let mut digits = String::new();
    for c in v.chars() {
        match c {
            '0'..='9' => digits.push(c),
            'h' | 'm' | 's' if !digits.is_empty() => {
                let n = num(&digits)?;
                total += n * match c {
                    'h' => 3600,
                    'm' => 60,
                    _ => 1,
                };
                digits.clear();
            }
            _ => return None,
        }
    }
    (digits.is_empty() && !v.is_empty()).then_some(total)
}

/// `$(date +%s)`.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Whether `mktemp -d -t` can make its folder. On Linux it makes it in
/// `TMPDIR` (when set and not empty) or `/tmp`, and fails when that
/// folder can't take one. macOS's mktemp falls back to the user's own
/// temp folder (`getconf DARWIN_USER_TEMP_DIR`) when `TMPDIR` can't be
/// used (some versions use that folder always), so it fails only when
/// that one can't take one either.
pub fn temp_usable() -> bool {
    let tmpdir = std::env::var_os("TMPDIR").filter(|d| !d.is_empty());
    if cfg!(target_os = "macos") {
        return tmpdir.is_some_and(|d| can_make_in(std::path::Path::new(&d)))
            || darwin_user_temp_dir().is_some_and(|d| can_make_in(&d))
            || can_make_in(std::path::Path::new("/tmp"));
    }
    let dir = tmpdir.map_or_else(
        || std::path::PathBuf::from("/tmp"),
        std::path::PathBuf::from,
    );
    can_make_in(&dir)
}

/// Whether a folder can be made in `dir` (made and taken away again).
fn can_make_in(dir: &std::path::Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!("k5-compute-wait-probe.{}", std::process::id()));
    match std::fs::create_dir(&probe) {
        Ok(()) => std::fs::remove_dir(&probe).is_ok(),
        Err(_) => false,
    }
}

/// `confstr(_CS_DARWIN_USER_TEMP_DIR)`: the user's own temp folder.
#[cfg(target_os = "macos")]
pub fn darwin_user_temp_dir() -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0u8; 1024];
    // SAFETY: the buffer is as long as the length given.
    let n = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    if n == 0 || n > buf.len() {
        return None;
    }
    buf.truncate(n - 1);
    Some(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&buf)))
}

/// Only macOS has one.
#[cfg(not(target_os = "macos"))]
pub fn darwin_user_temp_dir() -> Option<std::path::PathBuf> {
    None
}

/// The poll of one job.
fn poll(cp: &mut Cp<'_>, job: &str, w: u64) -> Result<super::Split, Exit> {
    cp.send(
        false,
        ENDPOINT,
        &format!("job={}&wait={w}", quote(job)),
        false,
    )
}

/// The requests and what they print.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let deadline = a.max_wait.map(|s| now() + s);
    if a.any {
        return any(cp, a, deadline).unwrap_or_else(|e| e);
    }
    let mut bodies: Vec<Vec<u8>> = Vec::new();
    for job in &a.jobs {
        let mut fails = 0u32;
        let mut last = String::new();
        // Only each job's last answer is read at the end.
        let mut answer: Vec<u8>;
        loop {
            let mut w = LONG_POLL;
            if let Some(d) = deadline {
                w = w.min(d.saturating_sub(now()));
            }
            let s = match poll(cp, job, w) {
                Ok(s) => s,
                Err(e) => return e,
            };
            if s.status == NO_STATUS {
                fails += 1;
                if fails == 1 {
                    out::stderr_line(concat!(
                        "[",
                        k2_client::cli_name!(),
                        " compute] lost the ",
                        k2_client::product_name!(),
                        " server; retrying (the job keeps running on the node)"
                    ));
                }
                if fails > MAX_FAILS {
                    out::stderr_line(&format!(
                        concat!(
                            "{{\"error\":\"lost_contact\",\"hint\":\"no answer from the ",
                            k2_client::product_name!(),
                            " server for 10 minutes; the job may still be running: ",
                            k2_client::cli_name!(),
                            " compute wait {}\"}}"
                        ),
                        job
                    ));
                    return Exit::Refused;
                }
                std::thread::sleep(RETRY_PAUSE);
                continue;
            }
            if s.status != "200" {
                return fail_404_is_4(&s.status, &s.body);
            }
            fails = 0;
            let info = match info(&s.body) {
                Ok(t) => t,
                Err(r) => return stopped(String::new(), &r).print(),
            };
            answer = s.body;
            let (term, key, line) = split_info(&info);
            if term == "1" {
                break;
            }
            if key != last && !a.quiet {
                out::stderr_line(&format!(
                    concat!("[", k2_client::cli_name!(), " compute] {}"),
                    line
                ));
            }
            last = key;
            if deadline.is_some_and(|d| now() >= d) {
                break;
            }
        }
        bodies.push(answer);
    }
    end_of(&bodies, a.json).print()
}

/// `_compute_fail` with 4 for a 404 (`wait` and `wait --any` pass it).
fn fail_404_is_4(status: &str, body: &[u8]) -> Exit {
    let e = super::fail(status, body);
    if status == "404" {
        Exit::NotFound
    } else {
        e
    }
}

/// `term`, `key` and `line` as the shell cuts them from what `$(…)` held
/// of the reader's line: before the first tab, then before the next, and
/// the rest.
pub fn split_info(info: &str) -> (String, String, String) {
    let held = info.trim_end_matches('\n');
    let term = held.split_once('\t').map_or(held, |(t, _)| t);
    let rest = held.split_once('\t').map_or(held, |(_, r)| r);
    let key = rest.split_once('\t').map_or(rest, |(k, _)| k);
    let line = rest.split_once('\t').map_or(rest, |(_, l)| l);
    (term.to_string(), key.to_string(), line.to_string())
}

fn dict_or_empty(v: Option<&Value>) -> Value {
    match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => Value::Object(Vec::new()),
    }
}

fn get_of(v: &Value, key: &str, line: u32) -> Result<Option<Value>, Raise> {
    py_get(v, key, line).map(|o| o.cloned())
}

/// The progress reader's line for one answer: `<1|0>\t<key>\t<line>\n`.
pub fn info(body: &[u8]) -> Result<String, Raise> {
    use line::info as l;
    let j = load(body, l::LOAD)?;
    let q = dict_or_empty(py_get(&j, "queue", l::Q)?);
    let wt = dict_or_empty(get_of(&j, "wait", l::Q + 1)?.as_ref());
    let st = match get_of(&j, "state", l::Q + 2)? {
        Some(x) if py::truthy(&x) => x,
        _ => str_value("?"),
    };
    let reason = match get_of(&q, "waitReason", l::KEY)? {
        Some(x) if py::truthy(&x) => Some(x),
        _ => get_of(&q, "reason", l::KEY)?,
    };
    let key = format!(
        "{}|{}|{}",
        py::str_of(&st),
        str_at(get_of(&q, "position", l::KEY)?.as_ref()),
        str_at(reason.as_ref())
    );
    let term = TERM.iter().any(|t| super::is_str(Some(&st), t));
    let elsewhere = match get_of(&wt, "elsewhere", l::ELSEWHERE)? {
        Some(x) if py::truthy(&x) => match &x {
            Value::Str(_) => format!(" · {}", py::str_of(&x)),
            other => {
                return Err(Raise::at(
                    l::ELSEWHERE,
                    format!(
                        "TypeError: can only concatenate str (not \"{}\") to str",
                        py::type_name(other)
                    ),
                ))
            }
        },
        _ => String::new(),
    };
    let short = str_at(get(pairs(&j), "short"));
    let queued = super::is_str(Some(&st), "queued");
    let truthy = |v: &Value, k: &str| get(pairs(v), k).is_some_and(py::truthy);
    let line = if queued && truthy(&q, "line") {
        format!("{short} {}{elsewhere}", str_at(get(pairs(&q), "line")))
    } else if queued && truthy(&wt, "words") {
        format!(
            "{short} queued: {}{elsewhere}",
            str_at(get(pairs(&wt), "words"))
        )
    } else if queued && py::truthy(&q) {
        let of = if truthy(&q, "of") {
            format!(" of {}", str_at(get(pairs(&q), "of")))
        } else {
            String::new()
        };
        format!(
            "{short} queued on {}: position {}{of}{elsewhere} ({})",
            str_at(get(pairs(&j), "node")),
            str_at(get(pairs(&q), "position")),
            str_at(get(pairs(&q), "reason"))
        )
    } else {
        format!(
            "{short} {} on {}",
            py::str_of(&st),
            str_at(get(pairs(&j), "node"))
        )
    };
    Ok(format!("{}\t{key}\t{line}\n", u8::from(term)))
}

/// An object's pairs; nothing for any other value.
fn pairs(v: &Value) -> &[(Text, Value)] {
    match v {
        Value::Object(p) => p,
        _ => &[],
    }
}

/// A number Python adds, takes away and divides.
#[derive(Debug, Clone, Copy)]
enum Num {
    Int(i128),
    Float(f64),
}

fn num(v: &Value) -> Option<Num> {
    match v {
        Value::Int(d) => d.parse().ok().map(Num::Int),
        Value::Bool(b) => Some(Num::Int(i128::from(*b))),
        Value::Float(f) => Some(Num::Float(*f)),
        _ => None,
    }
}

fn unsupported(op: &str, a: &Value, b: &Value) -> String {
    format!(
        "TypeError: unsupported operand type(s) for {op}: '{}' and '{}'",
        py::type_name(a),
        py::type_name(b)
    )
}

/// `b - a`.
fn minus(b: &Value, a: &Value) -> Result<Num, String> {
    match (num(b), num(a)) {
        (Some(Num::Int(x)), Some(Num::Int(y))) => Ok(Num::Int(x - y)),
        (Some(x), Some(y)) => Ok(Num::Float(as_f(x) - as_f(y))),
        _ => Err(unsupported("-", b, a)),
    }
}

fn as_f(n: Num) -> f64 {
    match n {
        Num::Int(i) => i as f64,
        Num::Float(f) => f,
    }
}

/// `'%d' % (n // d)` and `'%02d' % (n % d)` (`d` 60): Python's floor
/// division and modulo, then `int()`.
fn min_sec(n: Num) -> Result<(String, String), String> {
    match n {
        Num::Int(i) => Ok((
            i.div_euclid(60).to_string(),
            format!("{:02}", i.rem_euclid(60)),
        )),
        Num::Float(f) => {
            if f.is_nan() {
                return Err("ValueError: cannot convert float NaN to integer".into());
            }
            if f.is_infinite() {
                return Err("OverflowError: cannot convert float infinity to integer".into());
            }
            let q = (f / 60.0).floor();
            let r = f - 60.0 * q;
            let q = py::percent_d(&Value::Float(q))?;
            let r = py::percent_d(&Value::Float(r))?;
            Ok((q, format!("{r:0>2}")))
        }
    }
}

/// `n // 60`, `'%d'` of it.
fn minutes(v: &Value) -> Result<String, String> {
    match num(v) {
        Some(Num::Int(i)) => Ok(i.div_euclid(60).to_string()),
        Some(Num::Float(f)) => py::percent_d(&Value::Float((f / 60.0).floor())),
        None => Err(format!(
            "TypeError: unsupported operand type(s) for //: '{}' and 'int'",
            py::type_name(v)
        )),
    }
}

/// `took`: ` after <m>m<ss>s` when the job has both times.
fn took(j: &Value, line: u32) -> Result<String, Raise> {
    let p = pairs(j);
    let a = match get(p, "startedAt") {
        Some(x) if py::truthy(x) => Some(x.clone()),
        _ => get(p, "assignedAt").cloned(),
    };
    let b = get(p, "endedAt").cloned();
    match (a, b) {
        (Some(a), Some(b)) if py::truthy(&a) && py::truthy(&b) => {
            let n = minus(&b, &a).map_err(|e| Raise::at(line, e))?;
            let (m, s) = min_sec(n).map_err(|e| Raise::at(line, e))?;
            Ok(format!(" after {m}m{s}s"))
        }
        _ => Ok(String::new()),
    }
}

/// A job's code and what it says, as the readers work them out.
fn code_and_what(
    j: &Value,
    exit_line: u32,
    signal_line: u32,
    st_line: u32,
) -> Result<(String, String), Raise> {
    let p = pairs(j);
    let st = get(p, "state");
    let done = super::is_str(st, "done");
    let exit_code = get(p, "exitCode").filter(|v| **v != Value::Null);
    if done {
        if let Some(e) = exit_code {
            let code = int_of(e).map_err(|x| Raise::at(exit_line, x))?;
            return Ok((code, format!("exit {}", py::str_of(e))));
        }
        let signal = match get(p, "signal") {
            Some(x) if py::truthy(x) => x.clone(),
            _ => Value::Int("0".into()),
        };
        let n = int_of(&signal).map_err(|x| Raise::at(signal_line, x))?;
        let code = add_128(&n);
        return Ok((
            code,
            format!("killed by signal {}", str_at(get(p, "signal"))),
        ));
    }
    let st = st.cloned().unwrap_or(Value::Null);
    let Value::Str(_) = st else {
        return Err(Raise::at(
            st_line,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                py::type_name(&st)
            ),
        ));
    };
    let reason = match get(p, "reason") {
        Some(x) if py::truthy(x) => format!(" ({})", py::str_of(x)),
        _ => String::new(),
    };
    Ok(("3".to_string(), format!("{}{reason}", py::str_of(&st))))
}

/// `128 + n` on an integer's digits.
fn add_128(n: &str) -> String {
    match n.parse::<i128>() {
        Ok(i) => (i + 128).to_string(),
        Err(_) => n.to_string(),
    }
}

/// `exit "<digits>"`: the low 8 bits, as a shell takes them.
pub fn exit_of(digits: &str) -> Exit {
    let (neg, d) = digits
        .strip_prefix('-')
        .map_or((false, digits), |r| (true, r));
    let mut m: i64 = 0;
    for b in d.bytes() {
        m = (m * 10 + i64::from(b - b'0')) % 256;
    }
    let m = if neg { (256 - m) % 256 } else { m };
    Exit::from_curl(m as i32)
}

/// What the end's reader worked out: the code, the failed jobs' ids and
/// every answer.
type EndRead = (Option<String>, Vec<String>, Vec<Value>);

/// The end's reader on every job's last answer: the lines on stderr,
/// `--json`'s list on stdout, and the exit. The stderr lines are
/// written as the reader goes, so a raise comes after the ones before it.
pub fn end_of(bodies: &[Vec<u8>], json: bool) -> Shown {
    use line::end as l;
    let r = (|| -> Result<EndRead, Raise> {
        let mut values = Vec::new();
        for b in bodies {
            values.push(load(b, 5)?);
        }
        let mut rc: Option<String> = None;
        let mut unfinished: Vec<Value> = Vec::new();
        let mut errs: Vec<String> = Vec::new();
        let mut failed: Vec<Value> = Vec::new();
        for j in &values {
            let p = pairs(j);
            let st = get(p, "state");
            if !TERM.iter().any(|t| super::is_str(st, t)) {
                unfinished.push(get(p, "short").cloned().unwrap_or(Value::Null));
                continue;
            }
            let mut took = took(j, l::TOOK)?;
            let rl = dict_or_empty(get(p, "reportedLate"));
            let Value::Object(rlp) = &rl else {
                return Err(Raise::no_attribute(l::RL, &rl, "get"));
            };
            if let Some(secs) = get(rlp, "secs").filter(|v| py::truthy(v)) {
                let m = minutes(secs).map_err(|e| Raise::at(l::LATE, e))?;
                took.push_str(&format!(" (node time; this server heard {m}m late)"));
            }
            let (code, what) = code_and_what(j, l::EXIT, l::SIGNAL, l::ST)?;
            if !super::is_str(st, "done") {
                let short = str_at(get(p, "short"));
                let reason = match get(p, "reason") {
                    Some(x) if py::truthy(x) => x.clone(),
                    _ => str_value(""),
                };
                errs.push(dumps(&object(vec![
                    ("error", st.cloned().unwrap_or(Value::Null)),
                    ("reason", reason),
                    (
                        "hint",
                        str_value(&format!(
                            concat!(
                                k2_client::cli_name!(),
                                " compute get {s}; ",
                                k2_client::cli_name!(),
                                " compute retry {s}"
                            ),
                            s = short
                        )),
                    ),
                ])));
            }
            let line = format!(
                concat!(
                    "[",
                    k2_client::cli_name!(),
                    " compute] job {} on {}: {}{}\n"
                ),
                str_at(get(p, "short")),
                str_at(get(p, "node")),
                what,
                took
            );
            out::stderr_line(line.trim_end_matches('\n'));
            if code != "0" {
                failed.push(get(p, "id").cloned().unwrap_or(Value::Null));
                if rc.is_none() {
                    rc = Some(code);
                }
            }
        }
        for e in &errs {
            out::stderr_line(e);
        }
        if !unfinished.is_empty() {
            let mut names = Vec::new();
            for (i, u) in unfinished.iter().enumerate() {
                match u {
                    Value::Str(_) => names.push(py::str_of(u)),
                    other => {
                        return Err(Raise::at(
                            l::STILL,
                            format!(
                                "TypeError: sequence item {i}: expected str instance, {} found",
                                py::type_name(other)
                            ),
                        ))
                    }
                }
            }
            out::stderr_line(&dumps(&object(vec![
                ("error", str_value("still_running")),
                ("jobs", Value::Array(unfinished.clone())),
                (
                    "hint",
                    str_value(&format!(
                        concat!(k2_client::cli_name!(), " compute wait {}"),
                        names.join(" ")
                    )),
                ),
            ])));
            rc = Some("3".to_string());
        }
        let mut ids = Vec::new();
        for (i, f) in failed.iter().filter(|f| py::truthy(f)).enumerate() {
            match f {
                Value::Str(_) => ids.push(py::str_of(f)),
                other => {
                    return Err(Raise::at(
                        l::RC,
                        format!(
                            "TypeError: sequence item {i}: expected str instance, {} found",
                            py::type_name(other)
                        ),
                    ))
                }
            }
        }
        Ok((rc, ids, values))
    })();
    match r {
        Ok((rc, _ids, values)) => {
            let rc = rc.unwrap_or_else(|| "0".to_string());
            let stdout = if json {
                format!("{}\n", dumps(&Value::Array(values)))
            } else {
                String::new()
            };
            Shown {
                stdout,
                stderr: String::new(),
                exit: exit_of(&rc),
            }
        }
        Err(e) => stopped(String::new(), &e),
    }
}

/// `--any`.
fn any(cp: &mut Cp<'_>, a: &Ask, deadline: Option<u64>) -> Result<Exit, Exit> {
    let mut fails = 0u32;
    let mut said = false;
    'round: loop {
        for job in &a.jobs {
            let s = poll(cp, job, 1)?;
            if s.status == NO_STATUS {
                fails += 1;
                if fails == 1 {
                    out::stderr_line(concat!(
                        "[",
                        k2_client::cli_name!(),
                        " compute] lost the ",
                        k2_client::product_name!(),
                        " server; retrying (the jobs keep running on the nodes)"
                    ));
                }
                if fails > MAX_FAILS {
                    out::stderr_line(concat!(
                        "{\"error\":\"lost_contact\",\"hint\":\"no answer from the ",
                        k2_client::product_name!(),
                        " server for 10 minutes; the jobs may still be running\"}"
                    ));
                    return Ok(Exit::Refused);
                }
                std::thread::sleep(RETRY_PAUSE);
                continue 'round;
            }
            if s.status != "200" {
                return Ok(fail_404_is_4(&s.status, &s.body));
            }
            fails = 0;
            let ended = match any_reader(&s.body, a.json) {
                Ok(Some(e)) => e,
                Ok(None) => continue,
                Err(r) => return Ok(stopped(String::new(), &r).print()),
            };
            if let Some(json) = &ended.json {
                if let Err(e) = out::stdout_line(json) {
                    return Ok(out::after_stdout_error(&e));
                }
            }
            let others: Vec<&str> = a
                .jobs
                .iter()
                .filter(|o| *o != job)
                .map(String::as_str)
                .collect();
            if !others.is_empty() && !a.quiet {
                let list = others.join(" ");
                out::stderr_line(&format!(
                    concat!(
                        "[",
                        k2_client::cli_name!(),
                        " compute] still running: {l} (",
                        k2_client::cli_name!(),
                        " compute wait {l} --any)"
                    ),
                    l = list
                ));
            }
            return Ok(exit_of(&ended.rc));
        }
        if deadline.is_some_and(|d| now() >= d) {
            let all = a.jobs.join(" ");
            out::stderr_line(&format!(
                concat!(
                    "{{\"error\":\"still_running\",\"jobs\":\"{all}\",\"hint\":\"none ended yet: ",
                    k2_client::cli_name!(),
                    " compute wait {all} --any\"}}"
                ),
                all = all
            ));
            return Ok(Exit::Refused);
        }
        if !a.quiet && !said {
            out::stderr_line(&format!(
                concat!(
                    "[",
                    k2_client::cli_name!(),
                    " compute] waiting for the first of {} job(s) to end"
                ),
                a.jobs.len()
            ));
            said = true;
        }
    }
}

/// What `--any`'s reader made of a job that ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnyEnded {
    /// `--json`: the answer as `json.dumps` writes it.
    pub json: Option<String>,
    /// The code, as `RC <code>` holds it.
    pub rc: String,
}

/// `--any`'s reader on one answer: `None` while the job runs; else its
/// line (on stderr here) and its code.
pub fn any_reader(body: &[u8], json: bool) -> Result<Option<AnyEnded>, Raise> {
    use line::any as l;
    let j = load(body, l::LOAD)?;
    let st = py_get(&j, "state", l::ST)?.cloned();
    if !TERM.iter().any(|t| super::is_str(st.as_ref(), t)) {
        return Ok(None);
    }
    let took = took(&j, l::TOOK)?;
    let (code, what) = code_and_what(&j, l::EXIT, l::SIGNAL, l::ST_PLUS)?;
    let p = pairs(&j);
    out::stderr_line(&format!(
        concat!(
            "[",
            k2_client::cli_name!(),
            " compute] job {} on {} ended first: {}{}"
        ),
        str_at(get(p, "short")),
        str_at(get(p, "node")),
        what,
        took
    ));
    Ok(Some(AnyEnded {
        json: json.then(|| dumps(&j)),
        rc: code,
    }))
}
