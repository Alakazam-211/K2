//! `k5 sources` (also `source`), as the shell CLI's `cmd_sources` answers
//! it (`cli/src/sources.sh`). Native so far: the help (the noun's page and
//! each verb's), `list` (`ls`), `show`, `read`, `history`, `status`,
//! `export`, `pause`, `resume`, `mute`, `unmute`, `remove` (`rm`),
//! `pending` and `access`, each with `--json`.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings, the virtual desktop's guard (not with `-h` or `--help`
//!    anywhere) and the connection gate (not for the help).
//! 2. `sources` is an `id` tool: with `HOOK_SOCK` set, the stale-passport
//!    or require-passport lines, exit 3, the help too.
//! 3. The verb's words: each problem is one line, exit 2. A value flag as
//!    the last word is the shell's `shift 2` failing under `set -e`: exit
//!    1, nothing said (K2's D6 class; a bash bug, copied until bash is
//!    fixed).
//! 4. `_compute_send` ([`super::board::send`]): the cell socket with the
//!    passport when there is a live one, else TCP; `access` goes on TCP.
//! 5. Not 200: `_sources_fail` ([`fail`]): the refusal (`--json`: the
//!    body, laid out, on stdout), a line naming who can lift it, and the
//!    exit its code maps to (3, 4, else 1). No closing line.
//! 6. `--json` prints the body laid out as `json.dumps(indent=2)`; else the
//!    shell's Python lays it out ([`render`]).
//!
//! The shell CLI answers these itself ([`run`] returns `None`): the verbs
//! not listed above (`propose`, `add`, `edit`, `try`, `refresh`,
//! `simulate`, `approve`, `secret`, `pin`, `unpin`, `trigger`), a word
//! that is not UTF-8, `HOOK_SOCK` set with a `CODEX_` variable exported, a
//! port curl won't take, a `python3` the binary can't tell, a `PWD` bash
//! would tidy, and a duration bash's own arithmetic fails on (it names
//! the script's path: D6).
//!
//! Where the shell's Python raises, this prints a Python 3.12 traceback,
//! as [`super::sessions`] says; its frames may differ, its exception's
//! name is Python's.

use super::board::{send, send_on, Sent};
use super::{curl_port, get, py_d, py_get, py_iter, py_left, py_len, raised, s, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::offline::{self, Env};
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::{json as pyjson, ops};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Gate};
use crate::prompt;
use crate::python;
use crate::transport::encode::quote;
use crate::transport::Client;
use k2_client::brand::CLI_NAME as CLI;
use k2_client::brand::PRODUCT_NAME;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["sources", "source"];
/// The binary answers the verbs above ([`run`] hands the rest to the
/// shell CLI).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: the whole row is native.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy.
pub const TOOL: &str = "sources";

/// The verbs this answers; the rest go to the shell CLI.
const NATIVE: [&str; 17] = [
    "list", "ls", "show", "read", "history", "status", "export", "pause", "resume", "mute",
    "unmute", "remove", "rm", "pending", "access", "simulate", "refresh",
];

/// What `cmd_sources` does with its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// The noun's help, or a verb's page (`_sources_verb_help`).
    Help(String),
    /// A verb and the words after it.
    Verb(String, Vec<String>),
}

/// `cmd_sources`'s first look.
pub fn parse(words: &[&str]) -> Parsed {
    let sub = words.first().copied().unwrap_or("");
    if matches!(sub, "" | "help" | "--help" | "-h") {
        return Parsed::Help(String::new());
    }
    if words[1..].iter().any(|w| matches!(*w, "-h" | "--help")) {
        return Parsed::Help(sub.to_string());
    }
    Parsed::Verb(
        sub.to_string(),
        words[1..].iter().map(|w| w.to_string()).collect(),
    )
}

/// `_sources_verb_help "$sub" || _sources_usage`.
pub fn help_page(sub: &str) -> String {
    let page = |k: &str| crate::text::help(k).unwrap_or_default().to_string();
    let flags = |lead: String| lead + "\n" + &page("sources/_flags");
    match sub {
        "propose" => flags(format!(
            "{CLI} sources propose --from <file.json> | --name N --label L <fetch> [flags\u{2026}]  (agents: it waits for approval)"
        )),
        "add" => flags(format!(
            "{CLI} sources add --from <file.json> | --name N --label L <fetch> [flags\u{2026}]  (people only; active at once without a secret)"
        )),
        "edit" => flags(format!(
            "{CLI} sources edit <name> [flags\u{2026}]  (agents: only their own; a changed definition needs approving again)"
        )),
        "try" => flags(format!(
            "{CLI} sources try --from <file.json> | --url URL [flags\u{2026}] | --name <saved> [--targets N] [--tree-depth N] [--json]\n  Fetch once and show the shape (pointer \u{b7} type \u{b7} sample) and the card it would make. Saves nothing.\n  A fan-out source fetches every target (or the first N with --targets N)."
        )),
        "history" => page("sources/history"),
        "export" => page("sources/export"),
        "pending" => page("sources/pending"),
        "read" | "show" | "list" => page("sources/read"),
        _ => page("sources"),
    }
}

/// Why the words stop before any request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// One line on stderr, exit 2.
    Usage(String),
    /// `shift 2` with one word left: exit 1, nothing said.
    Shift,
    /// Bash's own error (its arithmetic): the shell CLI answers.
    Shell,
}

/// What a verb asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// A GET of `/cli/sources/<route>` with a query, laid out by `kind`
    /// (`export`: always as JSON).
    Get {
        route: &'static str,
        query: String,
        kind: &'static str,
    },
    /// A POST of `/cli/sources/<route>`, then `done` on stdout.
    Post {
        route: &'static str,
        body: String,
        done: String,
    },
    /// `remove` with no `--yes`: ask first.
    Remove { name: String, yes: bool },
    /// `pending --names`.
    Pending { names: String, json: bool },
    /// `simulate <name> <state> [--for]`.
    Simulate { body: String, json: bool },
    /// `refresh <name> [--wait]`.
    Refresh { name: String, wait: bool },
    /// `access on|off`.
    Access {
        on: bool,
        workspace: Option<String>,
        json: bool,
    },
}

/// `_sources_name_arg <verb>`: the first word that doesn't start `--`.
pub fn name_arg(verb: &str, words: &[String]) -> Result<String, Stop> {
    words
        .iter()
        .find(|w| !w.starts_with("--"))
        .cloned()
        .ok_or_else(|| Stop::Usage(format!("Error: usage: {CLI} sources {verb} <name>")))
}

/// A number bash's `$(( ))` reads as decimal: digits, no leading zero
/// (an octal number goes to the shell), and in range.
fn decimal(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if text.len() > 1 && text.starts_with('0') {
        return None;
    }
    if text.len() > 15 {
        return None;
    }
    text.parse().ok()
}

/// `_sources_secs "$v" "$flag"` where its arithmetic runs: the seconds,
/// as text (`*s` and a bare number are the text as typed).
pub fn secs(v: &str, flag: &str) -> Result<String, Stop> {
    if v.is_empty()
        || !v
            .bytes()
            .all(|b| b.is_ascii_digit() || b"smhd".contains(&b))
    {
        return Err(Stop::Usage(format!(
            "Error: {flag} takes a duration like 30s, 5m, 2h, 1d"
        )));
    }
    let (num, unit) = v.split_at(v.len() - 1);
    let mult = match unit {
        "d" => 86_400,
        "h" => 3_600,
        "m" => 60,
        "s" => {
            return if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
                Ok(num.to_string())
            } else {
                Err(Stop::Shell)
            }
        }
        _ => {
            return if v.bytes().all(|b| b.is_ascii_digit()) {
                Ok(v.to_string())
            } else {
                Err(Stop::Shell)
            }
        }
    };
    decimal(num)
        .map(|n| (n * mult).to_string())
        .ok_or(Stop::Shell)
}

/// `_sources_b2_secs "$v"` where its arithmetic runs: the seconds as
/// text.
pub fn b2_secs(v: &str) -> Result<String, Stop> {
    let all_digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let cut = v.char_indices().last().map_or(0, |(i, _)| i);
    let (num, unit) = v.split_at(cut);
    let mult = match unit {
        "d" => 86_400,
        "h" => 3_600,
        "m" => 60,
        // `echo "${v%s}"`: Python's int() takes digits only.
        "s" => {
            return if all_digits(num) {
                Ok(num.to_string())
            } else {
                Err(Stop::Shell)
            }
        }
        _ if all_digits(v) => return Ok(v.to_string()),
        _ => {
            return Err(Stop::Usage(format!(
                "usage: bad duration '{v}' (use 30s, 5m, 2h, 1d)"
            )))
        }
    };
    decimal(num)
        .map(|n| (n * mult).to_string())
        .ok_or(Stop::Shell)
}

/// `history`'s `--from` or `--to` as the query takes it: epoch seconds as
/// typed, or now less a duration.
pub fn history_time(v: &str, flag: &str, now: i64) -> Result<String, Stop> {
    if v.ends_with(['s', 'm', 'h', 'd']) {
        let n = secs(v, flag).map_err(|_| Stop::Shell)?;
        return decimal(&n)
            .map(|n| (now - n).to_string())
            .ok_or(Stop::Shell);
    }
    if !v.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Stop::Usage(format!(
            "Error: {flag} takes a duration (6h) or epoch seconds"
        )));
    }
    Ok(v.to_string())
}

/// `json.dumps({"name": name})`.
fn name_body(name: &str) -> String {
    dumps(&Value::Object(vec![(Text::from("name"), s(name))]))
}

fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).unwrap_or_default()
}

/// A verb's words, to what it asks. `now`: `date +%s`.
pub fn ask(sub: &str, words: &[String], now: i64) -> Result<Ask, Stop> {
    let get = |route, query: String, kind| Ok(Ask::Get { route, query, kind });
    let post = |route, name: &str, done: &str| {
        Ok(Ask::Post {
            route,
            body: name_body(name),
            done: format!("{name}: {done}"),
        })
    };
    match sub {
        "list" | "ls" => get("list", String::new(), "list"),
        "status" => get("status", String::new(), "status"),
        "show" | "read" | "export" => {
            let name = name_arg(sub, words)?;
            let route = match sub {
                "show" => "show",
                "read" => "read",
                _ => "export",
            };
            get(route, format!("name={}", quote(&name)), route)
        }
        "history" => {
            let (mut name, mut field) = (String::new(), String::new());
            let (mut from, mut to, mut item) = (String::new(), String::new(), String::new());
            let mut i = 0;
            while i < words.len() {
                let a = words[i].as_str();
                match a {
                    "--from" | "--to" | "--item" => {
                        let v = words.get(i + 1).ok_or(Stop::Shift)?.clone();
                        match a {
                            "--from" => from = v,
                            "--to" => to = v,
                            _ => item = v,
                        }
                        i += 1;
                    }
                    "--json" => {}
                    _ if a.starts_with("--") => {
                        return Err(Stop::Usage(format!("Error: unknown flag {a}")))
                    }
                    _ => {
                        if name.is_empty() {
                            name = a.to_string();
                        } else if field.is_empty() {
                            field = a.to_string();
                        }
                    }
                }
                i += 1;
            }
            if name.is_empty() || field.is_empty() {
                return Err(Stop::Usage(format!(
                    "Error: usage: {CLI} sources history <name> health|<trend|count|aggregate> [--item <key>] [--from 6h|epoch] [--to \u{2026}]"
                )));
            }
            let mut q = format!("name={}&field={}", quote(&name), quote(&field));
            if !item.is_empty() {
                q.push_str(&format!("&item={}", quote(&item)));
            }
            for (t, v) in [("from", &from), ("to", &to)] {
                if !v.is_empty() {
                    let at = history_time(v, &format!("--{t}"), now)?;
                    q.push_str(&format!("&{t}={at}"));
                }
            }
            get("history", q, "history")
        }
        "pause" => post("pause", &name_arg(sub, words)?, "paused"),
        "resume" => post("resume", &name_arg(sub, words)?, "active"),
        "unmute" => post("unmute", &name_arg(sub, words)?, "unmuted"),
        "mute" => {
            let (mut name, mut for_, mut reason) = (String::new(), String::new(), String::new());
            let mut i = 0;
            while i < words.len() {
                let a = words[i].as_str();
                match a {
                    "--for" | "--reason" => {
                        let v = words.get(i + 1).ok_or(Stop::Shift)?.clone();
                        if a == "--for" {
                            for_ = v;
                        } else {
                            reason = v;
                        }
                        i += 1;
                    }
                    "--json" => {}
                    _ if a.starts_with("--") => {
                        return Err(Stop::Usage(format!("Error: unknown flag {a}")))
                    }
                    _ => name = a.to_string(),
                }
                i += 1;
            }
            if name.is_empty() || for_.is_empty() {
                return Err(Stop::Usage(format!(
                    "Error: usage: {CLI} sources mute <name> --for 30m [--reason \"\u{2026}\"]"
                )));
            }
            let n = secs(&for_, "--for")?;
            // Python's int() of the digits.
            let n = n.trim_start_matches('0');
            let n = if n.is_empty() { "0" } else { n };
            let mut b = vec![
                (Text::from("name"), s(&name)),
                (Text::from("forSecs"), Value::Int(n.to_string())),
            ];
            if !reason.is_empty() {
                b.push((Text::from("reason"), s(&reason)));
            }
            Ok(Ask::Post {
                route: "mute",
                body: dumps(&Value::Object(b)),
                done: format!("{name}: muted for {for_}"),
            })
        }
        "remove" | "rm" => Ok(Ask::Remove {
            name: name_arg("remove", words)?,
            yes: words.iter().any(|w| w == "--yes"),
        }),
        "simulate" => {
            let usage = || {
                Stop::Usage(format!(
                    "usage: {CLI} sources simulate <name> ok|warn|crit|stale|unknown|slow|off [--for 1h]"
                ))
            };
            let name = words.first().cloned().unwrap_or_default();
            let state = words.get(1).cloned().unwrap_or_default();
            if name.is_empty() || name.starts_with('-') {
                return Err(usage());
            }
            if !matches!(
                state.as_str(),
                "ok" | "warn" | "crit" | "stale" | "unknown" | "slow" | "off"
            ) {
                return Err(usage());
            }
            let (mut for_secs, mut json) = (String::new(), false);
            let rest = &words[2..];
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--for" => {
                        let v = rest
                            .get(i + 1)
                            .filter(|v| !v.is_empty())
                            .ok_or_else(|| Stop::Usage("usage: --for 1h".into()))?;
                        for_secs = b2_secs(v)?;
                        i += 1;
                    }
                    "--json" => json = true,
                    a => return Err(Stop::Usage(format!("Error: unknown flag '{a}'"))),
                }
                i += 1;
            }
            let mut b = vec![
                (Text::from("name"), s(&name)),
                (Text::from("state"), s(&state)),
            ];
            if !for_secs.is_empty() {
                let n = for_secs.trim_start_matches('0');
                let n = if n.is_empty() { "0" } else { n };
                b.push((Text::from("forSecs"), Value::Int(n.to_string())));
            }
            Ok(Ask::Simulate {
                body: dumps(&Value::Object(b)),
                json,
            })
        }
        "refresh" => Ok(Ask::Refresh {
            name: name_arg(sub, words)?,
            wait: words.iter().any(|w| w == "--wait"),
        }),
        "pending" => {
            let (mut names, mut json) = (String::new(), false);
            let mut i = 0;
            while i < words.len() {
                match words[i].as_str() {
                    "--names" => {
                        names = words.get(i + 1).ok_or(Stop::Shift)?.clone();
                        i += 1;
                    }
                    "--json" => json = true,
                    a => return Err(Stop::Usage(format!("Error: unknown flag '{a}'"))),
                }
                i += 1;
            }
            Ok(Ask::Pending { names, json })
        }
        "access" => {
            let usage = format!("usage: {CLI} sources access on|off [--workspace <ws>] [--json]");
            let (mut state, mut workspace, mut json) = (String::new(), None, false);
            let mut i = 0;
            while i < words.len() {
                match words[i].as_str() {
                    a @ ("on" | "off") => {
                        if !state.is_empty() {
                            return Err(Stop::Usage(format!(
                                "usage: {CLI} sources access on|off [--workspace <ws>]"
                            )));
                        }
                        state = a.to_string();
                    }
                    "--workspace" => {
                        let v = words.get(i + 1).filter(|v| !v.is_empty()).ok_or_else(|| {
                            Stop::Usage("usage: --workspace <workspace name or path>".into())
                        })?;
                        workspace = Some(v.clone());
                        i += 1;
                    }
                    "--json" => json = true,
                    _ => return Err(Stop::Usage(usage)),
                }
                i += 1;
            }
            if state.is_empty() {
                return Err(Stop::Usage(usage));
            }
            Ok(Ask::Access {
                on: state == "on",
                workspace,
                json,
            })
        }
        _ => Err(Stop::Shell),
    }
}

/// Whether a command line skips the virtual desktop's guard.
fn offline_verb(words: &[&str]) -> bool {
    words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

/// `date +%s`.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `k5 sources ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let all: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let parsed = parse(all.get(1..).unwrap_or_default());
    let json_any = all.iter().skip(2).any(|w| *w == "--json");
    let asked = match &parsed {
        Parsed::Verb(sub, rest) => {
            if !NATIVE.contains(&sub.as_str()) {
                return None;
            }
            let a = ask(sub, rest, now());
            if a == Err(Stop::Shell) {
                return None;
            }
            Some(a)
        }
        Parsed::Help(_) => None,
    };
    if let Some(Ok(Ask::Access {
        workspace: None, ..
    })) = &asked
    {
        offline::shell_pwd(&Env::current()).ok()?;
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
    let help = matches!(parsed, Parsed::Help(_));
    let gate = prelude::gate(&settings);
    if !help && gate.is_none() && settings.port_text.as_deref().and_then(curl_port).is_none() {
        return None;
    }
    Some(run_with(
        args,
        &parsed,
        asked,
        json_any,
        offline_verb(&all),
        hook_sock.as_deref(),
        settings,
    ))
}

fn run_with(
    args: &[OsString],
    parsed: &Parsed,
    asked: Option<Result<Ask, Stop>>,
    json_any: bool,
    offline: bool,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    let help = matches!(parsed, Parsed::Help(_));
    match prelude::gate(&settings) {
        Some(Gate::VirtualDesktop) if !offline => return Gate::VirtualDesktop.stop(),
        Some(_) if !help => return Gate::CannotConnect.stop(),
        _ => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let ask = match (parsed, asked) {
        (Parsed::Help(sub), _) => return Shown::out(help_page(sub)).print(),
        (_, Some(Ok(a))) => a,
        (_, Some(Err(Stop::Usage(line)))) => {
            out::stderr_line(&line);
            return Exit::Usage;
        }
        (_, _) => return Exit::Error,
    };
    if let Ask::Remove { name, yes: false } = &ask {
        if in_session() || !prompt::stdin_is_tty() {
            out::stderr_line(&format!("Error: {CLI} sources remove needs --yes here"));
            return Exit::Usage;
        }
        prompt::prompt(&format!(
            "Remove {name}? Gardens on other computers that show it will say removed; the name is held for 7 days. [y/N] "
        ));
        // `read -r ans`: the end of the input with no newline ends the
        // command (`set -e`): exit 1, nothing said.
        let Some(answer) = prompt::read_line_or_end(prompt::Echo::Shown) else {
            return Exit::Error;
        };
        let text = String::from_utf8_lossy(answer.expose()).into_owned();
        if !matches!(text.trim_matches([' ', '\t', '\n']), "y" | "Y" | "yes") {
            out::stderr_line("Not removed.");
            return Exit::Error;
        }
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    match go(&mut client, ask, json_any) {
        Ok(e) | Err(e) => e,
    }
}

/// `${K5_HOOK_SOCK:-}${K2_HOOK_SOCK:-}${K2SO_HOOK_SOCK:-}` is not empty.
fn in_session() -> bool {
    k2_client::env_names::first_nonempty_os_in(k2_client::env_names::HOOK_SOCK, |n| {
        std::env::var_os(n)
    })
    .is_some()
}

/// `_sources_call`: the answer when it is a 200; else `_sources_fail`'s
/// output and exit.
fn call(c: &mut Client, post: bool, path: &str, payload: &str, json: bool) -> Result<Sent, Exit> {
    let sent = send(c, post, path, payload)?;
    if sent.status != "200" {
        return Err(fail(&sent.status, &sent.resp, json).print());
    }
    Ok(sent)
}

fn go(c: &mut Client, ask: Ask, json_any: bool) -> Result<Exit, Exit> {
    match ask {
        Ask::Get { route, query, kind } => {
            let sent = call(c, false, &format!("/cli/sources/{route}"), &query, json_any)?;
            if json_any || kind == "export" {
                return Ok(Shown::out(print_json(&sent.resp)).print());
            }
            Ok(render(kind, &sent.resp).print())
        }
        Ask::Post { route, body, done } => {
            let sent = call(c, true, &format!("/cli/sources/{route}"), &body, json_any)?;
            if json_any {
                return Ok(Shown::out(print_json(&sent.resp)).print());
            }
            Ok(Shown::out(done + "\n").print())
        }
        Ask::Remove { name, .. } => {
            let sent = call(c, true, "/cli/sources/remove", &name_body(&name), json_any)?;
            if json_any {
                return Ok(Shown::out(print_json(&sent.resp)).print());
            }
            Ok(Shown::out(format!("Removed {name} (name held for 7 days).\n")).print())
        }
        Ask::Pending { names, json } => {
            let q = if names.is_empty() {
                String::new()
            } else {
                format!("names={}", quote(&names).replace("%20", "+"))
            };
            let sent = call(c, false, "/cli/sources/pending", &q, json_any)?;
            if json {
                return Ok(Shown::out(print_json(&sent.resp)).print());
            }
            Ok(lay_out(&sent.resp, pending_lines).print())
        }
        Ask::Simulate { body, json } => {
            let sent = call(c, true, "/cli/sources/simulate", &body, json_any)?;
            if json {
                return Ok(Shown::out(print_json(&sent.resp)).print());
            }
            Ok(lay_out(&sent.resp, simulate_lines).print())
        }
        Ask::Refresh { name, wait } => {
            let q = format!("name={}", quote(&name));
            let checked = |sent: &Sent| -> Result<String, Exit> {
                let v = load(&sent.resp, 1)
                    .map_err(|r| raised(String::new(), String::new(), &r).print())?;
                let at = py_get(&v, "checkedAt", 1)
                    .map_err(|r| raised(String::new(), String::new(), &r).print())?;
                Ok(ops::s_of(at))
            };
            let before = if wait {
                let sent = call(c, false, "/cli/sources/read", &q, json_any)?;
                Some(checked(&sent)?)
            } else {
                None
            };
            let sent = call(c, true, "/cli/sources/refresh", &name_body(&name), json_any)?;
            let Some(before) = before else {
                if json_any {
                    return Ok(Shown::out(print_json(&sent.resp)).print());
                }
                return Ok(Shown::out(format!("{name}: check queued\n")).print());
            };
            for _ in 0..30 {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let sent = call(c, false, "/cli/sources/read", &q, json_any)?;
                if checked(&sent)? != before {
                    if json_any {
                        return Ok(Shown::out(print_json(&sent.resp)).print());
                    }
                    return Ok(render("read", &sent.resp).print());
                }
            }
            out::stderr_line("Error: no new check within 30 s");
            Err(Exit::Error)
        }
        Ask::Access {
            on,
            workspace,
            json,
        } => {
            let workspace = match workspace {
                Some(w) => w,
                None => offline::shell_pwd(&Env::current())
                    .map_or_else(|_| String::new(), |p| p.to_string_lossy().into_owned()),
            };
            let body = dumps(&Value::Object(vec![
                (Text::from("workspace"), s(&workspace)),
                (Text::from("toggle"), s("gardens_manage")),
                (Text::from("value"), Value::Bool(on)),
            ]));
            let sent = send_on(c, true, "/cli/agent-access/set", &body, true)?;
            if sent.status != "200" {
                if sent.status == "403" {
                    out::stderr_line(&format!("Error: forbidden: only the owner or an admin turns this on or off (outside {PRODUCT_NAME}, or Settings \u{2192} Workspaces / Agents \u{2192} <ws> \u{2192} Permissions). An agent never can."));
                    return Err(Exit::Refused);
                }
                return Err(fail(&sent.status, &sent.resp, json_any).print());
            }
            if json {
                return Ok(Shown::out(print_json(&sent.resp)).print());
            }
            Ok(lay_out(&sent.resp, access_lines).print())
        }
    }
}

/// `_sources_print_json`: the body as `json.dumps(indent=2,
/// ensure_ascii=False)` when it reads as JSON, else as it came; a newline
/// after either.
pub fn print_json(body: &[u8]) -> String {
    let raw = String::from_utf8_lossy(body);
    let pretty = pyjson::loads(&raw).ok().and_then(|v| {
        json::to_string(
            &v,
            &Options {
                indent: Some(2),
                ensure_ascii: false,
                ..Options::default()
            },
        )
        .ok()
    });
    pretty.unwrap_or_else(|| raw.into_owned()) + "\n"
}

/// The codes `_sources_fail` maps to 3.
const PERSON: [&str; 8] = [
    "person_only",
    "owner_only",
    "vault_confirm_required",
    "confirm_required",
    "lan_owner_only",
    "field_person_only",
    "secret_not_usable",
    "forbidden",
];

/// `_sources_fail <status> <body>`.
pub fn fail(status: &str, body: &[u8], json_flag: bool) -> Shown {
    let raw = String::from_utf8_lossy(body).into_owned();
    let parsed = pyjson::loads(&raw).ok();
    let error = match &parsed {
        Some(Value::Object(pairs)) => get(pairs, "error").cloned(),
        _ => None,
    };
    let code = match &error {
        Some(Value::Object(e)) => get(e, "code").map_or_else(String::new, py::str_of),
        Some(e) if py::truthy(e) => py::str_of(e),
        _ => String::new(),
    };
    let daemon_hint = match &error {
        Some(Value::Object(e)) => get(e, "hint")
            .filter(|h| py::truthy(h))
            .is_some_and(|h| !py::str_of(h).trim_matches(super::py_space).is_empty()),
        _ => false,
    };
    let mut shown = Shown::err(String::new(), Exit::Error);
    if json_flag && !raw.is_empty() {
        shown.stdout = print_json(body);
    } else if !raw.is_empty() {
        shown.stderr = match (&parsed, &error) {
            (Some(Value::Object(_)), Some(Value::Object(e))) => format!(
                "Error: {}: {}\n",
                get(e, "code").map_or_else(|| "error".into(), py::str_of),
                get(e, "hint").map_or_else(String::new, py::str_of)
            ),
            (Some(Value::Object(_)), e) => format!(
                "Error: {}\n",
                e.as_ref().map_or_else(|| "None".into(), py::str_of)
            ),
            _ => format!(
                "Error: daemon returned: {}\n",
                raw.chars().take(200).collect::<String>()
            ),
        };
    } else {
        shown.stderr = format!(
            "Error: no answer from the {PRODUCT_NAME} daemon (status {})\n",
            if status.is_empty() { "none" } else { status }
        );
    }
    if PERSON.contains(&code.as_str()) {
        let person_code = matches!(
            code.as_str(),
            "person_only" | "vault_confirm_required" | "confirm_required" | "field_person_only"
        );
        if code != "owner_only" && (!daemon_hint || person_code) {
            if code == "secret_not_usable" {
                shown.stderr.push_str("Bind a secret from your own workspace, or ask your human for \"Let this agent manage server-wide secrets\".\n");
            } else {
                shown.stderr.push_str(&format!(
                    "This needs a person (run it outside {PRODUCT_NAME}, or Settings \u{2192} Gardens \u{2192} a Garden \u{2192} External Data), or the agent switch \"Let this agent manage Gardens and data sources\" ({CLI} sources --help).\n"
                ));
            }
        }
        shown.exit = Exit::Refused;
    } else if matches!(
        code.as_str(),
        "source_not_found" | "trigger_not_found" | "removed"
    ) {
        shown.exit = Exit::NotFound;
    }
    shown
}

/// `json.load(sys.stdin)` at `line`.
fn load(body: &[u8], line: u32) -> Result<Value, Raise> {
    let raw = String::from_utf8_lossy(body);
    pyjson::loads(&raw).map_err(|f| Raise::at(line, f.last_line()))
}

/// A reader on the body: what it printed, and its traceback if it raised.
fn lay_out(body: &[u8], lines: fn(&Value, &mut String) -> Result<(), Raise>) -> Shown {
    let mut o = String::new();
    match load(body, 3).and_then(|v| lines(&v, &mut o)) {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

/// `_sources_render <kind>`.
pub fn render(kind: &str, body: &[u8]) -> Shown {
    let mut o = String::new();
    let r = load(body, 4).and_then(|d| match kind {
        "list" => list_lines(&d, &mut o),
        "read" => card(&d, &mut o),
        "show" => show_lines(&d, &mut o),
        "status" => status_lines(&d, &mut o),
        _ => history_lines(&d, &mut o),
    });
    match r {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

fn push_line(o: &mut String, text: &str) {
    o.push_str(text);
    o.push('\n');
}

/// The renderer's line numbers.
pub mod line {
    pub const AGO: u32 = 9;
    pub const S_COUNT: u32 = 23;
    pub const S_LEVEL: u32 = 25;
    pub const S_SOFT: u32 = 26;
    pub const S_RETRY: u32 = 27;
    pub const LINE_C: u32 = 30;
    pub const LINE_COUNTS: u32 = 31;
    pub const LINE_BITS: u32 = 32;
    pub const LINE_AGO: u32 = 35;
    pub const LINE_REASON: u32 = 36;
    pub const LINE_SOFT: u32 = 38;
    pub const LINE_MUTED: u32 = 40;
    pub const LINE_SIM: u32 = 42;
    pub const LINE_JOIN: u32 = 46;
    pub const CARD_HEAD: u32 = 49;
    pub const CARD_LINE: u32 = 50;
    pub const CARD_REASON: u32 = 51;
    pub const CARD_GAP: u32 = 55;
    pub const CARD_FIELDS: u32 = 56;
    pub const CARD_ITEMS: u32 = 58;
    pub const CARD_F: u32 = 59;
    pub const CARD_R: u32 = 60;
    pub const CARD_SH: u32 = 61;
    pub const CARD_ITEM: u32 = 62;
    pub const LIST_ROWS: u32 = 67;
    pub const LIST_FOR: u32 = 70;
    pub const LIST_VIEW: u32 = 71;
    pub const LIST_STATUS: u32 = 73;
    pub const LIST_SECRET: u32 = 76;
    pub const LIST_TRIG: u32 = 78;
    pub const LIST_KEEP: u32 = 79;
    pub const LIST_ROW: u32 = 81;
    pub const SHOW_SOURCE: u32 = 85;
    pub const SHOW_CARD: u32 = 86;
    pub const SHOW_FETCH: u32 = 87;
    pub const SHOW_URL: u32 = 88;
    pub const SHOW_URL_P: u32 = 89;
    pub const SHOW_TARGETS: u32 = 90;
    pub const SHOW_TARGETS_P: u32 = 91;
    pub const SHOW_SECRET_IN: u32 = 92;
    pub const SHOW_SECRET_P: u32 = 93;
    pub const SHOW_STATUS: u32 = 95;
    pub const SHOW_SECRET: u32 = 96;
    pub const SHOW_SECRET_LINE: u32 = 97;
    pub const SHOW_WARN: u32 = 98;
    pub const SHOW_WARN_LINE: u32 = 99;
    pub const ST_LINE: u32 = 102;
    pub const ST_PARKED: u32 = 104;
    pub const ST_PARK: u32 = 105;
    pub const ST_GAP: u32 = 106;
    pub const ST_GAP_LINE: u32 = 108;
    pub const ST_AIR: u32 = 109;
    pub const H_FIELD: u32 = 112;
    pub const H_EVS: u32 = 113;
    pub const H_FOR: u32 = 116;
    pub const H_WHO: u32 = 117;
    pub const H_WHY: u32 = 118;
    pub const H_LINE: u32 = 119;
    pub const H_POINTS: u32 = 121;
    pub const H_POINT: u32 = 122;
}

/// `d.get(key)` (`None` when missing) on a value that must be a dict.
fn at(v: &Value, key: &str, l: u32) -> Result<Value, Raise> {
    Ok(py_get(v, key, l)?.cloned().unwrap_or(Value::Null))
}

/// `d[key]`.
fn idx(v: &Value, key: &str, l: u32) -> Result<Value, Raise> {
    ops::index(v, key, l).cloned()
}

/// `a + b` for two strings, or Python's `TypeError`.
fn concat(a: &Value, b: &Value, l: u32) -> Result<String, Raise> {
    match (a, b) {
        (Value::Str(_), Value::Str(_)) => Ok(py::str_of(a) + &py::str_of(b)),
        (Value::Str(_), other) => Err(Raise::at(
            l,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
        (other, _) => Err(Raise::at(
            l,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and '{}'",
                py::type_name(other),
                py::type_name(b)
            ),
        )),
    }
}

/// A Python number from JSON.
#[derive(Debug, Clone, Copy)]
enum Num {
    I(i128),
    F(f64),
}

fn num(v: &Value) -> Option<Num> {
    match v {
        Value::Bool(b) => Some(Num::I(i128::from(*b))),
        Value::Int(d) => d.parse().ok().map(Num::I),
        Value::Float(f) => Some(Num::F(*f)),
        _ => None,
    }
}

fn num_value(n: Num) -> Value {
    match n {
        Num::I(i) => Value::Int(i.to_string()),
        Num::F(f) => Value::Float(f),
    }
}

/// `a - b`.
fn sub(a: &Value, b: &Value, l: u32) -> Result<Value, Raise> {
    let bad = || {
        Raise::at(
            l,
            format!(
                "TypeError: unsupported operand type(s) for -: '{}' and '{}'",
                py::type_name(a),
                py::type_name(b)
            ),
        )
    };
    Ok(match (num(a).ok_or_else(bad)?, num(b).ok_or_else(bad)?) {
        (Num::I(x), Num::I(y)) => num_value(Num::I(x - y)),
        (x, y) => {
            let f = |n: Num| match n {
                Num::I(i) => i as f64,
                Num::F(f) => f,
            };
            num_value(Num::F(f(x) - f(y)))
        }
    })
}

/// `x // n` for a number `x` that is at least 0.
fn floor_div(x: Num, n: i128) -> Num {
    match x {
        Num::I(i) => Num::I(i.div_euclid(n)),
        Num::F(f) => Num::F((f / n as f64).floor()),
    }
}

/// `ago(now, t)`.
fn ago(now: &Value, t: &Value) -> Result<String, Raise> {
    if *t == Value::Null {
        return Ok("never".into());
    }
    let d = sub(now, t, line::AGO)?;
    let d = match num(&d) {
        Some(Num::I(i)) => Num::I(i.max(0)),
        Some(Num::F(f)) => Num::F(if f < 0.0 { 0.0 } else { f }),
        None => Num::I(0),
    };
    let lt = |n: i128| match d {
        Num::I(i) => i < n,
        Num::F(f) => f < n as f64,
    };
    let (n, unit) = if lt(90) {
        (d, "s ago")
    } else if lt(5400) {
        (floor_div(d, 60), "min ago")
    } else if lt(172_800) {
        (floor_div(d, 3600), "h ago")
    } else {
        (floor_div(d, 86_400), "d ago")
    };
    Ok(format!("{} {unit}", py_d(&num_value(n), line::AGO)?))
}

/// `DOT.get(x, "?")`.
fn dot(x: &Value, l: u32) -> Result<&'static str, Raise> {
    if matches!(x, Value::Array(_) | Value::Object(_)) {
        return Err(Raise::at(
            l,
            format!("TypeError: unhashable type: '{}'", py::type_name(x)),
        ));
    }
    let Value::Str(t) = x else {
        return Ok("?");
    };
    Ok(match t.as_str() {
        Some("ok") => "\u{25cf}",
        Some("warn") => "\u{25b2}",
        Some("crit") => "\u{2716}",
        Some("unknown") => "?",
        Some("pending") => "\u{25cb}",
        Some("paused") => "\u{2016}",
        Some("stale") => "\u{25cc}",
        _ => "?",
    })
}

/// `soft_hint(s)`.
fn soft_hint(sv: &Value) -> Result<String, Raise> {
    let empty = Value::Object(Vec::new());
    let sv = if py::truthy(sv) { sv } else { &empty };
    if !py_get(sv, "count", line::S_COUNT)?.is_some_and(py::truthy) {
        return Ok(String::new());
    }
    if py_get(sv, "level", line::S_LEVEL)?.is_some_and(py::truthy) {
        let l = line::S_SOFT;
        let level = py::str_of(&idx(sv, "level", l)?);
        let count = py_d(&idx(sv, "count", l)?, l)?;
        let of = py_d(&idx(sv, "of", l)?, l)?;
        return Ok(format!("soft {level} {count}/{of}"));
    }
    let l = line::S_RETRY;
    let count = py_d(&idx(sv, "count", l)?, l)?;
    let of = py_d(&idx(sv, "of", l)?, l)?;
    Ok(format!("retrying {count}/{of}"))
}

/// `line(v)`.
fn view_line(v: &Value) -> Result<String, Raise> {
    let empty = Value::Object(Vec::new());
    let c = at(v, "counts", line::LINE_C)?;
    let c = if py::truthy(&c) { c } else { empty.clone() };
    let mut counts = Vec::new();
    for k in ["ok", "warn", "crit", "unknown"] {
        if py_get(&c, k, line::LINE_COUNTS)?.is_some_and(py::truthy) {
            let n = py_d(&idx(&c, k, line::LINE_COUNTS)?, line::LINE_COUNTS)?;
            counts.push(format!("{n} {k}"));
        }
    }
    let mut bits = vec![py_get(v, "display", line::LINE_BITS)?
        .cloned()
        .unwrap_or_else(|| s("?"))];
    if !counts.is_empty() {
        bits.push(s(&counts.join(", ")));
    }
    let server_now = idx(v, "serverNow", line::LINE_AGO)?;
    let updated = at(v, "updatedAt", line::LINE_AGO)?;
    bits.push(s(&format!("updated {}", ago(&server_now, &updated)?)));
    if py_get(v, "reason", line::LINE_REASON)?.is_some_and(py::truthy) {
        bits.push(idx(v, "reason", line::LINE_REASON + 1)?);
    }
    let soft = soft_hint(&at(v, "soft", line::LINE_SOFT)?)?;
    if !soft.is_empty() {
        bits.push(s(&soft));
    }
    if py_get(v, "muted", line::LINE_MUTED)?.is_some_and(py::truthy) {
        let by = idx(
            &idx(v, "muted", line::LINE_MUTED + 1)?,
            "by",
            line::LINE_MUTED + 1,
        )?;
        bits.push(s(&format!("muted by {}", py::str_of(&by))));
    }
    if py_get(v, "simulated", line::LINE_SIM)?.is_some_and(py::truthy) {
        let state = idx(
            &idx(v, "simulated", line::LINE_SIM + 1)?,
            "state",
            line::LINE_SIM + 1,
        )?;
        bits.push(s(&concat(&s("TEST "), &state, line::LINE_SIM + 1)?));
    }
    if py_get(v, "pinned", line::LINE_SIM + 2)?.is_some_and(py::truthy) {
        bits.push(s("pinned"));
    }
    ops::join(&Value::Array(bits), " \u{b7} ", line::LINE_JOIN)
}

/// `json.dumps(x)`.
fn json_dumps(x: &Value) -> String {
    dumps(x)
}

/// `sorted(d.items())` of a value that must be a dict (`x or {}` first).
fn items(v: &Value, l: u32) -> Result<Vec<(Text, Value)>, Raise> {
    match v {
        Value::Object(pairs) => {
            let mut p = pairs.clone();
            p.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(p)
        }
        v if !py::truthy(v) => Ok(Vec::new()),
        other => Err(Raise::no_attribute(l, other, "items")),
    }
}

/// `card(v)`.
fn card(v: &Value, o: &mut String) -> Result<(), Raise> {
    let d = dot(&at(v, "display", line::CARD_HEAD)?, line::CARD_HEAD)?;
    let name = idx(v, "name", line::CARD_HEAD)?;
    let label = idx(v, "label", line::CARD_HEAD)?;
    push_line(
        o,
        &format!("{d} {} ({})", py::str_of(&name), py::str_of(&label)),
    );
    let l = view_line(v)?;
    push_line(o, &format!("  {l}"));
    if py_get(v, "reasonText", line::CARD_REASON)?.is_some_and(py::truthy) {
        let t = idx(v, "reasonText", line::CARD_REASON + 1)?;
        push_line(o, &concat(&s("  "), &t, line::CARD_REASON + 1)?);
    }
    if py_get(v, "gap", line::CARD_GAP - 2)?.is_some_and(py::truthy) {
        let g = idx(v, "gap", line::CARD_GAP - 1)?;
        let kind = idx(&g, "kind", line::CARD_GAP)?;
        let span = sub(
            &idx(&g, "to", line::CARD_GAP)?,
            &idx(&g, "from", line::CARD_GAP)?,
            line::CARD_GAP,
        )?;
        push_line(
            o,
            &format!("  gap: {} ({} s)", py::str_of(&kind), py::str_of(&span)),
        );
    }
    for (k, x) in items(&at(v, "fields", line::CARD_FIELDS)?, line::CARD_FIELDS)? {
        push_line(
            o,
            &format!("  {} {}", py_left(&Value::Str(k), 18), json_dumps(&x)),
        );
    }
    let its = at(v, "items", line::CARD_ITEMS)?;
    let its = if py::truthy(&its) {
        its
    } else {
        Value::Array(Vec::new())
    };
    for it in py_iter(&its, line::CARD_ITEMS)? {
        let fields = items(&at(&it, "fields", line::CARD_F)?, line::CARD_F)?;
        let f = fields
            .into_iter()
            .map(|(k, x)| format!("{}={}", py::str_of(&Value::Str(k)), json_dumps(&x)))
            .collect::<Vec<_>>()
            .join(" ");
        let r = if py_get(&it, "reason", line::CARD_R)?.is_some_and(py::truthy) {
            let reason = idx(&it, "reason", line::CARD_R)?;
            concat(&s(" ("), &reason, line::CARD_R)? + ")"
        } else {
            String::new()
        };
        let soft = soft_hint(&at(&it, "soft", line::CARD_SH)?)?;
        let sh = if soft.is_empty() {
            String::new()
        } else {
            format!(" [{soft}]")
        };
        let health = idx(&it, "health", line::CARD_ITEM)?;
        let d = dot(&health, line::CARD_ITEM)?;
        let key = idx(&it, "key", line::CARD_ITEM)?;
        push_line(
            o,
            &format!(
                "  {d} {} {}{r}{sh} {f}",
                py_left(&key, 24),
                py::str_of(&health)
            ),
        );
    }
    if py_get(v, "masked", line::CARD_ITEM + 1)?.is_some_and(py::truthy) {
        push_line(o, "  (values hidden from agents on this source)");
    }
    Ok(())
}

fn list_lines(d: &Value, o: &mut String) -> Result<(), Raise> {
    let rows = at(d, "sources", line::LIST_ROWS)?;
    let rows = if py::truthy(&rows) {
        rows
    } else {
        Value::Array(Vec::new())
    };
    if !py::truthy(&rows) {
        push_line(
            o,
            &format!("No data sources on this server. Propose one: {CLI} sources propose --help"),
        );
    }
    for r in py_iter(&rows, line::LIST_FOR)? {
        let v = idx(&r, "view", line::LIST_VIEW)?;
        let mut extra: Vec<Value> = Vec::new();
        let status = idx(&r, "status", line::LIST_STATUS)?;
        if !ops::py_eq(&status, &s("active")) {
            extra.push(status);
        }
        if py_get(&r, "secret", line::LIST_SECRET - 1)?.is_some_and(py::truthy) {
            let sec = idx(&r, "secret", line::LIST_SECRET)?;
            extra.push(s(&concat(&s("secret "), &sec, line::LIST_SECRET)?));
        }
        if py_get(&r, "triggers", line::LIST_TRIG - 1)?.is_some_and(py::truthy) {
            let t = idx(&r, "triggers", line::LIST_TRIG)?;
            let one = ops::py_eq(&t, &Value::Int("1".into()));
            extra.push(s(&format!(
                "{} trigger{}",
                py_d(&t, line::LIST_TRIG)?,
                if one { "" } else { "s" }
            )));
        }
        if ops::py_eq(&at(&r, "keep", line::LIST_KEEP)?, &s("states")) {
            extra.push(s("states only"));
        }
        let d = dot(&at(&v, "display", line::LIST_ROW)?, line::LIST_ROW)?;
        let name = idx(&r, "name", line::LIST_ROW)?;
        let l = view_line(&v)?;
        let tail = if extra.is_empty() {
            String::new()
        } else {
            format!(
                " \u{b7} {}",
                ops::join(&Value::Array(extra), " \u{b7} ", line::LIST_ROW)?
            )
        };
        push_line(o, &format!("{d} {} {l}{tail}", py_left(&name, 24)));
    }
    Ok(())
}

fn show_lines(d: &Value, o: &mut String) -> Result<(), Raise> {
    let src = idx(d, "source", line::SHOW_SOURCE)?;
    card(&idx(d, "view", line::SHOW_CARD)?, o)?;
    let f = at(&src, "fetch", line::SHOW_FETCH)?;
    let f = if py::truthy(&f) {
        f
    } else {
        Value::Object(Vec::new())
    };
    if ops::contains(&f, "url", line::SHOW_URL)? {
        let u = idx(&f, "url", line::SHOW_URL_P)?;
        push_line(o, &format!("  fetch: {}", py::str_of(&u)));
    } else if ops::contains(&f, "targets", line::SHOW_TARGETS)? {
        let t = idx(&f, "targets", line::SHOW_TARGETS_P)?;
        push_line(
            o,
            &format!("  fetch: {} targets", py_len(&t, line::SHOW_TARGETS_P)?),
        );
    } else if ops::contains(&f, "urlSecret", line::SHOW_SECRET_IN)? {
        let u = idx(&f, "urlSecret", line::SHOW_SECRET_P)?;
        let name = idx(&u, "name", line::SHOW_SECRET_P)?;
        let origin = idx(&u, "origin", line::SHOW_SECRET_P)?;
        push_line(
            o,
            &format!(
                "  fetch: URL in the vault ({}) at {}",
                py::str_of(&name),
                py::str_of(&origin)
            ),
        );
    }
    let l = line::SHOW_STATUS;
    let status = idx(&src, "status", l)?;
    let every = idx(&src, "everySecs", l)?;
    let made = idx(&src, "madeBy", l)?;
    let viewers = at(&src, "viewers", l)?;
    let access = at(&src, "agentAccess", l)?;
    let keep = at(&src, "keep", l)?;
    push_line(
        o,
        &format!(
            "  status: {} \u{b7} every {} s \u{b7} made by {} \u{b7} viewers {} \u{b7} agents see {} \u{b7} keep {}",
            py::str_of(&status),
            py::str_of(&every),
            py::str_of(&made),
            py::str_of(&viewers),
            py::str_of(&access),
            py::str_of(&keep)
        ),
    );
    if py_get(&src, "secret", line::SHOW_SECRET)?.is_some_and(py::truthy) {
        let l = line::SHOW_SECRET_LINE;
        let sec = idx(&src, "secret", l)?;
        let bound = if py_get(&src, "secretBound", l)?.is_some_and(py::truthy) {
            "bound"
        } else {
            "not bound"
        };
        push_line(o, &format!("  secret: {} ({bound})", py::str_of(&sec)));
    }
    let ws = at(d, "warnings", line::SHOW_WARN)?;
    let ws = if py::truthy(&ws) {
        ws
    } else {
        Value::Array(Vec::new())
    };
    for w in py_iter(&ws, line::SHOW_WARN)? {
        let hint = idx(&w, "hint", line::SHOW_WARN_LINE)?;
        push_line(o, &format!("  warning: {}", py::str_of(&hint)));
    }
    Ok(())
}

fn status_lines(d: &Value, o: &mut String) -> Result<(), Raise> {
    let l = line::ST_LINE;
    let running = if py_get(d, "running", l)?.is_some_and(py::truthy) {
        "running"
    } else {
        "not running"
    };
    let sources = idx(d, "sources", l)?;
    let active = idx(d, "active", l)?;
    let targets = idx(d, "targets", l)?;
    let in_flight = idx(d, "inFlight", l)?;
    let on_time = if !py_get(d, "behindSecs", l)?.is_some_and(py::truthy) {
        "on time".to_string()
    } else {
        format!("behind by {} s", py_d(&idx(d, "behindSecs", l)?, l)?)
    };
    push_line(
        o,
        &format!(
            "Poller: {running} \u{b7} {} sources ({} active) \u{b7} {} targets \u{b7} {} in flight \u{b7} {on_time}",
            py_d(&sources, l)?,
            py_d(&active, l)?,
            py_d(&targets, l)?,
            py_d(&in_flight, l)?
        ),
    );
    let parked = at(d, "parkedOrigins", line::ST_PARKED)?;
    let parked = if py::truthy(&parked) {
        parked
    } else {
        Value::Array(Vec::new())
    };
    for p in py_iter(&parked, line::ST_PARKED)? {
        let l = line::ST_PARK;
        let origin = idx(&p, "origin", l)?;
        let until = idx(&p, "untilAt", l)?;
        let reason = idx(&p, "reason", l)?;
        push_line(
            o,
            &format!(
                "  parked: {} until {} ({})",
                py::str_of(&origin),
                py::str_of(&until),
                py::str_of(&reason)
            ),
        );
    }
    if py_get(d, "lastGap", line::ST_GAP)?.is_some_and(py::truthy) {
        let g = idx(d, "lastGap", line::ST_GAP + 1)?;
        let l = line::ST_GAP_LINE;
        let kind = idx(&g, "kind", l)?;
        let span = sub(&idx(&g, "to", l)?, &idx(&g, "from", l)?, l)?;
        push_line(
            o,
            &format!("  last gap: {} ({} s)", py::str_of(&kind), py_d(&span, l)?),
        );
    }
    if py_get(d, "airgap", line::ST_AIR)?.is_some_and(py::truthy) {
        push_line(o, "  air-gap is on: only exact-IP sources run");
    }
    Ok(())
}

fn history_lines(d: &Value, o: &mut String) -> Result<(), Raise> {
    if ops::py_eq(&at(d, "field", line::H_FIELD)?, &s("health")) {
        let evs = at(d, "events", line::H_EVS)?;
        if !py::truthy(&evs) {
            push_line(o, "No state changes in this window.");
        }
        let evs = if py::truthy(&evs) {
            evs
        } else {
            Value::Array(Vec::new())
        };
        for e in py_iter(&evs, line::H_FOR)? {
            let who = if py_get(&e, "item", line::H_WHO)?.is_some_and(py::truthy) {
                concat(&s(" "), &idx(&e, "item", line::H_WHO)?, line::H_WHO)?
            } else {
                String::new()
            };
            let why = if py_get(&e, "reason", line::H_WHY)?.is_some_and(py::truthy) {
                let r = concat(&s(" ("), &idx(&e, "reason", line::H_WHY)?, line::H_WHY)?;
                r + ")"
            } else {
                String::new()
            };
            let l = line::H_LINE;
            let t = idx(&e, "t", l)?;
            let kind = idx(&e, "kind", l)?;
            let dash = s("-");
            let from = ops::or(py_get(&e, "from", l)?, &dash).clone();
            let to = ops::or(py_get(&e, "to", l)?, &dash).clone();
            push_line(
                o,
                &format!(
                    "{}\t{}{who}\t{} \u{2192} {}{why}",
                    py_d(&t, l)?,
                    py::str_of(&kind),
                    py::str_of(&from),
                    py::str_of(&to)
                ),
            );
        }
    } else {
        let ps = at(d, "points", line::H_POINTS)?;
        let ps = if py::truthy(&ps) {
            ps
        } else {
            Value::Array(Vec::new())
        };
        for p in py_iter(&ps, line::H_POINTS)? {
            let l = line::H_POINT;
            let t = idx(&p, "t", l)?;
            let v = idx(&p, "v", l)?;
            let shown = if v == Value::Null {
                "gap".to_string()
            } else {
                py::str_of(&v)
            };
            push_line(o, &format!("{}\t{shown}", py_d(&t, l)?));
        }
    }
    Ok(())
}

fn pending_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let none = Value::Array(Vec::new());
    let src = ops::or(py_get(v, "sources", 4)?, &none).clone();
    let trg = ops::or(py_get(v, "triggers", 4)?, &none).clone();
    let own = ops::py_eq(&at(v, "scope", 5)?, &s("own"));
    if !py::truthy(&src) && !py::truthy(&trg) {
        push_line(
            o,
            if own {
                "None of your proposals is waiting."
            } else {
                "Nothing is waiting for approval."
            },
        );
        return Ok(());
    }
    if own {
        push_line(
            o,
            "Your proposals waiting for a person (or an agent with the switch):",
        );
    }
    for sv in py_iter(&src, 10)? {
        let empty = Value::Object(Vec::new());
        let m = ops::or(py_get(&sv, "maker", 11)?, &empty).clone();
        let by = {
            let h = at(&m, "handle", 12)?;
            if py::truthy(&h) {
                h
            } else {
                let u = at(&m, "user", 12)?;
                if py::truthy(&u) {
                    u
                } else {
                    at(&sv, "madeBy", 12)?
                }
            }
        };
        let sec = {
            let u = at(&sv, "urlSecret", 13)?;
            if py::truthy(&u) {
                u
            } else {
                at(&sv, "secret", 13)?
            }
        };
        let l = 14;
        let name = idx(&sv, "name", l)?;
        let approval = idx(&sv, "approval", l)?;
        let every = idx(&sv, "everySecs", l)?;
        let targets = idx(&sv, "targets", l)?;
        let origins = ops::join(ops::or(py_get(&sv, "origins", l)?, &none), ", ", l)?;
        let wants = if py::truthy(&sec) {
            concat(&s(" \u{b7} wants secret "), &sec, l)?
        } else {
            String::new()
        };
        push_line(
            o,
            &format!(
                "{} {} every {}s \u{b7} {} target(s) \u{b7} {origins} \u{b7} by {}{wants}",
                py_left(&name, 20),
                py_left(&approval, 11),
                py::str_of(&every),
                py::str_of(&targets),
                py::str_of(&by)
            ),
        );
    }
    for t in py_iter(&trg, 15)? {
        let on_v = idx(&t, "on", 16)?;
        let kind = idx(&on_v, "kind", 16)?;
        let tail = if py_get(&on_v, "secs", 16)?.is_some_and(py::truthy) {
            format!(":{}s", py::str_of(&idx(&on_v, "secs", 16)?))
        } else {
            String::new()
        };
        let on = concat(&kind, &s(&tail), 16)?;
        let l = 17;
        let id = idx(&t, "id", l)?;
        let id = super::sidecar::py_slice(&id, 8, l)?;
        let src_name = idx(&t, "source", l)?;
        let agent = idx(&t, "agent", l)?;
        let handle = at(&agent, "handle", l)?;
        let made = at(&t, "madeBy", l)?;
        push_line(
            o,
            &format!(
                "trigger {}  {} on {on} \u{2192} {} (by {})",
                py::str_of(&id),
                py::str_of(&src_name),
                py::str_of(&handle),
                py::str_of(&made)
            ),
        );
    }
    push_line(o, "");
    if own {
        push_line(o, &format!("A person approves them in Settings \u{2192} Gardens \u{2192} a Garden \u{2192} External Data (or {CLI} sources approve outside {PRODUCT_NAME})."));
    } else {
        push_line(
            o,
            &format!(
                "Approve: {CLI} sources approve <name>\u{2026} \u{b7} {CLI} sources trigger approve <id>"
            ),
        );
    }
    Ok(())
}

fn simulate_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let sim = at(v, "simulated", 3)?;
    let name = py::str_of(&at(v, "name", 5)?);
    if !py::truthy(&sim) {
        push_line(o, &format!("{name}: simulate off (real state shows again)"));
        return Ok(());
    }
    let until = utc_minutes(&idx(&sim, "until", 7)?).map_err(|e| Raise::at(7, e))?;
    let state = py::str_of(&idx(&sim, "state", 8)?);
    push_line(o, &format!("{name}: TEST {state} until {until}. Triggers never fire for it; nothing is written to history."));
    Ok(())
}

/// `datetime.datetime.fromtimestamp(t, timezone.utc).strftime("%H:%M UTC")`.
fn utc_minutes(t: &Value) -> Result<String, String> {
    let secs: i64 = match t {
        Value::Bool(b) => i64::from(*b),
        Value::Int(d) => d
            .parse()
            .map_err(|_| "OverflowError: timestamp out of range for platform time_t".to_string())?,
        Value::Float(f) if f.is_finite() => {
            // Python rounds to the microsecond first.
            let us = (f * 1e6).round_ties_even();
            (us / 1e6).floor() as i64
        }
        Value::Float(_) => return Err("ValueError: Invalid value NaN (not a number)".into()),
        other => {
            return Err(format!(
                "TypeError: '{}' object cannot be interpreted as an integer",
                py::type_name(other)
            ))
        }
    };
    // Years 1 to 9999 only.
    if !(-62_135_596_800..=253_402_300_799).contains(&secs) {
        return Err("ValueError: year is out of range".to_string());
    }
    let day = secs.rem_euclid(86_400);
    Ok(format!("{:02}:{:02} UTC", day / 3600, day % 3600 / 60))
}

fn access_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let on = py_get(v, "agentsCanManageGardens", 4)?.is_some_and(py::truthy);
    let ws = py::str_of(&at(v, "workspace", 5)?);
    if on {
        push_line(o, &format!("Manage Gardens and data sources is ON for {ws}: its agent approves sources, binds secrets it may use, pauses, pins, enables triggers and edits Gardens with no confirm (each act is logged)."));
    } else {
        push_line(o, &format!("Manage Gardens and data sources is OFF for {ws}: its agent proposes; a person approves."));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
