//! The writes of what an App may reach past its wall, as the shell CLI
//! answers them: `app egress add|rm|remove|mode` (`cmd_app_egress`,
//! `cli/src/app-db.sh`) and `app box-service request|revoke|rm`
//! (`cmd_app_box_service`, `cli/src/app-box.sh`). The reads are
//! [`super::walls`]'.
//!
//! Each write is the shell's word loop, one POST sent once
//! (`_app_rt_post`), then:
//! - `--json`: the answer as it came, and exit 2 when it was held;
//! - else `_app_held_exit`'s lines ([`super::updates_set::held_lines`]):
//!   allowed (0), held for the owner (2) or not applied (3).
//!
//! A held `egress add` or `box-service request` from a terminal outside a
//! session (stdin a terminal, `NO_APPROVE` not `1`) goes straight on to
//! the owner's approval, as the shell's does ([`super::approve`]).

use super::call::{Cj, Ctx, Split};
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use k2_client::cli_name;

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const EGRESS_ADD: &str = concat!(
        "usage: ",
        cli_name!(),
        " app egress add <app-id> <host[:port]>... [--reason TEXT]"
    );
    pub const EGRESS_RM: &str = concat!(
        "usage: ",
        cli_name!(),
        " app egress rm <app-id> <host[:port]>"
    );
    pub const EGRESS_MODE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app egress mode <app-id> allowlist|broad"
    );
    pub const BOX_REQUEST: &str = concat!(
        "usage: ",
        cli_name!(),
        " app box-service request <app-id> <name> [<host-port>] [--app-port N] [--reason TEXT]"
    );
    pub const BOX_REVOKE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app box-service revoke <app-id> <name>"
    );
}

/// Which write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    EgressAdd,
    EgressRm,
    EgressMode,
    BoxRequest,
    BoxRevoke,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// No App named: the family's page on stderr, exit 2.
    Page(&'static str),
    /// One POST.
    Post {
        write: Write,
        /// The App, for `mode`'s line.
        app: String,
        body: String,
        json: bool,
    },
}

/// The words after `egress` or `box-service`. `None`: not a write here
/// (or a help form).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let sub = *words.first()?;
    let rest = &words[1..];
    let write = match (verb, sub) {
        ("egress", "add") => Write::EgressAdd,
        ("egress", "rm" | "remove") => Write::EgressRm,
        ("egress", "mode") => Write::EgressMode,
        ("box-service" | "box-services", "request") => Write::BoxRequest,
        ("box-service" | "box-services", "revoke" | "rm") => Write::BoxRevoke,
        _ => return None,
    };
    Some(loop_words(write, rest))
}

/// The family's loop over the words, then the write's checks.
fn loop_words(write: Write, words: &[&str]) -> Result<Ask, Stop> {
    let egress = matches!(
        write,
        Write::EgressAdd | Write::EgressRm | Write::EgressMode
    );
    let (mut app, mut json, mut reason, mut app_port) =
        (String::new(), false, String::new(), String::new());
    let mut more: Vec<&str> = Vec::new();
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let value_flag = match w {
            "--reason" => true,
            "--tail" => egress,
            "--app-port" => !egress,
            _ => false,
        };
        if value_flag {
            // `x="${2:-}"; shift 2`: with one word left, set -e ends it.
            let v = rest.get(1).ok_or_else(|| Stop::silent(Exit::Error))?;
            match w {
                "--reason" => reason = v.to_string(),
                "--app-port" => app_port = v.to_string(),
                _ => {}
            }
            rest = &rest[2..];
            continue;
        }
        match w {
            "--json" => json = true,
            _ if w.starts_with('-') => {
                let tail = if egress {
                    super::walls::usage::EGRESS_FLAG_TAIL
                } else {
                    super::walls::usage::BOX_FLAG_TAIL
                };
                return Err(Stop::usage(format!("Unknown flag: {w}{tail}")));
            }
            _ if app.is_empty() => app = w.to_string(),
            _ => more.push(w),
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Ok(Ask::Page(if egress {
            "app/egress"
        } else {
            "app/box-service"
        }));
    }
    let body = match write {
        Write::EgressAdd => {
            if more.is_empty() {
                return Err(Stop::usage(usage::EGRESS_ADD));
            }
            // `json.dumps(sys.argv[1:])`.
            let hosts = Value::Array(more.iter().map(|h| Value::Str(Text::from(*h))).collect());
            let hj = json::to_string(&hosts, &Options::default()).unwrap_or_default();
            Cj::new()
                .s("app", &app)
                .j("hosts", &hj)
                .s("reason", &reason)
        }
        Write::EgressRm => {
            if more.len() != 1 {
                return Err(Stop::usage(usage::EGRESS_RM));
            }
            Cj::new().s("app", &app).s("host", more[0])
        }
        Write::EgressMode => {
            if more.len() != 1 {
                return Err(Stop::usage(usage::EGRESS_MODE));
            }
            Cj::new()
                .s("app", &app)
                .s("mode", more[0])
                .s("reason", &reason)
        }
        Write::BoxRequest => {
            if more.is_empty() || more.len() > 2 {
                return Err(Stop::usage(usage::BOX_REQUEST));
            }
            Cj::new()
                .s("app", &app)
                .s("name", more[0])
                .s("hostPort", more.get(1).copied().unwrap_or(""))
                .s("appPort", &app_port)
                .s("reason", &reason)
        }
        Write::BoxRevoke => {
            if more.len() != 1 {
                return Err(Stop::usage(usage::BOX_REVOKE));
            }
            Cj::new().s("app", &app).s("name", more[0])
        }
    };
    Ok(Ask::Post {
        write,
        app,
        body: body.body(),
        json,
    })
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    let (write, app, body, json) = match ask {
        Ask::Page(page) => {
            let text = crate::text::help(page).unwrap_or_default();
            return Ok(Shown::err(text, Exit::Usage));
        }
        Ask::Post {
            write,
            app,
            body,
            json,
        } => (*write, app, body, *json),
    };
    let path = match write {
        Write::EgressAdd => "/cli/app/egress/add",
        Write::EgressRm => "/cli/app/egress/rm",
        Write::EgressMode => "/cli/app/egress/mode",
        Write::BoxRequest => "/cli/app/box-service/request",
        Write::BoxRevoke => "/cli/app/box-service/revoke",
    };
    let s = ctx.send(true, path, body, false)?;
    let held_too = matches!(
        write,
        Write::EgressAdd | Write::EgressMode | Write::BoxRequest
    );
    if !(s.ok() || held_too && s.status == "202") {
        return Err(ctx.fail(&s));
    }
    match write {
        Write::EgressAdd | Write::BoxRequest => held_or_approve(ctx, &s, json),
        Write::EgressMode => Ok(mode(&s, app, json)),
        Write::EgressRm => Ok(super::access::answer(&s.body, json, rm_lines)),
        Write::BoxRevoke => Ok(super::access::answer(&s.body, json, revoke_lines)),
    }
}

/// `sys.exit(2 if json.load(sys.stdin).get("status") == "held" else 0)`.
fn json_held_exit(body: &[u8]) -> Shown {
    let mut shown = super::wallet::raw(body);
    let r = (|| -> Result<Exit, Raise> {
        let held = py::dget(&py::load(body, 1)?, "status", 1)? == py::text("held");
        Ok(if held { Exit::Usage } else { Exit::Ok })
    })();
    let tail = Out::default().done(r);
    shown.stderr = tail.stderr;
    shown.exit = tail.exit;
    shown
}

/// `egress add` and `box-service request` after the POST.
fn held_or_approve(ctx: &mut Ctx, s: &Split, json: bool) -> Answer {
    if json {
        return Ok(json_held_exit(&s.body));
    }
    let mut o = Out::default();
    let r = super::updates_set::held_lines(&s.body, &mut o);
    let shown = o.done(r);
    let at_terminal = {
        use std::io::IsTerminal;
        std::io::stdin().is_terminal()
    };
    let approve_here = shown.exit == Exit::Usage
        && !ctx.in_session()
        && at_terminal
        && k2_client::env_names::first_set(k2_client::env_names::NO_APPROVE)
            .is_none_or(|(_, v)| v != "1");
    if !approve_here {
        return Ok(shown);
    }
    // The owner at a terminal outside a session: approve right here.
    let printed = shown.print();
    if printed != Exit::Usage {
        return Err(printed);
    }
    let id = match (|| -> Result<String, Raise> {
        let v = py::load(&s.body, 1)?;
        Ok(py::s(&py::idx(&py::idx(&v, "request", 1)?, "id", 1)?))
    })() {
        Ok(id) => id.trim_end_matches('\n').to_string(),
        Err(e) => return Ok(Out::default().done(Err(e))),
    };
    let approved = super::approve::approve(ctx, &id)?;
    // `_app_approve "$id"; return 0`: a printer that raised or exited 1
    // ends the command there (set -e).
    Ok(approved)
}

/// `egress mode` after the POST.
fn mode(s: &Split, app: &str, json: bool) -> Shown {
    if json {
        let mut shown = super::wallet::raw(&s.body);
        if s.status == "202" {
            shown.exit = Exit::Usage;
        }
        return shown;
    }
    // `if … sys.exit(0 if … == "held" else 1)`: a raise is its traceback,
    // and reads as not held.
    let held = (|| -> Result<bool, Raise> {
        Ok(py::dget(&py::load(&s.body, 1)?, "status", 1)? == py::text("held"))
    })();
    let mut err = String::new();
    match held {
        Ok(true) => {
            let mut o = Out::default();
            let r = super::updates_set::held_lines(&s.body, &mut o);
            return o.done(r);
        }
        Ok(false) => {}
        Err(e) => err.push_str(&Out::default().done(Err(e)).stderr),
    }
    // `echo "$app: egress mode $(… print(json.load(sys.stdin)["mode"]))"`:
    // a raise inside leaves the word empty, and echo still prints.
    let m =
        (|| -> Result<String, Raise> { Ok(py::s(&py::idx(&py::load(&s.body, 1)?, "mode", 1)?)) })();
    let m = match m {
        Ok(m) => m.trim_end_matches('\n').to_string(),
        Err(e) => {
            err.push_str(&Out::default().done(Err(e)).stderr);
            String::new()
        }
    };
    Shown {
        stdout: format!("{app}: egress mode {m}\n"),
        stderr: err,
        exit: Exit::Ok,
    }
}

/// `egress rm`'s Python.
fn rm_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let line = if py::truthy(&py::dget(&v, "removed", 4)?) {
        format!("removed {} (at once)", py::s(&py::idx(&v, "host", 4)?))
    } else {
        py::s(&py::dget_or(&v, "hint", py::text("not on the list"), 4)?)
    };
    o.line(&line);
    Ok(Exit::Ok)
}

/// `box-service revoke`'s Python.
fn revoke_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let line = if py::truthy(&py::dget(&v, "revoked", 4)?) {
        format!("revoked {} (shut at once)", py::s(&py::idx(&v, "name", 4)?))
    } else {
        py::s(&py::dget_or(
            &v,
            "hint",
            py::text("not a box service of this App"),
            4,
        )?)
    };
    o.line(&line);
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(write: Write, app: &str, body: &str, json: bool) -> Option<Result<Ask, Stop>> {
        Some(Ok(Ask::Post {
            write,
            app: app.into(),
            body: body.into(),
            json,
        }))
    }

    #[test]
    fn the_words_read_as_the_shell_reads_them() {
        assert_eq!(
            parse(
                "egress",
                &["add", "acme", "a.example", "b \u{e9}", "--reason", "r"]
            ),
            post(
                Write::EgressAdd,
                "acme",
                r#"{"app": "acme", "hosts": ["a.example", "b \u00e9"], "reason": "r"}"#,
                false
            )
        );
        assert_eq!(
            parse("egress", &["add", "acme"]),
            Some(Err(Stop::usage(usage::EGRESS_ADD)))
        );
        assert_eq!(
            parse("egress", &["remove", "acme", "h", "--json"]),
            post(
                Write::EgressRm,
                "acme",
                r#"{"app": "acme", "host": "h"}"#,
                true
            )
        );
        assert_eq!(
            parse("egress", &["mode", "acme"]),
            Some(Err(Stop::usage(usage::EGRESS_MODE)))
        );
        assert_eq!(
            parse("egress", &["mode", "acme", "broad", "--tail"]),
            Some(Err(Stop::silent(Exit::Error)))
        );
        assert_eq!(parse("egress", &["add"]), Some(Ok(Ask::Page("app/egress"))));
        assert_eq!(parse("egress", &["acme"]), None);
        assert_eq!(parse("egress", &["add", "a", "h", "-h"]), None);
        assert_eq!(
            parse(
                "box-service",
                &["request", "acme", "pg", "5432", "--app-port", "15432"]
            ),
            post(
                Write::BoxRequest,
                "acme",
                r#"{"app": "acme", "name": "pg", "hostPort": "5432", "appPort": "15432"}"#,
                false
            )
        );
        assert_eq!(
            parse("box-service", &["request", "acme", "a", "b", "c"]),
            Some(Err(Stop::usage(usage::BOX_REQUEST)))
        );
        assert_eq!(
            parse("box-service", &["rm", "acme", "pg"]),
            post(
                Write::BoxRevoke,
                "acme",
                r#"{"app": "acme", "name": "pg"}"#,
                false
            )
        );
        assert!(matches!(
            parse("box-service", &["revoke", "acme", "--tail", "1"]),
            Some(Err(s)) if s.line.starts_with("Unknown flag: --tail")
        ));
    }

    #[test]
    fn mode_reads_held_then_the_mode() {
        let split = |status: &str, body: &str| Split {
            status: status.into(),
            body: body.as_bytes().to_vec(),
        };
        let set = mode(&split("200", r#"{"mode":"broad"}"#), "acme", false);
        assert_eq!(set.stdout, "acme: egress mode broad\n");
        let bad = mode(&split("200", "[]"), "acme", false);
        assert_eq!(bad.exit, Exit::Ok);
        assert_eq!(bad.stdout, "acme: egress mode \n");
        assert_eq!(bad.stderr.matches("Traceback").count(), 2);
        let held = mode(&split("202", r#"{"x":1}"#), "acme", true);
        assert_eq!(held.exit, Exit::Usage);
    }
}
