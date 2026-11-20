//! `k5 commit`, and the router's other small arms (`cli/src/router.sh`)
//! whose modules hand their command lines here: `context`, `state`,
//! `status`, `checkin`, `trust-folders` and `reviews`.
//!
//! - `context`: three lines pointing at `agent context`, exit 2.
//! - `state` and `status`: `fail_deprecated`'s lines
//!   (`cli/src/deprecation.sh`), exit 1.
//! - `checkin`: `_checkin_retired`'s lines (`cli/src/checkin-retired.sh`),
//!   exit 2.
//! - `trust-folders`: `cli_request_post /cli/trust-folders`, the answer as
//!   it came (its words are not read).
//! - `commit [-m m] [--merge]`: `Requesting AI Commit...` (or `... &
//!   Merge...`), then `cli_request_post /cli/commit` (or
//!   `/cli/commit-merge`) and the answer as it came. A refusal's body is
//!   printed with exit 0, as the shell does (a known T5 row).
//! - `reviews`: `cli_request /cli/reviews | python3 … 2>/dev/null || cat`
//!   ([`reviews`]): when the reader or the call fails, `cat` copies this
//!   command's own stdin to stdout, as the shell's does.
//!
//! Each runs after the settings and the two gates ([`crate::prelude`]);
//! each is an open tool. A request with no answer is the transport's
//! line, exit 1 (`reviews`: the line, then SIGPIPE). The shell answers:
//! `-h` or `--help` (the front sends those on; the shell's `commit` then
//! sends its request, a bash bug K2 fixes), `commit`'s `-m` as the last
//! word (`$2: unbound variable`) and `commit --merge` alone (bash before
//! 4.4 stops on its empty array), a word that is not UTF-8, a port curl
//! won't take, and a `python3` the binary can't tell.

use super::{curl_port, ljust, pipefail, py_iter, without_nuls, Shown};
use crate::daemon;
use crate::exit::Exit;
use crate::families::remind::shared::py_get;
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Error};
use k2_client::cli_name;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["commit"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

pub const TRUST_FOLDERS: &str = "/cli/trust-folders";
pub const COMMIT: &str = "/cli/commit";
pub const COMMIT_MERGE: &str = "/cli/commit-merge";
pub const REVIEWS: &str = "/cli/reviews";

/// What an arm does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// Lines on stderr, and the exit.
    Lines(Vec<String>, Exit),
    TrustFolders,
    /// The line on stdout first, the endpoint and its argument.
    Commit(&'static str, &'static str, String),
    Reviews,
}

fn deprecated(old: &str, new: &str) -> Ask {
    Ask::Lines(
        vec![
            format!("Error: '{old}' is no longer supported."),
            String::new(),
            "Use instead:".into(),
            format!("  {new}"),
            String::new(),
            concat!(
                "Run '",
                cli_name!(),
                " help-deprecated' for the full map of retired verbs."
            )
            .into(),
        ],
        Exit::Error,
    )
}

/// The command line's words (the arm's word first). `None`: the shell's.
pub fn parse(words: &[&str]) -> Option<Ask> {
    let (noun, rest) = words.split_first()?;
    Some(match *noun {
        "context" => Ask::Lines(
            vec![
                concat!(
                    "Use: ",
                    cli_name!(),
                    " agent context <list|add|remove|on|off|move|show|regen|catalog>"
                )
                .into(),
                "Day-2 management of the always-on AGENTS.md context management stack.".into(),
                concat!("See: ", cli_name!(), " agent context --help").into(),
            ],
            Exit::Usage,
        ),
        "state" => deprecated("state", "(removed — Workspace States no longer exist)"),
        "status" => deprecated(
            "status",
            concat!(cli_name!(), " organization update \"<news>\""),
        ),
        "checkin" => {
            let joined = format!(" {} ", rest.join(" "));
            let form = if joined.contains(" --done ") {
                "checkin --done"
            } else if joined.contains(" --status ") {
                "checkin --status"
            } else {
                "checkin"
            };
            let mut lines = vec![format!(
                concat!("Error: '", cli_name!(), " {}' is retired. Use instead:"),
                form
            )];
            match form {
                "checkin --done" => lines.push(
                    concat!(
                        "  finish your task: ",
                        cli_name!(),
                        " done [--blocked \"reason\"]"
                    )
                    .into(),
                ),
                "checkin --status" => lines.push(
                    concat!(
                        "  send status to your head: ",
                        cli_name!(),
                        " organization update \"<news>\""
                    )
                    .into(),
                ),
                _ => {
                    lines.push(
                        concat!(
                            "  bulletin and status: ",
                            cli_name!(),
                            " organization bulletin / ",
                            cli_name!(),
                            " organization update \"<news>\""
                        )
                        .into(),
                    );
                    lines.push(
                        concat!(
                            "  people: ",
                            cli_name!(),
                            " thread    work: ",
                            cli_name!(),
                            " tickets, ",
                            cli_name!(),
                            " inbox    finishing: ",
                            cli_name!(),
                            " done"
                        )
                        .into(),
                    );
                }
            }
            Ask::Lines(lines, Exit::Usage)
        }
        "trust-folders" => Ask::TrustFolders,
        "commit" => {
            let merge = rest.contains(&"--merge");
            let rest: Vec<&str> = rest.iter().copied().filter(|w| *w != "--merge").collect();
            if merge && rest.is_empty() {
                return None;
            }
            let mut message = String::new();
            let mut i = 0;
            while i < rest.len() {
                if matches!(rest[i], "-m" | "--message") {
                    message = rest.get(i + 1)?.to_string();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let param = if message.is_empty() {
                String::new()
            } else {
                format!("message={}", quote(&message))
            };
            if merge {
                Ask::Commit("Requesting AI Commit & Merge...", COMMIT_MERGE, param)
            } else {
                Ask::Commit("Requesting AI Commit...", COMMIT, param)
            }
        }
        "reviews" => Ask::Reviews,
        _ => return None,
    })
}

/// `k5 <arm> ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let ask = parse(&words)?;
    if !python::family_native(python::here()) {
        return None;
    }
    let mut settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    if let Ask::Lines(lines, exit) = &ask {
        for l in lines {
            out::stderr_line(l);
        }
        return Some(*exit);
    }
    settings.port = Some(settings.port_text.as_deref().and_then(curl_port)?);
    let would_refuse = passport::current(args).would_refuse();
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let passport = client.passport().is_some();
    let mut closing = Closing::default();
    let exit = match &ask {
        Ask::Reviews => {
            let piped = match client.call(Channel::Tcp, REVIEWS, Call::Request(&[])) {
                Ok(r) => {
                    closing.answer(&r, passport);
                    super::Piped {
                        body: super::piped_body(&r),
                        exit: Exit::Ok,
                    }
                }
                Err(e @ Error::NoAnswer(_)) => {
                    out::stderr_line(&e.to_string());
                    super::end_in_pipe()
                }
                Err(e) => super::Piped {
                    body: Vec::new(),
                    exit: Exit::from_curl(e.curl_code()),
                },
            };
            let shown = reviews(&String::from_utf8_lossy(&piped.body));
            let exit = pipefail(piped.exit, shown.print());
            if exit == Exit::Ok {
                exit
            } else {
                // `|| cat`: this command's own stdin, as it is.
                let mut stdout = std::io::stdout().lock();
                match std::io::copy(&mut std::io::stdin().lock(), &mut stdout) {
                    Ok(_) => Exit::Ok,
                    Err(e) => out::after_stdout_error(&e),
                }
            }
        }
        other => {
            let (endpoint, param) = match other {
                Ask::Commit(line, endpoint, param) => {
                    if let Err(e) = out::stdout_line(line) {
                        return Some(out::after_stdout_error(&e));
                    }
                    (*endpoint, param.as_str())
                }
                _ => (TRUST_FOLDERS, ""),
            };
            let args: Vec<&str> = if matches!(other, Ask::Commit(..)) {
                vec![param]
            } else {
                vec![]
            };
            match client.call(Channel::Tcp, endpoint, Call::RequestPost(&args)) {
                Ok(r) => {
                    closing.answer(&r, passport);
                    match out::write_flushed(&mut std::io::stdout().lock(), &without_nuls(&r.body))
                    {
                        Ok(()) => Exit::Ok,
                        Err(e) => out::after_stdout_error(&e),
                    }
                }
                Err(e @ Error::NoAnswer(_)) => {
                    out::stderr_line(&e.to_string());
                    Exit::Error
                }
                Err(e) => Exit::from_curl(e.curl_code()),
            }
        }
    };
    closing.flush(&mut client, exit);
    Some(exit)
}

/// `format(x, '<n')` as Python does it for the values JSON gives: a
/// string or a number padded to `n`; `None` for a type that refuses it.
fn padded(v: &Value, n: usize) -> Option<String> {
    match v {
        Value::Str(_) | Value::Int(_) | Value::Float(_) => Some(ljust(&py::str_of(v), n)),
        // `bool` formats as the int it is.
        Value::Bool(b) => Some(ljust(if *b { "1" } else { "0" }, n)),
        _ => None,
    }
}

/// `reviews`'s Python, its stderr thrown away: stdout, and exit 1 where
/// it raises.
pub fn reviews(raw: &str) -> Shown {
    let mut out = String::new();
    let r = (|| -> Option<()> {
        let data = json::parse(raw).ok()?;
        if !py::truthy(&data) {
            out.push_str("No pending reviews.\n");
            return Some(());
        }
        for r in py_iter(&data, 0).ok()? {
            let g = |k: &str, d: Value| -> Option<Value> {
                Some(py_get(&r, k).ok()?.cloned().unwrap_or(d))
            };
            let agent = g("agentName", super::s("?"))?;
            let branch = g("branch", super::s(""))?;
            let items = py_len(&g("workItems", Value::Array(vec![]))?)?;
            let files = py_len(&g("diffSummary", Value::Array(vec![]))?)?;
            out.push_str(&format!(
                "  {} {} {items} items, {files} files changed\n",
                padded(&agent, 20)?,
                padded(&branch, 30)?
            ));
        }
        Some(())
    })();
    Shown {
        stdout: out,
        stderr: String::new(),
        exit: if r.is_some() { Exit::Ok } else { Exit::Error },
    }
}

fn py_len(v: &Value) -> Option<usize> {
    super::py_len(v, 0).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arms_read_their_words_as_the_shell_does() {
        assert!(
            matches!(parse(&["context", "x"]), Some(Ask::Lines(l, Exit::Usage)) if l.len() == 3)
        );
        assert!(matches!(parse(&["state"]), Some(Ask::Lines(l, Exit::Error)) if l.len() == 6));
        assert!(
            matches!(parse(&["checkin", "a", "--status"]), Some(Ask::Lines(l, Exit::Usage)) if l.len() == 2)
        );
        assert!(matches!(parse(&["checkin"]), Some(Ask::Lines(l, _)) if l.len() == 3));
        assert_eq!(parse(&["commit", "-m"]), None);
        assert_eq!(parse(&["commit", "--merge"]), None);
        assert_eq!(
            parse(&["commit", "x", "--merge", "-m", "fix é"]),
            Some(Ask::Commit(
                "Requesting AI Commit & Merge...",
                COMMIT_MERGE,
                "message=fix%20%C3%A9".into()
            ))
        );
        assert_eq!(
            parse(&["commit"]),
            Some(Ask::Commit(
                "Requesting AI Commit...",
                COMMIT,
                String::new()
            ))
        );
    }

    #[test]
    fn reviews_pads_as_python_does() {
        let s = reviews(
            r#"[{"agentName":"ada","branch":"b","workItems":[1,2],"diffSummary":"abc"},{"agentName":7}]"#,
        );
        assert_eq!(
            s.stdout,
            format!(
                "  {:<20} {:<30} 2 items, 3 files changed\n  {:<20} {:<30} 0 items, 0 files changed\n",
                "ada", "b", "7", ""
            )
        );
        assert_eq!(reviews("[]").stdout, "No pending reviews.\n");
        assert_eq!(reviews(r#"[{"agentName":null}]"#).exit, Exit::Error);
        assert_eq!(reviews("<html>").exit, Exit::Error);
    }
}
