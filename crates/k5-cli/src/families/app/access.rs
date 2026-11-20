//! Who reaches an App and how its people sign in, as the shell CLI's
//! `cli/src/app-people.sh` answers it: `app people`, `app discovery`,
//! `app stay-signed-in`, `app profile settings` and `app set`. Each is the
//! shell's word loop, one `_compute_call`, `--json` as it came (where the
//! verb takes it), else the shell's Python line for line.
//!
//! `app login settings` and `app invite-settings` (they read body files)
//! stay the shell's.

use super::call::{Cj, Ctx};
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::Raise;
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::cli_name;

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const PEOPLE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app people <app> [<user>] [--json]"
    );
    pub const DISCOVERY: &str = concat!("usage: ", cli_name!(), " app discovery <app> on|off");
    pub const STAY: &str = concat!(
        "usage: ",
        cli_name!(),
        " app stay-signed-in <app> <days>   (0 = off, at most 90)"
    );
    pub const PROFILE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app profile settings <app> [--names on|off] [--image on|off] [--json]"
    );
    pub const PROFILE_APP: &str = concat!(
        "usage: ",
        cli_name!(),
        " app profile settings <app> [--names on|off] [--image on|off]   (",
        cli_name!(),
        " app profile settings --help)"
    );
    pub const PROFILE_ON_OFF: &str = "usage: --names and --image take on or off";
    pub const SET_ONE_APP: &str = concat!("usage: one App only (", cli_name!(), " app set --help)");
    pub const SET_MODE: &str = "usage: --account-deletion takes request or immediate";
    pub const SET_DAYS: &str =
        "usage: --deletion-grace and --client-logs-days take a number of days (1 to 90)";
}

/// `app set`'s page (`_app_set_usage`).
pub const SET_PAGE: &str = "app/set";

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `people <app> [<user>] [--json]`.
    People {
        app: String,
        user: String,
        json: bool,
    },
    /// `discovery <app> on|off`: `listed` is `true` or `false`.
    Discovery { app: String, listed: &'static str },
    /// `stay-signed-in <app> <days>`.
    Stay { app: String, days: String },
    /// `profile settings <app> [--names on|off] [--image on|off] [--json]`.
    Profile {
        app: String,
        names: String,
        image: String,
        json: bool,
    },
    /// `set <app> [--account-deletion …] [--deletion-grace …]
    /// [--client-logs-days …] [--json]`.
    Set {
        app: String,
        mode: String,
        grace: String,
        logs: String,
        json: bool,
    },
}

/// The verbs here.
pub const VERBS: &[&str] = &["people", "discovery", "stay-signed-in", "profile", "set"];

/// The words after the verb. `None`: a help form (the shell's).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    Some(match verb {
        "people" => people(words),
        "discovery" => discovery(words),
        "stay-signed-in" => stay(words),
        "profile" => profile(words),
        "set" => set(words),
        _ => return None,
    })
}

fn unknown_flag(w: &str) -> Stop {
    Stop::usage(format!("Unknown flag: {w}"))
}

/// `shift 2` with one word left: `set -e` ends the command, exit 1, with
/// nothing said.
fn value<'a>(rest: &[&'a str]) -> Result<&'a str, Stop> {
    rest.get(1)
        .copied()
        .ok_or_else(|| Stop::silent(Exit::Error))
}

fn people(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut user, mut json) = (String::new(), String::new(), false);
    for w in words {
        match *w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ if app.is_empty() => app = w.to_string(),
            _ => user = w.to_string(),
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::PEOPLE));
    }
    Ok(Ask::People { app, user, json })
}

fn discovery(words: &[&str]) -> Result<Ask, Stop> {
    let app = words.first().copied().unwrap_or("");
    let listed = match words.get(1).copied() {
        Some("on") => "true",
        Some("off") => "false",
        _ => return Err(Stop::usage(usage::DISCOVERY)),
    };
    if app.is_empty() {
        return Err(Stop::usage(usage::DISCOVERY));
    }
    Ok(Ask::Discovery {
        app: app.to_string(),
        listed,
    })
}

fn stay(words: &[&str]) -> Result<Ask, Stop> {
    let app = words.first().copied().unwrap_or("");
    let days = words.get(1).copied().unwrap_or("");
    if days.is_empty() || !days.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Stop::usage(usage::STAY));
    }
    Ok(Ask::Stay {
        app: app.to_string(),
        days: days.to_string(),
    })
}

fn profile(words: &[&str]) -> Result<Ask, Stop> {
    if words.first() != Some(&"settings") {
        return Err(Stop::usage(usage::PROFILE));
    }
    let (mut app, mut names, mut image, mut json) =
        (String::new(), String::new(), String::new(), false);
    let mut rest = &words[1..];
    while let Some(w) = rest.first().copied() {
        match w {
            "--names" => {
                names = value(rest)?.to_string();
                rest = &rest[2..];
                continue;
            }
            "--image" | "--picture" => {
                image = value(rest)?.to_string();
                rest = &rest[2..];
                continue;
            }
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::PROFILE_APP));
    }
    for v in [&names, &image] {
        if !matches!(v.as_str(), "" | "on" | "off") {
            return Err(Stop::usage(usage::PROFILE_ON_OFF));
        }
    }
    Ok(Ask::Profile {
        app,
        names,
        image,
        json,
    })
}

/// `_app_set_usage >&2`: the page on stderr, exit 2.
fn set_page() -> Stop {
    let page = crate::text::help(SET_PAGE).unwrap_or_default();
    Stop::usage(page.strip_suffix('\n').unwrap_or(page))
}

fn set(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut mode, mut grace, mut logs, mut json) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        false,
    );
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let two = |slot: &mut String| -> Result<(), Stop> {
            *slot = value(rest)?.to_string();
            Ok(())
        };
        match w {
            "--account-deletion" => two(&mut mode)?,
            "--deletion-grace" => two(&mut grace)?,
            "--client-logs-days" => two(&mut logs)?,
            _ => {
                if let Some(v) = w.strip_prefix("--account-deletion=") {
                    mode = v.to_string();
                } else if let Some(v) = w.strip_prefix("--deletion-grace=") {
                    grace = v.to_string();
                } else if let Some(v) = w.strip_prefix("--client-logs-days=") {
                    logs = v.to_string();
                } else if w == "--json" {
                    json = true;
                } else if w.starts_with('-') {
                    return Err(Stop::usage(format!(
                        concat!("Unknown flag: {} (", cli_name!(), " app set --help)"),
                        w
                    )));
                } else if app.is_empty() {
                    app = w.to_string();
                } else {
                    return Err(Stop::usage(usage::SET_ONE_APP));
                }
                rest = &rest[1..];
                continue;
            }
        }
        rest = &rest[2..];
    }
    if app.is_empty() {
        return Err(set_page());
    }
    if !matches!(mode.as_str(), "" | "request" | "immediate") {
        return Err(Stop::usage(usage::SET_MODE));
    }
    // `grace="${grace%d}"`: one `d` off the end.
    for v in [&mut grace, &mut logs] {
        if v.ends_with('d') {
            v.pop();
        }
    }
    for v in [&grace, &logs] {
        if !v.is_empty() && !v.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Stop::usage(usage::SET_DAYS));
        }
    }
    Ok(Ask::Set {
        app,
        mode,
        grace,
        logs,
        json,
    })
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::People { app, user, json } => {
            let mut q = format!("app={}", quote(app));
            if !user.is_empty() {
                q.push_str(&format!("&user={}", quote(user)));
            }
            let body = ctx.call_on(false, "/cli/skin/access", &q, true)?;
            Ok(answer(&body, *json, people_lines))
        }
        Ask::Discovery { app, listed } => {
            let b = Cj::new().s("app", app).j("listed", listed).body();
            let body = ctx.call(true, "/cli/skin/access/listed", &b)?;
            Ok(answer(&body, false, discovery_lines))
        }
        Ask::Stay { app, days } => {
            let b = Cj::new().s("app", app).n("staySignedInDays", days).body();
            let body = ctx.call_on(true, "/cli/skin/access/settings", &b, true)?;
            Ok(answer(&body, false, stay_lines))
        }
        Ask::Profile {
            app,
            names,
            image,
            json,
        } => {
            let body = if names.is_empty() && image.is_empty() {
                ctx.call(
                    false,
                    "/cli/app/profile-settings",
                    &format!("app={}", quote(app)),
                )?
            } else {
                let bit = |v: &str| match v {
                    "" => "",
                    "on" => "1",
                    _ => "0",
                };
                let b = Cj::new()
                    .s("app", app)
                    .b("editNames", bit(names.as_str()))
                    .b("editImage", bit(image.as_str()))
                    .body();
                ctx.call(true, "/cli/app/profile-settings", &b)?
            };
            Ok(answer(&body, *json, profile_lines))
        }
        Ask::Set {
            app,
            mode,
            grace,
            logs,
            json,
        } => {
            let body = if mode.is_empty() && grace.is_empty() && logs.is_empty() {
                ctx.call(false, "/cli/app/settings", &format!("app={}", quote(app)))?
            } else {
                let b = Cj::new()
                    .s("app", app)
                    .s("accountDeletion", mode)
                    .n("deletionGraceDays", grace)
                    .n("clientLogsDays", logs)
                    .body();
                ctx.call(true, "/cli/app/settings", &b)?
            };
            Ok(answer(&body, *json, set_lines))
        }
    }
}

/// `--json`: the answer as it came; else the printer.
pub fn answer(
    body: &[u8],
    json: bool,
    lines: fn(&[u8], &mut Out) -> Result<Exit, Raise>,
) -> crate::families::Shown {
    if json {
        return raw(body);
    }
    let mut o = Out::default();
    let r = lines(body, &mut o);
    o.done(r)
}

fn on_off(v: &Value) -> &'static str {
    if py::truthy(v) {
        "on"
    } else {
        "off"
    }
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

/// `cmd_app_people`'s Python.
pub fn people_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let s = py::or(py::dget(&v, "settings", 4)?, empty_dict());
    let stay = py::dget(&s, "staySignedInDays", 5)?;
    let app = py::dget(&v, "app", 6)?;
    let max = py::dget(&s, "maxDevices", 6)?;
    let stay = if py::truthy(&stay) {
        format!("{} days", py::s(&stay))
    } else {
        "off (7-day login)".to_string()
    };
    let listed = on_off(&py::dget(&s, "listed", 6)?);
    o.line(&format!(
        "App {}: at most {} devices a person; stay signed in {stay}; listed for sign-in {listed}",
        py::s(&app),
        py::s(&max)
    ));
    let people = py::or(py::dget(&v, "people", 7)?, py::empty_list());
    if !py::truthy(&people) {
        o.line("  nobody has this App yet");
    }
    for p in py::iter(&people, 10)? {
        let who = py::or(py::dget(&p, "fullName", 11)?, py::dget(&p, "username", 11)?);
        let enabled = py::dget(&p, "enabled", 12)?;
        let state = if py::truthy(&enabled) {
            py::or(py::dget(&p, "role", 12)?, py::text("rooms only (Thread)"))
        } else {
            py::text("off")
        };
        let username = py::dget(&p, "username", 13)?;
        o.line(&format!(
            "  {} ({}) \u{b7} {}",
            py::s(&who),
            py::s(&username),
            py::s(&state)
        ));
        let cells = py::or(py::dget(&p, "cells", 14)?, py::empty_list());
        if !py::truthy(&cells) && py::truthy(&py::dget(&p, "enabled", 15)?) {
            o.line("    no agents");
        }
        for c in py::iter(&cells, 17)? {
            let f = py::or(py::dget(&c, "folders", 18)?, empty_dict());
            let mut parts = Vec::new();
            let read = py::or(py::dget(&f, "read", 19)?, py::empty_list());
            for x in py::iter(&read, 19)? {
                parts.push(py::text(&py::concat("read ", &x, 19)?));
            }
            let write = py::or(py::dget(&f, "write", 19)?, py::empty_list());
            for x in py::iter(&write, 19)? {
                parts.push(py::text(&py::concat("write ", &x, 19)?));
            }
            let folders = py::join(", ", &Value::Array(parts), 19)?;
            let handle = py::dget(&c, "handle", 20)?;
            let caps = py::or(py::dget(&c, "caps", 20)?, py::empty_list());
            let caps = py::join(", ", &caps, 20)?;
            let folders = if folders.is_empty() {
                String::new()
            } else {
                format!("  [{folders}]")
            };
            o.line(&format!("    {} {caps}{folders}", py::left(&handle, 20)));
        }
    }
    let devices = py::or(py::dget(&v, "devices", 21)?, py::empty_list());
    for d in py::iter(&devices, 21)? {
        let id = py::dget(&d, "id", 22)?;
        let username = py::dget(&d, "username", 22)?;
        let name = py::dget(&d, "name", 22)?;
        let qr = if py::dget(&d, "via", 22)? == py::text("pairing") {
            " \u{b7} paired by QR"
        } else {
            ""
        };
        o.line(&format!(
            "  device {}: {} \u{b7} {}{qr}",
            py::s(&id),
            py::s(&username),
            py::s(&name)
        ));
    }
    Ok(Exit::Ok)
}

/// `cmd_app_discovery`'s Python.
pub fn discovery_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let s = py::or(py::dget(&v, "settings", 4)?, empty_dict());
    let listed = on_off(&py::dget(&s, "listed", 5)?);
    let changed = if py::truthy(&py::dget(&v, "changed", 5)?) {
        ""
    } else {
        " (unchanged)"
    };
    o.line(&format!("Listed for sign-in: {listed}{changed}"));
    Ok(Exit::Ok)
}

/// `cmd_app_stay`'s Python.
pub fn stay_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let s = py::or(py::dget(&v, "settings", 3)?, empty_dict());
    let d = py::dget(&s, "staySignedInDays", 4)?;
    let d = if py::truthy(&d) {
        format!("{} days", py::s(&d))
    } else {
        "off (7-day login)".to_string()
    };
    o.line(&format!("Stay signed in: {d}"));
    Ok(Exit::Ok)
}

/// `cmd_app_profile_settings`' Python.
pub fn profile_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let s = py::or(py::dget(&v, "settings", 3)?, empty_dict());
    let app = py::dget(&s, "app", 4)?;
    let names = on_off(&py::dget(&s, "editNames", 4)?);
    let image = on_off(&py::dget(&s, "editImage", 4)?);
    o.line(&format!(
        "App {}: people may change their names {names}, their picture {image}",
        py::s(&app)
    ));
    Ok(Exit::Ok)
}

/// `cmd_app_set`'s Python.
pub fn set_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let d = py::or(py::dget(&v, "accountDeletion", 4)?, empty_dict());
    let c = py::or(py::dget(&v, "clientLogs", 5)?, empty_dict());
    o.line(&format!("App {}", py::s(&py::dget(&v, "app", 6)?)));
    let mode = py::dget(&d, "mode", 7)?;
    let grace = py::dget(&d, "graceDays", 7)?;
    let set = if py::truthy(&py::dget(&d, "set", 7)?) {
        ""
    } else {
        " (the defaults)"
    };
    o.line(&format!(
        "  account deletion   {}, grace {} days{set}",
        py::s(&mode),
        py::s(&grace)
    ));
    let keep = py::dget(&c, "keepDays", 8)?;
    let set = if py::truthy(&py::dget(&c, "set", 8)?) {
        ""
    } else {
        " (the default)"
    };
    o.line(&format!("  client logs kept   {} days{set}", py::s(&keep)));
    Ok(Exit::Ok)
}
