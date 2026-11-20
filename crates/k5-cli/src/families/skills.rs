//! `k5 skills [list|create|profile|remove|regenerate]`: the workspace's
//! skill profiles, as the shell CLI's `cmd_skills` (`cli/src/wiki.sh`).
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]). `skills` is an open tool: no passport check
//!    comes after them.
//! 2. The verb (none is `list`) and its words: a missing name is the
//!    usage line, exit 1; a verb it doesn't know, its seven lines, exit 1.
//! 3. One request on the left of a pipe into the verb's Python reader,
//!    whose stderr is thrown away (`2>/dev/null`): an error, or anything
//!    the reader can't read, is exit 1 with nothing more on stderr
//!    (`regenerate`'s reader then ends with 0). `list`: `cli_request
//!    /cli/agents/list`, one line per skill. `create`: `cli_request_post
//!    /cli/agents/create`. `profile`: `cli_request /cli/agents/profile`.
//!    `remove` (`delete`): `cli_request_post /cli/agents/delete`.
//!    `regenerate`: `cli_request_post /cli/skills/regenerate`.
//! 4. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! A request with no answer is the transport's line, then the command
//! dies of SIGPIPE ([`super::end_in_pipe`]). The shell answers: `-h` or
//! `--help` anywhere (the front sends those on; the shell prints the
//! usage and sends nothing), `create`'s `--template`, `--from` or `--role` as the last word
//! (`$2: unbound variable`), a word that is not UTF-8, a port curl won't
//! take, and a `python3` the binary can't tell.

use super::{curl_port, ljust, pipefail, py_iter, Shown};
use crate::daemon;
use crate::exit::Exit;
use crate::families::remind::shared::{py_get, py_in};
use crate::out::json::{self, Options, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel};
use k2_client::cli_name;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["skills"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

pub const LIST: &str = "/cli/agents/list";
pub const CREATE: &str = "/cli/agents/create";
pub const PROFILE: &str = "/cli/agents/profile";
pub const DELETE: &str = "/cli/agents/delete";
pub const REGENERATE: &str = "/cli/skills/regenerate";

const USAGE: [&str; 7] = [
    concat!(
        "Usage: ",
        cli_name!(),
        " skills <list|create|profile|remove|regenerate> [options]"
    ),
    "  list                              Skill profiles in this workspace",
    "  create <name> [--template <src>]  Create a new skill profile",
    "                                    (optionally seeded from another skill)",
    "  profile <name>                    Print the skill's SKILL.md body",
    "  remove <name>                     Trash the skill profile",
    "  regenerate                        Refresh every skill's SKILL.md",
];

const CREATE_USAGE: [&str; 5] = [
    concat!(
        "Usage: ",
        cli_name!(),
        " skills create <name> [--template <existing-skill>] [--role \"...\"]"
    ),
    "  --template <name>   Copy an existing skill as the starting point. Any",
    "                      skill in this workspace can seed any other; templates",
    "                      aren't a separate kind post-Phase-2.5b.",
    "  --role \"...\"        Frontmatter role/description for the new skill.",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    List,
    Create,
    Profile,
    Remove,
    Regenerate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Send(Verb, Option<String>),
    Stop(Vec<&'static str>),
}

/// The words after `skills`. `None`: the shell's.
pub fn parse(words: &[&str]) -> Option<Ask> {
    let verb = match words.first().copied() {
        None | Some("") => "list",
        Some(v) => v,
    };
    let rest = words.get(1..).unwrap_or(&[]);
    let name = rest.first().copied().unwrap_or("");
    let named = |v: Verb, usage: &'static str| {
        if name.is_empty() {
            Ask::Stop(vec![usage])
        } else {
            Ask::Send(v, Some(format!("agent={}", quote(name))))
        }
    };
    Some(match verb {
        "list" => Ask::Send(Verb::List, None),
        "regenerate" => Ask::Send(Verb::Regenerate, None),
        "profile" => named(
            Verb::Profile,
            concat!("Usage: ", cli_name!(), " skills profile <name>"),
        ),
        "remove" | "delete" => named(
            Verb::Remove,
            concat!("Usage: ", cli_name!(), " skills remove <name>"),
        ),
        "create" => {
            let (mut name, mut source, mut role) = (String::new(), String::new(), String::new());
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "--template" | "--from" => {
                        source = rest.get(i + 1)?.to_string();
                        i += 2;
                    }
                    "--role" => {
                        role = rest.get(i + 1)?.to_string();
                        i += 2;
                    }
                    w if w.starts_with("--") => i += 1,
                    w => {
                        if name.is_empty() {
                            name = w.to_string();
                        }
                        i += 1;
                    }
                }
            }
            if name.is_empty() {
                return Some(Ask::Stop(CREATE_USAGE.to_vec()));
            }
            let mut p = format!("agent={}", quote(&name));
            if !role.is_empty() {
                p.push_str(&format!("&role={}", quote(&role)));
            }
            if !source.is_empty() {
                p.push_str(&format!("&template={}", quote(&source)));
            }
            Ask::Send(Verb::Create, Some(p))
        }
        _ => Ask::Stop(USAGE.to_vec()),
    })
}

/// `k5 skills ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let ask = parse(&words)?;
    if !python::family_native(python::here()) {
        return None;
    }
    let mut settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    settings.port = Some(settings.port_text.as_deref().and_then(curl_port)?);
    let (verb, param) = match ask {
        Ask::Stop(lines) => {
            for l in lines {
                out::stderr_line(l);
            }
            return Some(Exit::Error);
        }
        Ask::Send(v, p) => (v, p),
    };
    let would_refuse = passport::current(args).would_refuse();
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let mut closing = Closing::default();
    let params: Vec<&str> = param.iter().map(String::as_str).collect();
    let (endpoint, call) = match verb {
        Verb::List => (LIST, Call::Request(&params)),
        Verb::Profile => (PROFILE, Call::Request(&params)),
        Verb::Create => (CREATE, Call::RequestPost(&params)),
        Verb::Remove => (DELETE, Call::RequestPost(&params)),
        Verb::Regenerate => (REGENERATE, Call::RequestPost(&params)),
    };
    let r = client.call(Channel::Tcp, endpoint, call);
    let passport = client.passport().is_some();
    let piped = match r {
        Ok(r) => {
            closing.answer(&r, passport);
            super::Piped {
                body: super::piped_body(&r),
                exit: Exit::Ok,
            }
        }
        Err(e @ crate::transport::Error::NoAnswer(_)) => {
            out::stderr_line(&e.to_string());
            super::end_in_pipe()
        }
        Err(e) => super::Piped {
            body: Vec::new(),
            exit: Exit::from_curl(e.curl_code()),
        },
    };
    let shown = reader(verb, &String::from_utf8_lossy(&piped.body));
    let exit = pipefail(piped.exit, shown.print());
    closing.flush(&mut client, exit);
    Some(exit)
}

/// What a reader with its stderr thrown away shows: stdout, and its exit.
fn shown(out: String, exit: Exit) -> Shown {
    Shown {
        stdout: out,
        stderr: String::new(),
        exit,
    }
}

/// `isinstance(d, dict) and 'error' in d`.
fn dict_error(d: &Value) -> bool {
    matches!(d, Value::Object(p) if super::get(p, "error").is_some())
}

/// The verb's Python on what it read.
pub fn reader(verb: Verb, raw: &str) -> Shown {
    let mut out = String::new();
    let Ok(d) = json::parse(raw) else {
        // `regenerate`'s `except` doesn't exit 1.
        let exit = if verb == Verb::Regenerate {
            Exit::Ok
        } else {
            Exit::Error
        };
        return shown(out, exit);
    };
    match verb {
        Verb::Regenerate => {
            let exit = (|| -> Result<Exit, String> {
                if py_in("error", &d)? {
                    return Ok(Exit::Error);
                }
                let count = py_get(&d, "updated")?
                    .cloned()
                    .unwrap_or(Value::Int("0".into()));
                let n = py::percent_d(&count)?;
                out.push_str(&format!("Regenerated SKILL.md for {n} skill(s).\n"));
                Ok(Exit::Ok)
            })();
            shown(out, exit.unwrap_or(Exit::Ok))
        }
        _ if dict_error(&d) => shown(out, Exit::Error),
        Verb::Remove => {
            out.push_str("Removed skill (sent to recycle bin).\n");
            shown(out, Exit::Ok)
        }
        Verb::Create => match py_get(&d, "name") {
            Ok(nm) => {
                let nm = nm.map_or_else(|| "?".to_string(), py::str_of);
                out.push_str(&format!(
                    "Created skill: {nm}\n  Edit: .k2/skills/{nm}/SKILL.md\n"
                ));
                shown(out, Exit::Ok)
            }
            Err(_) => shown(out, Exit::Error),
        },
        Verb::Profile => {
            let body = match &d {
                Value::Object(p) => super::get(p, "profile").cloned().unwrap_or(Value::Null),
                other => other.clone(),
            };
            let text = match &body {
                Value::Str(t) if t.as_str().is_some() => py::str_of(&body),
                Value::Str(_) => return shown(out, Exit::Error),
                _ => {
                    let opts = Options {
                        indent: Some(2),
                        ..Options::default()
                    };
                    match json::to_string(&d, &opts) {
                        Ok(t) => t,
                        Err(_) => return shown(out, Exit::Error),
                    }
                }
            };
            out.push_str(&text);
            out.push('\n');
            shown(out, Exit::Ok)
        }
        Verb::List => {
            let exit = list(&d, &mut out).map_or(Exit::Error, |()| Exit::Ok);
            shown(out, exit)
        }
    }
}

/// `list`'s Python: `Err` where it raises (its message is thrown away).
fn list(d: &Value, out: &mut String) -> Result<(), ()> {
    let items = match d {
        Value::Array(_) => d.clone(),
        _ => py_get(d, "agents")
            .map_err(|_| ())?
            .cloned()
            .unwrap_or(Value::Array(Vec::new())),
    };
    if !py::truthy(&items) {
        out.push_str("No skills in this workspace.\n");
        return Ok(());
    }
    let items = py_iter(&items, 0).map_err(|_| ())?;
    let name_of = |s: &Value| -> Result<Value, ()> {
        Ok(py_get(s, "name")
            .map_err(|_| ())?
            .cloned()
            .unwrap_or_else(|| super::s("")))
    };
    let mut width = 0;
    for s in &items {
        let n = match name_of(s)? {
            Value::Str(t) => t.code_points().len(),
            Value::Array(a) => a.len(),
            Value::Object(p) => p.len(),
            _ => return Err(()),
        };
        width = width.max(n);
    }
    for s in &items {
        let nm = name_of(s)?;
        let Value::Str(_) = nm else {
            // `format(x, '<n')` takes only a string here.
            return Err(());
        };
        let role = py_get(s, "role")
            .map_err(|_| ())?
            .cloned()
            .unwrap_or_else(|| super::s(""));
        let atype = crate::families::remind::shared::or(
            py_get(s, "agentType").map_err(|_| ())?,
            Value::Null,
        );
        let atype = if py::truthy(&atype) {
            atype
        } else {
            crate::families::remind::shared::or(
                py_get(s, "agent_type").map_err(|_| ())?,
                super::s("skill"),
            )
        };
        out.push_str(&format!(
            "  {}  [{}]  {}\n",
            ljust(&py::str_of(&nm), width),
            py::str_of(&atype),
            py::str_of(&role)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_shells() {
        assert_eq!(parse(&[]), Some(Ask::Send(Verb::List, None)));
        assert_eq!(parse(&["create", "a", "--role"]), None);
        assert_eq!(
            parse(&["create", "--x", "a b", "c", "--from", "t", "--role", "r"]),
            Some(Ask::Send(
                Verb::Create,
                Some("agent=a%20b&role=r&template=t".into())
            ))
        );
        assert_eq!(parse(&["create"]), Some(Ask::Stop(CREATE_USAGE.to_vec())));
        assert_eq!(parse(&["nope"]), Some(Ask::Stop(USAGE.to_vec())));
        assert!(matches!(parse(&["delete"]), Some(Ask::Stop(l)) if l.len() == 1));
    }

    #[test]
    fn list_pads_names_as_python_does() {
        let s = reader(
            Verb::List,
            r#"{"agents":[{"name":"ab","role":"r","agent_type":"t"},{"name":"é","agentType":null}]}"#,
        );
        assert_eq!(s.stdout, "  ab  [t]  r\n  é   [skill]  \n");
        assert_eq!(s.exit, Exit::Ok);
        let s = reader(Verb::List, r#"[{"name":"a"},{"name":5}]"#);
        assert_eq!((s.stdout.as_str(), s.exit), ("", Exit::Error));
        let s = reader(Verb::Regenerate, r#"{"updated":"x"}"#);
        assert_eq!((s.stdout.as_str(), s.exit), ("", Exit::Ok));
        let s = reader(Verb::Regenerate, r#"{"updated":3}"#);
        assert_eq!(s.stdout, "Regenerated SKILL.md for 3 skill(s).\n");
    }
}
