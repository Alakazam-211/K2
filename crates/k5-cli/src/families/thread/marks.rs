//! `k5 thread read-all <agent>` and `k5 thread unread`, as
//! `cmd_thread_read_all` and `cmd_thread_unread` do them
//! (`cli/src/browser.sh`): a person marks all of an agent's Threads read
//! (`POST /cli/thread/read-all`), or reads their own unread and waiting
//! counts (`GET /cli/thread/unread`). An agent's passport gets 403.
//!
//! Each drops the command line's first word, whatever it is (the shell's
//! `shift # "read-all"`), before it reads the rest. `unread --kind` as the
//! last word is [`super::post::needs_value`]'s line and exit 2 (K2's D6).

use super::super::msg::py::eq;
use super::super::msg::send::echo;
use super::super::msg::send_once::held;
use super::super::{py_len, Raise};
use super::post::echo_out;
use super::py::{self, dumps, get_at, get_or, loads, not_ok, or, s};
use crate::exit::Exit;
use crate::out::json::{Text, Value};
use crate::out::py::{str_of, truthy, type_name};
use crate::out::{self};
use crate::prelude::Closing;
use crate::transport::encode::quote;
use crate::transport::{Call, Client, Error};
use k2_client::brand::CLI_NAME;

/// The routes.
pub const READ_ALL: &str = "/cli/thread/read-all";
pub const UNREAD: &str = "/cli/thread/unread";

/// The words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    ReadAll { agent: String, json: bool },
    Unread { kind: String, json: bool },
}

/// The words, or the lines and exit that stop the command first.
pub type Parsed = Result<Mark, (String, Exit)>;

/// The verb's words (`verb`: the word [`super::first_word`] found).
pub fn parse(verb: &str, words: &[&str]) -> Parsed {
    let rest = words.get(1..).unwrap_or(&[]);
    let mut json = false;
    if verb == "read-all" {
        let mut agent = String::new();
        let form = format!("({CLI_NAME} thread read-all <agent> [--json])");
        for w in rest {
            match *w {
                "--json" => json = true,
                w if w.starts_with('-') => {
                    return Err((format!("Error: unknown flag '{w}' {form}\n"), Exit::Error));
                }
                w => {
                    if !agent.is_empty() {
                        return Err((format!("Error: one agent at a time {form}\n"), Exit::Error));
                    }
                    agent = w.to_string();
                }
            }
        }
        if agent.is_empty() {
            return Err((
                format!(
                    "Error: agent required\nUsage: {CLI_NAME} thread read-all <agent> [--json]   # mark all of an agent's Thread messages read for you\n"
                ),
                Exit::Error,
            ));
        }
        return Ok(Mark::ReadAll { agent, json });
    }
    let mut kind = String::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--kind" => {
                let Some(v) = rest.get(i + 1) else {
                    return Err((super::post::needs_value("--kind"), Exit::Usage));
                };
                kind = v.to_string();
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            w => {
                return Err((
                    format!(
                        "Error: unknown argument '{w}' ({CLI_NAME} thread unread [--kind thread|project] [--json])\n"
                    ),
                    Exit::Error,
                ))
            }
        }
    }
    Ok(Mark::Unread { kind, json })
}

/// The verb (its words read; the lines that stop it printed here).
pub fn run(client: &mut Client, closing: &mut Closing, parsed: Parsed) -> Exit {
    let m = match parsed {
        Ok(m) => m,
        Err((text, exit)) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
            return exit;
        }
    };
    let passport = client.passport().is_some();
    let (result, json) = match &m {
        Mark::ReadAll { agent, json } => {
            let body = dumps(&Value::Object(vec![(Text::from("agent"), s(agent))]));
            (
                client.send(READ_ALL, Call::PostJson(body.as_bytes())),
                *json,
            )
        }
        Mark::Unread { kind, json } => {
            let q = if kind.is_empty() {
                String::new()
            } else {
                format!("kind={}", quote(kind))
            };
            (client.send(UNREAD, Call::Request(&[q.as_str()])), *json)
        }
    };
    let response = match result {
        Ok(r) => {
            closing.answer(&r, passport);
            held(&r.body)
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            return Exit::Error;
        }
        Err(e) => return Exit::from_curl(e.curl_code()),
    };
    if json {
        return echo_out(&response);
    }
    let raw = echo(&response);
    match &m {
        Mark::ReadAll { agent, .. } => marked(&raw, agent).print(),
        Mark::Unread { .. } => counts(&raw).print(),
    }
}

/// A reader: not JSON is written back as it came, exit 2; an answer
/// whose `ok` is not true is the error line, exit 2; else `f`.
fn read_with(raw: &[u8], f: impl FnOnce(&Value, &mut py::Shown) -> Result<(), Raise>) -> py::Shown {
    let mut o = py::Shown::default();
    let Some(d) = loads(raw) else {
        o.stdout = raw.to_vec();
        return o.exit(2);
    };
    let run = |o: &mut py::Shown| -> Result<i32, Raise> {
        if not_ok(&d, 9)? {
            let (line, _) = py::error_line(&d, 10)?;
            o.err(&line);
            return Ok(2);
        }
        f(&d, o)?;
        Ok(0)
    };
    match run(&mut o) {
        Ok(code) => o.exit(code),
        Err(r) => o.raised(&r),
    }
}

/// `read-all`'s Python.
pub fn marked(raw: &[u8], agent: &str) -> py::Shown {
    read_with(raw, |d, o| {
        let marked = or(get_at(d, "marked", 17)?, Value::Array(Vec::new()));
        let n = py_len(&marked, 17)?;
        if n > 0 {
            let unread = get_or(d, "unread", Value::Int("0".into()), 19)?;
            o.line(&format!(
                "{agent}: marked {n} Thread{} read; unread now {}",
                if n == 1 { "" } else { "s" },
                str_of(&unread)
            ));
        } else {
            o.line(&format!("{agent}: nothing unread"));
        }
        let w = get_or(d, "waiting", Value::Int("0".into()), 22)?;
        if truthy(&w) {
            let one = eq(&w, &Value::Int("1".into()));
            o.line(&format!(
                "  {} card{} still waiting on you (answer {} to clear)",
                str_of(&w),
                if one { "" } else { "s" },
                if one { "it" } else { "them" }
            ));
        }
        Ok(())
    })
}

/// `n > 99` for a count the reader takes as a number.
fn over_99(n: &Value, line: u32) -> Result<bool, Raise> {
    match n {
        Value::Bool(_) => Ok(false),
        Value::Int(d) => Ok(d.parse::<i128>().map_or(!d.starts_with('-'), |v| v > 99)),
        Value::Float(f) => Ok(*f > 99.0),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '>' not supported between instances of '{}' and 'int'",
                type_name(other)
            ),
        )),
    }
}

/// `unread`'s Python.
pub fn counts(raw: &[u8]) -> py::Shown {
    read_with(raw, |d, o| {
        let zero = || Value::Int("0".into());
        o.line(&format!(
            "{}: {} unread+waiting (badge), {} waiting",
            str_of(&get_or(d, "person", s(""), 17)?),
            str_of(&get_or(d, "total", zero(), 17)?),
            str_of(&get_or(d, "waitingTotal", zero(), 17)?)
        ));
        for c in py::items(get_at(d, "byConversation", 18)?, 18)? {
            let n = get_or(&c, "unread", zero(), 19)?;
            let shown = if over_99(&n, 20)? {
                "99+".to_string()
            } else {
                str_of(&n)
            };
            let mut flags = Vec::new();
            let waiting = get_at(&c, "waiting", 22)?;
            if truthy(&waiting) {
                flags.push(format!("{} waiting", str_of(&waiting)));
            }
            if truthy(&get_at(&c, "muted", 24)?) {
                flags.push("muted".to_string());
            }
            let kind = get_or(&c, "kind", s(""), 26)?;
            let id = get_or(&c, "id", s(""), 26)?;
            let flags = if flags.is_empty() {
                String::new()
            } else {
                format!("  ({})", flags.join(", "))
            };
            o.line(&format!(
                "  {} {}  unread={shown}{flags}",
                super::super::ljust(&str_of(&kind), 7),
                str_of(&id)
            ));
        }
        Ok(())
    })
}
