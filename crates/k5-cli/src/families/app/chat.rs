//! `app chat <app> [--json]`, as the shell CLI's `cmd_app_chat` answers it
//! (`cli/src/app-recording.sh`): the App's chat key, each Space bound to
//! it, their sync steps, channels, people and leavers. One `_compute_call`,
//! `--json` as it came, else the shell's Python line for line. `-h` or
//! `--help` anywhere is the shell's.

use super::call::Ctx;
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::Raise;
use crate::out::json::Value;
use crate::out::pylib::ops::py_eq;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// The usage line (exit 2).
pub const USAGE: &str = concat!("usage: ", cli_name!(), " app chat <app> [--json]");

/// `app chat <app>`: the App and `--json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub app: String,
    pub json: bool,
}

/// The words after `chat`. `None`: a help form (the shell's).
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let (mut app, mut json) = (String::new(), false);
    for w in words {
        match *w {
            "--json" => json = true,
            _ if w.starts_with('-') => {
                return Some(Err(Stop::usage(format!(
                    concat!("Unknown flag: {} (", cli_name!(), " app chat <app> [--json])"),
                    w
                ))))
            }
            _ if !app.is_empty() => return Some(Err(Stop::usage(USAGE))),
            _ => app = w.to_string(),
        }
    }
    if app.is_empty() {
        return Some(Err(Stop::usage(USAGE)));
    }
    Some(Ok(Ask { app, json }))
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    let body = ctx.call(
        false,
        "/cli/sew/app-chat",
        &format!("app={}", quote(&ask.app)),
    )?;
    Ok(super::access::answer(&body, ask.json, lines))
}

fn dict() -> Value {
    Value::Object(Vec::new())
}

/// `", ".join(v)`.
fn joined(v: &Value, line: u32) -> Result<String, Raise> {
    py::join(", ", v, line)
}

/// `cmd_app_chat`'s Python.
fn lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let c = py::or(py::dget(&v, "chat", 4)?, dict());
    let k = py::dget(&c, "key", 5)?;
    o.line(&format!(
        "{} ({})",
        py::s(&py::dget(&v, "name", 6)?),
        py::s(&py::dget(&c, "appId", 6)?)
    ));
    if !py::truthy(&py::dget(&c, "feature", 7)?) {
        o.line("  this box's Sew has no App-governed Spaces yet (it needs Sew 0.6.0: app_governed_spaces)");
    }
    if py::truthy(&k) {
        let on = if py::truthy(&py::dget(&k, "enabled", 10)?) {
            "on"
        } else {
            "off"
        };
        o.line(&format!(
            "  chat key     {on}, level ceiling {}, up to {} people",
            py::s(&py::dget(&k, "maxLevel", 10)?),
            py::s(&py::dget(&k, "maxMembers", 10)?)
        ));
    } else {
        o.line(concat!(
            "  chat key     none (",
            cli_name!(),
            " app grant <app> --kind chat-key, the owner in person)"
        ));
    }
    let channels = py::dget_or(&c, "manifestChannels", Value::Int("0".into()), 13)?;
    o.line(&format!(
        "  manifest     {} channel(s) in its chat block",
        py::s(&channels)
    ));
    let spaces = py::or(py::dget(&c, "spaces", 14)?, py::empty_list());
    if !py::truthy(&spaces) {
        o.line(concat!(
            "  no Space is bound to this App (",
            cli_name!(),
            " chat space create <slug> --name N --app <app>)"
        ));
    }
    for s in py::iter(&spaces, 17)? {
        space(&s, o)?;
    }
    Ok(Exit::Ok)
}

/// One Space's lines.
fn space(s: &Value, o: &mut Out) -> Result<(), Raise> {
    let g = py::or(py::dget(s, "governance", 18)?, py::text("sew"));
    let conf = py::dget(s, "governanceConfirmed", 19)?;
    let note = if py_eq(&py::or(conf, py::text("sew")), &g) {
        ""
    } else {
        " (sending to Sew)"
    };
    o.line(&format!(
        "Space {}: governed by {}{note}",
        py::s(&py::dget(s, "spaceId", 21)?),
        py::s(&g)
    ));
    let sync = py::or(py::dget(s, "sync", 22)?, py::empty_list());
    for st in py::iter(&sync, 22)? {
        let mut said = String::new();
        if py::dget(&st, "lastStatus", 24)? != Value::Null {
            said.push_str(&format!(
                " Sew answered {}",
                py::s(&py::idx(&st, "lastStatus", 25)?)
            ));
        }
        if py::truthy(&py::dget(&st, "lastError", 26)?) {
            said.push_str(&format!(": {}", py::s(&py::idx(&st, "lastError", 27)?)));
        }
        o.line(&format!(
            "  step: {} {}  {}, {} try(s).{said}",
            py::s(&py::dget(&st, "op", 28)?),
            py::s(&py::dget(&st, "subject", 28)?),
            py::s(&py::dget(&st, "state", 28)?),
            py::s(&py::dget(&st, "attempts", 28)?)
        ));
        if py_eq(&py::dget(&st, "state", 29)?, &py::text("blocked")) {
            o.line(&format!(
                concat!(
                    "    blocked: Sew refused this step, so ",
                    product_name!(),
                    " won't try it again by itself. Fix what Sew said first (ask the owner if you can't), then the owner retries it: ",
                    cli_name!(),
                    " app grant hooks retry {}"
                ),
                py::s(&py::dget(&st, "id", 30)?)
            ));
        }
    }
    let chans = py::or(py::dget(s, "channels", 31)?, py::empty_list());
    for ch in py::iter(&chans, 31)? {
        let why = if py::truthy(&py::dget(&ch, "why", 32)?) {
            format!(" ({})", py::s(&py::idx(&ch, "why", 32)?))
        } else {
            String::new()
        };
        let private = if py::truthy(&py::dget(&ch, "private", 33)?) {
            "private"
        } else {
            "public"
        };
        let id = py::or(py::dget(&ch, "channelId", 33)?, py::text("-"));
        o.line(&format!(
            "  #{} {} {} {} {}{why}",
            py::left(&py::dget(&ch, "slug", 33)?, 24),
            py::left(&py::dget(&ch, "type", 33)?, 5),
            py::left(&py::text(private), 7),
            py::left(&py::dget(&ch, "state", 33)?, 8),
            py::s(&id)
        ));
    }
    let w = py::or(py::dget(s, "waiting", 34)?, dict());
    for (key, line, head) in [
        ("channels", 35, "  waiting: channels not made in Sew yet: "),
        ("agents", 37, "  waiting: agents not in chat yet: "),
        (
            "refusedAgents",
            39,
            "  refused: these may not join the App's channels (not its own workspace or a room on its roles): ",
        ),
    ] {
        if py::truthy(&py::dget(&w, key, line)?) {
            o.line(&format!(
                "{head}{}",
                joined(&py::idx(&w, key, line + 1)?, line + 1)?
            ));
        }
    }
    let people = py::or(py::dget(s, "people", 41)?, py::empty_list());
    for p in py::iter(&people, 41)? {
        let why = if py::truthy(&py::dget(&p, "why", 42)?) {
            format!(" ({})", py::s(&py::idx(&p, "why", 42)?))
        } else {
            String::new()
        };
        let who = py::or(py::dget(&p, "username", 43)?, py::dget(&p, "externalId", 43)?);
        let level = py::dget(&p, "level", 44)?;
        let state = py::dget(&p, "state", 44)?;
        let chans = py::or(py::dget(&p, "channels", 44)?, py::empty_list());
        let chans = joined(&chans, 44)?;
        let chans = if chans.is_empty() {
            "-".to_string()
        } else {
            chans
        };
        o.line(&format!(
            "  {} {} {} {chans}{why}",
            py::left(&who, 24),
            py::left(&level, 9),
            py::left(&state, 8)
        ));
    }
    let leaving = py::or(py::dget(s, "leaving", 45)?, py::empty_list());
    for l in py::iter(&leaving, 45)? {
        let state = if py_eq(&py::dget(&l, "state", 46)?, &py::text("gone")) {
            "out"
        } else {
            "being removed"
        };
        o.line(&format!(
            "  leaving: box user {} ({state})",
            py::s(&py::dget(&l, "externalId", 46)?)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_as_the_shell_reads_them() {
        assert_eq!(
            parse(&["--json", "acme"]),
            Some(Ok(Ask {
                app: "acme".into(),
                json: true
            }))
        );
        assert_eq!(parse(&["a", "b"]), Some(Err(Stop::usage(USAGE))));
        assert_eq!(parse(&[]), Some(Err(Stop::usage(USAGE))));
        assert_eq!(parse(&["a", "-h"]), None);
    }
}
