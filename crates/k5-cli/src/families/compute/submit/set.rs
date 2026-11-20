//! `_compute_follow_set <job> [json]` (`cli/src/compute-run.sh`): a split
//! run (`--shards`) followed to its end. Each poll is one long GET of
//! `/cli/compute/jobs/get` (`wait=20`); a line on stderr each time a
//! shard's state, attempt, node or exit changes; at the end the combined
//! line and the exit of a single job (the first failing shard's code, 128
//! plus a signal, or 3 for an end that is no result, with its error JSON).
//! No answer: one "lost" line, a poll every 10 seconds, after 60 of them
//! `lost_contact` and exit 3. Ctrl-C prints the `detached` line, exit 130.
//!
//! The shell keeps the shards it has seen in a temporary file (a JSON
//! object of strings); the binary keeps them in memory.

use super::super::follow::{self, int_trap, LOST_PAUSE};
use super::super::more::py_line_q;
use super::super::pyrt::{self as p, s};
use super::super::render::{self, Cx};
use super::super::writes::{dumps, fail, object, str_value, Cp};
use super::SRC;
use crate::exit::Exit;
use crate::families::{raised, Raise};
use crate::out;
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::{brand::CLI_NAME, cli_name, product_name};

macro_rules! line {
    ($m:literal) => {
        const { py_line_q(SRC, "\n_compute_follow_set() {", $m) }
    };
}

/// The states a job ends in.
const TERM: [&str; 6] = [
    "done",
    "failed",
    "cancelled",
    "timeout",
    "interrupted",
    "unknown",
];

/// One poll's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Poll {
    pub end: bool,
    pub state: String,
    pub code: String,
    pub sig: String,
    /// The error JSON line for an end that is no result.
    pub err: Option<String>,
}

/// Follows the split run `job` to its end.
pub fn follow_set(cp: &mut Cp<'_>, job: &str, as_json: bool) -> Exit {
    int_trap::set(format!(
        "[{CLI_NAME} compute] detached; the shards keep running. {CLI_NAME} compute get {job} | {CLI_NAME} compute cancel {job}\n"
    ));
    let mut seen: Vec<(String, String)> = vec![];
    let mut fails = 0u32;
    loop {
        let q = format!("job={}&wait=20", quote(job));
        let split = match cp.send(false, follow::GET, &q, false) {
            Ok(s) => s,
            Err(e) => return e,
        };
        if split.status == "000" {
            fails += 1;
            if fails == 1 {
                out::stderr_line(&format!(
                    "[{CLI_NAME} compute] lost the {} server; retrying (the shards keep running)",
                    product_name!()
                ));
            }
            if fails > 60 {
                out::stderr_line(&format!(
                    "{{\"error\":\"lost_contact\",\"hint\":\"no answer from the {} server for 10 minutes: {CLI_NAME} compute get {job}\"}}",
                    product_name!()
                ));
                return Exit::Refused;
            }
            std::thread::sleep(LOST_PAUSE);
            continue;
        }
        if split.status != "200" {
            return fail(&split.status, &split.body);
        }
        fails = 0;
        let mut cx = Cx::new(0);
        let mut err = String::new();
        let r = poll(&mut cx, &split.body, &mut seen, &mut err);
        let _ = out::write_flushed(&mut std::io::stderr().lock(), err.as_bytes());
        let st = match r {
            Ok(st) => st,
            Err(e) => return raised(String::new(), String::new(), &e).print(),
        };
        if st.end {
            int_trap::clear();
            if as_json {
                if let Err(e) = follow::print_result(cp, job) {
                    return e;
                }
            }
            if st.state == "done" {
                if !st.code.is_empty() {
                    return Exit::from_curl((arith(&st.code) & 255) as i32);
                }
                return Exit::from_curl((128i64.wrapping_add(arith(&st.sig)) & 255) as i32);
            }
            if let Some(e) = &st.err {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), e.as_bytes());
            }
            return Exit::Refused;
        }
    }
}

/// A word in `exit` or `$(( ))`: digits, or 0.
fn arith(v: &str) -> i64 {
    if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
        v.bytes().fold(0i64, |n, b| {
            n.wrapping_mul(10).wrapping_add(i64::from(b - b'0'))
        })
    } else {
        0
    }
}

/// `re.sub(r"[^A-Za-z0-9_.:-]", "", str(x if x is not None else ""))`.
fn field(x: &Value) -> String {
    let t = if p::is_none(x) {
        String::new()
    } else {
        p::str_of(x)
    };
    t.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
        .collect()
}

/// `(" · %d passed, %d failed" % (t["passed"], t["failed"] + t["errors"])) if t else ""`.
fn tests_line(cx: &Cx, l: u32, fmt: &str, t: &Value) -> Result<String, Raise> {
    if !p::truthy(t) {
        return Ok(String::new());
    }
    let passed = cx.at(l, p::idx(t, "passed"))?;
    let failed = cx.at(l, p::idx(t, "failed"))?;
    let errors = cx.at(l, p::idx(t, "errors"))?;
    let bad = cx.at(l, p::add(&failed, &errors))?;
    cx.at(l, p::pct(fmt, &[passed, bad]))
}

/// One poll's Python: the shards' lines, then the fields.
pub fn poll(
    cx: &mut Cx,
    body: &[u8],
    seen: &mut Vec<(String, String)>,
    err: &mut String,
) -> Result<Poll, Raise> {
    let v = render::load(cx, body, line!("v = json.load"))?;
    let l = line!("r = v.get(\"result\") or {}");
    let r = p::or(cx.at(l, p::get(&v, "result"))?, Value::Object(vec![]));
    let l = line!("for x in r.get(\"shards\")");
    let shards = p::or(cx.at(l, p::get(&r, "shards"))?, Value::Array(vec![]));
    for x in cx.at(l, p::iter(&shards))? {
        let l = line!("key = \"%s|%s|%s|%s\"");
        let state = cx.at(l, p::idx(&x, "state"))?;
        let attempt = cx.at(l, p::get(&x, "attempt"))?;
        let node = cx.at(l, p::get(&x, "node"))?;
        let exit = cx.at(l, p::get(&x, "exit"))?;
        let key = cx.at(l, p::pct("%s|%s|%s|%s", &[state, attempt, node, exit]))?;
        let l = line!("if seen.get(str(x[\"index\"])) != key:");
        let index = p::str_of(&cx.at(l, p::idx(&x, "index"))?);
        if seen.iter().any(|(k, v)| *k == index && *v == key) {
            continue;
        }
        let at = seen.iter().position(|(k, _)| *k == index);
        match at {
            Some(i) => seen[i].1 = key,
            None => seen.push((index, key)),
        }
        let l = line!("what = x[\"state\"]");
        let mut what = cx.at(l, p::idx(&x, "state"))?;
        let l = line!("if x[\"state\"] == \"done\":");
        if p::eq_s(&cx.at(l, p::idx(&x, "state"))?, "done") {
            let l = line!("what = \"exit %s\"");
            let e = cx.at(l, p::get(&x, "exit"))?;
            what = s(&cx.at(l, p::pct("exit %s", &[e]))?);
        } else {
            let l = line!("elif x.get(\"reason\") and");
            if p::truthy(&cx.at(l, p::get(&x, "reason"))?)
                && !p::eq_s(&cx.at(l, p::idx(&x, "state"))?, "running")
            {
                let l = line!("what += \" (%s)\"");
                let reason = cx.at(l, p::idx(&x, "reason"))?;
                let more = cx.at(l, p::pct(" (%s)", &[reason]))?;
                what = cx.at(l, p::iadd(&what, &s(&more)))?;
            }
        }
        let l = line!("att = (\" attempt %s\"");
        let a = p::or(cx.at(l, p::get(&x, "attempt"))?, p::int(1));
        let att = if cx.at(l, p::cmp(">", &a, &p::int(1)))? {
            let a = cx.at(l, p::idx(&x, "attempt"))?;
            s(&cx.at(l, p::pct(" attempt %s", &[a]))?)
        } else {
            s("")
        };
        let l = line!("sys.stderr.write(\"[k2 compute] shard %s/%d");
        let index = cx.at(l, p::idx(&x, "index"))?;
        let all = cx.at(l, p::idx(&r, "shards"))?;
        let count = cx.at(l, p::len(&all))?;
        let node = p::or(cx.at(l, p::get(&x, "node"))?, s("?"));
        let t = cx.at(l, p::get(&x, "tests"))?;
        let tl = cx.call(l, "tl", |cx| {
            tests_line(
                cx,
                line!("return (\" · %d passed"),
                " \u{b7} %d passed, %d failed",
                &t,
            )
        })?;
        err.push_str(&cx.at(
            l,
            p::pct(
                &format!("[{CLI_NAME} compute] shard %s/%d on %s%s: %s%s\n"),
                &[index, p::int(count as i128), node, att, what, s(&tl)],
            ),
        )?);
    }
    let l = line!("term = v.get(\"state\") in");
    let state = cx.at(l, p::get(&v, "state"))?;
    let term = TERM.iter().any(|t| p::eq_s(&state, t));
    let mut error = None;
    if term {
        let l = line!("t = r.get(\"tests\")");
        let t = cx.at(l, p::get(&r, "tests"))?;
        let l = line!("took = (\" after %dm%02ds\"");
        let took = if p::is_none(&cx.at(l, p::get(&r, "durationSecs"))?) {
            String::new()
        } else {
            let d = cx.at(l, p::idx(&r, "durationSecs"))?;
            let m = cx.at(l, p::floordiv(&d, &p::int(60)))?;
            let d = cx.at(l, p::idx(&r, "durationSecs"))?;
            let sec = cx.at(l, p::rem(&d, &p::int(60)))?;
            cx.at(l, p::pct(" after %dm%02ds", &[m, sec]))?
        };
        let l = line!("if v[\"state\"] == \"done\":");
        if p::eq_s(&cx.at(l, p::idx(&v, "state"))?, "done") {
            let l = line!("shards: exit %s%s%s");
            let shards = p::or(cx.at(l, p::get(&r, "shards"))?, Value::Array(vec![]));
            let count = cx.at(l, p::len(&shards))?;
            let exit = cx.at(l, p::get(&r, "exit"))?;
            let tests = tests_line(cx, l, " \u{b7} tests %d passed, %d failed", &t)?;
            let short = cx.at(l, p::idx(&v, "short"))?;
            err.push_str(&cx.at(
                l,
                p::pct(
                    concat!(
                        "[",
                        cli_name!(),
                        " compute] %d shards: exit %s%s%s \u{b7} ",
                        cli_name!(),
                        " compute logs %s --failures\n"
                    ),
                    &[p::int(count as i128), exit, s(&took), s(&tests), short],
                ),
            )?);
        } else {
            let l = line!("err = {\"error\": v[\"state\"]");
            let st = cx.at(l, p::idx(&v, "state"))?;
            let reason = cx.at(l, p::get(&v, "reason"))?;
            let detail = cx.at(l, p::get(&v, "detail"))?;
            let short = cx.at(l, p::idx(&v, "short"))?;
            let hint = cx.at(
                l,
                p::pct(
                    concat!(
                        cli_name!(),
                        " compute get %s --json; ",
                        cli_name!(),
                        " compute retry %s"
                    ),
                    &[short.clone(), short],
                ),
            )?;
            error = Some(
                dumps(&object(vec![
                    ("error", st),
                    ("reason", reason),
                    ("detail", detail),
                    ("hint", str_value(&hint)),
                ])) + "\n",
            );
        }
    }
    let l = line!("print(\"|\".join(");
    let code = field(&cx.at(l, p::get(&r, "exit"))?);
    let sig = field(&cx.at(l, p::get(&r, "signal"))?);
    Ok(Poll {
        end: term,
        state: field(&state),
        code,
        sig,
        err: error,
    })
}
