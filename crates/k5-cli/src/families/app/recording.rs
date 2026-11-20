//! An App's recording settings and a room's guest names, as the shell CLI
//! answers them (`cli/src/app-recording.sh`): `app recording <app> [set
//! …]` and `app room-names <room> hide|show`.
//!
//! - `recording`: the word loop, the read (`_compute_call`), or the change
//!   (`_app_rt_post`) with its checks. A change the daemon holds for a
//!   person's confirm (`vault_confirm_required` or
//!   `owner_confirm_required`, outside a session: `_rec_needs_confirm`)
//!   asks for a confirm token over the vault socket and sends the same
//!   body again with it, over TCP (`_rec_send_confirmed`). Then `--json`
//!   as it came, else the shell's Python line for line.
//! - `room-names`: the owner's, outside a session (refused in one before
//!   anything is sent), one POST over TCP.

use super::call::{Cj, Ctx, Split};
use super::phone_set::error_field;
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// `_app_recording_usage`'s page.
pub const PAGE: &str = "app/recording";

/// The usage lines (exit 2).
pub mod usage {
    use super::{cli_name, product_name};
    pub const START_LEVEL: &str = "usage: --start-level none|admin|moderator|member";
    pub const DAYS: &str = "usage: --audio-max-days N|org";
    pub const REQUIRED: &str = "usage: --required on|off";
    pub const SERVER_READ: &str = "usage: --server-read on|off";
    pub const FLAG_TAIL: &str = concat!(" (", cli_name!(), " app recording --help)");
    pub const ONE_APP: &str = concat!(
        "usage: one App only (",
        cli_name!(),
        " app recording --help)"
    );
    pub const TO_CHANGE: &str = concat!(
        "usage: to change a setting: ",
        cli_name!(),
        " app recording "
    );
    pub const TO_CHANGE_TAIL: &str = concat!(
        " set --start-level \u{2026} (",
        cli_name!(),
        " app recording --help)"
    );
    pub const NAME_A_CHANGE: &str =
        concat!("usage: name a change: ", cli_name!(), " app recording ");
    pub const NAME_A_CHANGE_TAIL: &str = " set [--start-level L] [--audio-max-days N|org] [--required on|off] [--server-read on|off]";
    pub const LEVELS: &str = "usage: --start-level takes none, admin, moderator or member";
    pub const DAYS_RANGE: &str =
        "usage: --audio-max-days takes a number of days (1 to 3650) or org (the server's limit)";
    pub const REQUIRED_ON_OFF: &str = "usage: --required takes on or off";
    pub const SERVER_READ_ON_OFF: &str = "usage: --server-read takes on or off";
    pub const ROOM_NAMES: &str = concat!(
        "usage: ",
        cli_name!(),
        " app room-names <room> hide|show   (the owner, outside ",
        product_name!(),
        ")"
    );
}

/// `cmd_app_room_names`'s line in a session (exit 3).
const ROOM_NAMES_OWNER_ONLY: &str = concat!(
    "{\"error\":{\"code\":\"owner_only\",\"hint\":\"only the owner hides or shows guests names in a room: ",
    cli_name!(),
    " app room-names <room> hide|show from a terminal outside ",
    product_name!(),
    "\"}}"
);

/// The printer's line when the server's chat can't record in Apps.
const NEEDS_SEW: &str = concat!(
    "  This server's chat can't record in Apps yet (it needs Sew 0.9.0: ",
    cli_name!(),
    " hostchat upgrade)."
);

/// The printer's line when no Space is bound.
const NO_SPACES: &str = concat!(
    "  Spaces         none bound (",
    cli_name!(),
    " hostchat app-space bind <app> <space-id>)"
);

/// The printer's line when the change is not delivered yet.
const NOT_DELIVERED: &str = concat!("not yet: ", product_name!(), " is sending it to Sew");

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `recording <app> [--json]`.
    Read { app: String, json: bool },
    /// `recording <app> set …`: the body `_compute_json` builds.
    Set {
        app: String,
        body: String,
        json: bool,
    },
    /// `room-names <room> hide|show`: `hide` is `1` or `0`.
    RoomNames {
        room: String,
        hide: &'static str,
        json: bool,
    },
}

/// The verbs here.
pub const VERBS: &[&str] = &["recording", "recordings", "room-names"];

/// The words after the verb. `None`: a help form (the front's).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    Some(match verb {
        "room-names" => room_names(words),
        _ => recording(words),
    })
}

/// `_rec_switch_json <v>` with no second word: `inherit` is refused.
fn switch_json(v: &str) -> Option<&'static str> {
    match v {
        "" => Some(""),
        "on" | "true" | "yes" => Some("true"),
        "off" | "false" | "no" => Some("false"),
        _ => None,
    }
}

/// `$((10#<digits>))`: base 10, wrapping as bash's arithmetic does.
fn base10(digits: &str) -> i64 {
    digits.bytes().fold(0i64, |n, b| {
        n.wrapping_mul(10).wrapping_add(i64::from(b - b'0'))
    })
}

fn recording(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut set_mode, mut json) = (String::new(), false, false);
    let (mut level, mut days, mut required, mut server_read) =
        (String::new(), String::new(), String::new(), String::new());
    let mut days_named = false;
    let mut i = 0;
    // `x="${2:-}"; shift 2 || { echo <usage>; exit 2; }`.
    let value = |i: usize, line: &str| -> Result<String, Stop> {
        match words.get(i + 1) {
            Some(v) => Ok(v.to_string()),
            None => Err(Stop::usage(line)),
        }
    };
    while i < words.len() {
        let w = words[i];
        match w {
            "set" => set_mode = true,
            "--start-level" => {
                level = value(i, usage::START_LEVEL)?;
                i += 1;
            }
            "--audio-max-days" => {
                days = value(i, usage::DAYS)?;
                days_named = true;
                i += 1;
            }
            "--required" => {
                required = value(i, usage::REQUIRED)?;
                i += 1;
            }
            "--server-read" => {
                server_read = value(i, usage::SERVER_READ)?;
                i += 1;
            }
            "--json" => json = true,
            _ if w.starts_with("--start-level=") => level = after_eq(w),
            _ if w.starts_with("--audio-max-days=") => {
                days = after_eq(w);
                days_named = true;
            }
            _ if w.starts_with("--required=") => required = after_eq(w),
            _ if w.starts_with("--server-read=") => server_read = after_eq(w),
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!(
                    "Unknown flag: {w}{}",
                    usage::FLAG_TAIL
                )))
            }
            _ => {
                if !app.is_empty() {
                    return Err(Stop::usage(usage::ONE_APP));
                }
                app = w.to_string();
            }
        }
        i += 1;
    }
    if app.is_empty() {
        let page = crate::text::help(PAGE).unwrap_or_default();
        return Err(Stop::usage(page.strip_suffix('\n').unwrap_or(page)));
    }
    let any = !level.is_empty() || days_named || !required.is_empty() || !server_read.is_empty();
    if !set_mode && any {
        return Err(Stop::usage(format!(
            "{}{app}{}",
            usage::TO_CHANGE,
            usage::TO_CHANGE_TAIL
        )));
    }
    if set_mode && !any {
        return Err(Stop::usage(format!(
            "{}{app}{}",
            usage::NAME_A_CHANGE,
            usage::NAME_A_CHANGE_TAIL
        )));
    }
    if !set_mode {
        return Ok(Ask::Read { app, json });
    }
    if !matches!(
        level.as_str(),
        "" | "none" | "admin" | "moderator" | "member"
    ) {
        return Err(Stop::usage(usage::LEVELS));
    }
    let mut days_j = String::new();
    if days_named {
        let d = days.strip_suffix('d').unwrap_or(&days);
        if d == "org" {
            days_j = "null".to_string();
        } else if !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) {
            let n = base10(d);
            if !(1..=3650).contains(&n) {
                return Err(Stop::usage(usage::DAYS_RANGE));
            }
            days_j = n.to_string();
        } else {
            return Err(Stop::usage(usage::DAYS_RANGE));
        }
    }
    let req_j = switch_json(&required).ok_or_else(|| Stop::usage(usage::REQUIRED_ON_OFF))?;
    let srv_j = switch_json(&server_read).ok_or_else(|| Stop::usage(usage::SERVER_READ_ON_OFF))?;
    let body = Cj::new()
        .s("app", &app)
        .s("startLevel", &level)
        .j("audioMaxDays", &days_j)
        .j("required", req_j)
        .j("serverRead", srv_j)
        .body();
    Ok(Ask::Set { app, body, json })
}

/// `"${1#*=}"`.
fn after_eq(w: &str) -> String {
    w.split_once('=').map_or("", |(_, v)| v).to_string()
}

fn room_names(words: &[&str]) -> Result<Ask, Stop> {
    let (mut room, mut mode, mut json) = (String::new(), String::new(), false);
    for w in words {
        match *w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(Stop::usage(format!("Unknown flag: {w}"))),
            _ if room.is_empty() => room = w.to_string(),
            _ => mode = w.to_string(),
        }
    }
    let hide = match mode.as_str() {
        "hide" => "1",
        "show" => "0",
        _ => return Err(Stop::usage(usage::ROOM_NAMES)),
    };
    if room.is_empty() {
        return Err(Stop::usage(usage::ROOM_NAMES));
    }
    Ok(Ask::RoomNames { room, hide, json })
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Read { app, json } => {
            let body = ctx.call(false, "/cli/app/recording", &format!("app={}", quote(app)))?;
            Ok(printed(&body, *json, false))
        }
        Ask::Set { app, body, json } => {
            let answer = set(ctx, app, body)?;
            Ok(printed(&answer, *json, true))
        }
        Ask::RoomNames { room, hide, json } => {
            if ctx.in_session() {
                crate::out::stderr_line(ROOM_NAMES_OWNER_ONLY);
                return Err(Exit::Refused);
            }
            let b = Cj::new().s("workspace", room).b("hide", hide).body();
            let body = ctx.call_on(true, "/cli/app/room-names", &b, true)?;
            if *json {
                return Ok(raw(&body));
            }
            let mut o = Out::default();
            let r = room_names_lines(&body, &mut o);
            Ok(o.done(r))
        }
    }
}

/// `_rec_needs_confirm`: a 403 that asks for a person's confirm, outside
/// a session.
fn needs_confirm(ctx: &Ctx, s: &Split) -> bool {
    let has = |w: &[u8]| s.body.windows(w.len()).any(|x| x == w);
    s.status == "403"
        && (has(b"\"vault_confirm_required\"") || has(b"\"owner_confirm_required\""))
        && !ctx.in_session()
}

/// `_app_rt_post /cli/app/recording/set`, the confirm when the daemon
/// holds it, then `[ 200 ] || _compute_fail`: the body of a 200.
fn set(ctx: &mut Ctx, app: &str, body: &str) -> Result<Vec<u8>, Exit> {
    const PATH: &str = "/cli/app/recording/set";
    let mut s = ctx.send(true, PATH, body, false)?;
    if needs_confirm(ctx, &s) {
        let action = error_field(&s.body, &["action"])?;
        let svc = error_field(&s.body, &["serviceId"])?;
        if action.is_empty() || svc.is_empty() {
            return Err(ctx.fail(&s));
        }
        let label = format!("Recording settings for {app}");
        let Some(tok) = crate::confirm::confirm_token(&mut ctx.confirm(), &action, &svc, &label)
        else {
            return Err(Exit::Refused);
        };
        let Some(next) = crate::confirm::with_confirm_token(body, &tok, None) else {
            return Err(Exit::Error);
        };
        drop(tok);
        s = ctx.send(true, PATH, &next, true)?;
    }
    if !s.ok() {
        return Err(ctx.fail(&s));
    }
    Ok(s.body)
}

/// `--json`: the answer as it came; else the printer.
fn printed(body: &[u8], json: bool, set_mode: bool) -> Shown {
    if json {
        return raw(body);
    }
    let mut o = Out::default();
    let r = recording_lines(body, set_mode, &mut o);
    o.done(r)
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

fn yn(v: &Value) -> &'static str {
    if py::truthy(v) {
        "yes"
    } else {
        "no"
    }
}

fn onoff(v: &Value) -> &'static str {
    if py::truthy(v) {
        "on"
    } else {
        "off"
    }
}

/// `cmd_app_recording`'s Python (`K2_RC_SET` is `set_mode`).
pub fn recording_lines(body: &[u8], set_mode: bool, o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let a = py::or(py::dget(&v, "app", 4)?, empty_dict());
    let f = py::or(py::dget(&v, "flags", 5)?, empty_dict());
    let r = py::or(py::dget(&v, "recording", 6)?, empty_dict());
    let name = py::or(py::dget(&a, "name", 9)?, py::dget(&a, "serviceId", 9)?);
    if set_mode {
        let done = if py::truthy(&py::dget(&v, "changed", 11)?) {
            "saved"
        } else {
            "nothing changed (already so)"
        };
        o.line(&format!("{}: {done}", py::s(&name)));
    }
    let svc = py::dget(&a, "serviceId", 12)?;
    o.line(&format!(
        "App {} ({}): recording",
        py::s(&name),
        py::s(&svc)
    ));
    let flags = [
        yn(&py::dget(&f, "appRecording", 14)?),
        yn(&py::dget(&f, "voiceRecordCap", 14)?),
        yn(&py::dget(&f, "recordingRequired", 14)?),
        yn(&py::dget(&f, "mgmtRecordingReads", 14)?),
    ];
    o.line(&format!(
        "  Sew flags      record in Apps {} \u{b7} voice:record cap {} \u{b7} required {} \u{b7} server reads {}",
        flags[0], flags[1], flags[2], flags[3]
    ));
    if !py::truthy(&py::dget(&f, "appRecording", 15)?) {
        o.line(NEEDS_SEW);
    }
    if py::truthy(&py::dget(&v, "homeClient", 17)?) {
        o.line("  Not for the home client: this App sees every Space a person is in, so it takes no recording settings.");
        return Ok(Exit::Ok);
    }
    let lvl = py::or(py::dget(&r, "start_level", 20)?, py::text("none"));
    let tail = if lvl == py::text("none") {
        " (nobody starts a recording)"
    } else {
        " and above may start a recording"
    };
    o.line(&format!("  start level    {}{tail}", py::s(&lvl)));
    o.line(&format!(
        "  required       {}",
        onoff(&py::dget(&r, "required", 22)?)
    ));
    o.line(&format!(
        "  server reads   {}",
        onoff(&py::dget(&r, "server_read", 23)?)
    ));
    let org = py::dget(&v, "orgAudioMaxDays", 24)?;
    let d = py::dget(&r, "audio_max_days", 25)?;
    let kept = if d != Value::Null {
        format!("{} days", py::s(&d))
    } else if org != Value::Null {
        format!("the server's limit ({} days)", py::s(&org))
    } else {
        "the server's limit".to_string()
    };
    o.line(&format!("  audio kept     {kept}"));
    let delivered = if py::truthy(&py::dget(&v, "delivered", 27)?) {
        "yes"
    } else {
        NOT_DELIVERED
    };
    o.line(&format!("  delivered      {delivered}"));
    let sp = py::or(py::dget(&v, "spaces", 28)?, py::empty_list());
    if !py::truthy(&sp) {
        o.line(NO_SPACES);
    }
    for s in py::iter(&sp, 31)? {
        let rule = if py::truthy(&py::dget(&s, "locked", 32)?) {
            "LOCKED: no recording (CUI/FCI; the owner lifts it)".to_string()
        } else {
            let req = py::dget(&s, "required", 35)?;
            let level = py::or(py::dget(&s, "startLevel", 36)?, py::text("inherit"));
            let req = if req == Value::Null {
                "inherit"
            } else {
                onoff(&req)
            };
            format!("start level {}, required {req}", py::s(&level))
        };
        let id = py::dget(&s, "spaceId", 37)?;
        let gov = py::or(py::dget(&s, "governance", 37)?, py::text("sew"));
        o.line(&format!(
            "  Space {}: governed by {}; {rule}",
            py::s(&id),
            py::s(&gov)
        ));
    }
    Ok(Exit::Ok)
}

/// `cmd_app_room_names`'s Python.
pub fn room_names_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let state = if py::truthy(&py::idx(&v, "namesHidden", 4)?) {
        "hidden from Apps"
    } else {
        "shown to Apps that hold room:members"
    };
    let ws = py::idx(&v, "workspace", 5)?;
    let already = if py::truthy(&py::dget(&v, "changed", 5)?) {
        ""
    } else {
        " (already)"
    };
    o.line(&format!("{}: guests names {state}{already}", py::s(&ws)));
    Ok(Exit::Ok)
}
