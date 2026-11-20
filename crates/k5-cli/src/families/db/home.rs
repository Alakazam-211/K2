//! `k5 db home` (`_db_home`) and `k5 db cap` (`_db_cap`): the shell's
//! curl calls (`cli_request`, `cli_request_post`, `cli_post_json`, over
//! TCP), each held in `out="$(…)"`, then a `python3 -c` reader of
//! `$out`. A call with no answer is the transport's line and exit 1 (the
//! main shell's USR1 trap); any other failed call is curl's code.

use super::py::{
    at, contains, dumps_ascii, dumps_spaced, eq, err_line, g, is_false, obj, or, st, truthy,
};
use super::{usage, Form, Words};
use crate::exit::Exit;
use crate::families::{raised, s, Raise, Shown};
use crate::out;
use crate::out::json::{self, Value};
use crate::out::py;
use crate::prelude::Closing;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Client, Error, Response};
use k2_client::{cli_name, product_name};

/// `_db_home`'s words, after `home`.
pub fn parse(words: &[&str]) -> Form {
    let mut a = Words::new(words);
    let (mut keep, mut json) = (String::new(), false);
    while let Some(w) = a.first() {
        match w {
            "--keep" => {
                keep = a.at(1).to_string();
                if !a.shift(2) {
                    return usage("--keep needs k2.db or k2so.db");
                }
            }
            "--json" => {
                json = true;
                a.shift(1);
            }
            "-h" | "--help" | "help" => return Form::Help,
            _ if w.starts_with("--keep=") => {
                keep = w["--keep=".len()..].to_string();
                a.shift(1);
            }
            _ => {
                return usage(concat!(
                    cli_name!(),
                    " db home [--keep k2.db|k2so.db] [--json]"
                ))
            }
        }
    }
    if !keep.is_empty() && !matches!(keep.as_str(), "k2.db" | "k2so.db") {
        return usage("--keep must be k2.db or k2so.db");
    }
    Form::Home { keep, json }
}

/// `_db_cap`'s check of its value: a number of at most six digits, or
/// `default`.
pub fn cap_form(val: String, json: bool) -> Form {
    if val.is_empty() || val == "default" {
        return Form::Cap { val, json };
    }
    if !val.bytes().all(|b| b.is_ascii_digit()) {
        return usage(concat!(
            cli_name!(),
            " db cap takes a whole number (0 = unlimited) or 'default'"
        ));
    }
    if val.chars().count() > 6 {
        return usage(concat!(
            cli_name!(),
            " db cap: at most 999999 (0 = unlimited)"
        ));
    }
    // `$((10#$val))`.
    let n: u64 = val.parse().unwrap_or(0);
    Form::Cap {
        val: n.to_string(),
        json,
    }
}

/// `"$(call)"`: the answer's text, NULs and trailing newlines dropped, or
/// the exit the call ends the run with.
fn held(
    client: &mut Client,
    closing: &mut Closing,
    r: Result<Response, Error>,
) -> Result<String, Exit> {
    let passport = client.passport().is_some();
    match r {
        Ok(resp) => {
            closing.answer(&resp, passport);
            let mut body = super::super::without_nuls(&resp.body);
            while body.last() == Some(&b'\n') {
                body.pop();
            }
            Ok(String::from_utf8_lossy(&body).into_owned())
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Err(Exit::Error)
        }
        Err(e) => Err(Exit::from_curl(e.curl_code())),
    }
}

/// What a `python3 -c` reader printed, or its traceback.
struct Reader {
    stdout: String,
    stderr: String,
}

impl Reader {
    fn new() -> Reader {
        Reader {
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn print(&mut self, t: impl AsRef<str>) {
        self.stdout.push_str(t.as_ref());
        self.stdout.push('\n');
    }

    fn eprint(&mut self, t: impl AsRef<str>) {
        self.stderr.push_str(t.as_ref());
        self.stderr.push('\n');
    }

    fn end(self, r: Result<Exit, Raise>) -> Exit {
        let shown = match r {
            Ok(exit) => Shown {
                stdout: self.stdout,
                stderr: self.stderr,
                exit,
            },
            Err(e) => raised(self.stdout, self.stderr, &e),
        };
        shown.print()
    }
}

/// `_db_home`: the call and its reader.
pub fn home(client: &mut Client, closing: &mut Closing, keep: &str, json: bool) -> Exit {
    let r = if keep.is_empty() {
        client.call(Channel::Tcp, "/cli/db-home", Call::Request(&[]))
    } else {
        let q = format!("keep={}", quote(keep));
        client.call(Channel::Tcp, "/cli/db-home/keep", Call::RequestPost(&[&q]))
    };
    let raw = match held(client, closing, r) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let mut rd = Reader::new();
    let r = home_reader(&mut rd, &raw, json);
    rd.end(r)
}

/// The lines of `_db_home`'s reader (line 1 is the empty one after
/// `python3 -c '`).
mod home_line {
    pub const JSON_OK: u32 = 9;
    pub const ERROR: u32 = 10;
    pub const ERROR_GET: u32 = 12;
    pub const MESSAGE: u32 = 14;
    pub const FILE: u32 = 17;
    pub const BLOCKED: u32 = 22;
    pub const CHOICE: u32 = 24;
}

fn home_reader(rd: &mut Reader, raw: &str, json_out: bool) -> Result<Exit, Raise> {
    let Ok(v) = json::parse(raw) else {
        rd.print(raw);
        return Ok(Exit::Error);
    };
    if json_out {
        rd.print(dumps_spaced(&v));
        let ok = match g(&v, "ok", home_line::JSON_OK)? {
            Some(x) => py::truthy(x),
            None => true,
        };
        return Ok(if ok { Exit::Ok } else { Exit::Refused });
    }
    let err = g(&v, "error", home_line::ERROR)?;
    if truthy(err) {
        let e = err.unwrap_or(&Value::Null);
        let l = home_line::ERROR_GET;
        let line = format!("Error ({}): {}", st(g(e, "code", l)?), st(g(e, "hint", l)?));
        rd.eprint(line);
        return Ok(if super::py::is_str(at(e, "code"), "usage") {
            Exit::Usage
        } else {
            Exit::Refused
        });
    }
    if contains(&v, "message", home_line::MESSAGE)? {
        rd.print(st(at(&v, "message")));
        return Ok(Exit::Ok);
    }
    let h = or(at(&v, "homeDb"), Value::Object(Vec::new()));
    let l = home_line::FILE;
    let file = or(g(&h, "file", l)?, s("?"));
    let outcome = or(at(&h, "outcome"), s("not started"));
    rd.print(format!(
        concat!(product_name!(), " database:  {} ({})"),
        py::str_of(&file),
        py::str_of(&outcome)
    ));
    if truthy(at(&h, "reason")) {
        rd.print(format!("why:          {}", st(at(&h, "reason"))));
    }
    if truthy(at(&h, "markerAt")) {
        rd.print(format!("switched at:  {}", st(at(&h, "markerAt"))));
    }
    if truthy(at(&h, "held")) {
        let dir = match &h {
            Value::Object(pairs) if pairs.iter().any(|(k, _)| k.as_str() == Some("dir")) => {
                st(at(&h, "dir"))
            }
            _ => "~/.k2".to_string(),
        };
        rd.print(format!(
            "on hold:      {dir}/db-switch-hold exists (the switch waits)"
        ));
    }
    let b = or(at(&h, "blocked"), Value::Object(Vec::new()));
    if truthy(g(&b, "message", home_line::BLOCKED)?) {
        let m = at(&b, "message").unwrap_or(&Value::Null);
        match m {
            Value::Str(_) => rd.print(format!("\n{}", py::str_of(m))),
            other => {
                return Err(Raise::at(
                    home_line::BLOCKED,
                    format!(
                        "TypeError: can only concatenate str (not \"{}\") to str",
                        py::type_name(other)
                    ),
                ))
            }
        }
    }
    let c = or(at(&h, "choice"), Value::Object(Vec::new()));
    if truthy(g(&c, "keep", home_line::CHOICE)?) {
        rd.print(format!(
            concat!(
                "\nChosen: keep {} (applies at the next ",
                product_name!(),
                " start)."
            ),
            st(at(&c, "keep"))
        ));
    }
    Ok(Exit::Ok)
}

/// `_db_cap <val> <json>`: show the cap (no value), or set it.
pub fn cap(
    client: &mut Client,
    closing: &mut Closing,
    project: &str,
    val: &str,
    json: bool,
) -> Exit {
    if val.is_empty() {
        let q = format!("project={}", quote(project));
        let r = client.call(Channel::Tcp, "/cli/db/list", Call::Request(&[&q]));
        let raw = match held(client, closing, r) {
            Ok(t) => t,
            Err(e) => return e,
        };
        let mut rd = Reader::new();
        let r = cap_show(&mut rd, &raw, json);
        return rd.end(r);
    }
    let body = dumps_ascii(&obj(vec![
        ("project", s(project)),
        ("fields", obj(vec![("db_active_cap", s(val))])),
    ]));
    let r = client.call(
        Channel::Tcp,
        "/cli/workspace/set",
        Call::PostJson(body.as_bytes()),
    );
    let raw = match r {
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            return Exit::Error;
        }
        Err(_) => {
            out::stderr_line(&err_line(
                "error",
                concat!("could not reach the ", product_name!(), " daemon"),
            ));
            return Exit::Error;
        }
        r => match held(client, closing, r) {
            Ok(t) => t,
            Err(e) => return e,
        },
    };
    let mut rd = Reader::new();
    let r = cap_set(&mut rd, &raw, val, json);
    rd.end(r)
}

mod cap_line {
    pub const ERROR: u32 = 8;
    pub const SET_ERROR: u32 = 8;
    pub const SET_TOKEN: u32 = 11;
    pub const SET_CAP: u32 = 14;
}

fn cap_show(rd: &mut Reader, raw: &str, json_out: bool) -> Result<Exit, Raise> {
    let Ok(v) = json::parse(raw) else {
        rd.print(raw);
        return Ok(Exit::Error);
    };
    let err = g(&v, "error", cap_line::ERROR)?;
    if truthy(err) {
        let e = match err {
            Some(e @ Value::Object(_)) => e.clone(),
            other => obj(vec![("code", s("error")), ("hint", s(&st(other)))]),
        };
        rd.eprint(dumps_ascii(&obj(vec![("error", e.clone())])));
        let code = at(&e, "code");
        let refused = ["forbidden", "owner_only", "unsupported"]
            .iter()
            .any(|c| super::py::is_str(code, c));
        return Ok(if refused { Exit::Refused } else { Exit::Error });
    }
    let cap = at(&v, "cap").cloned().unwrap_or(Value::Null);
    let zero = eq(&cap, &Value::Int("0".into()));
    if json_out {
        rd.print(dumps_ascii(&obj(vec![
            ("ok", Value::Bool(true)),
            ("cap", cap),
            ("unlimited", Value::Bool(zero)),
        ])));
        return Ok(Exit::Ok);
    }
    rd.print(format!(
        "database cap: {}",
        if zero {
            "unlimited (0)".to_string()
        } else {
            py::str_of(&cap)
        }
    ));
    rd.print(concat!(
        "the owner sets it: ",
        cli_name!(),
        " db cap <n> [--workspace WS]   (0 = unlimited, default = back to 25)"
    ));
    Ok(Exit::Ok)
}

fn cap_set(rd: &mut Reader, raw: &str, stored: &str, json_out: bool) -> Result<Exit, Raise> {
    let Ok(v) = json::parse(raw) else {
        let head: String = raw.chars().take(200).collect();
        let hint = if head.is_empty() {
            concat!("no answer from the ", product_name!(), " daemon").to_string()
        } else {
            head
        };
        rd.eprint(dumps_ascii(&obj(vec![(
            "error",
            obj(vec![("code", s("error")), ("hint", s(&hint))]),
        )])));
        return Ok(Exit::Error);
    };
    let err = g(&v, "error", cap_line::SET_ERROR)?.cloned();
    if truthy(err.as_ref()) || is_false(at(&v, "ok")) {
        let hint = match &err {
            Some(e @ Value::Object(_)) => {
                let h = at(e, "hint");
                if truthy(h) {
                    h.cloned().unwrap_or(Value::Null)
                } else {
                    at(e, "code").cloned().unwrap_or(Value::Null)
                }
            }
            _ => {
                let pick = if truthy(err.as_ref()) {
                    err.clone().unwrap_or(Value::Null)
                } else if truthy(at(&v, "message")) {
                    at(&v, "message").cloned().unwrap_or(Value::Null)
                } else {
                    s(&raw.chars().take(200).collect::<String>())
                };
                s(&py::str_of(&pick))
            }
        };
        let token = contains(&hint, "token", cap_line::SET_TOKEN)?;
        if token || super::py::is_str(at(&v, "error"), "role_required") {
            rd.eprint(dumps_ascii(&obj(vec![(
                "error",
                obj(vec![
                    ("code", s("owner_only")),
                    (
                        "hint",
                        s(concat!(
                            "only the owner sets a workspace database cap (an agent never raises its own) — ask your human to run: ",
                            cli_name!(),
                            " db cap <n> --workspace <ws>"
                        )),
                    ),
                ]),
            )])));
            return Ok(Exit::Refused);
        }
        let usage = contains(&hint, "db_active_cap", cap_line::SET_CAP)?;
        rd.eprint(dumps_ascii(&obj(vec![(
            "error",
            obj(vec![
                ("code", s(if usage { "usage" } else { "error" })),
                ("hint", hint),
            ]),
        )])));
        return Ok(if usage { Exit::Usage } else { Exit::Error });
    }
    let default = stored == "default";
    let cap = if default {
        Value::Int("25".into())
    } else {
        Value::Int(stored.to_string())
    };
    let unlimited = !default && stored == "0";
    if json_out {
        rd.print(dumps_ascii(&obj(vec![
            ("ok", Value::Bool(true)),
            ("cap", cap),
            ("default", Value::Bool(default)),
            ("unlimited", Value::Bool(unlimited)),
        ])));
        return Ok(Exit::Ok);
    }
    rd.print(format!(
        "database cap: {}{}",
        if unlimited {
            "unlimited (0)".to_string()
        } else {
            py::str_of(&cap)
        },
        if default { " (the default)" } else { "" }
    ));
    Ok(Exit::Ok)
}
