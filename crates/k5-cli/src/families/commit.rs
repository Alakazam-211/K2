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
//!   `/cli/commit-merge`) and the answer as it came. `commit --merge`
//!   alone posts as bash 4.4 and newer do; bash before 4.4 stops on the
//!   empty `rest` array (the named difference `bash32-empty-array`). A refusal's body is
//!   printed with exit 0, as the shell does (a known T5 row).
//! - `reviews`: `cli_request /cli/reviews | python3 …` ([`reviews`]): the
//!   rows, or one error line and exit 1 for an answer that is not JSON, an
//!   empty one or an object (the daemon's error); a Python 3.12 traceback
//!   where the reader raises. This command's own stdin is never read. An
//!   answer whose bytes are not UTF-8 is read with each bad byte as U+FFFD
//!   (the shell's Python depends on the locale there).
//!
//! Each runs after the settings and the two gates ([`crate::prelude`]);
//! each is an open tool. A request with no answer is the transport's
//! line, exit 1 (`reviews`: the line, then SIGPIPE).
//! The shell answers: a line with `-h` or `--help` in it (the front sends
//! it to a help page or on: `commit` prints the usage, and since 0.46.17
//! `reviews` and `trust-folders` print their help, and none sends
//! anything), a word that is not UTF-8, a port curl won't take, and a
//! `python3` the binary can't tell.
//!
//! `commit`'s `-m` (or `--message`) as the last word is
//! [`out::need_value`]'s line and exit 2, nothing sent, after the gates:
//! the shell's `_need_value` line, the same.

use super::{curl_port, ljust, pipefail, py_iter, raised, without_nuls, Raise, Shown};
use crate::daemon;
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::pylib::json::loads;
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Error};
use k2_client::{cli_name, product_name};
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
            let mut message = String::new();
            let mut i = 0;
            while i < rest.len() {
                if matches!(rest[i], "-m" | "--message") {
                    let Some(value) = rest.get(i + 1) else {
                        return Some(Ask::Lines(vec![out::need_value(rest[i])], Exit::Usage));
                    };
                    message = value.to_string();
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
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
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
            pipefail(piped.exit, shown.print())
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
/// string or a number padded to `n`; the `TypeError` at `line` for a type
/// that refuses it.
fn padded(v: &Value, n: usize, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) | Value::Int(_) | Value::Float(_) => Ok(super::py_left(v, n)),
        // `bool` formats as the int it is.
        Value::Bool(b) => Ok(ljust(if *b { "1" } else { "0" }, n)),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported format string passed to {}.__format__",
                py::type_name(other)
            ),
        )),
    }
}

/// The lines of `reviews`' Python that raise (`python3 -c "` starts with a
/// newline, so its first line is empty).
pub mod line {
    pub const LOADS: u32 = 5;
    pub const FOR: u32 = 18;
    pub const AGENT: u32 = 19;
    pub const BRANCH: u32 = 20;
    pub const ITEMS: u32 = 21;
    pub const FILES: u32 = 22;
    pub const ROW: u32 = 23;
}

/// The error line's head.
const ERROR: &str = concat!("Error: ", cli_name!(), " reviews: ");
/// An empty answer (only whitespace).
const NO_ANSWER: &str = concat!("no answer from ", product_name!(), " on /cli/reviews");
/// An answer that is not JSON.
const NOT_JSON: &str = concat!(
    product_name!(),
    " answered /cli/reviews with something that is not JSON"
);
/// An object with no error in it.
const NOT_A_LIST: &str = concat!(
    product_name!(),
    " answered /cli/reviews with something that is not a list"
);

/// `reviews`' Python: stdout, its error line or traceback on stderr, and
/// its exit. An answer that is not JSON, an empty one or an object (the
/// daemon's error) is one error line and exit 1; a list prints its rows.
pub fn reviews(raw: &str) -> Shown {
    let data = match loads(raw) {
        Ok(d) => d,
        // `except ValueError:` does not catch a RecursionError.
        Err(f) if f.class == "RecursionError" => {
            return raised(
                String::new(),
                String::new(),
                &Raise::at(line::LOADS, f.last_line()),
            );
        }
        Err(_) => {
            let why = if raw.chars().all(super::py_space) {
                NO_ANSWER
            } else {
                NOT_JSON
            };
            return error(why.to_string());
        }
    };
    if let Value::Object(pairs) = &data {
        let mut err = super::get(pairs, "error").cloned();
        if let Some(Value::Object(e)) = &err {
            let hint = super::get(e, "hint");
            err = match hint.filter(|h| py::truthy(h)) {
                Some(h) => Some(h.clone()),
                None => super::get(e, "code").cloned(),
            };
        }
        return error(match err.filter(py::truthy) {
            Some(e) => py::str_of(&e),
            None => NOT_A_LIST.to_string(),
        });
    }
    if !py::truthy(&data) {
        return Shown::out("No pending reviews.\n");
    }
    let mut out = String::new();
    match rows(&data, &mut out) {
        Ok(()) => Shown::out(out),
        Err(r) => raised(out, String::new(), &r),
    }
}

/// The error line on stderr, exit 1.
fn error(why: String) -> Shown {
    Shown {
        stdout: String::new(),
        stderr: format!("{ERROR}{why}\n"),
        exit: Exit::Error,
    }
}

/// `for r in data:` and its rows, onto `out`.
fn rows(data: &Value, out: &mut String) -> Result<(), Raise> {
    for r in py_iter(data, line::FOR)? {
        let g = |k: &str, d: Value, at: u32| -> Result<Value, Raise> {
            Ok(super::py_get(&r, k, at)?.cloned().unwrap_or(d))
        };
        let agent = g("agentName", super::s("?"), line::AGENT)?;
        let branch = g("branch", super::s(""), line::BRANCH)?;
        let items = super::py_len(
            &g("workItems", Value::Array(vec![]), line::ITEMS)?,
            line::ITEMS,
        )?;
        let files = super::py_len(
            &g("diffSummary", Value::Array(vec![]), line::FILES)?,
            line::FILES,
        )?;
        let agent = padded(&agent, 20, line::ROW)?;
        let branch = padded(&branch, 30, line::ROW)?;
        out.push_str(&format!(
            "  {agent} {branch} {items} items, {files} files changed\n"
        ));
    }
    Ok(())
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
        assert_eq!(
            parse(&["commit", "-m"]),
            Some(Ask::Lines(
                vec!["Error: -m takes a value".into()],
                Exit::Usage
            ))
        );
        assert_eq!(
            parse(&["commit", "x", "--message", "--merge"]),
            Some(Ask::Lines(
                vec!["Error: --message takes a value".into()],
                Exit::Usage
            ))
        );
        assert_eq!(
            parse(&["commit", "--merge"]),
            Some(Ask::Commit(
                "Requesting AI Commit & Merge...",
                COMMIT_MERGE,
                String::new()
            ))
        );
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
        let none = reviews(r#"[{"agentName":"a"},{"agentName":null}]"#);
        assert_eq!(none.exit, Exit::Error);
        assert_eq!(none.stdout.lines().count(), 1);
        assert!(none.stderr.ends_with(
            "line 23, in <module>\nTypeError: unsupported format string passed to NoneType.__format__\n"
        ));
    }

    #[test]
    fn reviews_error_lines() {
        let head = format!("Error: {} reviews: ", cli_name!());
        for (raw, why) in [
            ("<html>", NOT_JSON),
            ("", NO_ANSWER),
            (" \n", NO_ANSWER),
            (r#"{"error":"nope"}"#, "nope"),
            (r#"{"error":{"hint":"","code":"c"}}"#, "c"),
            (r#"{"error":{"hint":"h","code":"c"}}"#, "h"),
            (r#"{"error":[1]}"#, "[1]"),
            ("{}", NOT_A_LIST),
        ] {
            let s = reviews(raw);
            assert_eq!(
                (s.stdout.as_str(), s.stderr, s.exit),
                ("", format!("{head}{why}\n"), Exit::Error),
                "{raw:?}"
            );
        }
        assert!(reviews("5")
            .stderr
            .ends_with("TypeError: 'int' object is not iterable\n"));
        assert!(reviews(r#"["x"]"#).stderr.ends_with(
            "line 19, in <module>\nAttributeError: 'str' object has no attribute 'get'\n"
        ));
    }

    /// The reader's lines, as the shell has them.
    #[test]
    fn reviews_shell_lines() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/src/daemon-install.sh");
        let text = std::fs::read_to_string(&path).expect("daemon-install.sh");
        let f = text
            .split_once("cmd_reviews() {\n")
            .expect("cmd_reviews")
            .1
            .split_once("\n}\n")
            .expect("its end")
            .0;
        let py: Vec<&str> = f
            .split_once("python3 -c \"")
            .expect("the reader")
            .1
            .lines()
            .collect();
        let at = |n: u32| py[n as usize - 1].trim();
        assert_eq!(at(line::LOADS), "data = json.loads(raw)");
        assert_eq!(at(line::FOR), "for r in data:");
        assert_eq!(at(line::AGENT), "agent = r.get('agentName', '?')");
        assert_eq!(at(line::BRANCH), "branch = r.get('branch', '')");
        assert_eq!(at(line::ITEMS), "items = len(r.get('workItems', []))");
        assert_eq!(at(line::FILES), "files = len(r.get('diffSummary', []))");
        assert_eq!(
            at(line::ROW),
            "print(f'  {agent:<20} {branch:<30} {items} items, {files} files changed')"
        );
        assert!(!f.contains("|| cat"));
        if k2_client::brand::CLI_NAME == "k2" {
            for want in [
                "    print('Error: k2 reviews: K2 answered /cli/reviews with something that is not JSON' if raw.strip() else 'Error: k2 reviews: no answer from K2 on /cli/reviews', file=sys.stderr)",
                "        err = err.get('hint') or err.get('code')",
                "    print('Error: k2 reviews: %s' % (err or 'K2 answered /cli/reviews with something that is not a list'), file=sys.stderr)",
            ] {
                assert!(f.lines().any(|l| l == want), "{want:?}");
            }
        }
    }
}
