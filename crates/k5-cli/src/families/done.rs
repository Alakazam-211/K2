//! `k5 done [--blocked "reason"] [--agent <name>]`: mark the agent's task
//! done, or blocked with a reason, as the shell CLI's `cmd_done` does
//! (`cli/src/msg-whoami-done.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    the 0.40.4 rename line, and the two gates ([`crate::prelude`]).
//! 2. The words after `done`, as the shell's loop reads them: `--agent
//!    <name>` (a next word that is there and doesn't start with `-`, else
//!    `k2 done: '--agent' needs a name.`, exit 2), `--blocked <reason>`
//!    (any next word, else `k2 done: '--blocked' needs a reason.`, exit 2),
//!    any other word starting with `-` is `k2 done: unknown flag '<word>'.
//!    …` (exit 2), and other words are skipped. The agent starts as
//!    `AGENT_NAME` (its K5_, K2_ and K2SO_ names, empty counts as unset);
//!    none is `Error: --agent required (or set K2_AGENT_NAME)`, exit 1.
//!    Nothing is sent after any of these.
//! 3. One `cli_request_post /cli/done "agent=<agent>[&blocked=<reason>]"`
//!    over TCP (the shell calls it directly: never the cell socket). The
//!    agent and the reason are escaped as `urlencode` does. This is the
//!    0.46.16 shell's query: before it the shell sent the agent raw, so a
//!    `#` cut the URL (and lost --blocked) and a `&` added a parameter.
//! 4. The answer's bytes on stdout as they came, NULs dropped, and exit 0,
//!    whatever the status: the shell prints `out` and returns curl's code.
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! A request with no answer is the transport's line and exit 1; any other
//! failure ends with the code the shell's call returns, with nothing
//! printed ([`crate::transport::Error::curl_code`]: 7 nothing listening,
//! 56 a reset, 75 a failure that is this machine's, and so on). A port
//! that is no port is curl's 3 (or 75 when `TMPDIR` can't be used).
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`), so
//! there is one copy of each: a session the API started (`API_CELL` 1,
//! true, yes, YES or True: the router sends `done` to
//! `cmd_session_complete` there); a command line with `-h` or `--help`
//! (the front sends those on, [`crate::front`]); and a word or an agent
//! setting that is not UTF-8. Once escaped, no byte of the agent is
//! special to the transport, so no agent is handed on for its bytes.

use super::{curl_port, without_nuls};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing};
use crate::transport::client::{tmpdir_unusable, TMPDIR_UNWRITABLE};
use crate::transport::encode::quote;
use crate::transport::wire::curl;
use crate::transport::{Call, Channel, Error};
use k2_client::env_names;
use std::ffi::OsString;
use std::path::Path;

/// The route.
pub const ENDPOINT: &str = "/cli/done";

/// `--agent` with no name after it.
pub const AGENT_NEEDS_A_NAME: &str = "k2 done: '--agent' needs a name.";
/// `--blocked` with nothing after it.
pub const BLOCKED_NEEDS_A_REASON: &str = "k2 done: '--blocked' needs a reason.";
/// No agent from the flags or the environment.
pub const AGENT_REQUIRED: &str = "Error: --agent required (or set K2_AGENT_NAME)";

/// The shell's line for a flag it doesn't take.
pub fn unknown_flag(word: &str) -> String {
    format!("k2 done: unknown flag '{word}'. k2 done takes --blocked \"reason\" and --agent <name> (k2 done --help).")
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub agent: String,
    /// Empty: not blocked (`--blocked ""` too).
    pub blocked: String,
}

/// Why the words send nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// `-h` or `--help` where the loop reads a flag: the shell's.
    Help,
    /// A line for stderr and the exit.
    Usage(String, Exit),
}

/// The words after `done`, read as the shell's loop reads them. `agent`
/// is `AGENT_NAME`'s value, or empty.
pub fn flags(words: &[&str], agent: &str) -> Result<Mark, Stop> {
    let mut mark = Mark {
        agent: agent.to_string(),
        blocked: String::new(),
    };
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--agent" => match words.get(i + 1) {
                Some(name) if !name.starts_with('-') => {
                    mark.agent = (*name).to_string();
                    i += 2;
                }
                _ => return Err(Stop::Usage(AGENT_NEEDS_A_NAME.to_string(), Exit::Usage)),
            },
            "--blocked" => match words.get(i + 1) {
                Some(reason) => {
                    mark.blocked = (*reason).to_string();
                    i += 2;
                }
                None => return Err(Stop::Usage(BLOCKED_NEEDS_A_REASON.to_string(), Exit::Usage)),
            },
            "-h" | "--help" => return Err(Stop::Help),
            w if w.starts_with('-') => return Err(Stop::Usage(unknown_flag(w), Exit::Usage)),
            _ => i += 1,
        }
    }
    if mark.agent.is_empty() {
        return Err(Stop::Usage(AGENT_REQUIRED.to_string(), Exit::Error));
    }
    Ok(mark)
}

/// The query the shell hands `cli_request_post`, one argument.
pub fn params(mark: &Mark) -> String {
    let mut p = format!("agent={}", quote(&mark.agent));
    if !mark.blocked.is_empty() {
        p.push_str("&blocked=");
        p.push_str(&quote(&mark.blocked));
    }
    p
}

/// `_is_api_lifecycle_context`: `API_CELL` is 1, true, yes, YES or True.
pub fn api_lifecycle(lookup: impl Fn(&str) -> Option<OsString>) -> bool {
    env_names::first_nonempty_os_in(env_names::API_CELL, lookup)
        .is_some_and(|(_, v)| matches!(v.to_str(), Some("1" | "true" | "yes" | "YES" | "True")))
}

/// `k5 done ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let env = |name: &str| std::env::var_os(name);
    if api_lifecycle(env) {
        return None;
    }
    let agent = match env_names::first_nonempty_os_in(env_names::AGENT_NAME, env) {
        Some((_, v)) => v.into_string().ok()?,
        None => String::new(),
    };
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let read = match flags(&words, &agent) {
        Err(Stop::Help) => return None,
        other => other,
    };
    Some(match daemon::current() {
        Ok(settings) => run_with(args, read, settings),
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            Exit::Error
        }
    })
}

fn run_with(args: &[OsString], read: Result<Mark, Stop>, mut settings: Settings) -> Exit {
    if let Some(line) = prelude::rename_notice(Path::new(&settings.project)) {
        out::stderr_line(line);
    }
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let mark = match read {
        Ok(mark) => mark,
        Err(Stop::Usage(line, exit)) => {
            out::stderr_line(&line);
            return exit;
        }
        // `run` hands help to the shell before this.
        Err(Stop::Help) => return Exit::Usage,
    };
    // The gate passed, so there is a port text and a token. curl refuses a
    // port that is no port (3), and `_cli_curl` reads that as any failure.
    let Some(port) = settings.port_text.as_deref().and_then(curl_port) else {
        return Exit::from_curl(if tmpdir_unusable() {
            TMPDIR_UNWRITABLE
        } else {
            curl::URL_MALFORMAT
        });
    };
    settings.port = Some(port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let q = params(&mark);
    let passport = client.passport().is_some();
    let mut closing = Closing::default();
    let exit = match client.call(Channel::Tcp, ENDPOINT, Call::RequestPost(&[q.as_str()])) {
        Ok(r) => {
            closing.answer(&r, passport);
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
    use std::collections::HashMap;

    fn source() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/src/msg-whoami-done.sh");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// Each line, and the request, as `cmd_done` has them.
    #[test]
    fn the_shell_s_lines() {
        let text = source();
        for line in [AGENT_NEEDS_A_NAME, BLOCKED_NEEDS_A_REASON, AGENT_REQUIRED] {
            assert!(
                text.contains(&format!("echo \"{line}\" >&2")),
                "{line:?} moved"
            );
        }
        assert!(
            text.contains(r#"-*) echo "k2 done: unknown flag '$1'. k2 done takes --blocked \"reason\" and --agent <name> (k2 done --help)." >&2; exit 2 ;;"#),
            "the unknown-flag line moved"
        );
        assert_eq!(unknown_flag("-x"), "k2 done: unknown flag '-x'. k2 done takes --blocked \"reason\" and --agent <name> (k2 done --help).");
        for shell in [
            r#"[[ $# -ge 2 && "$2" != -* ]] || { echo "k2 done: '--agent' needs a name." >&2; exit 2; }"#,
            r#"[[ $# -ge 2 ]] || { echo "k2 done: '--blocked' needs a reason." >&2; exit 2; }"#,
            r#"[[ -z "$agent" ]] && { echo "Error: --agent required (or set K2_AGENT_NAME)" >&2; exit 1; }"#,
            r#"[[ -n "$blocked" ]] && params="$params&blocked=$(urlencode "$blocked")""#,
            r#"cli_request_post "/cli/done" "$params""#,
        ] {
            assert!(text.contains(shell), "cmd_done no longer has {shell:?}");
        }
        // The binary escapes the agent as the 0.46.16 shell does; the shell
        // under this line is older and sends it raw. When the fix is here,
        // pin its line instead and make tests/done_parity.rs's
        // `done_escapes_the_agent` rows parity rows against bash.
        let raw = r#"local params="agent=$agent""#;
        let fixed = r#"local params="agent=$(urlencode "$agent")""#;
        assert!(
            text.contains(raw) && !text.contains(fixed),
            "cmd_done's agent line changed: if it is 0.46.16's {fixed:?}, pin it here and compare the escaped-agent rows with bash"
        );
        let session = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../cli/src/session-complete.sh"),
        )
        .unwrap();
        assert!(
            session.contains("        1|true|yes|YES|True) return 0 ;;"),
            "_is_api_lifecycle_context's values moved"
        );
    }

    fn mark(agent: &str, blocked: &str) -> Result<Mark, Stop> {
        Ok(Mark {
            agent: agent.into(),
            blocked: blocked.into(),
        })
    }

    fn usage(line: &str) -> Result<Mark, Stop> {
        Err(Stop::Usage(line.into(), Exit::Usage))
    }

    #[test]
    fn the_flags() {
        assert_eq!(flags(&[], "t"), mark("t", ""));
        assert_eq!(flags(&["shipped the fix"], "t"), mark("t", ""));
        assert_eq!(
            flags(&["--blocked", "waiting on review"], "t"),
            mark("t", "waiting on review")
        );
        assert_eq!(
            flags(&["--agent", "other", "--blocked", "-x"], "t"),
            mark("other", "-x")
        );
        assert_eq!(
            flags(&["--blocked", "a", "--blocked", ""], "t"),
            mark("t", "")
        );
        assert_eq!(
            flags(&["--agent", ""], "t"),
            Err(Stop::Usage(AGENT_REQUIRED.into(), Exit::Error))
        );
        assert_eq!(
            flags(&[], ""),
            Err(Stop::Usage(AGENT_REQUIRED.into(), Exit::Error))
        );
        assert_eq!(flags(&["--agent"], "t"), usage(AGENT_NEEDS_A_NAME));
        assert_eq!(flags(&["--agent", "-x"], "t"), usage(AGENT_NEEDS_A_NAME));
        assert_eq!(flags(&["--blocked"], "t"), usage(BLOCKED_NEEDS_A_REASON));
        assert_eq!(
            flags(&["--bogus", "--agent"], "t"),
            usage(&unknown_flag("--bogus"))
        );
        assert_eq!(flags(&["x", "-"], ""), usage(&unknown_flag("-")));
        assert_eq!(flags(&["--json"], "t"), usage(&unknown_flag("--json")));
        // The first stop wins, before the agent check.
        assert_eq!(flags(&["-x"], ""), usage(&unknown_flag("-x")));
        assert_eq!(flags(&["--help", "--bogus"], "t"), Err(Stop::Help));
        assert_eq!(flags(&["--blocked", "--help"], "t"), mark("t", "--help"));
    }

    #[test]
    fn the_query() {
        assert_eq!(
            params(&Mark {
                agent: "tester".into(),
                blocked: String::new()
            }),
            "agent=tester"
        );
        assert_eq!(
            params(&Mark {
                agent: "a&b=c".into(),
                blocked: "é & /x".into()
            }),
            "agent=a%26b%3Dc&blocked=%C3%A9%20%26%20%2Fx"
        );
        assert_eq!(
            params(&Mark {
                agent: "a#b".into(),
                blocked: "x".into()
            }),
            "agent=a%23b&blocked=x"
        );
        assert_eq!(
            params(&Mark {
                agent: "x&blocked=y".into(),
                blocked: String::new()
            }),
            "agent=x%26blocked%3Dy"
        );
        assert_eq!(
            params(&Mark {
                agent: "a b+c\t\u{7f}".into(),
                blocked: String::new()
            }),
            "agent=a%20b%2Bc%09%7F"
        );
    }

    #[test]
    fn a_session_the_api_started() {
        let env = |pairs: &[(&str, &str)]| {
            let map: HashMap<String, OsString> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), OsString::from(v)))
                .collect();
            move |name: &str| map.get(name).cloned()
        };
        for v in ["1", "true", "yes", "YES", "True"] {
            assert!(api_lifecycle(env(&[("K2_API_CELL", v)])), "{v}");
        }
        for v in ["0", "TRUE", "Yes", "on", " 1"] {
            assert!(!api_lifecycle(env(&[("K2_API_CELL", v)])), "{v}");
        }
        assert!(!api_lifecycle(env(&[])));
        assert!(api_lifecycle(env(&[
            ("K5_API_CELL", ""),
            ("K2SO_API_CELL", "1")
        ])));
        assert!(
            !api_lifecycle(env(&[("K5_API_CELL", "0"), ("K2_API_CELL", "1")])),
            "the first that is set decides"
        );
    }
}
