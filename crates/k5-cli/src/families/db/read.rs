//! Another workspace's database, read-only, on a card (0.46.17): `db
//! browse` (and `db list --all`), `db request-read` and `db requests`, as
//! `cmd_db` answers them (`cli/src/db-store.sh`, `_db_browse`,
//! `_db_request_read`, `_db_requests`), and the ask `app db connect
//! --read-only` shares with `request-read` ([`posted`], `_db_read_post`).
//!
//! - The requests are `_compute_send`'s ([`Cp`]): `GET /cli/db/browse`
//!   (`app=<it>` with `--app`), `POST /cli/db/request-read` (the body is
//!   `_compute_json`'s: `db`, `app`, `why`, `assign`, then `dryRun`) and
//!   `GET /cli/permission/status?action=db.read`. The POST's time limit is
//!   `K2_CP_MAX_TIME` or 120 s, as curl reads it; a value the binary can't
//!   read the same way is the shell's ([`super::cp::shells`]).
//! - A status but 200 is `_db_read_fail` ([`fail`]): the daemon's error
//!   as one JSON line on stderr, exit 2 for a 400, 4 for a 404, else 3.
//!   `requests` says `permission_cards_off` for a 404, exit 3.
//! - `--json` prints the answer as it came. Otherwise the shell's Python
//!   readers, copied line for line (line 1 is `import json,sys`); where
//!   one raises, its traceback is printed and the command ends with 1.
//! - `request-read` (and `connect --read-only`) first reads the answer's
//!   `status` (`st="$(… print(… .get("status") or ""))"`): a raise there
//!   ends the command before anything is printed. A card that is up
//!   (`pending`, `already_pending`) ends with the caller's exit: 0 for
//!   `request-read`, 2 (held) for `connect --read-only`.

use super::cp::{hint_of, print_body, Body, Cp};
use super::py::{dumps_ascii, err_line, obj};
use super::{usage, Form, Words};
use crate::exit::Exit;
use crate::families::{ljust, py_get, py_iter, py_len, raised, s, Raise, Shown};
use crate::out;
use crate::out::json::Value;
use crate::out::py;
use crate::out::pylib::json::loads;
use crate::out::pylib::ops::{index, join, py_eq, s_of};
use crate::transport::encode::quote;
use k2_client::cli_name;

const BROWSE: &str = "/cli/db/browse";
const REQUEST_READ: &str = "/cli/db/request-read";
const STATUS: &str = "/cli/permission/status";

const BROWSE_USAGE: &str = concat!(": ", cli_name!(), " db browse [--app <app>] [--json]");
const REQUEST_USAGE: &str = concat!(
    ": ",
    cli_name!(),
    " db request-read --db <name> | --app <app> [--why \"\u{2026}\"] [--assign <login>]"
);
const REQUESTS_USAGE: &str = concat!(": ", cli_name!(), " db requests [--json]");
const NAME_IT: &str = concat!(
    "name the database: --db <name> (",
    cli_name!(),
    " db browse lists them), or --app <app> for the one an App uses"
);
const CARDS_OFF: &str = concat!(
    "no card asks to show: permission cards are off for this agent (",
    cli_name!(),
    " db request-read says who turns them on)"
);
const NONE_ASKED: &str = concat!(
    "no read access asked for yet (",
    cli_name!(),
    " db request-read --db <name>)"
);

/// What a read verb asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `browse [--app <app>]`, and `list --all`.
    Browse { json: bool, app: String },
    /// `request-read`.
    RequestRead(Request),
    /// `requests`.
    Requests { json: bool },
}

/// `request-read`'s words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub json: bool,
    pub db: String,
    pub app: String,
    pub why: String,
    pub assign: String,
    pub dry: bool,
}

/// The form for `ask`.
pub fn form(ask: Ask) -> Form {
    Form::Cp(Box::new(super::cp::Ask::Read(ask)))
}

/// `_db_read_value "$@"`: the flag's value, or its usage line (a flag at
/// the end of the line, an empty value or one that starts with `-`).
fn value<'a>(a: &mut Words<'a>) -> Result<&'a str, Form> {
    let (flag, v) = (a.at(0), a.at(1));
    if a.len() < 2 || v.is_empty() || v.starts_with('-') {
        return Err(usage(format!(
            concat!("{} needs a value (", cli_name!(), " db --help)"),
            flag
        )));
    }
    a.shift(2);
    Ok(v)
}

/// `browse`: its words after the verb.
pub fn browse(a: &mut Words) -> Result<Form, Form> {
    let (mut json, mut app) = (false, String::new());
    while let Some(w) = a.first() {
        match w {
            "--json" => {
                json = true;
                a.shift(1);
            }
            "--app" => app = value(a)?.to_string(),
            _ if w.starts_with("--app=") => {
                app = w["--app=".len()..].to_string();
                a.shift(1);
            }
            _ => return Err(usage(format!("unexpected '{w}'{BROWSE_USAGE}"))),
        }
    }
    Ok(form(Ask::Browse { json, app }))
}

/// `request-read`: its words after the verb.
pub fn request_read(a: &mut Words) -> Result<Form, Form> {
    let mut r = Request::default();
    while let Some(w) = a.first() {
        match w {
            "--json" => {
                r.json = true;
                a.shift(1);
            }
            "--db" | "--app" | "--why" | "--assign" | "--to" => {
                let v = value(a)?.to_string();
                match w {
                    "--db" => r.db = v,
                    "--app" => r.app = v,
                    "--why" => r.why = v,
                    _ => r.assign = v,
                }
            }
            "--dry-run" => {
                r.dry = true;
                a.shift(1);
            }
            _ => {
                let eq = |flag: &str| w.strip_prefix(flag).map(str::to_string);
                if let Some(v) = eq("--db=") {
                    r.db = v;
                } else if let Some(v) = eq("--app=") {
                    r.app = v;
                } else if let Some(v) = eq("--why=") {
                    r.why = v;
                } else if let Some(v) = eq("--assign=").or_else(|| eq("--to=")) {
                    r.assign = v;
                } else {
                    return Err(usage(format!("unexpected '{w}'{REQUEST_USAGE}")));
                }
                a.shift(1);
            }
        }
    }
    if r.db.is_empty() && r.app.is_empty() {
        return Err(usage(NAME_IT));
    }
    Ok(form(Ask::RequestRead(r)))
}

/// `requests`: its words after the verb.
pub fn requests(a: &mut Words) -> Result<Form, Form> {
    let mut json = false;
    while let Some(w) = a.first() {
        match w {
            "--json" => json = true,
            _ => return Err(usage(format!("unexpected '{w}'{REQUESTS_USAGE}"))),
        }
        a.shift(1);
    }
    Ok(form(Ask::Requests { json }))
}

/// `_compute_json K2_CJ_db=… K2_CJ_app=… K2_CJ_why=… K2_CJ_assign=…
/// K2_CJ_dryRun_b=…`.
pub fn request_body(r: &Request) -> String {
    Body::default()
        .s("db", &r.db)
        .s("app", &r.app)
        .s("why", &r.why)
        .s("assign", &r.assign)
        .flag("dryRun", r.dry)
        .text()
}

/// Runs `ask`: the exit. `Err`: the transport ended the command (its line
/// printed).
pub fn run(cp: &mut Cp, ask: &Ask) -> Result<Exit, Exit> {
    match ask {
        Ask::Browse { json, app } => {
            let q = if app.is_empty() {
                String::new()
            } else {
                format!("app={}", quote(app))
            };
            let (status, body) = cp.send(false, BROWSE, &q, false)?;
            if status != "200" {
                return Ok(fail(&status, &body));
            }
            if *json {
                return Ok(print_body(&body));
            }
            Ok(browse_show(&body).print())
        }
        Ask::RequestRead(r) => {
            let max = crate::families::compute::writes::post_max_time(std::env::var_os(
                "K2_CP_MAX_TIME",
            ))
            .flatten();
            let before = std::mem::replace(&mut cp.client.compute_post_max_time, max);
            let sent = cp.send(true, REQUEST_READ, &request_body(r), false);
            cp.client.compute_post_max_time = before;
            let (status, body) = sent?;
            Ok(posted(&status, &body, r.json, Exit::Ok))
        }
        Ask::Requests { json } => {
            let (status, body) = cp.send(false, STATUS, "action=db.read", false)?;
            if status == "404" {
                out::stderr_line(&err_line("permission_cards_off", CARDS_OFF));
                return Ok(Exit::Refused);
            }
            if status != "200" {
                return Ok(fail(&status, &body));
            }
            if *json {
                return Ok(print_body(&body));
            }
            Ok(requests_show(&body).print())
        }
    }
}

/// `_db_read_fail`: the error line on stderr; exit 2 for a 400, 4 for a
/// 404, else 3.
pub fn fail(status: &str, body: &[u8]) -> Exit {
    let raw = String::from_utf8_lossy(body);
    let v = loads(&raw).unwrap_or_else(|_| {
        obj(vec![(
            "error",
            obj(vec![
                ("code", s("daemon_unreachable")),
                ("hint", hint_of(&raw)),
            ]),
        )])
    });
    let dict = matches!(v, Value::Object(_));
    let e = if dict { at(&v, "error") } else { None };
    let e = match e {
        Some(e @ Value::Object(_)) => e,
        other => {
            let code = truthy_or(other.as_ref(), s("error"));
            let hint = if dict {
                let h = at(&v, "hint");
                let m = at(&v, "message");
                truthy_or(
                    h.as_ref().filter(|x| py::truthy(x)).or(m.as_ref()),
                    s(""),
                )
            } else {
                s("")
            };
            obj(vec![("code", code), ("hint", hint)])
        }
    };
    let code = at(&e, "code").unwrap_or(Value::Null);
    let hint = at(&e, "hint").unwrap_or_else(|| s(""));
    out::stderr_line(&dumps_ascii(&obj(vec![(
        "error",
        obj(vec![("code", code), ("hint", hint)]),
    )])));
    match status {
        "400" => Exit::Usage,
        "404" => Exit::NotFound,
        _ => Exit::Refused,
    }
}

/// `_db_read_post`'s answer: a status but 200 is [`fail`]; else the
/// status word, then the answer (`--json` as it came, or the reader's
/// lines); `held` when a card is up, else 0.
pub fn posted(status: &str, body: &[u8], json: bool, held: Exit) -> Exit {
    if status != "200" {
        return fail(status, body);
    }
    let st = match status_word(body) {
        Ok(st) => st,
        Err(r) => return raised(String::new(), String::new(), &r).print(),
    };
    let exit = if json {
        print_body(body)
    } else {
        post_show(body).print()
    };
    if exit != Exit::Ok {
        return exit;
    }
    if matches!(st.as_str(), "pending" | "already_pending") {
        held
    } else {
        Exit::Ok
    }
}

/// `d.get(k)` on a value known to be a dict, cloned.
fn at(v: &Value, key: &str) -> Option<Value> {
    super::py::at(v, key).cloned()
}

/// `x or default`.
fn truthy_or(x: Option<&Value>, default: Value) -> Value {
    x.filter(|v| py::truthy(v)).cloned().unwrap_or(default)
}

/// `json.load(sys.stdin)` at `line`.
fn load(body: &[u8], line: u32) -> Result<Value, Raise> {
    loads(&String::from_utf8_lossy(body)).map_err(|f| Raise::at(line, f.last_line()))
}

/// `print(json.load(sys.stdin).get("status") or "")`, as `$(…)` keeps it.
fn status_word(body: &[u8]) -> Result<String, Raise> {
    let v = load(body, 1)?;
    let st = truthy_or(py_get(&v, "status", 1)?, s(""));
    Ok(py::str_of(&st).trim_end_matches('\n').to_string())
}

/// A reader's output so far.
#[derive(Default)]
struct P {
    out: String,
    err: String,
}

impl P {
    fn print(&mut self, line: impl AsRef<str>) {
        self.out.push_str(line.as_ref());
        self.out.push('\n');
    }

    fn eprint(&mut self, line: impl AsRef<str>) {
        self.err.push_str(line.as_ref());
        self.err.push('\n');
    }

    fn done(self, r: Result<(), Raise>) -> Shown {
        match r {
            Ok(()) => Shown {
                stdout: self.out,
                stderr: self.err,
                exit: Exit::Ok,
            },
            Err(e) => raised(self.out, self.err, &e),
        }
    }
}

/// `_db_browse`'s reader.
fn browse_show(body: &[u8]) -> Shown {
    let mut p = P::default();
    let r = (|| -> Result<(), Raise> {
        let v = load(body, 2)?;
        let rows = truthy_or(py_get(&v, "databases", 3)?, Value::Array(Vec::new()));
        p.print(format!("{} database(s) on this server", py_len(&rows, 4)?));
        for r in py_iter(&rows, 5)? {
            let name = s_of(py_get(&r, "name", 6)?);
            let owner = s_of(py_get(&r, "owner", 6)?);
            let apps = truthy_or(py_get(&r, "apps", 6)?, Value::Array(Vec::new()));
            let apps = join(&apps, ", ", 6)?;
            let apps = if apps.is_empty() { "-".to_string() } else { apps };
            let yours = py::str_of(&truthy_or(py_get(&r, "yours", 6)?, s("-")));
            p.print(format!(
                "  {} owner {} apps {} you {yours}",
                ljust(&name, 30),
                ljust(&owner, 22),
                ljust(&apps, 22)
            ));
        }
        if py_get(&v, "hint", 7)?.is_some_and(py::truthy) {
            p.print(py::str_of(index(&v, "hint", 8)?));
        }
        Ok(())
    })();
    p.done(r)
}

/// `", ".join(x.get("login") for x in v.get("routedTo") or [] if
/// x.get("login"))`: every row is read before the join checks a type.
fn logins(v: &Value, line: u32) -> Result<String, Raise> {
    let routed = truthy_or(py_get(v, "routedTo", line)?, Value::Array(Vec::new()));
    let mut picked = Vec::new();
    for x in py_iter(&routed, line)? {
        if let Some(l) = py_get(&x, "login", line)?.filter(|l| py::truthy(l)) {
            picked.push(l.clone());
        }
    }
    join(&Value::Array(picked), ", ", line)
}

/// `" (" + v["addr"] + ")"`.
fn addr_part(v: &Value, line: u32) -> Result<String, Raise> {
    match index(v, "addr", line)? {
        a @ Value::Str(_) => Ok(format!(" ({})", py::str_of(a))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// `_db_read_post`'s reader.
fn post_show(body: &[u8]) -> Shown {
    let mut p = P::default();
    let r = (|| -> Result<(), Raise> {
        let v = load(body, 2)?;
        let st = py_get(&v, "status", 3)?.cloned().unwrap_or(Value::Null);
        let is = |w: &str| py_eq(&st, &s(w));
        if is("pending") || is("already_pending") {
            let id = s_of(py_get(&v, "id", 5)?);
            let head = if is("pending") {
                format!("Card {id} raised on your Thread")
            } else {
                format!("Card {id} is already waiting on your Thread (nothing new was asked)")
            };
            let addr = if py_get(&v, "addr", 6)?.is_some_and(py::truthy) {
                addr_part(&v, 6)?
            } else {
                String::new()
            };
            let id = s_of(py_get(&v, "id", 6)?);
            p.print(format!(
                "{head}{addr}: they answer it there (desktop, phone or web); you get one [permission:{id}] turn with the answer."
            ));
            if py_get(&v, "text", 7)?.is_some_and(py::truthy) {
                p.print(format!("Card: {}", py::str_of(index(&v, "text", 8)?)));
            }
            let to = logins(&v, 9)?;
            let asks = if to.is_empty() {
                py::str_of(&truthy_or(
                    py_get(&v, "approvers", 10)?,
                    s("an owner or admin"),
                ))
            } else {
                to
            };
            p.print(format!("Asks: {asks}"));
            if py_get(&v, "next", 11)?.is_some_and(py::truthy) {
                p.print(format!("Next: {}", py::str_of(index(&v, "next", 12)?)));
            }
        } else if is("dry_run") {
            p.print(format!("Would ask: {}", s_of(py_get(&v, "text", 14)?)));
            let to = logins(&v, 15)?;
            let asks = if to.is_empty() {
                s_of(py_get(&v, "approvers", 15)?)
            } else {
                to
            };
            p.print(format!("Asks: {asks}"));
        } else if is("read_only") {
            p.print(py::str_of(&truthy_or(py_get(&v, "dsn", 17)?, s(""))));
            if py_get(&v, "dsnHint", 18)?.is_some_and(py::truthy) {
                p.eprint(format!(
                    "read-only; {}",
                    py::str_of(index(&v, "dsnHint", 19)?)
                ));
            }
        } else {
            let text = py_get(&v, "text", 21)?.filter(|t| py::truthy(t));
            p.print(text.map_or_else(|| dumps_ascii(&v), py::str_of));
        }
        Ok(())
    })();
    p.done(r)
}

/// `_db_requests`' reader.
fn requests_show(body: &[u8]) -> Shown {
    let mut p = P::default();
    let r = (|| -> Result<(), Raise> {
        let v = load(body, 2)?;
        let rows = truthy_or(py_get(&v, "requests", 2)?, Value::Array(Vec::new()));
        if !py::truthy(&rows) {
            p.print(NONE_ASKED);
        }
        for r in py_iter(&rows, 5)? {
            let params = truthy_or(py_get(&r, "params", 6)?, Value::Object(Vec::new()));
            let db = py_get(&params, "db", 6)?.cloned().unwrap_or_else(|| s("?"));
            let tail = match py_get(&r, "message", 7)?.filter(|m| py::truthy(m)) {
                Some(m) => m.clone(),
                None => truthy_or(py_get(&r, "reason", 7)?, s("")),
            };
            p.print(format!(
                "  {} {} {} {}",
                ljust(&s_of(py_get(&r, "id", 8)?), 14),
                ljust(&s_of(py_get(&r, "state", 8)?), 13),
                ljust(&py::str_of(&db), 30),
                py::str_of(&tail)
            ));
        }
        Ok(())
    })();
    p.done(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Form {
        super::super::parse(words).form
    }

    #[test]
    fn the_words() {
        assert_eq!(
            parse(&["browse", "--app", "jobs", "--json"]),
            form(Ask::Browse {
                json: true,
                app: "jobs".into()
            })
        );
        assert_eq!(
            parse(&["list", "--all"]),
            form(Ask::Browse {
                json: false,
                app: String::new()
            })
        );
        assert_eq!(
            parse(&["browse", "--app"]),
            usage(concat!("--app needs a value (", cli_name!(), " db --help)"))
        );
        assert_eq!(
            parse(&["browse", "--app", "--json"]),
            usage(concat!("--app needs a value (", cli_name!(), " db --help)"))
        );
        let r = Request {
            db: "d".into(),
            why: "w".into(),
            assign: "ada".into(),
            dry: true,
            ..Request::default()
        };
        assert_eq!(
            parse(&["request-read", "--db=d", "--why", "w", "--to=ada", "--dry-run"]),
            form(Ask::RequestRead(r.clone()))
        );
        assert_eq!(
            request_body(&r),
            r#"{"db": "d", "why": "w", "assign": "ada", "dryRun": true}"#
        );
        assert_eq!(parse(&["request-read"]), usage(NAME_IT));
        assert_eq!(parse(&["request-read", "--db="]), usage(NAME_IT));
        assert_eq!(
            parse(&["request-read", "--db", "x", "--frob"]),
            usage(format!("unexpected '--frob'{REQUEST_USAGE}"))
        );
        assert_eq!(
            parse(&["requests", "extra"]),
            usage(format!("unexpected 'extra'{REQUESTS_USAGE}"))
        );
        assert_eq!(
            parse(&["requests", "--json"]),
            form(Ask::Requests { json: true })
        );
    }

    #[test]
    fn the_readers_print_as_the_shell_s() {
        let b = browse_show(
            br#"{"databases":[{"name":"ws_o_jobs","owner":"ops","apps":["jobs"],"yours":null},{"name":"n","owner":"o","apps":[],"yours":"read"}],"hint":"names only"}"#,
        );
        assert_eq!(
            b.stdout,
            format!(
                "2 database(s) on this server\n  {:<30} owner {:<22} apps {:<22} you -\n  {:<30} owner {:<22} apps {:<22} you read\nnames only\n",
                "ws_o_jobs", "ops", "jobs", "n", "o", "-"
            )
        );
        let r = requests_show(br#"{"requests":[{"id":"pr_1","state":"approved","params":{"db":"x"},"reason":"why"}]}"#);
        assert_eq!(
            r.stdout,
            format!("  {:<14} {:<13} {:<30} why\n", "pr_1", "approved", "x")
        );
        let none = requests_show(br#"{"requests":[]}"#);
        assert_eq!(none.stdout, format!("{NONE_ASKED}\n"));
        let up = post_show(
            br#"{"id":"pr_1","status":"pending","addr":"reader","text":"Let it read.","routedTo":[{"login":"ada"},{"role":"x"}],"next":"wait"}"#,
        );
        assert_eq!(
            up.stdout,
            "Card pr_1 raised on your Thread (reader): they answer it there (desktop, phone or web); you get one [permission:pr_1] turn with the answer.\nCard: Let it read.\nAsks: ada\nNext: wait\n"
        );
        let ro = post_show(br#"{"status":"read_only","dsn":"postgres://r@h/d","dsnHint":"no password"}"#);
        assert_eq!(ro.stdout, "postgres://r@h/d\n");
        assert_eq!(ro.stderr, "read-only; no password\n");
        let bad = post_show(br#"{"status":"pending","addr":7}"#);
        assert_eq!(bad.exit, Exit::Error);
        assert!(bad
            .stderr
            .ends_with("TypeError: can only concatenate str (not \"int\") to str\n"));
    }

    #[test]
    fn the_lines_name_the_brand() {
        for line in [BROWSE_USAGE, REQUEST_USAGE, REQUESTS_USAGE, NAME_IT, CARDS_OFF, NONE_ASKED] {
            assert!(line.contains(&format!("{} db ", cli_name!())), "{line}");
        }
    }
}
