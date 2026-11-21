//! `k5 skin`, the leftover name of `k2 app` for its people, roles and
//! grants, as the shell CLI's router and `cmd_skin` answer it
//! (`cli/src/router.sh`, `cli/src/app-wallet.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the two gates ([`crate::prelude`]).
//! 2. `skin` is the `skin` tool in the shell's policy, an `id` tool: with
//!    `HOOK_SOCK` set, a stale passport or none at all is refused (exit 3,
//!    [`crate::prelude::id_tool`]).
//! 3. The router's teaching line on stderr ([`TEACH`]), on every form.
//! 4. The words ([`parse`]): each verb's own loop, as `cmd_skin` has it.
//!    Usage lines go to stderr. A value flag the shell reads with
//!    `"${2:-}"; shift 2` as the last word is the shell's `_need_value`
//!    line, exit 2.
//! 5. The request over TCP (the shell calls `cli_request` and
//!    `cli_post_json` directly), held in `$(…)`; the body is what the
//!    shell's Python (or `_compute_json`) builds. No answer is the
//!    transport's line and exit 1; any other failure ends the command
//!    with curl's code (`set -e`).
//! 6. `_skin_die_if_err` on the answer ([`super::skin_token::die_if_err`]),
//!    `_skin_exit_if_held` where the verb has it ([`print::held`]), the
//!    answer as it came with `--json`, then the verb's printer
//!    ([`print`]), a Python 3.12 traceback where the shell's Python raises.
//! 7. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell answers these itself (`run` returns `None`, before anything
//! is printed):
//! - `skin` alone, an unknown verb, and the help forms: `_skin_usage`
//!   prints the App family's page through `_perm_lines`;
//! - `user chat` and `role suggest` (`_compute_call`), and `grant
//!   copy-host` (`_app_rt_post`; `app grant copy-host` is the App
//!   family's, [`super::app::guests`]); `grant hooks` with `-h` or
//!   `--help` ([`hooks`] answers the rest);
//! - test guests (`user add|remove|password … --test`, `_app_test_guest`):
//!   the shell's fix that takes the password on stdin is not on the line;
//! - `grant add` and `grant rm` with a passport: `_grant_owner_next_step`
//!   may ask the permission probe after the answer;
//! - a `K2_CJ_` name in the environment (`_compute_json` reads it);
//! - `front-door --ui-port` with a value Python's `int()` reads but the
//!   binary does not (anything but ASCII digits, a sign and `_`);
//! - a word that is not UTF-8, `HOOK_SOCK` with a `CODEX_` variable
//!   exported, a port curl won't take, and a `python3` the binary can't
//!   tell.

pub mod hooks;
pub mod print;

use super::curl_port;
use super::skin_token::{self, py};
use crate::confirm;
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::out::json::{Text, Value};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::prompt::{self, Secret};
use crate::python;
use crate::transport::{encode, Call, Channel, Client};
use k2_client::cli_name;
use py::{dumps, obj, s};
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["skin"];
/// The binary answers it ([`run`] may still hand a form to the shell).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "skin";
/// `_teach_skin_now_app`.
pub const TEACH: &str = concat!(
    cli_name!(),
    " skin is now ",
    cli_name!(),
    " app \u{2014} this command still works."
);

/// A request: a GET with its pre-encoded pairs, or a JSON POST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Req {
    Get(&'static str, Vec<String>),
    Post(&'static str, String),
    /// `pw=$(cat)` then a POST whose body has `password` at this place:
    /// the body without it, and the place. The password is read from
    /// stdin when the request is sent and never leaves the body.
    PostPassword(&'static str, String, usize),
}

/// Which printer reads the answer, with what the shell hands it in its
/// environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reader {
    UserAdd { app: String },
    UserPassword,
    UserShow,
    UserImage,
    UserEnded,
    UserSessions,
    UserSessionsJson,
    UserInvite,
    UserReset,
    UserUnlock,
    UserEmail,
    UserFullName,
    UserAgent,
    UserRemove,
    UserRooms,
    UserRole,
    UserUnassign,
    RoleCreate,
    RoleList,
    RoleRoom,
    RolePaths { handle: String },
    RoleUpdate,
    RoleRemove,
    RoleAdd,
    GrantList,
    GrantAdd,
    GrantRm,
    GrantSet,
    GrantRole { role: String },
    FrontDoor,
    Hydra,
}

/// One request, then `_skin_die_if_err`, then the steps the verb has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plain {
    pub req: Req,
    /// `_skin_exit_if_held || exit $?`.
    pub held: bool,
    /// `role room`'s own held check, before the printer.
    pub room_held: bool,
    /// `--json`: the answer as it came, and exit 0.
    pub json: bool,
    pub reader: Reader,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// One request and its printer.
    Plain(Plain),
    /// `user list`: `/cli/skin/users`, then `/cli/skin/roles`.
    UserList,
    /// `grant list`: the agent's own logins with a passport, else every
    /// grant.
    GrantList,
    /// `grant add|rm`: the shell's with a passport.
    OwnerOnly(Plain),
    /// `front-door connect|direct`: the POST, no `_skin_die_if_err`.
    FrontDoorSet { body: String, json: bool },
    /// `grant hooks …` ([`hooks`]).
    Hooks(hooks::Ask),
    /// `skin` alone or an unknown verb: `_skin_usage` (the App family's
    /// page through `_perm_lines`) on stderr, exit 1.
    Usage,
    /// `role suggest [--json]`: `_compute_call GET /cli/skin/roles/suggest`.
    RoleSuggest { json: bool },
    /// The shell answers it.
    Shell,
}

/// Lines on stderr (or stdout) and the exit: the words stop here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub stderr: Vec<String>,
    pub stdout: Vec<String>,
    pub exit: Exit,
}

impl Stop {
    fn err(lines: &[&str], exit: Exit) -> Stop {
        Stop {
            stderr: lines.iter().map(|l| l.to_string()).collect(),
            stdout: Vec::new(),
            exit,
        }
    }

    fn line(line: String, exit: Exit) -> Stop {
        Stop {
            stderr: vec![line],
            stdout: Vec::new(),
            exit,
        }
    }

    /// A value flag as the last word: the shell's `_need_value` line.
    fn shift(flag: &str) -> Stop {
        Stop::line(crate::out::need_value(flag), Exit::Usage)
    }

    fn unknown_flag(w: &str, exit: Exit) -> Stop {
        Stop::line(format!("Unknown flag: {w}"), exit)
    }

    fn print(&self) -> Exit {
        for l in &self.stdout {
            if let Err(e) = out::stdout_line(l) {
                return out::after_stdout_error(&e);
            }
        }
        for l in &self.stderr {
            out::stderr_line(l);
        }
        self.exit
    }
}

/// The words of one verb, read as the shell's `while [[ $# -gt 0 ]]` loop
/// reads them.
struct Words<'a> {
    w: &'a [&'a str],
    i: usize,
}

impl<'a> Words<'a> {
    fn new(w: &'a [&'a str]) -> Words<'a> {
        Words { w, i: 0 }
    }

    fn take(&mut self) -> Option<&'a str> {
        let w = self.w.get(self.i).copied();
        self.i += 1;
        w
    }

    /// `x="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi`.
    fn soft(&mut self) -> String {
        let v = self.w.get(self.i).copied().unwrap_or("");
        self.i += 1;
        v.to_string()
    }

    /// `x="${2:-}"; shift 2`: the value, or `_need_value`'s line after the
    /// flag just taken.
    fn hard(&mut self) -> Result<String, Stop> {
        let flag = self.i.checked_sub(1).map_or("", |j| self.w[j]);
        let v = self
            .w
            .get(self.i)
            .copied()
            .ok_or_else(|| Stop::shift(flag))?;
        self.i += 1;
        Ok(v.to_string())
    }
}

/// `"${1:-}"` and the rest after `shift || true`.
fn first<'a>(words: &'a [&'a str]) -> (&'a str, &'a [&'a str]) {
    (
        words.first().copied().unwrap_or(""),
        words.get(1..).unwrap_or_default(),
    )
}

/// The agents as the shell hands them to Python: each with a newline
/// after it, then `splitlines()`, stripped, the empty ones left out.
fn rooms_of(agents: &[String]) -> Vec<Value> {
    let joined: String = agents.iter().map(|a| format!("{a}\n")).collect();
    py::line_list(&joined)
}

/// `[s.strip() for s in v.split(",") if s.strip()]`.
fn comma(v: &str) -> Vec<Value> {
    py::comma_list(v)
}

/// `_compute_json K2_CJ_a=… …`: the pairs with a value, in order; a key
/// ending in `_b` is `true` for `1`, else `false`.
fn compute_json(pairs: &[(&str, &str)]) -> String {
    let mut out = Vec::new();
    for (k, v) in pairs {
        if v.is_empty() {
            continue;
        }
        match k.strip_suffix("_b") {
            Some(key) => out.push((key, Value::Bool(*v == "1"))),
            None => out.push((*k, s(v))),
        }
    }
    dumps(&obj(out))
}

fn plain(req: Req, reader: Reader) -> Plain {
    Plain {
        req,
        held: false,
        room_held: false,
        json: false,
        reader,
    }
}

fn post(path: &'static str, body: String, reader: Reader) -> Ask {
    Ask::Plain(plain(Req::Post(path, body), reader))
}

fn post_held(path: &'static str, body: String, reader: Reader) -> Ask {
    Ask::Plain(Plain {
        held: true,
        ..plain(Req::Post(path, body), reader)
    })
}

/// The words after `skin`, as `cmd_skin` reads them.
pub fn parse(words: &[&str]) -> Result<Ask, Stop> {
    let (sub, rest) = first(words);
    match sub {
        "user" | "users" => user(rest),
        "role" | "roles" => role(rest),
        "grant" | "grants" => grant(rest),
        "front-door" | "frontdoor" => front_door(rest),
        "hydra" => hydra(rest),
        "--help" | "-h" => Ok(Ask::Shell),
        _ => Ok(Ask::Usage),
    }
}

const USER_ROLE_ERR: &str = concat!(
    "Error: Connect --role is not a skin command. Use ",
    cli_name!(),
    " app user role <username> <role> or ",
    cli_name!(),
    " app role create (--role on user add needs --app)."
);

fn user(words: &[&str]) -> Result<Ask, Stop> {
    let has_app = words
        .iter()
        .any(|a| matches!(*a, "--app" | "--test") || a.starts_with("--app="));
    if !has_app && words.contains(&"--role") {
        return Err(Stop::err(&[USER_ROLE_ERR], Exit::Usage));
    }
    let (action, rest) = match words.first() {
        Some(a) if !a.is_empty() => (*a, &words[1..]),
        Some(_) => ("list", &words[1..]),
        None => ("list", words),
    };
    if matches!(action, "add" | "remove" | "rm" | "password") && rest.contains(&"--test") {
        return Ok(Ask::Shell);
    }
    match action {
        "add" => user_add(rest),
        "show" => user_show(rest),
        "image" => user_image(rest),
        "sessions" => user_sessions(rest),
        "invite" => user_invite(rest),
        "reset" => user_reset(rest),
        "password" => user_password(rest),
        "chat" => Ok(Ask::Shell),
        "unlock" => user_unlock(rest),
        "email" => user_email(rest),
        "full-name" | "fullname" => user_full_name(rest),
        "agent" => user_agent(rest),
        "list" => Ok(Ask::UserList),
        "remove" | "rm" => user_remove(rest),
        "rooms" => user_rooms(rest),
        "role" => user_role(rest),
        "unassign" => user_unassign(rest),
        _ => Err(Stop::err(
            &[concat!(
                "Usage: ",
                cli_name!(),
                " app user <add|list|remove|rooms|password|email|full-name|agent|role|unassign|unlock>"
            )],
            Exit::Error,
        )),
    }
}

/// The user loops' shared arms: `--help|-h` (its line, exit 0), `-*` (an
/// unknown flag, exit 1) and a positional (the first one kept).
fn positional(w: &str, username: &mut String, help: &str) -> Result<(), Stop> {
    match w {
        "--help" | "-h" => Err(Stop::err(&[help], Exit::Ok)),
        f if f.starts_with('-') => Err(Stop::unknown_flag(f, Exit::Error)),
        _ => {
            if username.is_empty() {
                *username = w.to_string();
            }
            Ok(())
        }
    }
}

fn user_add(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user add <username> [--email e] [--password-stdin | --temp-password] [--app <app> [--role R]]"
    );
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user add <username> [--email e] [--password-stdin | --temp-password]"
    );
    let (mut username, mut email, mut app, mut role) =
        (String::new(), String::new(), String::new(), String::new());
    let (mut pw_stdin, mut temp) = (false, false);
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--password-stdin" => pw_stdin = true,
            "--temp-password" => temp = true,
            "--email" => email = w.soft(),
            "--app" => app = w.soft(),
            "--role" => role = w.soft(),
            _ if a.starts_with("--email=") => email = a["--email=".len()..].to_string(),
            _ if a.starts_with("--app=") => app = a["--app=".len()..].to_string(),
            _ if a.starts_with("--role=") => role = a["--role=".len()..].to_string(),
            _ => positional(a, &mut username, HELP)?,
        }
    }
    if username.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    if pw_stdin && temp {
        return Err(Stop::err(
            &["Use --password-stdin or --temp-password, not both."],
            Exit::Usage,
        ));
    }
    let mut b = vec![("username", s(&username))];
    if temp {
        b.push(("generate", Value::Bool(true)));
    }
    if !email.is_empty() {
        b.push(("email", s(&email)));
    }
    let app_s = py::strip(&app);
    if !app_s.is_empty() {
        b.push(("app", s(app_s)));
    }
    let role_s = py::strip(&role);
    if !role_s.is_empty() {
        b.push(("role", s(role_s)));
    }
    let body = dumps(&obj(b));
    let req = if pw_stdin {
        Req::PostPassword("/cli/skin/users", body, 1)
    } else {
        Req::Post("/cli/skin/users", body)
    };
    Ok(Ask::Plain(Plain {
        held: true,
        ..plain(req, Reader::UserAdd { app })
    }))
}

fn user_show(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user show <username> --app <app> [--json]   (owner only)"
    );
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user show <username> --app <app> [--json]"
    );
    let (mut username, mut app, mut json) = (String::new(), String::new(), false);
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--app" => app = w.soft(),
            "--json" => json = true,
            _ if a.starts_with("--app=") => app = a["--app=".len()..].to_string(),
            _ => positional(a, &mut username, HELP)?,
        }
    }
    if username.is_empty() || app.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    Ok(Ask::Plain(Plain {
        json,
        ..plain(
            Req::Get(
                "/cli/skin/access/profile",
                vec![
                    format!("app={}", encode::quote(&app)),
                    format!("user={}", encode::quote(&username)),
                ],
            ),
            Reader::UserShow,
        )
    }))
}

fn user_image(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user image <username> --clear   (owner only)"
    );
    const USAGE: &str = concat!("Usage: ", cli_name!(), " app user image <username> --clear");
    let (mut username, mut clear) = (String::new(), false);
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--clear" => clear = true,
            _ => positional(a, &mut username, HELP)?,
        }
    }
    if username.is_empty() || !clear {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    Ok(post(
        "/cli/skin/access/profile/image/clear",
        compute_json(&[("user", &username)]),
        Reader::UserImage,
    ))
}

fn user_sessions(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user sessions <username> --app <app> [--revoke <id> | --revoke-all] [--json]   (the owner, or the App's own agent with \"Let this agent manage its Apps' users\")"
    );
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user sessions <username> --app <app> [--revoke <id> | --revoke-all]"
    );
    let (mut username, mut app, mut revoke) = (String::new(), String::new(), String::new());
    let (mut all, mut json) = (false, false);
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--app" => app = w.soft(),
            "--revoke" => revoke = w.soft(),
            "--revoke-all" => all = true,
            "--json" => json = true,
            _ if a.starts_with("--app=") => app = a["--app=".len()..].to_string(),
            _ => positional(a, &mut username, HELP)?,
        }
    }
    if username.is_empty() || app.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    if !revoke.is_empty() || all {
        let all = if all { "1" } else { "0" };
        let body = compute_json(&[
            ("user", &username),
            ("app", &app),
            ("id", &revoke),
            ("all_b", all),
        ]);
        return Ok(Ask::Plain(Plain {
            json,
            ..plain(
                Req::Post("/cli/skin/users/sessions/revoke", body),
                Reader::UserEnded,
            )
        }));
    }
    Ok(Ask::Plain(plain(
        Req::Get(
            "/cli/skin/users/sessions",
            vec![
                format!("app={}", encode::quote(&app)),
                format!("user={}", encode::quote(&username)),
            ],
        ),
        if json {
            Reader::UserSessionsJson
        } else {
            Reader::UserSessions
        },
    )))
}

fn user_invite(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user invite <username> --app <app> [--email e] [--role R] [--from <address>] [--json]"
    );
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user invite <username> --app <app> [--email e] [--role R] [--from <address>]"
    );
    let (mut username, mut app, mut email, mut role, mut from): (
        String,
        String,
        String,
        String,
        String,
    ) = Default::default();
    let mut json = false;
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--app" => app = w.soft(),
            "--email" => email = w.soft(),
            "--role" => role = w.soft(),
            "--from" => from = w.soft(),
            "--json" => json = true,
            _ if a.starts_with("--app=") => app = a["--app=".len()..].to_string(),
            _ if a.starts_with("--email=") => email = a["--email=".len()..].to_string(),
            _ if a.starts_with("--role=") => role = a["--role=".len()..].to_string(),
            _ if a.starts_with("--from=") => from = a["--from=".len()..].to_string(),
            _ => positional(a, &mut username, HELP)?,
        }
    }
    if username.is_empty() || app.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    let body = compute_json(&[
        ("username", &username),
        ("app", &app),
        ("email", &email),
        ("role", &role),
        ("from", &from),
    ]);
    Ok(Ask::Plain(Plain {
        json,
        ..plain(
            Req::Post("/cli/skin/users/invite", body),
            Reader::UserInvite,
        )
    }))
}

fn user_reset(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user reset <username> [--app <app>] [--revoke] [--to <email> | --code] [--from <address>] [--json]"
    );
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user reset <username> [--app <app>] [--revoke] [--to <email> | --code] [--from <address>]"
    );
    let (mut username, mut app, mut to, mut from): (String, String, String, String) =
        Default::default();
    let (mut code, mut json, mut revoke) = (false, false, false);
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--app" => app = w.soft(),
            "--revoke" => revoke = true,
            "--to" => to = w.soft(),
            "--code" => code = true,
            "--from" => from = w.soft(),
            "--json" => json = true,
            _ if a.starts_with("--app=") => app = a["--app=".len()..].to_string(),
            _ if a.starts_with("--to=") => to = a["--to=".len()..].to_string(),
            _ if a.starts_with("--from=") => from = a["--from=".len()..].to_string(),
            _ => positional(a, &mut username, HELP)?,
        }
    }
    if username.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    let mut body = compute_json(&[
        ("username", &username),
        ("app", &app),
        ("to", &to),
        ("code_b", if code { "1" } else { "0" }),
        ("from", &from),
    ]);
    if revoke {
        // `d["revoke"]=True; print(json.dumps(d))` on the body.
        let mut v = crate::out::json::parse(&body).expect("the body is JSON");
        if let Value::Object(pairs) = &mut v {
            pairs.push(("revoke".into(), Value::Bool(true)));
        }
        body = dumps(&v);
    }
    Ok(Ask::Plain(Plain {
        held: true,
        json,
        ..plain(
            Req::Post("/cli/skin/users/reset-link", body),
            Reader::UserReset,
        )
    }))
}

fn user_password(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user password <username> --password-stdin"
    );
    let (mut username, mut pw_stdin) = (String::new(), false);
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--password-stdin" => pw_stdin = true,
            _ => positional(a, &mut username, HELP)?,
        }
    }
    if username.is_empty() || !pw_stdin {
        return Err(Stop::err(&[HELP], Exit::Error));
    }
    let body = dumps(&obj(vec![("username", s(&username))]));
    Ok(Ask::Plain(Plain {
        held: true,
        ..plain(
            Req::PostPassword("/cli/skin/users/password", body, 1),
            Reader::UserPassword,
        )
    }))
}

fn user_unlock(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user unlock <username> [--json]"
    );
    let (mut username, mut json) = (String::new(), false);
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--json" => json = true,
            _ => positional(a, &mut username, HELP)?,
        }
    }
    if username.is_empty() {
        return Err(Stop::err(&[HELP], Exit::Error));
    }
    Ok(Ask::Plain(Plain {
        json,
        ..plain(
            Req::Post(
                "/cli/skin/users/unlock",
                dumps(&obj(vec![("username", s(&username))])),
            ),
            Reader::UserUnlock,
        )
    }))
}

fn user_email(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user email <username> [--email e]"
    );
    let (mut username, mut email) = (String::new(), String::new());
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--email" => email = w.soft(),
            _ if a.starts_with("--email=") => email = a["--email=".len()..].to_string(),
            _ => positional(a, &mut username, HELP)?,
        }
    }
    if username.is_empty() {
        return Err(Stop::err(&[HELP], Exit::Error));
    }
    Ok(post(
        "/cli/skin/users/email",
        dumps(&obj(vec![("username", s(&username)), ("email", s(&email))])),
        Reader::UserEmail,
    ))
}

fn user_full_name(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user full-name <username> [--name <text>]"
    );
    let (mut username, mut name) = (String::new(), String::new());
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--name" => name = w.soft(),
            _ if a.starts_with("--name=") => name = a["--name=".len()..].to_string(),
            _ => positional(a, &mut username, HELP)?,
        }
    }
    if username.is_empty() {
        return Err(Stop::err(&[HELP], Exit::Error));
    }
    Ok(post(
        "/cli/skin/users/full-name",
        dumps(&obj(vec![
            ("username", s(&username)),
            ("fullName", s(&name)),
        ])),
        Reader::UserFullName,
    ))
}

fn user_agent(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user agent <username> <workspace> | ",
        cli_name!(),
        " app user agent <username> --clear"
    );
    let (mut username, mut workspace, mut clear) = (String::new(), String::new(), false);
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--clear" | "--no-agent" => clear = true,
            "--workspace" => workspace = w.soft(),
            _ if a.starts_with("--workspace=") => workspace = a["--workspace=".len()..].to_string(),
            "--help" | "-h" => return Err(Stop::err(&[HELP], Exit::Ok)),
            f if f.starts_with('-') => return Err(Stop::unknown_flag(f, Exit::Error)),
            _ => {
                if username.is_empty() {
                    username = a.to_string();
                } else if workspace.is_empty() {
                    workspace = a.to_string();
                }
            }
        }
    }
    if username.is_empty() || (clear && !workspace.is_empty()) || (!clear && workspace.is_empty()) {
        return Err(Stop::err(&[HELP], Exit::Error));
    }
    let mut b = vec![("username", s(&username))];
    if clear {
        b.push(("clear", Value::Bool(true)));
    } else {
        b.push(("workspace", s(&workspace)));
    }
    Ok(post(
        "/cli/skin/users/agent",
        dumps(&obj(b)),
        Reader::UserAgent,
    ))
}

fn user_remove(rest: &[&str]) -> Result<Ask, Stop> {
    let username = rest.first().copied().unwrap_or("");
    if username.is_empty() {
        return Err(Stop::err(
            &[concat!(
                "Usage: ",
                cli_name!(),
                " app user remove <username>"
            )],
            Exit::Error,
        ));
    }
    Ok(post_held(
        "/cli/skin/users/remove",
        dumps(&obj(vec![("username", s(username))])),
        Reader::UserRemove,
    ))
}

fn user_rooms(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user rooms <username> --agent sales [--app <app>]"
    );
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app user rooms <username> --agent sales"
    );
    const GONE: &str = "--apply-tokens is gone: signed-in people get a rooms change at their next request, so there is nothing to apply. Run it without the flag.";
    let (mut username, mut app, mut agents) = (String::new(), String::new(), Vec::new());
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--agent" | "--agents" => agents.push(w.hard()?),
            "--apply-tokens" | "--applyTokens" => return Err(Stop::err(&[GONE], Exit::Usage)),
            "--app" => app = w.soft(),
            _ if a.starts_with("--app=") => app = a["--app=".len()..].to_string(),
            _ => positional(a, &mut username, HELP)?,
        }
    }
    if username.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    let mut b = vec![
        ("username", s(&username)),
        ("rooms", Value::Array(rooms_of(&agents))),
    ];
    let app_s = py::strip(&app);
    if !app_s.is_empty() {
        b.push(("app", s(app_s)));
    }
    Ok(post(
        "/cli/skin/users/rooms",
        dumps(&obj(b)),
        Reader::UserRooms,
    ))
}

fn user_role(rest: &[&str]) -> Result<Ask, Stop> {
    let username = rest.first().copied().unwrap_or("");
    let role = rest.get(1).copied().unwrap_or("");
    if username.is_empty() || role.is_empty() {
        return Err(Stop::err(
            &[concat!(
                "Usage: ",
                cli_name!(),
                " app user role <username> <role>"
            )],
            Exit::Error,
        ));
    }
    Ok(post_held(
        "/cli/skin/roles/assign",
        dumps(&obj(vec![("username", s(username)), ("role", s(role))])),
        Reader::UserRole,
    ))
}

fn user_unassign(rest: &[&str]) -> Result<Ask, Stop> {
    let username = rest.first().copied().unwrap_or("");
    if username.is_empty() {
        return Err(Stop::err(
            &[concat!(
                "Usage: ",
                cli_name!(),
                " app user unassign <username>"
            )],
            Exit::Error,
        ));
    }
    Ok(post_held(
        "/cli/skin/roles/unassign",
        dumps(&obj(vec![("username", s(username))])),
        Reader::UserUnassign,
    ))
}

const ROLE_CAPS: &str = concat!(
    "Error: use ",
    cli_name!(),
    " app role room <role> <handle> [--caps \u{2026}], not --caps with --agent"
);

fn role(words: &[&str]) -> Result<Ask, Stop> {
    if words.iter().any(|a| matches!(*a, "--user" | "--username")) {
        return Err(Stop::err(
            &[concat!(
                "Error: use ",
                cli_name!(),
                " app user role <username> <role>, not ",
                cli_name!(),
                " app role --user"
            )],
            Exit::Usage,
        ));
    }
    let (action, rest) = match words.first() {
        Some(a) if !a.is_empty() => (*a, &words[1..]),
        Some(_) => ("list", &words[1..]),
        None => ("list", words),
    };
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app role <create|list|room|paths|update|remove|suggest|add --from>"
    );
    match action {
        "create" => role_create(rest),
        "list" => Ok(Ask::Plain(plain(
            Req::Get("/cli/skin/roles", vec![]),
            Reader::RoleList,
        ))),
        "room" => role_room(rest),
        "paths" | "folders" => role_paths(rest),
        "update" => role_update(rest),
        "remove" | "rm" => {
            let name = rest.first().copied().unwrap_or("");
            if name.is_empty() {
                return Err(Stop::err(
                    &[concat!("Usage: ", cli_name!(), " app role remove <name>")],
                    Exit::Error,
                ));
            }
            Ok(post(
                "/cli/skin/roles/remove",
                dumps(&obj(vec![("name", s(name))])),
                Reader::RoleRemove,
            ))
        }
        "suggest" | "suggestions" | "templates" => Ok(Ask::RoleSuggest {
            json: rest.first() == Some(&"--json"),
        }),
        "add" => role_add(rest),
        "--help" | "-h" => Err(Stop::err(&[USAGE], Exit::Ok)),
        _ => Err(Stop::err(&[USAGE], Exit::Error)),
    }
}

fn role_create(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app role create <name> [--agent sales] [--app <app>]"
    );
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app role create <name> [--agent sales]"
    );
    let (mut name, mut caps, mut app, mut agents) =
        (String::new(), String::new(), String::new(), Vec::new());
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--caps" | "--cap" => caps = w.hard()?,
            "--agent" | "--agents" => agents.push(w.hard()?),
            "--app" => app = w.soft(),
            _ if a.starts_with("--app=") => app = a["--app=".len()..].to_string(),
            _ => positional(a, &mut name, HELP)?,
        }
    }
    if name.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    if !caps.is_empty() {
        return Err(Stop::err(&[ROLE_CAPS], Exit::Usage));
    }
    let mut b = vec![("name", s(&name))];
    let rooms = rooms_of(&agents);
    if !rooms.is_empty() {
        b.push(("rooms", Value::Array(rooms)));
    }
    let app_s = py::strip(&app);
    if !app_s.is_empty() {
        b.push(("app", s(app_s)));
    }
    Ok(post("/cli/skin/roles", dumps(&obj(b)), Reader::RoleCreate))
}

fn role_room(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app role room <role> <handle> [--caps thread:read,thread:post,files:read] | --clear"
    );
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app role room <role> <handle> [--caps \u{2026}] | --clear"
    );
    let (mut role, mut handle, mut caps, mut clear) =
        (String::new(), String::new(), String::new(), false);
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--caps" | "--cap" => caps = w.hard()?,
            "--clear" => clear = true,
            "--help" | "-h" => return Err(Stop::err(&[HELP], Exit::Ok)),
            f if f.starts_with('-') => return Err(Stop::unknown_flag(f, Exit::Error)),
            _ => {
                if role.is_empty() {
                    role = a.to_string();
                } else if handle.is_empty() {
                    handle = a.to_string();
                }
            }
        }
    }
    if role.is_empty() || handle.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    let mut b = vec![("name", s(&role)), ("handle", s(&handle))];
    if clear {
        b.push(("clear", Value::Bool(true)));
    } else {
        let c = py::strip(&caps);
        if !c.is_empty() {
            b.push(("caps", Value::Array(comma(c))));
        }
    }
    Ok(Ask::Plain(Plain {
        room_held: true,
        ..plain(
            Req::Post("/cli/skin/roles/room", dumps(&obj(b))),
            Reader::RoleRoom,
        )
    }))
}

fn role_paths(rest: &[&str]) -> Result<Ask, Stop> {
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app role paths <role> <room> [--read a/,b/] [--write c/] [--upload d/] [--upload-dir c/in] [--download e/,f.pdf | --no-download] [--hide .k2,.env] [--unhide .env | --no-hide] | --clear"
    );
    let (mut role, mut handle) = (String::new(), String::new());
    let (mut read, mut write, mut upload, mut drop, mut download): (
        String,
        String,
        String,
        String,
        String,
    ) = Default::default();
    let (mut hide, mut unhide) = (String::new(), String::new());
    let (mut nodownload, mut clear, mut nohide) = (false, false, false);
    let mut w = Words::new(rest);
    // `x="${x:+$x,}${2:-}"`: a comma before the next value once there is
    // one.
    let add = |to: &mut String, v: String| {
        if !to.is_empty() {
            to.push(',');
        }
        to.push_str(&v);
    };
    while let Some(a) = w.take() {
        match a {
            "--hide" => add(&mut hide, w.hard()?),
            "--unhide" => add(&mut unhide, w.hard()?),
            "--no-hide" => nohide = true,
            "--read" => read = w.hard()?,
            "--write" => write = w.hard()?,
            "--upload" => drop = w.hard()?,
            "--upload-dir" => upload = w.hard()?,
            "--download" => download = w.hard()?,
            "--no-download" => nodownload = true,
            "--clear" => clear = true,
            "--help" | "-h" => return Err(Stop::err(&[USAGE], Exit::Ok)),
            f if f.starts_with('-') => return Err(Stop::unknown_flag(f, Exit::Error)),
            _ => {
                if role.is_empty() {
                    role = a.to_string();
                } else if handle.is_empty() {
                    handle = a.to_string();
                } else {
                    return Err(Stop::line(format!("Unexpected argument: {a}"), Exit::Error));
                }
            }
        }
    }
    if role.is_empty() || handle.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    let folders = !read.is_empty()
        || !write.is_empty()
        || !upload.is_empty()
        || !drop.is_empty()
        || !download.is_empty();
    if clear && (folders || nodownload) {
        return Err(Stop::err(
            &["Error: --clear puts the room back to the whole room; don't pass folders with it"],
            Exit::Usage,
        ));
    }
    if !download.is_empty() && nodownload {
        return Err(Stop::err(
            &["Error: --download sets the download entries, --no-download clears them; pass one"],
            Exit::Usage,
        ));
    }
    if nohide && (!hide.is_empty() || !unhide.is_empty()) {
        return Err(Stop::err(
            &["Error: --no-hide empties the hidden list; don't pass --hide or --unhide with it"],
            Exit::Usage,
        ));
    }
    if !clear && !folders && !nodownload && hide.is_empty() && unhide.is_empty() && !nohide {
        return Err(Stop::err(
            &["Error: name folders (--read, --write, --upload, --upload-dir, --download), hidden paths (--hide, --unhide, --no-hide) or pass --clear for the whole room"],
            Exit::Usage,
        ));
    }
    let mut b = vec![("name", s(&role)), ("handle", s(&handle))];
    if nohide {
        b.push(("hide", Value::Array(vec![])));
    } else {
        if !comma(&hide).is_empty() {
            b.push(("hideAdd", Value::Array(comma(&hide))));
        }
        if !comma(&unhide).is_empty() {
            b.push(("hideRemove", Value::Array(comma(&unhide))));
        }
    }
    if clear {
        b.push(("clear", Value::Bool(true)));
    } else {
        for (key, v) in [("read", &read), ("write", &write), ("upload", &drop)] {
            let v = comma(v);
            if !v.is_empty() {
                b.push((key, Value::Array(v)));
            }
        }
        let u = py::strip(&upload);
        if !u.is_empty() {
            b.push(("uploadDir", s(u)));
        }
        let d = comma(&download);
        if !d.is_empty() {
            b.push(("download", Value::Array(d)));
        } else if nodownload {
            b.push(("download", Value::Array(vec![])));
        }
    }
    Ok(post(
        "/cli/skin/roles/paths",
        dumps(&obj(b)),
        Reader::RolePaths { handle },
    ))
}

fn role_update(rest: &[&str]) -> Result<Ask, Stop> {
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app role update <name> [--agent \u{2026} | --clear]"
    );
    let (mut name, mut caps, mut clear, mut agents) =
        (String::new(), String::new(), false, Vec::new());
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--caps" | "--cap" => caps = w.hard()?,
            "--agent" | "--agents" => agents.push(w.hard()?),
            "--clear" => clear = true,
            _ => positional(a, &mut name, USAGE)?,
        }
    }
    if name.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    if !caps.is_empty() {
        return Err(Stop::err(&[ROLE_CAPS], Exit::Usage));
    }
    let mut b = vec![("name", s(&name))];
    let joined: String = agents.iter().map(|a| format!("{a}\n")).collect();
    if clear {
        b.push(("roomAccess", Value::Array(vec![])));
    } else if !py::strip(&joined).is_empty() {
        b.push(("rooms", Value::Array(rooms_of(&agents))));
    }
    Ok(post(
        "/cli/skin/roles/update",
        dumps(&obj(b)),
        Reader::RoleUpdate,
    ))
}

fn role_add(rest: &[&str]) -> Result<Ask, Stop> {
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app roles add --from <template> --app <app> [--name <name>] [--agent <room> \u{2026}]"
    );
    let (mut from, mut app, mut name, mut agents) =
        (String::new(), String::new(), String::new(), Vec::new());
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--from" | "--template" => from = w.hard()?,
            "--app" => app = w.hard()?,
            "--name" => name = w.hard()?,
            "--agent" | "--agents" | "--room" => agents.push(w.hard()?),
            "--help" | "-h" => {
                return Err(Stop {
                    stderr: Vec::new(),
                    stdout: vec![format!(
                        "{USAGE}   ({} app roles suggest lists the templates)",
                        cli_name!()
                    )],
                    exit: Exit::Ok,
                })
            }
            _ => {
                return Err(Stop::line(
                    format!(
                        "Unknown argument: {a} ({} app roles add --from <template> --app <app>)",
                        cli_name!()
                    ),
                    Exit::Usage,
                ))
            }
        }
    }
    if from.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Usage));
    }
    let mut b = vec![("template", s(&from))];
    for (k, v) in [("app", &app), ("name", &name)] {
        let v = py::strip(v);
        if !v.is_empty() {
            b.push((k, s(v)));
        }
    }
    let rooms = rooms_of(&agents);
    if !rooms.is_empty() {
        b.push(("rooms", Value::Array(rooms)));
    }
    Ok(post(
        "/cli/skin/roles/from-template",
        dumps(&obj(b)),
        Reader::RoleAdd,
    ))
}

const GRANT_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " app grant <list|add|set|rm|copy-host|hooks>"
);

fn grant(words: &[&str]) -> Result<Ask, Stop> {
    let (action, rest) = match words.first() {
        Some(a) => (*a, &words[1..]),
        None => ("list", words),
    };
    match action {
        "list" | "" => Ok(Ask::GrantList),
        "add" | "create" => grant_add(rest),
        "rm" | "remove" | "delete" | "revoke" => grant_rm(rest),
        "set" | "update" => grant_set(rest),
        "role" => grant_role(rest),
        "hooks" | "hook" => match hooks::parse(rest) {
            Some(r) => r.map(Ask::Hooks),
            None => Ok(Ask::Shell),
        },
        "copy-host" | "copyhost" => Ok(Ask::Shell),
        "--help" | "-h" => Err(Stop::err(&[GRANT_USAGE], Exit::Ok)),
        _ => Err(Stop::err(&[GRANT_USAGE], Exit::Error)),
    }
}

/// `"$(printf %s "$kind" | tr "[:upper:]" "[:lower:]")" == "chat"`.
fn is_chat(kind: &str) -> bool {
    kind.eq_ignore_ascii_case("chat")
}

fn grant_add(rest: &[&str]) -> Result<Ask, Stop> {
    const HELP: [&str; 3] = [
        concat!(
            "Usage: ",
            cli_name!(),
            " app grant add --subject-kind principal|workspace --subject <id> --kind app|mailbox|database|cli|chat --target <id> [--role <role> | --room <handle>\u{2026}] [--scope <text>]"
        ),
        concat!(
            "       ",
            cli_name!(),
            " app grant add --kind chat --subject-kind principal|workspace|connect --subject <name|handle|login|id> [--scope member|guest|coworker|assistant:<person>]"
        ),
        "       chat grants create the chat account; roles and channels are set in Sew.",
    ];
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app grant add --subject-kind principal|workspace --subject <id> --kind app|mailbox|database|cli|chat --target <id> [--role <role>] [--scope <text>]"
    );
    let (mut subject_kind, mut subject, mut kind, mut target): (String, String, String, String) =
        Default::default();
    let (mut role, mut scope, mut rooms_nl) = (String::new(), String::new(), String::new());
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (a, None),
        };
        let slot: Option<&mut String> = match name {
            "--subject-kind" => Some(&mut subject_kind),
            "--subject" => Some(&mut subject),
            "--kind" => Some(&mut kind),
            "--target" => Some(&mut target),
            "--role" => Some(&mut role),
            "--scope" => Some(&mut scope),
            "--room" => None,
            _ => {
                if inline.is_none() {
                    match a {
                        "--help" | "-h" => return Err(Stop::err(&HELP, Exit::Ok)),
                        f if f.starts_with('-') => return Err(Stop::unknown_flag(f, Exit::Error)),
                        _ => return Err(Stop::line(format!("Unknown argument: {a}"), Exit::Error)),
                    }
                }
                return Err(Stop::unknown_flag(a, Exit::Error));
            }
        };
        let v = inline.unwrap_or_else(|| w.soft());
        match slot {
            Some(slot) => *slot = v,
            None => {
                rooms_nl.push_str(&v);
                rooms_nl.push('\n');
            }
        }
    }
    if is_chat(&kind) && target.is_empty() {
        target = "sew".into();
    }
    if subject_kind.is_empty() || subject.is_empty() || kind.is_empty() || target.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    let mut b = vec![
        ("subjectKind", s(&subject_kind)),
        ("subjectId", s(&subject)),
        ("kind", s(&kind)),
        ("targetId", s(&target)),
    ];
    let (role_s, scope_s) = (py::strip(&role), py::strip(&scope));
    let rooms = py::line_list(&rooms_nl);
    if !role_s.is_empty() {
        b.push(("roleId", s(role_s)));
    }
    if !scope_s.is_empty() {
        b.push(("scope", s(scope_s)));
    }
    if !rooms.is_empty() {
        b.push(("rooms", Value::Array(rooms)));
    }
    Ok(Ask::OwnerOnly(plain(
        Req::Post("/cli/skin/grants", dumps(&obj(b))),
        Reader::GrantAdd,
    )))
}

fn grant_rm(rest: &[&str]) -> Result<Ask, Stop> {
    const USAGE: [&str; 2] = [
        concat!("Usage: ", cli_name!(), " app grant rm --id <id>"),
        concat!(
            "       ",
            cli_name!(),
            " app grant rm --subject-kind principal|workspace --subject <id> --kind <kind> --target <id>"
        ),
    ];
    let (mut gid, mut subject_kind, mut subject, mut kind, mut target): (
        String,
        String,
        String,
        String,
        String,
    ) = Default::default();
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (a, None),
        };
        let slot: &mut String = match name {
            "--id" => &mut gid,
            "--subject-kind" => &mut subject_kind,
            "--subject" => &mut subject,
            "--kind" => &mut kind,
            "--target" => &mut target,
            _ => {
                return Err(match a {
                    "--help" | "-h" => Stop::err(&USAGE, Exit::Ok),
                    f if f.starts_with('-') => Stop::unknown_flag(f, Exit::Error),
                    _ => Stop::line(format!("Unknown argument: {a}"), Exit::Error),
                })
            }
        };
        *slot = inline.unwrap_or_else(|| w.soft());
    }
    if gid.is_empty() && is_chat(&kind) && target.is_empty() {
        target = "sew".into();
    }
    if gid.is_empty()
        && (subject_kind.is_empty() || subject.is_empty() || kind.is_empty() || target.is_empty())
    {
        return Err(Stop::err(&USAGE, Exit::Error));
    }
    let g = py::strip(&gid);
    let body = if !g.is_empty() {
        dumps(&obj(vec![("id", s(g))]))
    } else {
        dumps(&obj(vec![
            ("subjectKind", s(&subject_kind)),
            ("subjectId", s(&subject)),
            ("kind", s(&kind)),
            ("targetId", s(&target)),
        ]))
    };
    Ok(Ask::OwnerOnly(plain(
        Req::Post("/cli/skin/grants/delete", body),
        Reader::GrantRm,
    )))
}

fn grant_set(rest: &[&str]) -> Result<Ask, Stop> {
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app grant set --id <grant-id> --scope <scope>   (chat grants only)"
    );
    let (mut gid, mut scope) = (String::new(), String::new());
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--id" => gid = w.soft(),
            "--scope" => scope = w.soft(),
            _ if a.starts_with("--id=") => gid = a["--id=".len()..].to_string(),
            _ if a.starts_with("--scope=") => scope = a["--scope=".len()..].to_string(),
            "--help" | "-h" => return Err(Stop::err(&[USAGE], Exit::Ok)),
            f if f.starts_with('-') => return Err(Stop::unknown_flag(f, Exit::Error)),
            _ => return Err(Stop::line(format!("Unknown argument: {a}"), Exit::Error)),
        }
    }
    if gid.is_empty() || scope.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    Ok(post(
        "/cli/skin/grants/update",
        dumps(&obj(vec![("id", s(&gid)), ("scope", s(&scope))])),
        Reader::GrantSet,
    ))
}

fn grant_role(rest: &[&str]) -> Result<Ask, Stop> {
    const USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app grant role <grant-id> --role <role>   (an App login, kind=app; the owner, or your own App's agent with its Apps' users switch)"
    );
    const MORE: &str = "  Re-roles the login in place: live logins stay signed in and get the new role's rooms now.";
    let (mut gid, mut role) = (String::new(), String::new());
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--id" => gid = w.soft(),
            "--role" => role = w.soft(),
            _ if a.starts_with("--id=") => gid = a["--id=".len()..].to_string(),
            _ if a.starts_with("--role=") => role = a["--role=".len()..].to_string(),
            "--help" | "-h" => return Err(Stop::err(&[USAGE, MORE], Exit::Ok)),
            f if f.starts_with('-') => return Err(Stop::unknown_flag(f, Exit::Error)),
            _ => {
                if gid.is_empty() {
                    gid = a.to_string();
                } else {
                    return Err(Stop::line(format!("Unknown argument: {a}"), Exit::Error));
                }
            }
        }
    }
    if gid.is_empty() || role.is_empty() {
        return Err(Stop::err(&[USAGE], Exit::Error));
    }
    Ok(post(
        "/cli/skin/grants/role",
        dumps(&obj(vec![("id", s(&gid)), ("role", s(&role))])),
        Reader::GrantRole { role },
    ))
}

/// Python's `int(p)` for the plain forms: ASCII digits with an optional
/// sign and single `_` between digits. `None`: the shell decides.
fn py_int(p: &str) -> Option<Option<String>> {
    if !p.is_ascii() {
        return None;
    }
    let (neg, digits) = match p.as_bytes().first() {
        Some(b'-') => (true, &p[1..]),
        Some(b'+') => (false, &p[1..]),
        _ => (false, p),
    };
    let ok = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
        && !digits.starts_with('_')
        && !digits.ends_with('_')
        && !digits.contains("__");
    if !ok {
        // ValueError: the string stays a string.
        return Some(None);
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    let trimmed = clean.trim_start_matches('0');
    let n = if trimmed.is_empty() { "0" } else { trimmed };
    Some(Some(if neg && n != "0" {
        format!("-{n}")
    } else {
        n.to_string()
    }))
}

fn front_door(rest: &[&str]) -> Result<Ask, Stop> {
    const MODE_USAGE: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app front-door [connect|direct] [--json]"
    );
    const HELP: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app front-door [connect|direct] [--url URL] [--hint TEXT] [--ui-port N] [--json]"
    );
    let (mut json, mut mode, mut url, mut hint, mut ui_port) = (
        false,
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let mut w = Words::new(rest);
    while let Some(a) = w.take() {
        match a {
            "--json" => json = true,
            "--url" => url = w.hard()?,
            "--hint" => hint = w.hard()?,
            "--ui-port" | "--uiPort" => ui_port = w.hard()?,
            "connect" | "direct" => {
                if !mode.is_empty() {
                    return Err(Stop::err(&[MODE_USAGE], Exit::Error));
                }
                mode = a.to_string();
            }
            "--help" | "-h" => return Err(Stop::err(&[HELP], Exit::Ok)),
            _ => return Err(Stop::unknown_flag(a, Exit::Error)),
        }
    }
    if mode.is_empty() {
        return Ok(Ask::Plain(Plain {
            json,
            ..plain(Req::Get("/cli/skin/front-door", vec![]), Reader::FrontDoor)
        }));
    }
    let mut b = vec![("mode", s(&mode)), ("apply", Value::Bool(true))];
    let u = py::strip(&url);
    if !u.is_empty() {
        b.push(("url", s(u)));
    }
    let h = py::strip(&hint);
    if !h.is_empty() {
        b.push(("hint", s(h)));
    }
    let p = py::strip(&ui_port);
    if !p.is_empty() {
        match py_int(p) {
            None => return Ok(Ask::Shell),
            Some(Some(n)) => b.push(("uiPort", Value::Int(n))),
            Some(None) => b.push(("uiPort", s(p))),
        }
    }
    Ok(Ask::FrontDoorSet {
        body: dumps(&obj(b)),
        json,
    })
}

fn hydra(rest: &[&str]) -> Result<Ask, Stop> {
    const USAGE: &str = concat!("Usage: ", cli_name!(), " app hydra [on|off] [--json]");
    let (mut json, mut action) = (false, "");
    for a in rest {
        match *a {
            "--json" => json = true,
            "--help" | "-h" => return Err(Stop::err(&[USAGE], Exit::Ok)),
            "on" | "off" => {
                if !action.is_empty() {
                    return Err(Stop::err(&[USAGE], Exit::Usage));
                }
                action = a;
            }
            _ => return Err(Stop::err(&[USAGE], Exit::Usage)),
        }
    }
    let req = match action {
        "" => Req::Get("/cli/skin/hydra", vec![]),
        "on" => Req::Post("/cli/skin/hydra", r#"{"enabled":true,"apply":true}"#.into()),
        _ => Req::Post(
            "/cli/skin/hydra",
            r#"{"enabled":false,"apply":true}"#.into(),
        ),
    };
    Ok(Ask::Plain(Plain {
        json,
        ..plain(req, Reader::Hydra)
    }))
}

/// `k5 skin ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    run_as(args, true)
}

/// The `grant` words `cmd_app` hands to `cmd_skin` (`grant|grants)` in
/// `cmd_app`); any other third word is an App's own owner grant.
const APP_GRANT_WORDS: [&str; 17] = [
    "",
    "list",
    "add",
    "create",
    "rm",
    "remove",
    "delete",
    "revoke",
    "set",
    "update",
    "role",
    "hooks",
    "hook",
    "copy-host",
    "copyhost",
    "-h",
    "--help",
];

/// `k5 app user|role|grant|front-door|hydra ...`, and `app` alone:
/// `cmd_app` hands these words to `cmd_skin` as they are, with no
/// teaching line. `None`: the shell CLI answers it.
pub fn run_app(args: &[OsString]) -> Option<Exit> {
    let verb = args.get(1).map_or(Some(""), |a| a.to_str())?;
    if matches!(verb, "grant" | "grants") {
        let third = args.get(2).map_or(Some(""), |a| a.to_str())?;
        if !APP_GRANT_WORDS.contains(&third) {
            return None;
        }
    }
    run_as(args, false)
}

fn run_as(args: &[OsString], teach: bool) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    if std::env::vars_os().any(|(n, _)| n.to_string_lossy().starts_with("K2_CJ_")) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let ask = parse(&words);
    if matches!(ask, Ok(Ask::Shell)) {
        return None;
    }
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
    if matches!(ask, Ok(Ask::OwnerOnly(_))) && prelude::gate(&settings).is_none() {
        let would_refuse = passport::current(args).would_refuse();
        let mut s = settings.clone();
        s.port = s.port_text.as_deref().and_then(curl_port);
        if s.client(would_refuse).is_ok_and(|c| c.passport().is_some()) {
            return None;
        }
    }
    // `_skin_usage` asks whether permission cards are on (with a
    // passport); the probe's settings must be ones the binary can send.
    let cards = if matches!(ask, Ok(Ask::Usage)) {
        let env = crate::offline::Env::current();
        let pre = crate::offline::shell_pwd(&env)
            .and_then(|cwd| crate::offline::prelude(&env, &cwd))
            .ok()?;
        Some(crate::cards::ask(&env, &pre, || {
            passport::current(args).would_refuse()
        })?)
    } else {
        None
    };
    Some(run_with(
        args,
        ask,
        hook_sock.as_deref(),
        settings,
        teach,
        cards,
    ))
}

fn run_with(
    args: &[OsString],
    ask: Result<Ask, Stop>,
    hook_sock: Option<&Path>,
    mut settings: Settings,
    teach: bool,
    cards: Option<crate::cards::Ask>,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    if teach {
        out::stderr_line(TEACH);
    }
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
    let exit = match ask {
        Ask::Plain(p) | Ask::OwnerOnly(p) => plain_run(&mut client, &mut closing, &p),
        Ask::GrantList => {
            let path = if client.passport().is_some() {
                "/cli/skin/grants/own"
            } else {
                "/cli/skin/grants"
            };
            let p = plain(Req::Get(path, vec![]), Reader::GrantList);
            plain_run(&mut client, &mut closing, &p)
        }
        Ask::UserList => user_list(&mut client, &mut closing),
        Ask::Hooks(h) => hooks::run(&mut client, &mut closing, &h),
        Ask::FrontDoorSet { body, json } => {
            match send(
                &mut client,
                &mut closing,
                &Req::Post("/cli/skin/front-door", body),
            ) {
                Err(exit) => exit,
                Ok(raw) if json => {
                    if let Err(exit) = raw_line(&raw) {
                        return finish(&mut client, closing, exit);
                    }
                    print::front_door_json_exit(&String::from_utf8_lossy(&raw)).print()
                }
                Ok(raw) => print::front_door_set(&String::from_utf8_lossy(&raw)).print(),
            }
        }
        Ask::Usage => {
            let on = cards.is_some_and(crate::cards::on);
            let page = crate::text::help("app").expect("the App family's page is embedded");
            let text = crate::cards::perm_lines(page, on);
            let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
            Exit::Error
        }
        Ask::RoleSuggest { json } => {
            let mut ctx = super::app::call::Ctx::new(client, false);
            let exit =
                match super::approve::cp::call(&mut ctx, false, "/cli/skin/roles/suggest", "") {
                    Err(exit) => exit,
                    Ok(body) if json => super::approve::cp::body_line(&body),
                    Ok(body) => print::role_suggest(&body).print(),
                };
            let super::app::call::Ctx {
                mut client,
                closing: cp_closing,
                ..
            } = ctx;
            cp_closing.flush(&mut client, exit);
            return finish(&mut client, closing, exit);
        }
        Ask::Shell => unreachable!("handed to the shell before the run"),
    };
    finish(&mut client, closing, exit)
}

fn finish(client: &mut Client, closing: Closing, exit: Exit) -> Exit {
    closing.flush(client, exit);
    exit
}

/// `raw=$(cli_request …)` or `raw=$(cli_post_json …)`.
fn send(client: &mut Client, closing: &mut Closing, req: &Req) -> Result<Vec<u8>, Exit> {
    let r = match req {
        Req::Get(path, pairs) => {
            let pairs: Vec<&str> = pairs.iter().map(String::as_str).collect();
            client.call(Channel::Tcp, path, Call::Request(&pairs))
        }
        Req::Post(path, body) => client.call(Channel::Tcp, path, Call::PostJson(body.as_bytes())),
        Req::PostPassword(path, body, at) => {
            let body = with_password(body, *at, &prompt::read_all_stdin());
            client.call(Channel::Tcp, path, Call::PostJson(body.expose()))
        }
    };
    skin_token::held(client, closing, r)
}

/// The body with `"password"` at `at`: what `sys.stdin.read()` gives
/// Python of the bytes `$(cat)` kept ([`confirm::text_of_bytes`]).
pub fn with_password(body: &str, at: usize, pw: &Secret) -> Secret {
    let Ok(Value::Object(mut pairs)) = crate::out::json::parse(body) else {
        unreachable!("the verb's own body is a JSON object")
    };
    let at = at.min(pairs.len());
    pairs.insert(
        at,
        (Text::from("password"), confirm::text_of_bytes(pw.expose())),
    );
    Secret::new(dumps(&Value::Object(pairs)).into_bytes())
}

/// `printf '%s\n' "$raw"`.
fn raw_line(raw: &[u8]) -> Result<(), Exit> {
    let mut b = raw.to_vec();
    b.push(b'\n');
    out::write_flushed(&mut std::io::stdout().lock(), &b).map_err(|e| out::after_stdout_error(&e))
}

fn plain_run(client: &mut Client, closing: &mut Closing, p: &Plain) -> Exit {
    let raw = match send(client, closing, &p.req) {
        Ok(raw) => raw,
        Err(exit) => return exit,
    };
    let exit = skin_token::die_if_err(&raw).print();
    if exit != Exit::Ok {
        return exit;
    }
    let text = String::from_utf8_lossy(&raw);
    if p.held {
        let exit = print::held(&text).print();
        if exit != Exit::Ok {
            return exit;
        }
    }
    if p.room_held {
        let exit = print::room_held(&text).print();
        if exit != Exit::Ok {
            return exit;
        }
    }
    if p.json {
        if let Err(exit) = raw_line(&raw) {
            return exit;
        }
        return Exit::Ok;
    }
    print::read(&p.reader, &text).print()
}

/// `user list`: both answers through `_skin_die_if_err`, then the
/// printer reads one line of each.
fn user_list(client: &mut Client, closing: &mut Closing) -> Exit {
    let raw = match send(client, closing, &Req::Get("/cli/skin/users", vec![])) {
        Ok(raw) => raw,
        Err(exit) => return exit,
    };
    let exit = skin_token::die_if_err(&raw).print();
    if exit != Exit::Ok {
        return exit;
    }
    let roles = match send(client, closing, &Req::Get("/cli/skin/roles", vec![])) {
        Ok(raw) => raw,
        Err(exit) => return exit,
    };
    let exit = skin_token::die_if_err(&roles).print();
    if exit != Exit::Ok {
        return exit;
    }
    let mut stream = raw;
    stream.push(b'\n');
    stream.extend_from_slice(&roles);
    stream.push(b'\n');
    print::user_list(&String::from_utf8_lossy(&stream)).print()
}

#[cfg(test)]
pub mod tests;
