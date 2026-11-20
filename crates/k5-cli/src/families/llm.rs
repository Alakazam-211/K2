//! `k5 llm`, as the shell CLI's `cmd_llm` and `cmd_llm_accounts` answer
//! it (`cli/src/usage.sh`, `cli/src/llm.sh`): the LLM tokens. `llm
//! tokens` (also `token`, `accounts`, `account`) and every verb: `list`
//! (`ls`), `status`, `usage`, `add`, `login`, `switch` (`use`), `next`,
//! `rename`, `remove` (`rm`), `clear-default`, `refresh`, `storage`,
//! `auto-next` (`auto-rotate`), `pins`, `for`, `pin`, `unpin` and
//! `add-key`, each with `--json`, and the help.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), then the virtual desktop's guard
//!    and the connection gate ([`crate::prelude::gate`]); the help forms
//!    skip the gate ([`skips_gate`]), `-h` or `--help` anywhere the guard.
//! 2. `llm` is an `id` tool (`llm-accounts`), so with `HOOK_SOCK` set: the
//!    stale-passport or require-passport lines, exit 3, the help too.
//! 3. The words ([`parse`]): the help, a `usage:` line (exit 2), or a verb.
//! 4. Each request is a `_sidecar_send` (the cell socket with the passport,
//!    else TCP) of `/cli/llm/accounts/<route>`; a status that is not 200 is
//!    `_sidecar_fail` (its line, the refusal's closing line, its exit).
//!    `<tool> <label>` is found through `list` first (`_llma_resolve`).
//! 5. `--json` prints the body as it came; else the shell's Python lays it
//!    out ([`render`]). `add` and `login` follow the sign-in
//!    (`_llma_follow_login`, [`follow_login`]).
//!
//! Secrets: the API token of `add-key` and the code pasted for a sign-in
//! are read from stdin and go only into a request's body, as the shell's
//! do (stdin, never argv or the environment). The confirm token of
//! `auto-next on` stays in the process.
//!
//! The shell CLI answers ([`run`] returns `None`): a word that is not
//! UTF-8, `HOOK_SOCK` set with a `CODEX_` variable exported, a `python3`
//! the binary can't tell, and a form that asks the daemon with a port that
//! is not one curl takes.

pub mod render;

use super::sessions::{exit_for, fail_line};
use super::{curl_port, Shown};
use crate::confirm::{self, Confirm, Split};
use crate::daemon;
use crate::exit::Exit;
use crate::out;
use crate::out::json::{Text, Value};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::prompt;
use crate::python;
use crate::transport::encode::quote;
use crate::transport::Client;
use k2_client::brand::CLI_NAME as CLI;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["llm"];
/// The binary answers every form ([`run`] may still hand one to the
/// shell CLI).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: the whole row is native.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "llm-accounts";
/// `_LLMA_BASE`.
pub const BASE: &str = "/cli/llm/accounts/";
/// The nouns `cmd_llm` hands to `cmd_llm_accounts`.
pub const NOUNS: &[&str] = &["tokens", "token", "accounts", "account"];
/// The verbs `cmd_llm_accounts` knows, as the shell's unknown-verb line
/// lists them.
pub const VERB_LIST: &str = "list|status|usage|add|add-key|login|switch|next|rename|remove|refresh|pin|unpin|pins|for|auto-next";

/// A word `cmd_llm` doesn't have.
pub fn unknown_noun(sub: &str) -> String {
    format!("usage: unknown llm verb '{sub}' ({CLI} llm tokens --help)")
}

/// A verb `cmd_llm_accounts` doesn't have.
pub fn unknown_verb(sub: &str) -> String {
    format!("usage: unknown llm tokens verb '{sub}' ({VERB_LIST} \u{2014} {CLI} llm tokens --help)")
}

/// A `--` flag it doesn't have.
pub fn unknown_flag(flag: &str) -> String {
    format!("usage: unknown flag '{flag}' ({CLI} llm tokens --help)")
}

/// `usage: k2 llm tokens <rest>`.
pub fn usage(rest: &str) -> String {
    format!("usage: {CLI} llm tokens {rest}")
}

/// `_llma_tool_ok`: a tool that has subscriptions, or its usage line.
pub fn tool_ok(tool: &str) -> Result<(), String> {
    match tool {
        "claude" | "codex" | "grok" => Ok(()),
        "gemini" | "cursor" | "cursor-agent" | "pi" | "hermes" => Err(format!(
            "usage: {tool} has no subscriptions yet (claude, codex, grok)"
        )),
        "" => Err(format!(
            "usage: missing <tool> (claude, codex or grok) \u{2014} {CLI} llm tokens --help"
        )),
        _ => Err(format!(
            "usage: unknown tool '{tool}' (claude, codex or grok) \u{2014} {CLI} llm tokens --help"
        )),
    }
}

/// `_llma_key_tool_ok`: a tool that takes API tokens, or its usage line.
pub fn key_tool_ok(tool: &str) -> Result<(), String> {
    match tool {
        "claude" | "codex" | "grok" | "gemini" => Ok(()),
        "cursor" | "cursor-agent" | "pi" | "hermes" => Err(format!(
            "usage: {tool} has no LLM tokens yet (claude, codex, grok, gemini)"
        )),
        "" => Err(format!(
            "usage: missing <tool> (claude, codex, grok or gemini) \u{2014} {CLI} llm tokens --help"
        )),
        _ => Err(format!(
            "usage: unknown tool '{tool}' (claude, codex, grok or gemini) \u{2014} {CLI} llm tokens --help"
        )),
    }
}

/// `cmd_llm_accounts`'s flags and words.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    /// The verb, as typed.
    pub sub: String,
    pub json: bool,
    pub device: bool,
    pub usage: bool,
    pub refresh: bool,
    pub clear_default: bool,
    pub tool: String,
    pub workspace: String,
    pub session: String,
    pub pos: Vec<String>,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// `_llm_accounts_usage`, exit 0.
    Help,
    /// A `usage:` line, exit 2.
    Usage(String),
    Line(Line),
}

/// The words after `llm`.
pub fn parse(words: &[&str]) -> Parsed {
    let noun = words.first().copied().unwrap_or("");
    if matches!(noun, "" | "help" | "--help" | "-h") {
        return Parsed::Help;
    }
    if !NOUNS.contains(&noun) {
        return Parsed::Usage(unknown_noun(noun));
    }
    let rest = &words[1..];
    let sub = rest.first().copied().unwrap_or("");
    if matches!(sub, "" | "help" | "--help" | "-h") {
        return Parsed::Help;
    }
    let rest = &rest[1..];
    if rest.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return Parsed::Help;
    }
    let mut l = Line {
        sub: sub.to_string(),
        ..Line::default()
    };
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        match a {
            "--json" => l.json = true,
            "--device" => l.device = true,
            "--refresh" => l.refresh = true,
            "--clear-default" => l.clear_default = true,
            "--usage" => l.usage = true,
            "--workspace" | "--session" | "--tool" => {
                let v = rest.get(i + 1).copied().unwrap_or("");
                if v.is_empty() {
                    return Parsed::Usage(format!("usage: {a} needs a value"));
                }
                match a {
                    "--workspace" => l.workspace = v.into(),
                    "--session" => l.session = v.into(),
                    _ => l.tool = v.into(),
                }
                i += 1;
            }
            _ if a.starts_with("--") => return Parsed::Usage(unknown_flag(a)),
            _ => l.pos.push(a.to_string()),
        }
        i += 1;
    }
    Parsed::Line(l)
}

/// The shell skips its connection gate (`cli/src/cli-tools-policy.sh`).
pub fn skips_gate(words: &[&str]) -> bool {
    match words.first().copied().unwrap_or("") {
        "" | "help" | "--help" | "-h" => true,
        n if NOUNS.contains(&n) => {
            matches!(
                words.get(1).copied().unwrap_or(""),
                "" | "help" | "--help" | "-h"
            ) || words[1..].iter().any(|w| matches!(*w, "-h" | "--help"))
        }
        _ => false,
    }
}

/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let all: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let words = all.get(1..).unwrap_or_default();
    if !python::family_native(python::here()) {
        return None;
    }
    let parsed = parse(words);
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let skip = skips_gate(words);
    let port = settings.port_text.as_deref().and_then(curl_port);
    let gate = prelude::gate(&settings);
    if matches!(parsed, Parsed::Line(_)) && port.is_none() && (gate.is_none() || skip) {
        return None;
    }
    let offline = all.iter().any(|w| matches!(*w, "-h" | "--help"));
    match gate {
        Some(Gate::VirtualDesktop) if !offline => return Some(Gate::VirtualDesktop.stop()),
        Some(_) if !skip => return Some(Gate::CannotConnect.stop()),
        _ => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, TOOL) {
        return Some(stop);
    }
    let line = match parsed {
        Parsed::Help => {
            return Some(Shown::out(crate::text::help("llm").unwrap_or_default()).print())
        }
        Parsed::Usage(l) => {
            out::stderr_line(&l);
            return Some(Exit::Usage);
        }
        Parsed::Line(l) => l,
    };
    let mut settings = settings;
    settings.port = port;
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let mut cx = Cx {
        client,
        closing: Closing::default(),
        sock: confirm::sock_path(&settings.home),
    };
    let exit = match verb(&mut cx, &line) {
        Ok(e) | Err(e) => e,
    };
    cx.closing.flush(&mut cx.client, exit);
    Some(exit)
}

/// What every verb works with.
pub struct Cx {
    pub client: Client,
    pub closing: Closing,
    /// `<K2 home>/vault.sock`, for the confirm of `auto-next on`.
    pub sock: PathBuf,
}

/// The verb ends the command: its exit (`Err` and `Ok` alike).
type Done = Result<Exit, Exit>;

impl Cx {
    /// `_sidecar_split "$(_sidecar_send …)"`, then `_sidecar_fail` for a
    /// status that is not 200.
    fn call(&mut self, post: bool, route: &str, payload: &str) -> Result<Split, Exit> {
        let s = confirm::sidecar_call(&mut self.client, post, &format!("{BASE}{route}"), payload)?;
        if s.ok() {
            return Ok(s);
        }
        out::stderr_line(&fail_line(&s.status, &s.body));
        let passport = self.client.passport().is_some();
        self.closing.answer(&s.response(), passport);
        Err(exit_for(&s.status))
    }

    /// `_llma_get <route> [query]`.
    pub fn get(&mut self, route: &str, query: &str) -> Result<Split, Exit> {
        self.call(false, route, query)
    }

    /// `_llma_post <route> <json>`.
    pub fn post(&mut self, route: &str, body: &str) -> Result<Split, Exit> {
        self.call(true, route, body)
    }

    /// `_llma_resolve <tool> <label|id>`: the account's id through `list`,
    /// or `not_found`, exit 4.
    pub fn resolve(&mut self, tool: &str, key: &str) -> Result<String, Exit> {
        let s = self.get("list", "")?;
        let id = render::resolve(&s.body, tool, key);
        if id.is_empty() {
            out::stderr_line(&format!(
                "not_found: no {tool} token named '{key}' ({CLI} llm tokens list --tool {tool})"
            ));
            return Err(Exit::NotFound);
        }
        Ok(id)
    }
}

/// `_llma_json k1 v1 k2 v2 …`: `"__true__"` and `"__false__"` are
/// booleans, an empty value is left out.
pub fn json_of(pairs: &[(&str, &[u8])]) -> String {
    let mut out: Vec<(Text, Value)> = Vec::new();
    for (k, v) in pairs {
        let value = match *v {
            b"__true__" => Value::Bool(true),
            b"__false__" => Value::Bool(false),
            b"" => continue,
            v => confirm::text_of_bytes(v),
        };
        // `out[k] = v`: a key given twice keeps its first place.
        match out.iter_mut().find(|e| e.0 == Text::from(*k)) {
            Some((_, old)) => *old = value,
            None => out.push((Text::from(*k), value)),
        }
    }
    confirm::dumps(&Value::Object(out))
}

/// `printf '%s\n' "$_SC_BODY"; exit 0`.
fn print_json(s: &Split) -> Done {
    let mut text = s.body.clone();
    text.push(b'\n');
    match out::write_flushed(&mut std::io::stdout().lock(), &text) {
        Ok(()) => Ok(Exit::Ok),
        Err(e) => Err(out::after_stdout_error(&e)),
    }
}

/// `echo` lines on stdout.
fn say(lines: &[String]) -> Done {
    let mut text = String::new();
    for l in lines {
        text.push_str(l);
        text.push('\n');
    }
    match out::stdout(&text) {
        Ok(()) => Ok(Exit::Ok),
        Err(e) => Err(out::after_stdout_error(&e)),
    }
}

fn usage_err(line: &str) -> Exit {
    out::stderr_line(line);
    Exit::Usage
}

/// `[ "${#pos[@]}" -eq n ] || usage`.
fn need(l: &Line, n: usize, usage_rest: &str) -> Result<(), Exit> {
    if l.pos.len() == n {
        Ok(())
    } else {
        Err(usage_err(&usage(usage_rest)))
    }
}

fn check(r: Result<(), String>) -> Result<(), Exit> {
    r.map_err(|l| usage_err(&l))
}

/// `cmd_llm_accounts`'s verbs.
pub fn verb(cx: &mut Cx, l: &Line) -> Done {
    let p = |i: usize| l.pos[i].as_str();
    let mode: &[u8] = if l.device {
        b"other_device"
    } else {
        b"this_computer"
    };
    match l.sub.as_str() {
        "list" | "ls" => {
            need(l, 0, "list [--tool <tool>] [--json]")?;
            if !l.tool.is_empty() {
                check(tool_ok(&l.tool))?;
            }
            let s = cx.get("list", "")?;
            if l.json {
                return print_json(&s);
            }
            Ok(render::list(&s.body, &l.tool).print())
        }
        "status" => {
            need(l, 2, "status <tool> <label|id> [--json]")?;
            check(tool_ok(p(0)))?;
            let q = format!("id={}&tool={}", quote(p(1)), quote(p(0)));
            let s = cx.get("status", &q)?;
            if l.json {
                return print_json(&s);
            }
            Ok(render::status(&s.body).print())
        }
        "usage" => {
            need(l, 0, "usage [--json]")?;
            let s = cx.get("usage", "")?;
            if l.json {
                return print_json(&s);
            }
            Ok(render::usage(&s.body).print())
        }
        "add" | "login" => {
            need(l, 2, &format!("{} <tool> <label> [--device]", l.sub))?;
            check(tool_ok(p(0)))?;
            let s = if l.sub == "add" {
                let b = json_of(&[
                    ("tool", p(0).as_bytes()),
                    ("label", p(1).as_bytes()),
                    ("mode", mode),
                ]);
                cx.post("add", &b)?
            } else {
                let id = cx.resolve(p(0), p(1))?;
                cx.post("login", &json_of(&[("id", id.as_bytes()), ("mode", mode)]))?
            };
            if l.json {
                return print_json(&s);
            }
            follow_login(cx, &s.body)
        }
        "switch" | "use" | "next" => {
            let s = if l.sub == "next" {
                need(l, 1, "next <tool>")?;
                check(tool_ok(p(0)))?;
                cx.post("next", &json_of(&[("tool", p(0).as_bytes())]))?
            } else {
                need(l, 2, "switch <tool> <label>")?;
                check(tool_ok(p(0)))?;
                let id = cx.resolve(p(0), p(1))?;
                cx.post("switch", &json_of(&[("id", id.as_bytes())]))?
            };
            if l.json {
                return print_json(&s);
            }
            Ok(render::switch(&s.body).print())
        }
        "rename" => {
            need(l, 3, "rename <tool> <label> <new-label>")?;
            check(tool_ok(p(0)))?;
            let id = cx.resolve(p(0), p(1))?;
            let s = cx.post(
                "rename",
                &json_of(&[("id", id.as_bytes()), ("label", p(2).as_bytes())]),
            )?;
            if l.json {
                return print_json(&s);
            }
            say(&[format!("renamed {} '{}' to '{}'", p(0), p(1), p(2))])
        }
        "remove" | "rm" => {
            need(l, 2, "remove <tool> <label> [--clear-default]")?;
            check(tool_ok(p(0)))?;
            let id = cx.resolve(p(0), p(1))?;
            let clear: &[u8] = if l.clear_default { b"__true__" } else { b"" };
            let s = cx.post(
                "remove",
                &json_of(&[("id", id.as_bytes()), ("clearDefault", clear)]),
            )?;
            if l.json {
                return print_json(&s);
            }
            say(&[format!(
                "removed {} token '{}' (its folder is kept in ~/{}/llm-accounts/.removed)",
                p(0),
                p(1),
                k2_client::paths::HOME_DIR_NAME
            )])
        }
        "clear-default" => {
            need(l, 1, "clear-default <tool>")?;
            check(tool_ok(p(0)))?;
            let s = cx.post("clear-default", &json_of(&[("tool", p(0).as_bytes())]))?;
            if l.json {
                return print_json(&s);
            }
            Ok(render::clear_default(&s.body, p(0)).print())
        }
        "refresh" => {
            let mut id = String::new();
            match l.pos.len() {
                0 => {}
                2 => {
                    check(tool_ok(p(0)))?;
                    id = cx.resolve(p(0), p(1))?;
                }
                _ => return Err(usage_err(&usage("refresh [<tool> <label>] [--usage]"))),
            }
            let u: &[u8] = if l.usage { b"__true__" } else { b"" };
            let s = cx.post("refresh", &json_of(&[("id", id.as_bytes()), ("usage", u)]))?;
            if l.json {
                return print_json(&s);
            }
            Ok(render::refresh(&s.body).print())
        }
        "storage" => {
            let s = match l.pos.len() {
                0 => cx.get("storage", "")?,
                1 if matches!(p(0), "files" | "vault") => {
                    cx.post("storage/set", &json_of(&[("storage", p(0).as_bytes())]))?
                }
                _ => return Err(usage_err(&usage("storage [files|vault]"))),
            };
            if l.json {
                return print_json(&s);
            }
            Ok(render::storage(&s.body).print())
        }
        "auto-next" | "auto-rotate" => {
            let bad = || usage_err(&usage("auto-next [<tool> on|off]"));
            let s = match l.pos.len() {
                0 => cx.get("auto-next", "")?,
                2 => {
                    check(tool_ok(p(0)))?;
                    match p(1) {
                        "on" => {
                            let label = format!("auto-rotate {}", p(0));
                            let passport = cx.client.passport().is_some();
                            let mut c = Confirm {
                                client: &mut cx.client,
                                sock: &cx.sock,
                                closing: &mut cx.closing,
                                passport,
                            };
                            let Some(tok) =
                                confirm::confirm_token(&mut c, "llm-auto-next", p(0), &label)
                            else {
                                return Err(Exit::Refused);
                            };
                            let b = json_of(&[
                                ("tool", p(0).as_bytes()),
                                ("on", &b"__true__"[..]),
                                ("confirmToken", tok.as_bytes()),
                            ]);
                            drop(tok);
                            cx.post("auto-next/set", &b)?
                        }
                        "off" => cx.post(
                            "auto-next/set",
                            &json_of(&[("tool", p(0).as_bytes()), ("on", &b"__false__"[..])]),
                        )?,
                        _ => return Err(bad()),
                    }
                }
                _ => return Err(bad()),
            };
            if l.json {
                return print_json(&s);
            }
            Ok(render::auto_next(&s.body).print())
        }
        "pins" => {
            need(l, 0, "pins [--json]")?;
            let pins = cx.get("pins", "")?;
            if l.json {
                return print_json(&pins);
            }
            let list = cx.get("list", "")?;
            Ok(render::pins(&pins.body, &list.body).print())
        }
        "for" => {
            if l.pos.len() != 1 {
                return Err(usage_err(&usage(
                    "for <agent> [--refresh] [--json]   (<agent> = a workspace or a sidecar ws/2)",
                )));
            }
            let s = if l.refresh {
                cx.post("for/refresh", &json_of(&[("addr", p(0).as_bytes())]))?
            } else {
                cx.get("for", &format!("addr={}", quote(p(0))))?
            };
            if l.json {
                return print_json(&s);
            }
            Ok(render::for_agent(&s.body, p(0)).print())
        }
        "pin" | "unpin" => pin(cx, l),
        "add-key" => add_key(cx, l),
        other => Err(usage_err(&unknown_verb(other))),
    }
}

/// `pin` and `unpin`.
fn pin(cx: &mut Cx, l: &Line) -> Done {
    if !l.workspace.is_empty() && !l.session.is_empty() {
        return Err(usage_err("usage: pass --workspace or --session, not both"));
    }
    let (scope, scope_id) = if !l.workspace.is_empty() {
        ("workspace", l.workspace.as_str())
    } else if !l.session.is_empty() {
        ("session", l.session.as_str())
    } else {
        let label = if l.sub == "pin" { " <label>" } else { "" };
        return Err(usage_err(&usage(&format!(
            "{} --workspace <ws>|--session <key> <tool>{label}",
            l.sub
        ))));
    };
    let word = if scope == "session" { "chat" } else { scope };
    let p = |i: usize| l.pos[i].as_str();
    if l.sub == "pin" {
        need(l, 2, "pin --workspace <ws>|--session <key> <tool> <label>")?;
        check(key_tool_ok(p(0)))?;
        let id = cx.resolve(p(0), p(1))?;
        let s = cx.post(
            "pin",
            &json_of(&[
                ("scope", scope.as_bytes()),
                ("scopeId", scope_id.as_bytes()),
                ("tool", p(0).as_bytes()),
                ("id", id.as_bytes()),
            ]),
        )?;
        if l.json {
            return print_json(&s);
        }
        return Ok(render::pinned(&s.body, p(1)).print());
    }
    need(l, 1, "unpin --workspace <ws>|--session <key> <tool>")?;
    check(key_tool_ok(p(0)))?;
    let s = cx.post(
        "unpin",
        &json_of(&[
            ("scope", scope.as_bytes()),
            ("scopeId", scope_id.as_bytes()),
            ("tool", p(0).as_bytes()),
        ]),
    )?;
    if l.json {
        return print_json(&s);
    }
    // `if printf … | python3 -c '…sys.exit(0 if … else 1)'; then`: a
    // raise is the else branch, its traceback printed.
    let line = match render::unpinned(&s.body) {
        Ok(true) => format!("{word} {scope_id} {}: back to Server default", p(0)),
        Ok(false) => format!(
            "{word} {scope_id} has no {} token of its own (it uses Server default)",
            p(0)
        ),
        Err(r) => {
            out::stderr_line(r.text().trim_end_matches('\n'));
            format!(
                "{word} {scope_id} has no {} token of its own (it uses Server default)",
                p(0)
            )
        }
    };
    say(&[line])
}

/// `add-key <tool> <label>`: the API token from stdin.
fn add_key(cx: &mut Cx, l: &Line) -> Done {
    if l.pos.len() > 2 {
        return Err(usage_err(&format!(
            "usage: the API token is read from stdin, never an argument \u{2014} {CLI} llm tokens add-key <tool> <label>"
        )));
    }
    need(
        l,
        2,
        "add-key <tool> <label>   (the API token is read from stdin)",
    )?;
    let (tool, label) = (l.pos[0].as_str(), l.pos[1].as_str());
    check(key_tool_ok(tool))?;
    let key = if prompt::stdin_is_tty() {
        prompt::ask_hidden("Paste the API token: ")
    } else {
        prompt::read_line(prompt::Echo::Shown, prompt::AtEof::Keep)
    };
    if key.expose().is_empty() {
        return Err(usage_err("usage: no API token on stdin"));
    }
    // `sys.stdin.read().strip()`.
    let v = confirm::text_of_bytes(key.expose());
    drop(key);
    let stripped = match &v {
        Value::Str(t) => match t.as_str() {
            Some(s) => Value::Str(Text::from(s.trim_matches(super::py_space))),
            None => v.clone(),
        },
        other => other.clone(),
    };
    let body = prompt::Secret::new(
        confirm::dumps(&confirm::object(vec![
            ("tool", Value::Str(Text::from(tool))),
            ("label", Value::Str(Text::from(label))),
            ("key", stripped),
        ]))
        .into_bytes(),
    );
    drop(v);
    let s = cx.post("add-key", &String::from_utf8_lossy(body.expose()))?;
    drop(body);
    if l.json {
        return print_json(&s);
    }
    let r = say(&[
        format!("added {tool} API token '{label}' (billed per token; the key is not shown again)"),
        format!(
            "Use it: {CLI} llm tokens pin --workspace <ws> {tool} '{label}', or make it the server default ({CLI} llm tokens switch)."
        ),
    ]);
    kessel_hint(cx);
    r
}

/// `_setup_kessel_hint` (`cli/src/setup.sh`): the meet-Kessel line when
/// the first-run list offers it; nothing on any failure.
pub fn kessel_hint(cx: &mut Cx) {
    let Ok(r) = cx
        .client
        .setup_send(false, "/cli/first-run/status", false, b"")
    else {
        return;
    };
    let raw = super::setup::held(&Ok(r), cx.client.refused_before_answer());
    let Some(nl) = raw.iter().rposition(|b| *b == b'\n') else {
        return;
    };
    if &raw[nl + 1..] != b"200" {
        return;
    }
    let needle = br#""offer":"meet_kessel""#;
    if raw[..nl].windows(needle.len()).any(|w| w == needle) {
        let _ = out::stdout_line(super::setup::MEET_KESSEL);
    }
}

static STOPPED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_signal(_: libc::c_int) {
    STOPPED.store(true, Ordering::SeqCst);
}

/// `trap '…' INT TERM`.
fn trap_signals(on: bool) {
    #[cfg(unix)]
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        let h = if on {
            on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t
        } else {
            libc::SIG_DFL
        };
        libc::signal(libc::SIGINT, h);
        libc::signal(libc::SIGTERM, h);
    }
    #[cfg(not(unix))]
    let _ = on;
}

/// `_llma_post login/cancel … >/dev/null 2>&1`: `_sidecar_fail` exits
/// from inside it, so a cancel that isn't answered 200 ends the command
/// there with its exit and nothing shown (the `|| true` after it never
/// runs). `Err`: that exit.
fn cancel(cx: &mut Cx, login_id: &str) -> Result<(), Exit> {
    let body = json_of(&[("loginId", login_id.as_bytes())]);
    match cx
        .client
        .sidecar_post(&format!("{BASE}login/cancel"), &body)
    {
        Ok(r) if r.status == 200 => Ok(()),
        Ok(r) => Err(exit_for(&format!("{:03}", r.status))),
        Err(_) => Err(Exit::Error),
    }
}

/// The trap's own words: cancel the sign-in, say so, exit 130.
fn stopped(cx: &mut Cx, login_id: &str) -> Exit {
    if let Err(e) = cancel(cx, login_id) {
        return e;
    }
    out::stderr_line("cancelled");
    Exit::Interrupted
}

/// `_envn_into poll LLMA_POLL_SECS 2`, as `sleep` reads it.
fn poll_secs() -> Duration {
    let v = k2_client::env_names::first_nonempty_os_in(k2_client::env_names::LLMA_POLL_SECS, |n| {
        std::env::var_os(n)
    })
    .and_then(|(_, v)| v.into_string().ok())
    .unwrap_or_else(|| "2".into());
    Duration::from_secs_f64(v.trim().parse::<f64>().unwrap_or(2.0).max(0.0))
}

/// `echo "$text"` (no `-n` or `-e` words are given here by the daemon in
/// practice; a word that is only such flags prints as `echo` does).
fn echo_text(text: &str) -> String {
    let flags = text.len() > 1
        && text.starts_with('-')
        && text[1..].chars().all(|c| matches!(c, 'n' | 'e' | 'E'));
    if flags {
        if text.contains('n') {
            String::new()
        } else {
            "\n".into()
        }
    } else {
        format!("{text}\n")
    }
}

/// `_llma_follow_login <start body>`.
pub fn follow_login(cx: &mut Cx, start: &[u8]) -> Done {
    // `login_id="$(… 2>/dev/null)"` under `set -e`: a raise ends the
    // command, exit 1, nothing shown.
    let Ok(login_id) = render::login_id(start) else {
        return Err(Exit::Error);
    };
    if login_id.is_empty() {
        out::stderr_line("daemon_error: the daemon did not start a sign-in");
        return Err(Exit::Error);
    }
    STOPPED.store(false, Ordering::SeqCst);
    trap_signals(true);
    let r = follow(cx, &login_id);
    trap_signals(false);
    r
}

fn follow(cx: &mut Cx, login_id: &str) -> Done {
    let poll = poll_secs();
    let (mut shown_url, mut shown_code, mut shown_banner) =
        (String::new(), String::new(), String::new());
    let mut prompted = false;
    let deadline = Instant::now() + Duration::from_secs(1200);
    loop {
        let s = cx.get("login/status", &format!("loginId={}", quote(login_id)))?;
        if STOPPED.load(Ordering::SeqCst) {
            return Err(stopped(cx, login_id));
        }
        let f = match render::login_fields(&s.body) {
            Ok(f) => f,
            Err(r) => return Err(Shown::err(r.text(), Exit::Error).print()),
        };
        let mut text = String::new();
        if !f.banner.is_empty() && f.banner != shown_banner {
            prompt::prompt(&echo_text(&f.banner));
            shown_banner = f.banner.clone();
        }
        if !f.url.is_empty() && f.url != shown_url {
            if prompt::stdout_is_tty() {
                text.push_str("Open this page to sign in:\n");
                text.push_str(&format!("\x1b]8;;{0}\x1b\\{0}\x1b]8;;\x1b\\\n", f.url));
            } else {
                text.push_str(&format!("Open this page to sign in: {}\n", f.url));
            }
            shown_url = f.url.clone();
        }
        if !f.code.is_empty() && f.code != shown_code {
            text.push_str(&format!("Enter this code on that page: {}\n", f.code));
            shown_code = f.code.clone();
        }
        if !text.is_empty() {
            if let Err(e) = out::stdout(&text) {
                return Err(out::after_stdout_error(&e));
            }
        }
        match f.state.as_str() {
            "signed_in" => {
                let mut lines = vec![format!("Signed in: {} subscription '{}'.", f.tool, f.label)];
                if f.offer == "1" {
                    lines.push(format!(
                        "It is not the server default. To use it now: {CLI} llm tokens switch {} '{}'",
                        f.tool, f.label
                    ));
                    lines.push("Changing the server default token affects every chat that uses Server default on this server.".into());
                }
                say(&lines)?;
                kessel_hint(cx);
                return Ok(Exit::Ok);
            }
            "failed" | "timed_out" | "cancelled" => {
                let err = if f.err.is_empty() {
                    String::new()
                } else {
                    format!(": {}", f.err)
                };
                out::stderr_line(&format!("sign-in {}{err}", f.state));
                return Err(Exit::Error);
            }
            "waiting_for_code" if !prompted => {
                prompted = true;
                prompt::prompt("Paste the code: ");
                let pasted = if prompt::stdin_is_tty() {
                    let v = prompt::read_line(prompt::Echo::Hidden, prompt::AtEof::Drop);
                    prompt::prompt("\n");
                    v
                } else {
                    prompt::read_line(prompt::Echo::Shown, prompt::AtEof::Drop)
                };
                if STOPPED.load(Ordering::SeqCst) {
                    return Err(stopped(cx, login_id));
                }
                if pasted.expose().is_empty() {
                    cancel(cx, login_id)?;
                    out::stderr_line("no code entered; sign-in cancelled");
                    return Err(Exit::Error);
                }
                let body = prompt::Secret::new(
                    json_of(&[("loginId", login_id.as_bytes()), ("text", pasted.expose())])
                        .into_bytes(),
                );
                drop(pasted);
                cx.post("login/input", &String::from_utf8_lossy(body.expose()))?;
                drop(body);
                continue;
            }
            _ => {}
        }
        if f.done == "1" {
            let state = if f.state.is_empty() {
                "unknown"
            } else {
                f.state.as_str()
            };
            let err = if f.err.is_empty() {
                String::new()
            } else {
                format!(": {}", f.err)
            };
            out::stderr_line(&format!("sign-in ended ({state}){err}"));
            return Err(Exit::Error);
        }
        if Instant::now() >= deadline {
            cancel(cx, login_id)?;
            out::stderr_line("sign-in timed out");
            return Err(Exit::Error);
        }
        // `sleep "$poll"`, cut short by the trap.
        let until = Instant::now() + poll;
        while Instant::now() < until {
            if STOPPED.load(Ordering::SeqCst) {
                return Err(stopped(cx, login_id));
            }
            std::thread::sleep((until - Instant::now()).min(Duration::from_millis(50)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_json_is_the_shell_s() {
        assert_eq!(
            json_of(&[
                ("id", &b"a"[..]),
                ("x", &b""[..]),
                ("on", &b"__true__"[..]),
                ("off", &b"__false__"[..])
            ]),
            r#"{"id": "a", "on": true, "off": false}"#
        );
        assert_eq!(json_of(&[]), "{}");
        assert_eq!(json_of(&[("t", &b"\xff"[..])]), r#"{"t": "\udcff"}"#);
    }

    #[test]
    fn the_words_are_the_shell_s() {
        assert_eq!(parse(&[]), Parsed::Help);
        assert_eq!(parse(&["tokens"]), Parsed::Help);
        assert_eq!(parse(&["tokens", "list", "x", "--help"]), Parsed::Help);
        assert_eq!(parse(&["nope"]), Parsed::Usage(unknown_noun("nope")));
        assert_eq!(
            parse(&["tokens", "list", "--tool"]),
            Parsed::Usage("usage: --tool needs a value".into())
        );
        assert!(skips_gate(&["accounts", "list", "-h"]));
        assert!(!skips_gate(&["accounts", "list"]));
        assert!(!skips_gate(&["nope", "-h"]));
    }
}
