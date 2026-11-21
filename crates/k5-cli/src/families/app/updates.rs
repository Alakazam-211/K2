//! The reads of an App's update feed, as the shell CLI's `cmd_app_updates`
//! (`cli/src/app-updates.sh`) answers them: `app updates ls|list`,
//! `stats` and `fleet`, its page (`app updates`, `app updates help`) and
//! its usage line for a verb it doesn't know. `key`, `token`, `push`,
//! `promote`, `rollback` and `pull` are [`super::updates_set`]'s.

use super::access::answer;
use super::call::Ctx;
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::cli_name;

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const VERBS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app updates ls|key|token|push|promote|rollback|pull|stats|fleet \u{2026} (",
        cli_name!(),
        " app updates --help)"
    );
    pub const FLAG_TAIL: &str = concat!(" (", cli_name!(), " app updates --help)");
    pub const LS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app updates ls <app> [--channel <name>] [--release <id|version>] [--json]"
    );
    pub const STATS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app updates stats <app> [--days 30]"
    );
    pub const FLEET: &str = concat!("usage: ", cli_name!(), " app updates fleet [--json]");
}

/// `_app_updates_usage`.
pub const PAGE: &str = "app/updates";

/// What `ls` says when the App has no updater key.
const KEY_NOT_SET: &str = concat!(
    "not set: ",
    cli_name!(),
    " app updates key {app} --set <pubkey>"
);

/// What `ls` says when the App has no channel.
const NO_CHANNELS: &str = concat!(
    "Channels: none yet (",
    cli_name!(),
    " app updates push {app} --channel stable \u{2026})"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// No verb, or `help`: the page on stdout.
    Page,
    /// A verb `cmd_app_updates` doesn't know: its usage line.
    Unknown,
    Ls {
        app: String,
        channel: String,
        release: String,
        json: bool,
    },
    Stats {
        app: String,
        days: String,
        json: bool,
    },
    Fleet {
        json: bool,
    },
}

/// The words after `updates`. `None`: a help form or a verb that is the
/// shell's.
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let sub = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    Some(match sub {
        "" | "help" => Ok(Ask::Page),
        "ls" | "list" => ls(rest),
        "stats" => stats(rest),
        "fleet" => fleet(rest),
        "key" | "token" | "push" | "promote" | "rollback" | "pull" => return None,
        _ => Ok(Ask::Unknown),
    })
}

fn unknown_flag(w: &str) -> Stop {
    Stop::usage(format!("Unknown flag: {w}{}", usage::FLAG_TAIL))
}

/// `x="${2:-}"; shift 2`.
fn value<'a>(rest: &[&'a str]) -> Result<&'a str, Stop> {
    rest.get(1)
        .copied()
        .ok_or_else(|| Stop::need_value(rest[0]))
}

fn ls(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut channel, mut release, mut json) =
        (String::new(), String::new(), String::new(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--channel" => {
                channel = value(rest)?.to_string();
                rest = &rest[2..];
                continue;
            }
            "--release" => {
                release = value(rest)?.to_string();
                rest = &rest[2..];
                continue;
            }
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ if app.is_empty() => app = w.to_string(),
            _ => return Err(Stop::usage(usage::LS)),
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::LS));
    }
    Ok(Ask::Ls {
        app,
        channel,
        release,
        json,
    })
}

fn stats(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut days, mut json) = (String::new(), "30".to_string(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--days" => {
                days = value(rest)?.to_string();
                rest = &rest[2..];
                continue;
            }
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ if app.is_empty() => app = w.to_string(),
            _ => return Err(Stop::usage(usage::STATS)),
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::STATS));
    }
    Ok(Ask::Stats { app, days, json })
}

fn fleet(words: &[&str]) -> Result<Ask, Stop> {
    let mut json = false;
    for w in words {
        match *w {
            "--json" => json = true,
            _ => return Err(Stop::usage(usage::FLEET)),
        }
    }
    Ok(Ask::Fleet { json })
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Page => Ok(Shown::out(crate::text::help(PAGE).unwrap_or_default())),
        Ask::Unknown => Ok(Shown::err(format!("{}\n", usage::VERBS), Exit::Usage)),
        Ask::Ls {
            app,
            channel,
            release,
            json,
        } => {
            let mut q = format!("app={}", quote(app));
            if !channel.is_empty() {
                q.push_str(&format!("&channel={}", quote(channel)));
            }
            if !release.is_empty() {
                q.push_str(&format!("&release={}", quote(release)));
            }
            let body = ctx.call(false, "/cli/app/updates", &q)?;
            Ok(answer(&body, *json, ls_lines))
        }
        Ask::Stats { app, days, json } => {
            let q = format!("app={}&days={}", quote(app), quote(days));
            let body = ctx.call(false, "/cli/app/updates/stats", &q)?;
            Ok(answer(&body, *json, stats_lines))
        }
        Ask::Fleet { json } => {
            let body = ctx.call(false, "/cli/app/updates/fleet", "")?;
            Ok(answer(&body, *json, fleet_lines))
        }
    }
}

/// `", ".join("%s %s" % (c["channel"], c.get("version") or "-") for c in
/// chans)`.
fn channels(chans: &Value, line: u32) -> Result<String, Raise> {
    let mut parts = Vec::new();
    for c in py::iter(chans, line)? {
        let name = py::idx(&c, "channel", line)?;
        let version = py::or(py::dget(&c, "version", line)?, py::text("-"));
        parts.push(format!("{} {}", py::s(&name), py::s(&version)));
    }
    Ok(parts.join(", "))
}

const MIB: f64 = 1_048_576.0;

/// `_app_updates_ls`'s Python.
pub fn ls_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let k = py::dget(&v, "key", 4)?;
    let app = py::idx(&v, "app", 5)?;
    let dl = py::or(py::dget(&v, "dlId", 5)?, py::text("-"));
    let key = if py::truthy(&k) {
        py::s(&py::idx(&k, "keyId", 5)?)
    } else {
        KEY_NOT_SET.replace("{app}", &py::s(&py::idx(&v, "app", 5)?))
    };
    o.line(&format!(
        "{}  (download id {}; updater key {key})",
        py::s(&app),
        py::s(&dl)
    ));
    let chans = py::or(py::dget(&v, "channels", 6)?, py::empty_list());
    if py::truthy(&chans) {
        o.line(&format!("Channels: {}", channels(&chans, 8)?));
    } else {
        let app = py::s(&py::idx(&v, "app", 10)?);
        o.line(&NO_CHANNELS.replace("{app}", &app));
    }
    let u = py::or(py::dget(&v, "activeUsers", 11)?, Value::Object(Vec::new()));
    let today = py::dget_or(&u, "today", Value::Int("0".into()), 12)?;
    let week = py::dget_or(&u, "last7Days", Value::Int("0".into()), 12)?;
    let counting = py::dget_or(&u, "counting", py::text("checks"), 12)?;
    o.line(&format!(
        "Active users: {} today, {} at most on one day this week (counting {})",
        py::s(&today),
        py::s(&week),
        py::s(&counting)
    ));
    let releases = py::dget_or(&v, "releases", py::empty_list(), 13)?;
    for r in py::iter(&releases, 13)? {
        let on = py::or(py::dget(&r, "channels", 14)?, py::empty_list());
        let on = py::join(", ", &on, 14)?;
        let version = py::idx(&r, "version", 15)?;
        let state = py::idx(&r, "state", 15)?;
        let on = if on.is_empty() { "-".to_string() } else { on };
        let size = py::div(
            &py::dget_or(&r, "size", Value::Int("0".into()), 15)?,
            MIB,
            15,
        )?;
        let users = py::dget_or(&r, "activeUsers", Value::Int("0".into()), 15)?;
        let err = if py::truthy(&py::dget(&r, "error", 15)?) {
            py::concat("  ", &py::idx(&r, "error", 15)?, 15)?
        } else {
            String::new()
        };
        o.line(&format!(
            "  {} {} {} {} MiB  {} users{err}",
            py::left(&version, 14),
            py::left(&state, 9),
            crate::families::ljust(&on, 16),
            py::f(size, 6, 1),
            py::s(&users)
        ));
        let files = py::dget_or(&r, "files", py::empty_list(), 16)?;
        for f in py::iter(&files, 16)? {
            let platform = py::idx(&f, "platform", 17)?;
            let size = py::div(&py::idx(&f, "size", 17)?, MIB, 17)?;
            let at = py::or(py::dget(&f, "url", 17)?, py::dget(&f, "upload", 17)?);
            o.line(&format!(
                "      {} {} MiB  {}",
                py::left(&platform, 24),
                py::f(size, 8, 1),
                py::s(&at)
            ));
        }
    }
    let e = py::dget(&v, "endpoints", 18)?;
    if py::truthy(&e) {
        let dynamic = py::idx(&e, "dynamic", 20)?;
        o.line(&py::concat(
            "Updater endpoint (tauri.conf.json plugins.updater.endpoints): ",
            &dynamic,
            20,
        )?);
    }
    Ok(Exit::Ok)
}

/// `_app_updates_stats`'s Python.
pub fn stats_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let d = py::idx(&v, "downloads", 4)?;
    let app = py::idx(&v, "app", 5)?;
    let from = py::idx(&v, "from", 5)?;
    let to = py::idx(&v, "to", 5)?;
    o.line(&format!(
        "{}, {} to {}",
        py::s(&app),
        py::s(&from),
        py::s(&to)
    ));
    if py::truthy(&py::dget(&d, "available", 6)?) {
        let total = crate::families::py_d(&py::idx(&d, "total", 7)?, 7)?;
        o.line(&format!("Downloads: {total}"));
        let by = py::dget_or(&d, "byVersion", py::empty_list(), 8)?;
        for x in py::iter(&by, 8)? {
            let version = py::idx(&x, "version", 9)?;
            let n = py::idx(&x, "downloads", 9)?;
            o.line(&format!(
                "  {} {}",
                py::left(&version, 14),
                py::d(&n, 6, 9)?
            ));
        }
    } else {
        let why = py::dget(&d, "why", 11)?;
        o.line(&format!("Downloads: unavailable ({})", py::s(&why)));
    }
    let u = py::idx(&v, "activeUsers", 12)?;
    let mut f = Vec::new();
    for k in ["today", "last7Days", "counting", "checks"] {
        f.push(py::s(&py::idx(&u, k, 13)?));
    }
    o.line(&format!(
        "Active users: {} today, {} at most on one day this week (counting {}); {} checks",
        f[0], f[1], f[2], f[3]
    ));
    let by = py::or(py::dget(&u, "byVersion", 14)?, Value::Object(Vec::new()));
    let mut items = match &by {
        Value::Object(pairs) => pairs.clone(),
        other => return Err(Raise::no_attribute(14, other, "items")),
    };
    items.sort_by_key(|(k, _)| k.code_points());
    for (ver, n) in items {
        o.line(&format!(
            "  {} {}",
            py::left(&Value::Str(ver), 14),
            py::d(&n, 6, 15)?
        ));
    }
    Ok(Exit::Ok)
}

/// `_app_updates_fleet`'s Python.
pub fn fleet_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let apps = py::idx(&v, "apps", 4)?;
    if !py::truthy(&apps) {
        o.line("No App on this box has an update feed.");
        return Ok(Exit::Ok);
    }
    for a in py::iter(&apps, 6)? {
        let ch = channels(&py::idx(&a, "channels", 7)?, 7)?;
        let ch = if ch.is_empty() {
            "no channel".to_string()
        } else {
            ch
        };
        let app = py::idx(&a, "app", 8)?;
        let dl = py::or(py::dget(&a, "dlId", 8)?, py::text("-"));
        let releases = py::idx(&a, "releases", 8)?;
        let week = py::idx(&py::idx(&a, "activeUsers", 8)?, "last7Days", 8)?;
        let releases = crate::families::py_d(&releases, 8)?;
        o.line(&format!(
            "{} {} {ch}; {releases} releases; {} users this week",
            py::left(&app, 20),
            py::left(&dl, 36),
            py::s(&week)
        ));
    }
    Ok(Exit::Ok)
}
