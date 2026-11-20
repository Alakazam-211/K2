//! `k5 agent ...`: the agent management plane, as the shell CLI's
//! `agent)` arm and its `cmd_agent_*` functions answer it
//! (`cli/src/router.sh`, `cli/src/agent.sh`).
//!
//! Native here: `conf`, `get`, `list`, `retire`, `restart` and `context`
//! (`cli/src/agent-context.sh`). Every other
//! verb, and the family alone, is still the shell's: [`run`] hands it
//! back before anything is printed or sent.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the two gates ([`crate::prelude::gate`]).
//! 2. These verbs are `open` tools in the shell's policy: outside a
//!    session the credential is the owner token (or the token set); in a
//!    session with a cell socket and a passport the shell sends the owner
//!    token from disk (COMPAT-58, `_cli_apply_auth_for_verb`), the binary
//!    the passport (`scripts/cli-parity.sh`'s named difference
//!    `owner-token-with-passport`, PRD 13.9 #4). In a session with no
//!    passport both send the owner token from disk.
//! 3. The verb's words: a word it can't take is `_agent_err bad_usage`,
//!    one line of JSON on stderr, exit 2, nothing sent.
//! 4. `_agent_conf_fetch` (every verb but `list`): `cli_request
//!    /cli/agent/conf q=<name or path>` over TCP, and its Python reader
//!    ([`conf::fetch`]).
//! 5. The verb's own Python: a printer of the conf (`conf`, `get`), of
//!    `/cli/agent/list` (`list`), or a urllib POST of its own ([`Opener`],
//!    K2's ruling D4: `/cli/agent/retire`, `/cli/workspace/ensure-pinned-chat`).
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! Answers are read from the response body, never handed on in the
//! environment. Where the shell's Python raises, this prints a traceback
//! whose last line is Python 3.12's.
//!
//! Handed to the shell, before anything is printed or sent: a word that is
//! not UTF-8, `-h` or `--help` anywhere on the line (the shell's
//! `_wants_help` prints the help and asks nothing), a `HOME` that is not
//! set or not UTF-8, a port curl won't take, and a `python3` the binary
//! can't tell.
//!
//! # Where this differs from the shell (named differences)
//!
//! - `owner-token-with-passport` (above).
//! - An answer above what one environment variable may hold (128 KiB on
//!   Linux): the shell hands `/cli/agent/list`'s answer (and the conf) to
//!   its Python in `RESP` (`CONF`), so it stops there with `Argument list
//!   too long` (exit 126). The binary prints the list. For K2 to rule on.
//!
//! [`Opener`]: crate::transport::urllib::Opener

mod conf;
mod context;
mod list;
mod retire;

use super::curl_port;
use super::feedback::write::traceback_stdin;
use super::{Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["agent"];
/// Every form is the family's; [`run`] hands back what is still the
/// shell's.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `_agent_conf_fetch`'s route.
pub const CONF: &str = "/cli/agent/conf";
/// `cmd_agent_list`'s route.
pub const LIST: &str = "/cli/agent/list";
/// `cmd_agent_retire`'s route.
pub const RETIRE: &str = "/cli/agent/retire";
/// `cmd_agent_restart`'s route.
pub const ENSURE_PINNED_CHAT: &str = "/cli/workspace/ensure-pinned-chat";

/// A native verb and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Form {
    Conf(conf::ConfArgs),
    Get(conf::GetArgs),
    List { json: bool },
    Retire(retire::RetireArgs),
    Restart(retire::RestartArgs),
    Context(context::ContextArgs),
}

pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let (verb, rest) = words.split_first()?;
    if rest.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let form = match *verb {
        "conf" => conf::parse_conf(rest).map(Form::Conf),
        "get" => conf::parse_get(rest).map(Form::Get),
        "list" => list::parse(rest).map(|json| Form::List { json }),
        "retire" => retire::parse_retire(rest).map(Form::Retire),
        "restart" => retire::parse_restart(rest).map(Form::Restart),
        "context" => Ok(Form::Context(context::parse(rest))),
        _ => return None,
    };
    if !python::family_native(python::here()) {
        return None;
    }
    std::env::var_os("HOME")?.to_str()?;
    for name in k2_client::env_names::PROJECT_PATH {
        if let Some(v) = std::env::var_os(name) {
            v.to_str()?;
        }
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    curl_port(settings.port_text.as_deref()?)?;
    Some(run_form(args, form, settings))
}

fn run_form(args: &[OsString], form: Result<Form, String>, mut settings: Settings) -> Exit {
    let form = match form {
        Ok(f) => f,
        Err(hint) => {
            out::stderr_line(&err_json("bad_usage", &hint));
            return Exit::Usage;
        }
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let exit = match form {
        Form::Conf(a) => conf::conf(&mut client, &mut closing, &a),
        Form::Get(a) => conf::get(&mut client, &mut closing, &a),
        Form::List { json } => list::list(&mut client, &mut closing, json),
        Form::Retire(a) => retire::retire(&mut client, &mut closing, &a),
        Form::Restart(a) => retire::restart(&mut client, &mut closing, &a),
        Form::Context(a) => context::run(&mut client, &mut closing, &a),
    };
    closing.flush(&mut client, exit);
    exit
}

/// The command's name, as every hint prints it.
fn cli() -> &'static str {
    k2_client::brand::CLI_NAME
}

/// The product's name, as every hint prints it.
fn product() -> &'static str {
    k2_client::brand::PRODUCT_NAME
}

// ---------------------------------------------------------------------
// Python's values, as the shell's blocks use them.

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `json.dumps(v)`: Python's defaults.
fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default())
        .expect("json.dumps with ensure_ascii writes any value json.loads gave")
}

/// `json.dumps({"error": {"code": code, "hint": hint}})`: `_agent_err`'s
/// line and the blocks' own.
fn err_json(code: &str, hint: &str) -> String {
    dumps(&obj(vec![(
        "error",
        obj(vec![("code", s(code)), ("hint", s(hint))]),
    )]))
}

/// `d.get(key)` on a dict for sure.
fn at<'a>(d: &'a Value, key: &str) -> Option<&'a Value> {
    match d {
        Value::Object(pairs) => super::get(pairs, key),
        _ => None,
    }
}

/// `d.get(key, default)` printed with `%s`.
fn str_or(v: Option<&Value>, default: &str) -> String {
    v.map_or_else(|| default.to_string(), py::str_of)
}

/// `str(v)[:n]`.
fn head(v: &Value, n: usize) -> String {
    py::str_of(v).chars().take(n).collect()
}

/// `raw[:n]` of a text.
fn cut(raw: &str, n: usize) -> String {
    raw.chars().take(n).collect()
}

/// What a block printed before it raised, then the traceback (`python3
/// -`'s `<stdin>` frames): exit 1.
fn raised(stdout: String, stderr: String, r: &Raise) -> Shown {
    Shown {
        stdout,
        stderr: stderr + &traceback_stdin(r),
        exit: Exit::Error,
    }
}

/// `str.strip()`: Python's whitespace off both ends.
fn py_strip(text: &str) -> &str {
    fn space(c: char) -> bool {
        matches!(
            c,
            '\t' | '\n'
                | '\u{b}'
                | '\u{c}'
                | '\r'
                | '\u{1c}'..='\u{1f}'
                | ' '
                | '\u{85}'
                | '\u{a0}'
                | '\u{1680}'
                | '\u{2000}'..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
        )
    }
    text.trim_matches(space)
}

/// `os.path.expanduser(p)`: `~` and `~/…` from `HOME` (set: [`run`]
/// hands a line with no `HOME` to the shell). A `~name` is kept as it is.
fn expanduser(p: &str) -> String {
    let Some(rest) = p.strip_prefix('~') else {
        return p.to_string();
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return p.to_string();
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let home = home.trim_end_matches('/');
    let joined = format!("{home}{rest}");
    if joined.is_empty() {
        "/".to_string()
    } else {
        joined
    }
}

/// `os.path.abspath(os.path.expanduser(p))`.
fn abs_user(p: &str) -> String {
    crate::out::pylib::path::abspath(&expanduser(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_is_pythons() {
        assert_eq!(py_strip(" \u{1f}claude\u{3000}\t"), "claude");
        assert_eq!(py_strip("\u{200b}x"), "\u{200b}x");
    }

    #[test]
    fn the_error_line_is_ascii_json() {
        assert_eq!(
            err_json("bad_usage", "a \u{2014} b"),
            r#"{"error": {"code": "bad_usage", "hint": "a — b"}}"#
        );
    }

    #[test]
    fn the_verbs_left_to_the_shell() {
        for words in [
            &["agent"][..],
            &["agent", ""],
            &["agent", "hire", "x"],
            &["agent", "set", "x"],
            &["agent", "template", "list"],
            &["agent", "conf", "--help"],
            &["agent", "list", "-h"],
            &["agent", "nope"],
        ] {
            let args: Vec<OsString> = words.iter().map(OsString::from).collect();
            assert_eq!(run(&args), None, "{words:?}");
        }
    }
}
