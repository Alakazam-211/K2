//! `k5 workspace`: the verbs a first run uses, as the shell CLI's
//! `workspace` arm (`cli/src/router.sh`) hands them to their functions
//! (`cli/src/agent-context.sh`, `cli/src/wiki.sh`).
//!
//! - [`list`]: `workspace list [--running] [--json]`, every workspace this
//!   server knows (`cmd_workspace_list`).
//! - [`register`]: `workspace create <path>` and `workspace open <path>`,
//!   how a folder becomes a workspace (`cmd_workspace_create`,
//!   `cmd_workspace_open`).
//! - [`agent`]: `workspace launch [<path|name>] [--json]`, `workspace
//!   profile [<path>]` and `workspace update --field <f> --value <v>`,
//!   the workspace's agent (`cmd_workspace_launch`,
//!   `cmd_workspace_profile`, `cmd_workspace_update`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]).
//!    `workspace` with any of these verbs is an ungoverned tool, so no
//!    passport check comes after them. In a session (`HOOK_SOCK` set)
//!    these verbs send the session's passport, or no credential when it
//!    has none, never the owner token (`_cli_apply_auth_for_verb`,
//!    [`Client::no_token`]).
//! 2. The verb's words, its requests and its output (each module says
//!    which). Every request is `_ws_request` ([`ws_request`]): the cell
//!    socket first for any route, so a refusal teaches, then TCP; in its
//!    own `$(…)`. None of these verbs runs a program or writes a file of
//!    its own: the daemon does the work.
//! 3. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The route table sends each of these verbs here on its own verb row
//! ([`crate::route`]), so each can turn back to the shell alone; every
//! other verb, `workspace` alone and its help stay the shell's. The shell
//! CLI also answers these forms itself ([`run`] returns `None`): a command
//! line with `-h` or `--help` (the front sends those on,
//! [`crate::front`]); a word that is not UTF-8; a port that is set but is
//! no port curl takes; a `python3` the binary can't tell
//! ([`crate::families`], Without Python); a session with no passport whose
//! cell socket is live (the shell tries it with an empty Bearer); and the
//! forms each module names (a path the binary can't resolve as bash's
//! `cd` would).
//!
//! Where this differs from the shell:
//!
//! - Without `python3` ([`crate::python::NATIVE_WITHOUT_PYTHON3`]) each
//!   verb answers as it does with one; the shell there stops at its first
//!   `urlencode` or Python reader with its own line.
//! - An answer with a NUL in it: bash 4.4 and later also print a warning
//!   of their own when `$(…)` drops it. An answer that is not UTF-8, and
//!   half of a surrogate pair, are as [`super::inbox`] says. The daemon
//!   sends neither.
//! - An answer cut short after its head (curl 18, or a reset mid-body):
//!   where the shell prints what came, this prints nothing.

use super::{curl_port, get, hook_sock, live_socket, without_nuls};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::client::LOCAL_TRANSPORT_DENIED;
use crate::transport::{permission, Auth, Call, Client, Error};
use k2_client::brand::PRODUCT_NAME;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["workspace"];
/// The shell's but for [`VERBS`].
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// `list`, `profile`, `launch`, `update`, `create` and `open`, the
/// verbs a first run uses; every other verb, and `workspace` alone, is
/// the shell's. One verb row each, so each can turn back alone.
pub const VERBS: &[crate::route::Verb] = &[
    crate::route::Verb {
        words: &["list"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["profile"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["create"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["open"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["launch"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["update"],
        owner: crate::route::Owner::Native,
    },
];

pub mod agent;
pub mod list;
pub mod register;

/// The verbs the binary answers, as the router's `case` names them.
pub const NATIVE_VERBS: [&str; 6] = ["list", "launch", "profile", "update", "create", "open"];

/// Lines that stop a verb before it sends anything, and the exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub lines: Vec<String>,
    pub exit: Exit,
}

impl Stop {
    /// Prints the lines on stderr: the exit.
    pub fn print(&self) -> Exit {
        for l in &self.lines {
            out::stderr_line(l);
        }
        self.exit
    }
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    List(list::Ask),
    /// `profile`'s first word, if any.
    Profile(Option<String>),
    /// `launch`'s target (empty for none), or why its words stop it.
    Launch(Result<String, Stop>),
    Update(Result<agent::Update, Stop>),
    Register(register::Verb, Result<register::Ask, Stop>),
}

/// The words after `workspace` (the verb first): the form, or `None` for
/// one the shell answers.
pub fn parse(words: &[&str]) -> Option<Form> {
    let (verb, rest) = words.split_first()?;
    let first = || rest.first().map(|w| (*w).to_string());
    Some(match *verb {
        "list" => Form::List(list::parse(rest)),
        "profile" => Form::Profile(first()),
        "launch" => Form::Launch(agent::parse_launch(rest)),
        "update" => Form::Update(agent::parse_update(rest)),
        "create" => Form::Register(
            register::Verb::Create,
            register::parse(register::Verb::Create, rest),
        ),
        "open" => Form::Register(
            register::Verb::Open,
            register::parse(register::Verb::Open, rest),
        ),
        _ => return None,
    })
}

/// A form made ready to run: a register verb's path as the shell resolves
/// it.
enum Ready {
    Form(Form),
    Register(register::Verb, register::Ask, String),
}

/// `k5 workspace <verb> ...`: `args` is the whole command line after
/// `argv[0]`. `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let form = parse(&words)?;
    if !python::family_native(python::here()) {
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
    }
    // A session with no passport and a live cell socket: the shell tries
    // the socket with an empty Bearer first.
    if settings.session
        && !matches!(settings.auth, Auth::Passport { .. })
        && live_socket(hook_sock().as_deref())
    {
        return None;
    }
    let ready = match form {
        Form::Register(verb, Ok(ask)) => {
            let pwd = daemon::logical_cwd(std::env::var_os("PWD")).ok()?;
            let pwd = pwd.to_str()?.to_string();
            let cdpath = std::env::var_os("CDPATH").is_some();
            let path = register::path(verb, &ask.target, &pwd, cdpath)?;
            Ready::Register(verb, ask, path)
        }
        Form::Update(Ok(u)) => {
            // `basename "$PROJECT"` is only sure for an absolute path.
            agent::basename(&settings.project)?;
            Ready::Form(Form::Update(Ok(u)))
        }
        other => Ready::Form(other),
    };
    Some(run_with(args, ready, settings))
}

fn run_with(args: &[OsString], ready: Ready, mut settings: Settings) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    match &ready {
        Ready::Form(Form::Update(Err(stop)))
        | Ready::Form(Form::Launch(Err(stop)))
        | Ready::Form(Form::Register(_, Err(stop))) => {
            return stop.print();
        }
        _ => {}
    }
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let would_refuse = passport::current(args).would_refuse();
    // Each request runs in its own `$(…)` subshell, from the shell's state
    // as it is now: a client each, made before any request, so a passport
    // one heals is not the one the next starts with. The EXIT trap's is
    // the main shell's.
    let make = || {
        settings.client(would_refuse).map(|mut c| {
            if settings.session && matches!(c.auth, Auth::Owner(_)) {
                c.auth = Auth::None;
                c.no_token = true;
            }
            c
        })
    };
    let (mut main, mut first, mut second) = match (make(), make(), make()) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut ws = Ws {
        closing: Closing::default(),
        base_url: format!(
            "http://{}:{}",
            settings.host,
            settings.port_text.as_deref().unwrap_or_default()
        ),
    };
    let project = settings.project.clone();
    let exit = match ready {
        Ready::Form(Form::List(ask)) => list::send(&mut ws, &mut first, ask),
        Ready::Form(Form::Profile(target)) => agent::profile(&mut ws, &mut first, &project, target),
        Ready::Form(Form::Launch(Ok(target))) => {
            agent::launch(&mut ws, &mut first, &mut second, &project, target)
        }
        Ready::Form(Form::Update(Ok(u))) => {
            agent::update(&mut ws, &mut first, &mut second, &project, &u)
        }
        Ready::Register(verb, ask, path) => register::send(&mut ws, &mut first, verb, &ask, &path),
        // Printed above.
        Ready::Form(Form::Update(Err(_)) | Form::Launch(Err(_)) | Form::Register(..)) => {
            Exit::Error
        }
    };
    ws.closing.flush(&mut main, exit);
    exit
}

/// What the verbs' requests share: the EXIT trap's lines, and `$BASE_URL`
/// for `_ws_request`'s lines.
pub struct Ws {
    pub closing: Closing,
    pub base_url: String,
}

/// `_ws_request <fn> <endpoint> [args...]` (`cli/src/agent-context.sh`):
/// `_WS_ANSWER="$(_K2_UDS_TRY=1 cli_send …)"`, the cell socket first for
/// any route ([`Client::send_trying`]), as `$(…)` holds the answer
/// ([`held`]). A request K2 never answered, or one that may have been
/// applied, is the transport's line; a connect nothing took, one this
/// machine refused, and any other failure are `_ws_request`'s own line
/// ([`failed_line`]). Each ends the command with exit 1. `quiet`:
/// `_K2_PERMISSION_QUIET=1`, a lookup whose refusal is no closing line.
pub fn ws_request(
    ws: &mut Ws,
    client: &mut Client,
    endpoint: &str,
    call: Call<'_>,
    quiet: bool,
) -> Result<Vec<u8>, Exit> {
    match client.send_trying(endpoint, call) {
        Ok(mut r) => {
            if quiet {
                r.permission = None;
            }
            ws.closing.answer(&r, client.passport().is_some());
            Ok(held(&r.body))
        }
        Err(e) => {
            out::stderr_line(&failed_line(&e, endpoint, &ws.base_url));
            Err(Exit::Error)
        }
    }
}

/// The line a failed `_ws_request` ends the command with.
pub fn failed_line(e: &Error, endpoint: &str, base_url: &str) -> String {
    match e {
        Error::NoAnswer(_) | Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. } => {
            e.to_string()
        }
        _ => match e.curl_code() {
            7 => format!("Error: {PRODUCT_NAME}'s server didn't answer {endpoint}: nothing took the connection at {base_url}; run k2 daemon status"),
            LOCAL_TRANSPORT_DENIED => format!("Error: this machine refused the connection to {PRODUCT_NAME}'s server at {base_url}, so {endpoint} was not sent"),
            rc => format!("Error: the request to {PRODUCT_NAME}'s server failed ({endpoint}, curl exit {rc}); run k2 daemon status"),
        },
    }
}

/// What `$(…)` holds of a body: NULs and trailing newlines gone.
pub fn held(body: &[u8]) -> Vec<u8> {
    let mut b = without_nuls(body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

/// `raw[:200]`: the first 200 code points.
pub fn head(raw: &str) -> String {
    raw.chars().take(200).collect()
}

/// What `cli_print_or_fail "$resp" <endpoint>` (`cli/src/wiki.sh`) makes
/// of `resp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// `printf '%s\n' "$resp"`, exit 0.
    Print,
    /// `Error: <it>` on stderr; exit 3 when the body has a permission
    /// card's closing line, else 1.
    Error(String),
}

/// `cli_print_or_fail`'s Python on `resp` (what `$(…)` held), for
/// `endpoint`: an empty answer, one that is not JSON and JSON that is not
/// an object are no success; an object's `error` is the line. A Python
/// that raises (a string it can't write out) is "could not read".
pub fn verdict(resp: &[u8], endpoint: &str) -> Verdict {
    let text = String::from_utf8_lossy(resp);
    let said = match json::parse(&text) {
        Err(_) if text.chars().all(super::py_space) => Some(Ok(format!(
            "{PRODUCT_NAME}'s server gave an empty answer to {endpoint}"
        ))),
        Err(_) => Some(Ok(format!(
            "{PRODUCT_NAME}'s server answered {endpoint} with something that is not JSON: {}",
            py::repr(&super::s(&head(&text)))
        ))),
        Ok(Value::Object(pairs)) => error_of(&pairs),
        Ok(_) => Some(Ok(format!(
            "{PRODUCT_NAME}'s server answered {endpoint} with JSON that is not an object: {}",
            head(&text)
        ))),
    };
    let err = match said {
        None => return Verdict::Print,
        Some(Ok(err)) => err,
        // The Python raised: nothing printed, a failed `$(…)`.
        Some(Err(())) => {
            return Verdict::Error(format!(
                "could not read {PRODUCT_NAME}'s server answer to {endpoint}"
            ))
        }
    };
    // `err=$(…)`: NULs and trailing newlines go.
    let err: String = err.chars().filter(|c| *c != '\0').collect();
    let err = err.trim_end_matches('\n');
    if err.is_empty() {
        Verdict::Print
    } else {
        Verdict::Error(err.to_string())
    }
}

/// An object's `error`, as the Python prints it: `None` when it has none
/// (or a falsy one), `Err` when printing it raises.
fn error_of(pairs: &[(json::Text, Value)]) -> Option<Result<String, ()>> {
    let e = get(pairs, "error").filter(|e| py::truthy(e))?;
    let shown = match e {
        Value::Object(ep) => [get(ep, "hint"), get(ep, "code")]
            .into_iter()
            .flatten()
            .find(|v| py::truthy(v))
            .cloned()
            .unwrap_or_else(|| super::s("daemon refused")),
        other => {
            let trust = ["folder_trust_pending", "folder_trust_denied"]
                .iter()
                .any(|w| *other == super::s(w));
            match get(pairs, "message").filter(|m| py::truthy(m)) {
                Some(m) if trust => {
                    return Some(printable(m).map(|m| format!("{}: {m}", py::str_of(other))))
                }
                _ => other.clone(),
            }
        }
    };
    Some(printable(&shown))
}

/// `print(v)`'s text, or `Err` for a string Python can't write out.
fn printable(v: &Value) -> Result<String, ()> {
    match v {
        Value::Str(t) if t.as_str().is_none() => Err(()),
        v => Ok(py::str_of(v)),
    }
}

/// `cli_print_or_fail "$resp" <endpoint>`: prints and gives the exit.
pub fn print_or_fail(resp: &[u8], endpoint: &str) -> Exit {
    match verdict(resp, endpoint) {
        Verdict::Print => {
            let mut bytes = resp.to_vec();
            bytes.push(b'\n');
            match out::write_flushed(&mut std::io::stdout().lock(), &bytes) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Verdict::Error(err) => {
            out::stderr_line(&format!("Error: {err}"));
            refused_or(resp, Exit::Error)
        }
    }
}

/// `_permission_in_body "$resp" && exit 3`, else `exit`.
pub fn refused_or(resp: &[u8], exit: Exit) -> Exit {
    if permission::note(resp).is_some() {
        Exit::Refused
    } else {
        exit
    }
}

#[cfg(test)]
mod tests;
