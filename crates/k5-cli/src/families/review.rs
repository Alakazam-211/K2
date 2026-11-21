//! `k5 review approve|reject|feedback`: a person's word on an agent's
//! branch, as the shell CLI's `cmd_review_*` (`cli/src/daemon-install.sh`).
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]). `review` is an open tool: no passport check
//!    comes after them.
//! 2. The words: `approve <agent> <branch>`, `reject <agent> [--reason
//!    r]`, `feedback <agent> --message|-m m`; a missing word is the usage
//!    line, exit 1. `reject` and `feedback` with no word at all end with
//!    1 and no line (`shift` fails under `set -e`).
//! 3. One `cli_request_post /cli/review/<verb>` and the answer as it came.
//!    A refusal's body is printed with exit 0, as the shell does (a known
//!    T5 row: `owner-fallback-impact.md`).
//! 4. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! No verb, or one it doesn't know: `Unknown review subcommand: <word>` on
//! stderr, then `cmd_help`'s daily tier on stdout (asked as the front asks
//! it, [`crate::pages::daily_tier`]), exit 1.
//!
//! The shell answers: `--reason`, `--message` or `-m` as the last word (its
//! `_need_value` line), `-h` or `--help` anywhere (the front sends those
//! on; the shell prints the usage and sends nothing), a word that is not
//! UTF-8, a port curl won't take, and a `python3` the binary
//! can't tell.

use super::{curl_port, without_nuls};
use crate::daemon;
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing};
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Error};
use k2_client::cli_name;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["review"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

pub const APPROVE: &str = "/cli/review/approve";
pub const REJECT: &str = "/cli/review/reject";
pub const FEEDBACK: &str = "/cli/review/feedback";

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The endpoint and its one argument.
    Send(&'static str, String),
    /// Lines on stderr and exit 1.
    Stop(Vec<&'static str>),
    /// No verb or one `review` has no arm for: its line, then the daily
    /// tier.
    Unknown(String),
}

/// The words after `review`. `None`: the shell's.
pub fn parse(words: &[&str]) -> Option<Ask> {
    let Some((verb, rest)) = words.split_first() else {
        return Some(Ask::Unknown(String::new()));
    };
    let arg = |i: usize| rest.get(i).copied().unwrap_or("");
    match *verb {
        "approve" => {
            let (agent, branch) = (arg(0), arg(1));
            if agent.is_empty() || branch.is_empty() {
                return Some(Ask::Stop(vec![concat!(
                    "Usage: ",
                    cli_name!(),
                    " review approve <agent> <branch>"
                )]));
            }
            Some(Ask::Send(
                APPROVE,
                format!("agent={}&branch={}", quote(agent), quote(branch)),
            ))
        }
        "reject" | "feedback" => {
            if rest.is_empty() {
                return Some(Ask::Stop(vec![]));
            }
            let agent = arg(0);
            let mut value = String::new();
            let mut i = 1;
            while i < rest.len() {
                let flag = match *verb {
                    "reject" => rest[i] == "--reason",
                    _ => matches!(rest[i], "--message" | "-m"),
                };
                if flag {
                    value = rest.get(i + 1)?.to_string();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if *verb == "reject" {
                if agent.is_empty() {
                    return Some(Ask::Stop(vec![concat!(
                        "Usage: ",
                        cli_name!(),
                        " review reject <agent> [--reason \"...\"]"
                    )]));
                }
                let mut p = format!("agent={}", quote(agent));
                if !value.is_empty() {
                    p.push_str(&format!("&reason={}", quote(&value)));
                }
                return Some(Ask::Send(REJECT, p));
            }
            if agent.is_empty() || value.is_empty() {
                return Some(Ask::Stop(vec![concat!(
                    "Usage: ",
                    cli_name!(),
                    " review feedback <agent> --message \"...\""
                )]));
            }
            Some(Ask::Send(
                FEEDBACK,
                format!("agent={}&feedback={}", quote(agent), quote(&value)),
            ))
        }
        other => Some(Ask::Unknown(other.to_string())),
    }
}

/// `k5 review ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let ask = parse(&words)?;
    if !crate::python::family_native(crate::python::here()) {
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
    settings.port = Some(settings.port_text.as_deref().and_then(curl_port)?);
    let (endpoint, param) = match ask {
        Ask::Stop(lines) => {
            for l in lines {
                out::stderr_line(l);
            }
            return Some(Exit::Error);
        }
        Ask::Send(e, p) => (e, p),
        Ask::Unknown(word) => return unknown(&word, args),
    };
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
    let exit = match client.call(Channel::Tcp, endpoint, Call::RequestPost(&[&param])) {
        Ok(r) => {
            closing.answer(&r, passport);
            match out::write_flushed(&mut std::io::stdout().lock(), &without_nuls(&r.body)) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Err(e @ Error::NoAnswer(_)) => {
            out::stderr_line(&e.to_string());
            Exit::Error
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    };
    closing.flush(&mut client, exit);
    Some(exit)
}

/// `echo "Unknown review subcommand: ${2:-}" >&2; cmd_help; exit 1`,
/// after the gates. `None`: the shell answers the daily tier's questions
/// ([`crate::pages::daily_tier`]), decided before anything is printed.
fn unknown(word: &str, args: &[OsString]) -> Option<Exit> {
    let (env, p) = super::help::shell_prelude()?;
    let text = crate::pages::daily_tier(&p, &env, || passport::current(args).would_refuse())?;
    Some(super::help::show(&crate::pages::Page {
        text: text.into(),
        stderr: false,
        exit: Exit::Error,
        lead: Some(format!("Unknown review subcommand: {word}\n")),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_shells() {
        assert_eq!(parse(&[]), Some(Ask::Unknown(String::new())));
        assert_eq!(parse(&["merge"]), Some(Ask::Unknown("merge".into())));
        assert_eq!(parse(&[""]), Some(Ask::Unknown(String::new())));
        assert_eq!(parse(&["reject"]), Some(Ask::Stop(vec![])));
        assert_eq!(parse(&["reject", "a", "--reason"]), None);
        assert_eq!(parse(&["feedback", "a", "-m"]), None);
        assert_eq!(
            parse(&["reject", "a b", "x", "--reason", "too big"]),
            Some(Ask::Send(REJECT, "agent=a%20b&reason=too%20big".into()))
        );
        assert_eq!(
            parse(&["feedback", "a", "--message", "one", "-m", "two"]),
            Some(Ask::Send(FEEDBACK, "agent=a&feedback=two".into()))
        );
        assert!(matches!(parse(&["approve", "a"]), Some(Ask::Stop(l)) if l.len() == 1));
        assert!(matches!(parse(&["feedback", "a"]), Some(Ask::Stop(l)) if l.len() == 1));
    }
}
