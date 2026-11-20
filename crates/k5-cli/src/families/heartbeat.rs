//! `k5 heartbeat ...`: named heartbeats (scheduled wakes), as the shell
//! CLI's `heartbeat` arm answers them (`cli/src/router.sh`; the verbs in
//! `cli/src/remind.sh`, `cli/src/heartbeat.sh` and
//! `cli/src/wiki.sh`'s `cmd_heartbeat_toggle`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), and the two gates
//!    ([`crate::prelude`]). `heartbeat` is the `heartbeat` tool, an `id`
//!    tool: in a session a stale passport and no passport at all are
//!    refused with exit 3 ([`crate::prelude::id_tool`]).
//! 2. The words ([`parse`]): `remind` and `reminders` are
//!    [`super::remind`]'s; `help` prints the help; the retired verbs are
//!    `fail_deprecated`'s lines; an unknown verb is its two lines, exit 2.
//! 3. Each verb's requests through `cli_send` (the cell socket first in a
//!    session, `/cli/heartbeat/` being a route it serves), and its Python
//!    reader ([`read`]).
//! 4. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell answers these forms itself ([`run`] returns `None`): any
//! `-h` or `--help` (the front sends those to a page or the shell);
//! `wake` (`signal wake`), a Python block of its own that makes several
//! requests; a value flag as the last word (the shell stops on
//! `$2: unbound variable` or a `shift 2` out of range, K2's D6 class); a
//! word bash would expand as a file name in `--days` or `--months`; an
//! `--instructions-file` holding a NUL or bytes that are not UTF-8;
//! `wakeup --edit` with no `EDITOR`, or one not on `PATH`; a word that
//! is not UTF-8; a port curl won't take; a `python3` the binary can't
//! tell; and a `--workspace` folder the binary can't resolve as bash's
//! `cd` would ([`super::workspace::register::path`]).

pub mod py;
pub mod read;
pub mod spec;
pub mod when;

use self::read::{Edited, Shows};
use self::spec::Spec;
use super::remind::shared::post_piped;
use super::workspace::register::{self, Verb as RegisterVerb};
use super::{curl_port, hook_sock, piped_body, pipefail, request_piped, Piped, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Client, Error};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["heartbeat"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's tool policy.
pub const TOOL: &str = "heartbeat";

pub const LIST: &str = "/cli/heartbeat/list";
pub const LIST_ARCHIVED: &str = "/cli/heartbeat/list-archived";
pub const ADD: &str = "/cli/heartbeat/add";
pub const EDIT: &str = "/cli/heartbeat/edit";
pub const REMOVE: &str = "/cli/heartbeat/remove";
pub const ARCHIVE: &str = "/cli/heartbeat/archive";
pub const UNARCHIVE: &str = "/cli/heartbeat/unarchive";
pub const FIRE: &str = "/cli/heartbeat/fire";
pub const RESTART: &str = "/cli/heartbeat/restart";
pub const ENABLE: &str = "/cli/heartbeat/enable";
pub const PINNED: &str = "/cli/heartbeat/set-use-workspace-session";
pub const SET_SESSION: &str = "/cli/heartbeat/set-session";
pub const RENAME: &str = "/cli/heartbeat/rename";
pub const SHOW: &str = "/cli/heartbeat/show";
pub const STATUS: &str = "/cli/heartbeat/status";
pub const LOG: &str = "/cli/heartbeat-log";
pub const SCHEDULER: &str = "/cli/heartbeat/scheduler-status";

/// What a command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    /// `heartbeat help`: the help on stdout.
    Help,
    /// Lines on stderr, and the exit; nothing sent.
    Stop(Vec<String>, Exit),
    List {
        json: bool,
        ws: Option<String>,
    },
    Archived {
        json: bool,
    },
    Remove {
        name: String,
        purge: bool,
    },
    Unarchive {
        name: String,
    },
    Fire {
        name: String,
        force: bool,
    },
    Restart {
        name: String,
        ws: Option<String>,
        json: bool,
    },
    Enable {
        on: bool,
        name: String,
        ws: Option<String>,
        json: bool,
    },
    Pinned {
        on: bool,
        name: String,
    },
    SessionShow {
        name: String,
    },
    SessionSet {
        name: String,
        mode: &'static str,
        id: String,
        provider: String,
    },
    Rename {
        from: String,
        to: String,
    },
    Edit {
        name: String,
        spec: Option<(String, String)>,
        instructions: Option<String>,
        tz: Option<String>,
    },
    Add {
        name: String,
        spec: (String, String),
        instructions: String,
        tz: Option<String>,
    },
    Show {
        name: String,
        json: bool,
        /// `--workspace` as sent, and whether it named a folder.
        ws: Option<(String, bool)>,
    },
    Wakeup {
        name: String,
        mode: Wake,
    },
    Status {
        name: String,
        limit: String,
    },
    Log {
        limit: String,
    },
}

/// What `wakeup` does with the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    Cat,
    Path,
    Edit,
}

fn stop(lines: &[&str], exit: Exit) -> Option<Form> {
    Some(Form::Stop(
        lines.iter().map(|l| l.to_string()).collect(),
        exit,
    ))
}

fn usage(line: String, exit: Exit) -> Option<Form> {
    Some(Form::Stop(vec![line], exit))
}

/// `fail_deprecated <old> <new>`.
fn deprecated(old: &str, new: &str) -> Option<Form> {
    Some(Form::Stop(
        vec![
            format!("Error: '{old}' is no longer supported."),
            String::new(),
            "Use instead:".into(),
            format!("  {new}"),
            String::new(),
            format!(
                "Run '{} help-deprecated' for the full map of retired verbs.",
                cli_name!()
            ),
        ],
        Exit::Error,
    ))
}

/// The words after `heartbeat` (not `remind` or `reminders`). `resolve`
/// is `cd "$w" && pwd` on a folder (`None`: the shell's). `None`: a form
/// the shell answers.
pub fn parse(rest: &[&str], resolve: &dyn Fn(&str) -> Option<String>) -> Option<Form> {
    let tail = rest.get(1..).unwrap_or_default();
    match rest.first().copied().unwrap_or("") {
        "" => list(&[], resolve),
        "help" => Some(Form::Help),
        "schedule" => {
            let more = tail.get(1..).unwrap_or_default();
            match tail.first().copied().unwrap_or("") {
                "" => list(tail, resolve),
                "list" => list(more, resolve),
                "add" => add(more),
                "remove" | "archive" => remove(more),
                "unarchive" => unarchive(more),
                "edit" => edit(more),
                "rename" => rename(more),
                "enable" => enable(true, more, resolve),
                "disable" => enable(false, more, resolve),
                "off" => deprecated(
                    "heartbeat schedule off",
                    &format!(
                        "{} heartbeat schedule disable <name>   (per-heartbeat pause)",
                        cli_name!()
                    ),
                ),
                f @ ("hourly" | "daily" | "weekly" | "monthly" | "yearly") => deprecated(
                    &format!("heartbeat schedule {f}"),
                    &format!(
                        "{} heartbeat schedule add --name <n> --{f} ...   (see '{} help heartbeat')",
                        cli_name!(),
                        cli_name!()
                    ),
                ),
                other => usage2(
                    format!("Unknown 'heartbeat schedule' subcommand: {other}"),
                    format!("Verbs: add, list [--archived], remove <n> [--purge], unarchive, edit, rename, enable, disable. See '{} help heartbeat'.", cli_name!()),
                ),
            }
        }
        "signal" => {
            let more = tail.get(1..).unwrap_or_default();
            match tail.first().copied().unwrap_or("") {
                "fire" => fire(more),
                "wakeup" => wakeup(more),
                "wake" => None,
                other => usage2(
                    format!("Unknown 'heartbeat signal' subcommand: {other}"),
                    format!(
                        "Verbs: fire <n>, wakeup <n> [--path-only|--edit], wake. See '{} help heartbeat'.",
                        cli_name!()
                    ),
                ),
            }
        }
        "wake" => None,
        "log" => log(tail),
        "add" => add(tail),
        "list" => list(tail, resolve),
        "list-archived" => Some(Form::Archived {
            json: tail.contains(&"--json"),
        }),
        "archive" | "remove" => remove(tail),
        "unarchive" => unarchive(tail),
        "fire" => fire(tail),
        "enable" => enable(true, tail, resolve),
        "disable" => enable(false, tail, resolve),
        "use-pinned-session" => {
            let on = match tail.first().copied().unwrap_or("") {
                "on" | "true" | "1" => true,
                "off" | "false" | "0" => false,
                _ => return pinned_usage(),
            };
            match tail.get(1).copied().unwrap_or("") {
                "" => pinned_usage(),
                name => Some(Form::Pinned {
                    on,
                    name: name.to_string(),
                }),
            }
        }
        "session" => session(tail),
        "restart" => restart(tail, resolve),
        "rename" => rename(tail),
        "edit" => edit(tail),
        "show" => show(tail, resolve),
        "wakeup" => wakeup(tail),
        "status" => status(tail),
        other => toggle(other),
    }
}

fn usage2(a: String, b: String) -> Option<Form> {
    Some(Form::Stop(vec![a, b], Exit::Usage))
}

fn pinned_usage() -> Option<Form> {
    usage(
        format!(
            "Usage: {} heartbeat use-pinned-session [on|off] <name>",
            cli_name!()
        ),
        Exit::Error,
    )
}

/// `cmd_heartbeat_toggle`: the retired verbs, else unknown.
fn toggle(action: &str) -> Option<Form> {
    let c = cli_name!();
    let backoff = format!("nothing \u{2014} per-agent adaptive backoff was removed; heartbeat cadence is user-owned via '{c} heartbeat schedule'");
    match action {
        "on" | "enable" | "1" | "true" | "off" | "disable" | "0" | "false" => deprecated(
            &format!("heartbeat {action}"),
            &format!("{c} heartbeat schedule enable|disable <name>   (per-heartbeat pause/resume)"),
        ),
        "noop" | "action" => deprecated(&format!("heartbeat {action}"), &backoff),
        "force" => deprecated(
            "heartbeat force",
            &format!("{c} heartbeat signal fire <name>  (fires a named heartbeat now)"),
        ),
        "set" => deprecated(
            "heartbeat set",
            &format!(
                "{c} heartbeat schedule add/edit --name <n> <spec>  (see '{c} help heartbeat')"
            ),
        ),
        "get" => deprecated(
            "heartbeat get",
            &format!("{c} heartbeat show <name> [--json]"),
        ),
        _ => usage2(
            format!("Unknown 'heartbeat' subcommand: {action}"),
            format!("Run '{c} help heartbeat' for the schedule/signal/inspection surface."),
        ),
    }
}

/// `--workspace <w>`'s value as sent: a folder becomes its absolute path.
fn ws_value(w: &str, resolve: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    resolve(w)
}

/// `cmd_heartbeat_list`.
fn list(args: &[&str], resolve: &dyn Fn(&str) -> Option<String>) -> Option<Form> {
    let (mut json, mut archived, mut ws) = (false, false, None);
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--json" => json = true,
            "--archived" => archived = true,
            "--workspace" | "--project" => {
                let w = args.get(i + 1).copied().unwrap_or("");
                if w.is_empty() {
                    return usage(
                        format!(
                            "Usage: {} heartbeat schedule list [--archived] [--workspace <ws>] [--json]",
                            cli_name!()
                        ),
                        Exit::Usage,
                    );
                }
                ws = Some(ws_value(w, resolve)?);
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    if ws.is_some() && archived {
        return stop(
            &["Error: --archived with --workspace isn't supported; list the active ones, or run it from that workspace"],
            Exit::Usage,
        );
    }
    if archived {
        return Some(Form::Archived { json });
    }
    Some(Form::List { json, ws })
}

/// `cmd_heartbeat_remove`.
fn remove(args: &[&str]) -> Option<Form> {
    let (mut name, mut purge) = (String::new(), false);
    for a in args {
        match *a {
            "--purge" => purge = true,
            a if a.starts_with('-') => {}
            a => {
                if name.is_empty() {
                    name = a.to_string();
                }
            }
        }
    }
    if name.is_empty() {
        return usage(
            format!("Usage: {} heartbeat remove <name> [--purge]", cli_name!()),
            Exit::Error,
        );
    }
    Some(Form::Remove { name, purge })
}

/// `cmd_heartbeat_unarchive`.
fn unarchive(args: &[&str]) -> Option<Form> {
    match args.first().copied().unwrap_or("") {
        "" => usage(
            format!("Usage: {} heartbeat unarchive <name>", cli_name!()),
            Exit::Error,
        ),
        n => Some(Form::Unarchive {
            name: n.to_string(),
        }),
    }
}

/// `cmd_heartbeat_fire`.
fn fire(args: &[&str]) -> Option<Form> {
    let fire_usage = format!(
        "Usage: {} heartbeat signal fire <name> [--force]",
        cli_name!()
    );
    let (mut name, mut force) = (String::new(), false);
    for a in args {
        match *a {
            "--force" => force = true,
            a if a.starts_with('-') => {
                return Some(Form::Stop(
                    vec![
                        format!("Unknown flag for 'heartbeat signal fire': {a}"),
                        fire_usage,
                    ],
                    Exit::Usage,
                ))
            }
            a => {
                if name.is_empty() {
                    name = a.to_string();
                }
            }
        }
    }
    if name.is_empty() {
        return usage(fire_usage, Exit::Error);
    }
    Some(Form::Fire { name, force })
}

/// A verb's name, `--workspace` and `--json`.
type Named = (String, Option<String>, bool);

/// The loop `restart` and `enable`/`disable` share: the name,
/// `--workspace` and `--json`.
fn named(
    args: &[&str],
    usage_line: &str,
    empty: Exit,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> Option<Result<Named, Form>> {
    let stop2 = |a: String| {
        Some(Err(Form::Stop(
            vec![a, usage_line.to_string()],
            Exit::Usage,
        )))
    };
    let (mut name, mut ws, mut json) = (String::new(), String::new(), false);
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        match a {
            "--json" => json = true,
            "--workspace" | "--project" => {
                ws = args.get(i + 1).copied().unwrap_or("").to_string();
                if ws.is_empty() {
                    return Some(Err(Form::Stop(vec![usage_line.to_string()], Exit::Usage)));
                }
                i += 1;
            }
            _ if a.starts_with("--workspace=") || a.starts_with("--project=") => {
                ws = a.split_once('=').map_or("", |(_, v)| v).to_string();
            }
            _ if a.starts_with('-') => return stop2(format!("Unknown flag: {a}")),
            _ if name.is_empty() => name = a.to_string(),
            _ => return stop2(format!("Unexpected argument: {a}")),
        }
        i += 1;
    }
    if name.is_empty() {
        return Some(Err(Form::Stop(vec![usage_line.to_string()], empty)));
    }
    let ws = if ws.is_empty() {
        None
    } else {
        Some(ws_value(&ws, resolve)?)
    };
    Some(Ok((name, ws, json)))
}

/// `cmd_heartbeat_restart`.
fn restart(args: &[&str], resolve: &dyn Fn(&str) -> Option<String>) -> Option<Form> {
    let line = format!(
        "Usage: {} heartbeat restart <name> [--workspace <ws>] [--json]",
        cli_name!()
    );
    Some(match named(args, &line, Exit::Usage, resolve)? {
        Ok((name, ws, json)) => Form::Restart { name, ws, json },
        Err(f) => f,
    })
}

/// `cmd_heartbeat_enable_disable`.
fn enable(on: bool, args: &[&str], resolve: &dyn Fn(&str) -> Option<String>) -> Option<Form> {
    let line = format!(
        "Usage: {} heartbeat schedule enable|disable <name> [--workspace <ws>] [--json]",
        cli_name!()
    );
    Some(match named(args, &line, Exit::Error, resolve)? {
        Ok((name, ws, json)) => Form::Enable { on, name, ws, json },
        Err(f) => f,
    })
}

/// `cmd_heartbeat_session`.
fn session(args: &[&str]) -> Option<Form> {
    let line = format!(
        "Usage: {} heartbeat session <name> [--pinned | --auto | --set <session-id|workspace/handle> [--provider <p>]]",
        cli_name!()
    );
    let (mut name, mut mode, mut id, mut provider) =
        (String::new(), "", String::new(), String::new());
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--pinned" => mode = "pinned",
            "--auto" => mode = "auto",
            "--set" | "--provider" => {
                // `shift 2` out of range ends the shell's command.
                let v = args.get(i + 1)?.to_string();
                if args[i] == "--set" {
                    mode = "session";
                    id = v;
                } else {
                    provider = v;
                }
                i += 1;
            }
            a if a.starts_with('-') => {
                return Some(Form::Stop(
                    vec![format!("Unknown flag for 'heartbeat session': {a}"), line],
                    Exit::Error,
                ))
            }
            a => {
                if name.is_empty() {
                    name = a.to_string();
                }
            }
        }
        i += 1;
    }
    if name.is_empty() {
        return usage(line, Exit::Error);
    }
    if mode.is_empty() {
        return Some(Form::SessionShow { name });
    }
    if mode == "session" {
        if id.is_empty() {
            return usage(
                format!(
                    "Usage: {} heartbeat session <name> --set <session-id|workspace/handle> [--provider <p>]",
                    cli_name!()
                ),
                Exit::Error,
            );
        }
    } else if !provider.is_empty() {
        return stop(
            &["--provider only applies with --set <session-id|workspace/handle>"],
            Exit::Error,
        );
    }
    Some(Form::SessionSet {
        name,
        mode,
        id,
        provider,
    })
}

/// `cmd_heartbeat_rename`.
fn rename(args: &[&str]) -> Option<Form> {
    let from = args.first().copied().unwrap_or("");
    let to = args.get(1).copied().unwrap_or("");
    if from.is_empty() || to.is_empty() {
        return usage(
            format!(
                "Usage: {} heartbeat rename <old-name> <new-name>",
                cli_name!()
            ),
            Exit::Error,
        );
    }
    Some(Form::Rename {
        from: from.into(),
        to: to.into(),
    })
}

/// `instructions="$(cat "$file")"`: `Err` with the stop when the file is
/// not there, `None` for one the shell must read (a NUL, bytes that are
/// not UTF-8).
fn read_instructions(file: &str) -> Option<Result<String, Form>> {
    if !Path::new(file).is_file() {
        return Some(Err(Form::Stop(
            vec![format!("Error: --instructions-file not found: {file}")],
            Exit::Error,
        )));
    }
    let bytes = std::fs::read(file).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let mut text = String::from_utf8(bytes).ok()?;
    while text.ends_with('\n') {
        text.pop();
    }
    Some(Ok(text))
}

/// The flags `add` and `edit` share; `None`: the shell's.
struct Flags {
    name: String,
    instructions: String,
    file: String,
    have_instructions: bool,
    tz: Option<String>,
    spec: Vec<String>,
}

fn flags(args: &[&str], name_first: bool) -> Option<Result<Flags, Form>> {
    let mut f = Flags {
        name: String::new(),
        instructions: String::new(),
        file: String::new(),
        have_instructions: false,
        tz: None,
        spec: Vec::new(),
    };
    let mut i = 0;
    if name_first && args.first().is_some_and(|a| !a.starts_with("--")) {
        f.name = args[0].to_string();
        i = 1;
    }
    while i < args.len() {
        let a = args[i];
        let v = args.get(i + 1).copied();
        match a {
            "--name" => f.name = v?.to_string(),
            "--instructions" => {
                f.instructions = v?.to_string();
                f.have_instructions = true;
            }
            "--instructions-file" => {
                f.file = v?.to_string();
                f.have_instructions = true;
            }
            "--tz" => match v {
                Some(z) => f.tz = Some(z.to_string()),
                None => {
                    return Some(Err(Form::Stop(
                        vec![
                            "Error: --tz needs a zone (for example America/New_York), or server"
                                .into(),
                        ],
                        Exit::Usage,
                    )))
                }
            },
            _ => {
                f.spec.push(a.to_string());
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    Some(Ok(f))
}

/// `hb_parse_spec` on the spec words: `Ok(None)` for none given.
fn made(words: &[String]) -> Option<Result<(String, String), Form>> {
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    match spec::parse(&w) {
        Spec::Made { json, freq } => Some(Ok((json, freq))),
        Spec::Stop(lines, exit) => Some(Err(Form::Stop(lines, exit))),
        Spec::Shell => None,
    }
}

/// `cmd_heartbeat_add`.
fn add(args: &[&str]) -> Option<Form> {
    let mut f = match flags(args, false)? {
        Ok(f) => f,
        Err(stop) => return Some(stop),
    };
    if f.name.is_empty() {
        return usage(
            format!(
                "Usage: {} heartbeat schedule add --name <name> --daily|--weekly|... [--time HH:MM] [--days ...] [--tz Area/City] [--instructions \"...\"|--instructions-file <path>]",
                cli_name!()
            ),
            Exit::Error,
        );
    }
    if !f.file.is_empty() {
        match read_instructions(&f.file)? {
            Ok(t) => f.instructions = t,
            Err(stop) => return Some(stop),
        }
    }
    let spec = match made(&f.spec)? {
        Ok(s) => s,
        Err(stop) => return Some(stop),
    };
    Some(Form::Add {
        name: f.name,
        spec,
        instructions: f.instructions,
        tz: f.tz,
    })
}

/// `[ -z "$(printf '%s' "$x" | tr -d '[:space:]')" ]`.
fn blank(text: &str) -> bool {
    text.bytes()
        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r'))
}

/// `cmd_heartbeat_edit`.
fn edit(args: &[&str]) -> Option<Form> {
    let mut f = match flags(args, true)? {
        Ok(f) => f,
        Err(stop) => return Some(stop),
    };
    let line = format!(
        "Usage: {} heartbeat edit <name> [--daily|--weekly|... <spec>] [--tz Area/City|server] [--instructions \"...\"|--instructions-file <path>]",
        cli_name!()
    );
    if f.name.is_empty() || (f.spec.is_empty() && !f.have_instructions && f.tz.is_none()) {
        return usage(line, Exit::Error);
    }
    if !f.file.is_empty() {
        match read_instructions(&f.file)? {
            Ok(t) => f.instructions = t,
            Err(stop) => return Some(stop),
        }
    }
    let spec = if f.spec.is_empty() {
        None
    } else {
        match made(&f.spec)? {
            Ok(s) => Some(s),
            Err(stop) => return Some(stop),
        }
    };
    let instructions = if f.have_instructions {
        if blank(&f.instructions) {
            return stop(&["Error: instructions cannot be blank"], Exit::Error);
        }
        Some(f.instructions)
    } else {
        None
    };
    Some(Form::Edit {
        name: f.name,
        spec,
        instructions,
        tz: f.tz,
    })
}

/// `cmd_heartbeat_show`.
fn show(args: &[&str], resolve: &dyn Fn(&str) -> Option<String>) -> Option<Form> {
    let line = format!(
        "Usage: {} heartbeat show <name> [--workspace <ws>] [--json]",
        cli_name!()
    );
    let (mut name, mut json, mut ws) = (String::new(), false, String::new());
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--json" => json = true,
            "--workspace" | "--project" => {
                ws = args.get(i + 1).copied().unwrap_or("").to_string();
                if ws.is_empty() {
                    return usage(line, Exit::Usage);
                }
                i += 1;
            }
            a if a.starts_with('-') => {}
            a => {
                if name.is_empty() {
                    name = a.to_string();
                }
            }
        }
        i += 1;
    }
    if name.is_empty() {
        return usage(line, Exit::Error);
    }
    let ws = if ws.is_empty() {
        None
    } else {
        let dir = Path::new(&ws).is_dir();
        Some((ws_value(&ws, resolve)?, dir))
    };
    Some(Form::Show { name, json, ws })
}

/// Whether `$EDITOR` runs: a path, or a name on `PATH`.
#[cfg(not(unix))]
fn editor_runs(_editor: &str) -> bool {
    false
}

/// Whether `$EDITOR` runs: a path, or a name on `PATH`.
#[cfg(unix)]
fn editor_runs(editor: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let runs = |p: &Path| {
        p.is_file() && std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    };
    if editor.contains('/') {
        return runs(Path::new(editor));
    }
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|d| !editor.is_empty() && runs(&d.join(editor)))
    })
}

/// `cmd_heartbeat_wakeup`.
fn wakeup(args: &[&str]) -> Option<Form> {
    let (mut name, mut mode) = (String::new(), Wake::Cat);
    for a in args {
        match *a {
            "--path-only" => mode = Wake::Path,
            "--edit" => mode = Wake::Edit,
            a if a.starts_with('-') => {}
            a => {
                if name.is_empty() {
                    name = a.to_string();
                }
            }
        }
    }
    if name.is_empty() {
        return usage(
            format!(
                "Usage: {} heartbeat wakeup <name> [--path-only|--edit]",
                cli_name!()
            ),
            Exit::Error,
        );
    }
    if mode == Wake::Edit {
        let editor = std::env::var("EDITOR").ok()?;
        if !editor_runs(&editor) {
            return None;
        }
    }
    Some(Form::Wakeup { name, mode })
}

/// `cmd_heartbeat_status`.
fn status(args: &[&str]) -> Option<Form> {
    let (mut name, mut limit) = (String::new(), "10".to_string());
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--limit" | "-n" => {
                limit = args.get(i + 1)?.to_string();
                i += 1;
            }
            a => {
                if name.is_empty() {
                    name = a.to_string();
                }
            }
        }
        i += 1;
    }
    if name.is_empty() {
        return usage(
            format!("Usage: {} heartbeat status <name> [--limit N]", cli_name!()),
            Exit::Error,
        );
    }
    Some(Form::Status { name, limit })
}

/// `cmd_heartbeat_log`.
fn log(args: &[&str]) -> Option<Form> {
    let mut limit = "50".to_string();
    let mut i = 0;
    while i < args.len() {
        if matches!(args[i], "--limit" | "-n") {
            limit = args.get(i + 1)?.to_string();
            i += 1;
        }
        i += 1;
    }
    Some(Form::Log { limit })
}

/// `k5 heartbeat ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook = hook_sock();
    if hook.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let all: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let rest = all.get(1..).unwrap_or_default();
    if let Some(w) = rest
        .first()
        .copied()
        .filter(|w| matches!(*w, "remind" | "reminders"))
    {
        let mut again = vec![OsString::from(w)];
        again.extend(args.iter().skip(2).cloned());
        return super::remind::run(&again);
    }
    let pwd = daemon::logical_cwd(std::env::var_os("PWD")).ok()?;
    let pwd = pwd.to_str()?.to_string();
    let cdpath = std::env::var_os("CDPATH").is_some();
    let resolve = |w: &str| register::path(RegisterVerb::Open, w, &pwd, cdpath);
    let form = parse(rest, &resolve)?;
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
    Some(run_with(args, form, hook.as_deref(), settings, &pwd))
}

fn run_with(
    args: &[OsString],
    form: Form,
    hook: Option<&Path>,
    mut settings: Settings,
    pwd: &str,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook, would_refuse, TOOL) {
        return stop;
    }
    match form {
        Form::Stop(lines, exit) => {
            for l in lines {
                out::stderr_line(&l);
            }
            return exit;
        }
        Form::Help => {
            let text = crate::text::help("heartbeat").unwrap_or_default();
            return match out::stdout(text) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            };
        }
        _ => {}
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut hb = Hb {
        client,
        closing: Closing::default(),
    };
    let exit = hb.answer(form, pwd);
    let Hb {
        mut client,
        closing,
    } = hb;
    closing.flush(&mut client, exit);
    exit
}

/// A verb's run: the client and the EXIT trap's lines.
struct Hb {
    client: Client,
    closing: Closing,
}

fn name_param(name: &str) -> String {
    format!("name={}", quote(name))
}

fn ws_param(ws: &Option<String>) -> Vec<String> {
    ws.iter()
        .map(|w| format!("workspace={}", quote(w)))
        .collect()
}

fn text(body: &[u8]) -> String {
    String::from_utf8_lossy(body).into_owned()
}

/// What `$(…)` holds of a body: NULs and trailing newlines gone.
fn held(body: &[u8]) -> Vec<u8> {
    let mut b = super::without_nuls(body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

impl Hb {
    /// `cli_send cli_request <endpoint> <params> | <reader>`.
    fn get_piped(&mut self, endpoint: &str, params: &[String]) -> Piped {
        let p: Vec<&str> = params.iter().map(String::as_str).collect();
        request_piped(&mut self.client, &mut self.closing, endpoint, &p)
    }

    /// `_K2_PIPE_MARK=1 cli_send cli_request_post <endpoint> <params> | <reader>`.
    fn post_piped(&mut self, endpoint: &str, params: &[String]) -> Piped {
        let p: Vec<&str> = params.iter().map(String::as_str).collect();
        post_piped(&mut self.client, &mut self.closing, endpoint, &p)
    }

    /// `out="$(cli_send <call> …)"`: the answer as `$(…)` holds it.
    /// `Err`: the command ends there. A failed call holds nothing when
    /// `soft` (`|| true`), else ends the command with its code (`set -e`).
    fn held(
        &mut self,
        endpoint: &str,
        post: bool,
        params: &[String],
        soft: bool,
    ) -> Result<Vec<u8>, Exit> {
        let p: Vec<&str> = params.iter().map(String::as_str).collect();
        let call = if post {
            Call::RequestPost(&p)
        } else {
            Call::Request(&p)
        };
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
            Err(_) if soft => Ok(Vec::new()),
            Err(e) => Err(Exit::from_curl(e.curl_code())),
        }
    }

    /// `cli_send cli_request <endpoint> <params>` on stdout.
    fn raw(&mut self, endpoint: &str, params: &[String]) -> Exit {
        let p: Vec<&str> = params.iter().map(String::as_str).collect();
        let passport = self.client.passport().is_some();
        match self.client.send(endpoint, Call::Request(&p)) {
            Ok(r) => {
                self.closing.answer(&r, passport);
                let body = if r.channel == Channel::Uds {
                    held(&r.body)
                } else {
                    r.body.clone()
                };
                match out::write_flushed(&mut std::io::stdout().lock(), &body) {
                    Ok(()) => Exit::Ok,
                    Err(e) => out::after_stdout_error(&e),
                }
            }
            Err(e @ (Error::NoAnswer(_) | Error::NoOwnerInSession { .. })) => {
                out::stderr_line(&e.to_string());
                Exit::Error
            }
            Err(e) => Exit::from_curl(e.curl_code()),
        }
    }

    /// A piped reader's end: the reader's output, `set -o pipefail`.
    fn shown(piped: &Piped, shown: Shown) -> Exit {
        let exit = pipefail(piped.exit, shown.exit);
        Shown { exit, ..shown }.print()
    }

    fn answer(&mut self, form: Form, pwd: &str) -> Exit {
        match form {
            Form::Help | Form::Stop(..) => Exit::Ok,
            Form::List { json, ws } => {
                let params = ws_param(&ws);
                if json {
                    return self.raw(LIST, &params);
                }
                let p = self.get_piped(LIST, &params);
                Self::shown(&p, read::list(&text(&p.body)))
            }
            Form::Archived { json } => {
                if json {
                    return self.raw(LIST_ARCHIVED, &[]);
                }
                let p = self.get_piped(LIST_ARCHIVED, &[]);
                Self::shown(&p, read::archived(&text(&p.body)))
            }
            Form::Remove { name, purge } => {
                let (endpoint, line) = if purge {
                    (
                        REMOVE,
                        format!("Purged heartbeat: {name} (row deleted, folder removed)"),
                    )
                } else {
                    (ARCHIVE, format!("Archived heartbeat: {name}"))
                };
                let p = self.post_piped(endpoint, &[name_param(&name)]);
                Self::shown(&p, read::success(&text(&p.body), &line))
            }
            Form::Unarchive { name } => {
                let p = self.post_piped(UNARCHIVE, &[name_param(&name)]);
                let line = format!("Unarchived heartbeat: {name}");
                Self::shown(&p, read::success(&text(&p.body), &line))
            }
            Form::Fire { name, force } => {
                let mut params = name_param(&name);
                if force {
                    params.push_str("&force=1");
                }
                let p = self.post_piped(FIRE, &[params]);
                Self::shown(&p, read::fire(&text(&p.body)))
            }
            Form::Restart { name, ws, json } => {
                let mut params = vec![name_param(&name)];
                params.extend(ws_param(&ws));
                let out = match self.held(RESTART, true, &params, true) {
                    Ok(b) => b,
                    Err(exit) => return exit,
                };
                read::restart(&text(&out), &name, json).print()
            }
            Form::Enable { on, name, ws, json } => {
                let mut params = vec![format!("{}&enabled={on}", name_param(&name))];
                params.extend(ws_param(&ws));
                let out = match self.held(ENABLE, true, &params, true) {
                    Ok(b) => b,
                    Err(exit) => return exit,
                };
                read::enable(&text(&out), &name, on, json).print()
            }
            Form::Pinned { on, name } => {
                let params = format!("{}&enabled={on}", name_param(&name));
                let p = self.post_piped(PINNED, &[params]);
                let state = if on { "enabled" } else { "disabled" };
                let line = format!("Pinned-session delivery {state} for heartbeat: {name}");
                Self::shown(&p, read::success(&text(&p.body), &line))
            }
            Form::SessionShow { name } => {
                let p = self.get_piped(LIST, &[]);
                Self::shown(&p, read::session_show(&text(&p.body), &name))
            }
            Form::SessionSet {
                name,
                mode,
                id,
                provider,
            } => {
                let params = format!(
                    "{}&mode={mode}&session_id={}&provider={}",
                    name_param(&name),
                    quote(&id),
                    quote(&provider)
                );
                let p = self.post_piped(SET_SESSION, &[params]);
                Self::shown(&p, read::session_set(&text(&p.body), &name))
            }
            Form::Rename { from, to } => {
                let params = format!("from={}&to={}", quote(&from), quote(&to));
                let p = self.post_piped(RENAME, &[params]);
                let line = format!("Renamed heartbeat: {from} -> {to}");
                Self::shown(&p, read::success(&text(&p.body), &line))
            }
            Form::Edit {
                name,
                spec,
                instructions,
                tz,
            } => self.edit(&name, spec, instructions, tz),
            Form::Add {
                name,
                spec,
                instructions,
                tz,
            } => self.add(&name, spec, &instructions, tz),
            Form::Show { name, json, ws } => {
                let mut params = vec![name_param(&name)];
                let mut shown_ws = String::new();
                if let Some((w, dir)) = &ws {
                    if *dir {
                        self.client.project = w.clone();
                    }
                    params.push(format!("workspace={}", quote(w)));
                    shown_ws = w.clone();
                }
                let p = self.get_piped(SHOW, &params);
                let w = Shows {
                    name: &name,
                    json,
                    ws: &shown_ws,
                };
                Self::shown(&p, read::show(&text(&p.body), &w))
            }
            Form::Wakeup { name, mode } => self.wakeup(&name, mode, pwd),
            Form::Status { name, limit } => {
                let p = self.get_piped(LIST, &[]);
                let first = read::status_row(&text(&p.body), &name);
                let exit = pipefail(p.exit, first.exit);
                let printed = first.print();
                if exit != Exit::Ok || printed != Exit::Ok {
                    return Exit::Error;
                }
                let params = format!("{}&limit={limit}", name_param(&name));
                let p = self.get_piped(STATUS, &[params]);
                Self::shown(&p, read::status_history(&text(&p.body), &name))
            }
            Form::Log { limit } => {
                let p = self.get_piped(LOG, &[format!("limit={limit}")]);
                Self::shown(&p, read::log(&text(&p.body)))
            }
        }
    }

    /// `cmd_heartbeat_add` after its flags.
    fn add(
        &mut self,
        name: &str,
        spec: (String, String),
        instructions: &str,
        tz: Option<String>,
    ) -> Exit {
        let (spec_json, freq) = spec;
        let mut params = format!(
            "{}&frequency={freq}&spec={}",
            name_param(name),
            quote(&spec_json)
        );
        if !instructions.is_empty() {
            params.push_str(&format!("&instructions={}", quote(instructions)));
        }
        if let Some(z) = &tz {
            params.push_str(&format!("&tz={}", quote(z)));
        }
        let resp = match self.held(ADD, true, &[params], false) {
            Ok(b) => text(&b),
            Err(exit) => return exit,
        };
        let parsed = crate::out::json::parse(&resp);
        if let Ok(crate::out::json::Value::Object(pairs)) = &parsed {
            if super::get(pairs, "error").is_some() {
                return super::remind::print_err(&resp).print();
            }
        }
        let d = match self::py::loads(&resp) {
            Ok(d @ crate::out::json::Value::Object(_)) => d,
            Ok(other) => {
                out::stderr_line(&format!(
                    "Error: '{}' object has no attribute 'get'",
                    crate::out::py::type_name(&other)
                ));
                return Exit::Error;
            }
            Err(e) => {
                out::stderr_line(&format!("Error: {}", self::py::msg(&e)));
                return Exit::Error;
            }
        };
        let get = |k: &str| self::py::dget_v(&d, k).unwrap_or(crate::out::json::Value::Null);
        use crate::out::py::{str_of, truthy};
        let wakeup_abs = {
            let v = get("wakeupAbs");
            let mut s = if truthy(&v) {
                str_of(&v)
            } else {
                String::new()
            };
            while s.ends_with('\n') {
                s.pop();
            }
            s
        };
        let written = match get("instructionsWritten") {
            crate::out::json::Value::Null => "missing",
            v if truthy(&v) => "true",
            _ => "false",
        };
        let body_bytes = match get("wakeupBodyBytes") {
            crate::out::json::Value::Int(n) => n,
            crate::out::json::Value::Bool(b) => if b { "True" } else { "False" }.to_string(),
            _ if self::py::is(&get("waitReason"), "wakeup_empty") => "0".into(),
            _ => "-1".into(),
        };
        let kept = truthy(&get("instructionsKeptExisting"));
        let mut o = format!("Created heartbeat: {name}\n");
        let report = read::tz_report(&resp, tz.is_some());
        o.push_str(&report.stdout);
        if let Err(e) = out::stdout(&o) {
            return out::after_stdout_error(&e);
        }
        if !report.stderr.is_empty() {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), report.stderr.as_bytes());
        }
        if report.exit != Exit::Ok {
            return Exit::Error;
        }
        let mut o = String::new();
        if !wakeup_abs.is_empty() {
            o.push_str(&format!(
                "Wakeup file (on the daemon's machine): {wakeup_abs}\n"
            ));
        }
        o.push('\n');
        if let Err(e) = out::stdout(&o) {
            return out::after_stdout_error(&e);
        }
        let c = cli_name!();
        out::stderr_line("NOTE: this schedule lands in the workspace PINNED chat.");
        out::stderr_line(&format!(
            "      A sidecar + `{c} heartbeat session {name} --set sales/reviewer`"
        ));
        out::stderr_line(
            "      (or --set <session-id> --provider <p>) is often better for a trained flow.",
        );
        if let Err(e) = out::stdout("\n") {
            return out::after_stdout_error(&e);
        }
        if !instructions.is_empty() && written == "missing" {
            out::stderr_line(
                "Error: this daemon is older than 'heartbeat add --instructions' and did not write them.",
            );
            out::stderr_line(
                "       The heartbeat exists with an empty WAKEUP.md. Write the instructions to",
            );
            out::stderr_line(&format!(
                "       {wakeup_abs} on the daemon's machine, or update the daemon."
            ));
            return Exit::Error;
        }
        let gt0 = body_bytes.parse::<i64>().is_ok_and(|n| n > 0);
        let mut o = String::new();
        if written == "true" {
            o.push_str(&format!(
                "Wake instructions written ({} bytes).\n",
                instructions.len()
            ));
        } else if kept {
            o.push_str(&format!("Kept the existing WAKEUP.md ({body_bytes} bytes); --instructions were NOT written over it.\n"));
            o.push_str(&format!("Edit it with: {c} heartbeat signal wakeup {name} --edit (or change the file on the daemon's machine).\n"));
        } else if gt0 {
            o.push_str(&format!(
                "Using the existing WAKEUP.md ({body_bytes} bytes).\n"
            ));
        } else if body_bytes == "0" {
            o.push_str(
                "Needs instructions: WAKEUP.md is empty. The heartbeat stays enabled and waits;\n",
            );
            o.push_str("it fires at the next scheduled slot after you add them (or re-run with --instructions).\n");
        }
        if let Err(e) = out::stdout(&o) {
            return out::after_stdout_error(&e);
        }
        self.warn_if_transport_down();
        Exit::Ok
    }

    /// `hb_warn_if_transport_down`: never fails the command, its send's
    /// stderr is dropped.
    fn warn_if_transport_down(&mut self) {
        let passport = self.client.passport().is_some();
        let body = match self.client.send(SCHEDULER, Call::Request(&[])) {
            Ok(r) => {
                self.closing.answer(&r, passport);
                piped_body(&r)
            }
            Err(Error::NoAnswer(_)) => super::end_in_pipe(),
            Err(_) => Vec::new(),
        };
        let lines = read::transport_warning(&text(&body));
        if !lines.is_empty() {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), lines.as_bytes());
        }
    }

    /// `cmd_heartbeat_edit` after its flags.
    fn edit(
        &mut self,
        name: &str,
        spec: Option<(String, String)>,
        instructions: Option<String>,
        tz: Option<String>,
    ) -> Exit {
        let mut params = name_param(name);
        let mut freq = String::new();
        if let Some((json, f)) = spec {
            params.push_str(&format!("&frequency={f}&spec={}", quote(&json)));
            freq = f;
        }
        if let Some(t) = &instructions {
            params.push_str(&format!("&instructions={}", quote(t)));
        }
        if let Some(z) = &tz {
            params.push_str(&format!("&tz={}", quote(z)));
        }
        let resp = match self.held(EDIT, true, &[params], false) {
            Ok(b) => text(&b),
            Err(exit) => return exit,
        };
        let e = Edited {
            name,
            freq: &freq,
            instructions: instructions.is_some(),
            tz: tz.is_some(),
        };
        let first = read::edit(&resp, &e).print();
        if first != Exit::Ok {
            return first;
        }
        if tz.is_some() && read::tz_report(&resp, true).print() != Exit::Ok {
            return Exit::Error;
        }
        Exit::Ok
    }

    /// `cmd_heartbeat_wakeup` after its flags.
    fn wakeup(&mut self, name: &str, mode: Wake, pwd: &str) -> Exit {
        let p = self.get_piped(LIST, &[]);
        let rel = match read::wakeup_path(&text(&p.body), name) {
            Ok(path) if p.exit == Exit::Ok => path,
            Ok(_) => return p.exit,
            Err(shown) => return shown.print(),
        };
        let rel = rel.trim_end_matches('\n');
        if rel.is_empty() {
            out::stderr_line(&format!("Heartbeat '{name}' has no wakeup path on file."));
            return Exit::Error;
        }
        let base =
            k2_client::env_names::first_nonempty_in(k2_client::env_names::PROJECT_PATH, |n| {
                std::env::var(n).ok()
            })
            .map_or_else(|| pwd.to_string(), |(_, v)| v);
        let abs = format!("{base}/{rel}");
        let file = Path::new(&abs).is_file();
        match mode {
            Wake::Path => match out::stdout(&format!("{abs}\n")) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            },
            #[cfg(not(unix))]
            Wake::Edit => Exit::Error,
            #[cfg(unix)]
            Wake::Edit => {
                use std::os::unix::process::CommandExt;
                if !file {
                    let editor = std::env::var("EDITOR").unwrap_or_default();
                    if let Err(e) = out::stdout(&format!(
                        "Note: {abs} does not exist yet; {editor} will create it.\n"
                    )) {
                        return out::after_stdout_error(&e);
                    }
                }
                let editor = std::env::var_os("EDITOR").unwrap_or_default();
                let e = std::process::Command::new(editor).arg(&abs).exec();
                out::stderr_line(&format!("Error: {e}"));
                Exit::Curl(127)
            }
            Wake::Cat => {
                if !file {
                    out::stderr_line(&format!("(wakeup.md not present yet \u{2014} {abs})"));
                    return Exit::Ok;
                }
                let bytes = std::fs::read(&abs).unwrap_or_default();
                let mut o = format!("# {abs}\n\n").into_bytes();
                o.extend_from_slice(&bytes);
                match out::write_flushed(&mut std::io::stdout().lock(), &o) {
                    Ok(()) => Exit::Ok,
                    Err(e) => out::after_stdout_error(&e),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
