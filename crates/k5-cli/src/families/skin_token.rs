//! `k5 skin-token`, the leftover name of `k2 app token`, as the shell CLI's
//! router and `cmd_skin_token` answer it (`cli/src/router.sh`,
//! `cli/src/app-wallet.sh`): `create`, `list`, `rooms` and `revoke`.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the two gates ([`crate::prelude`]).
//! 2. `skin-token` is the `skin` tool in the shell's policy, an `id` tool:
//!    with `HOOK_SOCK` set, a stale passport or none at all is refused
//!    (exit 3, [`crate::prelude::id_tool`]).
//! 3. The router's teaching line on stderr ([`TEACH`]), on every form.
//! 4. The words ([`parse`]). Usage lines go to stderr. A value flag
//!    (`--name`, `--caps`, `--agent`) as the last word stops on its
//!    failed `shift 2` under `set -e`: exit 1 with nothing printed.
//! 5. One request over TCP (the shell calls `cli_request` and
//!    `cli_post_json` directly, never the cell socket), held in `$(…)`:
//!    `GET /cli/skin-tokens`, or a JSON `POST` to `/cli/skin-tokens`,
//!    `/cli/skin-tokens/rooms` or `/cli/skin-tokens/revoke` whose body is
//!    what the shell's Python builds ([`create_body`], [`rooms_body`],
//!    [`revoke_body`]). No answer is the transport's line and exit 1; any
//!    other failure ends the command with curl's code (`set -e`).
//! 6. `_skin_die_if_err` on the answer ([`die_if_err`]): the daemon's
//!    error on stderr with exit 1, 2 or 3. Then the verb's printer.
//! 7. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell answers itself (`run` returns `None`): a line with `-h` or
//! `--help` (the front sends those on), a word that is not UTF-8,
//! `HOOK_SOCK` with a `CODEX_` variable exported, a port curl won't take,
//! and a `python3` the binary can't tell. Where the shell's Python raises,
//! this prints a Python 3.12 traceback. An answer that is not UTF-8 is
//! read with U+FFFD for each bad byte, as [`super::inbox`] says.

pub mod py;

use super::{curl_port, raised, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::{self, py as pyout};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::{Call, Channel, Client, Error};
use py::{at, at_or, dict, join, load, obj, or, s};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["skin-token"];
/// The shell answers it until the flip.
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "skin";
/// `_teach_skin_token_now_app_token`.
pub const TEACH: &str = concat!(
    cli_name!(),
    " skin-token is now ",
    cli_name!(),
    " app token \u{2014} this command still works."
);
/// The routes.
pub const LIST: &str = "/cli/skin-tokens";
pub const CREATE: &str = "/cli/skin-tokens";
pub const ROOMS: &str = "/cli/skin-tokens/rooms";
pub const REVOKE: &str = "/cli/skin-tokens/revoke";

/// `create`'s usage line.
pub const CREATE_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " app token create --name <label> --agent sales [--caps thread:read,thread:post,files:read,files:write]"
);
/// `create` with a positional word, or no `--name`.
pub const USE_NAME: &str = "Error: use --name (platform label), not a positional username";
/// `create` with `--user`.
pub const NOT_USER: &str = "Error: use --name (platform label), not --user/--username";
/// `create` with no `--agent`.
pub const NO_AGENT: &str = concat!(
    "Error: ",
    cli_name!(),
    " app token create needs \u{2265}1 --agent <handle> (rooms). Empty rooms = Thread dark."
);
/// `rooms`'s usage line.
pub const ROOMS_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " app token rooms <id> --agent sales  |  --clear"
);
/// `revoke`'s usage line.
pub const REVOKE_USAGE: &str = concat!("Usage: ", cli_name!(), " app token revoke <id>");
/// The family's usage, its first line alone for an unknown verb.
pub const USAGE: [&str; 6] = [
    concat!("Usage: ", cli_name!(), " app token <create|list|revoke|rooms>"),
    "  create --name <label> --agent sales [--caps thread:read,thread:post,files:read,files:write]",
    "  list",
    "  rooms <id> --agent sales",
    "  rooms <id> --clear",
    "  revoke <id>",
];
/// What `create` prints last.
pub const STORE_NOW: &str =
    "STORE THIS KEY NOW \u{2014} it is shown only once and cannot be retrieved again.";

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    /// `_skin_die_if_err`.
    pub mod die {
        pub const ERR: u32 = 14;
        pub const OK: u32 = 15;
        pub const IN_HINT: u32 = 22;
        pub const PLUS: u32 = 23;
        pub const PRINT: u32 = 24;
    }
    /// Every printer's `json.load`.
    pub const LOAD: u32 = 3;
    pub mod create {
        pub const ID: u32 = 4;
        pub const CAPS: u32 = 6;
        pub const ROOMS: u32 = 8;
        pub const TOKEN: u32 = 10;
    }
    pub mod list {
        pub const TOKENS: u32 = 4;
        pub const FOR: u32 = 7;
        pub const STATE: u32 = 8;
        pub const CAPS: u32 = 9;
        pub const ROOMS: u32 = 11;
    }
    pub mod rooms {
        pub const HANDLES: u32 = 4;
        pub const PRINT: u32 = 5;
    }
    pub mod revoke {
        pub const PRINT: u32 = 4;
    }
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Create {
        name: String,
        caps: String,
        agents: Vec<String>,
    },
    List,
    Rooms {
        id: String,
        agents: Vec<String>,
    },
    Revoke {
        id: String,
    },
}

/// Lines on stderr and the exit: the words stop here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub lines: Vec<String>,
    pub exit: Exit,
}

impl Stop {
    fn new(lines: &[&str], exit: Exit) -> Stop {
        Stop {
            lines: lines.iter().map(|l| l.to_string()).collect(),
            exit,
        }
    }

    /// A failed `shift 2` under `set -e`.
    fn shift() -> Stop {
        Stop::new(&[], Exit::Error)
    }

    fn print(&self) -> Exit {
        for l in &self.lines {
            out::stderr_line(l);
        }
        self.exit
    }
}

/// The words after `skin-token`, as `cmd_skin_token` reads them.
pub fn parse(words: &[&str]) -> Result<Ask, Stop> {
    let sub = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    match sub {
        "create" => parse_create(rest),
        "list" => Ok(Ask::List),
        "rooms" => parse_rooms(rest),
        "revoke" => match rest.first() {
            Some(id) if !id.is_empty() => Ok(Ask::Revoke { id: id.to_string() }),
            _ => Err(Stop::new(&[REVOKE_USAGE], Exit::Error)),
        },
        "" | "--help" | "-h" => Err(Stop::new(
            &USAGE,
            if sub.is_empty() { Exit::Error } else { Exit::Ok },
        )),
        _ => Err(Stop::new(&USAGE[..1], Exit::Error)),
    }
}

fn parse_create(rest: &[&str]) -> Result<Ask, Stop> {
    let (mut name, mut caps, mut agents) = (String::new(), String::new(), Vec::new());
    let mut i = 0;
    while i < rest.len() {
        let value = || rest.get(i + 1).map(|v| v.to_string()).ok_or_else(Stop::shift);
        match rest[i] {
            "--name" => name = value()?,
            "--caps" | "--cap" => caps = value()?,
            "--agent" | "--agents" => agents.push(value()?),
            "--user" | "--username" => {
                return Err(Stop::new(&[NOT_USER, CREATE_USAGE], Exit::Usage))
            }
            "--help" | "-h" => return Err(Stop::new(&[CREATE_USAGE], Exit::Ok)),
            w if w.starts_with('-') => {
                return Err(Stop {
                    lines: vec![format!("Unknown flag: {w}")],
                    exit: Exit::Error,
                })
            }
            _ => return Err(Stop::new(&[USE_NAME, CREATE_USAGE], Exit::Usage)),
        }
        i += 2;
    }
    if name.is_empty() {
        return Err(Stop::new(&[USE_NAME, CREATE_USAGE], Exit::Usage));
    }
    if agents.is_empty() {
        return Err(Stop::new(&[NO_AGENT, CREATE_USAGE], Exit::Usage));
    }
    Ok(Ask::Create { name, caps, agents })
}

fn parse_rooms(rest: &[&str]) -> Result<Ask, Stop> {
    let (mut id, mut clear, mut agents) = (String::new(), false, Vec::new());
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--agent" | "--agents" => {
                agents.push(rest.get(i + 1).ok_or_else(Stop::shift)?.to_string());
                i += 2;
                continue;
            }
            "--clear" => clear = true,
            "--help" | "-h" => return Err(Stop::new(&[ROOMS_USAGE], Exit::Ok)),
            w if w.starts_with('-') => {
                return Err(Stop {
                    lines: vec![format!("Unknown flag: {w}")],
                    exit: Exit::Error,
                })
            }
            w => {
                if id.is_empty() {
                    id = w.to_string();
                }
            }
        }
        i += 1;
    }
    if id.is_empty() {
        return Err(Stop::new(&[ROOMS_USAGE], Exit::Error));
    }
    if clear {
        agents.clear();
    }
    Ok(Ask::Rooms { id, agents })
}

/// The agents as the shell hands them to Python: each with a newline
/// after it, then `splitlines()`, stripped, the empty ones left out.
fn rooms_of(agents: &[String]) -> Value {
    let joined: String = agents.iter().map(|a| format!("{a}\n")).collect();
    Value::Array(py::line_list(&joined))
}

/// `create`'s body, as its Python prints it.
pub fn create_body(name: &str, caps: &str, agents: &[String]) -> String {
    let mut pairs = vec![("name", s(name))];
    let c = py::strip(caps);
    if !c.is_empty() {
        pairs.push(("caps", Value::Array(py::comma_list(c))));
    }
    pairs.push(("rooms", rooms_of(agents)));
    py::dumps(&obj(pairs))
}

/// `rooms`'s body.
pub fn rooms_body(id: &str, agents: &[String]) -> String {
    py::dumps(&obj(vec![("id", s(id)), ("rooms", rooms_of(agents))]))
}

/// `revoke`'s body.
pub fn revoke_body(id: &str) -> String {
    py::dumps(&obj(vec![("id", s(id))]))
}

/// `k5 skin-token ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let ask = parse(&words);
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
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(args, ask, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    ask: Result<Ask, Stop>,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    out::stderr_line(TEACH);
    let ask = match ask {
        Ok(a) => a,
        Err(stop) => return stop.print(),
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let exit = match &ask {
        Ask::List => {
            let r = client.call(Channel::Tcp, LIST, Call::Request(&[]));
            answered(&client, &mut closing, r, list_reader)
        }
        Ask::Create { name, caps, agents } => {
            let body = create_body(name, caps, agents);
            let r = client.call(Channel::Tcp, CREATE, Call::PostJson(body.as_bytes()));
            answered(&client, &mut closing, r, create_reader)
        }
        Ask::Rooms { id, agents } => {
            let body = rooms_body(id, agents);
            let r = client.call(Channel::Tcp, ROOMS, Call::PostJson(body.as_bytes()));
            answered(&client, &mut closing, r, rooms_reader)
        }
        Ask::Revoke { id } => {
            let body = revoke_body(id);
            let r = client.call(Channel::Tcp, REVOKE, Call::PostJson(body.as_bytes()));
            answered(&client, &mut closing, r, revoke_reader)
        }
    };
    closing.flush(&mut client, exit);
    exit
}

/// `raw=$(<call>)`: the answer as `$(…)` holds it (NULs and trailing
/// newlines dropped), or how the command ends: the transport's line and 1
/// when there was no answer, else curl's code (`set -e`).
pub fn held(
    client: &Client,
    closing: &mut Closing,
    result: Result<crate::transport::Response, Error>,
) -> Result<Vec<u8>, Exit> {
    match result {
        Ok(r) => {
            closing.answer(&r, client.passport().is_some());
            Ok(super::connections::held(&r.body))
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Err(Exit::Error)
        }
        Err(e) => Err(Exit::from_curl(e.curl_code())),
    }
}

/// `raw=$(<call>)`, `printf '%s' "$raw" | _skin_die_if_err`, then
/// `printf '%s' "$raw" | python3 -c '<reader>'`.
fn answered(
    client: &Client,
    closing: &mut Closing,
    result: Result<crate::transport::Response, Error>,
    reader: fn(&str) -> Shown,
) -> Exit {
    let raw = match held(client, closing, result) {
        Ok(raw) => raw,
        Err(exit) => return exit,
    };
    let exit = die_if_err(&raw).print();
    if exit != Exit::Ok {
        return exit;
    }
    reader(&String::from_utf8_lossy(&raw)).print()
}

/// `_skin_die_if_err` (`cli/src/talk-and-workspace.sh`) on `raw`: nothing
/// and exit 0 when the answer is no error; else the error on stderr and
/// its exit.
pub fn die_if_err(raw: &[u8]) -> Shown {
    let text = String::from_utf8_lossy(raw);
    let raw = py::strip(&text);
    if raw.is_empty() {
        return Shown::err(
            "Error: empty response from daemon (is it running?)\n",
            Exit::Error,
        );
    }
    let Ok(d) = crate::out::json::parse(raw) else {
        return Shown::err(format!("Error: {}\n", py::head(raw, 200)), Exit::Error);
    };
    let mut stderr = String::new();
    match die_lines(&d, &mut stderr) {
        Ok(exit) => Shown::err(stderr, exit),
        Err(r) => raised(String::new(), stderr, &r),
    }
}

/// The codes that mean "ask your human".
const OWNER_CODES: [&str; 3] = ["owner_only", "forbidden", "gated"];

fn die_lines(d: &Value, stderr: &mut String) -> Result<Exit, Raise> {
    if !matches!(d, Value::Object(_)) {
        return Ok(Exit::Ok);
    }
    let mut err = at(d, "error").cloned().unwrap_or(Value::Null);
    if err == Value::Null && at(d, "ok") == Some(&Value::Bool(false)) {
        err = d.clone();
    }
    if err == Value::Null {
        return Ok(Exit::Ok);
    }
    if let Value::Object(_) = err {
        let code = or(at(&err, "code"), s("error"));
        let hint = or(at(&err, "hint"), or(at(&err, "message"), code.clone()));
        let owner = OWNER_CODES.iter().any(|c| code == s(c));
        let hint = if owner && !py::py_in("ask your human", &hint, line::die::IN_HINT)? {
            let head = if pyout::truthy(&hint) {
                py::plus_str(&hint, line::die::PLUS)? + " \u{2014} "
            } else {
                String::new()
            };
            s(&(head + "requires owner/admin \u{2014} ask your human"))
        } else {
            hint
        };
        let text = py::str_plus(&hint, line::die::PRINT)?;
        stderr.push_str(&format!("Error: {text}\n"));
        return Ok(if owner {
            Exit::Refused
        } else if code == s("usage") {
            Exit::Usage
        } else {
            Exit::Error
        });
    }
    stderr.push_str(&format!("Error: {}\n", pyout::str_of(&err)));
    if let Value::Str(_) = err {
        let low = pyout::str_of(&err).to_lowercase();
        if ["token", "forbidden", "owner"].iter().any(|w| low.contains(w)) {
            return Ok(Exit::Refused);
        }
    }
    Ok(Exit::Error)
}

/// A printer's run on `raw`: its lines, or what it printed before it
/// raised and the traceback.
fn printed(raw: &str, lines: impl FnOnce(&Value, &mut String) -> Result<(), Raise>) -> Shown {
    let mut stdout = String::new();
    let r = load(raw, line::LOAD).and_then(|d| lines(&d, &mut stdout));
    match r {
        Ok(()) => Shown::out(stdout),
        Err(r) => raised(stdout, String::new(), &r),
    }
}

/// `", ".join(x) if x else "(none)"`.
fn joined_or_none(sep: &str, x: &Value, line: u32) -> Result<String, Raise> {
    if pyout::truthy(x) {
        join(sep, x, line)
    } else {
        Ok("(none)".to_string())
    }
}

/// `d.get("roomHandles") or d.get("room_handles") or []`.
fn handles(d: &Value) -> Value {
    or(at(d, "roomHandles"), or(at(d, "room_handles"), Value::Array(vec![])))
}

/// `create`'s printer.
pub fn create_reader(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = dict(d, line::create::ID)?;
        let name = or(at(d, "name"), at_or(d, "username", s("")));
        o.push_str(&format!(
            "Skin token created (id: {}) name {}.\n",
            pyout::str_of(&at_or(d, "id", s(""))),
            pyout::str_of(&name)
        ));
        let caps = or(at(d, "caps"), Value::Array(vec![]));
        let caps = joined_or_none(", ", &caps, line::create::CAPS)?;
        o.push_str(&format!("Capabilities: {caps}\n"));
        let rooms = joined_or_none(", ", &handles(d), line::create::ROOMS)?;
        o.push_str(&format!("Rooms: {rooms}\n\n"));
        o.push_str(&format!(
            "  {}\n\n{STORE_NOW}\n",
            pyout::str_of(&at_or(d, "token", s("")))
        ));
        Ok(())
    })
}

/// `list`'s printer.
pub fn list_reader(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = dict(d, line::list::TOKENS)?;
        let tokens = at_or(d, "tokens", Value::Array(vec![]));
        if !pyout::truthy(&tokens) {
            o.push_str("No App tokens.\n");
            return Ok(());
        }
        for t in super::py_iter(&tokens, line::list::FOR)? {
            let t = dict(&t, line::list::STATE)?;
            let state = if at(t, "revokedAt").is_some_and(pyout::truthy) {
                "revoked"
            } else {
                "active"
            };
            let caps = join(",", &or(at(t, "caps"), Value::Array(vec![])), line::list::CAPS)?;
            let caps = if caps.is_empty() { "none".into() } else { caps };
            let rooms = joined_or_none(",", &handles(t), line::list::ROOMS)?;
            let name = or(at(t, "name"), at_or(t, "username", s("")));
            o.push_str(&format!(
                "{}  [{state}]  {}  {}  caps={caps}  rooms={rooms}\n",
                pyout::str_of(&at_or(t, "id", s(""))),
                pyout::str_of(&name),
                pyout::str_of(&at_or(t, "prefix", s(""))),
            ));
        }
        Ok(())
    })
}

/// `rooms`'s printer.
pub fn rooms_reader(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = dict(d, line::rooms::HANDLES)?;
        let rooms = joined_or_none(", ", &handles(d), line::rooms::PRINT)?;
        o.push_str(&format!(
            "Skin token {} rooms: {rooms}\n",
            pyout::str_of(&at_or(d, "id", s("")))
        ));
        Ok(())
    })
}

/// `revoke`'s printer.
pub fn revoke_reader(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = dict(d, line::revoke::PRINT)?;
        o.push_str(if at(d, "success").is_some_and(pyout::truthy) {
            "Skin token revoked.\n"
        } else {
            "No matching active token (unknown or already revoked).\n"
        });
        Ok(())
    })
}

#[cfg(test)]
pub mod tests;
