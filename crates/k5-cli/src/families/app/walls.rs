//! The reads of what an App may reach past its wall, as the shell CLI
//! answers them: `app egress [show] <app>` and `app egress log <app>
//! [--tail N]` (`cmd_app_egress`, `cli/src/app-db.sh`), `app box-service
//! <app>` (`cmd_app_box_service`, `cli/src/app-box.sh`) and `app db status
//! <app>` (`cmd_app_db`, `cli/src/app-db.sh`), with each family's page and
//! its usage lines. Each read is the shell's word loop, one request,
//! `--json` as it came, else the shell's Python line for line.
//!
//! `db` keeps its own rules (R7): a usage mistake is a JSON line and exit
//! 1, and a 400 is `_app_db_fail`'s JSON line and exit 1.
//!
//! The writes are [`super::walls_set`]' (`egress add|rm|mode`,
//! `box-service request|revoke`) and [`super::db_set`]' (`db
//! connect|check|disconnect`).

use super::call::{Ctx, Split};
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::py as pyfmt;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// The usage lines.
pub mod usage {
    use super::cli_name;
    pub const EGRESS_FLAG_TAIL: &str = concat!(" (", cli_name!(), " app egress --help)");
    pub const EGRESS_SHOW: &str = concat!(
        "usage: ",
        cli_name!(),
        " app egress <app-id> (to ask for a host: ",
        cli_name!(),
        " app egress add <app-id> <host>)"
    );
    pub const BOX_FLAG_TAIL: &str = concat!(" (", cli_name!(), " app box-service --help)");
    pub const BOX_SHOW: &str = concat!(
        "usage: ",
        cli_name!(),
        " app box-service <app-id> (to ask: ",
        cli_name!(),
        " app box-service request <app-id> <name> [<host-port>])"
    );
    pub const DB_VERB: &str = concat!(
        "{\"error\":\"usage\",\"hint\":\"",
        cli_name!(),
        " app db connect|status|check|disconnect <app-id> (",
        cli_name!(),
        " app db --help)\"}"
    );
    pub const DB_TAIL: &str = concat!(" (", cli_name!(), " app db --help)");
    pub const DB_NAME_APP: &str = concat!("name the App: ", cli_name!(), " app db ");
}

/// Which read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Read {
    Egress,
    EgressLog,
    BoxService,
    DbStatus,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// A family's page (`help/app/<page>`): on stdout, exit 0; or on
    /// stderr, exit 2 (no App named).
    Page { page: &'static str, usage: bool },
    /// One GET.
    Get {
        read: Read,
        query: String,
        json: bool,
    },
}

/// The verbs here.
pub const VERBS: &[&str] = &["egress", "box-service", "box-services", "db", "database"];

/// The words after the verb. `None`: a help form or a write (the
/// shell's).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let sub = words.first().copied().unwrap_or("");
    match verb {
        "egress" => match sub {
            "" | "help" => page("app/egress"),
            "add" | "rm" | "remove" | "mode" => None,
            "log" => Some(egress(&words[1..], true)),
            _ => Some(egress(words, false)),
        },
        "db" | "database" => match sub {
            "" | "help" => page("app/db"),
            "connect" | "check" | "disconnect" => None,
            "status" => Some(db_status(&words[1..])),
            _ => Some(Err(Stop {
                line: usage::DB_VERB.to_string(),
                exit: Exit::Error,
            })),
        },
        _ => match sub {
            "" | "help" => page("app/box-service"),
            "request" | "revoke" | "rm" => None,
            _ => Some(box_service(words)),
        },
    }
}

fn page(page: &'static str) -> Option<Result<Ask, Stop>> {
    Some(Ok(Ask::Page { page, usage: false }))
}

/// `x="${2:-}"; shift 2`: with one word left, set -e ends the command.
fn value<'a>(rest: &[&'a str]) -> Result<&'a str, Stop> {
    rest.get(1)
        .copied()
        .ok_or_else(|| Stop::silent(Exit::Error))
}

/// `cmd_app_egress`' loop, for `show` (`log` false) or `log`.
fn egress(words: &[&str], log: bool) -> Result<Ask, Stop> {
    let (mut app, mut tail, mut json) = (String::new(), String::new(), false);
    let mut hosts = 0usize;
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--json" => json = true,
            "--reason" | "--tail" => {
                let v = value(rest)?;
                if w == "--tail" {
                    tail = v.to_string();
                }
                rest = &rest[2..];
                continue;
            }
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!(
                    "Unknown flag: {w}{}",
                    usage::EGRESS_FLAG_TAIL
                )))
            }
            _ if app.is_empty() => app = w.to_string(),
            _ => hosts += 1,
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Ok(Ask::Page {
            page: "app/egress",
            usage: true,
        });
    }
    let mut query = format!("app={}", quote(&app));
    if log {
        if !tail.is_empty() {
            query.push_str(&format!("&tail={}", quote(&tail)));
        }
        return Ok(Ask::Get {
            read: Read::EgressLog,
            query,
            json,
        });
    }
    if hosts > 0 {
        return Err(Stop::usage(usage::EGRESS_SHOW));
    }
    Ok(Ask::Get {
        read: Read::Egress,
        query,
        json,
    })
}

/// `cmd_app_box_service`' loop, for `show`.
fn box_service(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut json, mut more) = (String::new(), false, 0usize);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--json" => json = true,
            "--reason" | "--app-port" => {
                value(rest)?;
                rest = &rest[2..];
                continue;
            }
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!(
                    "Unknown flag: {w}{}",
                    usage::BOX_FLAG_TAIL
                )))
            }
            _ if app.is_empty() => app = w.to_string(),
            _ => more += 1,
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Ok(Ask::Page {
            page: "app/box-service",
            usage: true,
        });
    }
    if more > 0 {
        return Err(Stop::usage(usage::BOX_SHOW));
    }
    Ok(Ask::Get {
        read: Read::BoxService,
        query: format!("app={}", quote(&app)),
        json,
    })
}

/// `cmd_app_db status`' loop and checks ([`super::db_set::words`]).
fn db_status(words: &[&str]) -> Result<Ask, Stop> {
    let w = super::db_set::words("status", words)?;
    Ok(Ask::Get {
        read: Read::DbStatus,
        query: format!("app={}", quote(&w.app)),
        json: w.json,
    })
}

/// A read's printer: the shell's Python for its answer.
type Printer = fn(&[u8], &mut Out) -> Result<Exit, Raise>;

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    let (read, query, json) = match ask {
        Ask::Page { page, usage } => {
            let text = crate::text::help(page).unwrap_or_default();
            return Ok(if *usage {
                Shown::err(text, Exit::Usage)
            } else {
                Shown::out(text)
            });
        }
        Ask::Get { read, query, json } => (*read, query, *json),
    };
    let (path, lines): (&str, Printer) = match read {
        Read::Egress => ("/cli/app/egress", egress_lines),
        Read::EgressLog => ("/cli/app/egress/log", egress_log_lines),
        Read::BoxService => ("/cli/app/box-service", box_lines),
        Read::DbStatus => ("/cli/app/db/status", db_status_lines),
    };
    let body = if read == Read::DbStatus {
        let s = ctx.send(false, path, query, false)?;
        if !s.ok() {
            return Err(db_fail(ctx, &s));
        }
        s.body
    } else {
        ctx.call(false, path, query)?
    };
    Ok(super::access::answer(&body, json, lines))
}

/// `_app_db_fail`: a 400 is its Python's JSON line on stderr and exit 1;
/// anything else is `_compute_fail`.
pub fn db_fail(ctx: &Ctx, s: &Split) -> Exit {
    if s.status != "400" {
        return ctx.fail(s);
    }
    let shown = db_fail_400(&s.body);
    shown.print();
    Exit::Error
}

/// `_app_db_fail`'s Python for a 400.
fn db_fail_400(body: &[u8]) -> Shown {
    let parsed = std::str::from_utf8(body)
        .ok()
        .and_then(|t| crate::out::pylib::json::loads(t).ok());
    // `try: e = json.load(sys.stdin).get("error") or {}` / `except: e = {}`.
    let e = match parsed {
        Some(Value::Object(pairs)) => py::or(
            crate::families::get(&pairs, "error")
                .cloned()
                .unwrap_or(Value::Null),
            empty_dict(),
        ),
        _ => empty_dict(),
    };
    let mut o = Out::default();
    let r = (|| {
        let code = py::dget_or(&e, "code", py::text("usage"), 7)?;
        let hint = py::dget_or(&e, "hint", py::text(""), 7)?;
        let line = json::to_string(
            &Value::Object(vec![
                (Text::from("error"), code),
                (Text::from("hint"), hint),
            ]),
            &Options::default(),
        )
        .unwrap_or_default();
        o.err(&line);
        Ok(Exit::Error)
    })();
    o.done(r)
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

/// `"  " + v.get(key, "")`.
fn indented(v: &Value, prefix: &str, key: &str, line: u32) -> Result<String, Raise> {
    py::concat(prefix, &py::dget_or(v, key, py::text(""), line)?, line)
}

/// `cmd_app_egress show`'s Python.
pub fn egress_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let app = py::idx(&v, "app", 4)?;
    let mode = py::idx(&v, "mode", 4)?;
    let enforced = if py::truthy(&py::dget(&v, "enforced", 4)?) {
        ""
    } else {
        "  (NOT enforced: process engine)"
    };
    o.line(&format!(
        "{}  egress {}{enforced}",
        py::s(&app),
        py::s(&mode)
    ));
    o.line(&indented(&v, "  ", "note", 5)?);
    let hosts = py::or(py::dget(&v, "hosts", 6)?, py::empty_list());
    if !py::truthy(&hosts) {
        o.line(concat!(
            "  no hosts: this App reaches nothing outside ",
            product_name!()
        ));
    }
    for h in py::iter(&hosts, 9)? {
        let host = py::idx(&h, "host", 10)?;
        let hp = if py::contains(&host, ":", 10)? {
            format!(
                "[{}]:{}",
                py::s(&py::idx(&h, "host", 10)?),
                py::s(&py::idx(&h, "port", 10)?)
            )
        } else {
            format!(
                "{}:{}",
                py::s(&py::idx(&h, "host", 10)?),
                py::s(&py::idx(&h, "port", 10)?)
            )
        };
        let extra = if py::truthy(&py::dget(&h, "requestId", 11)?) {
            py::concat("  request ", &py::idx(&h, "requestId", 11)?, 11)?
        } else {
            String::new()
        };
        let status = py::idx(&h, "status", 12)?;
        let source = py::idx(&h, "source", 12)?;
        let hits = py::idx(&h, "hits", 12)?;
        let refused = py::idx(&h, "refused", 12)?;
        o.line(&format!(
            "  {} {} {} hits {} refused {}{extra}",
            py::left(&status, 8),
            pyfmt_left(&hp, 40),
            py::left(&source, 10),
            py::left(&hits, 6),
            py::s(&refused)
        ));
    }
    pending(&v, 13, o)?;
    let p = py::or(py::dget(&v, "proxy", 15)?, empty_dict());
    if py::truthy(&py::dget(&p, "listening", 16)?) {
        let port = py::idx(&p, "port", 17)?;
        let active = py::idx(&p, "active", 17)?;
        let conns = py::idx(&p, "connections", 17)?;
        let refused = py::idx(&p, "refused", 17)?;
        o.line(&format!(
            "  proxy    127.0.0.1:{}  {} open, {} total, {} refused",
            py::s(&port),
            py::s(&active),
            py::s(&conns),
            py::s(&refused)
        ));
    } else {
        o.line("  proxy    not listening (it opens when the guarded App starts)");
    }
    Ok(Exit::Ok)
}

/// `'%-<width>s' % text`.
fn pyfmt_left(text: &str, width: usize) -> String {
    crate::families::ljust(text, width)
}

/// `for r in v.get("pending") or []: print("  pending  …")`, from `line`.
fn pending(v: &Value, line: u32, o: &mut Out) -> Result<(), Raise> {
    let pending = py::or(py::dget(v, "pending", line)?, py::empty_list());
    for r in py::iter(&pending, line)? {
        let id = py::idx(&r, "id", line + 1)?;
        let summary = py::idx(&r, "summary", line + 1)?;
        let by = py::idx(&r, "requestedBy", line + 1)?;
        o.line(&format!(
            "  pending  {}  {} (by {})",
            py::s(&id),
            py::s(&summary),
            py::s(&by)
        ));
    }
    Ok(())
}

/// `cmd_app_egress log`'s Python.
pub fn egress_log_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let lines = py::or(py::dget(&v, "lines", 3)?, py::empty_list());
    for l in py::iter(&lines, 3)? {
        let at = py::dget_or(&l, "at", Value::Int("0".into()), 4)?;
        let at = py::local_time(&at, "%Y-%m-%d %H:%M:%S", 4)?;
        let host = py::dget_or(&l, "host", py::text("-"), 5)?;
        let port = py::dget_or(&l, "port", py::text("-"), 5)?;
        let hp = format!("{}:{}", py::s(&host), py::s(&port));
        if py::dget(&l, "result", 6)? == py::text("ok") {
            let kind = py::dget_or(&l, "kind", py::text(""), 7)?;
            let up = py::dget(&l, "up", 7)?;
            let down = py::dget(&l, "down", 7)?;
            let ms = py::dget(&l, "ms", 7)?;
            let end = py::dget(&l, "end", 7)?;
            o.line(&format!(
                "{at}  ok       {} {} up {} down {}  {}ms ({})",
                py::left(&kind, 6),
                pyfmt_left(&hp, 40),
                py::s(&up),
                py::s(&down),
                py::s(&ms),
                py::s(&end)
            ));
        } else {
            let result = py::dget(&l, "result", 9)?;
            let kind = py::dget_or(&l, "kind", py::text(""), 9)?;
            let code = py::dget(&l, "code", 9)?;
            o.line(&format!(
                "{at}  {} {} {} {}",
                py::left(&result, 8),
                py::left(&kind, 6),
                pyfmt_left(&hp, 40),
                py::s(&code)
            ));
        }
    }
    Ok(Exit::Ok)
}

/// `cmd_app_box_service show`'s Python.
pub fn box_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let app = py::idx(&v, "app", 4)?;
    let wall = py::idx(&v, "wall", 4)?;
    o.line(&format!("{}  box services ({})", py::s(&app), py::s(&wall)));
    o.line(&indented(&v, "  ", "note", 5)?);
    let svcs = py::or(py::dget(&v, "services", 6)?, py::empty_list());
    if !py::truthy(&svcs) {
        o.line(concat!(
            "  none: this App reaches nothing on this box but ",
            product_name!(),
            " (its gateway link)"
        ));
    }
    for s in py::iter(&svcs, 9)? {
        let fw = py::dget(&s, "forwarded", 10)?;
        let fwt = if fw == Value::Null || fw == py::text("direct") {
            String::new()
        } else if py::truthy(&fw) {
            "  forwarded".to_string()
        } else {
            format!(
                "  NOT in the running App ({} app restart {})",
                cli_name!(),
                py::s(&py::idx(&v, "app", 11)?)
            )
        };
        let req = if py::truthy(&py::dget(&s, "requestId", 12)?) {
            py::concat("  request ", &py::idx(&s, "requestId", 12)?, 12)?
        } else {
            String::new()
        };
        let status = py::idx(&s, "status", 13)?;
        let name = py::idx(&s, "name", 13)?;
        let port = py::idx(&s, "hostPort", 13)?;
        let in_wall = py::idx(&s, "inWall", 13)?;
        let env = py::idx(&s, "env", 13)?;
        o.line(&format!(
            "  {} {} host 127.0.0.1:{} in the wall {}  {}{fwt}{req}",
            py::left(&status, 8),
            py::left(&name, 14),
            py::left(&port, 5),
            py::s(&in_wall),
            py::s(&env)
        ));
    }
    pending(&v, 14, o)?;
    let presets = py::or(py::dget(&v, "presets", 16)?, py::empty_list());
    for p in py::iter(&presets, 16)? {
        if py::truthy(&py::dget(&p, "error", 17)?) {
            let name = py::idx(&p, "name", 18)?;
            let error = py::idx(&p, "error", 18)?;
            o.line(&format!(
                "  preset   {} {}",
                py::left(&name, 14),
                py::s(&error)
            ));
        } else {
            let name = py::idx(&p, "name", 20)?;
            let port = py::idx(&p, "hostPort", 20)?;
            let from = py::idx(&p, "from", 20)?;
            let what = py::idx(&p, "what", 20)?;
            o.line(&format!(
                "  preset   {} 127.0.0.1:{} ({}): {}",
                py::left(&name, 14),
                py::s(&port),
                py::s(&from),
                py::s(&what)
            ));
        }
    }
    o.line(&indented(&v, "  never    ", "refused", 21)?);
    Ok(Exit::Ok)
}

/// `v[0]`.
pub fn first(v: &Value, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Array(items) => items
            .first()
            .cloned()
            .ok_or_else(|| Raise::at(line, "IndexError: list index out of range")),
        Value::Str(_) => py::head(v, 1, line).and_then(|h| {
            if py::truthy(&h) {
                Ok(h)
            } else {
                Err(Raise::at(line, "IndexError: string index out of range"))
            }
        }),
        // A JSON object's keys are strings: the int 0 is never one.
        Value::Object(_) => Err(Raise::at(line, "KeyError: 0")),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                pyfmt::type_name(other)
            ),
        )),
    }
}

/// `cmd_app_db status`' Python.
pub fn db_status_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let app = py::idx(&v, "app", 4)?;
    let kind = py::dget(&v, "kind", 4)?;
    let running = if py::truthy(&py::dget(&v, "running", 4)?) {
        "running"
    } else {
        "not running"
    };
    o.line(&format!(
        "{}  {} \u{b7} {running}",
        py::s(&app),
        py::s(&kind)
    ));
    if !py::truthy(&py::dget(&v, "postgres", 5)?) {
        o.line("  this box runs no Postgres for Apps (Linux only): use SQLite in $K2_APP_DATA");
    }
    let cs = py::or(py::dget(&v, "connections", 7)?, py::empty_list());
    if !py::truthy(&cs) {
        o.line(&format!(
            "  no database connected ({} app db connect {} --new | --db <name>)",
            cli_name!(),
            py::s(&py::idx(&v, "app", 9)?)
        ));
    }
    for c in py::iter(&cs, 10)? {
        let k = py::or(py::dget(&c, "check", 11)?, empty_dict());
        let ok = py::dget(&k, "ok", 12)?;
        let res = if ok == Value::Bool(true) {
            format!("PASS {} ms", py::s(&py::dget(&k, "latencyMs", 12)?))
        } else if py::dget(&k, "ok", 12)? == Value::Bool(false) {
            format!("FAIL {}", py::s(&py::dget(&k, "step", 12)?))
        } else {
            "not checked".to_string()
        };
        let r#as = py::idx(&c, "as", 13)?;
        let db = py::idx(&c, "db", 13)?;
        let major = py::dget(&c, "major", 13)?;
        let cluster = py::dget(&c, "cluster", 13)?;
        let roles = py::or(
            py::dget(&c, "roles", 14)?,
            Value::Array(vec![py::text("?")]),
        );
        let login = first(&roles, 14)?;
        let door = if py::truthy(&py::dget(&c, "doorOpen", 14)?) {
            "open"
        } else {
            "SHUT"
        };
        o.line(&format!(
            "  {} \u{2192} {} (Postgres {}, {})  login {}  door {door}  check {res}",
            py::left(&r#as, 22),
            py::s(&db),
            py::s(&major),
            py::s(&cluster),
            py::s(&login)
        ));
    }
    let pending = py::or(py::dget(&v, "pending", 15)?, py::empty_list());
    for p in py::iter(&pending, 15)? {
        let id = py::idx(&p, "id", 16)?;
        let summary = py::idx(&p, "summary", 16)?;
        o.line(&format!(
            "  waiting for the owner: card {} ({})",
            py::s(&id),
            py::s(&summary)
        ));
    }
    let majors = py::or(py::dget(&v, "majors", 17)?, py::empty_list());
    let majors: Vec<String> = py::iter(&majors, 17)?.iter().map(py::s).collect();
    o.line(&format!(
        concat!("  majors ", product_name!(), " runs here: {}"),
        majors.join(", ")
    ));
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_read_as_the_shell_reads_them() {
        let get = |read, query: &str, json| {
            Some(Ok(Ask::Get {
                read,
                query: query.to_string(),
                json,
            }))
        };
        assert_eq!(
            parse("egress", &["acme", "--json"]),
            get(Read::Egress, "app=acme", true)
        );
        assert_eq!(
            parse("egress", &["log", "acme", "--tail", "5", "x"]),
            get(Read::EgressLog, "app=acme&tail=5", false)
        );
        assert_eq!(
            parse("egress", &["--json"]),
            Some(Ok(Ask::Page {
                page: "app/egress",
                usage: true
            }))
        );
        assert_eq!(
            parse("egress", &["acme", "--tail"]),
            Some(Err(Stop::silent(Exit::Error)))
        );
        assert_eq!(parse("egress", &["add", "a", "h"]), None);
        assert_eq!(
            parse("box-service", &["acme", "x"]),
            Some(Err(Stop::usage(usage::BOX_SHOW)))
        );
        assert_eq!(parse("box-service", &["request", "a", "n"]), None);
        assert_eq!(
            parse("db", &["status", "acme", "--timeout", "1h30m"]),
            get(Read::DbStatus, "app=acme", false)
        );
        assert_eq!(
            parse("db", &["status", "acme", "--timeout", "1x"]),
            Some(Err(Stop::usage(
                "usage: bad duration '1x' (use 2h, 90m, 1h30m, 300s)"
            )))
        );
        let bad = |words: &[&str]| match parse("db", words) {
            Some(Err(s)) if s.exit == Exit::Error => s.line,
            other => panic!("{other:?}"),
        };
        assert!(bad(&["status"]).contains("name the App: "));
        assert!(bad(&["status", "a", "--major", "7"]).contains("--major takes"));
        assert!(bad(&["status", "a", "--db"]).contains("--db needs a value"));
        assert!(bad(&["status", "a", "b"]).contains("one App at a time (got b too)"));
        assert!(bad(&["zap"]).contains("connect|status|check|disconnect"));
        assert_eq!(parse("db", &["connect", "a", "--new"]), None);
    }

    #[test]
    fn a_db_400_is_its_own_line() {
        assert_eq!(
            db_fail_400(br#"{"error":{"code":"bad_as","hint":"h \u00e9"}}"#).stderr,
            "{\"error\": \"bad_as\", \"hint\": \"h \\u00e9\"}\n"
        );
        assert_eq!(
            db_fail_400(b"not json").stderr,
            "{\"error\": \"usage\", \"hint\": \"\"}\n"
        );
        let raised = db_fail_400(br#"{"error":"x"}"#);
        assert!(raised
            .stderr
            .ends_with("AttributeError: 'str' object has no attribute 'get'\n"));
    }
}
