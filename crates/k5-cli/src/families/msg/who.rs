//! `k5 msg who <m_…> [--json]`: who sent a delivery, as the shell's
//! `cmd_msg_who` (`cli/src/agent-template.sh`) asks and `_hire_print who`
//! (`cli/src/agent-hire.sh`) prints it.
//!
//! The words: `--json`; any other word starting with `-` is a usage line
//! naming it; a second id is a usage line; no id is a usage line; each
//! exit 2 with nothing sent. Then `cli_request /cli/msg/who id=<id>` over
//! TCP on the left of a pipe into the reader ([`reader`]), so the command
//! ends with the reader's code, or curl's when the reader's is 0
//! ([`super::super::pipefail`]); no answer ends it by SIGPIPE.

use super::super::{pipefail, raised, request_piped_tcp, Raise, Shown};
use super::py::{self, dumps, dumps_indent, eq, get_or_null, index, s, Printed};
use crate::exit::Exit;
use crate::out;
use crate::out::json::Value;
use crate::out::py::{str_of, truthy};
use crate::prelude::Closing;
use crate::transport::encode::quote;
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The route.
pub const ENDPOINT: &str = "/cli/msg/who";

const USAGE: &str = concat!("Usage: ", cli_name!(), " msg who <m_…> [--json]");

/// No id.
pub fn no_id() -> String {
    format!("{USAGE}  (the id in [from … · m_…])")
}

/// What the words ask for: the id and `--json`, or the usage line.
pub fn parse(words: &[&str]) -> Option<Result<(String, bool), String>> {
    let (mut id, mut json) = (String::new(), false);
    for w in words {
        match *w {
            "--json" => json = true,
            "-h" | "--help" => return None,
            w if w.starts_with('-') => return Some(Err(format!("{USAGE}  (unknown flag {w})"))),
            w => {
                if !id.is_empty() {
                    return Some(Err(format!("{USAGE}  (one id only)")));
                }
                id = w.to_string();
            }
        }
    }
    if id.is_empty() {
        return Some(Err(no_id()));
    }
    Some(Ok((id, json)))
}

/// `k5 msg who …`. `None`: the shell's.
pub fn run(args: &[OsString], words: &[&str], hook_sock: Option<&Path>) -> Option<Exit> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let parsed = parse(words)?;
    let mut client = match super::client(args, hook_sock)? {
        Ok(c) => c,
        Err(exit) => return Some(exit),
    };
    let (id, json) = match parsed {
        Ok(p) => p,
        Err(line) => {
            out::stderr_line(&line);
            return Some(Exit::Usage);
        }
    };
    let mut closing = Closing::default();
    let arg = format!("id={}", quote(&id));
    let piped = request_piped_tcp(&mut client, &mut closing, ENDPOINT, &[arg.as_str()]);
    let shown = reader(&piped.body, json);
    let printed = shown.print();
    let exit = if printed == shown.exit {
        pipefail(piped.exit, shown.exit)
    } else {
        printed
    };
    closing.flush(&mut client, exit);
    Some(exit)
}

/// `_hire_print who "$as_json"` on what the call printed.
pub fn reader(raw: &[u8], json: bool) -> Shown {
    let mut p = Printed::default();
    let Some(d) = py::loads(raw) else {
        return Shown::err(py::daemon_answered(raw) + "\n", Exit::Error);
    };
    match lines(&d, json, &mut p) {
        Ok(code) => Shown {
            stdout: p.stdout,
            stderr: p.stderr,
            exit: Exit::from_curl(code),
        },
        Err(r) => raised(p.stdout, p.stderr, &r),
    }
}

fn lines(d: &Value, json: bool, p: &mut Printed) -> Result<i32, Raise> {
    if let Some(code) = py::error_branch(d, p, &[("not_found", 4), ("usage", 2)], true)? {
        return Ok(code);
    }
    if json {
        p.line(&dumps_indent(d));
        return Ok(0);
    }
    let sender = py::or(get_or_null(d, "sender", 86)?, Value::Object(Vec::new()));
    p.line(&format!("Delivery {}", str_of(&get_or_null(d, "id", 87)?)));
    let via = get_or_null(d, "via", 88)?;
    let own = if eq(&via, &s("passport")) {
        "  (the sender agent's own session)"
    } else {
        ""
    };
    p.line(&format!("  via:     {}{own}", str_of(&via)));
    let handle = get_or_null(&sender, "handle", 89)?;
    let who = if truthy(&handle) {
        handle
    } else {
        py::or(get_or_null(&sender, "label", 89)?, s("-"))
    };
    p.line(&format!("  sender:  {}", str_of(&who)));
    p.line(&format!(
        "  actor:   {}",
        str_of(&get_or_null(d, "actor", 90)?)
    ));
    if truthy(&get_or_null(d, "onBehalfOf", 91)?) {
        let ob = index(d, "onBehalfOf", 91)?;
        let v = index(ob, "value", 91)?;
        p.line(&format!(
            "  for:     {} (app-asserted, not verified)",
            str_of(v)
        ));
    }
    let linked = if truthy(&get_or_null(d, "linked", 92)?) {
        let links = py::or(get_or_null(d, "links", 92)?, Value::Array(Vec::new()));
        let mut parts = Vec::new();
        for l in super::super::py_iter(&links, 92)? {
            let genexpr = |r: Raise| Raise::inside(92, 92, "<genexpr>", r.exception);
            let id = index(&l, "id", 92).map_err(genexpr)?;
            let scope = get_or_null(&l, "scope", 92).map_err(genexpr)?;
            parts.push(format!("{} {}", str_of(id), dumps(&scope)));
        }
        format!("yes {}", parts.join(", "))
    } else {
        "no".to_string()
    };
    p.line(&format!("  linked:  {linked}"));
    Ok(0)
}
