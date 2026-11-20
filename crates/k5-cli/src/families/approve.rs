//! `k5 approve`: signed approvals, as the shell CLI's `cmd_approve`
//! (`cli/src/permission.sh`, `BEGIN_CLI_APPROVE`) and `cmd_approve_local`
//! (`cli/src/daemon-install.sh`).
//!
//! 1. The settings and the gates ([`cp::open`]). `approve` alone, `help`,
//!    `verify`, `show` and `key-id` need no daemon: only the virtual
//!    desktop's guard stops them. `approve` is an open tool.
//! 2. The verb:
//!    - none or `help`: the help page.
//!    - `verify`, `show`, `key-id`: the `k2-approve` program, run in this
//!      process's place ([`local`]).
//!    - `request --payload <file> [--to <login>] [--wait] [--out <file>]
//!      [--json]`: the payload's bytes in base64, one POST, then with
//!      `--wait` a GET of `/cli/approve/wait` until the state is not
//!      `waiting`, and the approval record written to `--out`.
//!    - `status [<id>] [--out <file>] [--json]`, `cancel <id>`, `agents
//!      [<workspace> on|off]`.
//!
//!    Each request is `_compute_call` ([`cp::call`]); the answers are
//!    printed as the shell's Python prints them, its tracebacks too.
//! 3. The EXIT trap's lines ([`cp::close`]).
//!
//! A flag that takes a value and comes last ends the command with 1 and
//! no line (`shift 2` fails under `set -e`), as the shell's does. `-h` and
//! `--help` anywhere are the front's: the shell answers them, and on
//! `cancel` and `agents` it sends the request (a bash bug K2 fixes).

pub mod cp;

use super::app::call::{Cj, Ctx};
use super::app::py::{self as p, Out};
use super::{Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::json::os_error;
use crate::out::{self, py};
use crate::transport::encode::quote;
use cp::{body_line, raised, read_one, show, stdout_line, Opened};
use k2_client::{cli_name, product_name};
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["approve"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

const USAGE_REQUEST: &str = concat!(
    "Usage: ",
    cli_name!(),
    " approve request --payload <file> [--to <login>] [--wait] [--out <file>]"
);
const USAGE_STATUS: &str = concat!(
    "Usage: ",
    cli_name!(),
    " approve status [<id>] [--out <file>] [--json]"
);
const USAGE_CANCEL: &str = concat!("Usage: ", cli_name!(), " approve cancel <id>");
const USAGE_AGENTS: &str = concat!(
    "Usage: ",
    cli_name!(),
    " approve agents [<workspace> on|off]"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Help,
    /// `verify|show|key-id` and the words after the verb.
    Local(Vec<String>),
    Request {
        payload: String,
        to: String,
        wait: bool,
        out: String,
        json: bool,
    },
    Status {
        id: String,
        out: String,
        json: bool,
    },
    Cancel(String),
    AgentsList,
    AgentsSet(String, bool),
    /// Lines on stderr and an exit, no request.
    Stop(Vec<String>, Exit),
}

fn stop(line: &str, exit: Exit) -> Ask {
    Ask::Stop(vec![line.to_string()], exit)
}

/// The words after `approve`.
pub fn parse(words: &[&str]) -> Ask {
    let (sub, rest) = match words.split_first() {
        Some((s, r)) => (*s, r),
        None => ("", &[][..]),
    };
    // `shift 2` with one word left fails: exit 1, no line.
    let shift_fails = Ask::Stop(vec![], Exit::Error);
    match sub {
        "" | "help" => Ask::Help,
        "verify" | "show" | "key-id" => {
            Ask::Local(words.iter().map(|w| (*w).to_string()).collect())
        }
        "request" => {
            let (mut payload, mut to, mut out) = (String::new(), String::new(), String::new());
            let (mut wait, mut json) = (false, false);
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "--payload" | "--to" | "--out" => {
                        let Some(v) = rest.get(i + 1) else {
                            return shift_fails;
                        };
                        let slot = match rest[i] {
                            "--payload" => &mut payload,
                            "--to" => &mut to,
                            _ => &mut out,
                        };
                        *slot = (*v).to_string();
                        i += 2;
                    }
                    "--wait" => {
                        wait = true;
                        i += 1;
                    }
                    "--json" => {
                        json = true;
                        i += 1;
                    }
                    "-h" | "--help" => return Ask::Help,
                    other => {
                        return stop(
                            &format!(
                                concat!(
                                    "Error: unexpected argument '{}' (",
                                    cli_name!(),
                                    " approve --help)"
                                ),
                                other
                            ),
                            Exit::Usage,
                        )
                    }
                }
            }
            if payload.is_empty() {
                return stop(USAGE_REQUEST, Exit::Usage);
            }
            Ask::Request {
                payload,
                to,
                wait,
                out,
                json,
            }
        }
        "status" => {
            let (mut id, mut out, mut json) = (String::new(), String::new(), false);
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "--out" => {
                        let Some(v) = rest.get(i + 1) else {
                            return shift_fails;
                        };
                        out = (*v).to_string();
                        i += 2;
                    }
                    "--json" => {
                        json = true;
                        i += 1;
                    }
                    w if w.starts_with('-') => return stop(USAGE_STATUS, Exit::Usage),
                    w => {
                        id = w.to_string();
                        i += 1;
                    }
                }
            }
            if !out.is_empty() && id.is_empty() {
                return stop("Error: --out needs a request id", Exit::Usage);
            }
            Ask::Status { id, out, json }
        }
        "cancel" => match rest.first() {
            Some(id) if !id.is_empty() => Ask::Cancel((*id).to_string()),
            _ => stop(USAGE_CANCEL, Exit::Usage),
        },
        "agents" => match rest.first() {
            Some(ws) if !ws.is_empty() => match rest.get(1) {
                Some(&"on") => Ask::AgentsSet((*ws).to_string(), true),
                Some(&"off") => Ask::AgentsSet((*ws).to_string(), false),
                _ => stop(USAGE_AGENTS, Exit::Usage),
            },
            _ => Ask::AgentsList,
        },
        other => Ask::Stop(
            vec![format!(
                concat!("Error: unknown ", cli_name!(), " approve command: {}"),
                other
            )],
            Exit::Usage,
        ),
    }
}

/// `k5 approve ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let ask = parse(&words);
    let local = matches!(
        words.first(),
        None | Some(&("" | "help" | "verify" | "show" | "key-id"))
    );
    let mut ctx = match cp::open(args, local) {
        Opened::Shell => return None,
        Opened::Stop(Exit::Ok) if local => return local_run(&ask),
        Opened::Stop(e) => return Some(e),
        Opened::Ready(c, _) => c,
    };
    let exit = match ask {
        Ask::Stop(lines, exit) => {
            for l in &lines {
                out::stderr_line(l);
            }
            // The unknown verb's help follows its line.
            if lines.first().is_some_and(|l| l.starts_with("Error: unknown")) {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), help().as_bytes());
            }
            exit
        }
        Ask::Request {
            payload,
            to,
            wait,
            out,
            json,
        } => request(&mut ctx, &payload, &to, wait, &out, json),
        Ask::Status { id, out, json } => status(&mut ctx, &id, &out, json),
        Ask::Cancel(id) => cancel(&mut ctx, &id),
        Ask::AgentsList => agents_list(&mut ctx),
        Ask::AgentsSet(ws, on) => agents_set(&mut ctx, &ws, on),
        // `local` lines never get a client.
        Ask::Help | Ask::Local(_) => Exit::Error,
    };
    Some(cp::close(ctx, exit))
}

/// The help page (`_approve_usage`).
fn help() -> &'static str {
    crate::text::help("approve").expect("the approve help page is embedded")
}

fn print_help() -> Exit {
    match out::stdout(help()) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// A line that needs no daemon: the help, or `k2-approve`.
fn local_run(ask: &Ask) -> Option<Exit> {
    match ask {
        Ask::Help => Some(print_help()),
        Ask::Local(words) => local(words),
        _ => None,
    }
}

/// `cmd_approve_local`: `K5_`, `K2_` or `K2SO_APPROVE_BIN` (it must be a
/// program), else `k2-approve` in the folder of the program people ran,
/// else on `PATH`; then it runs in this process's place with the verb and
/// its words. `None`: the shell's (a `K2_APPROVE_BIN` that is executable
/// but not a file, which bash can't run either and says so its own way).
fn local(words: &[String]) -> Option<Exit> {
    let named = ["K5_APPROVE_BIN", "K2_APPROVE_BIN", "K2SO_APPROVE_BIN"]
        .iter()
        .filter_map(|n| std::env::var_os(n))
        .find(|v| !v.is_empty());
    let program = |p: &std::path::Path| p.is_file() && executable(p);
    let bin = match named {
        Some(b) => {
            let path = std::path::PathBuf::from(&b);
            if !executable(&path) {
                out::stderr_line(&format!(
                    concat!(cli_name!(), " approve: K2_APPROVE_BIN={} is not a program"),
                    path.to_string_lossy()
                ));
                return Some(Exit::Usage);
            }
            if !path.is_file() {
                return None;
            }
            Some(path)
        }
        None => {
            let beside = std::env::current_exe()
                .ok()
                .and_then(|exe| crate::fallthrough::script_dir_for(&exe))
                .map(|d| d.join("k2-approve"))
                .filter(|p| program(p));
            beside.or_else(|| {
                std::env::var_os("PATH").and_then(|path| {
                    std::env::split_paths(&path)
                        .map(|d| {
                            if d.as_os_str().is_empty() {
                                std::path::PathBuf::from("k2-approve")
                            } else {
                                d.join("k2-approve")
                            }
                        })
                        .find(|p| program(p))
                })
            })
        }
    };
    let Some(bin) = bin else {
        let verb = words.first().map_or("", String::as_str);
        out::stderr_line(&format!(
            concat!(
                cli_name!(),
                " approve {}: the k2-approve program isn't installed here."
            ),
            verb
        ));
        out::stderr_line(concat!(
            "  Build it from the ",
            product_name!(),
            " source: cargo install --path crates/k2-approve"
        ));
        out::stderr_line(
            "  Or check by hand with openssl: docs/signed-approvals.md (\"With openssl only\").",
        );
        return Some(Exit::Usage);
    };
    Some(exec(&bin, words))
}

#[cfg(unix)]
fn executable(p: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: a valid NUL-terminated path and a plain mode.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

#[cfg(not(unix))]
fn executable(p: &std::path::Path) -> bool {
    p.exists()
}

/// `exec "$bin" "$@"`.
#[cfg(unix)]
fn exec(bin: &std::path::Path, words: &[String]) -> Exit {
    use std::os::unix::process::CommandExt;
    let e = std::process::Command::new(bin).args(words).exec();
    out::stderr_line(&format!("{}: {e}", bin.to_string_lossy()));
    Exit::from_curl(126)
}

#[cfg(not(unix))]
fn exec(bin: &std::path::Path, words: &[String]) -> Exit {
    match std::process::Command::new(bin).args(words).status() {
        Ok(s) => Exit::from_curl(s.code().unwrap_or(1)),
        Err(e) => {
            out::stderr_line(&format!("{}: {e}", bin.to_string_lossy()));
            Exit::from_curl(126)
        }
    }
}

fn request(ctx: &mut Ctx, payload: &str, to: &str, wait: bool, out: &str, json: bool) -> Exit {
    if !std::path::Path::new(payload).is_file() {
        out::stderr_line(&format!("Error: no payload file at {payload}"));
        return Exit::Usage;
    }
    if !out.is_empty() && !wait {
        out::stderr_line(concat!(
            "Error: --out needs --wait (or use ",
            cli_name!(),
            " approve status <id> --out <file> later)"
        ));
        return Exit::Usage;
    }
    let bytes = match std::fs::read(payload) {
        Ok(b) => b,
        Err(e) => {
            let f = os_error(&e, payload);
            return raised(&Raise::at(3, f.last_line()));
        }
    };
    let mut body = vec![(
        Text::from("payload_b64"),
        p::text(&super::vault::verbs::b64(&bytes)),
    )];
    if !to.is_empty() {
        body.push((Text::from("to"), p::text(to)));
    }
    let body = json::to_string(&Value::Object(body), &Options::default())
        .expect("json.dumps of strings");
    let answer = match cp::call(ctx, true, "/cli/approve/request", &body) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let id = match read_one(&answer, "id", false) {
        Ok(id) => id,
        Err(r) => return raised(&r),
    };
    if wait {
        if !json {
            out::stderr_line(&format!(
                "Asked: {id}. Waiting for the answer on their phone\u{2026}"
            ));
        }
        return wait_for(ctx, &id, out, json);
    }
    if json {
        return body_line(&answer);
    }
    if let Err(e) = show(&print(&answer)) {
        return e;
    }
    stdout_line(&format!(
        concat!(
            "They approve on their phone. Check with: ",
            cli_name!(),
            " approve status {} (or ask again with --wait)."
        ),
        id
    ))
}

/// `_approve_wait`: exit 0 approved, 1 otherwise.
fn wait_for(ctx: &mut Ctx, id: &str, out: &str, json: bool) -> Exit {
    let query = format!("id={}&timeout=45", quote(id));
    let (answer, state) = loop {
        let answer = match cp::call(ctx, false, "/cli/approve/wait", &query) {
            Ok(b) => b,
            Err(e) => return e,
        };
        let state = match read_one(&answer, "state", true) {
            Ok(s) => s,
            Err(r) => return raised(&r),
        };
        if state != "waiting" {
            break (answer, state);
        }
    };
    let shown = if json {
        body_line(&answer)
    } else {
        print(&answer).print()
    };
    if shown != Exit::Ok {
        return shown;
    }
    if state != "approved" {
        return Exit::Error;
    }
    if !out.is_empty() {
        let s = write_record(&answer, out);
        // `>&2`: what it prints goes to stderr.
        let all = s.stdout.clone() + &s.stderr;
        return Shown::err(all, s.exit).print();
    }
    Exit::Ok
}

fn status(ctx: &mut Ctx, id: &str, out: &str, json: bool) -> Exit {
    let query = if id.is_empty() {
        String::new()
    } else {
        format!("id={}", quote(id))
    };
    let answer = match cp::call(ctx, false, "/cli/approve/status", &query) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let shown = if json {
        body_line(&answer)
    } else {
        print(&answer).print()
    };
    if shown != Exit::Ok || out.is_empty() {
        return shown;
    }
    write_record(&answer, out).print()
}

fn cancel(ctx: &mut Ctx, id: &str) -> Exit {
    let body = Cj::new().s("id", id).body();
    if let Err(e) = cp::call(ctx, true, "/cli/approve/cancel", &body) {
        return e;
    }
    stdout_line(&format!(
        "Cancelled: {id}. The card on the Thread says so."
    ))
}

fn agents_set(ctx: &mut Ctx, ws: &str, on: bool) -> Exit {
    let body = Cj::new()
        .s("workspace", ws)
        .b("on", if on { "1" } else { "0" })
        .body();
    if let Err(e) = cp::call(ctx, true, "/cli/approve/agents", &body) {
        return e;
    }
    stdout_line(&format!(
        "approve.request is {} for {ws}.",
        if on { "on" } else { "off" }
    ))
}

fn agents_list(ctx: &mut Ctx) -> Exit {
    let answer = match cp::call(ctx, false, "/cli/approve/agents", "") {
        Ok(b) => b,
        Err(e) => return e,
    };
    agents_reader(&answer).print()
}

/// The `agents` list's Python.
pub fn agents_reader(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let rows = p::or(p::dget(&v, "agents", 3)?, p::empty_list());
        if !p::truthy(&rows) {
            o.line("No agent may ask for signed approvals yet.");
            return Ok(Exit::Ok);
        }
        let mut lines = Vec::new();
        let gen = |e: Raise| Raise {
            frames: vec![(4, "<module>"), (4, "<genexpr>")],
            exception: e.exception,
        };
        for r in p::iter(&rows, 4)? {
            let ws = p::idx(&r, "workspace", 4).map_err(gen)?;
            let by = p::idx(&r, "grantedBy", 4).map_err(gen)?;
            lines.push(format!(
                "{}  (turned on by {})",
                py::str_of(&ws),
                py::str_of(&by)
            ));
        }
        o.line(&lines.join("\n"));
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// `_approve_print`: one request, or the agent's list.
pub fn print(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let rows = p::dget(&v, "requests", 4)?;
        if rows == Value::Null {
            let l = line(&v).map_err(|e| from(13, e))?;
            o.line(&l);
        } else if !p::truthy(&rows) {
            o.line("No approval requests from this agent.");
        } else {
            for r in p::iter(&rows, 17)? {
                let l = line(&r).map_err(|e| from(18, e))?;
                o.line(&l);
            }
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// An exception inside `line(r)`, called from the module's line `at`.
fn from(at: u32, mut e: Raise) -> Raise {
    let mut frames = vec![(at, "<module>")];
    frames.append(&mut e.frames);
    e.frames = frames;
    e
}

/// `line(r)`: its frames start at `line` itself.
fn line(r: &Value) -> Result<String, Raise> {
    let at = |e: Raise| Raise {
        frames: vec![(8, "line")],
        exception: e.exception,
    };
    let id = p::idx(r, "id", 8).map_err(at)?;
    let state = p::idx(r, "state", 8).map_err(at)?;
    let kind = p::dget_or(r, "kind", p::text(""), 8).map_err(at)?;
    let person = p::dget_or(r, "person", p::text(""), 8).map_err(at)?;
    let by = p::dget(r, "answerBy", 8).map_err(at)?;
    let by = if p::truthy(&by) {
        cp::utc_time(&by, "%H:%M:%S UTC", 6).map_err(|e| Raise {
            frames: vec![(8, "line"), (6, "t")],
            exception: e.exception,
        })?
    } else {
        "-".to_string()
    };
    let mut out = format!(
        "{}  {}  {}  to {}, answer by {by}",
        py::str_of(&id),
        super::py_left(&state, 9),
        py::str_of(&kind),
        py::str_of(&person),
    );
    let st = p::dget(r, "state", 9).map_err(at)?;
    if (st == p::text("approved") || st == p::text("declined"))
        && p::truthy(&p::dget(r, "device", 9).map_err(at)?)
    {
        let device = p::idx(r, "device", 10).map_err(|e| Raise {
            frames: vec![(10, "line")],
            exception: e.exception,
        })?;
        out.push_str(&format!("  (on {})", py::str_of(&device)));
    }
    Ok(out)
}

/// `_approve_write_record <body> <file>`.
pub fn write_record(body: &[u8], path: &str) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let rec = p::dget(&v, "recordText", 4)?;
        let state = p::dget(&v, "state", 5)?;
        let usable = matches!(&rec, Value::Str(_)) && p::truthy(&rec);
        if state != p::text("approved") || !usable {
            let id = p::dget(&v, "id", 6)?;
            o.err(&format!(
                "Error: {} is {}; there is no approval record to write",
                py::str_of(&id),
                py::str_of(&p::dget(&v, "state", 6)?)
            ));
            return Ok(Exit::Error);
        }
        std::fs::write(path, py::str_of(&rec))
            .map_err(|e| Raise::at(9, os_error(&e, path).last_line()))?;
        o.line(&format!(
            "Wrote the approval record to {path}. Give it to the relying party: it checks the signature."
        ));
        Ok(Exit::Ok)
    })();
    o.done(r)
}

#[cfg(test)]
mod tests;
