//! `k5 whoami [--json]`: this cell's identity, as the shell CLI's
//! `cmd_whoami` prints it (`cli/src/msg-whoami-done.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    the 0.40.4 rename line, and the two gates ([`crate::prelude`]).
//! 2. The flags: `--json`; any other word starting with `-` is
//!    `Error: unknown flag '<word>' (try 'k2 whoami --help')` and exit 1;
//!    other words are ignored. `-h` and `--help` never get here: the front
//!    sends a command line holding either to the shell CLI, which prints
//!    the help ([`crate::front`]).
//! 3. One `cli_send cli_request /cli/whoami <query>`: the query holds
//!    `session=`, `cell=`, `sidecar_name=` and `primary=` from
//!    `SESSION_ID`, `CELL`, `SIDECAR_NAME` and `PRIMARY` (each under its
//!    K5_, K2_ and K2SO_ names, empty counts as unset), escaped as
//!    `urlencode` does. The cell socket doesn't serve the route, so it goes
//!    over TCP. The shell hands `cli_request` the query as one argument even
//!    when it is empty, so the TCP query then ends in `&`.
//! 4. The answer, as the shell reads it into `resp=$(…)`: trailing
//!    newlines and NULs dropped.
//!    - `--json`: `resp` and a newline on stdout; exit 0 when it is a JSON
//!      object with a `role` key, else 1.
//!    - Otherwise the shell's Python: not JSON is `Error: daemon returned an
//!      unparseable whoami response` (exit 1); an object with an `error`
//!      key is `Error: <its hint, else its code, else itself>` for an
//!      object, else `Error: <error>` and the `hint` line (exit 1); else
//!      five lines, `'%-10s %s'` of `workspace:`, `role:`, `address:`,
//!      `primary:` and `session:` with Python's `str()` of each value, and
//!      `others:    <n> live here (k2 sessions)` when `siblings` is an
//!      object with `live` (`'%d'`).
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! A request with no answer is the transport's line and exit 1; any other
//! failure ends with the code the shell's call returns, with nothing
//! printed, as `set -e` passes curl's code on
//! ([`crate::transport::Error::curl_code`]: 7 nothing listening, 56 a
//! reset, 75 a failure that is this machine's, and so on). A port that is
//! no port is curl's 3 (or 75 when `TMPDIR` can't be used).
//!
//! Where the shell's Python itself fails (an answer that is JSON but not
//! an object, or a `siblings.live` that is no number), it prints a
//! traceback whose text depends on the Python version; this prints the
//! Python 3.12 one, without source lines. The exit code and stdout are the
//! shell's.

use super::curl_port;
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::transport::client::{tmpdir_unusable, LOCAL_TRANSPORT_DENIED};
use crate::transport::encode::quote_bytes;
use crate::transport::wire::curl;
use crate::transport::{Call, Error};
use k2_client::env_names;
use std::ffi::OsString;
use std::path::Path;

/// The route.
pub const ENDPOINT: &str = "/cli/whoami";

/// The shell's line for a flag it doesn't take.
pub fn unknown_flag(word: &str) -> String {
    format!("Error: unknown flag '{word}' (try 'k2 whoami --help')")
}

/// The line when the answer isn't JSON.
pub const UNPARSEABLE: &str = "Error: daemon returned an unparseable whoami response";

/// What the flags ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flags {
    Run { json: bool },
    /// The first word starting with `-` that isn't `--json`.
    Unknown(String),
}

/// The words after `whoami`, as the shell's loop reads them.
pub fn flags(words: &[&str]) -> Flags {
    let mut json = false;
    for w in words {
        match *w {
            "--json" => json = true,
            w if w.starts_with('-') => return Flags::Unknown(w.to_string()),
            _ => {}
        }
    }
    Flags::Run { json }
}

/// The session's names for the query, `key=<escaped value>` joined by `&`.
pub fn query(lookup: impl Fn(&str) -> Option<OsString>) -> String {
    let fields: [(&str, &[&'static str]); 4] = [
        ("session", env_names::SESSION_ID),
        ("cell", env_names::CELL),
        ("sidecar_name", env_names::SIDECAR_NAME),
        ("primary", env_names::PRIMARY),
    ];
    let mut parts = Vec::new();
    for (key, names) in fields {
        if let Some((_, v)) = env_names::first_nonempty_os_in(names, &lookup) {
            parts.push(format!("{key}={}", quote_bytes(&os_bytes(&v))));
        }
    }
    parts.join("&")
}

#[cfg(unix)]
fn os_bytes(v: &OsString) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    v.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_bytes(v: &OsString) -> Vec<u8> {
    v.to_string_lossy().into_owned().into_bytes()
}

/// `k5 whoami ...`: `args` is the whole command line after `argv[0]`.
pub fn run(args: &[OsString]) -> Exit {
    match daemon::current() {
        Ok(settings) => run_with(args, settings),
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            Exit::Error
        }
    }
}

fn run_with(args: &[OsString], mut settings: Settings) -> Exit {
    if let Some(line) = prelude::rename_notice(Path::new(&settings.project)) {
        out::stderr_line(line);
    }
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let words: Vec<&str> = args.iter().skip(1).filter_map(|a| a.to_str()).collect();
    let json = match flags(&words) {
        Flags::Run { json } => json,
        Flags::Unknown(word) => {
            out::stderr_line(&unknown_flag(&word));
            return Exit::Error;
        }
    };
    // The gate passed, so there is a port text and a token. curl refuses a
    // port that is no port (3), and `_cli_curl` reads that as any failure.
    let Some(port) = settings.port_text.as_deref().and_then(curl_port) else {
        return Exit::from_curl(if tmpdir_unusable() { LOCAL_TRANSPORT_DENIED } else { curl::URL_MALFORMAT });
    };
    settings.port = Some(port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let q = query(|name| std::env::var_os(name));
    let passport = client.passport().is_some();
    let mut closing = Closing::default();
    let exit = match client.send(ENDPOINT, Call::Request(&[q.as_str()])) {
        Ok(r) => {
            closing.answer(&r, passport);
            let resp = substituted(&r.body);
            if json {
                print_json(&resp)
            } else {
                let shown = human(&resp);
                shown.print()
            }
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Exit::Error
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    };
    closing.flush(&mut client, exit);
    exit
}

/// What `resp=$(…)` keeps of a body: NULs and trailing newlines dropped.
pub fn substituted(body: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = body.iter().copied().filter(|b| *b != 0).collect();
    while out.last() == Some(&b'\n') {
        out.pop();
    }
    out
}

/// `--json`: the answer and a newline; 0 for an object with `role`.
fn print_json(resp: &[u8]) -> Exit {
    let mut line = resp.to_vec();
    line.push(b'\n');
    if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &line) {
        return out::after_stdout_error(&e);
    }
    if json_has_role(resp) {
        Exit::Ok
    } else {
        Exit::Error
    }
}

/// The shell's check: JSON, an object, a `role` key.
pub fn json_has_role(resp: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(resp) else {
        return false;
    };
    matches!(json::parse(text), Ok(Value::Object(pairs)) if pairs.iter().any(|(k, _)| *k == Text::from("role")))
}

/// What the shell's Python prints: stdout, stderr and its exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub stdout: String,
    pub stderr: String,
    pub exit: Exit,
}

impl Shown {
    fn print(&self) -> Exit {
        if !self.stdout.is_empty() {
            if let Err(e) = out::stdout(&self.stdout) {
                return out::after_stdout_error(&e);
            }
        }
        if !self.stderr.is_empty() {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), self.stderr.as_bytes());
        }
        self.exit
    }
}

/// The line numbers of the shell's Python, for its tracebacks: the line
/// that prints the first field, the `field` helper's return, and the
/// `others:` line (`tests` pins them to the script).
pub const PY_FIRST_PRINT: u32 = 21;
pub const PY_FIELD_RETURN: u32 = 20;
pub const PY_OTHERS: u32 = 28;

fn traceback(frames: &[(u32, &str)], exception: &str) -> String {
    let mut s = String::from("Traceback (most recent call last):\n");
    for (line, name) in frames {
        s.push_str(&format!("  File \"<string>\", line {line}, in {name}\n"));
    }
    s.push_str(exception);
    s.push('\n');
    s
}

fn get<'a>(pairs: &'a [(Text, Value)], key: &str) -> Option<&'a Value> {
    let key = Text::from(key);
    pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// The shell's Python for the plain form, on `resp`.
pub fn human(resp: &[u8]) -> Shown {
    let fail = |stderr: String| Shown { stdout: String::new(), stderr, exit: Exit::Error };
    let parsed = std::str::from_utf8(resp).ok().and_then(|t| json::parse(t).ok());
    let Some(d) = parsed else {
        return fail(format!("{UNPARSEABLE}\n"));
    };
    let Value::Object(pairs) = &d else {
        let exception = format!("AttributeError: '{}' object has no attribute 'get'", py::type_name(&d));
        return fail(traceback(&[(PY_FIRST_PRINT, "<module>"), (PY_FIELD_RETURN, "field")], &exception));
    };
    if let Some(err) = get(pairs, "error") {
        let mut stderr = String::new();
        match err {
            Value::Object(e) => {
                let pick = [get(e, "hint"), get(e, "code")].into_iter().flatten().find(|v| py::truthy(v));
                stderr.push_str(&format!("Error: {}\n", pick.map_or_else(|| py::str_of(err), py::str_of)));
            }
            other => {
                stderr.push_str(&format!("Error: {}\n", py::str_of(other)));
                if let Some(hint) = get(pairs, "hint").filter(|h| py::truthy(h)) {
                    stderr.push_str(&format!("{}\n", py::str_of(hint)));
                }
            }
        }
        return fail(stderr);
    }
    let field = |label: &str, key: &str| -> String {
        let value = get(pairs, key).map_or_else(String::new, py::str_of);
        format!("{:<10} {value}\n", format!("{label}:"))
    };
    let mut stdout = String::new();
    for (label, key) in [("workspace", "workspace"), ("role", "role"), ("address", "address"), ("primary", "primary"), ("session", "session")] {
        stdout.push_str(&field(label, key));
    }
    if let Some(Value::Object(sib)) = get(pairs, "siblings") {
        if let Some(live) = get(sib, "live") {
            match py::percent_d(live) {
                Ok(n) => stdout.push_str(&format!("{:<10} {n} live here (k2 sessions)\n", "others:")),
                Err(exception) => {
                    return Shown { stdout, stderr: traceback(&[(PY_OTHERS, "<module>")], &exception), exit: Exit::Error };
                }
            }
        }
    }
    Shown { stdout, stderr: String::new(), exit: Exit::Ok }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn shown(body: &str) -> Shown {
        human(body.as_bytes())
    }

    fn ok(stdout: &str) -> Shown {
        Shown { stdout: stdout.to_string(), stderr: String::new(), exit: Exit::Ok }
    }

    fn err(stderr: &str) -> Shown {
        Shown { stdout: String::new(), stderr: stderr.to_string(), exit: Exit::Error }
    }

    /// The shell's Python, its lines as `python3 -c` numbers them.
    fn script() -> Vec<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/msg-whoami-done.sh");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let body = text.split_once("cmd_whoami() {").expect("cmd_whoami in msg-whoami-done.sh").1;
        let start = body.find("| python3 -c \"\nimport json, sys\n").expect("the plain form's Python");
        let py = &body[start..];
        let py = &py[py.find("-c \"").expect("-c") + 4..];
        let end = py.find("\n\"\n").expect("the end of the Python");
        py[..end].split('\n').map(str::to_string).collect()
    }

    #[test]
    fn the_traceback_lines_are_the_script_s() {
        let lines = script();
        let line = |n: u32| lines[n as usize - 1].trim().to_string();
        assert_eq!(line(PY_FIRST_PRINT), "print(field('workspace', 'workspace'))");
        assert_eq!(line(PY_FIELD_RETURN), "return '%-10s %s' % (label + ':', d.get(key, ''))");
        assert_eq!(line(PY_OTHERS), "print('%-10s %d live here (k2 sessions)' % ('others:', sib.get('live', 0)))");
        assert_eq!(line(7), format!("sys.stderr.write('{UNPARSEABLE}\\n')"));
    }

    #[test]
    fn the_shell_s_lines() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/msg-whoami-done.sh");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#"echo "Error: unknown flag '$arg' (try 'k2 whoami --help')" >&2"#), "the unknown-flag line moved");
        assert_eq!(unknown_flag("--x"), "Error: unknown flag '--x' (try 'k2 whoami --help')");
        assert!(text.contains(r#"resp=$(cli_send cli_request "/cli/whoami" "${q%&}")"#), "the request moved");
    }

    #[test]
    fn the_flags() {
        assert_eq!(flags(&[]), Flags::Run { json: false });
        assert_eq!(flags(&["--json"]), Flags::Run { json: true });
        assert_eq!(flags(&["extra", "--json", "words"]), Flags::Run { json: true });
        assert_eq!(flags(&["--bogus", "--json"]), Flags::Unknown("--bogus".into()));
        assert_eq!(flags(&["--json", "-"]), Flags::Unknown("-".into()));
        assert_eq!(flags(&["--json=1"]), Flags::Unknown("--json=1".into()));
        assert_eq!(flags(&["-j"]), Flags::Unknown("-j".into()));
    }

    #[test]
    fn the_query() {
        let env = |pairs: &[(&str, &str)]| {
            let map: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
            move |name: &str| map.get(name).cloned()
        };
        assert_eq!(query(env(&[])), "");
        assert_eq!(
            query(env(&[("K5_SESSION_ID", "s 5/é"), ("K2_CELL", "sidecar"), ("K2SO_SIDECAR_NAME", "rev&x"), ("K2_PRIMARY", "acme")])),
            "session=s%205%2F%C3%A9&cell=sidecar&sidecar_name=rev%26x&primary=acme"
        );
        assert_eq!(query(env(&[("K5_SESSION_ID", ""), ("K2_SESSION_ID", "s2"), ("K5_PRIMARY", "p")])), "session=s2&primary=p");
    }

    #[test]
    fn the_answer_as_resp_holds_it() {
        assert_eq!(substituted(b"{\"a\":1}\n\n"), b"{\"a\":1}");
        assert_eq!(substituted(b"a\0b\n \n"), b"ab\n ");
        assert_eq!(substituted(b"\n\n"), b"");
    }

    #[test]
    fn the_json_check() {
        assert!(json_has_role(br#"{"role":"canonical"}"#));
        assert!(json_has_role(br#"{"x":1,"role":null}"#));
        assert!(!json_has_role(br#"{"error":"x"}"#));
        assert!(!json_has_role(br#"[{"role":1}]"#));
        assert!(!json_has_role(b"not json"));
        assert!(!json_has_role(b""));
        assert!(!json_has_role(b"\xff"));
    }

    #[test]
    fn the_plain_form() {
        let full = r#"{"workspace":"sales","role":"sidecar","address":"sales/reviewer","primary":"sales","session":"s1","siblings":{"live":3}}"#;
        assert_eq!(
            shown(full),
            ok("workspace: sales\nrole:      sidecar\naddress:   sales/reviewer\nprimary:   sales\nsession:   s1\nothers:    3 live here (k2 sessions)\n")
        );
        // Missing keys are empty, others are Python's str().
        assert_eq!(
            shown(r#"{"role":null,"primary":1.0,"session":[1,"a"],"siblings":{"other":1}}"#),
            ok("workspace: \nrole:      None\naddress:   \nprimary:   1.0\nsession:   [1, 'a']\n")
        );
        assert_eq!(shown(r#"{"siblings":{"live":true}}"#).stdout.lines().last(), Some("others:    1 live here (k2 sessions)"));
        assert_eq!(shown(r#"{"siblings":{"live":2.7}}"#).stdout.lines().last(), Some("others:    2 live here (k2 sessions)"));
        assert_eq!(shown(r#"{"siblings":[1]}"#).stdout.lines().count(), 5);
    }

    #[test]
    fn the_errors() {
        assert_eq!(shown(""), err(&format!("{UNPARSEABLE}\n")));
        assert_eq!(shown("{"), err(&format!("{UNPARSEABLE}\n")));
        assert_eq!(human(b"\xff"), err(&format!("{UNPARSEABLE}\n")));
        assert_eq!(
            shown(r#"{"error":"cannot determine identity","hint":"run inside a workspace session"}"#),
            err("Error: cannot determine identity\nrun inside a workspace session\n")
        );
        assert_eq!(shown(r#"{"error":"x","hint":""}"#), err("Error: x\n"));
        assert_eq!(shown(r#"{"error":null}"#), err("Error: None\n"));
        assert_eq!(shown(r#"{"error":{"hint":"h","code":"c"}}"#), err("Error: h\n"));
        assert_eq!(shown(r#"{"error":{"hint":"","code":"c"}}"#), err("Error: c\n"));
        assert_eq!(shown(r#"{"error":{"code":0,"msg":"m"},"hint":"not read"}"#), err("Error: {'code': 0, 'msg': 'm'}\n"));
        assert_eq!(shown(r#"{"error":{}}"#), err("Error: {}\n"));
    }

    #[test]
    fn where_the_shell_s_python_fails() {
        let s = shown("[1]");
        assert_eq!((s.stdout.as_str(), s.exit), ("", Exit::Error));
        assert_eq!(
            s.stderr,
            "Traceback (most recent call last):\n  File \"<string>\", line 21, in <module>\n  File \"<string>\", line 20, in field\nAttributeError: 'list' object has no attribute 'get'\n"
        );
        assert!(shown("null").stderr.ends_with("AttributeError: 'NoneType' object has no attribute 'get'\n"));
        let s = shown(r#"{"role":"r","siblings":{"live":"3"}}"#);
        assert_eq!(s.exit, Exit::Error);
        assert_eq!(s.stdout.lines().count(), 5, "the five lines are out first");
        assert_eq!(
            s.stderr,
            "Traceback (most recent call last):\n  File \"<string>\", line 28, in <module>\nTypeError: %d format: a real number is required, not str\n"
        );
    }
}
