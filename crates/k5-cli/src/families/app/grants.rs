//! `app grants <app> [--json]`: an App's owner grants, as the shell CLI
//! answers it (`cmd_app_grants`, `cli/src/app-guests.sh`): one
//! `_compute_call GET /cli/app/grants app=<app>`, `--json` as it came,
//! else the shell's Python line for line. `cmd_app` sends `grants` here
//! only when the next word is none of `cmd_skin`'s ([`super::SKIN_GRANT`]).
//!
//! The credential is the shell's: the session's passport in a session,
//! the owner token outside one ([`Ctx::call`]).

use super::call::Ctx;
use super::grace_set::{help, walk};
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::Raise;
use crate::out::pylib::ops::py_eq;
use crate::transport::encode::quote;
use k2_client::cli_name;

/// The usage line (exit 2).
pub const USAGE: &str = concat!("usage: ", cli_name!(), " app grants <app> [--json]");

/// `"%s: no owner grants (k2 app grant %s --kind guests-rooms, the
/// owner)"`, split where the values go.
const NONE_MID: &str = concat!(": no owner grants (", cli_name!(), " app grant ");

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub app: String,
    pub json: bool,
}

/// The words after `grants`. `None`: a help form (the shell's).
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    Some(walk(words, &[], &["--json"]).and_then(|w| {
        let app = w.last();
        if app.is_empty() {
            return Err(Stop::usage(USAGE));
        }
        Ok(Ask {
            app,
            json: w.has("--json"),
        })
    }))
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    let body = ctx.call(
        false,
        "/cli/app/grants",
        &format!("app={}", quote(&ask.app)),
    )?;
    if ask.json {
        return Ok(raw(&body));
    }
    let mut o = Out::default();
    let r = lines(&body, &mut o).map(|()| Exit::Ok);
    Ok(o.done(r))
}

/// `cmd_app_grants`' Python.
fn lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let a = py::idx(&v, "app", 4)?;
    let gs = py::or(py::dget(&v, "grants", 5)?, py::empty_list());
    if !py::truthy(&gs) {
        o.line(&format!(
            "{}{NONE_MID}{} --kind guests-rooms, the owner)",
            py::s(&py::dget(&a, "handle", 7)?),
            py::s(&py::dget(&a, "name", 7)?)
        ));
        return Ok(());
    }
    for g in py::iter(&gs, 8)? {
        let kind = py::idx(&g, "kind", 9)?;
        let what = if py_eq(&kind, &py::text("chat-key")) {
            let off = if py::truthy(&py::dget(&g, "enabled", 10)?) {
                ""
            } else {
                "(off) "
            };
            let caps = py::join(
                " ",
                &py::or(py::dget(&g, "caps", 10)?, py::empty_list()),
                10,
            )?;
            let level = py::dget(&g, "maxLevel", 10)?;
            let members = py::dget(&g, "maxMembers", 10)?;
            let uploads = if py::truthy(&py::dget(&g, "uploads", 10)?) {
                "on"
            } else {
                "off"
            };
            format!(
                "{off}{caps}, up to level {}, up to {} people, uploads {uploads}",
                py::s(&level),
                py::s(&members)
            )
        } else if py_eq(&py::idx(&g, "kind", 11)?, &py::text("guests-chat")) {
            format!("up to {} people", py::s(&py::dget(&g, "maxMembers", 12)?))
        } else {
            let out = py::or(py::dget(&g, "handsOut", 14)?, py::empty_list());
            format!("hands out {}", py::join(", ", &out, 14)?)
        };
        let handle = py::dget(&a, "handle", 15)?;
        let kind = py::idx(&g, "kind", 15)?;
        let by = py::dget(&g, "grantedBy", 15)?;
        let at = py::local_minutes(&py::idx(&g, "grantedAt", 16)?, 16)?;
        o.line(&format!(
            "{}  {}  {what}  by {} {at}",
            py::s(&handle),
            py::s(&kind),
            py::s(&by)
        ));
    }
    Ok(())
}

/// For the unit tests: a grant row's value.
#[cfg(test)]
fn printed(body: &str) -> String {
    let mut o = Out::default();
    lines(body.as_bytes(), &mut o).unwrap();
    o.stdout
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grants_read_as_the_shell_prints_them() {
        assert_eq!(
            printed(r#"{"app":{"handle":"h","name":"n"}}"#),
            format!("h{NONE_MID}n --kind guests-rooms, the owner)\n")
        );
        assert_eq!(
            parse(&["--json"]),
            Some(Err(Stop::usage(USAGE))),
            "--json alone names no App"
        );
        assert!(parse(&["a", "-h"]).is_none());
    }
}
