//! `k5 whats-new [--all|--reset|--mark-seen]`: the changelog highlights
//! since the last version seen, as the shell CLI's `cmd_whatsnew`
//! (`cli/src/whats-new.sh`).
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]). `whats-new` is an open tool: no passport check
//!    comes after them.
//! 2. The words: `--reset`, `--all` and `--mark-seen` pick the action (the
//!    last one wins), any other word is skipped.
//! 3. `--reset`: `resp=$(cli_request_post /cli/whats_new/reset "")`, then
//!    `echo "$resp"` and a line on stderr. `--mark-seen`:
//!    `cli_request_post /cli/whats_new/mark_seen ""`, the answer as it
//!    came. Else `resp=$(cli_request /cli/whats_new "")` and the Python
//!    reader on `echo "$resp"` ([`text`]): the content when there is news
//!    (or always with `--all`), the versions on stderr; any exception is
//!    `could not parse response: <it>`, exit 1.
//! 4. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! A request held in `$(…)` with no answer is the transport's line and
//! exit 1; one that failed any other way ends the command with curl's
//! code. The shell answers: `-h` or `--help` (the front sends those on),
//! a word that is not UTF-8, a port curl won't take, and a `python3` the
//! binary can't tell.

use super::{curl_port, without_nuls, Shown};
use crate::daemon;
use crate::exit::Exit;
use crate::families::remind::shared::{decode_error, py_get};
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::{Call, Channel, Error};
use k2_client::brand::PRODUCT_NAME;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["whats-new"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

pub const SHOW: &str = "/cli/whats_new";
pub const RESET: &str = "/cli/whats_new/reset";
pub const MARK_SEEN: &str = "/cli/whats_new/mark_seen";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Show,
    All,
    Reset,
    MarkSeen,
}

/// The words after `whats-new`. `None`: help (the front's or the shell's).
pub fn parse(words: &[&str]) -> Option<Action> {
    let mut action = Action::Show;
    for w in words {
        match *w {
            "--reset" => action = Action::Reset,
            "--all" => action = Action::All,
            "--mark-seen" => action = Action::MarkSeen,
            "--help" | "-h" => return None,
            _ => {}
        }
    }
    Some(action)
}

/// `k5 whats-new ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let action = parse(&words)?;
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
    // A port curl won't take is the shell's.
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
    let call = match action {
        Action::Reset | Action::MarkSeen => Call::RequestPost(&[""]),
        Action::Show | Action::All => Call::Request(&[""]),
    };
    let endpoint = match action {
        Action::Reset => RESET,
        Action::MarkSeen => MARK_SEEN,
        _ => SHOW,
    };
    let exit = match client.call(Channel::Tcp, endpoint, call) {
        Ok(r) => {
            closing.answer(&r, passport);
            match action {
                Action::MarkSeen => write(&without_nuls(&r.body)),
                Action::Reset => {
                    let mut b = super::workspace::held(&r.body);
                    b.push(b'\n');
                    let exit = write(&b);
                    if exit == Exit::Ok {
                        out::stderr_line(&format!(
                            "({PRODUCT_NAME} will show the popup again on next app launch.)"
                        ));
                    }
                    exit
                }
                _ => {
                    let held = super::workspace::held(&r.body);
                    let raw = String::from_utf8_lossy(&held) + "\n";
                    text(&raw, action == Action::All).print()
                }
            }
        }
        Err(e @ (Error::NoAnswer(_) | Error::MaybeApplied { .. })) => {
            out::stderr_line(&e.to_string());
            Exit::Error
        }
        Err(e) => {
            // `cli_request_post` at the top prints what came: nothing.
            Exit::from_curl(e.curl_code())
        }
    };
    closing.flush(&mut client, exit);
    Some(exit)
}

fn write(bytes: &[u8]) -> Exit {
    match out::write_flushed(&mut std::io::stdout().lock(), bytes) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// `str(e)` for `x.strip()` on what is not a string.
fn strip(v: &Value) -> Result<String, String> {
    match v {
        Value::Str(_) => Ok(py::str_of(v).trim_matches(super::py_space).to_string()),
        other => Err(format!(
            "'{}' object has no attribute 'strip'",
            py::type_name(other)
        )),
    }
}

/// `"<head>" + v + "<tail>"` with `v` a string.
fn plus(head: &str, v: &Value) -> Result<String, String> {
    crate::families::remind::shared::str_plus(head, v)
}

/// The reader on `echo "$resp"`: `all` is `--all`.
pub fn text(raw: &str, all: bool) -> Shown {
    let mut out = String::new();
    let mut err = String::new();
    let run = |out: &mut String, err: &mut String| -> Result<(), String> {
        let d = json::parse(raw).map_err(|_| decode_error(raw))?;
        let g = |k: &str| -> Result<Option<Value>, String> { Ok(py_get(&d, k)?.cloned()) };
        let or = |v: Option<Value>, default: &str| {
            crate::families::remind::shared::or(v.as_ref(), super::s(default))
        };
        if all {
            let content = strip(&g("content")?.unwrap_or_else(|| super::s("")))?;
            let content = if content.is_empty() {
                "(no changelog available)".to_string()
            } else {
                content
            };
            let cv = g("current_version")?.unwrap_or_else(|| super::s("?"));
            let ls = or(g("last_seen_version")?, "(never)");
            out.push_str(&content);
            out.push('\n');
            let line = plus("\n— current: ", &cv)? + ", last seen: ";
            let line = plus(&line, &ls)? + "\n";
            err.push_str(&line);
            return Ok(());
        }
        if py::truthy(&g("has_new")?.unwrap_or(Value::Null)) {
            let content = strip(&g("content")?.unwrap_or_else(|| super::s("")))?;
            out.push_str(&content);
            out.push('\n');
            let cv = g("current_version")?.unwrap_or_else(|| super::s("?"));
            err.push_str(&(plus(&format!("\n— up through {PRODUCT_NAME} "), &cv)? + "\n"));
        } else {
            let cv = g("current_version")?.unwrap_or_else(|| super::s("?"));
            let ls = or(g("last_seen_version")?, "(never)");
            let line =
                plus(&format!("You are up to date on {PRODUCT_NAME} "), &cv)? + ". (Last seen: ";
            err.push_str(&(plus(&line, &ls)? + ")\n"));
        }
        Ok(())
    };
    match run(&mut out, &mut err) {
        Ok(()) => Shown {
            stdout: out,
            stderr: err,
            exit: Exit::Ok,
        },
        Err(e) => Shown {
            stdout: out,
            stderr: format!("{err}could not parse response: {e}\n"),
            exit: Exit::Error,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_pick_the_action() {
        assert_eq!(parse(&[]), Some(Action::Show));
        assert_eq!(parse(&["--reset", "x", "--all"]), Some(Action::All));
        assert_eq!(parse(&["--mark-seen"]), Some(Action::MarkSeen));
        assert_eq!(parse(&["--reset", "-h"]), None);
    }

    #[test]
    fn the_reader_prints_as_python_does() {
        let t = text(
            "{\"has_new\":true,\"content\":\"  News \\n\",\"current_version\":\"1.2\"}\n",
            false,
        );
        assert_eq!(t.stdout, "News\n");
        assert_eq!(t.stderr, format!("\n— up through {PRODUCT_NAME} 1.2\n"));
        let t = text("{\"current_version\":5}\n", false);
        assert_eq!(t.exit, Exit::Error);
        assert_eq!(
            t.stderr,
            "could not parse response: can only concatenate str (not \"int\") to str\n"
        );
        let t = text("{\"content\":null}\n", true);
        assert_eq!(
            t.stderr,
            "could not parse response: 'NoneType' object has no attribute 'strip'\n"
        );
    }
}
