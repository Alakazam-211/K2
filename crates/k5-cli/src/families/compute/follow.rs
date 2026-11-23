//! `_compute_follow <job> <raw> [cursor|seq:N] [json]`
//! (`cli/src/compute-lib.sh`): a job's output, followed until the job
//! ends, then its end line and its exit. Shared by every verb that follows
//! a job (`attach` here; `run --wait`, `retry`, `wait --follow` can use
//! it).
//!
//! Each poll is one GET of `/cli/compute/jobs/logs` (`wait=20`, the
//! cell socket first), whose chunks go to stdout (`out`) or stderr as they
//! come, escape sequences and control bytes taken out unless `raw`. No
//! answer: one "lost" line, a poll every 10 seconds, and after 60 of
//! them the `lost_contact` line, exit 3. Any status but 200:
//! `_compute_fail`. A generation that ended without ending the job while a
//! new one waits: a line, and the new one is followed. The line says
//! `moved:` and the daemon's words when the attempt record says it moved
//! (or, from an older daemon, the attempt number stayed), and "hit an
//! infrastructure failure" only for a real retry. A queued job: one
//! line each time its place or its reason changes. At the end: `done`
//! with an exit code exits with it (`exit N`), killed by a signal 128 plus
//! it; any other end its line and the job's error JSON, exit 3.
//!
//! The shell keeps each poll's fields in a temporary file (`mktemp`); the
//! binary keeps them in memory, and callers hand a line to the shell where
//! that `mktemp` would fail ([`usable`]). Ctrl-C prints the shell's
//! `detached` line and exits 130, as its `INT` trap does.
//!
//! Not here (callers hand these to the shell): `_CP_FAILURES` (the
//! failing tests after a failing exit), and fields that bash's arithmetic
//! or `exit` would refuse (the daemon sends whole numbers).

use super::lines::{py_line, LIB};
use super::pyrt::{self as p, s};
use super::writes::{fail, load, stopped, str_value, Cp};
use crate::exit::Exit;
use crate::families::Raise;
use crate::out::{self, json::Value};
use crate::transport::encode::quote;
use k2_client::{brand::CLI_NAME, cli_name, product_name};
use std::time::Duration;

pub const LOGS: &str = "/cli/compute/jobs/logs";
pub const GET: &str = "/cli/compute/jobs/get";

/// The pause after a poll with no answer.
pub const LOST_PAUSE: Duration = Duration::from_secs(10);

macro_rules! line {
    ($m:literal) => {
        const { py_line(LIB, "\n_compute_follow() {", $m) }
    };
}

/// Where to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// `seq:N`: the chunks from sequence number N (`since_seq=N`, as given).
    Seq(String),
    /// A cursor.
    Cursor(String),
}

/// What to follow, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Follow {
    pub job: String,
    pub raw: bool,
    pub start: Start,
    /// Every log line to stderr, and the job's `result` on stdout at the
    /// end.
    pub as_json: bool,
    /// `_CP_DEADLINE`: epoch seconds.
    pub deadline: Option<i64>,
    /// `${_CP_DEADLINE_RC:-3}`: the exit past the deadline (`wait --follow
    /// --timeout` sets 124).
    pub deadline_exit: Exit,
}

/// Whether the shell's `mktemp -t k2-compute-XXXXXX` would make its file.
pub fn usable() -> bool {
    super::writes::wait::temp_usable()
        && crate::families::update::shell_program(&crate::offline::Env::current(), "mktemp")
            .is_some()
}

/// One poll's fields, as the shell reads them back from its state file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub cursor: String,
    pub end: bool,
    pub state: String,
    pub code: String,
    pub sig: String,
    pub reason: String,
    pub short: String,
    pub node: String,
    pub started: String,
    pub ended: String,
    pub qpos: String,
    pub qreason: String,
    pub fgen: String,
    pub jgen: String,
    pub attempt: String,
    pub lastfail: String,
    /// The move's words (`.mv`): empty unless the generation read moved.
    pub moved: String,
    /// The wait line (`.q`).
    pub qline: String,
    /// The job's error JSON (`.err`).
    pub err: String,
}

/// `${job:0:8}`.
fn head8(job: &str) -> String {
    job.chars().take(8).collect()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A name or number in `$(( … ))`: digits (a `-` before them), or a name
/// or nothing, which reads as 0 (the daemon sends whole numbers).
fn arith(v: &str) -> i64 {
    let (neg, d) = v.strip_prefix('-').map_or((false, v), |r| (true, r));
    let n = if !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) {
        d.bytes().fold(0i64, |n, b| {
            n.wrapping_mul(10).wrapping_add(i64::from(b - b'0'))
        })
    } else {
        0
    };
    if neg {
        n.wrapping_neg()
    } else {
        n
    }
}

/// `[[ v =~ ^[0-9]+$ ]]`: a value from the daemon (and the node behind
/// it) reaches the shell's `$(( ))` only when it is digits.
fn digits(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())
}

/// The detached line the `INT` trap prints, then exit 130.
#[cfg(unix)]
pub mod int_trap {
    use std::sync::OnceLock;

    static LINE: OnceLock<Vec<u8>> = OnceLock::new();

    extern "C" fn on_int(_sig: i32) {
        if let Some(b) = LINE.get() {
            // SAFETY: write and _exit are async-signal-safe.
            unsafe {
                libc::write(2, b.as_ptr().cast(), b.len());
            }
        }
        unsafe { libc::_exit(130) };
    }

    /// Sets the trap (unless SIGINT was ignored when the command started:
    /// a shell can't trap it then either).
    pub fn set(line: String) {
        let _ = LINE.set(line.into_bytes());
        let handler = on_int as extern "C" fn(i32) as libc::sighandler_t;
        // SAFETY: installs a handler that only calls safe functions.
        let before = unsafe { libc::signal(libc::SIGINT, handler) };
        if before == libc::SIG_IGN {
            unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN) };
        }
    }

    /// `trap - INT`.
    pub fn clear() {
        // SAFETY: back to the default.
        unsafe { libc::signal(libc::SIGINT, libc::SIG_DFL) };
    }
}

#[cfg(not(unix))]
pub mod int_trap {
    pub fn set(_line: String) {}
    pub fn clear() {}
}

/// Follows `f.job` to its end: the exit the shell's would end with.
pub fn follow(cp: &mut Cp<'_>, f: &Follow) -> Exit {
    let job8 = head8(&f.job);
    int_trap::set(format!(
        "[{CLI_NAME} compute] detached; the job keeps running. {CLI_NAME} compute attach {job8} | {CLI_NAME} compute cancel {job8} --force\n"
    ));
    let exit = follow_on(cp, f, &job8);
    int_trap::clear();
    exit
}

fn follow_on(cp: &mut Cp<'_>, f: &Follow, job8: &str) -> Exit {
    let (mut cursor, mut query) = match &f.start {
        Start::Seq(n) => ("0".to_string(), Some(format!("since_seq={n}"))),
        Start::Cursor(c) => (c.clone(), None),
    };
    let (mut fails, mut lost) = (0u32, false);
    let (mut gen, mut last_q) = (String::new(), String::new());
    loop {
        let mut poll = 20i64;
        if let Some(deadline) = f.deadline {
            let left = deadline - now();
            if left <= 0 {
                out::stderr_line(&format!(
                    "{{\"error\":\"still_running\",\"jobs\":[\"{job8}\"],\"hint\":\"the wait limit passed; the job keeps going: {CLI_NAME} compute wait {job8} --follow\"}}"
                ));
                // `${_CP_DEADLINE_RC:-3}`: run hands the line to the shell
                // when it is set from outside; wait --follow sets its own.
                return f.deadline_exit;
            }
            poll = poll.min(left);
        }
        let mut q = format!("job={}&wait={poll}", quote(&f.job));
        if !gen.is_empty() {
            q.push_str(&format!("&generation={gen}"));
        }
        match query.take() {
            Some(x) => q.push_str(&format!("&{x}")),
            None => q.push_str(&format!("&cursor={cursor}")),
        }
        let split = match cp.send(false, LOGS, &q, false) {
            Ok(s) => s,
            Err(e) => return e,
        };
        if split.status == "000" {
            fails += 1;
            if fails == 1 {
                out::stderr_line(&format!(
                    "[{CLI_NAME} compute] lost the {} server; retrying (the job keeps running on the node)",
                    product_name!()
                ));
            }
            if fails > 60 {
                out::stderr_line(&format!(
                    "{{\"error\":\"lost_contact\",\"hint\":\"no answer from the {} server for 10 minutes; the job may still be running: {CLI_NAME} compute attach {}\"}}",
                    product_name!(),
                    f.job
                ));
                return Exit::Refused;
            }
            lost = true;
            std::thread::sleep(LOST_PAUSE);
            continue;
        }
        if split.status != "200" {
            return fail(&split.status, &split.body);
        }
        fails = 0;
        let st = match poll_reader(&split.body, f.raw, lost, f.as_json) {
            Ok(st) => st,
            Err(exit) => return exit,
        };
        cursor = st.cursor.clone();
        lost = false;
        if gen.is_empty() {
            gen = st.fgen.clone();
        }
        let ended = matches!(
            st.state.as_str(),
            "done" | "failed" | "cancelled" | "timeout" | "interrupted" | "unknown"
        );
        if st.end && !st.jgen.is_empty() && st.jgen != gen && !ended && !st.moved.is_empty() {
            // A move is not a failure and uses no retry.
            out::stderr_line(&format!(
                "[{CLI_NAME} compute] moved: {}; now on {} (not a failure, no retry used)",
                st.moved, st.node
            ));
            gen = st.jgen.clone();
            cursor = "0".to_string();
            last_q.clear();
            continue;
        }
        if st.end && !st.jgen.is_empty() && st.jgen != gen && !ended {
            let lastfail = if st.lastfail.is_empty() {
                "?"
            } else {
                &st.lastfail
            };
            // The attempt before this one: `?` when the attempt isn't digits.
            let prev = if digits(&st.attempt) {
                arith(&st.attempt).wrapping_sub(1).to_string()
            } else {
                "?".to_string()
            };
            out::stderr_line(&format!(
                "[{CLI_NAME} compute] attempt {prev} hit an infrastructure failure ({lastfail}); retrying as attempt {} on {}",
                st.attempt,
                st.node
            ));
            gen = st.jgen.clone();
            cursor = "0".to_string();
            last_q.clear();
            continue;
        }
        let here = format!("{}|{}", st.qpos, st.qreason);
        if st.state == "queued" && !st.qreason.is_empty() && here != last_q {
            if st.qline.is_empty() {
                out::stderr_line(&format!(
                    "[{CLI_NAME} compute] queued on {}: position {} ({})",
                    st.node, st.qpos, st.qreason
                ));
            } else {
                out::stderr_line(&format!("[{CLI_NAME} compute] {}", st.qline));
            }
            last_q = here;
        }
        if st.end {
            int_trap::clear();
            return end(cp, f, &st);
        }
    }
}

/// The job ended: its line and exit.
fn end(cp: &mut Cp<'_>, f: &Follow, st: &State) -> Exit {
    if f.as_json {
        if let Err(e) = print_result(cp, &f.job) {
            return e;
        }
    }
    let took = if digits(&st.started) && digits(&st.ended) {
        let d = arith(&st.ended).wrapping_sub(arith(&st.started));
        format!(" after {}m{}s", d / 60, d % 60)
    } else {
        String::new()
    };
    if st.state == "done" {
        if !st.code.is_empty() {
            out::stderr_line(&format!(
                "[{CLI_NAME} compute] exit {}{took} \u{b7} {CLI_NAME} compute logs {} --failures",
                st.code, st.short
            ));
            return Exit::from_curl((arith(&st.code) & 255) as i32);
        }
        // A signal that isn't digits is none: 128, as with no signal.
        let sig = if digits(&st.sig) { st.sig.as_str() } else { "" };
        out::stderr_line(&format!(
            "[{CLI_NAME} compute] killed by signal {sig}{took}"
        ));
        return Exit::from_curl((128i64.wrapping_add(arith(sig)) & 255) as i32);
    }
    let reason = if st.reason.is_empty() {
        String::new()
    } else {
        format!(" ({})", st.reason)
    };
    out::stderr_line(&format!(
        "[{CLI_NAME} compute] job {} {}{reason}{took}",
        st.short, st.state
    ));
    let _ = out::write_flushed(&mut std::io::stderr().lock(), st.err.as_bytes());
    Exit::Refused
}

/// `_compute_print_result <job>`: the job's `result` as indented JSON.
pub fn print_result(cp: &mut Cp<'_>, job: &str) -> Result<(), Exit> {
    let body = cp.call(false, GET, &format!("job={}", quote(job)), false)?;
    let r = load(&body, 1).and_then(|v| {
        let res = p::get(&v, "result").map_err(|e| Raise::at(1, e))?;
        p::dumps(&res, false, Some(2)).map_err(|e| Raise::at(1, e))
    });
    match r {
        Ok(text) => match out::stdout(&(text + "\n")) {
            Ok(()) => Ok(()),
            Err(e) => Err(out::after_stdout_error(&e)),
        },
        Err(e) => Err(stopped(String::new(), &e).print()),
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

/// One poll's Python: the chunks written as they come, then the fields.
/// A raise prints its traceback and ends the command (exit 1).
pub fn poll_reader(body: &[u8], raw: bool, lost: bool, to_err: bool) -> Result<State, Exit> {
    let r = (|| -> Result<State, Raise> {
        let at = |l: u32| move |e: String| Raise::at(l, e);
        let v = load(body, line!("v = json.load"))?;
        let l = line!("chunks = v.get(\"chunks\"");
        let chunks = p::get_or(&v, "chunks", Value::Array(vec![])).map_err(at(l))?;
        if lost && p::truthy(&chunks) {
            let l = line!("sys.stderr.write(\"[k2 compute] reattached");
            let seq = p::at(&chunks, 0)
                .and_then(|c| p::idx(&c, "seq"))
                .map_err(at(l))?;
            let text = p::pct(
                &format!("[{CLI_NAME} compute] reattached at seq %d\n"),
                &[seq],
            )
            .map_err(at(l))?;
            let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
        }
        for c in p::iter(&chunks).map_err(at(line!("for c in chunks")))? {
            let l = line!("data = base64");
            let data = p::idx(&c, "data")
                .and_then(|d| crate::families::mail::reads::b64decode(&d))
                .map_err(at(l))?;
            let data = if raw { data } else { super::logs::strip(&data) };
            let l = line!("if c[\"stream\"]");
            let stream = p::idx(&c, "stream").map_err(at(l))?;
            if p::eq_s(&stream, "out") && !to_err {
                if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &data) {
                    return Err(Raise::at(
                        line!("sys.stdout.buffer.write(data)"),
                        format!("BrokenPipeError: {e}"),
                    ));
                }
            } else {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), &data);
            }
        }
        let l = line!("j = v.get(\"job\"");
        let j = p::get_or(&v, "job", Value::Object(vec![])).map_err(at(l))?;
        let l = line!("q = v.get(\"queue\")");
        let q = p::get(&v, "queue")
            .map(|x| p::or(x, Value::Object(vec![])))
            .map_err(at(l))?;
        let l = line!("fa = (");
        let ctl = p::get(&j, "ctl")
            .map(|x| p::or(x, Value::Object(vec![])))
            .map_err(at(l))?;
        let mut fa = p::get(&ctl, "failedAttempts")
            .map(|x| p::or(x, Value::Array(vec![Value::Object(vec![])])))
            .and_then(|x| p::at(&x, -1))
            .map_err(at(l))?;
        // The record of the generation this answer read, once it ended
        // without ending the job: a move (no retry used) or a retry.
        let l = line!("rec = next(");
        let mut rec = None;
        let attempts = p::get(&v, "attempts").map_err(at(l))?;
        for a in p::iter_or_empty(attempts).map_err(at(l))? {
            if p::is_dict(&a)
                && !p::truthy(&p::get(&a, "current").map_err(at(l))?)
                && p::eq(
                    &p::get(&a, "generation").map_err(at(l))?,
                    &p::get(&v, "generation").map_err(at(l))?,
                )
            {
                rec = Some(a);
                break;
            }
        }
        let mut moved = String::new();
        if let Some(rec) = rec {
            let l = line!("if rec.get(\"moved\")");
            let said = p::get(&rec, "moved").map_err(at(l))?;
            let l = line!("elif \"moved\" not in rec");
            if p::truthy(&said) {
                moved = p::str_of(&said);
            } else if !p::has(&rec, "moved").map_err(at(l))?
                && p::eq(
                    &p::get(&rec, "attempt").map_err(at(l))?,
                    &p::get(&j, "attempt").map_err(at(l))?,
                )
            {
                // An older daemon: the attempt number stayed, so a move.
                let l = line!("moved = str(rec.get(\"next\")");
                let next = p::get(&rec, "next").map_err(at(l))?;
                moved = p::str_of(&p::or(next, s("placed again")));
            } else {
                let l = line!("fa = {\"reason\"");
                fa = super::writes::object(vec![
                    ("reason", p::get(&rec, "reason").map_err(at(l))?),
                    ("state", p::get(&rec, "state").map_err(at(l))?),
                ]);
            }
        }
        // `IFS= read -r moved` of `moved.replace("\0", "").replace("\n",
        // " ")`: no NUL and no newline is left.
        let moved: String = moved
            .replace('\n', " ")
            .chars()
            .filter(|c| *c != '\0')
            .collect();
        let g = |x: &Value, k: &str, l: u32| p::get(x, k).map_err(at(l));
        let (r1, r2, r3, r4) = (
            line!("s(v.get(\"cursor\", 0))"),
            line!("s(j.get(\"signal\"))"),
            line!("s(j.get(\"startedAt\"))"),
            line!("s(v.get(\"generation\"))"),
        );
        let cursor = field(&p::get_or(&v, "cursor", p::int(0)).map_err(at(r1))?);
        let end = p::truthy(&g(&v, "end", r1)?);
        let state = field(&g(&j, "state", r1)?);
        let code = field(&g(&j, "exitCode", r1)?);
        let sig = field(&g(&j, "signal", r2)?);
        let reason = field(&g(&j, "reason", r2)?);
        let short = field(&g(&j, "short", r2)?);
        let node = field(&g(&j, "node", r2)?);
        let started = field(&g(&j, "startedAt", r3)?);
        let ended = field(&g(&j, "endedAt", r3)?);
        let qpos = field(&g(&q, "position", r3)?);
        let qreason = field(&p::or(g(&q, "waitReason", r3)?, g(&q, "reason", r3)?));
        let fgen = field(&g(&v, "generation", r4)?);
        let jgen = field(&g(&j, "generation", r4)?);
        let attempt = field(&g(&j, "attempt", r4)?);
        let lastfail = field(&p::or(g(&fa, "reason", r4)?, g(&fa, "state", r4)?));
        let l = line!("open(os.environ[\"K2_CP_STATE\"] + \".q\"");
        let line = p::or(g(&q, "line", l)?, s(""));
        let qline = p::replace(&line, "\n", " ").map_err(at(l))?;
        // `IFS= read -r qline`: the first line, NULs dropped.
        let qline: String = qline
            .split('\n')
            .next()
            .unwrap_or("")
            .chars()
            .filter(|c| *c != '\0')
            .collect();
        let l = line!("err = {");
        let short_v = g(&j, "short", l + 1)?;
        let err = super::writes::object(vec![
            ("error", g(&j, "state", l)?),
            ("reason", g(&j, "reason", l)?),
            ("detail", g(&j, "detail", l)?),
            (
                "hint",
                str_value(
                    &p::pct(
                        concat!(
                            cli_name!(),
                            " compute get %s --json; ",
                            cli_name!(),
                            " compute retry %s"
                        ),
                        &[short_v.clone(), short_v],
                    )
                    .map_err(at(l + 1))?,
                ),
            ),
        ]);
        let err = super::writes::dumps(&err) + "\n";
        Ok(State {
            cursor,
            end,
            state,
            code,
            sig,
            reason,
            short,
            node,
            started,
            ended,
            qpos,
            qreason,
            fgen,
            jgen,
            attempt,
            lastfail,
            moved,
            qline,
            err,
        })
    })();
    r.map_err(|e| stopped(String::new(), &e).print())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines() {
        assert_eq!(line!("v = json.load"), 3);
        assert_eq!(line!("for c in chunks"), 10);
        assert_eq!(line!("fa = ("), 25);
        assert_eq!(line!("rec = next("), 29);
        assert_eq!(line!("fa = {\"reason\""), 38);
        assert_eq!(line!("err = {"), 51);
    }

    #[test]
    fn numbers_as_bash_reads_them() {
        assert_eq!(arith("42"), 42);
        assert_eq!(arith("-3"), -3);
        assert_eq!(arith(""), 0);
        assert_eq!(arith("x"), 0);
    }

    #[test]
    fn only_digits_reach_the_arithmetic() {
        for yes in ["0", "9", "0042"] {
            assert!(digits(yes), "{yes}");
        }
        for no in [
            "",
            "-3",
            "x",
            "BASH_VERSINFO[$(true)]",
            " 9",
            "9 ",
            "+9",
            "\u{663}",
        ] {
            assert!(!digits(no), "{no:?}");
        }
    }

    #[test]
    fn one_poll() {
        // "aGk=" is "hi"; "G1szMW1yZWQbWzBt" is ESC[31m red ESC[0m.
        let st = poll_reader(
            br#"{"cursor":"c:9","end":true,"generation":2,"chunks":[],"job":{"state":"done","exitCode":7,"short":"abcd1234","node":"n|1","startedAt":100,"endedAt":225,"generation":2,"attempt":1,"detail":"x"},"queue":{"line":"a\nb"}}"#,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            (
                st.cursor.as_str(),
                st.end,
                st.code.as_str(),
                st.node.as_str()
            ),
            ("c:9", true, "7", "n1")
        );
        assert_eq!(st.qline, "a b");
        assert_eq!(
            st.err,
            format!("{{\"error\": \"done\", \"reason\": null, \"detail\": \"x\", \"hint\": \"{CLI_NAME} compute get abcd1234 --json; {CLI_NAME} compute retry abcd1234\"}}\n")
        );
    }
}
