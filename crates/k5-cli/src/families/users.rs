//! `k5 users`: this server's connect-user logins, as the shell CLI's
//! `cmd_users` answers it (`cli/src/clone.sh`, the phone verbs in
//! `cli/src/users-phones.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]); `users` is the locked `users-admin` tool, so
//!    with `HOOK_SOCK` set the stale-passport or require-passport lines,
//!    exit 3 ([`crate::prelude::id_tool`]).
//! 2. The verb, `${1:-list}`, and its words ([`parse`]): a word it doesn't
//!    take is its own line (and for some, the help page), exit 2.
//! 3. A password, where the verb takes one: one line of stdin with
//!    `--password-stdin`, else two no-echo questions ([`password`]).
//!    Neither ever reaches argv or the environment: it goes from the read
//!    into the request's body.
//! 4. The requests, each `resp=$(cli_send …) || resp=""`: the answer as
//!    `$(…)` holds it, or nothing when the call failed. No answer (and a
//!    POST the cell socket may have run, and no passport in a session) is
//!    the transport's line and exit 1.
//!    - `list [--json]`: a GET of `/cli/users`;
//!    - `add <username> …`: a POST of `/cli/users/add`, then for `--role
//!      owner|admin` a POST of `/cli/users/set-role`;
//!    - `set-role`, `set-password`: one POST each; with the owner token an
//!      owner change may answer `vault_presence_required`, and from a
//!      terminal outside K2 with the vault socket there, this computer's
//!      own user check is asked ([`crate::confirm::confirm_token`]) and
//!      the change sent again with its token ([`post_checked`]);
//!    - `share <username>`: a GET of `/cli/users`, or with a flag a POST
//!      of `/cli/users/set-shares`;
//!    - `audit [--tail N] [--json]`, `set-can-clone`, `unlock`: one call;
//!    - `pair-phone`, `phones`, `unpair-phone` ([`phones`]): one call, and
//!      for `pair-phone` from a login, its password again and a second.
//!
//!    Each answer goes to the verb's reader ([`read`]), which prints what
//!    the shell's Python prints, its traceback included where it raises.
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! Still the shell's: a word that is not UTF-8, a command line with `-h`
//! or `--help` (the front sends those on; the router prints the help page
//! and acts on nothing), `HOOK_SOCK` set with a `CODEX_` variable
//! exported, a port that is set but is no port curl takes, a `python3` the
//! binary can't tell, the two password questions when stdin is not a
//! terminal (Python's `getpass` falls back to stdin with its own warning
//! there), a password on stdin in a locale the binary can't read Python's
//! stdin by, and an `audit --tail` that `grep` passes but is not only
//! digits (a newline in it).

pub mod phones;
pub mod read;

use crate::cards::Decode;
use crate::confirm::{self, Confirm};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::offline::Env;
use crate::out::json::{Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::prompt::{self, Secret};
use crate::python;
use crate::transport::{Call, Client, Error};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["users"];
/// The binary's, the whole row.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// None: the row is native whole.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "users-admin";

/// The routes.
pub const LIST: &str = "/cli/users";
pub const ADD: &str = "/cli/users/add";
pub const SET_ROLE: &str = "/cli/users/set-role";
pub const SET_SHARES: &str = "/cli/users/set-shares";
pub const SET_PASSWORD: &str = "/cli/users/set-password";
pub const AUDIT: &str = "/cli/users/audit";
pub const SET_CAN_CLONE: &str = "/cli/users/set-can-clone";
pub const UNLOCK: &str = "/cli/users/unlock";

/// `add` with no username, on stderr, then exit 2.
pub const ADD_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " users add <username> [--role owner|admin|member|voice_chat] [--share-agent <agent>]... [--share-garden <garden>]... [--share-source <name>]... [--fixed-password|--no-fixed-password] [--force] [--password-stdin] [--keep-password]"
);
pub const LIST_USAGE: &str = concat!("Usage: ", cli_name!(), " users list [--json]");
pub const SET_ROLE_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " users set-role <username> owner|admin|member|voice_chat [--fixed-password|--no-fixed-password]"
);
pub const SHARE_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " users share <username> [--agent <agent>]... [--garden <garden>]... [--source <name>]... [--force] [--clear] [--json]"
);
pub const SET_PASSWORD_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " users set-password <username> [--password-stdin] [--keep-password]"
);
pub const AUDIT_USAGE: &str = concat!("Usage: ", cli_name!(), " users audit [--tail N] [--json]");
pub const CAN_CLONE_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " users set-can-clone <username> on|off"
);
pub const UNLOCK_USAGE: &str = concat!("Usage: ", cli_name!(), " users unlock <username> [--json]");

/// A password on the command line: two lines, then exit 2.
pub const NO_PASSWORD_ON_ARGV: [&str; 2] = [
    "Error: passwords must not be passed on the command line.",
    "Use an interactive prompt or --password-stdin.",
];
/// `--role viewer` (the daemon's own text, `VIEWER_ROLE_REMOVED`).
pub const VIEWER_REMOVED: &str = concat!(
    "Error: the viewer role was removed: a login is owner, admin or member, or Voice & Chat only (voice_chat) for someone who should only use voice rooms. For other limited access, give the person an app pass instead (",
    cli_name!(),
    " app)."
);
/// Before an owner change, on stderr.
pub const OWNER_CHECK: &str = "Making an owner login asks for this computer's own user check.";
/// The confirm's action for an owner change.
pub const OWNER_LOGIN: &str = "owner-login";

/// How a verb's words end before anything is sent: its lines, the help
/// page after them when `page`, and the exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub lines: Vec<String>,
    pub page: bool,
    pub exit: Exit,
}

impl Stop {
    fn usage(line: impl Into<String>) -> Stop {
        Stop {
            lines: vec![line.into()],
            page: false,
            exit: Exit::Usage,
        }
    }
    fn with_page(line: impl Into<String>) -> Stop {
        Stop {
            lines: vec![line.into()],
            page: true,
            exit: Exit::Usage,
        }
    }
    /// The help page alone, exit 0 (`help`, `-h`, `--help`).
    fn help() -> Stop {
        Stop {
            lines: Vec::new(),
            page: true,
            exit: Exit::Ok,
        }
    }
    fn print(&self) -> Exit {
        for l in &self.lines {
            out::stderr_line(l);
        }
        if self.page {
            let page = crate::text::help("users").unwrap_or_default();
            let _ = out::write_flushed(&mut std::io::stderr().lock(), page.as_bytes());
        }
        self.exit
    }
}

/// Where a password comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pw {
    /// `--password-stdin`: one line.
    Stdin,
    /// Two no-echo questions.
    Ask,
}

/// `add`'s words.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Add {
    pub username: String,
    /// After `voice-chat` became `voice_chat`; empty: none given.
    pub role: String,
    pub stdin: bool,
    pub keep: bool,
    /// Each `\n`-ended, as the shell gathers them.
    pub agents: String,
    pub gardens: String,
    pub sources: String,
    /// `"1"`, `"0"` or empty.
    pub fixed: &'static str,
    pub force: bool,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    List {
        json: bool,
    },
    Add(Add),
    SetRole {
        user: String,
        role: String,
        /// `"true"`, `"false"` or empty.
        fixed: &'static str,
    },
    Share {
        user: String,
        agents: String,
        gardens: String,
        sources: String,
        set: bool,
        json: bool,
        force: bool,
    },
    SetPassword {
        user: String,
        stdin: bool,
        keep: bool,
    },
    Audit {
        tail: String,
        json: bool,
    },
    CanClone {
        user: String,
        on: bool,
    },
    Unlock {
        user: String,
        json: bool,
    },
    Phone(phones::Form),
    Stop(Stop),
}

impl Form {
    /// The password this form reads, if any.
    pub fn password(&self) -> Option<Pw> {
        let stdin = match self {
            Form::Add(a) => a.stdin,
            Form::SetPassword { stdin, .. } => *stdin,
            _ => return None,
        };
        Some(if stdin { Pw::Stdin } else { Pw::Ask })
    }
}

/// `${2:-}` after a flag, and whether `shift 2` worked.
fn next<'a>(rest: &[&'a str], i: usize) -> (&'a str, bool) {
    match rest.get(i + 1) {
        Some(v) => (v, true),
        None => ("", false),
    }
}

/// The words after `users`.
pub fn parse(words: &[&str]) -> Form {
    let sub = match words.first() {
        Some(w) if !w.is_empty() => *w,
        _ => "list",
    };
    let rest = words.get(1..).unwrap_or_default();
    match sub {
        "--help" | "-h" | "help" => Form::Stop(Stop::help()),
        "list" => parse_list(rest),
        "add" => parse_add(rest),
        "set-role" => parse_set_role(rest),
        "share" | "shares" => parse_share(rest),
        "set-password" => parse_set_password(rest),
        "audit" => parse_audit(rest),
        "set-can-clone" => parse_can_clone(rest),
        "unlock" => parse_unlock(rest),
        "pair-phone" | "phones" | "unpair-phone" => match phones::parse(sub, rest) {
            Ok(f) => Form::Phone(f),
            Err(stop) => Form::Stop(stop),
        },
        other => Form::Stop(Stop::with_page(format!(
            "Unknown '{} users' subcommand: {other}",
            cli_name!()
        ))),
    }
}

fn parse_list(rest: &[&str]) -> Form {
    let mut json = false;
    for w in rest {
        match *w {
            "--help" | "-h" => return Form::Stop(Stop::help()),
            "--json" => json = true,
            _ => return Form::Stop(Stop::usage(LIST_USAGE)),
        }
    }
    Form::List { json }
}

fn parse_add(rest: &[&str]) -> Form {
    let mut a = Add::default();
    let mut i = 0;
    let need = |what: &str| Form::Stop(Stop::usage(format!("Error: {what}")));
    while i < rest.len() {
        let w = rest[i];
        let (v, two) = next(rest, i);
        match w {
            "--fixed-password" => a.fixed = "1",
            "--no-fixed-password" => a.fixed = "0",
            "--force" => a.force = true,
            "--share-source" => {
                if v.is_empty() {
                    return need("--share-source needs a data source name");
                }
                a.sources.push_str(&format!("{v}\n"));
                i += 1;
            }
            "--role" => {
                a.role = v.to_string();
                if !two {
                    return need("--role needs a value");
                }
                i += 1;
            }
            "--share-agent" => {
                if v.is_empty() {
                    return need("--share-agent needs an agent address");
                }
                a.agents.push_str(&format!("{v}\n"));
                i += 1;
            }
            "--share-garden" => {
                if v.is_empty() {
                    return need("--share-garden needs a Garden id or name");
                }
                a.gardens.push_str(&format!("{v}\n"));
                i += 1;
            }
            "--password-stdin" => a.stdin = true,
            "--keep-password" => a.keep = true,
            "--password" | "--pass" => return no_password_on_argv(),
            "--help" | "-h" => return Form::Stop(Stop::help()),
            _ if w.starts_with("--role=") => a.role = w["--role=".len()..].to_string(),
            _ if w.starts_with("--share-agent=") => {
                a.agents
                    .push_str(&format!("{}\n", &w["--share-agent=".len()..]));
            }
            _ if w.starts_with("--share-garden=") => {
                a.gardens
                    .push_str(&format!("{}\n", &w["--share-garden=".len()..]));
            }
            _ if w.starts_with('-') => {
                return Form::Stop(Stop::with_page(format!("Error: unknown flag '{w}'")))
            }
            _ if a.username.is_empty() => a.username = w.to_string(),
            _ => return need(&format!("unexpected argument '{w}'")),
        }
        i += 1;
    }
    if a.username.is_empty() {
        return Form::Stop(Stop::usage(ADD_USAGE));
    }
    if !a.role.is_empty() {
        match a.role.as_str() {
            "owner" | "admin" | "member" => {}
            "voice_chat" | "voice-chat" => a.role = "voice_chat".into(),
            "viewer" | "Viewer" | "VIEWER" => return Form::Stop(Stop::usage(VIEWER_REMOVED)),
            other => return need(&format!(
                "invalid role '{other}' (expected owner|admin|member|voice_chat)"
            )),
        }
    }
    if (!a.agents.is_empty() || !a.gardens.is_empty() || !a.sources.is_empty())
        && a.role != "voice_chat"
    {
        return need(
            "--share-agent, --share-garden and --share-source are only for --role voice_chat",
        );
    }
    Form::Add(a)
}

fn no_password_on_argv() -> Form {
    Form::Stop(Stop {
        lines: NO_PASSWORD_ON_ARGV.iter().map(|l| l.to_string()).collect(),
        page: false,
        exit: Exit::Usage,
    })
}

fn parse_set_role(rest: &[&str]) -> Form {
    let (mut user, mut role, mut fixed) = (String::new(), String::new(), "");
    for w in rest {
        match *w {
            "--help" | "-h" => return Form::Stop(Stop::help()),
            "--fixed-password" => fixed = "true",
            "--no-fixed-password" => fixed = "false",
            _ if w.starts_with('-') => {
                return Form::Stop(Stop::usage(format!("Error: unknown flag '{w}'")))
            }
            _ if user.is_empty() => user = w.to_string(),
            _ if role.is_empty() => role = w.to_string(),
            _ => return Form::Stop(Stop::usage(format!("Error: unexpected argument '{w}'"))),
        }
    }
    if user.is_empty() || role.is_empty() {
        return Form::Stop(Stop::usage(SET_ROLE_USAGE));
    }
    match role.as_str() {
        "owner" | "admin" | "member" | "voice_chat" => {}
        "voice-chat" => role = "voice_chat".into(),
        other => {
            return Form::Stop(Stop::usage(format!(
                "Error: invalid role '{other}' (expected owner|admin|member|voice_chat)"
            )))
        }
    }
    Form::SetRole { user, role, fixed }
}

fn parse_share(rest: &[&str]) -> Form {
    let mut user = String::new();
    let (mut agents, mut gardens, mut sources) = (String::new(), String::new(), String::new());
    let (mut set, mut json, mut force) = (false, false, false);
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let (v, _) = next(rest, i);
        let list = match w {
            "--source" | "--share-source" => Some((&mut sources, "a data source name")),
            "--agent" | "--share-agent" => Some((&mut agents, "an agent address")),
            "--garden" | "--share-garden" => Some((&mut gardens, "a Garden id or name")),
            _ => None,
        };
        if let Some((list, what)) = list {
            if v.is_empty() {
                return Form::Stop(Stop::usage(format!("Error: {w} needs {what}")));
            }
            list.push_str(&format!("{v}\n"));
            set = true;
            i += 2;
            continue;
        }
        match w {
            "--force" => force = true,
            "--clear" => set = true,
            "--json" => json = true,
            "--help" | "-h" => return Form::Stop(Stop::help()),
            _ if w.starts_with('-') => {
                return Form::Stop(Stop::usage(format!("Error: unknown flag '{w}'")))
            }
            _ if user.is_empty() => user = w.to_string(),
            _ => return Form::Stop(Stop::usage(format!("Error: unexpected argument '{w}'"))),
        }
        i += 1;
    }
    if user.is_empty() {
        return Form::Stop(Stop::usage(SHARE_USAGE));
    }
    Form::Share {
        user,
        agents,
        gardens,
        sources,
        set,
        json,
        force,
    }
}

fn parse_set_password(rest: &[&str]) -> Form {
    let (mut user, mut stdin, mut keep) = (String::new(), false, false);
    for w in rest {
        match *w {
            "--password-stdin" => stdin = true,
            "--keep-password" => keep = true,
            "--password" | "--pass" => return no_password_on_argv(),
            "--help" | "-h" => return Form::Stop(Stop::help()),
            _ if w.starts_with('-') => {
                return Form::Stop(Stop::with_page(format!("Error: unknown flag '{w}'")))
            }
            _ if user.is_empty() => user = w.to_string(),
            _ => return Form::Stop(Stop::usage(format!("Error: unexpected argument '{w}'"))),
        }
    }
    if user.is_empty() {
        return Form::Stop(Stop::usage(SET_PASSWORD_USAGE));
    }
    Form::SetPassword { user, stdin, keep }
}

fn parse_audit(rest: &[&str]) -> Form {
    let (mut tail, mut json) = (String::new(), false);
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--tail" => {
                let (v, two) = next(rest, i);
                tail = v.to_string();
                if !two {
                    return Form::Stop(Stop::usage("Error: --tail needs a value"));
                }
                i += 1;
            }
            "--json" => json = true,
            "--help" | "-h" => return Form::Stop(Stop::help()),
            _ if w.starts_with("--tail=") => tail = w["--tail=".len()..].to_string(),
            _ => {
                return Form::Stop(Stop {
                    lines: vec![
                        format!("Error: unexpected argument '{w}'"),
                        AUDIT_USAGE.to_string(),
                    ],
                    page: false,
                    exit: Exit::Usage,
                })
            }
        }
        i += 1;
    }
    // `printf '%s' "$tail_n" | grep -Eq '^[0-9]+$'`: some line is only
    // digits.
    let digits = |l: &str| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit());
    if !tail.is_empty() && !tail.split('\n').any(digits) {
        return Form::Stop(Stop::usage(format!(
            "Error: --tail must be an integer 1..1000 (got '{tail}')"
        )));
    }
    Form::Audit { tail, json }
}

fn parse_can_clone(rest: &[&str]) -> Form {
    let user = rest.first().copied().unwrap_or("");
    let on = match rest.get(1).copied().unwrap_or("") {
        "on" | "true" | "yes" | "1" => true,
        "off" | "false" | "no" | "0" => false,
        _ => return Form::Stop(Stop::usage(CAN_CLONE_USAGE)),
    };
    if user.is_empty() {
        return Form::Stop(Stop::usage(CAN_CLONE_USAGE));
    }
    Form::CanClone {
        user: user.to_string(),
        on,
    }
}

fn parse_unlock(rest: &[&str]) -> Form {
    let (mut user, mut json) = (String::new(), false);
    for w in rest {
        match *w {
            "--json" => json = true,
            "--help" | "-h" => return Form::Stop(Stop::help()),
            _ if w.starts_with('-') => {
                return Form::Stop(Stop::with_page(format!("Error: unknown flag '{w}'")))
            }
            _ if user.is_empty() => user = w.to_string(),
            _ => return Form::Stop(Stop::usage(format!("Error: unexpected argument '{w}'"))),
        }
    }
    if user.is_empty() {
        return Form::Stop(Stop::usage(UNLOCK_USAGE));
    }
    Form::Unlock { user, json }
}

/// `k5 users ...`: `args` is the whole command line after `argv[0]`.
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
    if !python::family_native(python::here()) {
        return None;
    }
    let form = parse(&words);
    if let Form::Audit { tail, .. } = &form {
        if !tail.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    // How Python reads a password from stdin: the locale's decoding.
    let mut decode = Decode::Strict;
    match form.password() {
        Some(Pw::Ask) if !prompt::stdin_is_tty() => return None,
        Some(Pw::Stdin) => decode = crate::cards::decode(&Env::current())?,
        _ => {}
    }
    if let Form::Phone(phones::Form::Pair { .. }) = form {
        decode = crate::cards::decode(&Env::current())?;
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
    let port = settings.port_text.as_deref().and_then(super::curl_port)?;
    Some(run_with(args, form, settings, port, hook_sock.as_deref(), decode))
}

fn run_with(
    args: &[OsString],
    form: Form,
    settings: Settings,
    port: u16,
    hook_sock: Option<&Path>,
    decode: Decode,
) -> Exit {
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    if let Form::Stop(stop) = &form {
        return stop.print();
    }
    let mut client = settings.client_on(port, would_refuse);
    let mut closing = Closing::default();
    let mut u = Users {
        client: &mut client,
        closing: &mut closing,
        home: &settings.home,
        hook_sock,
    };
    let exit = match u.answer(form, decode) {
        Ok(exit) => exit,
        Err(exit) => exit,
    };
    closing.flush(&mut client, exit);
    exit
}

/// A verb's run: the client, the EXIT trap's lines, the K2 home.
pub struct Users<'a> {
    pub client: &'a mut Client,
    pub closing: &'a mut Closing,
    pub home: &'a Path,
    pub hook_sock: Option<&'a Path>,
}

impl Users<'_> {
    /// `resp=$(cli_send <call> <endpoint> …) || resp=""`: the answer as
    /// `$(…)` holds it (NULs and trailing newlines gone), or nothing when
    /// the call failed. `Err`: the call ended the command (its line was
    /// printed): no answer, a POST the cell socket may have run, or no
    /// passport in a session.
    pub fn held(&mut self, endpoint: &str, call: Call<'_>) -> Result<Vec<u8>, Exit> {
        let passport = self.client.passport().is_some();
        match self.client.send(endpoint, call) {
            Ok(r) => {
                self.closing.answer(&r, passport);
                Ok(held(&r.body))
            }
            Err(
                e @ (Error::NoAnswer(_)
                | Error::MaybeApplied { .. }
                | Error::NoOwnerInSession { .. }),
            ) => {
                out::stderr_line(&e.to_string());
                Err(Exit::Error)
            }
            Err(_) => Ok(Vec::new()),
        }
    }

    /// `_users_post_checked <endpoint> <body>`: the POST; when it answers
    /// `vault_presence_required` for an owner login with a secret id, and
    /// this is a terminal outside K2 with the vault socket there, this
    /// computer's own user check, then the POST again with its token. A
    /// check that fails ends the function with 3, so the caller holds
    /// nothing.
    pub fn post_checked(&mut self, endpoint: &str, body: &str) -> Result<Vec<u8>, Exit> {
        let resp = self.held(endpoint, Call::PostJson(body.as_bytes()))?;
        let sid = read::presence_secret(&resp);
        let sock = confirm::sock_path(self.home);
        if sid.is_empty() || confirm::in_k2_terminal() || !super::live_socket(Some(&sock)) {
            return Ok(resp);
        }
        let passport = self.client.passport().is_some();
        let mut c = Confirm {
            client: self.client,
            sock: &sock,
            closing: self.closing,
            passport,
        };
        let Some(tok) = confirm::confirm_token(&mut c, OWNER_LOGIN, &sid, "") else {
            return Ok(Vec::new());
        };
        let Some(body) = confirm::with_confirm_token(body, &tok, None) else {
            return Ok(Vec::new());
        };
        self.held(endpoint, Call::PostJson(body.as_bytes()))
    }

    fn answer(&mut self, form: Form, decode: Decode) -> Result<Exit, Exit> {
        Ok(match form {
            Form::Stop(stop) => stop.print(),
            Form::List { json } => {
                let raw = self.held(LIST, Call::Request(&[]))?;
                let agent = (self.hook_sock.is_some() && self.client.passport().is_some())
                    || hook_token_set();
                read::list(&raw, json, agent).print()
            }
            Form::Add(a) => {
                let pw = password(a.stdin, decode)?;
                let body = read::add_body(&a, &pw);
                drop(pw);
                let resp = self.held(ADD, Call::PostJson(body.as_bytes()))?;
                drop(body);
                let exit = read::add(&resp, &a.username, a.keep).print();
                if exit != Exit::Ok || (a.role != "owner" && a.role != "admin") {
                    return Ok(exit);
                }
                let body = confirm::dumps(&confirm::object(vec![
                    ("username", s(&a.username)),
                    ("role", s(&a.role)),
                ]));
                if a.role == "owner" {
                    out::stderr_line(OWNER_CHECK);
                }
                let resp = self.post_checked(SET_ROLE, &body)?;
                read::add_role(&resp, &a.username, &a.role).print()
            }
            Form::SetRole { user, role, fixed } => {
                let mut pairs = vec![("username", s(&user)), ("role", s(&role))];
                if !fixed.is_empty() {
                    pairs.push(("fixedPassword", Value::Bool(fixed == "true")));
                }
                let body = confirm::dumps(&confirm::object(pairs));
                if role == "owner" {
                    out::stderr_line(OWNER_CHECK);
                }
                let resp = self.post_checked(SET_ROLE, &body)?;
                read::set_role(&resp, &user, &role).print()
            }
            Form::Share {
                user,
                agents,
                gardens,
                sources,
                set,
                json,
                force,
            } => {
                let resp = if set {
                    let mut pairs = vec![
                        ("username", s(&user)),
                        ("agents", read::listed(&agents)),
                        ("gardens", read::listed(&gardens)),
                    ];
                    if !read::kept(&sources).is_empty() {
                        pairs.push(("sources", read::listed(&sources)));
                    }
                    if force {
                        pairs.push(("force", Value::Bool(true)));
                    }
                    let body = confirm::dumps(&confirm::object(pairs));
                    self.held(SET_SHARES, Call::PostJson(body.as_bytes()))?
                } else {
                    self.held(LIST, Call::Request(&[]))?
                };
                read::share(&resp, &user, json).print()
            }
            Form::SetPassword { user, stdin, keep } => {
                let pw = password(stdin, decode)?;
                let mut pairs = vec![("username", s(&user)), ("password", pw_value(&pw))];
                drop(pw);
                if keep {
                    pairs.push(("mustChangePassword", Value::Bool(false)));
                }
                let body = confirm::dumps(&confirm::object(pairs));
                let resp = self.post_checked(SET_PASSWORD, &body)?;
                drop(body);
                read::set_password(&resp, &user).print()
            }
            Form::Audit { tail, json } => {
                let pair = format!("tail={tail}");
                let args: &[&str] = if tail.is_empty() { &[] } else { &[&pair] };
                let raw = self.held(AUDIT, Call::Request(args))?;
                if json {
                    let mut b = raw;
                    b.push(b'\n');
                    print_bytes(&b)
                } else {
                    read::audit(&raw).print()
                }
            }
            Form::CanClone { user, on } => {
                let body = confirm::dumps(&confirm::object(vec![
                    ("username", s(&user)),
                    ("canClone", Value::Bool(on)),
                ]));
                let resp = self.held(SET_CAN_CLONE, Call::PostJson(body.as_bytes()))?;
                read::can_clone(&resp, &user, on).print()
            }
            Form::Unlock { user, json } => {
                let body = confirm::dumps(&confirm::object(vec![("username", s(&user))]));
                let resp = self.held(UNLOCK, Call::PostJson(body.as_bytes()))?;
                read::unlock(&resp, json).print()
            }
            Form::Phone(f) => phones::answer(self, f, decode)?,
        })
    }
}

/// `_envn_any HOOK_TOKEN`.
fn hook_token_set() -> bool {
    k2_client::env_names::first_nonempty_os_in(k2_client::env_names::HOOK_TOKEN, |n| {
        std::env::var_os(n)
    })
    .is_some()
}

/// What `$(…)` holds of a body: NULs and trailing newlines gone.
pub fn held(body: &[u8]) -> Vec<u8> {
    let mut b = super::without_nuls(body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

/// Bytes on stdout; exit 0, or what a broken stdout ends with.
pub fn print_bytes(bytes: &[u8]) -> Exit {
    match out::write_flushed(&mut std::io::stdout().lock(), bytes) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

pub fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// A password as the body's Python reads it from its stdin: the text, or
/// with surrogate escapes in a locale that keeps bytes that are not
/// UTF-8.
pub fn pw_value(pw: &Secret) -> Value {
    confirm::text_of_bytes(pw.expose())
}

/// `password="$(_users_read_password <stdin>)" || return 1`.
///
/// - `--password-stdin`: Python's `sys.stdin.readline()`, one `\n` then
///   one `\r` taken off the end; an empty one is its line and exit 1.
///   Bytes that are not UTF-8 in a UTF-8 locale: Python's traceback,
///   exit 1.
/// - Else `getpass` twice ([`prompt::ask_hidden`]): an empty one or two
///   that differ are their line; each exit 1.
///
/// `$(…)` then drops the NULs and trailing newlines of what it printed.
pub fn password(stdin: bool, decode: Decode) -> Result<Secret, Exit> {
    if stdin {
        let line = first_line_kept();
        let mut b = line;
        if b.last() == Some(&b'\n') {
            b.pop();
        }
        if b.last() == Some(&b'\r') {
            b.pop();
        }
        if decode == Decode::Strict {
            if let Err(e) = std::str::from_utf8(&b) {
                let at = e.valid_up_to();
                let shown = super::raised(
                    String::new(),
                    String::new(),
                    &super::Raise::at(
                        3,
                        format!(
                            "UnicodeDecodeError: 'utf-8' codec can't decode byte 0x{:02x} in position {at}: invalid start byte",
                            b[at]
                        ),
                    ),
                );
                shown.print();
                return Err(Exit::Error);
            }
        }
        if b.is_empty() {
            out::stderr_line("Error: empty password on stdin.");
            return Err(Exit::Error);
        }
        return Ok(Secret::new(held(&b)));
    }
    // `getpass.getpass` twice. On a terminal the end of the input reads as
    // an empty line here, where Python says "cancelled" (both exit 1).
    let a = prompt::ask_hidden("Password: ");
    let b = prompt::ask_hidden("Confirm password: ");
    if a.is_empty() {
        out::stderr_line("Error: password must not be empty.");
        return Err(Exit::Error);
    }
    if a != b {
        out::stderr_line("Error: passwords do not match.");
        return Err(Exit::Error);
    }
    drop(b);
    Ok(Secret::new(held(a.expose())))
}

/// `sys.stdin.readline()`: up to and with the first newline, or what is
/// left. Python reads stdin in blocks; nothing reads it after this.
fn first_line_kept() -> Vec<u8> {
    use std::io::Read;
    let mut bytes = Vec::new();
    let mut stdin = std::io::stdin().lock();
    let mut one = [0u8; 1];
    loop {
        match stdin.read(&mut one) {
            Ok(0) => break,
            Ok(_) => {
                bytes.push(one[0]);
                if one[0] == b'\n' {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    bytes
}

/// `py::str_of` for an optional value (`None` when missing).
pub fn str_opt(v: Option<&Value>) -> String {
    v.map_or_else(|| "None".to_string(), py::str_of)
}

#[cfg(test)]
mod tests;
