//! An App's guests and its owner grants, as the shell CLI answers them
//! (`cli/src/app-guests.sh`, and `cmd_skin`'s `grant copy-host` in
//! `cli/src/app-wallet.sh`):
//!
//! - `app guest rooms <app> <user> [set <room,…|->] [--json]`, `app guest
//!   ceiling <app> [--json]` and `app guest list <app> [--limit N]
//!   [--cursor U] [--json]`;
//! - `app grant <app> --kind guests-rooms|guests-chat|chat-key …`
//!   (`cmd_app_grant_kind`, `_app_chat_key` with its vault confirm), and
//!   `--revoke`;
//! - `app grant copy-host <app> [--dry-run] [--json]`.
//!
//! Each is the shell's word loop, then its requests ([`super::call`]),
//! the held answer ([`super::grace`]) where the shell has it, `--json`
//! as it came, else the shell's Python line for line.
//!
//! The shell answers these itself (`None`): `-h` or `--help` anywhere,
//! `guest` with no word, and a `--max` that is not plain digits
//! (`_compute_json`'s `int()` raises there and the shell posts an empty
//! body; a bash bug, reported).

use super::call::{Cj, Ctx};
use super::py::{self, Out};
use super::wallet::raw;
use super::{grace, Answer, Stop};
use crate::exit::Exit;
use crate::families::skin_token::py::comma_list;
use crate::families::{py_len, Raise};
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// The usage lines (exit 2, but where named).
pub mod usage {
    use super::cli_name;
    pub const GUEST: &str = concat!(
        "usage: ",
        cli_name!(),
        " app guest rooms|ceiling|list <app> \u{2026}"
    );
    pub const ROOMS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app guest rooms <app> <user> [set <room,...>]"
    );
    pub const ROOMS_SET: &str = concat!(
        "usage: ",
        cli_name!(),
        " app guest rooms <app> <user> set <room,room,...>   (- for no rooms)"
    );
    pub const GUEST_READ: &str = concat!("usage: ", cli_name!(), " app guest ");
    pub const KIND: [&str; 3] = [
        concat!(
            "usage: ",
            cli_name!(),
            " app grant <app> --kind guests-rooms [--room <handle>]... [--revoke]"
        ),
        concat!(
            "       ",
            cli_name!(),
            " app grant <app> --kind guests-chat [--max N] [--revoke]"
        ),
        concat!(
            "       ",
            cli_name!(),
            " app grant <app> --kind chat-key [--caps \"\u{2026}\"] [--max-level member|moderator|admin] [--uploads on|off] [--max N] [--revoke]"
        ),
    ];
    pub const UPLOADS: &str = "usage: --uploads on|off";
    /// Exit 1.
    pub const COPY_HOST: &str = concat!(
        "Usage: ",
        cli_name!(),
        " app grant copy-host <app> [--dry-run] [--json]"
    );
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `guest ceiling <app>`.
    Ceiling { app: String, json: bool },
    /// `guest list <app> [--limit N] [--cursor U]`.
    List {
        app: String,
        limit: String,
        cursor: String,
        json: bool,
    },
    /// `guest rooms <app> <user> [set <rooms>]`: `set` holds the rooms'
    /// comma list ("" for none).
    Rooms {
        app: String,
        user: String,
        set: Option<String>,
        json: bool,
    },
    /// `grant <app> --kind <kind> …`.
    Kind(Kind),
    /// `grant copy-host <app> [--dry-run]`.
    CopyHost { app: String, dry: bool, json: bool },
}

/// `cmd_app_grant_kind`'s words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Kind {
    pub app: String,
    pub kind: String,
    pub revoke: bool,
    pub json: bool,
    pub rooms_csv: String,
    pub max: String,
    pub caps: String,
    pub max_level: String,
    pub uploads: String,
}

fn help(words: &[&str]) -> bool {
    words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

/// `"${2:-}"; shift 2`: the value, or the failed `shift 2` (exit 1).
fn value<'a>(rest: &[&'a str]) -> Result<&'a str, Stop> {
    rest.get(1)
        .copied()
        .ok_or_else(|| Stop::silent(Exit::Error))
}

/// `int(v)` on what the word holds is the shell's own: plain digits only.
fn plain_number(v: &str) -> bool {
    v.is_empty() || v.bytes().all(|b| b.is_ascii_digit())
}

/// The words after `guest`. `None`: the shell's.
pub fn parse_guest(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    let what = *words.first()?;
    let rest = &words[1..];
    Some(match what {
        "rooms" => rooms(rest),
        "ceiling" | "list" => reads(what, rest),
        _ => Err(Stop::usage(usage::GUEST)),
    })
}

fn reads(what: &str, words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut limit, mut cursor, mut json) =
        (String::new(), String::new(), String::new(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--json" => json = true,
            "--limit" | "--cursor" => {
                let v = value(rest)?.to_string();
                if w == "--limit" {
                    limit = v;
                } else {
                    cursor = v;
                }
                rest = &rest[2..];
                continue;
            }
            _ if w.starts_with('-') => return Err(Stop::usage(format!("Unknown flag: {w}"))),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Err(Stop::usage(format!(
            "{}{what} <app> [--json]",
            usage::GUEST_READ
        )));
    }
    Ok(if what == "ceiling" {
        Ask::Ceiling { app, json }
    } else {
        Ask::List {
            app,
            limit,
            cursor,
            json,
        }
    })
}

fn rooms(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut user, mut json) = (String::new(), String::new(), false);
    let (mut set_mode, mut have_rooms, mut rooms_csv) = (false, false, String::new());
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        rest = &rest[1..];
        match w {
            "--json" => json = true,
            "set" => {
                set_mode = true;
                if let Some(v) = rest.first().copied().filter(|v| *v != "--json") {
                    rooms_csv = v.to_string();
                    have_rooms = true;
                    rest = &rest[1..];
                }
            }
            "-" if set_mode => {
                rooms_csv.clear();
                have_rooms = true;
            }
            _ if w.starts_with('-') => return Err(Stop::usage(format!("Unknown flag: {w}"))),
            _ if app.is_empty() => app = w.to_string(),
            _ if user.is_empty() => user = w.to_string(),
            _ => return Err(Stop::usage(format!("unexpected: {w}"))),
        }
    }
    if app.is_empty() || user.is_empty() {
        return Err(Stop::usage(usage::ROOMS));
    }
    let set = if set_mode {
        if !have_rooms {
            return Err(Stop::usage(usage::ROOMS_SET));
        }
        Some(if rooms_csv == "-" {
            String::new()
        } else {
            rooms_csv
        })
    } else {
        None
    };
    Ok(Ask::Rooms {
        app,
        user,
        set,
        json,
    })
}

/// The words after `grant` (or `grants`) that are not `cmd_skin`'s:
/// `copy-host`, or `cmd_app_grant_kind`. `None`: the shell's.
pub fn parse_grant(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    if matches!(words.first(), Some(&"copy-host" | &"copyhost")) {
        return Some(copy_host(&words[1..]));
    }
    let r = kind(words);
    if let Ok(Ask::Kind(k)) = &r {
        if !plain_number(&k.max) {
            return None;
        }
    }
    Some(r)
}

fn copy_host(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut dry, mut json) = (String::new(), false, false);
    for w in words {
        match *w {
            "--dry-run" => dry = true,
            "--json" => json = true,
            _ if w.starts_with('-') => {
                return Err(Stop {
                    line: format!("Unknown flag: {w}"),
                    exit: Exit::Error,
                })
            }
            _ if app.is_empty() => app = w.to_string(),
            _ => {}
        }
    }
    if app.is_empty() {
        return Err(Stop {
            line: usage::COPY_HOST.into(),
            exit: Exit::Error,
        });
    }
    Ok(Ask::CopyHost { app, dry, json })
}

fn kind(words: &[&str]) -> Result<Ask, Stop> {
    let mut k = Kind::default();
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let slot = match w {
            "--max" => Some(&mut k.max),
            "--caps" => Some(&mut k.caps),
            "--max-level" => Some(&mut k.max_level),
            "--uploads" => Some(&mut k.uploads),
            "--kind" => Some(&mut k.kind),
            "--room" | "--agent" => None,
            _ => {
                if let Some(v) = w.strip_prefix("--kind=") {
                    k.kind = v.to_string();
                } else if let Some(v) = w.strip_prefix("--room=") {
                    add_room(&mut k.rooms_csv, v);
                } else if w == "--revoke" {
                    k.revoke = true;
                } else if w == "--json" {
                    k.json = true;
                } else if w.starts_with('-') {
                    return Err(Stop::usage(format!("Unknown flag: {w}")));
                } else {
                    k.app = w.to_string();
                }
                rest = &rest[1..];
                continue;
            }
        };
        let v = rest.get(1).copied().unwrap_or("");
        match slot {
            Some(s) => *s = v.to_string(),
            None => add_room(&mut k.rooms_csv, v),
        }
        if rest.len() < 2 {
            return Err(Stop::silent(Exit::Error));
        }
        rest = &rest[2..];
    }
    if k.app.is_empty() || k.kind.is_empty() {
        return Err(Stop::usage(usage::KIND.join("\n")));
    }
    Ok(Ask::Kind(k))
}

/// `rooms_csv="${rooms_csv:+$rooms_csv,}<v>"`.
fn add_room(csv: &mut String, v: &str) {
    if !csv.is_empty() {
        csv.push(',');
    }
    csv.push_str(v);
}

/// `--json`: the answer as it came; else the printer.
fn answer(body: &[u8], json: bool, lines: fn(&[u8], &mut Out) -> Result<Exit, Raise>) -> Answer {
    Ok(super::access::answer(body, json, lines))
}

/// `[r.strip() for r in csv.split(",") if r.strip()]`, as JSON.
fn rooms_json(csv: &str) -> String {
    crate::families::skin_token::py::dumps(&Value::Array(comma_list(csv)))
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Ceiling { app, json } => {
            let body = ctx.call(
                false,
                "/cli/app/guest/ceiling",
                &format!("app={}", quote(app)),
            )?;
            answer(&body, *json, ceiling_lines)
        }
        Ask::List {
            app,
            limit,
            cursor,
            json,
        } => {
            let mut q = format!("app={}", quote(app));
            if !limit.is_empty() {
                q.push_str(&format!("&limit={}", quote(limit)));
            }
            if !cursor.is_empty() {
                q.push_str(&format!("&cursor={}", quote(cursor)));
            }
            let body = ctx.call(false, "/cli/app/guest/list", &q)?;
            answer(&body, *json, list_lines)
        }
        Ask::Rooms {
            app,
            user,
            set,
            json,
        } => {
            let body = match set {
                Some(csv) => {
                    let b = Cj::new()
                        .s("app", app)
                        .s("username", user)
                        .j("rooms", &rooms_json(csv))
                        .body();
                    ctx.call(true, "/cli/app/guest/rooms", &b)?
                }
                None => ctx.call(
                    false,
                    "/cli/app/guest/rooms",
                    &format!("app={}&username={}", quote(app), quote(user)),
                )?,
            };
            answer(&body, *json, rooms_lines)
        }
        Ask::Kind(k) => grant_kind(ctx, k),
        Ask::CopyHost { app, dry, json } => {
            let b = Cj::new()
                .s("app", app)
                .b("dryRun", if *dry { "1" } else { "" })
                .body();
            let s = ctx.send(true, "/cli/app/grant/copy-host", &b, false)?;
            let body = match grace::done(ctx, s, *json) {
                Ok(body) => body,
                Err(end) => return end,
            };
            answer(&body, *json, copy_host_lines)
        }
    }
}

/// `[ "$_CP_STATUS" = "200" ] || _compute_fail`.
fn ok_200(ctx: &Ctx, s: super::call::Split) -> Result<Vec<u8>, Exit> {
    if s.ok() {
        Ok(s.body)
    } else {
        Err(ctx.fail(&s))
    }
}

fn grant_kind(ctx: &mut Ctx, k: &Kind) -> Answer {
    if k.kind == "chat-key" {
        return chat_key(ctx, k);
    }
    if k.revoke {
        let b = Cj::new().s("app", &k.app).s("kind", &k.kind).body();
        let body = ctx.call(true, "/cli/app/grant/revoke", &b)?;
        if k.json {
            return Ok(raw(&body));
        }
        let mut o = Out::default();
        let r = revoke_lines(&body, &k.kind, &mut o);
        return Ok(o.done(r));
    }
    if k.kind == "guests-chat" {
        let b = Cj::new()
            .s("app", &k.app)
            .s("kind", &k.kind)
            .n("maxMembers", &k.max)
            .body();
        let s = ctx.send(true, "/cli/app/grant", &b, false)?;
        let body = ok_200(ctx, s)?;
        return answer(&body, k.json, guests_chat_lines);
    }
    let b = Cj::new()
        .s("app", &k.app)
        .s("kind", &k.kind)
        .j("rooms", &rooms_json(&k.rooms_csv))
        .body();
    let s = ctx.send(true, "/cli/app/grant", &b, false)?;
    let body = match grace::done(ctx, s, k.json) {
        Ok(body) => body,
        Err(end) => return end,
    };
    answer(&body, k.json, guests_rooms_lines)
}

fn chat_key(ctx: &mut Ctx, k: &Kind) -> Answer {
    if k.revoke {
        let b = Cj::new().s("app", &k.app).body();
        let s = ctx.send(true, "/cli/app/chat-key/revoke", &b, false)?;
        let body = ok_200(ctx, s)?;
        return answer(&body, k.json, chat_key_revoke_lines);
    }
    let up = match k.uploads.as_str() {
        "" => "",
        "on" | "true" | "yes" => "1",
        "off" | "false" | "no" => "0",
        _ => {
            crate::out::stderr_line(usage::UPLOADS);
            return Err(Exit::Usage);
        }
    };
    let caps = if k.caps.is_empty() {
        String::new()
    } else {
        let caps: Vec<Value> = k
            .caps
            .replace(',', " ")
            .split(crate::families::py_space)
            .filter(|c| !c.is_empty())
            .map(py::text)
            .collect();
        crate::families::skin_token::py::dumps(&Value::Array(caps))
    };
    let b = Cj::new()
        .s("app", &k.app)
        .j("caps", &caps)
        .s("maxLevel", &k.max_level)
        .b("uploads", up)
        .n("maxMembers", &k.max)
        .body();
    let body = super::phone_set::confirming(ctx, "/cli/app/chat-key", b, "chat-key", "")?;
    answer(&body, k.json, chat_key_lines)
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

/// `a.get("handle") or a.get("serviceId")`.
fn handle(a: &Value, line: u32) -> Result<Value, Raise> {
    Ok(py::or(
        py::dget(a, "handle", line)?,
        py::dget(a, "serviceId", line)?,
    ))
}

/// `cmd_app_guest_reads ceiling`'s Python.
fn ceiling_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let a = py::or(py::dget(&v, "appInfo", 4)?, empty_dict());
    let own = py::or(py::dget(&v, "ownRoom", 5)?, empty_dict());
    let who = py::or(py::dget(&a, "handle", 6)?, py::dget(&v, "app", 6)?);
    let room = py::or(py::dget(&own, "handle", 6)?, py::text("(gone)"));
    o.line(&format!("{}: own room {}", py::s(&who), py::s(&room)));
    if !py::truthy(&py::dget(&v, "granted", 7)?) {
        let name = py::or(py::dget(&a, "name", 8)?, py::dget(&v, "app", 8)?);
        o.line(&format!(
            concat!(
                "  no guests-rooms grant: its own code may hand out nothing (",
                cli_name!(),
                " app grant {} --kind guests-rooms). An agent with the App-users switch on sets any room itself."
            ),
            py::s(&name)
        ));
    } else {
        let grant_rooms = py::or(py::dget(&v, "grantRooms", 10)?, py::empty_list());
        let mut all = Vec::new();
        if py::truthy(&own) {
            all.push(py::dget(&own, "handle", 11)?);
        }
        for r in py::iter(&grant_rooms, 10)? {
            all.push(py::dget(&r, "handle", 10)?);
        }
        let joined = py::join(", ", &Value::Array(all), 11)?;
        let joined = if joined.is_empty() {
            "nothing".to_string()
        } else {
            joined
        };
        o.line(&format!("  granted; hands out: {joined}"));
    }
    Ok(Exit::Ok)
}

/// `cmd_app_guest_reads list`'s Python.
fn list_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let gs = py::or(py::dget(&v, "guests", 4)?, py::empty_list());
    if !py::truthy(&gs) {
        o.line(&format!("{}: no guests", py::s(&py::dget(&v, "app", 6)?)));
    }
    for g in py::iter(&gs, 7)? {
        let rooms = py::or(py::dget(&g, "roomHandles", 8)?, py::empty_list());
        let rooms = py::join(", ", &rooms, 8)?;
        let rooms = if rooms.is_empty() {
            "no rooms".to_string()
        } else {
            rooms
        };
        let mut tags = if py::truthy(&py::dget(&g, "role", 9)?) {
            py::concat(" role ", &py::idx(&g, "role", 9)?, 9)?
        } else {
            String::new()
        };
        if !py::truthy(&py::dget_or(&g, "enabled", Value::Bool(true), 10)?) {
            tags.push_str("  [disabled]");
        }
        if py::truthy(&py::dget(&g, "agent", 11)?) {
            tags.push_str("  [agent login]");
        }
        let user = py::dget(&g, "username", 12)?;
        o.line(&format!("{} {rooms}{tags}", py::left(&user, 24)));
    }
    if py::truthy(&py::dget(&v, "nextCursor", 13)?) {
        o.line(&format!(
            "more: --cursor {}",
            py::s(&py::idx(&v, "nextCursor", 14)?)
        ));
    }
    Ok(Exit::Ok)
}

/// `_app_guest_rooms_print`.
fn rooms_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let rooms = py::or(py::dget(&v, "roomHandles", 4)?, py::empty_list());
    let rooms = py::join(", ", &rooms, 4)?;
    let rooms = if rooms.is_empty() {
        "no rooms".to_string()
    } else {
        rooms
    };
    let role = if py::truthy(&py::dget(&v, "role", 5)?) {
        py::concat(" role ", &py::idx(&v, "role", 5)?, 5)?
    } else {
        " (Thread-only rooms)".to_string()
    };
    let state = if py::truthy(&py::dget_or(&v, "enabled", Value::Bool(true), 6)?) {
        ""
    } else {
        "  [disabled on this App]"
    };
    let mut line = format!(
        "{} on {}: {rooms}{role}{state}",
        py::s(&py::dget(&v, "username", 7)?),
        py::s(&py::dget(&v, "app", 7)?)
    );
    if py::contains(&v, "changed", 8)? && py::contains(&v, "before", 8)? {
        if py::truthy(&py::dget(&v, "changed", 9)?) {
            let before = py::or(py::dget(&v, "beforeHandles", 10)?, py::empty_list());
            let before = py::join(", ", &before, 10)?;
            let before = if before.is_empty() {
                "no rooms".to_string()
            } else {
                before
            };
            line.push_str(&format!("  (was: {before})"));
        } else {
            line.push_str("  (unchanged)");
        }
    }
    o.line(&line);
    Ok(Exit::Ok)
}

/// The revoke's Python; `kind` is its `sys.argv[1]`.
fn revoke_lines(body: &[u8], kind: &str, o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let a = py::idx(&v, "app", 4)?;
    let who = handle(&a, 5)?;
    let done = if py::truthy(&py::dget(&v, "revoked", 5)?) {
        "revoked"
    } else {
        "was not there"
    };
    o.line(&format!("{}: {kind} grant {done}", py::s(&who)));
    Ok(Exit::Ok)
}

/// `guests-chat`'s Python.
fn guests_chat_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let a = py::idx(&v, "app", 4)?;
    let g = py::idx(&v, "grant", 4)?;
    let who = handle(&a, 5)?;
    let max = py::dget(&g, "maxMembers", 5)?;
    let name = py::dget(&a, "name", 5)?;
    o.line(&format!(
        concat!(
            "{} may give its own guests chat: up to {} people (",
            cli_name!(),
            " app user chat <user> member|guest|none --app {})"
        ),
        py::s(&who),
        py::s(&max),
        py::s(&name)
    ));
    o.line("  its server: GET|POST /cli/skin/guests/chat with its own pass (k2.asApp().guests.setChat)");
    if py::truthy(&py::dget(&v, "warning", 7)?) {
        o.line(&format!(
            "  note: {}; until Sew has app_scoped_sessions the App cannot give chat itself (chat_not_app_scoped)",
            py::s(&py::idx(&v, "warning", 8)?)
        ));
    }
    Ok(Exit::Ok)
}

/// `guests-rooms`' Python.
fn guests_rooms_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let a = py::idx(&v, "app", 4)?;
    let g = py::idx(&v, "grant", 4)?;
    let who = handle(&a, 5)?;
    let out = py::or(py::dget(&g, "handsOut", 5)?, py::empty_list());
    let out = py::join(", ", &out, 5)?;
    let out = if out.is_empty() {
        "its own room".to_string()
    } else {
        out
    };
    o.line(&format!(
        "{} may set its own guests rooms: {out} (never above a guest role)",
        py::s(&who)
    ));
    o.line("  its server: GET|POST /cli/skin/guests/rooms with its own pass (k2.asApp().guests.setRooms)");
    Ok(Exit::Ok)
}

/// `_app_chat_key --revoke`'s Python.
fn chat_key_revoke_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let a = py::idx(&v, "app", 4)?;
    let name = py::s(&handle(&a, 5)?);
    if !py::truthy(&py::dget(&v, "revoked", 6)?) {
        o.line(&format!("{name}: its chat key was already off"));
    } else {
        o.line(&format!("{name}: chat key off (no more passes through it)"));
        o.line(if py::truthy(&py::dget(&v, "sessionsEnded", 10)?) {
            "  Sew ends this App's chat sessions"
        } else {
            "  this Sew does not end sessions by App (no app_governed_spaces); passes stop at once"
        });
    }
    Ok(Exit::Ok)
}

/// `_app_chat_key`'s Python.
fn chat_key_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let a = py::idx(&v, "app", 4)?;
    let k = py::idx(&v, "key", 4)?;
    o.line(&format!(
        concat!(
            "{} holds a chat key: ",
            product_name!(),
            " signs chat passes for its own users, no setup per person"
        ),
        py::s(&handle(&a, 5)?)
    ));
    let caps = py::or(py::dget(&k, "caps", 6)?, py::empty_list());
    let caps = py::join(" ", &caps, 6)?;
    let level = py::dget(&k, "maxLevel", 6)?;
    let max = py::dget(&k, "maxMembers", 6)?;
    let up = if py::truthy(&py::dget(&k, "uploads", 6)?) {
        "on"
    } else {
        "off"
    };
    o.line(&format!(
        "  caps {caps}, up to level {}, up to {} people, uploads {up}",
        py::s(&level),
        py::s(&max)
    ));
    o.line("  its server: chat.pass() with the person's own session (k2.withGuest); the first pass may answer chat_member_pending, retry after 2 s");
    Ok(Exit::Ok)
}

/// `grant copy-host`'s Python.
fn copy_host_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    const LABELS: [(&str, &str); 3] = [
        ("keptExistingCount", "kept (an App grant already exists)"),
        (
            "skippedOtherAppRoleCount",
            "skipped (role tagged for another App)",
        ),
        (
            "skippedOtherWorkspaceAgentCount",
            "skipped (another workspace's agent added it)",
        ),
    ];
    let d = py::load(body, 3)?;
    let verb = if py::truthy(&py::dget(&d, "dryRun", 4)?) {
        "Would copy"
    } else {
        "Copied"
    };
    let added = py::or(py::dget(&d, "added", 5)?, py::empty_list());
    let a = py::or(py::dget(&d, "appInfo", 6)?, empty_dict());
    let n = if py::contains(&d, "added", 7)? {
        py_len(&added, 7)?.to_string()
    } else {
        let c = py::or(py::dget(&d, "addedCount", 7)?, Value::Int("0".into()));
        crate::families::feedback::py_int(&c).map_err(|e| Raise::at(7, e))?
    };
    let who = py::or(
        py::dget(&a, "handle", 8)?,
        py::dget_or(&d, "app", py::text(""), 8)?,
    );
    let tail = if py::truthy(&added) {
        format!(": {}", py::join(", ", &added, 8)?)
    } else {
        String::new()
    };
    o.line(&format!(
        "{verb} the box login onto App {} for {n} user(s){tail}",
        py::s(&who)
    ));
    for (k, label) in LABELS {
        if py::truthy(&py::dget(&d, k, 10)?) {
            let count = py::d(&py::idx(&d, k, 11)?, 0, 11)?;
            o.line(&format!("  {label}: {count}"));
        }
    }
    if py::truthy(&py::dget(&d, "note", 12)?) {
        o.line(&py::concat("  ", &py::idx(&d, "note", 13)?, 13)?);
    }
    let kept = py::or(py::dget(&d, "keptExisting", 14)?, py::empty_list());
    for k in py::iter(&kept, 14)? {
        let user = py::dget_or(&k, "username", py::text(""), 15)?;
        let on = if py::truthy(&py::dget(&k, "enabled", 15)?) {
            "enabled"
        } else {
            "disabled"
        };
        o.line(&format!(
            "  kept {}: an App grant already exists ({on})",
            py::s(&user)
        ));
    }
    let other_role = py::or(py::dget(&d, "skippedOtherAppRole", 16)?, py::empty_list());
    for u in py::iter(&other_role, 16)? {
        o.line(&format!(
            "  skipped {}: the box-login role is tagged for another App",
            py::s(&u)
        ));
    }
    let other_ws = py::or(
        py::dget(&d, "skippedOtherWorkspaceAgent", 18)?,
        py::empty_list(),
    );
    for u in py::iter(&other_ws, 18)? {
        o.line(&format!(
            "  skipped {}: another workspace's agent added it",
            py::s(&u)
        ));
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(words: &[&str]) -> Option<Result<Ask, Stop>> {
        parse_guest(words)
    }

    #[test]
    fn the_guest_words_as_the_shell_reads_them() {
        assert_eq!(
            p(&["rooms", "a", "u", "set", "-"]),
            Some(Ok(Ask::Rooms {
                app: "a".into(),
                user: "u".into(),
                set: Some(String::new()),
                json: false
            }))
        );
        assert_eq!(
            p(&["rooms", "a", "u", "set", "--json"]),
            Some(Err(Stop::usage(usage::ROOMS_SET)))
        );
        assert_eq!(
            p(&["rooms", "a", "u", "set", "--json", "-"]),
            Some(Ok(Ask::Rooms {
                app: "a".into(),
                user: "u".into(),
                set: Some(String::new()),
                json: true
            }))
        );
        assert_eq!(
            p(&["rooms", "a", "u", "v"]),
            Some(Err(Stop::usage("unexpected: v")))
        );
        assert_eq!(
            p(&["list", "--limit"]),
            Some(Err(Stop::silent(Exit::Error)))
        );
        assert_eq!(p(&["nope"]), Some(Err(Stop::usage(usage::GUEST))));
        assert_eq!(p(&[]), None);
        assert_eq!(p(&["rooms", "a", "-h"]), None);
    }

    #[test]
    fn the_grant_words_as_the_shell_reads_them() {
        let Some(Ok(Ask::Kind(k))) = parse_grant(&[
            "acme",
            "--kind=guests-rooms",
            "--room",
            "a",
            "--agent",
            "b",
            "--room=c",
        ]) else {
            panic!("not a kind")
        };
        assert_eq!(k.rooms_csv, "a,b,c");
        assert_eq!(rooms_json(" a, ,b "), r#"["a", "b"]"#);
        assert_eq!(
            parse_grant(&["acme", "--kind"]),
            Some(Err(Stop::silent(Exit::Error)))
        );
        assert_eq!(
            parse_grant(&["acme", "--kind", "guests-chat", "--max", "x"]),
            None
        );
        assert_eq!(
            parse_grant(&["copy-host", "a", "b", "--dry-run"]),
            Some(Ok(Ask::CopyHost {
                app: "a".into(),
                dry: true,
                json: false
            }))
        );
    }

    /// The usage lines here and in [`super::super::sign_in`],
    /// [`super::super::chat`] and [`super::super::init`] are still the
    /// shell's, word for word (the command's name as the shell types it).
    #[test]
    fn usage_lines_are_the_shell_s() {
        use super::super::{chat, sign_in};
        const SHELL_NAME: &str = "k2";
        let src = |file: &str| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../cli/src")
                .join(file);
            std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        };
        let cases: [(&str, &[&str]); 5] = [
            (
                "app-guests.sh",
                &[
                    usage::GUEST,
                    usage::ROOMS,
                    usage::ROOMS_SET,
                    usage::GUEST_READ,
                    usage::KIND[0],
                    usage::KIND[1],
                    usage::UPLOADS,
                ],
            ),
            ("app-wallet.sh", &[usage::COPY_HOST, sign_in::usage::LOGIN]),
            (
                "app-auth.sh",
                &[
                    sign_in::usage::AUTH_HELP,
                    sign_in::usage::ORIGIN,
                    sign_in::usage::CLEAR,
                    sign_in::usage::DOOR,
                    sign_in::usage::ORIGINS,
                    sign_in::usage::BOTH,
                ],
            ),
            (
                "app-people.sh",
                &[sign_in::usage::SETTINGS, sign_in::usage::MAX_DEVICES],
            ),
            ("app-recording.sh", &[chat::USAGE]),
        ];
        for (file, lines) in cases {
            let text = src(file);
            for line in lines {
                let shell = line.replace(k2_client::brand::CLI_NAME, SHELL_NAME);
                assert!(text.contains(&shell), "{file} no longer has {shell}");
            }
        }
        let kind2 = usage::KIND[2]
            .replace(k2_client::brand::CLI_NAME, SHELL_NAME)
            .replace('"', "\\\"");
        assert!(src("app-guests.sh").contains(&kind2), "{kind2}");
    }
}
