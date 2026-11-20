//! `compute local …`: this computer as a compute node, as
//! `cmd_compute_local` answers it (`cli/src/compute-local-trust.sh`):
//!
//! - `status [--json]` (also no word): one GET of
//!   `/cli/compute/local/status`, in words or as it came;
//! - `pause [--now]`, `drain`, `resume`: one POST over TCP, one line;
//! - `policy`: the status's `policy` as indented JSON; `policy key=value
//!   …`: one POST over TCP (`-` in a key reads as `_`);
//! - `enable [...]`: one POST over TCP. The enroll code and the ticket come
//!   from stdin (`--code-stdin`, `--ticket-stdin`: one line, the shell's
//!   `_cli_read_secret_stdin`) or, with a note, from argv; either goes in
//!   the body only, never in argv or an environment;
//! - `remote-admin off|policy|full`: one POST over TCP;
//! - `disable [--yes]`: the status first; jobs running here are named
//!   and need `--yes` (or a yes at a terminal); then one POST over TCP.
//!
//! The GETs go the cell socket first, as the shell's do.

use super::super::super::lines::py_line;
use super::super::super::pyrt::{self as p, s};
use super::super::super::writes::{dumps, load, object, print_body, stopped, str_value, Cp, Words};
use super::{at, LOCAL_SRC};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::{self, json::Text, json::Value};
use crate::prompt::{self, AtEof, Echo, Secret};
use k2_client::{brand::CLI_NAME, cli_name, product_name};

pub const STATUS: &str = "/cli/compute/local/status";
pub const POLICY: &str = "/cli/compute/local/policy";
pub const ENABLE: &str = "/cli/compute/local/enable";
pub const REMOTE_ADMIN: &str = "/cli/compute/local/remote-admin";

pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute local status|pause [--now]|drain|resume|policy|enable|remote-admin|disable"
);
pub const POLICY_USAGE: &str =
    concat!("usage: ", cli_name!(), " compute local policy key=value...");
pub const ENABLE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute local enable [--controller URL --code-stdin --name NAME] [--private-home|--no-private-home] [--remote-admin off|policy|full]"
);
pub const REMOTE_ADMIN_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute local remote-admin off|policy|full"
);
pub const LEVEL_LINE: &str = "--remote-admin is off, policy or full";

macro_rules! status_line {
    ($m:literal) => {
        const { py_line(LOCAL_SRC, "        status)", $m) }
    };
}
macro_rules! said_line {
    ($after:literal, $m:literal) => {
        const { py_line(LOCAL_SRC, $after, $m) }
    };
}

/// `enable`'s words.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Enable {
    pub controller: String,
    pub code_stdin: bool,
    pub enroll: String,
    /// `--enroll` or `--code`, the last given.
    pub enroll_flag: String,
    pub ticket_stdin: bool,
    pub ticket: String,
    pub ticket_flag: bool,
    pub name: String,
    /// `--private-home` (`Some(true)`) or `--no-private-home`.
    pub private: Option<bool>,
    pub remote_admin: String,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Status {
        json: bool,
    },
    Pause {
        now: bool,
    },
    /// `drain` or `resume`.
    Control(&'static str),
    PolicyShow,
    PolicySet(Vec<(String, String)>),
    Enable(Enable),
    RemoteAdmin(String),
    Disable {
        yes: bool,
    },
}

/// The words after `local`.
pub fn parse(words: &[&str]) -> Words<Ask> {
    let sub = match words.first() {
        None | Some(&"") => "status",
        Some(w) => *w,
    };
    let rest = words.get(1..).unwrap_or(&[]);
    let first = rest.first().copied().unwrap_or("");
    Words::Ask(match sub {
        "status" => Ask::Status {
            json: first == "--json",
        },
        "pause" => Ask::Pause {
            now: first == "--now",
        },
        "drain" => Ask::Control("drain"),
        "resume" => Ask::Control("resume"),
        "policy" if rest.is_empty() => Ask::PolicyShow,
        "policy" => {
            let mut set = vec![];
            for a in rest {
                let Some((k, v)) = a.split_once('=') else {
                    return Words::Usage(POLICY_USAGE.to_string());
                };
                set.push((k.replace('-', "_"), v.to_string()));
            }
            Ask::PolicySet(set)
        }
        "enable" => match enable(rest) {
            Ok(e) => Ask::Enable(e),
            Err(w) => return w,
        },
        "remote-admin" if matches!(first, "off" | "policy" | "full") => {
            Ask::RemoteAdmin(first.to_string())
        }
        "remote-admin" => return Words::Usage(REMOTE_ADMIN_USAGE.to_string()),
        "disable" => Ask::Disable {
            yes: first == "--yes",
        },
        _ => return Words::Usage(USAGE.to_string()),
    })
}

fn enable(words: &[&str]) -> Result<Enable, Words<Ask>> {
    let mut e = Enable::default();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let v = words.get(i + 1).copied().unwrap_or("").to_string();
        let short = i + 2 > words.len();
        match w {
            "--controller" | "--enroll" | "--code" | "--ticket" | "--name" | "--remote-admin" => {
                match w {
                    "--controller" => e.controller = v,
                    "--ticket" => {
                        e.ticket = v;
                        e.ticket_flag = true;
                    }
                    "--name" => e.name = v,
                    "--remote-admin" => e.remote_admin = v,
                    _ => {
                        e.enroll = v;
                        e.enroll_flag = w.to_string();
                    }
                }
                if short {
                    return Err(Words::ShiftFailed);
                }
                if w == "--remote-admin"
                    && !matches!(e.remote_admin.as_str(), "off" | "policy" | "full")
                {
                    return Err(Words::Usage(LEVEL_LINE.to_string()));
                }
                i += 2;
                continue;
            }
            "--code-stdin" => e.code_stdin = true,
            "--ticket-stdin" => e.ticket_stdin = true,
            "--private-home" => e.private = Some(true),
            "--no-private-home" => e.private = Some(false),
            _ => return Err(Words::Usage(ENABLE_USAGE.to_string())),
        }
        i += 1;
    }
    Ok(e)
}

/// `_cli_read_secret_stdin "<prompt>"; v="${_K2_SECRET%$'\r'}"`.
fn read_secret(prompt_text: &str) -> Secret {
    let mut s = prompt::read_secret_stdin(prompt_text);
    if s.expose().last() == Some(&b'\r') {
        let mut b = s.expose().to_vec();
        b.pop();
        s = Secret::new(b);
    }
    s
}

/// The enroll code and the ticket, as the shell takes them (from stdin or
/// argv, with its notes), or the line that stops it and exit 2.
fn secrets(e: &Enable) -> Result<(Secret, Secret), String> {
    let me = concat!(cli_name!(), " compute local enable");
    if e.code_stdin && e.ticket_stdin {
        return Err(format!(
            "{me}: --code-stdin or --ticket-stdin, not both (stdin holds one)"
        ));
    }
    let ticket = if e.ticket_stdin {
        if !e.ticket.is_empty() {
            return Err(format!("{me}: --ticket-stdin or --ticket, not both"));
        }
        let t = read_secret("Ticket: ");
        if t.is_empty() {
            return Err(format!("{me}: --ticket-stdin: no ticket on stdin"));
        }
        t
    } else {
        if e.ticket_flag {
            out::stderr_line("note: --ticket is visible to every account on this computer while the command runs; use --ticket-stdin");
        }
        Secret::new(e.ticket.as_bytes().to_vec())
    };
    let enroll = if e.code_stdin {
        if !e.enroll.is_empty() {
            return Err(format!("{me}: --code-stdin or {}, not both", e.enroll_flag));
        }
        let c = read_secret("Enroll code: ");
        if c.is_empty() {
            return Err(format!("{me}: --code-stdin: no code on stdin"));
        }
        c
    } else {
        if !e.enroll_flag.is_empty() {
            out::stderr_line(&format!(
                "note: {} is visible to every account on this computer while the command runs; use --code-stdin",
                e.enroll_flag
            ));
        }
        Secret::new(e.enroll.as_bytes().to_vec())
    };
    Ok((enroll, ticket))
}

/// `enable`'s body: the secrets first (`enroll`, `ticket`), then the rest,
/// each left out when empty.
pub fn enable_body(e: &Enable, enroll: &[u8], ticket: &[u8]) -> String {
    let mut pairs = vec![];
    for (k, v) in [("enroll", enroll), ("ticket", ticket)] {
        if !v.is_empty() {
            pairs.push((k, crate::confirm::text_of_bytes(v)));
        }
    }
    for (k, v) in [("controller", &e.controller), ("name", &e.name)] {
        if !v.is_empty() {
            pairs.push((k, str_value(v)));
        }
    }
    if let Some(on) = e.private {
        pairs.push(("privateHome", Value::Bool(on)));
    }
    if !e.remote_admin.is_empty() {
        pairs.push(("remoteAdmin", str_value(&e.remote_admin)));
    }
    dumps(&object(pairs))
}

/// The request and what it prints.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    match a {
        Ask::Status { json } => match cp.call(false, STATUS, "", false) {
            Ok(b) if *json => print_body(&b),
            Ok(b) => status_reader(&b).print(),
            Err(e) => e,
        },
        Ask::Pause { now } => {
            let body = dumps(&object(vec![("now", Value::Bool(*now))]));
            said(cp.call(true, "/cli/compute/local/pause", &body, true))
        }
        Ask::Control(verb) => {
            said(cp.call(true, &format!("/cli/compute/local/{verb}"), "{}", true))
        }
        Ask::PolicyShow => match cp.call(false, STATUS, "", false) {
            Ok(b) => policy(&b, false).print(),
            Err(e) => e,
        },
        Ask::PolicySet(set) => {
            let mut d: Vec<(Text, Value)> = vec![];
            for (k, v) in set {
                let k = Text::from(k.as_str());
                match d.iter_mut().find(|(x, _)| *x == k) {
                    Some(slot) => slot.1 = str_value(v),
                    None => d.push((k, str_value(v))),
                }
            }
            let body = dumps(&object(vec![("set", Value::Object(d))]));
            match cp.call(true, POLICY, &body, true) {
                Ok(b) => policy(&b, true).print(),
                Err(e) => e,
            }
        }
        Ask::Enable(e) => {
            let (enroll, ticket) = match secrets(e) {
                Ok(s) => s,
                Err(line) => {
                    out::stderr_line(&line);
                    return Exit::Usage;
                }
            };
            let body = enable_body(e, enroll.expose(), ticket.expose());
            match cp.call(true, ENABLE, &body, true) {
                Ok(b) => message_reader(&b, true).print(),
                Err(e) => e,
            }
        }
        Ask::RemoteAdmin(level) => {
            let body = dumps(&object(vec![("level", str_value(level))]));
            match cp.call(true, REMOTE_ADMIN, &body, true) {
                Ok(b) => message_reader(&b, false).print(),
                Err(e) => e,
            }
        }
        Ask::Disable { yes } => disable(cp, *yes),
    }
}

/// `disable`: the jobs running here first, then the POST.
fn disable(cp: &mut Cp<'_>, yes: bool) -> Exit {
    let status = match cp.call(false, STATUS, "", false) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let running = match running(&status) {
        Ok(r) => r,
        Err(e) => return stopped(String::new(), &e).print(),
    };
    if !running.is_empty() && !yes {
        out::stderr_line(&format!(
            "These jobs are running here and stop now: {running}"
        ));
        if prompt::stdin_is_tty() {
            prompt::prompt("Stop being a compute node? [y/N] ");
            // `read -r answer`: IFS's spaces and tabs off both ends.
            let answer = prompt::read_line(Echo::Shown, AtEof::Drop);
            let ifs = |b: &u8| matches!(b, b' ' | b'\t' | b'\n');
            let raw = answer.expose();
            let start = raw.iter().position(|b| !ifs(b)).unwrap_or(raw.len());
            let end = raw.iter().rposition(|b| !ifs(b)).map_or(start, |e| e + 1);
            let word = &raw[start..end];
            if !matches!(word, b"y" | b"Y" | b"yes") {
                out::stderr_line("not stopped");
                return Exit::Usage;
            }
        } else {
            out::stderr_line(&format!(
                "pass --yes to stop them ({CLI_NAME} compute local disable --yes)"
            ));
            return Exit::Usage;
        }
    }
    match cp.call(true, "/cli/compute/local/disable", "{}", true) {
        Ok(b) => message_reader(&b, false).print(),
        Err(e) => e,
    }
}

/// `" ".join((v.get("node") or {}).get("running") or [])`, as `$(…)` holds
/// it (trailing newlines gone).
fn running(body: &[u8]) -> Result<String, Raise> {
    let v = load(body, 1)?;
    let n = p::get(&v, "node").map(|n| p::or(n, Value::Object(vec![])));
    let r = n
        .and_then(|n| p::get(&n, "running"))
        .map(|r| p::or(r, Value::Array(vec![])))
        .and_then(|r| p::join(" ", &r))
        .map_err(at(1))?;
    Ok(r.trim_end_matches('\n').to_string())
}

/// `print("%s: %s" % (v["control"], v["note"]))`.
fn said(answer: Result<Vec<u8>, Exit>) -> Exit {
    let body = match answer {
        Ok(b) => b,
        Err(e) => return e,
    };
    let r = load(&body, 1).and_then(|v| {
        let args = [
            p::idx(&v, "control").map_err(at(1))?,
            p::idx(&v, "note").map_err(at(1))?,
        ];
        p::pct("%s: %s", &args).map_err(at(1))
    });
    let shown = match r {
        Ok(line) => Shown::out(line + "\n"),
        Err(e) => stopped(String::new(), &e),
    };
    shown.print()
}

/// The policy as indented JSON: `.get("policy")` (the status) or
/// `["policy"]` (a change's answer).
fn policy(body: &[u8], set: bool) -> Shown {
    let r = load(body, 1).and_then(|v| {
        let pol = if set {
            p::idx(&v, "policy")
        } else {
            p::get(&v, "policy")
        }
        .map_err(at(1))?;
        p::dumps(&pol, false, Some(2)).map_err(at(1))
    });
    match r {
        Ok(text) => Shown::out(text + "\n"),
        Err(e) => stopped(String::new(), &e),
    }
}

/// The reader for `enable` (`sas`: its code line too), `remote-admin` and
/// `disable`: the message, then the command to run.
pub fn message_reader(body: &[u8], sas: bool) -> Shown {
    let mut out = String::new();
    let r = (|| -> Result<(), Raise> {
        let l = said_line!(
            "_compute_call POST /cli/compute/local/enable",
            "v = json.load"
        );
        let v = load(body, l)?;
        let l = said_line!(
            "_compute_call POST /cli/compute/local/enable",
            "print(v.get(\"message\""
        );
        let m = p::get_or(&v, "message", s("")).map_err(at(l))?;
        out.push_str(&p::str_of(&m));
        out.push('\n');
        let l = said_line!(
            "_compute_call POST /cli/compute/local/enable",
            "if v.get(\"command\")"
        );
        if p::truthy(&p::get(&v, "command").map_err(at(l))?) {
            let l = l + 1;
            let c = p::idx(&v, "command").map_err(at(l))?;
            out.push_str(&p::str_of(&p::add(&s("  "), &c).map_err(at(l))?));
            out.push('\n');
        }
        if sas {
            let l = said_line!(
                "_compute_call POST /cli/compute/local/enable",
                "if v.get(\"sas\")"
            );
            if p::truthy(&p::get(&v, "sas").map_err(at(l))?) {
                let l = l + 1;
                let code = p::idx(&v, "sas").map_err(at(l))?;
                let text = p::pct(
                    &format!(
                        "Code: %s \u{2014} confirm it on the controller: {CLI_NAME} compute node confirm <name> <code>"
                    ),
                    &[code],
                )
                .map_err(at(l))?;
                out.push_str(&text);
                out.push('\n');
            }
        }
        Ok(())
    })();
    match r {
        Ok(()) => Shown::out(out),
        Err(e) => stopped(out, &e),
    }
}

/// `{"helper": …, "prompt": …, "command": …}.get(e, "one sudo command")`.
fn elevation(e: &Value) -> Result<&'static str, String> {
    const ROOT: &[(&str, &str)] = &[
        ("helper", "the root helper installs it (no password)"),
        ("prompt", "an admin prompt or one sudo command"),
        (
            "command",
            "one sudo command (or passwordless sudo, tried on enable)",
        ),
    ];
    match e {
        Value::Array(_) | Value::Object(_) => {
            Err(format!("TypeError: unhashable type: '{}'", p::tname(e)))
        }
        Value::Str(t) => Ok(ROOT
            .iter()
            .find(|(k, _)| *t == Text::from(*k))
            .map_or("one sudo command", |(_, v)| *v)),
        _ => Ok("one sudo command"),
    }
}

/// The reader on `status`.
pub fn status_reader(body: &[u8]) -> Shown {
    let mut out = String::new();
    match status_read(body, &mut out) {
        Ok(()) => Shown::out(out),
        Err(e) => stopped(out, &e),
    }
}

fn status_read(body: &[u8], out: &mut String) -> Result<(), Raise> {
    let cli = CLI_NAME;
    let v = load(body, status_line!("v = json.load"))?;
    let l = status_line!("b = v.get(\"binary\")");
    let b = p::get(&v, "binary")
        .map(|x| p::or(x, Value::Object(vec![])))
        .map_err(at(l))?;
    let l = status_line!("hc = v.get(\"homeCheck\")");
    let hc = p::get(&v, "homeCheck")
        .map(|x| p::or(x, Value::Object(vec![])))
        .map_err(at(l))?;
    let l = status_line!("if not v.get(\"installed\")");
    if !p::truthy(&p::get(&v, "installed").map_err(at(l))?) {
        out.push_str("This computer is not a compute node.\n");
        let l = status_line!("print(\"  k2-node: %s\"");
        let what = if p::truthy(&b) {
            let args = [
                p::get(&b, "path").map_err(at(l))?,
                p::get(&b, "source").map_err(at(l))?,
                p::or(p::get(&b, "version").map_err(at(l))?, s("?")),
            ];
            p::pct("%s (%s, %s)", &args).map_err(at(l))?
        } else {
            "not on this machine (Linux: downloaded and signature-checked on enable)".to_string()
        };
        out.push_str(&format!("  k2-node: {what}\n"));
        let l = status_line!("print(\"  root: %s\"");
        let e = p::get(&v, "elevation").map_err(at(l))?;
        out.push_str(&format!("  root: {}\n", elevation(&e).map_err(at(l))?));
        if p::truthy(&hc) {
            let l = status_line!("print(\"  home: %s is %s");
            let home = p::get(&hc, "home").map_err(at(l))?;
            let private = if p::truthy(&p::get(&hc, "private").map_err(at(l))?) {
                "private"
            } else {
                "readable by other users"
            };
            let mode = p::get(&hc, "mode").map_err(at(l))?;
            out.push_str(
                &p::pct("  home: %s is %s (mode %s)", &[home, s(private), mode]).map_err(at(l))?,
            );
            out.push('\n');
        }
        out.push_str(&format!("Enable: {cli} compute local enable [--controller URL --code-stdin --name NAME] [--private-home|--no-private-home] [--remote-admin off|policy|full]\n"));
        return Ok(());
    }
    let l = status_line!("n = v.get(\"node\")");
    let n = p::get(&v, "node")
        .map(|x| p::or(x, Value::Object(vec![])))
        .map_err(at(l))?;
    let l = status_line!("c = v.get(\"control\")");
    let c = p::get(&v, "control")
        .map(|x| p::or(x, Value::Object(vec![])))
        .map_err(at(l))?;
    let l = status_line!("print(\"node %s");
    let name = p::get_or(&n, "name", s("?")).map_err(at(l))?;
    let state = p::get_or(&n, "state", s("unknown")).map_err(at(l))?;
    let via = if p::truthy(&p::get(&n, "route").map_err(at(l))?) {
        let r = p::idx(&n, "route").map_err(at(l))?;
        p::str_of(&p::add(&s(" via "), &r).map_err(at(l))?)
    } else {
        String::new()
    };
    out.push_str(&p::pct("node %s: %s%s", &[name, state, s(&via)]).map_err(at(l))?);
    out.push('\n');
    let l = status_line!("print(\"control: %s");
    let cs = p::get_or(&c, "state", s("active")).map_err(at(l))?;
    let by = if p::truthy(&p::get(&c, "by").map_err(at(l))?) {
        let b = p::idx(&c, "by").map_err(at(l))?;
        p::pct(" by %s", &[b]).map_err(at(l))?
    } else {
        String::new()
    };
    out.push_str(&p::pct("control: %s%s", &[cs, s(&by)]).map_err(at(l))?);
    out.push('\n');
    let l = status_line!("if n.get(\"running\")");
    if p::truthy(&p::get(&n, "running").map_err(at(l))?) {
        let r = p::idx(&n, "running")
            .and_then(|r| p::join(" ", &r))
            .map_err(at(l))?;
        out.push_str(&format!("running: {r}\n"));
    }
    let l = status_line!("if n.get(\"last_error\")");
    if p::truthy(&p::get(&n, "last_error").map_err(at(l))?) {
        let e = p::idx(&n, "last_error").map_err(at(l))?;
        out.push_str(&format!("last error: {}\n", p::str_of(&e)));
    }
    let l = status_line!("if n.get(\"sas\")");
    if p::truthy(&p::get(&n, "sas").map_err(at(l))?) {
        let e = p::idx(&n, "sas").map_err(at(l))?;
        out.push_str(&format!(
            "waiting for confirmation, code {}\n",
            p::str_of(&e)
        ));
    }
    let l = status_line!("r = v.get(\"enrollResult\")");
    let r = p::get(&v, "enrollResult")
        .map(|x| p::or(x, Value::Object(vec![])))
        .map_err(at(l))?;
    let l = status_line!("if r and not r.get(\"ok\")");
    if p::truthy(&r) && !p::truthy(&p::get(&r, "ok").map_err(at(l))?) {
        let m = p::get(&r, "message").map_err(at(l))?;
        out.push_str(&format!("last enroll request failed: {}\n", p::str_of(&m)));
    }
    let l = status_line!("p = v.get(\"packages\")");
    let pk = p::get(&v, "packages")
        .map(|x| p::or(x, Value::Object(vec![])))
        .map_err(at(l))?;
    let l = status_line!("if p.get(\"approved\")");
    if p::truthy(&p::get(&pk, "approved").map_err(at(l))?) {
        let a = p::idx(&pk, "approved")
            .and_then(|a| p::join(" ", &a))
            .map_err(at(l))?;
        out.push_str(&format!("approved packages: {a}\n"));
    }
    let l = status_line!("if hc and not hc.get(\"private\")");
    if p::truthy(&hc) && !p::truthy(&p::get(&hc, "private").map_err(at(l))?) {
        let home = p::get(&hc, "home").map_err(at(l))?;
        let mode = p::get(&hc, "mode").map_err(at(l))?;
        out.push_str(
            &p::pct(
                "note: %s is readable by other users (mode %s); jobs can read files there that are readable by everyone",
                &[home, mode],
            )
            .map_err(at(l))?,
        );
        out.push('\n');
    }
    let l = status_line!("ra = v.get(\"remoteAdmin\")");
    let ra = p::get(&v, "remoteAdmin").map_err(at(l))?;
    let l = status_line!("print(\"remote admin: %s\"");
    out.push_str(&p::pct("remote admin: %s", &[p::or(ra, s("not set up"))]).map_err(at(l))?);
    out.push('\n');
    let l = status_line!("if v.get(\"remoteAdminCommand\")");
    if p::truthy(&p::get(&v, "remoteAdminCommand").map_err(at(l))?) {
        let cmd = p::idx(&v, "remoteAdminCommand").map_err(at(l))?;
        let text = p::pct(
            &format!(
                "  {} can't update this node. Turn it on: {cli} compute local remote-admin policy (or: %s)",
                product_name!()
            ),
            &[cmd],
        )
        .map_err(at(l))?;
        out.push_str(&text);
        out.push('\n');
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words() {
        assert_eq!(parse(&[]), Words::Ask(Ask::Status { json: false }));
        assert_eq!(
            parse(&["", "--json"]),
            Words::Ask(Ask::Status { json: true })
        );
        assert_eq!(parse(&["--json"]), Words::Usage(USAGE.into()));
        assert_eq!(parse(&["policy", "a"]), Words::Usage(POLICY_USAGE.into()));
        assert_eq!(
            parse(&["enable", "--remote-admin", "some"]),
            Words::Usage(LEVEL_LINE.into())
        );
        assert_eq!(parse(&["enable", "--remote-admin"]), Words::ShiftFailed);
        assert_eq!(parse(&["enable", "--x"]), Words::Usage(ENABLE_USAGE.into()));
        assert_eq!(
            parse(&["remote-admin"]),
            Words::Usage(REMOTE_ADMIN_USAGE.into())
        );
    }

    #[test]
    fn the_enable_body() {
        let e = Enable {
            controller: "https://c".into(),
            name: "mini".into(),
            private: Some(false),
            remote_admin: "full".into(),
            ..Enable::default()
        };
        assert_eq!(
            enable_body(&e, b"CODE.FP", b"T\xff"),
            r#"{"enroll": "CODE.FP", "ticket": "T\udcff", "controller": "https://c", "name": "mini", "privateHome": false, "remoteAdmin": "full"}"#
        );
    }

    #[test]
    fn the_lines() {
        assert_eq!(status_line!("v = json.load"), 3);
        assert_eq!(status_line!("print(\"  root: %s\""), 9);
        assert_eq!(status_line!("print(\"node %s"), 16);
        assert_eq!(status_line!("if v.get(\"remoteAdminCommand\")"), 28);
    }

    #[test]
    fn the_status() {
        let r = status_reader(br#"{"installed":false,"elevation":"helper","homeCheck":{"home":"/home/a","private":false,"mode":"0755"}}"#);
        assert_eq!(
            r.stdout,
            format!("This computer is not a compute node.\n  k2-node: not on this machine (Linux: downloaded and signature-checked on enable)\n  root: the root helper installs it (no password)\n  home: /home/a is readable by other users (mode 0755)\nEnable: {CLI_NAME} compute local enable [--controller URL --code-stdin --name NAME] [--private-home|--no-private-home] [--remote-admin off|policy|full]\n")
        );
        let r = status_reader(br#"{"installed":true,"node":{"name":"mini","state":"online","route":"lan","running":["j1","j2"]},"control":{"state":"paused","by":"owner"},"remoteAdmin":"policy"}"#);
        assert_eq!(
            r.stdout,
            "node mini: online via lan\ncontrol: paused by owner\nrunning: j1 j2\nremote admin: policy\n"
        );
        let r = status_reader(br#"{"installed":false,"elevation":[1]}"#);
        assert!(r.stderr.ends_with("TypeError: unhashable type: 'list'\n"));
    }
}
