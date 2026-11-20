//! The reads of an App's phone and mail doors, as the shell CLI's
//! `cli/src/app-links.sh` and `cli/src/app-auth.sh` answer them: `app
//! links <app>` (status), `app pairing status|log`, `app push
//! status|log|devices` and `app email show|log|status`, with each
//! family's page and its usage line for a verb it doesn't know. Each read
//! is the shell's word loop, one `_compute_call`, `--json` as it came,
//! else the shell's Python line for line.
//!
//! The writes are [`super::phone_set`]'s.

use super::access::answer;
use super::call::Ctx;
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const LINKS_FLAG_TAIL: &str = concat!(" (", cli_name!(), " app links --help)");
    pub const LINKS_DASH: &str = concat!(
        "usage: ",
        cli_name!(),
        " app links <app> | set|off|check <app> (",
        cli_name!(),
        " app links --help)"
    );
    pub const LINKS_STATUS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app links status <app> (",
        cli_name!(),
        " app links --help)"
    );
    pub const LINKS_SET_ONLY: &str = concat!(" goes with ", cli_name!(), " app links set");
    pub const PAIRING: &str = concat!(
        "usage: ",
        cli_name!(),
        " app pairing status|setup|off|check|log (",
        cli_name!(),
        " app pairing --help)"
    );
    pub const PAIRING_STATUS: &str =
        concat!("usage: ", cli_name!(), " app pairing status <app> [--json]");
    pub const PAIRING_LOG: &str =
        concat!("usage: ", cli_name!(), " app pairing log <app> [--json]");
    pub const PUSH: &str = concat!(
        "usage: ",
        cli_name!(),
        " app push status|setup|creds|check|on|off|set|send|test|log|devices (",
        cli_name!(),
        " app push --help)"
    );
    pub const PUSH_STATUS: &str =
        concat!("usage: ", cli_name!(), " app push status <app> [--json]");
    pub const PUSH_LOG: &str = concat!("usage: ", cli_name!(), " app push log <app> [--json]");
    pub const PUSH_DEVICES: &str = concat!(
        "usage: ",
        cli_name!(),
        " app push devices <app> [--guest <user>] [--json]"
    );
    pub const EMAIL: &str = concat!(
        "usage: ",
        cli_name!(),
        " app email show|on|off|test|send|log|status (",
        cli_name!(),
        " app email --help)"
    );
    pub const EMAIL_SHOW: &str = concat!("usage: ", cli_name!(), " app email show <app> [--json]");
    pub const EMAIL_LOG: &str = concat!(
        "usage: ",
        cli_name!(),
        " app email log <app> [--limit N] [--status S] [--tag T] [--json]"
    );
    pub const EMAIL_STATUS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app email status <app> <id> [--json]"
    );
}

/// Which read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Read {
    Links,
    PairingStatus,
    PairingLog,
    PushStatus,
    PushLog,
    PushDevices,
    EmailShow,
    EmailLog,
    EmailStatus,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// A family's page (`help/app/<page>`), on stdout.
    Page(&'static str),
    /// A usage line on stderr, exit 2, after the gates.
    Usage(&'static str),
    /// One GET: the path, its query, and `--json`.
    Get {
        read: Read,
        query: String,
        json: bool,
    },
}

/// The verbs here.
pub const VERBS: &[&str] = &["links", "pairing", "push", "email"];

/// The words after the verb. `None`: a help form or a write (the
/// shell's).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let sub = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    let page = |p| Some(Ok(Ask::Page(p)));
    match (verb, sub) {
        ("links", "" | "help") => page("app/links"),
        ("links", "status") => Some(links(rest)),
        ("links", "set" | "off" | "check") => None,
        ("links", s) if s.starts_with('-') => Some(Err(Stop::usage(usage::LINKS_DASH))),
        ("links", _) => Some(links(words)),
        ("pairing", "" | "help") => page("app/pairing"),
        ("pairing", "status") => Some(one_app(rest, Read::PairingStatus, usage::PAIRING_STATUS)),
        ("pairing", "log") => Some(one_app(rest, Read::PairingLog, usage::PAIRING_LOG)),
        ("pairing", "setup" | "set" | "off" | "check") => None,
        ("pairing", _) => Some(Ok(Ask::Usage(usage::PAIRING))),
        ("push", "" | "help") => page("app/push"),
        ("push", "status") => Some(one_app(rest, Read::PushStatus, usage::PUSH_STATUS)),
        ("push", "log") => Some(one_app(rest, Read::PushLog, usage::PUSH_LOG)),
        ("push", "devices") => devices(rest),
        (
            "push",
            "setup" | "creds" | "credentials" | "check" | "on" | "off" | "set" | "send" | "test",
        ) => None,
        ("push", _) => Some(Ok(Ask::Usage(usage::PUSH))),
        ("email", "" | "help") => page("app/email"),
        ("email", "show" | "status-of") => Some(one_app(rest, Read::EmailShow, usage::EMAIL_SHOW)),
        ("email", "log") => Some(email_log(rest)),
        ("email", "status") => Some(email_status(rest)),
        ("email", "on" | "off" | "test" | "send") => None,
        ("email", _) => Some(Ok(Ask::Usage(usage::EMAIL))),
        _ => None,
    }
}

fn unknown_flag(w: &str) -> Stop {
    Stop::usage(format!("Unknown flag: {w}"))
}

/// `x="${2:-}"; shift 2`.
fn value<'a>(rest: &[&'a str]) -> Result<&'a str, Stop> {
    rest.get(1)
        .copied()
        .ok_or_else(|| Stop::silent(Exit::Error))
}

/// `--json`, plain words (the last wins), and nothing else.
fn one_app(words: &[&str], read: Read, line: &'static str) -> Result<Ask, Stop> {
    let (mut app, mut json) = (String::new(), false);
    for w in words {
        match *w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(line));
    }
    Ok(Ask::Get {
        read,
        query: format!("app={}", quote(&app)),
        json,
    })
}

/// `cmd_app_links`' loop for its status.
fn links(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut json) = (String::new(), false);
    for w in words {
        match *w {
            "--ios" | "--android" | "--path" | "--drop" => {
                return Err(Stop::usage(format!("{w}{}", usage::LINKS_SET_ONLY)))
            }
            "--json" => json = true,
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!(
                    "Unknown flag: {w}{}",
                    usage::LINKS_FLAG_TAIL
                )))
            }
            _ if app.is_empty() => app = w.to_string(),
            _ => {
                return Err(Stop::usage(format!(
                    "Unexpected argument: {w}{}",
                    usage::LINKS_FLAG_TAIL
                )))
            }
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::LINKS_STATUS));
    }
    Ok(Ask::Get {
        read: Read::Links,
        query: format!("app={}", quote(&app)),
        json,
    })
}

/// `cmd_app_push_devices`' loop. `None`: `devices rm` (the shell's).
fn devices(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if matches!(words.first(), Some(&"rm" | &"remove")) {
        return None;
    }
    let (mut app, mut guest, mut json) = (String::new(), String::new(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--guest" | "--user" => {
                match value(rest) {
                    Ok(v) => guest = v.to_string(),
                    Err(stop) => return Some(Err(stop)),
                }
                rest = &rest[2..];
                continue;
            }
            "--json" => json = true,
            _ if w.starts_with('-') => return Some(Err(unknown_flag(w))),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Some(Err(Stop::usage(usage::PUSH_DEVICES)));
    }
    let mut query = format!("app={}", quote(&app));
    if !guest.is_empty() {
        query.push_str(&format!("&guest={}", quote(&guest)));
    }
    Some(Ok(Ask::Get {
        read: Read::PushDevices,
        query,
        json,
    }))
}

fn email_log(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut limit, mut status, mut tag, mut json) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        false,
    );
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let slot = match w {
            "--limit" => Some(&mut limit),
            "--status" => Some(&mut status),
            "--tag" => Some(&mut tag),
            _ => None,
        };
        if let Some(slot) = slot {
            *slot = value(rest)?.to_string();
            rest = &rest[2..];
            continue;
        }
        match w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::EMAIL_LOG));
    }
    let mut query = format!("app={}", quote(&app));
    for (k, v) in [("limit", &limit), ("status", &status), ("tag", &tag)] {
        if !v.is_empty() {
            query.push_str(&format!("&{k}={}", quote(v)));
        }
    }
    Ok(Ask::Get {
        read: Read::EmailLog,
        query,
        json,
    })
}

fn email_status(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut id, mut json) = (String::new(), String::new(), false);
    for w in words {
        match *w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ if app.is_empty() => app = w.to_string(),
            _ => id = w.to_string(),
        }
    }
    if app.is_empty() || id.is_empty() {
        return Err(Stop::usage(usage::EMAIL_STATUS));
    }
    Ok(Ask::Get {
        read: Read::EmailStatus,
        query: format!("app={}&id={}", quote(&app), quote(&id)),
        json,
    })
}

/// A read's printer: the shell's Python for its answer.
type Printer = fn(&[u8], &mut Out) -> Result<Exit, Raise>;

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    let (read, query, json) = match ask {
        Ask::Page(p) => return Ok(Shown::out(crate::text::help(p).unwrap_or_default())),
        Ask::Usage(line) => return Ok(Shown::err(format!("{line}\n"), Exit::Usage)),
        Ask::Get { read, query, json } => (*read, query, *json),
    };
    let (path, lines): (&str, Printer) = match read {
        Read::Links => ("/cli/app/links", links_lines),
        Read::PairingStatus => ("/cli/app/pairing", pairing_lines),
        Read::PairingLog => ("/cli/app/pairing/log", pairing_log_lines),
        Read::PushStatus => ("/cli/app/push", push_lines),
        Read::PushLog => ("/cli/app/push/log", push_log_lines),
        Read::PushDevices => ("/cli/app/push/devices", devices_lines),
        Read::EmailShow => ("/cli/app/email", email_lines),
        Read::EmailLog => ("/cli/app/email/log", email_log_lines),
        Read::EmailStatus => ("/cli/app/email/status", email_status_lines),
    };
    let body = ctx.call(false, path, query)?;
    Ok(answer(&body, json, lines))
}

/// `v.get("name") or v.get("app")`.
fn name(v: &Value, line: u32) -> Result<Value, Raise> {
    Ok(py::or(
        py::dget(v, "name", line)?,
        py::dget(v, "app", line)?,
    ))
}

/// `", ".join(v)`, or `-` when `v` is empty.
fn joined_or_dash(v: &Value, line: u32) -> Result<String, Raise> {
    if !py::truthy(v) {
        return Ok("-".to_string());
    }
    py::join(", ", v, line)
}

/// `cmd_app_links`' Python (the status).
pub fn links_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let who = name(&v, 4)?;
    let state = py::dget(&v, "state", 4)?;
    let served = if py::truthy(&py::dget_or(&v, "served", Value::Bool(true), 4)?) {
        String::new()
    } else {
        let why = py::or(py::dget(&v, "notServed", 4)?, py::text("not served"));
        format!(" ({})", py::concat("", &why, 4)?)
    };
    o.line(&format!(
        "{}: link files {}{served}",
        py::s(&who),
        py::s(&state)
    ));
    if py::truthy(&py::dget(&v, "pairing", 5)?) {
        o.line(concat!(
            "  phone pairing is on too: ",
            product_name!(),
            " serves one file for both"
        ));
    }
    let ios = py::or(py::dget(&v, "ios", 7)?, py::empty_list());
    let mut android = Vec::new();
    for a in py::iter(&py::or(py::dget(&v, "android", 8)?, py::empty_list()), 8)? {
        android.push(py::dget(&a, "package", 8)?);
    }
    o.line(&format!("  iOS apps:     {}", joined_or_dash(&ios, 9)?));
    o.line(&format!(
        "  Android apps: {}",
        joined_or_dash(&Value::Array(android), 10)?
    ));
    let paths = py::or(py::dget(&v, "paths", 11)?, py::empty_list());
    if py::truthy(&paths) {
        o.line("  paths (first shipped):");
        for p in py::iter(&paths, 14)? {
            let spec = py::dget(&p, "spec", 15)?;
            let since = py::or(py::dget(&p, "since", 15)?, Value::Int("0".into()));
            let since = py::local_time(&since, "%Y-%m-%d", 15)?;
            o.line(&format!("    {} {since}", py::left(&spec, 40)));
        }
    } else {
        o.line("  paths: -");
    }
    let hosts = py::or(py::dget(&v, "hosts", 18)?, py::empty_list());
    let hosts = py::join(", ", &hosts, 18)?;
    let hosts = if hosts.is_empty() {
        "- (the App has no public host yet)".to_string()
    } else {
        hosts
    };
    o.line(&format!("  hosts:        {hosts}"));
    let lc = py::dget(&v, "lastCheck", 19)?;
    if py::truthy(&lc) {
        let at = py::or(py::dget(&lc, "at", 21)?, Value::Int("0".into()));
        o.line(&format!("  last check ({}):", py::local_minutes(&at, 21)?));
        for h in py::iter(&py::or(py::dget(&lc, "hosts", 22)?, py::empty_list()), 22)? {
            let mut f = Vec::new();
            for k in ["host", "aasa", "assetlinks", "appleCdn"] {
                f.push(py::s(&py::dget(&h, k, 23)?));
            }
            o.line(&format!(
                "    {}: iOS file {}, Android file {}, Apple CDN {}",
                f[0], f[1], f[2], f[3]
            ));
        }
    }
    for w in py::iter(&py::or(py::dget(&v, "warnings", 24)?, py::empty_list()), 24)? {
        o.line(&py::concat("note: ", &w, 25)?);
    }
    Ok(Exit::Ok)
}

/// `_app_pairing_status_print`'s Python.
pub fn pairing_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    const WHY: &[(&str, &str)] = &[
        ("owner_off", "turned off"),
        (
            "not_set_up",
            concat!("not set up yet (", cli_name!(), " app pairing setup)"),
        ),
        ("box_off", "K2_APP_PAIRING=off on this server"),
        ("no_app", "no such App"),
    ];
    let v = py::load(body, 3)?;
    let state = py::dget(&v, "state", 4)?;
    let mut line = format!("{}: phone pairing {}", py::s(&name(&v, 5)?), py::s(&state));
    if state != py::text("on") {
        let off = py::dget(&v, "off", 7)?;
        let mut why = py::word_for(WHY, &off, py::or(off.clone(), py::text("")), 7)?;
        if !py::truthy(&py::dget(&v, "served", 9)?) && py::truthy(&py::dget(&v, "notServed", 9)?) {
            let head = if py::truthy(&why) {
                py::concat("", &why, 10)? + "; "
            } else {
                String::new()
            };
            why = py::text(&py::concat(&head, &py::idx(&v, "notServed", 10)?, 10)?);
        }
        line.push_str(&format!(" ({})", py::concat("", &why, 11)?));
    }
    o.line(&line);
    if py::truthy(&py::dget(&v, "desktopOnly", 13)?) {
        o.line(
            "  apps:     desktop only (no phone app; people type the code into your desktop app)",
        );
    }
    for i in py::iter(&py::or(py::dget(&v, "ios", 15)?, py::empty_list()), 15)? {
        o.line(&py::concat("  iOS:      ", &i, 16)?);
    }
    for a in py::iter(&py::or(py::dget(&v, "android", 17)?, py::empty_list()), 17)? {
        let package = py::dget(&a, "package", 18)?;
        let n =
            crate::families::py_len(&py::or(py::dget(&a, "sha256", 18)?, py::empty_list()), 18)?;
        o.line(&format!(
            "  Android:  {} ({n} fingerprint{})",
            py::s(&package),
            if n == 1 { "" } else { "s" }
        ));
    }
    let hosts = py::or(py::dget(&v, "hosts", 19)?, py::empty_list());
    for h in py::iter(&hosts, 19)? {
        let host = py::dget(&h, "host", 20)?;
        let pairs = if py::truthy(&py::dget(&h, "pairs", 20)?) {
            ""
        } else {
            " (not pairing)"
        };
        o.line(&format!("  host:     {}{pairs}", py::s(&host)));
    }
    if !py::truthy(&hosts) {
        o.line(concat!(
            "  host:     none (the App needs ",
            product_name!(),
            " Connect and its own label)"
        ));
    }
    for x in py::iter(
        &py::or(py::dget(&v, "nativeOrigins", 23)?, py::empty_list()),
        23,
    )? {
        o.line(&py::concat("  webview:  ", &x, 24)?);
    }
    for x in py::iter(
        &py::or(py::dget(&v, "linkPaths", 25)?, py::empty_list()),
        25,
    )? {
        o.line(&py::concat("  link:     ", &x, 26)?);
    }
    let st = py::or(py::dget(&v, "store", 27)?, Value::Object(Vec::new()));
    for (k, n) in [("ios", "App Store"), ("android", "Google Play")] {
        if py::truthy(&py::dget(&st, k, 29)?) {
            let at = py::idx(&st, k, 30)?;
            o.line(&format!(
                "  {} {}",
                crate::families::ljust(&format!("{n}:"), 9),
                py::s(&at)
            ));
        }
    }
    let paired = py::dget_or(&v, "pairedPhones", Value::Int("0".into()), 31)?;
    let day = py::dget_or(&v, "pairingsLastDay", Value::Int("0".into()), 31)?;
    let stay = if py::truthy(&py::dget(&v, "staySignedInDays", 32)?) {
        format!("{} days", py::s(&py::idx(&v, "staySignedInDays", 32)?))
    } else {
        "off (no pairing)".to_string()
    };
    o.line(&format!(
        "  phones:   {} paired now, {} in the last day; stay signed in {stay}",
        py::s(&paired),
        py::s(&day)
    ));
    let lc = py::dget(&v, "lastCheck", 33)?;
    if py::truthy(&lc) {
        for h in py::iter(&py::or(py::dget(&lc, "hosts", 35)?, py::empty_list()), 35)? {
            let mut f = Vec::new();
            for k in ["host", "aasa", "assetlinks", "appleCdn"] {
                f.push(py::s(&py::dget(&h, k, 36)?));
            }
            o.line(&format!(
                "  checked:  {} aasa {}, assetlinks {}, Apple CDN {}",
                f[0], f[1], f[2], f[3]
            ));
        }
    }
    for w in py::iter(&py::or(py::dget(&v, "warnings", 37)?, py::empty_list()), 37)? {
        o.line(&py::concat("  note: ", &w, 38)?);
    }
    Ok(Exit::Ok)
}

/// `cmd_app_pairing_log`'s Python.
pub fn pairing_log_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let rows = py::or(py::dget(&v, "pairings", 4)?, py::empty_list());
    if !py::truthy(&rows) {
        o.line(&format!(
            "{}: no phone has paired yet",
            py::s(&name(&v, 6)?)
        ));
    }
    for r in py::iter(&rows, 7)? {
        let at = py::or(py::dget(&r, "pairedAt", 8)?, Value::Int("0".into()));
        let when = py::local_minutes(&at, 8)?;
        let state = if py::truthy(&py::dget(&r, "live", 9)?) {
            "live".to_string()
        } else {
            let ended = py::or(py::dget(&r, "ended", 9)?, py::text("expired"));
            py::concat("ended: ", &ended, 9)?
        };
        let user = py::dget(&r, "username", 10)?;
        let device = py::or(py::dget(&r, "deviceName", 10)?, py::text(""));
        let device = py::head(&device, 24, 10)?;
        let host = py::dget(&r, "host", 10)?;
        o.line(&format!(
            "{when}  {} {} {}  {state}",
            py::left(&user, 16),
            py::left(&device, 24),
            py::s(&host)
        ));
    }
    Ok(Exit::Ok)
}

/// `_app_push_status_print`'s Python.
pub fn push_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    const WHY: &[(&str, &str)] = &[
        ("owner_off", "the owner turned them off"),
        (
            "not_approved",
            concat!(
                "not approved yet (",
                cli_name!(),
                " app push on, or approve push:send)"
            ),
        ),
        ("box_off", "K2_APP_PUSH=off on this server"),
    ];
    let v = py::load(body, 3)?;
    let state = py::dget(&v, "state", 4)?;
    let mut line = format!("{}: notifications {}", py::s(&name(&v, 5)?), py::s(&state));
    if state != py::text("on") {
        let off = py::dget(&v, "off", 7)?;
        let why = py::word_for(WHY, &off, py::or(off.clone(), py::text("")), 7)?;
        line.push_str(&format!(" ({})", py::concat("", &why, 9)?));
    }
    o.line(&line);
    let a = py::dget(&v, "apns", 11)?;
    let apple = if py::truthy(&a) {
        let mut f = Vec::new();
        for k in ["keyId", "teamId", "bundleId", "keyFingerprint"] {
            f.push(py::s(&py::idx(&a, k, 12)?));
        }
        format!(
            "key {}, team {}, bundle {}, fingerprint {}",
            f[0], f[1], f[2], f[3]
        )
    } else {
        "no key".to_string()
    };
    o.line(&format!("  Apple:    {apple}"));
    let f = py::dget(&v, "fcm", 13)?;
    let firebase = if py::truthy(&f) {
        let mut g = Vec::new();
        for k in ["projectId", "clientEmail", "keyFingerprint"] {
            g.push(py::s(&py::idx(&f, k, 14)?));
        }
        format!("project {}, {}, fingerprint {}", g[0], g[1], g[2])
    } else {
        "no service account".to_string()
    };
    o.line(&format!("  Firebase: {firebase}"));
    if py::truthy(&py::dget(&v, "credError", 15)?) {
        o.line(&format!(
            "  last key refusal: {}",
            py::s(&py::idx(&v, "credError", 16)?)
        ));
    }
    let d = py::or(py::dget(&v, "devices", 17)?, Value::Object(Vec::new()));
    let mut g = Vec::new();
    for k in ["total", "guests", "apns", "fcm", "sandbox"] {
        g.push(py::s(&py::dget_or(&d, k, Value::Int("0".into()), 18)?));
    }
    o.line(&format!(
        "  devices:  {} for {} people (apns {}, fcm {}; sandbox {})",
        g[0], g[1], g[2], g[3], g[4]
    ));
    if py::truthy(&py::dget(&v, "serverOnly", 19)?) {
        o.line(concat!(
            "  devices register only through this App server (",
            cli_name!(),
            " app push set <app> --register any to change)"
        ));
    }
    let on = |b: bool| if b { "on" } else { "off" };
    let replies = on(py::truthy(&py::dget(&v, "agentReplies", 21)?));
    let previews = on(py::truthy(&py::dget(&v, "previews", 21)?));
    let max = py::dget(&v, "maxDevices", 21)?;
    o.line(&format!(
        "  agent replies {replies}, previews {previews}, at most {} devices",
        py::s(&max)
    ));
    if py::contains(&v, "chatPushes", 22)? {
        let pushes = py::truthy(&py::dget(&v, "chatPushes", 23)?);
        let mut chat = on(pushes).to_string();
        if pushes && !py::truthy(&py::dget(&v, "chatPushesBox", 24)?) {
            chat.push_str(" (waiting: K2_APP_CHAT_PUSH is not on for this server)");
        }
        o.line(&format!("  chat pushes {chat}"));
    }
    for s in py::iter(&py::or(py::dget(&v, "set", 27)?, py::empty_list()), 27)? {
        o.line(&py::concat("  set: ", &s, 28)?);
    }
    for r in py::iter(&py::or(py::dget(&v, "removed", 29)?, py::empty_list()), 29)? {
        o.line(&py::concat("  removed: ", &r, 30)?);
    }
    Ok(Exit::Ok)
}

/// `cmd_app_push_log`'s Python.
pub fn push_log_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let rows = py::or(py::dget(&v, "sends", 4)?, py::empty_list());
    if !py::truthy(&rows) {
        o.line("No sends in the last hour.");
        return Ok(Exit::Ok);
    }
    for r in py::iter(&rows, 7)? {
        let at = py::or(py::dget(&r, "at", 8)?, Value::Int("0".into()));
        let at = py::date_time(&at, "%H:%M:%S", 8)?;
        let skipped = py::or(py::dget(&r, "skipped", 9)?, Value::Object(Vec::new()));
        let sk = py::sum_values(&skipped, 9)?;
        let mut f = Vec::new();
        for k in [
            "sendId", "queued", "sent", "dead", "failed", "wrongApp", "dropped",
        ] {
            f.push(py::s(&py::dget(&r, k, 10)?));
        }
        o.line(&format!(
            "{at}  {}  queued {}  sent {}  dead {}  failed {}  wrong app {}  dropped {}  skipped {}",
            f[0],
            f[1],
            f[2],
            f[3],
            f[4],
            f[5],
            f[6],
            py::s(&sk)
        ));
    }
    if py::truthy(&py::dget(&v, "pending", 12)?) {
        o.line(&format!(
            "{} device send(s) still going out",
            py::s(&py::idx(&v, "pending", 13)?)
        ));
    }
    Ok(Exit::Ok)
}

/// `cmd_app_push_devices`' Python.
pub fn devices_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let ds = py::or(py::dget(&v, "devices", 4)?, py::empty_list());
    if !py::truthy(&ds) {
        o.line("No registered phones.");
        return Ok(Exit::Ok);
    }
    for d in py::iter(&ds, 7)? {
        let seen = py::or(py::dget(&d, "lastSeenAt", 8)?, Value::Int("0".into()));
        let seen = py::date_time(&seen, "%Y-%m-%d %H:%M", 8)?;
        let id = py::idx(&d, "id", 9)?;
        let user = py::head(&py::or(py::dget(&d, "username", 9)?, py::text("-")), 16, 9)?;
        let platform = py::dget(&d, "platform", 9)?;
        let env = py::dget(&d, "environment", 9)?;
        let device = py::head(&py::or(py::dget(&d, "deviceId", 9)?, py::text("")), 24, 9)?;
        o.line(&format!(
            "{}  {} {} {} {} seen {seen}",
            py::s(&id),
            py::left(&user, 16),
            py::left(&platform, 5),
            py::left(&env, 10),
            py::left(&device, 24)
        ));
    }
    Ok(Exit::Ok)
}

/// `_app_email_state_print`'s Python.
pub fn email_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let s = py::or(py::dget(&v, "sender", 4)?, Value::Object(Vec::new()));
    let on = py::truthy(&py::dget(&v, "on", 5)?);
    let state = if on { "on" } else { "off" };
    let who = name(&v, 6)?;
    let switch = if py::dget(&v, "boxSwitch", 6)? == py::text("on") {
        ""
    } else {
        " (K2_EMAIL_NOTIFICATIONS=off on this server)"
    };
    o.line(&format!("{}: email {state}{switch}", py::s(&who)));
    if py::truthy(&s) {
        let mut f = Vec::new();
        for k in ["address", "fromName", "path", "state"] {
            f.push(py::s(&py::dget(&s, k, 8)?));
        }
        o.line(&format!(
            "  sender:  {} ({}, {}, {})",
            f[0], f[1], f[2], f[3]
        ));
    } else {
        o.line(concat!(
            "  sender:  none (the owner grants one: ",
            cli_name!(),
            " app email on <app> --sender <address>)"
        ));
    }
    let scope = py::dget(&v, "recipientScope", 11)?;
    o.line(&format!("  scope:   {}", py::s(&scope)));
    if py::truthy(&py::dget(&v, "offReason", 12)?) && !on {
        o.line(&format!(
            "  off:     {}",
            py::s(&py::idx(&v, "offReason", 13)?)
        ));
    }
    Ok(Exit::Ok)
}

/// `to[0]`: a list's first item, a string's first character.
fn first(to: &Value, line: u32) -> Result<Value, Raise> {
    match to {
        Value::Array(items) => Ok(items[0].clone()),
        Value::Str(_) => py::head(to, 1, line),
        Value::Object(_) => Err(Raise::at(line, "KeyError: 0")),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                crate::out::py::type_name(other)
            ),
        )),
    }
}

/// `_email_log_print`'s Python.
pub fn email_log_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let rows = py::or(py::dget(&v, "items", 4)?, py::empty_list());
    if !py::truthy(&rows) {
        o.line("No email in the log (it keeps 30 days).");
        return Ok(Exit::Ok);
    }
    for r in py::iter(&rows, 7)? {
        let at = py::or(py::dget(&r, "createdAt", 8)?, Value::Int("0".into()));
        let at = py::date_time(&at, "%Y-%m-%d %H:%M", 8)?;
        let to = py::or(py::dget(&r, "to", 9)?, py::empty_list());
        let who = if py::truthy(&to) {
            let n = crate::families::py_len(&to, 10)?;
            let more = if n > 1 {
                format!(" +{}", n - 1)
            } else {
                String::new()
            };
            let head = first(&to, 10)?;
            match head {
                Value::Str(_) => format!("{}{more}", py::s(&head)),
                other => {
                    return Err(Raise::at(
                        10,
                        format!(
                            "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                            crate::out::py::type_name(&other)
                        ),
                    ))
                }
            }
        } else {
            "-".to_string()
        };
        let extra = if py::truthy(&py::dget(&r, "error", 11)?) {
            py::concat(" ", &py::idx(&r, "error", 11)?, 11)?
        } else {
            String::new()
        };
        let what = py::or(
            py::or(py::dget(&r, "subject", 12)?, py::dget(&r, "purpose", 12)?),
            py::text(""),
        );
        let id = py::dget(&r, "id", 13)?;
        let status = py::dget(&r, "status", 13)?;
        let what = py::head(&what, 60, 13)?;
        o.line(&format!(
            "{at}  {}  {}{extra}  {who}  {}",
            py::s(&id),
            py::left(&status, 9),
            py::s(&what)
        ));
    }
    Ok(Exit::Ok)
}

/// `cmd_app_email_status`' Python (one line).
pub fn email_status_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 1)?;
    let id = py::dget(&v, "id", 1)?;
    let status = py::dget(&v, "status", 1)?;
    let err = if py::truthy(&py::dget(&v, "error", 1)?) {
        py::concat(" ", &py::idx(&v, "error", 1)?, 1)?
    } else {
        String::new()
    };
    let n = py::dget(&v, "recipients", 1)?;
    o.line(&format!(
        "{}: {}{err} ({} recipients)",
        py::s(&id),
        py::s(&status),
        py::s(&n)
    ));
    Ok(Exit::Ok)
}
