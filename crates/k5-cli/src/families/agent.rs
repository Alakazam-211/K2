//! `k5 agent ...`: the agent management plane, as the shell CLI's
//! `agent)` arm and its `cmd_agent_*` functions answer it
//! (`cli/src/router.sh`, `cli/src/agent.sh`).
//!
//! Native here: `conf`, `get`, `set`, `list`, `retire`, `restart`, `context`
//! (`cli/src/agent-context.sh`), `template` (`cli/src/agent-template.sh`;
//! its help forms stay the shell's), `identity` (an `id` tool: the
//! session rules of [`crate::prelude::id_tool`]), `access`, and `harness`
//! and `refresh` (the `id` tool `agent-harness`), `hire` ([`hire`],
//! in a session too, with the session's passport), `team`,
//! `instances` and `hire-root` ([`teams`]; `team` and `instances` are the
//! `id` tool `agent-teams`), `profile` (`workspace profile`'s own code,
//! [`super::workspace`]), and the family alone, `update`, `complete`
//! and any other word ([`plane`]).
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
//!    `/cli/agent/retire`, `/cli/workspace/ensure-pinned-chat`).
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! Answers are read from the response body, never handed on in the
//! environment. Where the shell's Python raises, this prints a traceback
//! whose last line is Python 3.12's.
//!
//! Handed to the shell, before anything is printed or sent: a word that is
//! not UTF-8, `-h` or `--help` anywhere on the line (the shell's
//! `_wants_help` prints the help and asks nothing), a `HOME` that is not
//! UTF-8 (with none set, the password database's folder, as the shell's
//! Python reads it, when that is UTF-8), a port curl won't take, and a
//! `python3` the binary can't tell.
//!
//! # Where this differs from the shell (named differences)
//!
//! - `owner-token-with-passport` (above): also `hire-root` and
//!   `complete`, open verbs. The `id` tool `agent-teams` and the verbs
//!   that send the session's passport (`update`, `profile`) send what the
//!   shell sends.
//! - An answer above what one environment variable may hold (128 KiB on
//!   Linux): the shell hands `/cli/agent/list`'s answer (and the conf) to
//!   its Python in `RESP` (`CONF`), so it stops there with `Argument list
//!   too long` (exit 126). The binary prints the list. For K2 to rule on.
//!
//! [`Opener`]: crate::transport::urllib::Opener

mod conf;
mod context;
mod harness;
mod hire;
mod identity;
mod list;
mod plane;
mod retire;
mod set;
mod teams;
mod template;

use super::feedback::write::traceback_stdin;
use super::{curl_port, live_socket};
use super::{Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use std::ffi::OsString;

/// `_agent_conf_fetch <token>`, for the families that look a workspace up
/// by its conf as the shell's do (`focus-group add|remove`).
pub use conf::fetch as conf_fetch;

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

/// `team` and `instances`: the `id` tool of `_cli_tool_id_for_verb`.
pub const TEAMS_TOOL: &str = "agent-teams";

/// A native verb and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Form {
    /// The family alone: the help page.
    Bare,
    /// The router's `*)` arm.
    Unknown(String),
    Teams(teams::Ask),
    Complete(String, String),
    Update(plane::Update),
    /// Lines on stderr (maybe none) and the exit.
    Halt(Vec<String>, Exit),
    Conf(conf::ConfArgs),
    Get(conf::GetArgs),
    List {
        json: bool,
    },
    Retire(retire::RetireArgs),
    Restart(retire::RestartArgs),
    Context(context::ContextArgs),
    Template(template::Verb, bool),
    /// A template verb's usage line: plain text, not JSON.
    TemplateUsage(String),
    Identity(identity::IdentityArgs),
    Access(identity::Access),
    /// Plain lines on stderr, exit 2.
    Lines(Vec<String>),
    Harness(harness::Args),
    Set(set::SetArgs),
    Hire(Box<hire::HireArgs>),
    /// One line on stderr and its exit.
    Stop(String, Exit),
}

pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    run_words(args, &words, None)
}

/// `workspace set-agent-identity <words>`: `agent identity`'s old name
/// (`cmd_workspace_set_agent_identity`), as `agent identity <words>` with
/// `note` on stderr first, once the gates and the session's checks have
/// passed (the shell prints it in the verb's function). `args` is the
/// whole command line, for the passport. `None`: the shell's.
pub fn identity_alias(args: &[OsString], words: &[&str], note: &str) -> Option<Exit> {
    let mut all = vec!["identity"];
    all.extend_from_slice(words);
    run_words(args, &all, Some(note))
}

/// The `id` tool a word names (`_cli_tool_id_for_verb agent <word>`).
fn tool_named(word: &str) -> Option<&'static str> {
    match word {
        "identity" => Some(identity::IDENTITY_TOOL),
        "harness" | "refresh" => Some(harness::TOOL),
        "team" | "teams" | "instances" | "instance" => Some(TEAMS_TOOL),
        _ => None,
    }
}

/// `_cli_tool_id_for_args agent <words>`: the verb's tool, or past any
/// leading flags the first word's (after a `--`, the word after it).
fn tool_of(words: &[&str]) -> Option<&'static str> {
    let mut it = words.iter();
    while let Some(w) = it.next() {
        match *w {
            "-h" | "--help" => return None,
            "--" => return it.next().and_then(|w| tool_named(w)),
            w if w.starts_with('-') => {}
            w => return tool_named(w),
        }
    }
    None
}

fn run_words(args: &[OsString], words: &[&str], note: Option<&str>) -> Option<Exit> {
    let (verb, rest): (&str, &[&str]) = match words.split_first() {
        Some((v, r)) => (*v, r),
        None => ("", &[]),
    };
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    // Verbs only the shell has yet: it runs them.
    if matches!(verb, "isolation" | "whois") {
        return None;
    }
    // `agent template` alone only prints its page: the shell skips the
    // gate for it.
    if matches!(verb, "template" | "templates") && rest.first().is_none_or(|w| w.is_empty()) {
        return Some(Shown::out(plane::template_page()).print());
    }
    // `agent profile` is `cmd_workspace_profile`, with the same policy.
    if verb == "profile" {
        let mut all: Vec<OsString> = vec!["workspace".into(), "profile".into()];
        all.extend(rest.iter().map(OsString::from));
        return super::workspace::run(&all);
    }
    let hook_sock = super::hook_sock();
    // The `id` tools: the shell looks for a shared Codex server first.
    let tool = tool_of(words);
    if tool.is_some()
        && hook_sock.is_some()
        && prelude::codex_env(std::env::vars_os().map(|(name, _)| name))
    {
        return None;
    }
    let form = match verb {
        "" => Ok(Form::Bare),
        "team" | "teams" => teams_form(teams::parse_team(rest)?),
        "instances" | "instance" => teams_form(teams::parse_instances(rest)?),
        "hire-root" => teams_form(teams::parse_root(rest)),
        "complete" => match plane::parse_complete(rest) {
            Ok((agent, file)) => Ok(Form::Complete(agent, file)),
            Err(line) => Ok(Form::Halt(vec![line], Exit::Usage)),
        },
        "update" => match plane::parse_update(rest) {
            Ok(u) => Ok(Form::Update(u)),
            Err(lines) => Ok(Form::Halt(lines, Exit::Usage)),
        },
        "identity" => {
            let pwd = daemon::logical_cwd(std::env::var_os("PWD")).ok()?;
            match identity::parse_identity(rest, pwd.to_str()?)? {
                Ok(a) => Ok(Form::Identity(a)),
                Err(lines) => Ok(Form::Lines(lines)),
            }
        }
        "access" => Ok(Form::Access(identity::parse_access(rest))),
        "harness" | "refresh" => match harness::parse(verb == "harness", rest) {
            Ok(a) => Ok(Form::Harness(a)),
            Err((line, exit)) => Ok(Form::Stop(line, exit)),
        },
        "conf" => conf::parse_conf(rest).map(Form::Conf),
        "get" => conf::parse_get(rest).map(Form::Get),
        "list" => list::parse(rest).map(|json| Form::List { json }),
        "retire" => retire::parse_retire(rest).map(Form::Retire),
        "restart" => retire::parse_restart(rest).map(Form::Restart),
        "set" => set::parse(rest).map(Form::Set),
        "hire" => {
            // `_cli_caller_is_agent_passport`: a cell socket and a passport.
            let token =
                k2_client::env_names::first_nonempty_os_in(k2_client::env_names::HOOK_TOKEN, |n| {
                    std::env::var_os(n)
                });
            let agent_passport = hook_sock.is_some() && token.is_some();
            hire::parse(rest, agent_passport)?.map(|a| Form::Hire(Box::new(a)))
        }
        "context" => Ok(Form::Context(context::parse(rest))),
        "template" | "templates" => match template::parse(rest)? {
            Ok((v, json)) => Ok(Form::Template(v, json)),
            Err(line) => Ok(Form::TemplateUsage(line)),
        },
        _ => Ok(Form::Unknown(verb.to_string())),
    };
    // A recipe the binary can't tell how `python3` would decode.
    if matches!(form, Ok(Form::Teams(teams::Ask::Hire { .. })))
        && super::doc::py::decode_here().is_none()
    {
        return None;
    }
    if !python::family_native(python::here()) {
        return None;
    }
    python_home_known()?;
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
    // `update` in a session with no passport and a live cell socket: the
    // shell tries the socket with an empty Bearer first.
    if verb == "update"
        && settings.session
        && !matches!(settings.auth, crate::transport::Auth::Passport { .. })
        && live_socket(hook_sock.as_deref())
    {
        return None;
    }
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    curl_port(settings.port_text.as_deref()?)?;
    if let Some(tool) = tool {
        let would_refuse = passport::current(args).would_refuse();
        if let Some(stop) = prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, tool) {
            return Some(stop);
        }
    }
    if let Some(note) = note {
        out::stderr_line(note);
    }
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
    let form = match form {
        Form::Bare => return Shown::out(plane::page()).print(),
        Form::Unknown(word) => return plane::unknown(&word),
        Form::Halt(lines, exit) => {
            for l in lines {
                out::stderr_line(&l);
            }
            return exit;
        }
        Form::Update(u) => {
            let would_refuse = passport::current(args).would_refuse();
            let base_url = format!(
                "http://{}:{}",
                settings.host,
                settings.port_text.as_deref().unwrap_or_default()
            );
            let session = settings.session;
            let make = || settings.client(would_refuse).map_err(|e| e.to_string());
            return plane::update(make, base_url, session, &u);
        }
        other => other,
    };
    let project = settings.project.clone();
    // `_cli_caller_is_agent_passport`: a cell socket set and a passport.
    let scoped = matches!(settings.auth, crate::transport::Auth::Passport { .. });
    let agent_passport = settings.session && scoped;
    // `agent hire`: the self-grant refusal, then `_agent_hire_session` in
    // a session (a cell socket set), before anything is sent.
    let mut hire_in_session = false;
    if let Form::Hire(a) = &form {
        if a.self_grant {
            out::stderr_line(&hire::self_grant_line());
            return Exit::Refused;
        }
        if settings.session {
            if let Some(lines) = hire::session_stop(a, scoped) {
                for l in lines {
                    out::stderr_line(&l);
                }
                return Exit::Refused;
            }
            hire_in_session = true;
        }
    }
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let form = match form {
        Form::Access(identity::Access::Set(a)) => return identity::access(client, &a),
        other => other,
    };
    let mut closing = Closing::default();
    let exit = match form {
        Form::Conf(a) => conf::conf(&mut client, &mut closing, &a),
        Form::Get(a) => conf::get(&mut client, &mut closing, &a),
        Form::List { json } => list::list(&mut client, &mut closing, json),
        Form::Retire(a) => retire::retire(&mut client, &mut closing, &a),
        Form::Restart(a) => retire::restart(&mut client, &mut closing, &a),
        Form::Context(a) => context::run(&mut client, &mut closing, &a),
        Form::Template(v, json) => template::run(&mut client, &mut closing, &v, json),
        Form::TemplateUsage(line) => {
            out::stderr_line(&line);
            Exit::Usage
        }
        Form::Identity(a) => {
            identity::identity(&mut client, &mut closing, &a, &project, agent_passport)
        }
        Form::Access(identity::Access::Page) => {
            Shown::out(identity::access_page()).print();
            Exit::Usage
        }
        Form::Access(identity::Access::Usage(lines)) | Form::Lines(lines) => {
            for l in lines {
                out::stderr_line(&l);
            }
            Exit::Usage
        }
        Form::Access(identity::Access::Set(_)) => unreachable!("sent above"),
        Form::Harness(mut a) => match harness::check_refresh_ws(&mut a, scoped) {
            Ok(()) => harness::run(&mut client, &mut closing, &a),
            Err((line, exit)) => {
                out::stderr_line(&line);
                exit
            }
        },
        Form::Set(a) => set::set(&mut client, &mut closing, &a),
        Form::Hire(a) => {
            let from = hire::basename(&project);
            hire::hire(&mut client, &mut closing, &a, hire_in_session, &from)
        }
        Form::Teams(a) => teams::run(&mut client, &mut closing, &a),
        Form::Complete(agent, file) => plane::complete(&mut client, &mut closing, &agent, &file),
        Form::Bare | Form::Unknown(_) | Form::Halt(..) | Form::Update(_) => {
            unreachable!("answered above")
        }
        Form::Stop(line, exit) => {
            out::stderr_line(&line);
            exit
        }
    };
    closing.flush(&mut client, exit);
    exit
}

/// A `team`, `instances` or `hire-root` parse as a form.
fn teams_form(p: teams::Parsed) -> Result<Form, String> {
    Ok(match p {
        teams::Parsed::Ask(a) => Form::Teams(a),
        teams::Parsed::Stop(lines, exit) => Form::Halt(lines, exit),
    })
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

/// The home the shell's Python reads for `~`: `HOME` when it is set, else
/// this user's folder in the password database (bash sets a `HOME` of its
/// own when none is set, but doesn't export it). `None`: no entry for this
/// user, so Python leaves `~` as it is.
pub(crate) fn python_home() -> Option<String> {
    match std::env::var("HOME") {
        Ok(h) => Some(h),
        Err(_) => crate::out::pylib::path::passwd_home(None).flatten(),
    }
}

/// `Some` when the binary can tell [`python_home`]: a `HOME` that is
/// UTF-8, or, with none, a password database entry (or none) whose folder
/// is UTF-8.
pub(crate) fn python_home_known() -> Option<()> {
    match std::env::var_os("HOME") {
        Some(h) => h.to_str().map(|_| ()),
        None => crate::out::pylib::path::passwd_home(None).map(|_| ()),
    }
}

/// `os.path.expanduser(p)`: `~` and `~/…` from [`python_home`] ([`run`]
/// hands the line to the shell when the binary can't tell it). A `~name`
/// is kept as it is.
fn expanduser(p: &str) -> String {
    let Some(rest) = p.strip_prefix('~') else {
        return p.to_string();
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return p.to_string();
    }
    let Some(home) = python_home() else {
        return p.to_string();
    };
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
    fn the_tool_past_leading_flags() {
        assert_eq!(tool_of(&["team"]), Some(TEAMS_TOOL));
        assert_eq!(tool_of(&["--json", "-x", "instances"]), Some(TEAMS_TOOL));
        assert_eq!(tool_of(&["--", "harness"]), Some(harness::TOOL));
        assert_eq!(tool_of(&["--", "-x", "team"]), None);
        assert_eq!(tool_of(&["nope", "team"]), None);
        assert_eq!(tool_of(&["-x", "--help", "team"]), None);
        assert_eq!(tool_of(&["hire-root"]), None);
    }

    #[test]
    fn strip_is_pythons() {
        assert_eq!(py_strip(" \u{1f}claude\u{3000}\t"), "claude");
        assert_eq!(py_strip("\u{200b}x"), "\u{200b}x");
    }

    #[test]
    fn the_error_line_is_ascii_json() {
        assert_eq!(
            err_json("bad_usage", "a \u{2014} b"),
            "{\"error\": {\"code\": \"bad_usage\", \"hint\": \"a \\u2014 b\"}}"
        );
    }

    #[test]
    fn the_verbs_left_to_the_shell() {
        for words in [
            &["agent", "templates", "help"][..],
            &["agent", "team", "help"],
            &["agent", "instances", "help"],
            &["agent", "conf", "--help"],
            &["agent", "list", "-h"],
            &["agent", "nope", "-h"],
        ] {
            let args: Vec<OsString> = words.iter().map(OsString::from).collect();
            assert_eq!(run(&args), None, "{words:?}");
        }
    }
}
