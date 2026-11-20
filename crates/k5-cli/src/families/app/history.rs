//! What an App did and what went wrong, as the shell CLI's
//! `cli/src/app-doctor.sh` answers it: `app history`, `app errors` and
//! `app logs` (one read, and `--client`). Each is the shell's word loop,
//! one `_compute_call`, `--json` as it came where the verb takes it, else
//! the shell's Python line for line.
//!
//! Still the shell's: `app logs --follow` (a poll loop), and `app logs
//! --client --since`, whose request names a time the shell reads from the
//! clock (`date +%s`).

use super::access::answer;
use super::call::Ctx;
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::Raise;
use crate::out::json::{self, Options, Value};
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const HISTORY: &str = concat!(
        "usage: ",
        cli_name!(),
        " app history <app-id> [--users] [--limit N]  |  ",
        cli_name!(),
        " app history --widget <name>"
    );
    pub const ERRORS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app errors <app-id> [--since 1h] [--kind crash|health|build|deploy] [--json]"
    );
    pub const ERRORS_KIND: &str = "usage: --kind crash|health|build|deploy";
    pub const ERRORS_FOLLOW: &str = concat!(
        cli_name!(),
        " app errors --follow arrives with the runtime error pipe; loop on ",
        cli_name!(),
        " app errors for now"
    );
    pub const LOGS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app logs <app-id> [--deployment D] [--tail N] [--follow]  |  ",
        cli_name!(),
        " app logs <app> --client"
    );
    pub const LOGS_CLIENT_ONLY: &str = concat!(
        "usage: --user, --kind, --since, --limit and --json go with --client (",
        cli_name!(),
        " app logs <app> --client)"
    );
    pub const CLIENT: &str = concat!(
        "usage: ",
        cli_name!(),
        " app logs <app> --client [--user U] [--kind crash|log] [--since 1d|12h|30m] [--limit N] [--json]"
    );
    pub const CLIENT_KIND: &str = "usage: --kind takes crash or log";
    pub const CLIENT_LIMIT: &str = "usage: --limit takes a number (1 to 500)";
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    History {
        app: String,
        widget: String,
        limit: String,
        users: bool,
        json: bool,
    },
    Errors {
        app: String,
        since: String,
        kind: String,
        json: bool,
    },
    Logs {
        app: String,
        deployment: String,
        tail: String,
    },
    ClientLogs {
        app: String,
        user: String,
        kind: String,
        limit: String,
        json: bool,
    },
}

/// The verbs here.
pub const VERBS: &[&str] = &["history", "errors", "logs"];

/// The words after the verb. `None`: a help form, or a form that is the
/// shell's.
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    match verb {
        "history" => Some(history(words)),
        "errors" => Some(errors(words)),
        "logs" => logs(words),
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

fn history(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut widget, mut limit, mut users, mut json) =
        (String::new(), String::new(), String::new(), false, false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--widget" => {
                widget = value(rest)?.to_string();
                rest = &rest[2..];
                continue;
            }
            "--limit" => {
                limit = value(rest)?.to_string();
                rest = &rest[2..];
                continue;
            }
            "--users" => users = true,
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    if widget.is_empty() && app.is_empty() {
        return Err(Stop::usage(usage::HISTORY));
    }
    Ok(Ask::History {
        app,
        widget,
        limit,
        users,
        json,
    })
}

fn errors(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut since, mut kind, mut json) =
        (String::new(), "3600".to_string(), String::new(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--since" => {
                since = value(rest)?.to_string();
                rest = &rest[2..];
                continue;
            }
            "--kind" => {
                kind = value(rest)?.to_string();
                rest = &rest[2..];
                if !matches!(kind.as_str(), "crash" | "health" | "build" | "deploy") {
                    return Err(Stop::usage(usage::ERRORS_KIND));
                }
                continue;
            }
            "--json" => json = true,
            "--follow" => return Err(Stop::usage(usage::ERRORS_FOLLOW)),
            _ if w.starts_with('-') => return Err(unknown_flag(w)),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::ERRORS));
    }
    Ok(Ask::Errors {
        app,
        since,
        kind,
        json,
    })
}

/// `cmd_app_logs`'s loop. `None`: `--follow`, or `--client` with
/// `--since` (the shell's).
fn logs(words: &[&str]) -> Option<Result<Ask, Stop>> {
    let mut app = String::new();
    let mut deployment = String::new();
    let mut tail = "200".to_string();
    let (mut follow, mut client, mut json) = (false, false, false);
    let (mut user, mut kind, mut since, mut limit) =
        (String::new(), String::new(), String::new(), String::new());
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let slot = match w {
            "--deployment" => Some(&mut deployment),
            "--tail" => Some(&mut tail),
            "--user" => Some(&mut user),
            "--kind" => Some(&mut kind),
            "--since" => Some(&mut since),
            "--limit" => Some(&mut limit),
            _ => None,
        };
        if let Some(slot) = slot {
            match value(rest) {
                Ok(v) => *slot = v.to_string(),
                Err(stop) => return Some(Err(stop)),
            }
            rest = &rest[2..];
            continue;
        }
        match w {
            "--follow" | "-f" => follow = true,
            "--client" => client = true,
            "--json" => json = true,
            _ if w.starts_with('-') => return Some(Err(unknown_flag(w))),
            _ => app = w.to_string(),
        }
        rest = &rest[1..];
    }
    if client {
        if app.is_empty() {
            return Some(Err(Stop::usage(usage::CLIENT)));
        }
        if !matches!(kind.as_str(), "" | "crash" | "log") {
            return Some(Err(Stop::usage(usage::CLIENT_KIND)));
        }
        if !limit.is_empty() && !limit.bytes().all(|b| b.is_ascii_digit()) {
            return Some(Err(Stop::usage(usage::CLIENT_LIMIT)));
        }
        if !since.is_empty() {
            return None;
        }
        return Some(Ok(Ask::ClientLogs {
            app,
            user,
            kind,
            limit,
            json,
        }));
    }
    if !(user.is_empty() && kind.is_empty() && since.is_empty() && limit.is_empty()) || json {
        return Some(Err(Stop::usage(usage::LOGS_CLIENT_ONLY)));
    }
    if app.is_empty() {
        return Some(Err(Stop::usage(usage::LOGS)));
    }
    if follow {
        return None;
    }
    Some(Ok(Ask::Logs {
        app,
        deployment,
        tail,
    }))
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::History {
            app,
            widget,
            limit,
            users,
            json,
        } => {
            let mut q = if widget.is_empty() {
                format!("app={}", quote(app))
            } else {
                format!("widget={}", quote(widget))
            };
            if !limit.is_empty() {
                q.push_str(&format!("&limit={}", quote(limit)));
            }
            if *users {
                q.push_str("&users=1");
            }
            let body = ctx.call(false, "/cli/app/history", &q)?;
            Ok(answer(&body, *json, history_lines))
        }
        Ask::Errors {
            app,
            since,
            kind,
            json,
        } => {
            let mut q = format!("app={}&since={}", quote(app), quote(since));
            if !kind.is_empty() {
                q.push_str(&format!("&kind={}", quote(kind)));
            }
            let body = ctx.call(false, "/cli/app/errors", &q)?;
            Ok(answer(&body, *json, errors_lines))
        }
        Ask::Logs {
            app,
            deployment,
            tail,
        } => {
            let mut q = format!("app={}&tail={}", quote(app), quote(tail));
            if !deployment.is_empty() {
                q.push_str(&format!("&deployment={}", quote(deployment)));
            }
            let body = ctx.call(false, "/cli/app/logs", &q)?;
            Ok(answer(&body, false, framed_line))
        }
        Ask::ClientLogs {
            app,
            user,
            kind,
            limit,
            json,
        } => {
            let mut q = format!("app={}", quote(app));
            if !user.is_empty() {
                q.push_str(&format!("&user={}", quote(user)));
            }
            if !kind.is_empty() {
                q.push_str(&format!("&kind={kind}"));
            }
            if !limit.is_empty() {
                q.push_str(&format!("&limit={limit}"));
            }
            let body = ctx.call(false, "/cli/app/client-logs", &q)?;
            Ok(answer(&body, *json, client_lines))
        }
    }
}

/// `t(x)` in the history Python: `%Y-%m-%d %H:%M:%S`, or `-`; called from
/// `line`.
fn t(x: &Value, line: u32) -> Result<String, Raise> {
    if !py::truthy(x) {
        return Ok("-".to_string());
    }
    py::local_time(x, "%Y-%m-%d %H:%M:%S", 5).map_err(|e| Raise::inside(line, 5, "t", e.exception))
}

/// `json.dumps(v, sort_keys=True)`.
fn dumps_sorted(v: &Value) -> String {
    json::to_string(
        v,
        &Options {
            sort_keys: true,
            ..Options::default()
        },
    )
    .unwrap_or_default()
}

/// `(v or "-")[:12]`.
fn short(v: &Value, line: u32) -> Result<String, Raise> {
    Ok(py::s(&py::head(
        &py::or(v.clone(), py::text("-")),
        12,
        line,
    )?))
}

const BROKEN: &str = concat!("BROKEN (", cli_name!(), " app inspect)");

/// `cmd_app_history`'s Python.
pub fn history_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    if py::truthy(&py::dget(&v, "users", 6)?) {
        let audit = py::dget_or(&v, "audit", py::empty_list(), 7)?;
        for r in py::iter(&audit, 7)? {
            let at = t(&py::idx(&r, "at", 8)?, 8)?;
            let event = py::idx(&r, "event", 8)?;
            let actor = py::idx(&r, "actor", 8)?;
            let payload = dumps_sorted(&py::idx(&r, "payload", 8)?);
            o.line(&format!(
                "{at}  {} {} {payload}",
                py::left(&event, 26),
                py::left(&actor, 20)
            ));
        }
        if !py::truthy(&py::dget(&v, "audit", 9)?) {
            o.line("Nothing about this App's people yet.");
        }
        return Ok(Exit::Ok);
    }
    if py::contains(&v, "widget", 12)? {
        let audit = py::dget_or(&v, "audit", py::empty_list(), 13)?;
        for r in py::iter(&audit, 13)? {
            let at = t(&py::idx(&r, "at", 14)?, 14)?;
            let event = py::idx(&r, "event", 14)?;
            let actor = py::idx(&r, "actor", 14)?;
            let payload = dumps_sorted(&py::idx(&r, "payload", 14)?);
            o.line(&format!(
                "{at}  {} {} {payload}",
                py::left(&event, 10),
                py::left(&actor, 12)
            ));
        }
        if !py::truthy(&py::dget(&v, "audit", 15)?) {
            let widget = py::idx(&v, "widget", 16)?;
            o.line(&format!(
                "No deploy-audit rows for widget {} yet.",
                py::s(&widget)
            ));
        }
        return Ok(Exit::Ok);
    }
    let deployments = py::dget_or(&v, "deployments", py::empty_list(), 18)?;
    for d in py::iter(&deployments, 18)? {
        let mut live = String::new();
        if py::dget(&d, "toSeed", 19)? == py::dget(&v, "liveSeed", 19)?
            && py::idx(&d, "result", 19)? == py::text("live")
        {
            live.push_str("  <- live");
        }
        if py::truthy(&py::dget(&d, "rollbackable", 20)?) {
            live.push_str(if py::truthy(&py::dget(&d, "freedSoon", 21)?) {
                "  can roll back (freed soon)"
            } else {
                "  can roll back"
            });
        } else if py::truthy(&py::dget(&d, "crossesMigration", 22)?) {
            live.push_str("  across a data migration: refused");
        } else if py::truthy(&py::dget(&d, "liveAt", 24)?)
            && py::truthy(&py::dget(&d, "toSeed", 24)?)
            && py::dget(&d, "seedStored", 24)? == Value::Bool(false)
        {
            live.push_str("  Seed freed");
        }
        let started = t(&py::idx(&d, "startedAt", 26)?, 26)?;
        let kind = py::idx(&d, "kind", 26)?;
        let result = py::idx(&d, "result", 26)?;
        let from = short(&py::dget(&d, "fromSeed", 27)?, 27)?;
        let to = short(&py::dget(&d, "toSeed", 27)?, 27)?;
        let actor = py::idx(&d, "actor", 27)?;
        let commit = short(&py::dget(&d, "commitRef", 27)?, 27)?;
        o.line(&format!(
            "{started}  {} {} {from} -> {to}  by {} commit {commit}{live}",
            py::left(&kind, 13),
            py::left(&result, 11),
            py::left(&actor, 18)
        ));
        if py::truthy(&py::dget(&d, "reason", 28)?) {
            o.line(&format!("    why: {}", py::s(&py::idx(&d, "reason", 29)?)));
        }
        if py::truthy(&py::dget(&d, "error", 30)?) {
            o.line(&format!("    error: {}", py::s(&py::idx(&d, "error", 31)?)));
        }
    }
    if py::contains(&v, "keepDeployments", 32)? {
        let to = py::or(py::dget(&v, "rollbackTo", 33)?, py::empty_list());
        if py::truthy(&to) {
            let mut parts = Vec::new();
            for x in py::iter(&to, 35)? {
                let seed = py::head(&py::idx(&x, "seed", 35)?, 12, 35)?;
                let at = t(&py::idx(&x, "startedAt", 35)?, 35)?;
                parts.push(format!("{} ({at})", py::s(&seed)));
            }
            let app = py::idx(&v, "app", 35)?;
            o.line(&format!(
                concat!(
                    "roll back to: {} (",
                    cli_name!(),
                    " app rollback {} --to <seed>)"
                ),
                parts.join(", "),
                py::s(&app)
            ));
        } else {
            o.line(concat!(
                "roll back: nothing ",
                product_name!(),
                " keeps can be rolled back to"
            ));
        }
        let n = py::idx(&v, "keepDeployments", 38)?;
        let d = py::idx(&v, "keepUnusedDays", 38)?;
        let n = crate::families::py_d(&n, 38)?;
        let d = crate::families::py_d(&d, 38)?;
        o.line(&format!(
            concat!(
                product_name!(),
                " keeps the Seeds of the last {} deployments of each App and frees older ones {} days after they last ran."
            ),
            n, d
        ));
    }
    let verified = if py::truthy(&py::dget(&v, "auditVerified", 39)?) {
        "verifies"
    } else {
        BROKEN
    };
    o.line(&format!("audit chain: {verified}"));
    Ok(Exit::Ok)
}

/// `cmd_app_errors`' Python.
pub fn errors_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let fs = py::dget_or(&v, "failures", py::empty_list(), 4)?;
    o.line(if py::truthy(&fs) {
        "Failures (newest first):"
    } else {
        "No crashes, failed health gates or failed builds in the window."
    });
    for f in py::iter(&fs, 6)? {
        let mut how = Vec::new();
        if py::dget(&f, "exitCode", 8)? != Value::Null {
            how.push(format!("exit {}", py::s(&py::idx(&f, "exitCode", 9)?)));
        }
        if py::dget(&f, "signal", 10)? != Value::Null {
            how.push(format!("signal {}", py::s(&py::idx(&f, "signal", 11)?)));
        }
        if py::truthy(&py::dget(&f, "restartCount", 12)?) {
            how.push(format!(
                "restart {}",
                py::s(&py::idx(&f, "restartCount", 13)?)
            ));
        }
        if py::truthy(&py::dget(&f, "rolledBack", 14)?) {
            how.push("rolled back".to_string());
        }
        let at = py::div(&py::idx(&f, "atMs", 16)?, 1000.0, 16)?;
        let at = py::local_time(&Value::Float(at), "%m-%d %H:%M:%S", 16)?;
        let kind = py::idx(&f, "kind", 16)?;
        let dep = py::or(py::dget(&f, "deployment", 16)?, py::text("-"));
        let how = if how.is_empty() {
            String::new()
        } else {
            format!(" ({})", how.join(", "))
        };
        let reason = py::idx(&f, "reason", 16)?;
        o.line(&format!(
            "{at}  {} {}{how}: {}",
            py::left(&kind, 6),
            py::s(&dep),
            py::s(&reason)
        ));
    }
    if py::truthy(&py::dget(&v, "kind", 18)?) {
        return Ok(Exit::Ok);
    }
    o.line("");
    let events = py::dget_or(&v, "events", py::empty_list(), 21)?;
    for e in py::iter(&events, 21)? {
        let at = py::local_time(&py::idx(&e, "at", 22)?, "%H:%M:%S", 22)?;
        let kind = py::idx(&e, "kind", 22)?;
        let dep = py::idx(&e, "deployment", 22)?;
        let result = py::idx(&e, "result", 22)?;
        let err = py::or(py::dget(&e, "error", 22)?, py::text(""));
        o.line(&format!(
            "{at}  {} {}: {} \u{2014} {}",
            py::s(&kind),
            py::s(&dep),
            py::s(&result),
            py::s(&err)
        ));
    }
    if !py::truthy(&py::dget(&v, "events", 23)?) {
        o.line("No failed, rolled-back or stalled deploys in the window.");
    }
    o.line(&py::s(&py::idx(&v, "framed", 25)?));
    Ok(Exit::Ok)
}

/// `cmd_app_logs`' Python (one line): `print(json.load(sys.stdin)["framed"])`.
pub fn framed_line(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 1)?;
    o.line(&py::s(&py::idx(&v, "framed", 1)?));
    Ok(Exit::Ok)
}

/// `_app_client_logs`' Python.
pub fn client_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    if !py::truthy(&py::dget(&v, "count", 4)?) {
        let app = py::dget(&v, "app", 5)?;
        let keep = py::dget(&v, "keepDays", 5)?;
        o.line(&format!(
            "No client log bundles for {} (kept {} days).",
            py::s(&app),
            py::s(&keep)
        ));
        return Ok(Exit::Ok);
    }
    let count = py::idx(&v, "count", 7)?;
    let app = py::idx(&v, "app", 7)?;
    let keep = py::dget(&v, "keepDays", 7)?;
    o.line(&format!(
        "{} bundle(s) for {}, oldest first (kept {} days; --limit for more):",
        py::s(&count),
        py::s(&app),
        py::s(&keep)
    ));
    o.line(&py::s(&py::idx(&v, "framed", 8)?));
    Ok(Exit::Ok)
}
