//! An App's door and what it sees, as the shell CLI answers them
//! (`cli/src/app-door.sh`, `cli/src/app-recording.sh`): `app suspend`,
//! `app self`, `app usage`, `app members`, `app remote-pass` and
//! `app account-deletion`. Each is the shell's word loop, one
//! `_compute_call`, `--json` as it came, else the shell's Python line for
//! line.
//!
//! `app members` with no room: a usage line outside a session; in one
//! (`_compute_in_session`: the passport is the token) it asks for the
//! session's own room.

use super::access::answer;
use super::call::{Cj, Ctx};
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::cli_name;

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const SUSPEND: &str = concat!(
        "usage: ",
        cli_name!(),
        " app suspend <app-id> [--reason TEXT]"
    );
    pub const SELF: &str = concat!("usage: ", cli_name!(), " app self <app-id> [--json]");
    pub const USAGE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app usage <app-id> [--hours N] [--json]"
    );
    pub const HOURS: &str = "usage: --hours takes a whole number (1-720)";
    pub const MEMBERS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app members <room> [--app <app-id>|host|all] [--json]"
    );
    pub const REMOTE_PASS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app remote-pass <app-id> [--revoke [PASS-ID]] [--json]"
    );
    pub const AD_FLAG_TAIL: &str = concat!(" (", cli_name!(), " app account-deletion --help)");
    pub const AD_TOO_MANY: &str = concat!(
        "usage: too many arguments (",
        cli_name!(),
        " app account-deletion --help)"
    );
    pub const AD_LIST_WHO: &str = concat!(
        "usage: ",
        cli_name!(),
        " app account-deletion <app> (to act on one: complete|cancel <app> <request>)"
    );
    pub const AD_ACT: &str = concat!("usage: ", cli_name!(), " app account-deletion ");
}

/// `app account-deletion`'s page (`_app_account_deletion_usage`).
pub const ACCOUNT_DELETION_PAGE: &str = "app/account-deletion";

/// What `remote-pass` prints when an App has none.
const NO_PASSES: &str = concat!(
    "{app}: no remote passes (",
    cli_name!(),
    " app token rotate {app}, the owner)"
);

/// The line after a list of pending deletions.
const COMPLETE_NOW: &str = concat!(
    "Complete now: ",
    cli_name!(),
    " app account-deletion complete {app} <request|username>   Undo: ... cancel"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Suspend {
        app: String,
        reason: String,
        json: bool,
    },
    SelfView {
        app: String,
        json: bool,
    },
    Usage {
        app: String,
        hours: String,
        json: bool,
    },
    Members {
        room: String,
        app: String,
        json: bool,
    },
    RemotePass {
        app: String,
        revoke: bool,
        id: String,
        json: bool,
    },
    /// `action`: `list`, `complete` or `cancel`.
    AccountDeletion {
        action: &'static str,
        app: String,
        who: String,
        json: bool,
    },
}

/// The verbs here.
pub const VERBS: &[&str] = &[
    "suspend",
    "self",
    "usage",
    "members",
    "remote-pass",
    "account-deletion",
    "account-deletions",
];

/// The words after the verb. `None`: a help form (the shell's).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    Some(match verb {
        "suspend" => suspend(words),
        "self" => self_view(words),
        "usage" => usage_verb(words),
        "members" => members(words),
        "remote-pass" => remote_pass(words),
        "account-deletion" | "account-deletions" => account_deletion(words),
        _ => return None,
    })
}

fn unknown_flag(w: &str) -> Stop {
    Stop::usage(format!("Unknown flag: {w}"))
}

/// `x="${2:-}"; shift 2`: the value, or `set -e` ending the command (exit
/// 1, nothing said) when it is the last word.
fn value<'a>(rest: &[&'a str]) -> Result<&'a str, Stop> {
    rest.get(1)
        .copied()
        .ok_or_else(|| Stop::silent(Exit::Error))
}

/// A loop of `--json`, one `--<flag> <value>` (none when `flag` is
/// empty), and plain words (the last one wins).
fn simple(words: &[&str], flag: &str) -> Result<(String, String, bool), Stop> {
    let (mut word, mut v, mut json) = (String::new(), String::new(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        if !flag.is_empty() && w == flag {
            v = value(rest)?.to_string();
            rest = &rest[2..];
            continue;
        }
        match w {
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => word = w.to_string(),
        }
        rest = &rest[1..];
    }
    Ok((word, v, json))
}

fn suspend(words: &[&str]) -> Result<Ask, Stop> {
    let (app, reason, json) = simple(words, "--reason")?;
    if app.is_empty() {
        return Err(Stop::usage(usage::SUSPEND));
    }
    Ok(Ask::Suspend { app, reason, json })
}

fn self_view(words: &[&str]) -> Result<Ask, Stop> {
    let (app, _, json) = simple(words, "")?;
    if app.is_empty() {
        return Err(Stop::usage(usage::SELF));
    }
    Ok(Ask::SelfView { app, json })
}

fn usage_verb(words: &[&str]) -> Result<Ask, Stop> {
    let (app, hours, json) = simple(words, "--hours")?;
    if app.is_empty() {
        return Err(Stop::usage(usage::USAGE));
    }
    if !hours.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Stop::usage(usage::HOURS));
    }
    Ok(Ask::Usage { app, hours, json })
}

fn members(words: &[&str]) -> Result<Ask, Stop> {
    let (room, app, json) = simple(words, "--app")?;
    Ok(Ask::Members { room, app, json })
}

fn remote_pass(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut revoke, mut id, mut json) = (String::new(), false, String::new(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        rest = &rest[1..];
        match w {
            "--revoke" => {
                revoke = true;
                if let Some(next) = rest
                    .first()
                    .filter(|n| !n.is_empty() && !n.starts_with('-'))
                {
                    id = next.to_string();
                    rest = &rest[1..];
                }
            }
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::REMOTE_PASS));
    }
    Ok(Ask::RemotePass {
        app,
        revoke,
        id,
        json,
    })
}

fn account_deletion(words: &[&str]) -> Result<Ask, Stop> {
    let (action, mut rest) = match words.first().copied() {
        Some("complete") => ("complete", &words[1..]),
        Some("cancel") => ("cancel", &words[1..]),
        Some("list") => ("list", &words[1..]),
        _ => ("list", words),
    };
    let (mut app, mut who, mut json) = (String::new(), String::new(), false);
    while let Some(w) = rest.first().copied() {
        rest = &rest[1..];
        match w {
            "--json" => json = true,
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!(
                    "Unknown flag: {w}{}",
                    usage::AD_FLAG_TAIL
                )))
            }
            _ if app.is_empty() => app = w.to_string(),
            _ if who.is_empty() => who = w.to_string(),
            _ => return Err(Stop::usage(usage::AD_TOO_MANY)),
        }
    }
    if app.is_empty() {
        let page = crate::text::help(ACCOUNT_DELETION_PAGE).unwrap_or_default();
        return Err(Stop::usage(page.strip_suffix('\n').unwrap_or(page)));
    }
    if action == "list" && !who.is_empty() {
        return Err(Stop::usage(usage::AD_LIST_WHO));
    }
    if action != "list" && who.is_empty() {
        return Err(Stop::usage(format!(
            "{}{action} <app> <adr_\u{2026}|username>",
            usage::AD_ACT
        )));
    }
    Ok(Ask::AccountDeletion {
        action,
        app,
        who,
        json,
    })
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Suspend { app, reason, json } => {
            let b = Cj::new().s("app", app).s("reason", reason).body();
            let body = ctx.call(true, "/cli/app/suspend", &b)?;
            Ok(answer(&body, *json, suspend_lines))
        }
        Ask::SelfView { app, json } => {
            let body = ctx.call(false, "/cli/app/self", &format!("app={}", quote(app)))?;
            Ok(answer(&body, *json, self_lines))
        }
        Ask::Usage { app, hours, json } => {
            let mut q = format!("app={}", quote(app));
            if !hours.is_empty() {
                q.push_str(&format!("&hours={hours}"));
            }
            let body = ctx.call(false, "/cli/app/usage", &q)?;
            Ok(answer(&body, *json, usage_lines))
        }
        Ask::Members { room, app, json } => {
            // `_compute_in_session`: the passport is the token.
            if room.is_empty() && ctx.client.passport().is_none() {
                return Ok(Shown::err(format!("{}\n", usage::MEMBERS), Exit::Usage));
            }
            let mut q = String::new();
            if !room.is_empty() {
                q = format!("workspace={}", quote(room));
            }
            if !app.is_empty() {
                if !q.is_empty() {
                    q.push('&');
                }
                q.push_str(&format!("app={}", quote(app)));
            }
            let body = ctx.call(false, "/cli/app/members", &q)?;
            Ok(answer(&body, *json, members_lines))
        }
        Ask::RemotePass {
            app,
            revoke,
            id,
            json,
        } => {
            if *revoke {
                let b = Cj::new().s("app", app).s("id", id).body();
                let body = ctx.call(true, "/cli/app/remote-pass/revoke", &b)?;
                return Ok(answer(&body, *json, revoked_lines));
            }
            let body = ctx.call(
                false,
                "/cli/app/remote-pass",
                &format!("app={}", quote(app)),
            )?;
            Ok(answer(&body, *json, passes_lines))
        }
        Ask::AccountDeletion {
            action,
            app,
            who,
            json,
        } => {
            if *action == "list" {
                let body = ctx.call(
                    false,
                    "/cli/app/account-deletion",
                    &format!("app={}", quote(app)),
                )?;
                return Ok(answer(&body, *json, deletions_lines));
            }
            let b = Cj::new().s("app", app).s("request", who).body();
            let body = ctx.call(true, &format!("/cli/app/account-deletion/{action}"), &b)?;
            if *json {
                return Ok(raw(&body));
            }
            let mut o = Out::default();
            let r = deletion_done_lines(&body, *action == "cancel", &mut o);
            Ok(o.done(r))
        }
    }
}

/// `cmd_app_suspend`'s Python.
pub fn suspend_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let app = py::idx(&v, "app", 4)?;
    let already = if py::truthy(&py::dget(&v, "already", 4)?) {
        " (already)"
    } else {
        ""
    };
    let revoked = py::dget(&v, "passesRevoked", 4)?;
    o.line(&format!(
        "{} suspended{already}: {} passes revoked, deploys frozen",
        py::s(&app),
        py::s(&revoked)
    ));
    let st = py::dget(&v, "stopped", 5)?;
    if matches!(st, Value::Object(_)) {
        o.line(&format!(
            "  not stopped yet: {}",
            py::s(&py::dget(&st, "hint", 7)?)
        ));
    }
    let hint = py::dget_or(&v, "hint", py::text(""), 8)?;
    o.line(&format!("  {}", py::s(&hint)));
    Ok(Exit::Ok)
}

/// `cmd_app_self`'s Python.
pub fn self_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let app = py::idx(&v, "app", 4)?;
    let ws = py::idx(&v, "workspace", 4)?;
    let health = py::idx(&v, "health", 4)?;
    o.line(&format!(
        "{} (workspace {}): {}",
        py::s(&app),
        py::s(&ws),
        py::s(&health)
    ));
    let seed = py::or(py::dget(&v, "liveSeed", 5)?, py::text("none yet"));
    o.line(&format!("  live Seed   {}", py::s(&seed)));
    if py::truthy(&py::dget(&v, "deployment", 6)?) {
        let at = py::dget(&v, "deployedAt", 7)?;
        let when = if py::truthy(&at) {
            py::local_minutes(&at, 8)?
        } else {
            "not live".to_string()
        };
        let dep = py::idx(&v, "deployment", 9)?;
        o.line(&format!("  deployment  {}, live since {when}", py::s(&dep)));
    }
    Ok(Exit::Ok)
}

/// `cmd_app_usage`'s Python.
pub fn usage_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let t = py::idx(&v, "totals", 4)?;
    let q = py::idx(&v, "quotas", 4)?;
    let live = py::idx(&v, "live", 4)?;
    let mut f = Vec::new();
    f.push(py::idx(&v, "app", 5)?);
    f.push(py::idx(&v, "hours", 5)?);
    for k in ["calls", "denied", "bytesIn", "bytesOut"] {
        f.push(py::idx(&t, k, 5)?);
    }
    let f: Vec<String> = f.iter().map(py::s).collect();
    o.line(&format!(
        "{}, last {} h: {} calls, {} refused, {} B in, {} B out",
        f[0], f[1], f[2], f[3], f[4], f[5]
    ));
    let mut g = Vec::new();
    for k in ["rps", "burst", "sockets", "subscriptions"] {
        g.push(py::s(&py::idx(&q, k, 6)?));
    }
    o.line(&format!(
        "  quotas  {} calls/s (bursts of {}), {} sockets, {} subscriptions",
        g[0], g[1], g[2], g[3]
    ));
    let conns = py::idx(&live, "connections", 7)?;
    let subs = py::idx(&live, "subscriptions", 7)?;
    o.line(&format!(
        "  now     {} connections, {} subscriptions",
        py::s(&conns),
        py::s(&subs)
    ));
    let verbs = py::or(py::dget(&v, "verbs", 8)?, py::empty_list());
    for x in py::iter(&verbs, 8)? {
        let calls = py::idx(&x, "calls", 9)?;
        let verb = py::or(py::dget(&x, "verb", 9)?, py::text("-"));
        let route = py::idx(&x, "route", 9)?;
        let denied = if py::truthy(&py::idx(&x, "denied", 9)?) {
            format!("  ({} refused)", py::s(&py::idx(&x, "denied", 9)?))
        } else {
            String::new()
        };
        o.line(&format!(
            "  {}  {} {}{denied}",
            py::right(&calls, 6),
            py::left(&verb, 24),
            py::s(&route)
        ));
    }
    Ok(Exit::Ok)
}

/// `cmd_app_members`' Python.
pub fn members_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let hidden = if py::truthy(&py::dget(&v, "namesHidden", 4)?) {
        " (names hidden from Apps)"
    } else {
        ""
    };
    let ws = py::idx(&v, "workspace", 5)?;
    let count = py::idx(&v, "count", 5)?;
    let count = crate::families::py_d(&count, 5)?;
    o.line(&format!("{}: {count} member(s){hidden}", py::s(&ws)));
    let members = py::or(py::dget(&v, "members", 6)?, py::empty_list());
    for m in py::iter(&members, 6)? {
        let name = py::or(py::dget(&m, "username", 7)?, py::text("(hidden)"));
        let disp = py::dget(&m, "displayName", 8)?;
        let label = if !py::truthy(&disp) || disp == name {
            py::s(&name)
        } else {
            format!("{} ({})", py::s(&name), py::s(&disp))
        };
        let kind = py::idx(&m, "kind", 10)?;
        let app = py::idx(&m, "app", 10)?;
        let role = py::or(py::dget(&m, "role", 10)?, py::text("-"));
        let caps = py::or(py::dget(&m, "caps", 10)?, py::empty_list());
        let caps = py::join(", ", &caps, 10)?;
        o.line(&format!(
            "  {} {} {} role {} {caps}",
            crate::families::ljust(&label, 30),
            py::left(&kind, 6),
            py::left(&app, 12),
            py::left(&role, 12)
        ));
    }
    let admins = py::idx(&v, "admins", 11)?;
    o.line(&format!("  + {}", py::s(&admins)));
    Ok(Exit::Ok)
}

/// `cmd_app_remote_pass --revoke`'s Python (one line).
pub fn revoked_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 1)?;
    let app = py::idx(&v, "app", 1)?;
    let n = py::idx(&v, "revoked", 1)?;
    o.line(&format!(
        "{}: {} remote passes revoked",
        py::s(&app),
        py::s(&n)
    ));
    Ok(Exit::Ok)
}

/// `cmd_app_remote_pass`'s Python (the list).
pub fn passes_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let ps = py::or(py::dget(&v, "passes", 4)?, py::empty_list());
    if !py::truthy(&ps) {
        let app = py::s(&py::idx(&v, "app", 6)?);
        o.line(&NO_PASSES.replace("{app}", &app));
        return Ok(Exit::Ok);
    }
    let now = py::now();
    for p in py::iter(&ps, 8)? {
        let state = if py::truthy(&py::dget(&p, "revokedAt", 9)?) {
            "revoked"
        } else if py::le(&py::idx(&p, "expiresAt", 9)?, now, 9)? {
            "expired"
        } else {
            "live"
        };
        let id = py::head(&py::idx(&p, "id", 10)?, 8, 10)?;
        let prefix = py::idx(&p, "prefix", 10)?;
        let expires = py::local_minutes(&py::idx(&p, "expiresAt", 10)?, 10)?;
        let caps = py::or(py::dget(&p, "caps", 10)?, py::empty_list());
        let caps = py::join(", ", &caps, 10)?;
        let caps = if caps.is_empty() {
            "none".to_string()
        } else {
            caps
        };
        o.line(&format!(
            "{}  {} {}  expires {expires}  caps {caps}",
            py::s(&id),
            crate::families::ljust(state, 7),
            py::s(&prefix)
        ));
    }
    Ok(Exit::Ok)
}

/// `t(x)` in `cmd_app_account_deletion`'s Python, called from `line`.
fn t(x: &Value, line: u32) -> Result<String, Raise> {
    if !py::truthy(x) {
        return Ok("-".to_string());
    }
    py::local_minutes(x, 5).map_err(|e| Raise::inside(line, 5, "t", e.exception))
}

/// `cmd_app_account_deletion`'s Python (the list).
pub fn deletions_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let d = py::or(
        py::dget(&v, "accountDeletion", 4)?,
        Value::Object(Vec::new()),
    );
    let app = py::dget(&v, "app", 6)?;
    let mode = py::dget(&d, "mode", 6)?;
    let grace = py::dget(&d, "graceDays", 6)?;
    o.line(&format!(
        "App {}: account deletion {}, grace {} days",
        py::s(&app),
        py::s(&mode),
        py::s(&grace)
    ));
    let ps = py::or(py::dget(&v, "pending", 7)?, py::empty_list());
    if !py::truthy(&ps) {
        o.line("No pending deletion requests.");
    }
    for p in py::iter(&ps, 10)? {
        let id = py::idx(&p, "id", 11)?;
        let user = py::idx(&p, "username", 11)?;
        let asked = t(&py::idx(&p, "requestedAt", 11)?, 11)?;
        let due = t(&py::idx(&p, "dueAt", 11)?, 11)?;
        let what = if py::dget(&p, "scope", 11)? == py::text("account") {
            "the login goes"
        } else {
            "only this App access goes"
        };
        o.line(&format!(
            "  {} {} asked {asked}  completes {due}  ({what})",
            py::left(&id, 26),
            py::left(&user, 20)
        ));
    }
    if py::truthy(&ps) {
        let app = py::s(&py::dget(&v, "app", 13)?);
        o.line(&COMPLETE_NOW.replace("{app}", &app));
    }
    Ok(Exit::Ok)
}

/// `cmd_app_account_deletion complete|cancel`'s Python.
pub fn deletion_done_lines(body: &[u8], cancel: bool, o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    if cancel {
        let id = py::dget(&v, "requestId", 5)?;
        let user = py::dget(&v, "username", 5)?;
        let restored = py::dget(&v, "restored", 5)?;
        o.line(&format!(
            "Cancelled {} ({}): their access to this App is back ({} grant(s) on). They sign in again.",
            py::s(&id),
            py::s(&user),
            py::s(&restored)
        ));
        return Ok(Exit::Ok);
    }
    let user = py::dget(&v, "username", 7)?;
    let scope = if py::dget(&v, "scope", 7)? == py::text("account") {
        "the login"
    } else {
        "only this App access"
    };
    let done = py::or(py::dget(&v, "done", 7)?, py::empty_list());
    let done = py::join(", ", &done, 7)?;
    o.line(&format!("Deleted {} ({scope}): {done}.", py::s(&user)));
    let kept = py::or(py::dget(&v, "kept", 8)?, py::empty_list());
    for k in py::iter(&kept, 8)? {
        let item = py::idx(&k, "item", 9)?;
        let why = py::idx(&k, "why", 9)?;
        o.line(&format!("  kept {} {}", py::left(&item, 14), py::s(&why)));
    }
    o.line("Now delete what the App keeps for them in its own data.");
    Ok(Exit::Ok)
}
