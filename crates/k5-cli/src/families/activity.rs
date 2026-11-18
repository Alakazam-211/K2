//! `k5 activity [<limit>] [--limit N] [--agent <name>] [--json]`: the
//! cross-workspace activity feed, as the shell CLI's `cmd_activity` (its
//! `cmd_feed`, `cli/src/wiki.sh`) prints it.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    the 0.40.4 rename line, and the two gates ([`crate::prelude`]).
//!    `activity` is an open tool, so no passport check comes after them.
//! 2. The words ([`parse`]): `--json`, `--agent <name>` and `--limit <N>`
//!    (each needs the next word, else its line and exit 2), any other word
//!    starting with `-` is the unknown-flag line (exit 2), and any other
//!    word is the limit. A limit that is not all digits (an empty one too)
//!    is its line, exit 2. Nothing is sent after any of these.
//! 3. `_cli_use_passport_project` ([`super::use_passport_project`]): with a
//!    passport and no `PROJECT_PATH`, the workspace is the one the daemon
//!    says the passport belongs to.
//! 4. One `cli_request /cli/feed limit=<N>[&agent=<escaped>]` over TCP (the
//!    shell calls it directly: never the cell socket), on the left of a
//!    pipe into the shell's Python ([`super::request_piped_tcp`]).
//! 5. The Python ([`reader`]): the daemon's error on stderr (exit 1), the
//!    JSON as it came with `--json` (`json.dumps(d, indent=2)`), else one
//!    line per event with its local date and time, or `No activity.`. An
//!    answer that is not JSON is its first 200 characters, as Python's
//!    `%r` writes them, exit 1.
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]), with the
//!    pipeline's code.
//!
//! A request with no answer is the transport's line, and the command then
//! dies of SIGPIPE, as for [`super::inbox`].
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`): a
//! command line with `-h` or `--help` ([`crate::front`]), a word that is
//! not UTF-8, a port that is set but is no port curl takes, and no
//! `python3` on `PATH`.
//!
//! The time of an event is `time.localtime`'s, by the C library's
//! `localtime_r` and `strftime`, as Python's is: the same `TZ`, the same
//! text. Where Python raises on an event's `at` (a string, a list or an
//! object; a number past the platform's `time_t`), this prints a Python
//! 3.12 traceback; the exception's text for a number out of range may
//! differ by Python version. Bytes that are not UTF-8 and half of a
//! surrogate pair are as [`super::inbox`] says.

use super::{
    curl_port, pipefail, py_get, py_iter, raised, request_piped_tcp, use_passport_project, Raise,
    Shown,
};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use std::ffi::OsString;
use std::path::Path;

/// The route.
pub const ENDPOINT: &str = "/cli/feed";
/// `--agent` with nothing after it: exit 2.
pub const AGENT_NEEDS_A_NAME: &str = "Error: --agent needs a name (k2 activity --help)";
/// `--limit` with nothing after it: exit 2.
pub const LIMIT_NEEDS_A_NUMBER: &str = "Error: --limit needs a number (k2 activity --help)";
/// No events.
pub const NO_ACTIVITY: &str = "No activity.";
/// The Python on an answer that is not JSON, before its `%r`.
pub const UNUSABLE: &str = concat!(
    "Error: the ",
    k2_client::product_name!(),
    " daemon gave no usable answer: "
);
/// The lines of the shell's Python that raise (`tests` pins them).
pub const PY_FOR: u32 = 20;
pub const PY_TS: u32 = 21;

/// A flag the shell doesn't take: exit 2.
pub fn unknown_flag(word: &str) -> String {
    format!("Error: unknown flag '{word}' (k2 activity --help)")
}

/// A limit that is not a number: exit 2.
pub fn bad_limit(limit: &str) -> String {
    format!("Error: the limit is a number, got '{limit}' (k2 activity --help)")
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub limit: String,
    pub agent: String,
    pub json: bool,
}

/// The words after `activity`, as `cmd_feed`'s loop reads them: the ask,
/// or the usage line that stops it (exit 2). `None`: `-h` or `--help`,
/// the shell's.
pub fn parse(words: &[&str]) -> Option<Result<Ask, String>> {
    let mut ask = Ask {
        limit: "20".into(),
        agent: String::new(),
        json: false,
    };
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "-h" | "--help" => return None,
            "--json" => {
                ask.json = true;
                i += 1;
            }
            "--agent" | "--limit" => {
                let Some(value) = words.get(i + 1) else {
                    return Some(Err(if *w == "--agent" {
                        AGENT_NEEDS_A_NAME.to_string()
                    } else {
                        LIMIT_NEEDS_A_NUMBER.to_string()
                    }));
                };
                if *w == "--agent" {
                    ask.agent = (*value).to_string();
                } else {
                    ask.limit = (*value).to_string();
                }
                i += 2;
            }
            w if w.starts_with('-') => return Some(Err(unknown_flag(w))),
            w => {
                ask.limit = w.to_string();
                i += 1;
            }
        }
    }
    if ask.limit.is_empty() || !ask.limit.bytes().all(|b| b.is_ascii_digit()) {
        return Some(Err(bad_limit(&ask.limit)));
    }
    Some(Ok(ask))
}

/// `k5 activity ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let ask = parse(&words)?;
    if !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if prelude::gate(&settings).is_none() {
        // A port curl won't take is the shell's.
        settings.port_text.as_deref().and_then(curl_port)?;
    }
    Some(run_with(args, ask, settings))
}

fn run_with(args: &[OsString], ask: Result<Ask, String>, mut settings: Settings) -> Exit {
    if let Some(notice) = prelude::rename_notice(Path::new(&settings.project)) {
        out::stderr_line(notice);
    }
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let ask = match ask {
        Ok(a) => a,
        Err(line) => {
            out::stderr_line(&line);
            return Exit::Usage;
        }
    };
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    use_passport_project(&mut client);
    let mut q = format!("limit={}", ask.limit);
    if !ask.agent.is_empty() {
        q.push_str("&agent=");
        q.push_str(&quote(&ask.agent));
    }
    let mut closing = Closing::default();
    let piped = request_piped_tcp(&mut client, &mut closing, ENDPOINT, &[q.as_str()]);
    let exit = pipefail(piped.exit, reader(&piped.body, ask.json, now()).print());
    closing.flush(&mut client, exit);
    exit
}

/// `time.time()`'s whole seconds, for an event whose `at` is `null`.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The shell's Python on the answer it reads. `json`: `FEED_JSON=1`.
/// `now`: the time an `at` of `null` stands for.
pub fn reader(body: &[u8], as_json: bool, now: i64) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let Ok(d) = json::parse(&raw) else {
        let head: String = raw.chars().take(200).collect();
        return Shown::err(
            format!("{UNUSABLE}{}\n", py::repr(&Value::Str(Text::from(head)))),
            Exit::Error,
        );
    };
    let unexpected = super::s("unexpected answer");
    let err = match &d {
        Value::Object(pairs) => super::get(pairs, "error"),
        _ => Some(&unexpected),
    };
    if let Some(err) = err.filter(|e| **e != Value::Null) {
        let hint = match err {
            Value::Object(e) => [super::get(e, "hint"), super::get(e, "code")]
                .into_iter()
                .flatten()
                .find(|v| py::truthy(v))
                .cloned()
                .unwrap_or_else(|| super::s(&py::str_of(err))),
            other => super::s(&py::str_of(other)),
        };
        return Shown::err(format!("Error: {}\n", py::str_of(&hint)), Exit::Error);
    }
    if as_json {
        let text = json::to_string(
            &d,
            &Options {
                indent: Some(2),
                ..Options::default()
            },
        )
        .unwrap_or_default();
        return Shown::out(format!("{text}\n"));
    }
    let mut stdout = String::new();
    match events(&d, now, &mut stdout) {
        Ok(()) => Shown::out(stdout),
        Err(r) => raised(stdout, String::new(), &r),
    }
}

/// Lines 17 on: `No activity.`, then each event's line.
fn events(d: &Value, now: i64, stdout: &mut String) -> Result<(), Raise> {
    let none = Value::Array(Vec::new());
    let items = py_get(d, "feed", 17)?.unwrap_or(&none);
    if !py::truthy(items) {
        stdout.push_str(NO_ACTIVITY);
        stdout.push('\n');
    }
    for e in py_iter(items, PY_FOR)? {
        let at = py_get(&e, "at", PY_TS)?
            .cloned()
            .unwrap_or(Value::Int("0".into()));
        let ts = stamp(&at, now).map_err(|x| Raise::at(PY_TS, x))?;
        let or = |key: &str, default: &str| -> Value {
            py_get(&e, key, PY_TS)
                .ok()
                .flatten()
                .filter(|v| py::truthy(v))
                .cloned()
                .unwrap_or_else(|| super::s(default))
        };
        let agent = or("agent", "-");
        let etype = py_get(&e, "type", PY_TS)?
            .cloned()
            .unwrap_or_else(|| super::s("?"));
        let summary = or("summary", "");
        let frm = or("from", "");
        let to = or("to", "");
        let route = if py::truthy(&frm) && py::truthy(&to) {
            format!(" ({}\u{2192}{})", py::str_of(&frm), py::str_of(&to))
        } else {
            String::new()
        };
        stdout.push_str(&format!(
            "  [{ts}] {}: {}{route} {}\n",
            py::str_of(&agent),
            py::str_of(&etype),
            py::str_of(&summary)
        ));
    }
    Ok(())
}

/// `time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(at))`, or the
/// exception's line. `now`: what `None` stands for.
pub fn stamp(at: &Value, now: i64) -> Result<String, String> {
    const RANGE: &str = "OverflowError: timestamp out of range for platform time_t";
    let secs: i64 = match at {
        Value::Null => now,
        Value::Bool(b) => i64::from(*b),
        Value::Int(digits) => digits.parse().map_err(|_| RANGE.to_string())?,
        Value::Float(f) if f.is_nan() => {
            return Err("ValueError: Invalid value NaN (not a number)".into())
        }
        Value::Float(f) => {
            let w = f.floor();
            // i64::MAX as f64 rounds up to 2^63, which is out of range.
            if w < -9.223_372_036_854_776e18 || w >= 9.223_372_036_854_776e18 {
                return Err(RANGE.into());
            }
            w as i64
        }
        other => {
            return Err(format!(
                "TypeError: '{}' object cannot be interpreted as an integer",
                py::type_name(other)
            ))
        }
    };
    local_text(secs)
}

/// The local time of `secs` as `%Y-%m-%d %H:%M:%S`, by `localtime_r` and
/// `strftime`; `OSError` where `localtime_r` fails.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
fn local_text(secs: i64) -> Result<String, String> {
    // `time_t` is 64 bits on every platform the binary ships for.
    let t = secs as libc::time_t;
    // SAFETY: tzset reads TZ; localtime_r and strftime write only into the
    // buffers given, each sized as passed.
    unsafe {
        extern "C" {
            fn tzset();
        }
        tzset();
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            let e = std::io::Error::last_os_error();
            let n = e.raw_os_error().unwrap_or(0);
            let text = e.to_string();
            let msg = text
                .strip_suffix(&format!(" (os error {n})"))
                .unwrap_or(&text);
            return Err(format!("OSError: [Errno {n}] {msg}"));
        }
        let mut buf = [0u8; 64];
        let fmt = b"%Y-%m-%d %H:%M:%S\0";
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr().cast(), &tm);
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }
}

#[cfg(not(unix))]
fn local_text(_secs: i64) -> Result<String, String> {
    Err("OSError: no local time here".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/wiki.sh");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// Each line, and the request, as `cmd_feed` has them.
    #[test]
    fn the_shell_s_lines() {
        let text = source();
        for shell in [
            r#"    local limit="20" agent="" as_json=0"#,
            r#"            -h|--help) cmd_help_activity; return 0 ;;"#,
            r#"            --json) as_json=1; shift ;;"#,
            r#"                [[ $# -ge 2 ]] || { echo "Error: --agent needs a name (k2 activity --help)" >&2; exit 2; }"#,
            r#"                [[ $# -ge 2 ]] || { echo "Error: --limit needs a number (k2 activity --help)" >&2; exit 2; }"#,
            r#"            -*) echo "Error: unknown flag '$1' (k2 activity --help)" >&2; exit 2 ;;"#,
            r#"            *) limit="$1"; shift ;;"#,
            r#"    case "$limit" in ''|*[!0-9]*) echo "Error: the limit is a number, got '$limit' (k2 activity --help)" >&2; exit 2 ;; esac"#,
            r#"    local params="limit=$limit""#,
            r#"    [[ -n "$agent" ]] && params="$params&agent=$(urlencode "$agent")""#,
            r#"    _cli_use_passport_project"#,
            r#"    cli_request "/cli/feed" "$params" | FEED_JSON="$as_json" python3 -c ""#,
        ] {
            assert!(
                text.lines().any(|l| l == shell),
                "cmd_feed no longer has {shell:?}"
            );
        }
        assert_eq!(
            AGENT_NEEDS_A_NAME,
            "Error: --agent needs a name (k2 activity --help)"
        );
        let activity = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/activity.sh"),
        )
        .unwrap();
        assert!(activity.contains("cmd_activity() {\n    # 0.39.0f Phase 2.1: feed → activity (PRD A5).\n    # Same endpoint; renamed at the CLI layer.\n    cmd_feed \"$@\"\n}"));
        let policy = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../cli/src/cli-tools-policy.sh"),
        )
        .unwrap();
        assert!(policy.contains("        help|schema|whats-new|checkin|activity|read|"));
    }

    /// The Python, its lines as `python3 -c` numbers them.
    #[test]
    fn the_python() {
        let lines = super::super::shell_python("wiki.sh", "cmd_feed() {");
        let line = |n: u32| lines[n as usize - 1].trim().to_string();
        assert_eq!(line(3), "raw = sys.stdin.read()");
        assert_eq!(line(5), "d = json.loads(raw)");
        assert_eq!(
            line(7),
            format!("print('{UNUSABLE}%r' % raw[:200], file=sys.stderr)")
        );
        assert_eq!(
            line(9),
            "err = d.get('error') if isinstance(d, dict) else 'unexpected answer'"
        );
        assert_eq!(line(10), "if err is not None:");
        assert_eq!(line(11), "hint = (err.get('hint') or err.get('code') or str(err)) if isinstance(err, dict) else str(err)");
        assert_eq!(line(12), "print('Error: %s' % hint, file=sys.stderr)");
        assert_eq!(line(15), "print(json.dumps(d, indent=2))");
        assert_eq!(line(17), "items = d.get('feed', [])");
        assert_eq!(line(19), format!("print('{NO_ACTIVITY}')"));
        assert_eq!(line(PY_FOR), "for e in items:");
        assert_eq!(
            line(PY_TS),
            "ts = time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(e.get('at', 0)))"
        );
        assert_eq!(line(22), "agent = e.get('agent') or '-'");
        assert_eq!(line(23), "etype = e.get('type', '?')");
        assert_eq!(line(24), "summary = e.get('summary') or ''");
        assert_eq!(line(25), "frm = e.get('from') or ''");
        assert_eq!(line(26), "to = e.get('to') or ''");
        assert_eq!(line(27), "route = f' ({frm}→{to})' if frm and to else ''");
        assert_eq!(
            line(28),
            "print(f'  [{ts}] {agent}: {etype}{route} {summary}')"
        );
        assert_eq!(lines.len(), 28);
    }

    fn ask(limit: &str, agent: &str, json: bool) -> Option<Result<Ask, String>> {
        Some(Ok(Ask {
            limit: limit.into(),
            agent: agent.into(),
            json,
        }))
    }

    #[test]
    fn the_words() {
        assert_eq!(parse(&[]), ask("20", "", false));
        assert_eq!(parse(&["5"]), ask("5", "", false));
        assert_eq!(parse(&["5", "7"]), ask("7", "", false));
        assert_eq!(parse(&["--limit", "9", "--json"]), ask("9", "", true));
        assert_eq!(parse(&["--agent", "a b", "3"]), ask("3", "a b", false));
        assert_eq!(parse(&["--agent", "--json"]), ask("20", "--json", false));
        assert_eq!(parse(&["--agent", ""]), ask("20", "", false));
        assert_eq!(parse(&["007"]), ask("007", "", false));
        assert_eq!(parse(&["--agent"]), Some(Err(AGENT_NEEDS_A_NAME.into())));
        assert_eq!(parse(&["--limit"]), Some(Err(LIMIT_NEEDS_A_NUMBER.into())));
        assert_eq!(parse(&["-x", "--agent"]), Some(Err(unknown_flag("-x"))));
        assert_eq!(parse(&["-"]), Some(Err(unknown_flag("-"))));
        assert_eq!(parse(&["x"]), Some(Err(bad_limit("x"))));
        assert_eq!(parse(&[""]), Some(Err(bad_limit(""))));
        assert_eq!(parse(&["--limit", "1 2"]), Some(Err(bad_limit("1 2"))));
        assert_eq!(parse(&["--limit", "-1"]), Some(Err(bad_limit("-1"))));
        assert_eq!(parse(&["٣"]), Some(Err(bad_limit("٣"))));
        assert_eq!(parse(&["x", "--help"]), None);
        // `--agent` takes `-h` as the name (the front sends any `-h` on).
        assert_eq!(parse(&["--agent", "-h"]), ask("20", "-h", false));
    }

    #[test]
    fn the_error_and_json_forms() {
        let f = |s: &str| reader(s.as_bytes(), false, 0);
        assert_eq!(f(""), Shown::err(format!("{UNUSABLE}''\n"), Exit::Error));
        assert_eq!(
            f("x\n"),
            Shown::err(format!("{UNUSABLE}'x\\n'\n"), Exit::Error)
        );
        assert_eq!(
            f("[1]"),
            Shown::err("Error: unexpected answer\n", Exit::Error)
        );
        assert_eq!(
            f(r#"{"error":{"hint":"","code":"gated"}}"#),
            Shown::err("Error: gated\n", Exit::Error)
        );
        assert_eq!(f(r#"{"error":{}}"#), Shown::err("Error: {}\n", Exit::Error));
        assert_eq!(
            f(r#"{"error":false}"#),
            Shown::err("Error: False\n", Exit::Error)
        );
        assert_eq!(f(r#"{"error":null}"#), Shown::out("No activity.\n"));
        assert_eq!(
            reader(br#"{"feed":[{"at":1,"x":"\u00e9"}]}"#, true, 0),
            Shown::out(
                "{\n  \"feed\": [\n    {\n      \"at\": 1,\n      \"x\": \"\\u00e9\"\n    }\n  ]\n}\n"
            )
        );
    }

    #[test]
    fn the_event_lines() {
        let f = |s: &str| reader(s.as_bytes(), false, 0);
        for none in ["{}", r#"{"feed":[]}"#, r#"{"feed":""}"#, r#"{"feed":{}}"#] {
            assert_eq!(f(none), Shown::out("No activity.\n"), "{none}");
        }
        let zero = stamp(&Value::Int("0".into()), 0).unwrap();
        assert_eq!(zero.len(), 19, "{zero}");
        assert_eq!(
            f(r#"{"feed":[{"agent":"a","type":"msg","from":"x","to":"y","summary":"s é"}]}"#),
            Shown::out(format!("  [{zero}] a: msg (x\u{2192}y) s é\n"))
        );
        assert_eq!(
            f(r#"{"feed":[{"type":null,"from":"x"}]}"#),
            Shown::out(format!("  [{zero}] -: None \n"))
        );
        assert_eq!(
            f(r#"{"feed":[{}]}"#),
            Shown::out(format!("  [{zero}] -: ? \n"))
        );
        assert_eq!(
            f(r#"{"feed":[{"agent":[1],"type":2.5,"summary":{"k":true},"from":1,"to":true}]}"#),
            Shown::out(format!(
                "  [{zero}] [1]: 2.5 (1\u{2192}True) {{'k': True}}\n"
            ))
        );
        // Where Python raises, what came before stays.
        let r = f(r#"{"feed":[{"agent":"a"},5]}"#);
        assert_eq!(r.stdout, format!("  [{zero}] a: ? \n"));
        assert!(r.stderr.ends_with(
            "line 21, in <module>\nAttributeError: 'int' object has no attribute 'get'\n"
        ));
        assert_eq!(r.exit, Exit::Error);
        let r = f(r#"{"feed":[{"at":"x"}]}"#);
        assert!(r
            .stderr
            .ends_with("TypeError: 'str' object cannot be interpreted as an integer\n"));
        for (feed, ty) in [("null", "NoneType"), ("0", "int"), ("true", "bool")] {
            let r = f(&format!(r#"{{"feed":{feed}}}"#));
            let lead = if ty == "bool" { "" } else { "No activity.\n" };
            assert_eq!(r.stdout, lead, "{feed}");
            assert!(
                r.stderr.ends_with(&format!(
                    "line 20, in <module>\nTypeError: '{ty}' object is not iterable\n"
                )),
                "{feed}: {}",
                r.stderr
            );
        }
        let r = f(r#"{"feed":"ab"}"#);
        assert!(r
            .stderr
            .ends_with("AttributeError: 'str' object has no attribute 'get'\n"));
        let r = f(r#"{"feed":[{"at":1e300}]}"#);
        assert!(r
            .stderr
            .ends_with("OverflowError: timestamp out of range for platform time_t\n"));
        let r = f(r#"{"feed":[{"at":NaN}]}"#);
        assert!(r
            .stderr
            .ends_with("ValueError: Invalid value NaN (not a number)\n"));
    }

    #[test]
    fn the_time_is_python_s() {
        let one = stamp(&Value::Int("86400".into()), 0).unwrap();
        assert_eq!(stamp(&Value::Float(86400.9), 0).unwrap(), one);
        assert_eq!(stamp(&Value::Null, 86400).unwrap(), one);
        assert_eq!(
            stamp(&Value::Bool(true), 0).unwrap(),
            stamp(&Value::Int("1".into()), 0).unwrap()
        );
        assert_ne!(
            stamp(&Value::Float(-0.5), 0).unwrap(),
            stamp(&Value::Int("0".into()), 0).unwrap(),
            "floor, not toward zero"
        );
        assert!(stamp(&Value::Int("99999999999999999999".into()), 0).is_err());
        assert_eq!(
            stamp(&Value::Array(vec![]), 0),
            Err("TypeError: 'list' object cannot be interpreted as an integer".into())
        );
    }
}
