//! `k5 clone ...`: clone proposals, as the shell CLI's `cmd_clone`
//! answers them (`cli/src/clone.sh`): `proposals` (`proposal`),
//! `propose`, `withdraw` and `deny`, each one `_compute_call` (the cell
//! socket first in a session; `deny` over TCP) and its Python's lines.
//! `clone` alone (or `help`) prints the help.
//!
//! The clone engine's verbs stay the shell's ([`run`] returns `None`):
//! `preview`, `to`, `from`, `here`, `approve` and a bare `<ws> --dest`
//! run `_clone_py`, a Python client of this server and a remote one
//! (chunked transfers, a remote token on stdin, a vault socket and
//! passwords on `/dev/tty`). So do `--token` and `--token-stdin` on any
//! verb, a value flag as the last word (`shift 2` out of range), any
//! `-h` or `--help` (the front's), a word that is not UTF-8, the shell's
//! own settings in the environment (`K2_CJ_*`, `_CP_404_EXIT`, ...), a
//! port curl won't take, and a `python3` the binary can't tell.
//!
//! The usage stops are `_clone_die`'s one JSON line on stderr
//! (`json.dumps`, Python's defaults), exit 2; a passport on a person's
//! verb is its `clone_agents_propose` line, exit 3.

use super::compute::writes::{body_of, print_body, shell_settings, Cp};
use super::heartbeat::py::{dget_or, dget_v, index, iter, left, loads, str_plus, Py, R};
use super::{curl_port, s};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use k2_client::cli_name;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["clone"];
/// The binary answers the forms [`run`] takes; the rest go to the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

pub const LIST: &str = "/cli/clone-proposal/list";
pub const NEW: &str = "/cli/clone-proposal/new";
pub const WITHDRAW: &str = "/cli/clone-proposal/withdraw";
pub const DENY: &str = "/cli/clone-proposal/deny";

/// `_clone_parse`'s options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opts {
    pub no_threads: bool,
    pub live_only: bool,
    pub json: bool,
    pub all: bool,
    pub copy: Vec<String>,
    pub blank: Vec<String>,
    pub skip: Vec<String>,
    pub since: Option<String>,
    pub dest: String,
    pub why: String,
    pub to: String,
    pub workspace: String,
    pub reason: String,
    pub args: Vec<String>,
}

/// What a command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    Help,
    /// `_clone_die <code> <hint> <rc>`.
    Die(&'static str, String, Exit),
    Proposals(Opts),
    Propose(Opts),
    Withdraw(String),
    Deny(String, String),
}

/// `lst(k)`: the comma list's stripped, non-empty names.
fn names(v: &str) -> Vec<String> {
    v.split(',')
        .map(|n| n.trim_matches(super::py_space))
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect()
}

/// `_clone_parse`: `None` for a form the shell must answer.
fn opts(words: &[&str]) -> Option<Result<Opts, Form>> {
    let mut o = Opts::default();
    let (mut copy, mut blank, mut skip) = (String::new(), String::new(), String::new());
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied();
        let two = |slot: &mut String| -> Option<()> {
            *slot = next?.to_string();
            Some(())
        };
        let append = |acc: &mut String| -> Option<()> {
            let v = next?;
            if !acc.is_empty() {
                acc.push(',');
            }
            acc.push_str(v);
            Some(())
        };
        match w {
            "--no-threads" => o.no_threads = true,
            "--own" | "--env-files" | "--yes" | "-y" | "--mine" => {}
            "--live-only" => o.live_only = true,
            "--json" => o.json = true,
            "--all" => o.all = true,
            "--threads-since" | "--since-days" => {
                let mut v = String::new();
                two(&mut v)?;
                o.since = Some(v);
                i += 2;
                continue;
            }
            "--copy" | "--blank" | "--skip" => {
                append(match w {
                    "--copy" => &mut copy,
                    "--blank" => &mut blank,
                    _ => &mut skip,
                })?;
                i += 2;
                continue;
            }
            "--dest" | "--to" | "--why" | "--reason" | "--workspace" | "--host" => {
                let mut sink = String::new();
                two(match w {
                    "--dest" => &mut o.dest,
                    "--to" => &mut o.to,
                    "--why" => &mut o.why,
                    "--reason" => &mut o.reason,
                    "--workspace" => &mut o.workspace,
                    _ => &mut sink,
                })?;
                i += 2;
                continue;
            }
            "--token" | "--token-stdin" | "-h" | "--help" => return None,
            w if w.starts_with('-') => {
                return Some(Err(Form::Die(
                    "usage",
                    format!("unknown flag '{w}' ({} clone --help)", cli_name!()),
                    Exit::Usage,
                )))
            }
            w => o.args.push(w.to_string()),
        }
        i += 1;
    }
    if let Some(since) = &o.since {
        if since.is_empty() || !since.bytes().all(|b| b.is_ascii_digit()) {
            return Some(Err(Form::Die(
                "usage",
                "--threads-since takes a number of days".into(),
                Exit::Usage,
            )));
        }
    }
    o.copy = names(&copy);
    o.blank = names(&blank);
    o.skip = names(&skip);
    Some(Ok(o))
}

/// The words after `clone`; `None`: the shell's.
pub fn parse(words: &[&str]) -> Option<Form> {
    let c = cli_name!();
    let sub = words.first().copied().unwrap_or("");
    if matches!(sub, "" | "-h" | "--help" | "help") {
        return Some(Form::Help);
    }
    if !matches!(
        sub,
        "proposals" | "proposal" | "propose" | "withdraw" | "deny"
    ) {
        return None;
    }
    let o = match opts(&words[1..])? {
        Ok(o) => o,
        Err(f) => return Some(f),
    };
    let usage = |hint: String| Some(Form::Die("usage", hint, Exit::Usage));
    match sub {
        "propose" => {
            if !o.args.is_empty() {
                return usage(format!(
                    "usage: {c} clone propose --to <host|here> --why \"\u{2026}\""
                ));
            }
            Some(Form::Propose(o))
        }
        "withdraw" => {
            if o.args.len() != 1 {
                return usage(format!("usage: {c} clone withdraw <id>"));
            }
            Some(Form::Withdraw(o.args[0].clone()))
        }
        "deny" => {
            if o.args.len() != 1 {
                return usage(format!("usage: {c} clone deny <id> [--reason TEXT]"));
            }
            let reason = o.reason.trim_end_matches('\n').to_string();
            Some(Form::Deny(o.args[0].clone(), reason))
        }
        _ => Some(Form::Proposals(o)),
    }
}

/// `_clone_die`'s line: `json.dumps({"error": code, "hint": hint})`.
fn die_line(code: &str, hint: &str) -> String {
    let v = Value::Object(vec![
        (Text::from("error"), s(code)),
        (Text::from("hint"), s(hint)),
    ]);
    json::to_string(&v, &Options::default()).unwrap_or_default()
}

/// `k5 clone ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let form = parse(&words)?;
    if shell_settings(std::env::vars_os()) || std::env::var_os("_K2_ARGS_JSON").is_some() {
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
    Some(run_with(args, form, settings))
}

/// `_clone_person_guard`: a passport (or any `HOOK_TOKEN`) refuses.
fn person_guard() -> Option<Exit> {
    let set = k2_client::env_names::first_nonempty_os(k2_client::env_names::HOOK_TOKEN).is_some();
    if !set {
        return None;
    }
    out::stderr_line(&die_line(
        "clone_agents_propose",
        &format!(
            "agents can't clone; propose it and a person approves: {} clone propose --to <host|here> --why \"\u{2026}\"",
            cli_name!()
        ),
    ));
    Some(Exit::Refused)
}

fn run_with(args: &[OsString], form: Form, mut settings: Settings) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    match &form {
        Form::Help => {
            let text = crate::text::help("clone").unwrap_or_default();
            return super::Shown::out(text).print();
        }
        Form::Die(code, hint, exit) => {
            out::stderr_line(&die_line(code, hint));
            return *exit;
        }
        Form::Deny(..) => {
            if let Some(stop) = person_guard() {
                return stop;
            }
        }
        _ => {}
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut cp = Cp {
        client: &mut client,
        closing: Closing::default(),
        project: settings.project.clone(),
        as_json: false,
    };
    let exit = answer(&mut cp, form);
    let Cp { closing, .. } = cp;
    closing.flush(&mut client, exit);
    exit
}

fn answer(cp: &mut Cp, form: Form) -> Exit {
    match form {
        Form::Proposals(o) => {
            let q = if o.all { "status=all" } else { "status=open" };
            let body = match cp.call(false, LIST, q, false) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if o.json {
                return print_body(&body);
            }
            proposals(&String::from_utf8_lossy(&body)).print()
        }
        Form::Propose(o) => {
            if o.to.is_empty() {
                out::stderr_line("usage: --to <host|here>");
                return Exit::Usage;
            }
            let list = |v: &[String]| Value::Array(v.iter().map(|x| s(x)).collect());
            let mut b = vec![
                (Text::from("to"), s(&o.to)),
                (Text::from("threads"), Value::Bool(!o.no_threads)),
                (Text::from("allHistory"), Value::Bool(!o.live_only)),
                (Text::from("copy"), list(&o.copy)),
                (Text::from("blank"), list(&o.blank)),
                (Text::from("skip"), list(&o.skip)),
            ];
            for (k, v) in [
                ("why", &o.why),
                ("dest", &o.dest),
                ("workspace", &o.workspace),
            ] {
                if !v.is_empty() {
                    b.push((Text::from(k), s(v)));
                }
            }
            if let Some(n) = &o.since {
                let n = n.trim_start_matches('0');
                if !n.is_empty() {
                    b.push((Text::from("sinceDays"), Value::Int(n.to_string())));
                }
            }
            let body = json::to_string(&Value::Object(b), &Options::default()).unwrap_or_default();
            let answer = match cp.call(true, NEW, &body, false) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if o.json {
                return print_body(&answer);
            }
            proposed(&String::from_utf8_lossy(&answer)).print()
        }
        Form::Withdraw(id) => {
            let body = body_of(&[("proposal", &id)]);
            match cp.call(true, WITHDRAW, &body, false) {
                Ok(b) => decided(&String::from_utf8_lossy(&b)).print(),
                Err(e) => e,
            }
        }
        Form::Deny(id, reason) => {
            let body = body_of(&[("proposal", &id), ("reason", &reason)]);
            match cp.call(true, DENY, &body, true) {
                Ok(b) => decided(&String::from_utf8_lossy(&b)).print(),
                Err(e) => e,
            }
        }
        Form::Help | Form::Die(..) => Exit::Ok,
    }
}

fn st(v: &Value) -> String {
    py::str_of(v)
}

/// The `proposals` reader.
pub fn proposals(raw: &str) -> super::Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let v = loads(raw)?;
        let rs = super::heartbeat::py::or(dget_v(&v, "proposals")?, Value::Array(Vec::new()));
        if !py::truthy(&rs) {
            p.out("No clone proposals.");
            return Ok(Exit::Ok);
        }
        for r in iter(&rs)? {
            let id = index(&r, "id")?;
            let status = index(&r, "status")?;
            let summary = index(&r, "summary")?;
            p.out(format!(
                "{}  {} {}",
                st(&id),
                left(&s(&st(&status)), 9)?,
                st(&summary)
            ));
            let by = index(&r, "proposedBy")?;
            let why = if py::truthy(&dget_v(&r, "why")?) {
                str_plus(": ", &index(&r, "why")?)?
            } else {
                String::new()
            };
            p.out(format!("    by {}{why}", st(&by)));
            if py::truthy(&dget_v(&r, "decidedBy")?) {
                p.out(format!(
                    "    {} by {}",
                    st(&index(&r, "status")?),
                    st(&index(&r, "decidedBy")?)
                ));
            }
        }
        Ok(Exit::Ok)
    })();
    p.raised(r, 3)
}

/// The `propose` reader.
pub fn proposed(raw: &str) -> super::Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let v = loads(raw)?;
        let prop = index(&v, "proposal")?;
        p.out(format!(
            "{} held: {}",
            st(&index(&prop, "id")?),
            st(&index(&prop, "summary")?)
        ));
        let hint = dget_or(&v, "hint", s(""))?;
        p.out(str_plus("  ", &hint)?);
        Ok(Exit::Ok)
    })();
    p.raised(r, 4)
}

/// The `withdraw` and `deny` reader.
pub fn decided(raw: &str) -> super::Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let prop = index(&loads(raw)?, "proposal")?;
        p.out(format!(
            "{} {}",
            st(&index(&prop, "id")?),
            st(&index(&prop, "status")?)
        ));
        Ok(Exit::Ok)
    })();
    p.raised(r, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forms() {
        assert_eq!(parse(&[]), Some(Form::Help));
        assert_eq!(parse(&["preview", "w"]), None);
        assert_eq!(parse(&["propose", "--to", "here", "--token", "t"]), None);
        assert_eq!(parse(&["proposals", "--why"]), None);
        assert!(matches!(
            parse(&["withdraw"]),
            Some(Form::Die("usage", _, Exit::Usage))
        ));
        let Some(Form::Propose(o)) =
            parse(&["propose", "--to", "h", "--copy", "a, b", "--copy", ",c"])
        else {
            panic!("not propose");
        };
        assert_eq!(o.copy, vec!["a", "b", "c"]);
        assert_eq!(
            die_line("usage", "x \u{2026}"),
            "{\"error\": \"usage\", \"hint\": \"x \\u2026\"}"
        );
    }
}
