//! `app user rooms-ok <app> <test-login> [--assign <login>] [--reason T]
//! [--json]` (and `skin user rooms-ok`, the same `cmd_skin` arm), as the
//! shell CLI answers it (`_app_test_guest_rooms_ok`,
//! `cli/src/app-updates.sh`): let a test login an agent made into the
//! rooms its role names.
//!
//! - The words are its own loop's ([`parse`]): `--reason` (the shell's
//!   `_need_value` line when it is the last word), `--reason=`, `--json`,
//!   any other flag is `Unknown flag: <f> (<usage>)`, a third positional
//!   or a missing one the usage line, each exit 2. `cmd_skin`'s `--role`
//!   check comes first ([`super::super::skin`]).
//! - `--assign` is the pre-scan's (`_app_take_assign`, [`super::assign`]):
//!   from `app` it rides the POST as `?assign=`; `skin` has no pre-scan,
//!   so there it is an unknown flag.
//! - One POST to `/cli/app/test-guest/rooms-ok` (`_app_rt_post`); any
//!   status but 200 is `_compute_fail` ([`Ctx::call`]).
//! - The answer as it came with `--json`, else the shell's Python line
//!   for line; then the exit: 2 while the card waits (`status` is
//!   `asked`), else 0, read by a second Python that raises on an answer
//!   that is no JSON object.
//!
//! `-h` or `--help` in its loop is its own page, which only the shell
//! prints (`None`): the router answers help before the verb reads its
//! words, on both sides.

use super::call::{Cj, Ctx};
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{raised, Raise, Shown};
use crate::out::json::Value;
use crate::out::pylib::ops::py_eq;
use k2_client::cli_name;

/// The usage line (exit 2), from `app` and from `skin` alike.
pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " app user rooms-ok <app-id> <test-login> [--assign <login>] [--reason TEXT] [--json]"
);

/// The route.
pub const PATH: &str = "/cli/app/test-guest/rooms-ok";

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomsOk {
    pub app: String,
    pub login: String,
    pub reason: String,
    pub json: bool,
}

/// The words after `user rooms-ok`. `None`: `-h` or `--help` (the
/// shell's page).
pub fn parse(words: &[&str]) -> Option<Result<RoomsOk, Stop>> {
    let (mut app, mut login, mut reason, mut json) =
        (String::new(), String::new(), String::new(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        if let Some(v) = w.strip_prefix("--reason=") {
            reason = v.to_string();
            rest = &rest[1..];
            continue;
        }
        match w {
            "--reason" => {
                let Some(v) = rest.get(1) else {
                    return Some(Err(Stop::need_value(w)));
                };
                reason = v.to_string();
                rest = &rest[2..];
                continue;
            }
            "--json" => json = true,
            "-h" | "--help" => return None,
            _ if w.starts_with('-') => {
                return Some(Err(Stop::usage(format!("Unknown flag: {w} ({USAGE})"))))
            }
            // `[ -z "$app" ]`: an empty word leaves the slot open.
            _ if app.is_empty() => app = w.to_string(),
            _ if login.is_empty() => login = w.to_string(),
            _ => return Some(Err(Stop::usage(USAGE))),
        }
        rest = &rest[1..];
    }
    if app.is_empty() || login.is_empty() {
        return Some(Err(Stop::usage(USAGE)));
    }
    Some(Ok(RoomsOk {
        app,
        login,
        reason,
        json,
    }))
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, r: &RoomsOk) -> Answer {
    let body = Cj::new()
        .s("app", &r.app)
        .s("username", &r.login)
        .s("reason", &r.reason)
        .body();
    let answer = ctx.call(true, PATH, &body)?;
    Ok(shown(&answer, r.json))
}

/// The answer printed, then the exit the second Python gives it.
fn shown(body: &[u8], json: bool) -> Shown {
    let mut shown = if json {
        raw(body)
    } else {
        let mut o = Out::default();
        let r = lines(body, &mut o);
        let printed = o.done(r);
        // The printer raised: `set -e` ends the command there.
        if printed.exit != Exit::Ok {
            return printed;
        }
        printed
    };
    // `sys.exit(2 if json.load(sys.stdin).get("status") == "asked" else 0)`.
    match asked(body) {
        Ok(true) => shown.exit = Exit::Usage,
        Ok(false) => shown.exit = Exit::Ok,
        Err(e) => {
            let tail = raised(String::new(), String::new(), &e);
            shown.stderr.push_str(&tail.stderr);
            shown.exit = tail.exit;
        }
    }
    shown
}

/// The second Python, on its line 1.
fn asked(body: &[u8]) -> Result<bool, Raise> {
    let v = py::load(body, 1)?;
    Ok(py_eq(&py::dget(&v, "status", 1)?, &py::text("asked")))
}

/// `", ".join(v.get("rooms") or [])` at `line`.
fn rooms(v: &Value, line: u32) -> Result<String, Raise> {
    py::join(
        ", ",
        &py::or(py::dget(v, "rooms", line)?, py::empty_list()),
        line,
    )
}

/// The printer's Python, line for line (its first line is empty).
fn lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let st = py::dget(&v, "status", 4)?;
    if py_eq(&st, &py::text("asked")) {
        let user = py::dget(&v, "username", 6)?;
        let joined = rooms(&v, 6)?;
        let role = py::dget(&v, "role", 6)?;
        o.line(&format!(
            "Asked: {} into {joined} with its role {}.",
            py::s(&user),
            py::s(&role)
        ));
        // `"  " + ((v.get("card") or {}).get("id") and v.get("where") or "")`.
        let card = py::or(py::dget(&v, "card", 7)?, Value::Object(Vec::new()));
        let id = py::dget(&card, "id", 7)?;
        let where_ = if py::truthy(&id) {
            py::or(py::dget(&v, "where", 7)?, py::text(""))
        } else {
            py::text("")
        };
        o.line(&py::concat("  ", &where_, 7)?);
    } else if py_eq(&st, &py::text("already_open")) {
        let user = py::dget(&v, "username", 9)?;
        o.line(&format!("{} already uses {}.", py::s(&user), rooms(&v, 9)?));
    } else {
        let note = py::dget(&v, "note", 11)?;
        let line = if py::truthy(&note) {
            py::s(&note)
        } else {
            let user = py::dget(&v, "username", 11)?;
            format!("{} can now use {}.", py::s(&user), rooms(&v, 11)?)
        };
        o.line(&line);
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(words: &[&str]) -> Option<Result<RoomsOk, Stop>> {
        parse(words)
    }

    #[test]
    fn the_words_as_its_loop_reads_them() {
        let want = RoomsOk {
            app: "qa1".into(),
            login: "tess".into(),
            reason: "r".into(),
            json: true,
        };
        assert_eq!(
            p(&["qa1", "--reason", "r", "tess", "--json"]),
            Some(Ok(want.clone()))
        );
        assert_eq!(
            p(&["--json", "", "qa1", "--reason=r", "tess"]),
            Some(Ok(want))
        );
        assert_eq!(
            p(&["qa1", "tess", "--reason"]),
            Some(Err(Stop::need_value("--reason")))
        );
        assert_eq!(p(&["qa1"]), Some(Err(Stop::usage(USAGE))));
        assert_eq!(p(&["a", "b", "c"]), Some(Err(Stop::usage(USAGE))));
        assert_eq!(
            p(&["a", "b", "--assign", "ada"]),
            Some(Err(Stop::usage(format!("Unknown flag: --assign ({USAGE})"))))
        );
        assert_eq!(p(&["a", "-h"]), None);
        assert_eq!(p(&["a", "b", "c", "-h"]), Some(Err(Stop::usage(USAGE))));
    }

    #[test]
    fn the_answer_and_its_exit() {
        let asked = br#"{"status":"asked","username":"tess","role":"tester","rooms":["sales","ops"],"card":{"id":"pr_1"},"where":"a card"}"#;
        let s = shown(asked, false);
        assert_eq!(
            s.stdout,
            "Asked: tess into sales, ops with its role tester.\n  a card\n"
        );
        assert_eq!(s.exit, Exit::Usage);
        let s = shown(asked, true);
        assert_eq!(s.stdout, format!("{}\n", String::from_utf8_lossy(asked)));
        assert_eq!(s.exit, Exit::Usage);
        let s = shown(br#"{"status":"asked","username":"t","card":{}}"#, false);
        assert_eq!(s.stdout, "Asked: t into  with its role None.\n  \n");
        let open = br#"{"status":"already_open","username":"t","rooms":"ab"}"#;
        let s = shown(open, false);
        assert_eq!(s.stdout, "t already uses a, b.\n");
        assert_eq!(s.exit, Exit::Ok);
        let opened = br#"{"status":"opened","username":"t","rooms":["x"]}"#;
        let s = shown(opened, false);
        assert_eq!(s.stdout, "t can now use x.\n");
        let s = shown(br#"{"note":"n"}"#, false);
        assert_eq!(s.stdout, "n\n");
        // A raise in the printer ends it (exit 1); with --json the second
        // Python raises after the answer is out.
        let s = shown(b"[1]", false);
        assert_eq!((s.stdout.as_str(), s.exit), ("", Exit::Error));
        let last = "AttributeError: 'list' object has no attribute 'get'\n";
        assert!(s.stderr.ends_with(last), "{}", s.stderr);
        let s = shown(b"[1]", true);
        assert_eq!((s.stdout.as_str(), s.exit), ("[1]\n", Exit::Error));
        let s = shown(br#"{"status":"asked","card":[1]}"#, false);
        assert_eq!(s.exit, Exit::Error);
        let first = "Asked: None into  with its role None.\n";
        assert!(s.stdout.starts_with(first), "{}", s.stdout);
    }
}
