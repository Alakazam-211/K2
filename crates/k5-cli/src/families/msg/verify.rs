//! `k5 msg verify <code> [--text "..." | --text-file <f> | --text-file -]
//! [--json]`: who sent a message, and whether its text is the one
//! delivered, as the shell's `cmd_msg_verify` (`cli/src/msg-verify.sh`)
//! asks and prints it.
//!
//! The words, in order: `--json`; `--text <t>`; `--text-file <f>` (`-`
//! reads stdin; a file is read as `$(cat …)` holds it: NULs and trailing
//! newlines gone); each with its usage line when the value is missing,
//! `no file <f>` when it isn't a file, an unknown flag, a second code, and
//! no code; each of those exit 2 with nothing sent. With a text, the
//! query carries the SHA-256s of the text as given, without its label
//! line and without the "> " quote marks ([`hashes`], the shell's
//! `_msg_verify_hashes`). Then `cli_request /cli/msg/verify` over TCP on
//! the left of a pipe into the reader ([`reader`]).

use super::super::{pipefail, py_space, raised, request_piped_tcp, Raise, Shown};
use super::py::{self, concat, dumps, dumps_indent, eq, get_or_null, index, s, Printed};
use crate::exit::Exit;
use crate::out;
use crate::out::json::Value;
use crate::out::py::{str_of, truthy, type_name};
use crate::prelude::Closing;
use crate::transport::encode::quote;
use k2_client::cli_name;
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;

/// The route.
pub const ENDPOINT: &str = "/cli/msg/verify";

const P: &str = concat!(cli_name!(), " msg verify: ");

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub code: String,
    pub text: Option<String>,
    pub json: bool,
}

/// Where a `--text-file -` reads from.
pub trait Stdin {
    fn read_all(&mut self) -> Vec<u8>;
}

struct Real;

impl Stdin for Real {
    fn read_all(&mut self) -> Vec<u8> {
        let mut v = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut v);
        v
    }
}

/// `$(cat …)`: NULs and trailing newlines gone.
fn held(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&super::send_once::held(bytes)).into_owned()
}

/// The words, read as `cmd_msg_verify`'s loop reads them: the ask, or
/// stderr and the exit. A file that can't be read is `cat`'s line and
/// exit 1, as `text="$(cat -- "$2")"` ends under `set -e`.
pub fn parse(words: &[&str], stdin: &mut dyn Stdin) -> Result<Ask, (String, Exit)> {
    let usage = |line: &str| Err((format!("{P}{line}"), Exit::Usage));
    let mut a = Ask {
        code: String::new(),
        text: None,
        json: false,
    };
    let mut i = 0;
    while i < words.len() {
        let next = words.get(i + 1).copied();
        match words[i] {
            "--json" => a.json = true,
            "--text" => {
                let Some(t) = next else {
                    return usage("--text needs the text");
                };
                a.text = Some(t.to_string());
                i += 1;
            }
            "--text-file" => {
                let Some(f) = next else {
                    return usage("--text-file needs a file (- for stdin)");
                };
                if f == "-" {
                    a.text = Some(held(&stdin.read_all()));
                } else if Path::new(f).is_file() {
                    match std::fs::read(f) {
                        Ok(b) => a.text = Some(held(&b)),
                        Err(e) => return Err((format!("cat: {f}: {}", why(&e)), Exit::Error)),
                    }
                } else {
                    return usage(&format!("no file {f}"));
                }
                i += 1;
            }
            w if w.starts_with('-') => {
                return usage(&format!(
                    concat!("unknown flag '{}' (", cli_name!(), " msg verify --help)"),
                    w
                ))
            }
            w => {
                if !a.code.is_empty() {
                    return usage(concat!(
                        "one code only (",
                        cli_name!(),
                        " msg verify --help)"
                    ));
                }
                a.code = w.to_string();
            }
        }
        i += 1;
    }
    if a.code.is_empty() {
        return Err((
            concat!(
                "Usage: ",
                cli_name!(),
                " msg verify <code> [--text-file f]  (the m_… or p_… in the message's label)"
            )
            .to_string(),
            Exit::Usage,
        ));
    }
    Ok(a)
}

/// The OS's words for an error, as `cat` prints them.
fn why(e: &std::io::Error) -> String {
    let text = e.to_string();
    match text.rsplit_once(" (os error ") {
        Some((words, _)) => words.to_string(),
        None => text,
    }
}

/// `k5 msg verify …`. `None`: the shell's.
pub fn run(args: &[OsString], words: &[&str], hook_sock: Option<&Path>) -> Option<Exit> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let mut client = match super::client(args, hook_sock)? {
        Ok(c) => c,
        Err(exit) => return Some(exit),
    };
    let ask = match parse(words, &mut Real) {
        Ok(a) => a,
        Err((line, exit)) => {
            out::stderr_line(&line);
            return Some(exit);
        }
    };
    let mut q = format!("code={}", quote(&ask.code));
    if let Some(t) = &ask.text {
        q.push_str("&text_sha256=");
        q.push_str(&hashes(&ask.code, t));
    }
    let mut closing = Closing::default();
    let piped = request_piped_tcp(&mut client, &mut closing, ENDPOINT, &[q.as_str()]);
    let shown = reader(&piped.body, ask.json, now_local);
    let printed = shown.print();
    let exit = if printed == shown.exit {
        pipefail(piped.exit, shown.exit)
    } else {
        printed
    };
    closing.flush(&mut client, exit);
    Some(exit)
}

/// `str.rstrip()`.
fn rstrip(t: &str) -> &str {
    t.trim_end_matches(py_space)
}

fn norm(t: &str) -> String {
    rstrip(&t.replace("\r\n", "\n").replace('\r', "\n")).to_string()
}

fn unquote(t: &str) -> String {
    norm(t)
        .split('\n')
        .map(|line| {
            if let Some(rest) = line.strip_prefix("> ") {
                rest
            } else if line == ">" {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn unlabel(t: &str, code: &str) -> String {
    let n = norm(t);
    let mut lines: Vec<&str> = n.split('\n').collect();
    let first: Vec<char> = lines[0].chars().collect();
    let first_s = lines[0];
    if first_s.contains(code) && first_s.trim_start_matches(py_space).starts_with('[') {
        let code_chars: Vec<char> = code.chars().collect();
        let at = find(&first, &code_chars, 0).unwrap_or(0);
        let rest: String = match find(&first, &[']'], at) {
            Some(end) => first[end + 1..]
                .iter()
                .collect::<String>()
                .trim_start_matches(py_space)
                .to_string(),
            None => String::new(),
        };
        let tail: Vec<&str> = lines.split_off(1);
        let mut out: Vec<String> = Vec::new();
        if !rest.is_empty() {
            out.push(rest);
        }
        out.extend(tail.iter().map(|l| l.to_string()));
        return out.join("\n");
    }
    lines.join("\n")
}

/// `str.find(sub, start)` in code points.
fn find(hay: &[char], sub: &[char], start: usize) -> Option<usize> {
    if sub.is_empty() {
        return (start <= hay.len()).then_some(start);
    }
    (start..hay.len().saturating_sub(sub.len() - 1)).find(|&i| hay[i..i + sub.len()] == *sub)
}

/// `_msg_verify_hashes <code>` on `raw`: the SHA-256 of each reading, in
/// order, without repeats, joined by commas.
pub fn hashes(code: &str, raw: &str) -> String {
    let h = |t: &str| k2_client::release_sums::sha256_hex(norm(t).as_bytes());
    let mut seen: Vec<String> = Vec::new();
    let unl = unlabel(raw, code);
    for t in [raw.to_string(), unquote(raw), unl.clone(), unquote(&unl)] {
        let d = h(&t);
        if !seen.contains(&d) {
            seen.push(d);
        }
    }
    seen.join(",")
}

/// `time.strftime("%Y-%m-%d %H:%M:%S %Z", time.localtime(at))`, or the
/// exception's last line.
pub type Clock = fn(i64) -> Option<String>;

/// The verify reader on what the call printed. `clock` writes a time as
/// the shell's Python does.
pub fn reader(raw: &[u8], json: bool, clock: Clock) -> Shown {
    let mut p = Printed::default();
    let Some(d) = py::loads(raw) else {
        return Shown::err(py::daemon_answered(raw) + "\n", Exit::Error);
    };
    match lines(&d, json, clock, &mut p) {
        Ok(code) => Shown {
            stdout: p.stdout,
            stderr: p.stderr,
            exit: Exit::from_curl(code),
        },
        Err(r) => raised(p.stdout, p.stderr, &r),
    }
}

fn mismatch(content: &Value) -> i32 {
    i32::from(eq(content, &s("mismatch")))
}

fn lines(d: &Value, json: bool, clock: Clock, p: &mut Printed) -> Result<i32, Raise> {
    let exits = [("not_found", 4), ("usage", 2), ("forbidden", 3)];
    if let Some(code) = py::error_branch(d, p, &exits, false)? {
        return Ok(code);
    }
    let content = get_or_null(d, "content", 14)?;
    if json {
        p.line(&dumps_indent(d));
        return Ok(mismatch(&content));
    }
    let sender = py::or(get_or_null(d, "sender", 18)?, Value::Object(Vec::new()));
    let who = [("address", 19), ("handle", 19), ("label", 19)]
        .iter()
        .try_fold(Value::Null, |acc, (k, line)| {
            if truthy(&acc) {
                Ok(acc)
            } else {
                get_or_null(&sender, k, *line)
            }
        })?;
    let who = py::or(who, s("?"));
    let mut extra = Vec::new();
    let handle = get_or_null(&sender, "handle", 21)?;
    if truthy(&handle) && !eq(&handle, &who) {
        extra.push(concat("workspace ", index(&sender, "handle", 21)?, 21)?);
    }
    if truthy(&get_or_null(&sender, "fingerprint", 22)?) {
        extra.push(concat("key ", index(&sender, "fingerprint", 22)?, 22)?);
    }
    p.line(&format!(
        "code      {} ({})",
        str_of(&get_or_null(d, "code", 23)?),
        str_of(&get_or_null(d, "kind", 23)?)
    ));
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!("  [{}]", extra.join(", "))
    };
    p.line(&format!(
        "from      {}  via {}{extra}",
        str_of(&who),
        str_of(&get_or_null(d, "via", 24)?)
    ));
    if truthy(&get_or_null(d, "actor", 25)?) {
        p.line(&format!("actor     {}", str_of(index(d, "actor", 25)?)));
    }
    let ob = get_or_null(d, "onBehalfOf", 26)?;
    if truthy(&ob) {
        p.line(&format!(
            "for       {} (app-asserted, not verified)",
            str_of(&get_or_null(&ob, "value", 27)?)
        ));
    }
    let t = py::or(get_or_null(d, "target", 28)?, Value::Object(Vec::new()));
    let token = get_or_null(&t, "token", 29)?;
    let to = if truthy(&token) {
        token
    } else {
        py::or(get_or_null(&t, "workspaceId", 29)?, s("?"))
    };
    p.line(&format!("to        {}", str_of(&to)));
    let at = py::or(get_or_null(d, "at", 30)?, Value::Int("0".into()));
    p.line(&format!("when      {}", when(&at, clock)?));
    let typed = if truthy(&get_or_null(d, "delivered", 32)?) {
        "yes"
    } else {
        "no (the delivery did not reach the agent)"
    };
    p.line(&format!("typed in  {typed}"));
    let badge = get_or_null(d, "badge", 33)?;
    if truthy(&badge) && !eq(&badge, &get_or_null(d, "code", 33)?) {
        p.line(&format!("badge     {}", str_of(index(d, "badge", 33)?)));
    }
    let landed = get_or_null(d, "landed", 34)?;
    if landed != Value::Null {
        p.line(&format!("landed    {}", dumps(&landed)));
    }
    let hint = match d {
        Value::Object(pairs) => super::super::get(pairs, "contentHint")
            .cloned()
            .unwrap_or_else(|| s("")),
        _ => s(""),
    };
    p.line(&format!(
        "content   {}: {}",
        str_of(&content),
        str_of(&hint)
    ));
    Ok(mismatch(&content))
}

/// Line 31: `time.localtime(at)` as the time `clock` writes.
fn when(at: &Value, clock: Clock) -> Result<String, Raise> {
    const LINE: u32 = 31;
    let overflow = || {
        Raise::at(
            LINE,
            "OverflowError: timestamp out of range for platform time_t",
        )
    };
    let secs: i64 = match at {
        Value::Bool(b) => i64::from(*b),
        Value::Int(d) => d.parse().map_err(|_| overflow())?,
        Value::Float(f) if f.is_nan() => {
            return Err(Raise::at(
                LINE,
                "ValueError: Invalid value NaN (not a number)",
            ))
        }
        Value::Float(f) => {
            let fl = f.floor();
            if !(-9.2e18..9.2e18).contains(&fl) {
                return Err(overflow());
            }
            fl as i64
        }
        other => {
            return Err(Raise::at(
                LINE,
                format!(
                    "TypeError: '{}' object cannot be interpreted as an integer",
                    type_name(other)
                ),
            ))
        }
    };
    clock(secs).ok_or_else(overflow)
}

#[cfg(unix)]
extern "C" {
    /// POSIX `tzset`: reads `TZ`, as Python's `time` module does when it
    /// loads.
    fn tzset();
}

/// This machine's local time for `secs`, `%Y-%m-%d %H:%M:%S %Z`.
#[cfg(unix)]
pub fn now_local(secs: i64) -> Option<String> {
    // time_t is narrower than i64 on some targets.
    #[allow(clippy::useless_conversion)]
    let t: libc::time_t = secs.try_into().ok()?;
    // SAFETY: `tm` is filled by localtime_r before it is read; strftime
    // writes at most `buf.len()` bytes and returns how many.
    unsafe {
        tzset();
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return None;
        }
        let mut buf = [0u8; 128];
        let fmt = c"%Y-%m-%d %H:%M:%S %Z";
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr(), &tm);
        Some(String::from_utf8_lossy(&buf[..n]).into_owned())
    }
}

/// Elsewhere: UTC.
#[cfg(not(unix))]
pub fn now_local(secs: i64) -> Option<String> {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    Some(format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    ))
}
