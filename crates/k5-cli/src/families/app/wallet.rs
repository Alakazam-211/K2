//! `k5 app wallet settings|people|remove|personal-agent|personal-agents`,
//! as the shell CLI's `cmd_app_wallet` (`cli/src/app-wallet.sh`): an App
//! owner's side of App users' personal wallets.
//!
//! The words are read as the shell's loop reads them: the first is the
//! verb, then `--json`, `--on`, `--off`, `--fallback on|off`, `--clear`
//! and up to three plain words; any other flag, a fourth word or a bad
//! `--fallback` is a usage line, exit 2, before anything is sent. Each
//! verb is one `_compute_call`; `--json` prints the answer as it came.

use super::super::Shown;
use super::call::{Cj, Ctx};
use super::py::{self, Out};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::cli_name;

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const SETTINGS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app wallet settings <app> [--on|--off] [--fallback on|off]"
    );
    pub const REMOVE: &str = concat!("usage: ", cli_name!(), " app wallet remove <item-id>");
    pub const CLEAR: &str = concat!(
        "usage: ",
        cli_name!(),
        " app wallet personal-agent --clear <workspace>"
    );
    pub const PERSONAL_AGENT: &str = concat!(
        "usage: ",
        cli_name!(),
        " app wallet personal-agent <app> <user> <workspace>"
    );
    pub const VERBS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app wallet settings|people|remove|personal-agent|personal-agents (",
        cli_name!(),
        " app wallet --help)"
    );
}

/// The verbs' usage page (`_app_wallet_usage`).
pub const USAGE_PAGE: &str = "app/wallet";

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub sub: String,
    pub json: bool,
    pub a1: String,
    pub a2: String,
    pub a3: String,
    /// `--on` (`1`) or `--off` (`0`); empty for neither.
    pub on: String,
    /// `--fallback on` (`1`) or `off` (`0`); empty for neither.
    pub fallback: String,
    pub clear: bool,
    /// `-h` or `--help` in the loop: the usage page, exit 0.
    pub help: bool,
}

/// The shell's loop over the words after `wallet`. `Err`: the usage line
/// (exit 2).
pub fn parse(words: &[&str]) -> Result<Ask, String> {
    let mut ask = Ask {
        sub: words.first().copied().unwrap_or("").to_string(),
        json: false,
        a1: String::new(),
        a2: String::new(),
        a3: String::new(),
        on: String::new(),
        fallback: String::new(),
        clear: false,
        help: false,
    };
    let mut rest = words.get(1..).unwrap_or_default();
    while let Some(w) = rest.first().copied() {
        rest = &rest[1..];
        match w {
            "--json" => ask.json = true,
            "--on" => ask.on = "1".into(),
            "--off" => ask.on = "0".into(),
            "--fallback" => {
                ask.fallback = match rest.first().copied() {
                    Some("on") => "1".into(),
                    Some("off") => "0".into(),
                    _ => return Err("usage: --fallback on|off".into()),
                };
                rest = &rest[1..];
            }
            "--fallback=on" => ask.fallback = "1".into(),
            "--fallback=off" => ask.fallback = "0".into(),
            "--clear" => ask.clear = true,
            "-h" | "--help" => {
                ask.help = true;
                return Ok(ask);
            }
            _ if w.starts_with('-') => return Err(format!("Unknown flag: {w}")),
            _ => {
                if ask.a1.is_empty() {
                    ask.a1 = w.into();
                } else if ask.a2.is_empty() {
                    ask.a2 = w.into();
                } else if ask.a3.is_empty() {
                    ask.a3 = w.into();
                } else {
                    return Err(format!("unexpected: {w}"));
                }
            }
        }
    }
    Ok(ask)
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Result<Shown, Exit> {
    if ask.help {
        return Ok(usage_page());
    }
    let usage = |line: &str| Ok(Shown::err(format!("{line}\n"), Exit::Usage));
    match ask.sub.as_str() {
        "settings" => {
            if ask.a1.is_empty() {
                return usage(usage::SETTINGS);
            }
            let body = if !ask.on.is_empty() || !ask.fallback.is_empty() {
                let b = Cj::new()
                    .s("app", &ask.a1)
                    .b("enabled", &ask.on)
                    .b("fallback", &ask.fallback)
                    .body();
                ctx.call(true, "/cli/app/wallet/settings", &b)?
            } else {
                ctx.call(
                    false,
                    "/cli/app/wallet/settings",
                    &format!("app={}", quote(&ask.a1)),
                )?
            };
            Ok(answer(&body, ask.json, settings_lines))
        }
        "people" => {
            let q = if ask.a1.is_empty() {
                String::new()
            } else {
                format!("app={}", quote(&ask.a1))
            };
            let body = ctx.call(false, "/cli/app/wallet/people", &q)?;
            Ok(answer(&body, ask.json, people_lines))
        }
        "remove" | "rm" => {
            if ask.a1.is_empty() {
                return usage(usage::REMOVE);
            }
            let b = Cj::new().s("id", &ask.a1).body();
            let body = ctx.call(true, "/cli/app/wallet/remove", &b)?;
            if ask.json {
                return Ok(raw(&body));
            }
            Ok(Shown::out(format!(
                "Removed {} (deleted, not Trash; the person signs in again to use it).\n",
                ask.a1
            )))
        }
        "personal-agent" => {
            let b = if ask.clear {
                if ask.a1.is_empty() {
                    return usage(usage::CLEAR);
                }
                Cj::new().s("workspace", &ask.a1).b("clear", "1").body()
            } else {
                if ask.a3.is_empty() {
                    return usage(usage::PERSONAL_AGENT);
                }
                Cj::new()
                    .s("app", &ask.a1)
                    .s("user", &ask.a2)
                    .s("workspace", &ask.a3)
                    .body()
            };
            let body = ctx.call(true, "/cli/app/wallet/personal-agent", &b)?;
            Ok(answer(&body, ask.json, personal_agent_lines))
        }
        "personal-agents" => {
            let body = ctx.call(false, "/cli/app/wallet/personal-agents", "")?;
            Ok(answer(&body, ask.json, personal_agents_lines))
        }
        "" | "-h" | "--help" | "help" => Ok(usage_page()),
        _ => usage(usage::VERBS),
    }
}

/// `_app_wallet_usage`: the page on stdout, exit 0.
fn usage_page() -> Shown {
    Shown::out(crate::text::help(USAGE_PAGE).unwrap_or_default())
}

/// `printf '%s\n' "$_CP_BODY"`.
pub fn raw(body: &[u8]) -> Shown {
    Shown::out(format!("{}\n", String::from_utf8_lossy(body)))
}

/// `--json`: the answer as it came; else the printer.
fn answer(
    body: &[u8],
    json: bool,
    lines: fn(&[u8], &mut Out) -> Result<Exit, super::super::Raise>,
) -> Shown {
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

/// `settings`' Python.
pub fn settings_lines(body: &[u8], o: &mut Out) -> Result<Exit, super::super::Raise> {
    let v = py::load(body, 3)?;
    let s = py::idx(&v, "settings", 3)?;
    let name = if py::truthy(&py::dget(&v, "appId", 4)?) {
        let app = py::idx(&v, "appId", 4)?;
        let service = py::idx(&s, "serviceId", 4)?;
        format!("{} ({})", py::s(&app), py::s(&service))
    } else {
        py::s(&py::idx(&s, "serviceId", 4)?)
    };
    let enabled = py::idx(&s, "enabled", 5)?;
    let fallback = py::idx(&s, "fallback", 5)?;
    o.line(&format!(
        "{name}: wallet {}, fallback to the company plan {}",
        on_off(&enabled),
        on_off(&fallback)
    ));
    Ok(Exit::Ok)
}

/// `people`' Python.
pub fn people_lines(body: &[u8], o: &mut Out) -> Result<Exit, super::super::Raise> {
    let v = py::load(body, 3)?;
    let items = py::or(py::dget(&v, "items", 3)?, py::empty_list());
    if !py::truthy(&items) {
        o.line("No wallet items.");
        return Ok(Exit::Ok);
    }
    for i in py::iter(&items, 6)? {
        let used_by = py::or(py::dget(&i, "usedBy", 7)?, py::empty_list());
        let used = py::join(", ", &used_by, 7)?;
        let used = if used.is_empty() {
            "no agent".to_string()
        } else {
            used
        };
        let extra = if py::truthy(&py::dget(&i, "dormantSince", 8)?) {
            "  dormant"
        } else {
            ""
        };
        let mut f = Vec::new();
        for k in ["id", "username", "provider", "type", "label", "state"] {
            f.push(py::s(&py::idx(&i, k, 9)?));
        }
        o.line(&format!(
            "{}  {}  {} {} \"{}\"  {}  used by {used}{extra}",
            f[0], f[1], f[2], f[3], f[4], f[5]
        ));
    }
    Ok(Exit::Ok)
}

/// `personal-agent`'s Python.
pub fn personal_agent_lines(body: &[u8], o: &mut Out) -> Result<Exit, super::super::Raise> {
    let v = py::load(body, 3)?;
    if py::contains(&v, "cleared", 4)? {
        let ws = py::idx(&v, "workspace", 5)?;
        let what = if py::truthy(&py::idx(&v, "cleared", 5)?) {
            "no longer a personal agent"
        } else {
            "was not a personal agent"
        };
        o.line(&format!("{}: {what}", py::s(&ws)));
    } else {
        let ws = py::idx(&v, "workspace", 7)?;
        let user = py::idx(&v, "username", 7)?;
        let app = py::idx(&v, "app", 7)?;
        o.line(&format!(
            "{} is now {}'s own agent (through {})",
            py::s(&ws),
            py::s(&user),
            py::s(&app)
        ));
    }
    Ok(Exit::Ok)
}

/// `personal-agents`' Python.
pub fn personal_agents_lines(body: &[u8], o: &mut Out) -> Result<Exit, super::super::Raise> {
    let v = py::load(body, 3)?;
    let rows = py::or(py::dget(&v, "agents", 3)?, py::empty_list());
    if !py::truthy(&rows) {
        o.line("No personal agents.");
        return Ok(Exit::Ok);
    }
    for r in py::iter(&rows, 6)? {
        let handle = py::dget(&r, "handle", 7)?;
        let who = if py::truthy(&handle) {
            handle
        } else {
            py::idx(&r, "workspace", 7)?
        };
        let user = py::or(py::dget(&r, "username", 7)?, py::text("(gone)"));
        let app = py::idx(&r, "app", 7)?;
        o.line(&format!(
            "{}  {}  through {}",
            py::s(&who),
            py::s(&user),
            py::s(&app)
        ));
    }
    Ok(Exit::Ok)
}
