//! `k5 read <workspace> [--lines N] [--agent <name>] [--session <id>]`: the
//! last lines of a workspace's live terminal, as the shell CLI's
//! `cmd_read` prints them (`cli/src/daemon-install.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    the 0.40.4 rename line, and the two gates ([`crate::prelude`]).
//!    `read` is an open tool, so no passport check comes after them.
//! 2. The words ([`parse`]): the first is the workspace, whatever it is
//!    (`k2 read --session s` reads the workspace `--session`), then
//!    `--lines`, `--agent`, `--session` and `--token` each take the next
//!    word, and other words are skipped. No workspace and no session is
//!    the usage line, exit 1.
//! 3. The workspace as `_split_remote_target` leaves it locally
//!    ([`split_remote`]): `ws@` reads `ws`.
//! 4. One `cli_send cli_request /cli/terminal/read <query>` on the left of
//!    a pipe into the shell's Python ([`super::request_piped`]): the cell
//!    socket first when there is a passport with a live socket, then TCP.
//!    The query is `session=<escaped>&lines=<N>` with a session, else
//!    `workspace=<escaped>&lines=<N>[&agent=<escaped>]`; `N` (50 when not
//!    given) goes as it is, not escaped.
//! 5. The Python ([`reader`]): each of `lines`, one per line; a daemon
//!    error on stderr, as compact JSON for an object (exit 3 for
//!    `not_connected`, `gated` or `forbidden`, else 1) or as its text;
//!    an answer that is not JSON is its first 200 characters on stderr,
//!    exit 1. The command ends with the reader's code when it is not 0,
//!    else the call's.
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! A request with no answer is the transport's line, and the command then
//! dies of SIGPIPE with nothing else printed, as the shell's
//! `_k2_end_command` ends a pipe ([`super::end_in_pipe`]).
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`):
//!
//! - a workspace on another server (`ws@host`, `ws::host.example`): the
//!   shell points the call at that host with its own token;
//! - a value flag as the last word (the shell stops on `$2: unbound
//!   variable`, a line with its script's path in it), a word that is not
//!   UTF-8, and a command line with `-h` or `--help` ([`crate::front`]);
//! - a `--lines` value with a byte that is not printable ASCII, or a `#`:
//!   the shell hands it to curl as it is, and curl refuses it or cuts it;
//! - `HOOK_SOCK` naming a live socket and no passport: the shell tries the
//!   cell socket with an empty Bearer (the binary has no cell socket
//!   without a passport);
//! - a port that is set but is no port curl takes, and a `python3` the
//!   binary can't tell, as for [`super::inbox`].
//!
//! Where the shell's Python itself fails with a traceback (an answer that
//! is JSON but not an object, or `lines` that is not a list), the text
//! depends on the Python version; this prints the Python 3.12 one,
//! without source lines. Bytes that are not UTF-8, and half of a
//! surrogate pair, are as [`super::inbox`] says.

use super::{curl_port, get, pipefail, request_piped, traceback, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::Auth;
use std::ffi::OsString;
use std::path::Path;

/// The route.
pub const ENDPOINT: &str = "/cli/terminal/read";
/// No workspace and no session.
pub const USAGE: &str = "Usage: k2 read <workspace>[@<host>] [--lines N] [--agent <name>] [--session <id>] [--token <tok>]";
/// The line of the shell's Python that loops over `lines`, for its
/// tracebacks (`tests` pins it to the script).
pub const PY_LINES: u32 = 19;

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// The first word, as given (before the remote split).
    pub workspace: String,
    pub lines: String,
    pub agent: String,
    pub session: String,
}

/// The words after `read`, as `cmd_read`'s loop reads them. `None`: a
/// value flag with nothing after it (the shell's).
pub fn parse(words: &[&str]) -> Option<Ask> {
    let mut ask = Ask {
        workspace: words.first().map_or_else(String::new, |w| (*w).to_string()),
        lines: "50".into(),
        agent: String::new(),
        session: String::new(),
    };
    let mut i = 1;
    while let Some(w) = words.get(i) {
        let slot = match *w {
            "--lines" => Some(&mut ask.lines),
            "--agent" => Some(&mut ask.agent),
            "--session" => Some(&mut ask.session),
            // `--token` only matters for a remote host, which the shell
            // answers.
            "--token" => None,
            _ => {
                i += 1;
                continue;
            }
        };
        let value = words.get(i + 1)?;
        if let Some(slot) = slot {
            *slot = (*value).to_string();
        }
        i += 2;
    }
    Some(ask)
}

/// `_split_remote_target`: the bare target and the host (empty for this
/// server). Two `::` or more is the wire form, kept whole; `a::b` is
/// remote when both sides are there and `b` has a `.`; else the last `@`
/// splits it (an empty host stays on this server).
pub fn split_remote(raw: &str) -> (String, String) {
    if let Some(at) = raw.find("::") {
        if raw[at + 2..].contains("::") {
            return (raw.to_string(), String::new());
        }
        let (left, right) = (&raw[..at], &raw[at + 2..]);
        if !left.is_empty() && right.contains('.') {
            return (left.to_string(), right.to_string());
        }
        return (raw.to_string(), String::new());
    }
    match raw.rsplit_once('@') {
        Some((bare, host)) => (bare.to_string(), host.to_string()),
        None => (raw.to_string(), String::new()),
    }
}

/// A `--lines` value the binary sends as the shell's curl does: printable
/// ASCII and no `#`.
pub fn sendable_lines(lines: &str) -> bool {
    lines
        .bytes()
        .all(|b| (0x21..0x7f).contains(&b) && b != b'#')
}

/// The query the shell hands `cli_request`, one argument. `workspace` is
/// the bare one.
pub fn query(ask: &Ask, workspace: &str) -> String {
    if ask.session.is_empty() {
        let mut q = format!("workspace={}&lines={}", quote(workspace), ask.lines);
        if !ask.agent.is_empty() {
            q.push_str("&agent=");
            q.push_str(&quote(&ask.agent));
        }
        q
    } else {
        format!("session={}&lines={}", quote(&ask.session), ask.lines)
    }
}

/// `k5 read ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let ask = parse(&words)?;
    let (bare, host) = split_remote(&ask.workspace);
    if !host.is_empty() || !sendable_lines(&ask.lines) || !python::family_native(python::here()) {
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
        let passport = matches!(settings.auth, Auth::Passport { .. });
        if !passport && super::live_socket(super::hook_sock().as_deref()) {
            return None;
        }
    }
    Some(run_with(args, &ask, &bare, settings))
}

fn run_with(args: &[OsString], ask: &Ask, workspace: &str, mut settings: Settings) -> Exit {
    if let Some(notice) = prelude::rename_notice(Path::new(&settings.project)) {
        out::stderr_line(notice);
    }
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    if ask.workspace.is_empty() && ask.session.is_empty() {
        out::stderr_line(USAGE);
        return Exit::Error;
    }
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let q = query(ask, workspace);
    let piped = request_piped(&mut client, &mut closing, ENDPOINT, &[q.as_str()]);
    let exit = pipefail(piped.exit, reader(&piped.body).print());
    closing.flush(&mut client, exit);
    exit
}

/// The codes that are a refusal (exit 3).
const REFUSALS: [&str; 3] = ["not_connected", "gated", "forbidden"];

/// The shell's Python on the answer it reads.
pub fn reader(body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let Ok(data) = json::parse(&raw) else {
        let head: String = raw.chars().take(200).collect();
        return Shown::err(format!("{head}\n"), Exit::Error);
    };
    if let Value::Object(pairs) = &data {
        if let Some(err) = get(pairs, "error") {
            let Value::Object(e) = err else {
                return Shown::err(format!("{}\n", py::str_of(err)), Exit::Error);
            };
            let truthy = |key: &str| get(e, key).filter(|v| py::truthy(v)).cloned();
            let code = truthy("code").unwrap_or_else(|| Value::Str(Text::from("error")));
            let hint = truthy("hint").unwrap_or_else(|| code.clone());
            let refusal =
                matches!(&code, Value::Str(t) if t.as_str().is_some_and(|c| REFUSALS.contains(&c)));
            let shape = Value::Object(vec![(
                Text::from("error"),
                Value::Object(vec![(Text::from("code"), code), (Text::from("hint"), hint)]),
            )]);
            let compact = Options {
                compact: true,
                ensure_ascii: false,
                ..Options::default()
            };
            // Half of a surrogate pair can't be UTF-8: Python's stderr
            // writes it escaped, as `ensure_ascii` would.
            let text = json::to_string(&shape, &compact)
                .or_else(|_| {
                    json::to_string(
                        &shape,
                        &Options {
                            compact: true,
                            ..Options::default()
                        },
                    )
                })
                .unwrap_or_default();
            return Shown::err(
                format!("{text}\n"),
                if refusal { Exit::Refused } else { Exit::Error },
            );
        }
    }
    let Value::Object(pairs) = &data else {
        let exception = format!(
            "AttributeError: '{}' object has no attribute 'get'",
            py::type_name(&data)
        );
        return Shown::err(
            traceback(&[(PY_LINES, "<module>")], &exception),
            Exit::Error,
        );
    };
    let items: Vec<Value> = match get(pairs, "lines") {
        None => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        Some(Value::Str(t)) => t
            .code_points()
            .into_iter()
            .map(|cp| Value::Str(code_point(cp)))
            .collect(),
        Some(Value::Object(keys)) => keys.iter().map(|(k, _)| Value::Str(k.clone())).collect(),
        Some(other) => {
            let exception = format!(
                "TypeError: '{}' object is not iterable",
                py::type_name(other)
            );
            return Shown::err(
                traceback(&[(PY_LINES, "<module>")], &exception),
                Exit::Error,
            );
        }
    };
    let mut stdout = String::new();
    for item in &items {
        stdout.push_str(&py::str_of(item));
        stdout.push('\n');
    }
    Shown::out(stdout)
}

/// A one-code-point string, half of a surrogate pair too.
fn code_point(cp: u32) -> Text {
    match char::from_u32(cp) {
        Some(c) => Text::from(c.to_string()),
        None => match json::parse(&format!("\"\\u{cp:04x}\"")) {
            Ok(Value::Str(t)) => t,
            _ => Text::from("\u{fffd}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(file: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/src")
            .join(file);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// Each line and the request, as `cmd_read` has them.
    #[test]
    fn the_shell_s_lines() {
        let text = source("daemon-install.sh");
        let body = text
            .split_once("cmd_read() {")
            .expect("cmd_read in daemon-install.sh")
            .1;
        for shell in [
            r#"    local workspace="${1:-}""#,
            r#"    local lines=50"#,
            r#"    shift || true"#,
            r#"            --lines) lines="$2"; shift 2 ;;"#,
            r#"            --agent) agent="$2"; shift 2 ;;"#,
            r#"            --session) session="$2"; shift 2 ;;"#,
            r#"            --token) token="$2"; shift 2 ;;"#,
            r#"            *) shift ;;"#,
            r#"    if [ -z "$workspace" ] && [ -z "$session" ]; then"#,
            r#"    _split_remote_target "$workspace"; workspace="$_BARE_TARGET""#,
            r#"    _apply_remote_host "$_REMOTE_HOST" "$token" || exit 1"#,
            r#"        query="session=$(urlencode "$session")&lines=$lines""#,
            r#"        query="workspace=$(urlencode "$workspace")&lines=$lines""#,
            r#"            query="$query&agent=$(urlencode "$agent")""#,
            r#"    if command -v python3 >/dev/null 2>&1; then"#,
            r#"        cli_send cli_request "/cli/terminal/read" "$query" | python3 -c ""#,
            r#"        sys.exit(3 if code in ('not_connected', 'gated', 'forbidden') else 1)"#,
        ] {
            assert!(
                body.lines().any(|l| l == shell),
                "cmd_read no longer has {shell:?}"
            );
        }
        assert!(
            body.contains(&format!("        echo \"{USAGE}\" >&2\n        exit 1")),
            "the usage line moved"
        );
        let policy = source("cli-tools-policy.sh");
        assert!(
            policy.contains(r#"        read) echo "read" ;;"#),
            "read's tool id moved"
        );
        assert!(
            policy.contains("        help|schema|whats-new|checkin|activity|read|"),
            "read is no longer an open tool"
        );
        let remote = source("remote-target.sh");
        for shell in [
            r#"    if [[ "$raw" == *::*::* ]]; then"#,
            r#"        if [ -n "$left" ] && [ -n "$right" ] && { [[ "$right" == *.* ]] || [[ "$right" == *.k2.dev ]] || [[ "$right" == *.K2.DEV ]]; }; then"#,
            r#"            _REMOTE_HOST="${raw##*@}""#,
            r#"            _BARE_TARGET="${raw%@*}""#,
        ] {
            assert!(
                remote.lines().any(|l| l == shell),
                "_split_remote_target no longer has {shell:?}"
            );
        }
        let remote_msg = source("remote-msg.sh");
        assert!(
            remote_msg.contains("    [ -z \"$host\" ] && return 0\n"),
            "_apply_remote_host no longer leaves an empty host local"
        );
    }

    /// The shell's Python, its lines as `python3 -c` numbers them.
    #[test]
    fn the_python_s_lines() {
        let lines = super::super::shell_python("daemon-install.sh", "cmd_read() {");
        let line = |n: u32| lines[n as usize - 1].trim().to_string();
        assert_eq!(line(3), "raw = sys.stdin.read()");
        assert_eq!(line(7), "sys.stderr.write(raw[:200] + '\\n')");
        assert_eq!(line(9), "if isinstance(data, dict) and 'error' in data:");
        assert_eq!(line(12), "code = err.get('code') or 'error'");
        assert_eq!(line(13), "hint = err.get('hint') or code");
        assert_eq!(
            line(14),
            "sys.stderr.write(json.dumps({'error': {'code': code, 'hint': hint}},"
        );
        assert_eq!(
            line(15),
            "ensure_ascii=False, separators=(',', ':')) + '\\n')"
        );
        assert_eq!(line(17), "sys.stderr.write(str(err) + '\\n')");
        assert_eq!(line(PY_LINES), "for line in data.get('lines', []):");
        assert_eq!(line(20), "print(line)");
        assert_eq!(lines.len(), 20);
    }

    fn ask(workspace: &str, lines: &str, agent: &str, session: &str) -> Option<Ask> {
        Some(Ask {
            workspace: workspace.into(),
            lines: lines.into(),
            agent: agent.into(),
            session: session.into(),
        })
    }

    #[test]
    fn the_words() {
        assert_eq!(parse(&[]), ask("", "50", "", ""));
        assert_eq!(parse(&["ws"]), ask("ws", "50", "", ""));
        assert_eq!(
            parse(&["ws", "--lines", "5", "--agent", "x y"]),
            ask("ws", "5", "x y", "")
        );
        assert_eq!(
            parse(&["ws", "--session", "s1", "extra", "--token", "t"]),
            ask("ws", "50", "", "s1")
        );
        // The first word is the workspace, a flag too.
        assert_eq!(parse(&["--session", "s 1"]), ask("--session", "50", "", ""));
        assert_eq!(parse(&["--lines", "5"]), ask("--lines", "50", "", ""));
        assert_eq!(
            parse(&["ws", "--agent", "--lines"]),
            ask("ws", "50", "--lines", "")
        );
        assert_eq!(parse(&["ws", "--lines", ""]), ask("ws", "", "", ""));
        assert_eq!(
            parse(&["ws", "--lines", "1", "--lines", "2"]),
            ask("ws", "2", "", "")
        );
        for last in ["--lines", "--agent", "--session", "--token"] {
            assert_eq!(parse(&["ws", last]), None, "{last}");
            assert_eq!(parse(&["ws", "x", last]), None, "{last}");
        }
        assert_eq!(
            parse(&["--lines"]),
            ask("--lines", "50", "", ""),
            "the first word is never a flag"
        );
    }

    #[test]
    fn the_remote_split() {
        let s = |raw: &str| split_remote(raw);
        let local = |raw: &str| (raw.to_string(), String::new());
        assert_eq!(s("ws"), local("ws"));
        assert_eq!(s(""), local(""));
        assert_eq!(s("ws@host"), ("ws".into(), "host".into()));
        assert_eq!(s("a@b@c"), ("a@b".into(), "c".into()));
        assert_eq!(s("ws@"), ("ws".into(), String::new()));
        assert_eq!(s("@host"), (String::new(), "host".into()));
        assert_eq!(s("ws::box.k2.dev"), ("ws".into(), "box.k2.dev".into()));
        assert_eq!(s("ws::box"), local("ws::box"));
        assert_eq!(s("::box.k2.dev"), local("::box.k2.dev"));
        assert_eq!(s("ws::"), local("ws::"));
        assert_eq!(s("fp::ws::agent"), local("fp::ws::agent"));
        assert_eq!(s("a::b.c@d"), ("a".into(), "b.c@d".into()));
        assert_eq!(s("a:::b"), local("a:::b"));
        assert_eq!(s("a::::b"), local("a::::b"));
        assert_eq!(s("a::b::"), local("a::b::"));
    }

    #[test]
    fn the_query() {
        let a = |lines: &str, agent: &str, session: &str| Ask {
            workspace: "w".into(),
            lines: lines.into(),
            agent: agent.into(),
            session: session.into(),
        };
        assert_eq!(
            query(&a("50", "", ""), "acme ws"),
            "workspace=acme%20ws&lines=50"
        );
        assert_eq!(
            query(&a("5", "x&y", ""), "w"),
            "workspace=w&lines=5&agent=x%26y"
        );
        assert_eq!(
            query(&a("5&x=1", "a", "s/1"), "w"),
            "session=s%2F1&lines=5&x=1"
        );
        assert!(sendable_lines("50") && sendable_lines("5&x=%41") && sendable_lines(""));
        for bad in ["5 6", "5#6", "5\t", "é", "\u{7f}"] {
            assert!(!sendable_lines(bad), "{bad:?}");
        }
    }

    fn ok(s: &str) -> Shown {
        Shown::out(s)
    }

    #[test]
    fn the_reader() {
        let f = |s: &str| reader(s.as_bytes());
        assert_eq!(
            f(r#"{"lines":["a","b",1,null,[1,"x"]]}"#),
            ok("a\nb\n1\nNone\n[1, 'x']\n")
        );
        assert_eq!(f(r#"{"lines":[]}"#), ok(""));
        assert_eq!(f(r#"{"x":1}"#), ok(""));
        assert_eq!(f(r#"{"lines":"ab"}"#), ok("a\nb\n"));
        assert_eq!(f(r#"{"lines":{"k":1,"j":2}}"#), ok("k\nj\n"));
        assert_eq!(f(r#"{"lines":["é \t"]}"#), ok("é \t\n"));
        assert_eq!(
            f(r#"{"error":{"code":"forbidden","hint":"nope é"}}"#),
            Shown::err(
                "{\"error\":{\"code\":\"forbidden\",\"hint\":\"nope é\"}}\n",
                Exit::Refused
            )
        );
        for code in ["not_connected", "gated"] {
            assert_eq!(
                f(&format!(r#"{{"error":{{"code":"{code}"}}}}"#)),
                Shown::err(
                    format!("{{\"error\":{{\"code\":\"{code}\",\"hint\":\"{code}\"}}}}\n"),
                    Exit::Refused
                )
            );
        }
        assert_eq!(
            f(r#"{"error":{"code":"","hint":""}}"#),
            Shown::err(
                "{\"error\":{\"code\":\"error\",\"hint\":\"error\"}}\n",
                Exit::Error
            )
        );
        assert_eq!(
            f(r#"{"error":{"code":5,"hint":[1]}}"#),
            Shown::err("{\"error\":{\"code\":5,\"hint\":[1]}}\n", Exit::Error)
        );
        assert_eq!(f(r#"{"error":{"code":"Forbidden"}}"#).exit, Exit::Error);
        assert_eq!(
            f(r#"{"error":{}}"#),
            Shown::err(
                "{\"error\":{\"code\":\"error\",\"hint\":\"error\"}}\n",
                Exit::Error
            )
        );
        assert_eq!(
            f(r#"{"error":"bad","lines":["x"]}"#),
            Shown::err("bad\n", Exit::Error)
        );
        assert_eq!(f(r#"{"error":null}"#), Shown::err("None\n", Exit::Error));
        assert_eq!(
            f(r#"{"error":[1,"a"]}"#),
            Shown::err("[1, 'a']\n", Exit::Error)
        );
        assert_eq!(f("x\n"), Shown::err("x\n\n", Exit::Error));
        assert_eq!(f(""), Shown::err("\n", Exit::Error));
        assert_eq!(
            f(&"z".repeat(250)),
            Shown::err(format!("{}\n", "z".repeat(200)), Exit::Error)
        );
        assert_eq!(
            f("[1]"),
            Shown::err("Traceback (most recent call last):\n  File \"<string>\", line 19, in <module>\nAttributeError: 'list' object has no attribute 'get'\n", Exit::Error)
        );
        assert!(f("\"s\"")
            .stderr
            .ends_with("AttributeError: 'str' object has no attribute 'get'\n"));
        assert!(f("null")
            .stderr
            .ends_with("AttributeError: 'NoneType' object has no attribute 'get'\n"));
        assert_eq!(
            f(r#"{"lines":5}"#),
            Shown::err("Traceback (most recent call last):\n  File \"<string>\", line 19, in <module>\nTypeError: 'int' object is not iterable\n", Exit::Error)
        );
        assert!(f(r#"{"lines":null}"#)
            .stderr
            .ends_with("TypeError: 'NoneType' object is not iterable\n"));
        assert!(f(r#"{"lines":true}"#)
            .stderr
            .ends_with("TypeError: 'bool' object is not iterable\n"));
    }
}
