//! `k5 respond [--final] <message>`: a response line from inside a
//! session the API started, for the API caller, as the shell CLI's
//! `cmd_respond` does (`cli/src/respond.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    the 0.40.4 rename line, and the two gates ([`crate::prelude`]).
//! 2. `respond` is an `id` tool in the shell's policy, so with `HOOK_SOCK`
//!    set: a socket that is gone with a passport the daemon refuses (the
//!    probe) is the stale-passport lines, exit 3; no passport is the
//!    require-passport lines, exit 3 ([`crate::prelude`]).
//! 3. The words: `--final` as often as it comes first, then `--` once, and
//!    the rest joined by spaces is the message (`$*`). An empty message is
//!    `Usage: k2 respond [--final] <message>` on stderr, exit 1.
//! 4. `message=<escaped>&final=<0|1>` as a form:
//!    - `HOOK_SOCK` is a socket: `cli_post_form_uds /cli/respond`, the cell
//!      socket only (never TCP after it). The answer's bytes on stdout,
//!      NULs dropped, exit 0 whatever the status; no answer is the
//!      transport's line and exit 1; any other failure is curl's code
//!      with nothing printed. Then the EXIT trap's lines
//!      ([`crate::prelude::Closing`]).
//!    - Else a passport with a `.` in it: the shell's own `_k2_curl` POST
//!      over TCP with the passport as Bearer
//!      ([`crate::transport::Client::post_form_bearer`]): the answer's
//!      bytes as they came, exit 0; any failure is curl's code with
//!      nothing printed, a no-answer one too. Nothing is noted, so the
//!      EXIT trap prints nothing.
//!    - Else `Error: 'k2 respond' needs an API-launched session (per-cell
//!      socket or scoped session token).`, exit 1.
//!
//! Neither arm sends a request again after it may have gone out: the cell
//! socket's try is the only one (no TCP fallback), and each arm's own
//! tries are for a connect nothing accepted. So there is nothing to
//! dedupe.
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`): a
//! command line with `-h` or `--help` (the front sends those on,
//! [`crate::front`]), and `HOOK_SOCK` set with a `CODEX_` variable
//! exported, where the shell looks for a shared Codex server in the
//! process's ancestry first ([`crate::prelude::codex_env`]).

use super::{curl_port, without_nuls};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing};
use crate::transport::client::is_socket;
use crate::transport::encode::quote;
use crate::transport::wire::curl;
use crate::transport::{Auth, Call, Channel, Error};
use k2_client::env_names;
use std::ffi::OsString;
use std::path::Path;

/// The route.
pub const ENDPOINT: &str = "/cli/respond";
/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "respond";
/// No message.
pub const USAGE: &str = "Usage: k2 respond [--final] <message>";
/// No cell socket and no passport to send it with.
pub const NO_SESSION: &str = "Error: 'k2 respond' needs an API-launched session (per-cell socket or scoped session token).";

/// What the words say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub last: bool,
    /// `$*`: the words after the flags, joined by spaces.
    pub message: String,
}

/// The words after `respond`, as the shell's loop reads them. `None`:
/// `-h` or `--help` among the leading flags (the shell's).
pub fn parse(words: &[&str]) -> Option<Line> {
    let mut last = false;
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--final" => {
                last = true;
                i += 1;
            }
            "--help" | "-h" => return None,
            "--" => {
                i += 1;
                break;
            }
            _ => break,
        }
    }
    Some(Line { last, message: words[i..].join(" ") })
}

/// The form both arms send.
pub fn form(line: &Line) -> [String; 2] {
    [format!("message={}", quote(&line.message)), format!("final={}", u8::from(line.last))]
}

/// `k5 respond ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = env_names::first_nonempty_os_in(env_names::HOOK_SOCK, |n| std::env::var_os(n)).map(|(_, v)| v);
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args.iter().skip(1).map(|a| a.to_str()).collect::<Option<_>>()?;
    let line = parse(&words)?;
    Some(match daemon::current() {
        Ok(settings) => run_with(args, &line, hook_sock.as_deref().map(Path::new), settings),
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            Exit::Error
        }
    })
}

fn run_with(args: &[OsString], line: &Line, hook_sock: Option<&Path>, mut settings: Settings) -> Exit {
    if let Some(notice) = prelude::rename_notice(Path::new(&settings.project)) {
        out::stderr_line(notice);
    }
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    let port = settings.port_text.as_deref().and_then(curl_port);
    let passport = match &settings.auth {
        Auth::Passport { passport, .. } => Some(passport.as_str().to_string()),
        Auth::Owner(_) | Auth::None => None,
    };
    if let Some(sock) = hook_sock {
        // `_cli_check_session_passport`: the probe asks only when the
        // socket is gone; curl can't ask on a port that is no port.
        if !is_socket(sock) && passport.is_some() {
            if let Some(port) = port {
                let probe = Settings { port: Some(port), ..settings.clone() };
                if probe.client(would_refuse).is_ok_and(|mut c| c.passport_rejected()) {
                    for l in prelude::stale_passport_lines(&sock.to_string_lossy()) {
                        out::stderr_line(&l);
                    }
                    return Exit::Refused;
                }
            }
        }
        // `_cli_apply_auth_for_verb`: an `id` tool never runs as the owner
        // in a session.
        if passport.is_none() {
            for l in prelude::require_passport_lines(TOOL) {
                out::stderr_line(&l);
            }
            return Exit::Refused;
        }
    }
    if line.message.is_empty() {
        out::stderr_line(USAGE);
        return Exit::Error;
    }
    let form = form(line);
    let form = [form[0].as_str(), form[1].as_str()];
    if hook_sock.is_some_and(is_socket) {
        // The cell socket doesn't need the port: a port that is no port
        // is only the closing probe's, which then can't connect.
        settings.port = Some(port.unwrap_or(0));
        return cell_socket(settings, would_refuse, &form);
    }
    if !passport.is_some_and(|p| p.contains('.')) {
        out::stderr_line(NO_SESSION);
        return Exit::Error;
    }
    let Some(port) = port else {
        return Exit::from_curl(curl::URL_MALFORMAT);
    };
    settings.port = Some(port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    match client.post_form_bearer(ENDPOINT, &form) {
        Ok(r) => match out::write_flushed(&mut std::io::stdout().lock(), &r.body) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        },
        Err(e) => Exit::from_curl(e.curl_code()),
    }
}

/// `cli_post_form_uds /cli/respond <form>`, then the EXIT trap.
fn cell_socket(settings: Settings, would_refuse: bool, form: &[&str]) -> Exit {
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let exit = match client.call(Channel::Uds, ENDPOINT, Call::PostForm(form)) {
        Ok(r) => {
            closing.answer(&r, true);
            match out::write_flushed(&mut std::io::stdout().lock(), &without_nuls(&r.body)) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/respond.sh");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// Each line, and the two requests, as `cmd_respond` has them.
    #[test]
    fn the_shell_s_lines() {
        let text = source();
        for shell in [
            r#"            --final) final="1"; shift ;;"#,
            r#"            --) shift; break ;;"#,
            r#"            *) break ;;"#,
            r#"    local message="$*""#,
            r#"        echo "Usage: k2 respond [--final] <message>" >&2"#,
            r#"        cli_post_form_uds "/cli/respond" "message=$(urlencode "$message")" "final=${final}""#,
            r#"    if [[ "$SCOPED_TOKEN" == *.* ]] && [ -n "$PORT" ]; then"#,
            r#"        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}""#,
            r#"        _k2_cfg_add data-raw "message=$(urlencode "$message")&final=${final}""#,
            r#"        _k2_curl "$_K2_CFG" -s -X POST "${BASE_URL}/cli/respond" \"#,
            r#"            --connect-timeout 5 --max-time 30 \"#,
            r#"            -H "Content-Type: application/x-www-form-urlencoded" 2>/dev/null"#,
            r#"    echo "Error: 'k2 respond' needs an API-launched session (per-cell socket or scoped session token)." >&2"#,
        ] {
            assert!(text.lines().any(|l| l == shell), "cmd_respond no longer has {shell:?}");
        }
        assert_eq!(USAGE, "Usage: k2 respond [--final] <message>");
        let policy = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/cli-tools-policy.sh")).unwrap();
        assert!(policy.contains(r#"        respond) echo "respond" ;;"#), "respond's tool id moved");
        assert!(policy.contains("        msg|inbox|respond|awareness|"), "respond is no longer an id tool");
    }

    fn line(last: bool, message: &str) -> Option<Line> {
        Some(Line { last, message: message.into() })
    }

    #[test]
    fn the_words() {
        assert_eq!(parse(&[]), line(false, ""));
        assert_eq!(parse(&["hello", "world"]), line(false, "hello world"));
        assert_eq!(parse(&["--final", "--final", "x"]), line(true, "x"));
        assert_eq!(parse(&["--", "--final"]), line(false, "--final"));
        assert_eq!(parse(&["--final", "--", "--", "x"]), line(true, "-- x"));
        assert_eq!(parse(&["-x", "hi"]), line(false, "-x hi"));
        assert_eq!(parse(&["hi", "--final"]), line(false, "hi --final"));
        assert_eq!(parse(&["", ""]), line(false, " "));
        assert_eq!(parse(&[""]), line(false, ""));
        assert_eq!(parse(&["--final", "--help"]), None);
        assert_eq!(parse(&["hi", "--help"]), line(false, "hi --help"));
    }

    #[test]
    fn the_form() {
        assert_eq!(form(&Line { last: true, message: "é & x=1".into() }), ["message=%C3%A9%20%26%20x%3D1".to_string(), "final=1".to_string()]);
        assert_eq!(form(&Line { last: false, message: "a".into() })[1], "final=0");
    }
}
