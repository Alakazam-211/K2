//! `app grant hooks …` (and `skin grant hooks …`), as the shell CLI's
//! `_app_grant_hooks` answers it (`cli/src/talk-and-workspace.sh`): the
//! grant hooks' status, the outbox log, `retry|skip <id>`, `reconcile`
//! and `target chat --on|--off`. Each is one request (`cli_request` or
//! `cli_post_json`, [`super::send`]), `_skin_die_if_err`, `--json` as it
//! came where the verb takes it, then the shell's Python line for line.
//!
//! `-h` or `--help` anywhere is the shell's (it prints the usage and
//! sends nothing).

use super::{raw_line, send, Req, Stop};
use crate::exit::Exit;
use crate::families::app::py::{self, Out};
use crate::families::skin_token::{self, py::dumps, py::obj};
use crate::families::{py_iter, py_len, Raise};
use crate::out::json::{Text, Value};
use crate::out::pylib::ops::py_eq;
use crate::prelude::Closing;
use crate::transport::{encode, Client};
use k2_client::{cli_name, product_name};

/// `_app_grant_hooks`' usage line for an unknown action (exit 1).
pub const USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " app grant hooks [status|log|retry <id>|skip <id>|reconcile|target chat --on|--off]"
);
const LOG_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " app grant hooks log [--subject <ext id>] [--limit N] [--json]"
);
const TARGET_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " app grant hooks target chat --on|--off [--socket <path>] [--token-file <path>]"
);
const ROW_USAGE: &str = concat!("Usage: ", cli_name!(), " app grant hooks ");

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Status {
        json: bool,
    },
    Log {
        pairs: Vec<String>,
        json: bool,
    },
    /// `retry` or `skip`, and the row's body.
    Row {
        action: &'static str,
        body: String,
    },
    Reconcile,
    Target {
        body: String,
    },
}

fn stop(line: &str) -> Stop {
    Stop::line(line.to_string(), Exit::Error)
}

/// The words after `hooks`. `None`: a help form (the shell's).
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let (action, rest) = match words.first() {
        // `case "${1:-}" in ""|--json) set -- status "$@"`.
        None | Some(&"--json" | &"") => ("status", words),
        Some(a) => (*a, &words[1..]),
    };
    Some(match action {
        "status" => Ok(Ask::Status {
            json: rest.first() == Some(&"--json"),
        }),
        "log" => log(rest),
        "retry" | "skip" => {
            let action = if action == "retry" { "retry" } else { "skip" };
            match rest.first() {
                Some(id) if !id.is_empty() => Ok(Ask::Row {
                    action,
                    body: dumps(&obj(vec![("id", Value::Str(Text::from(*id)))])),
                }),
                _ => Err(stop(&format!("{ROW_USAGE}{action} <outbox-id>"))),
            }
        }
        "reconcile" => Ok(Ask::Reconcile),
        "target" => target(rest),
        _ => Err(stop(USAGE)),
    })
}

/// `x="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi`.
fn soft<'a>(rest: &mut &'a [&'a str]) -> String {
    let v = rest.get(1).copied().unwrap_or("").to_string();
    *rest = rest.get(2..).unwrap_or_default();
    v
}

fn log(words: &[&str]) -> Result<Ask, Stop> {
    let (mut subject, mut limit, mut json) = (String::new(), "50".to_string(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--subject" => {
                subject = soft(&mut rest);
                continue;
            }
            "--limit" => {
                limit = soft(&mut rest);
                continue;
            }
            "--json" => json = true,
            _ => {
                if let Some(v) = w.strip_prefix("--subject=") {
                    subject = v.to_string();
                } else if let Some(v) = w.strip_prefix("--limit=") {
                    limit = v.to_string();
                } else {
                    return Err(stop(LOG_USAGE));
                }
            }
        }
        rest = &rest[1..];
    }
    let mut pairs = vec![format!("limit={}", encode::quote(&limit))];
    if !subject.is_empty() {
        pairs.push(format!("subject={}", encode::quote(&subject)));
    }
    Ok(Ask::Log { pairs, json })
}

fn target(words: &[&str]) -> Result<Ask, Stop> {
    let kind = words.first().copied().unwrap_or("");
    let mut rest = words.get(1..).unwrap_or_default();
    let (mut enabled, mut socket, mut token_file) = (None, String::new(), String::new());
    while let Some(w) = rest.first().copied() {
        match w {
            "--on" => enabled = Some(true),
            "--off" => enabled = Some(false),
            "--socket" => {
                socket = soft(&mut rest);
                continue;
            }
            "--token-file" => {
                token_file = soft(&mut rest);
                continue;
            }
            _ => {
                if let Some(v) = w.strip_prefix("--socket=") {
                    socket = v.to_string();
                } else if let Some(v) = w.strip_prefix("--token-file=") {
                    token_file = v.to_string();
                } else {
                    return Err(stop(TARGET_USAGE));
                }
            }
        }
        rest = &rest[1..];
    }
    let Some(on) = enabled.filter(|_| !kind.is_empty()) else {
        return Err(stop(TARGET_USAGE));
    };
    let mut b = vec![
        ("kind", Value::Str(Text::from(kind))),
        ("enabled", Value::Bool(on)),
    ];
    if !socket.is_empty() {
        b.push(("socket", Value::Str(Text::from(socket.as_str()))));
    }
    if !token_file.is_empty() {
        b.push(("tokenFile", Value::Str(Text::from(token_file.as_str()))));
    }
    Ok(Ask::Target {
        body: dumps(&obj(b)),
    })
}

/// Runs it: the exit, its lines printed.
pub fn run(client: &mut Client, closing: &mut Closing, ask: &Ask) -> Exit {
    let (req, json, lines): (Req, bool, Option<Printer>) = match ask {
        Ask::Status { json } => (
            Req::Get("/cli/skin/grant-hooks", vec![]),
            *json,
            Some(status_lines),
        ),
        Ask::Log { pairs, json } => (
            Req::Get("/cli/skin/grant-hooks/log", pairs.clone()),
            *json,
            Some(log_lines),
        ),
        Ask::Row { action, body } => (
            Req::Post(
                if *action == "retry" {
                    "/cli/skin/grant-hooks/retry"
                } else {
                    "/cli/skin/grant-hooks/skip"
                },
                body.clone(),
            ),
            false,
            Some(row_lines),
        ),
        Ask::Reconcile => (
            Req::Post("/cli/skin/grant-hooks/reconcile", "{}".into()),
            false,
            None,
        ),
        Ask::Target { body } => (
            Req::Post("/cli/skin/grant-hooks/target", body.clone()),
            false,
            Some(target_lines),
        ),
    };
    let raw = match send(client, closing, &req) {
        Ok(raw) => raw,
        Err(exit) => return exit,
    };
    let exit = skin_token::die_if_err(&raw).print();
    if exit != Exit::Ok {
        return exit;
    }
    if json {
        return match raw_line(&raw) {
            Ok(()) => Exit::Ok,
            Err(exit) => exit,
        };
    }
    let Some(lines) = lines else {
        return crate::families::Shown::out("Reconcile started.\n").print();
    };
    let mut o = Out::default();
    let r = lines(&raw, &mut o);
    o.done(r).print()
}

type Printer = fn(&[u8], &mut Out) -> Result<Exit, Raise>;

/// `datetime.datetime.fromtimestamp(ts, datetime.timezone.utc)
/// .strftime("%Y-%m-%d %H:%M:%SZ") if ts else "?"`.
fn when(ts: &Value, line: u32) -> Result<String, Raise> {
    if !py::truthy(ts) {
        return Ok("?".into());
    }
    let secs: i64 = match ts {
        Value::Bool(b) => i64::from(*b),
        Value::Int(d) => d.parse().map_err(|_| {
            Raise::at(
                line,
                "OverflowError: timestamp out of range for platform time_t",
            )
        })?,
        Value::Float(f) if f.is_nan() => {
            return Err(Raise::at(
                line,
                "ValueError: Invalid value NaN (not a number)",
            ))
        }
        Value::Float(f) if f.is_infinite() => {
            return Err(Raise::at(
                line,
                "OverflowError: cannot convert float infinity to integer",
            ))
        }
        Value::Float(f) => {
            // Rounded half to even to the microsecond, then the second.
            let whole = f.floor();
            let us = ((f - whole) * 1e6).round_ties_even();
            if !(-1e17..1e17).contains(&whole) {
                return Err(Raise::at(
                    line,
                    "OverflowError: timestamp out of range for platform time_t",
                ));
            }
            whole as i64 + i64::from(us >= 1e6)
        }
        other => {
            return Err(Raise::at(
                line,
                format!(
                    "TypeError: '{}' object cannot be interpreted as an integer",
                    crate::out::py::type_name(other)
                ),
            ))
        }
    };
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    if !(1..=9999).contains(&y) {
        return Err(Raise::at(
            line,
            format!("ValueError: year {y} is out of range"),
        ));
    }
    Ok(format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    ))
}

/// The proleptic Gregorian date `days` after 1970-01-01.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `_pass(s)`.
fn pass(s: &Value, line: u32) -> Result<String, Raise> {
    const KEYS: [(&str, &str); 8] = [
        ("provisioned", "provisioned"),
        ("updated", "updated"),
        ("deprovisioned", "deprovisioned"),
        ("orphansRemoved", "orphans removed"),
        ("skippedBusy", "skipped busy"),
        ("skippedHeld", "held"),
        ("ignoredLocal", "ignored local"),
        ("ignoredOther", "ignored other"),
    ];
    let mut parts = Vec::new();
    for (k, label) in KEYS {
        let v = py::dget(s, k, line)?;
        if py::truthy(&v) {
            parts.push(format!("{} {label}", py::s(&v)));
        }
    }
    Ok(if parts.is_empty() {
        "no changes".into()
    } else {
        parts.join(", ")
    })
}

fn get_or(v: &Value, key: &str, default: Value, line: u32) -> Result<Value, Raise> {
    py::dget_or(v, key, default, line)
}

fn zero() -> Value {
    Value::Int("0".into())
}

/// The status' Python.
fn status_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let d = py::load(body, 3)?;
    let ts = py::or(py::dget(&d, "targets", 4)?, py::empty_list());
    if !py::truthy(&ts) {
        o.line("No grant-hook targets (chat is not enabled on this box).");
    }
    for t in py_iter(&ts, 7)? {
        let kind = get_or(&t, "kind", py::text(""), 8)?;
        let on = if py::truthy(&py::dget(&t, "enabled", 8)?) {
            "on"
        } else {
            "off"
        };
        let health = if py::truthy(&py::dget(&t, "healthy", 9)?) {
            "healthy".to_string()
        } else {
            let e = py::or(py::dget(&t, "lastError", 9)?, py::text("?"));
            py::concat("unhealthy: ", &e, 9)?
        };
        let socket = get_or(&t, "socket", py::text(""), 9)?;
        o.line(&format!(
            "{}  {on}  {health}  socket {}",
            py::s(&kind),
            py::s(&socket)
        ));
        let last = py::dget(&t, "lastReconcile", 19)?;
        if last != Value::Null {
            let at = when(&py::dget(&t, "lastReconcileAt", 20)?, 12)?;
            let p = pass(&py::idx(&t, "lastReconcile", 20)?, 17)?;
            o.line(&format!("  last reconcile: {at}  {p}"));
        }
        if py::truthy(&py::dget(&t, "lastChange", 21)?) {
            let change_at = py::dget(&t, "lastChangeAt", 22)?;
            if !py_eq(&change_at, &py::dget(&t, "lastReconcileAt", 22)?) {
                let at = when(&change_at, 12)?;
                let p = pass(&py::idx(&t, "lastChange", 23)?, 17)?;
                o.line(&format!("  last change   : {at}  {p}"));
            }
        } else if py::dget(&t, "lastReconcile", 24)? != Value::Null {
            o.line("  last change   : none yet (every pass so far found nothing to fix)");
        }
        let hist = py::or(py::dget(&t, "reconcileHistory", 26)?, py::empty_list());
        if py_len(&hist, 27)? > 1 {
            o.line("  recent passes (newest first):");
            let mut items = py_iter(&hist, 29)?;
            items.reverse();
            for h in items.iter().take(5) {
                let at = when(&py::dget(h, "at", 30)?, 12)?;
                o.line(&format!("    {at}  {}", pass(h, 17)?));
            }
        }
    }
    let ob = py::or(py::dget(&d, "outbox", 31)?, Value::Object(Vec::new()));
    let age = py::dget(&ob, "oldestPendingAgeSecs", 32)?;
    let pending = get_or(&ob, "pending", zero(), 33)?;
    let blocked = get_or(&ob, "blocked", zero(), 33)?;
    let held = get_or(&ob, "held", zero(), 33)?;
    let age = if age == Value::Null {
        String::new()
    } else {
        format!("  (oldest pending {}s)", py::s(&age))
    };
    o.line(&format!(
        "outbox: {} pending, {} blocked, {} held{age}",
        py::s(&pending),
        py::s(&blocked),
        py::s(&held)
    ));
    for (key, label, line) in [("blocked", "BLOCKED", 35), ("held", "HELD   ", 37)] {
        let rows = py::or(py::dget(&d, key, line)?, py::empty_list());
        for r in py_iter(&rows, line)? {
            let id = get_or(&r, "id", py::text(""), line + 1)?;
            let op = get_or(&r, "op", py::text(""), line + 1)?;
            let subject = get_or(&r, "subjectKey", py::text(""), line + 1)?;
            let err = py::or(py::dget(&r, "lastError", line + 1)?, py::text(""));
            o.line(&format!(
                "  {label} {}  {} {}  {}",
                py::s(&id),
                py::s(&op),
                py::s(&subject),
                py::s(&err)
            ));
        }
    }
    o.line(concat!(
        "Actors are reported by ",
        product_name!(),
        "; not proof a person acted."
    ));
    Ok(Exit::Ok)
}

/// The log's Python.
fn log_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let d = py::load(body, 3)?;
    let rows = py::or(py::dget(&d, "rows", 4)?, py::empty_list());
    if !py::truthy(&rows) {
        o.line("No outbox rows.");
    }
    for r in py_iter(&rows, 6)? {
        let f = |k: &str, default: Value| get_or(&r, k, default, 7);
        let id = f("id", py::text(""))?;
        let state = f("state", py::text(""))?;
        let op = f("op", py::text(""))?;
        let subject = f("subjectKey", py::text(""))?;
        let actor = f("actor", py::text(""))?;
        let cause = f("cause", py::text(""))?;
        let attempts = f("attempts", zero())?;
        let err = if py::truthy(&py::dget(&r, "lastError", 9)?) {
            py::concat("  ", &py::idx(&r, "lastError", 9)?, 9)?
        } else {
            String::new()
        };
        o.line(&format!(
            "{}  {} {} {}  by {}  ({}, {} attempt(s)){err}",
            py::s(&id),
            py::left(&state, 9),
            py::left(&op, 11),
            py::s(&subject),
            py::s(&actor),
            py::s(&cause),
            py::s(&attempts)
        ));
    }
    Ok(Exit::Ok)
}

/// `retry|skip`'s Python.
fn row_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let d = py::load(body, 3)?;
    let id = get_or(&d, "id", py::text(""), 4)?;
    let state = get_or(&d, "state", py::text(""), 4)?;
    o.line(&format!(
        "Outbox row {} is now {}.",
        py::s(&id),
        py::s(&state)
    ));
    Ok(Exit::Ok)
}

/// `target`'s Python.
fn target_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let d = py::load(body, 3)?;
    let kind = get_or(&d, "kind", py::text(""), 4)?;
    let on = if py::truthy(&py::dget(&d, "enabled", 4)?) {
        "on"
    } else {
        "off"
    };
    let socket = get_or(&d, "socket", py::text(""), 4)?;
    o.line(&format!(
        "Target {} {on} (socket {})",
        py::s(&kind),
        py::s(&socket)
    ));
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_times_as_python_writes_them() {
        assert_eq!(when(&Value::Int("0".into()), 1).unwrap(), "?");
        assert_eq!(
            when(&Value::Int("1700000000".into()), 1).unwrap(),
            "2023-11-14 22:13:20Z"
        );
        assert_eq!(
            when(&Value::Float(1700000000.9999996), 1).unwrap(),
            "2023-11-14 22:13:21Z"
        );
        assert_eq!(
            when(&Value::Int("-86401".into()), 1).unwrap(),
            "1969-12-30 23:59:59Z"
        );
        assert!(when(&py::text("x"), 1).is_err());
    }

    #[test]
    fn the_words_as_the_shell_reads_them() {
        assert_eq!(parse(&[]), Some(Ok(Ask::Status { json: false })));
        assert_eq!(parse(&["--json"]), Some(Ok(Ask::Status { json: true })));
        assert_eq!(
            parse(&["status", "x", "--json"]),
            Some(Ok(Ask::Status { json: false }))
        );
        assert_eq!(parse(&["retry", "x", "-h"]), None);
        assert_eq!(
            parse(&["log", "--limit"]),
            Some(Ok(Ask::Log {
                pairs: vec!["limit=".into()],
                json: false
            }))
        );
        assert_eq!(parse(&["target", "--on"]), Some(Err(stop(TARGET_USAGE))));
        assert_eq!(
            parse(&["target", "chat", "--on", "--socket=/s"]),
            Some(Ok(Ask::Target {
                body: r#"{"kind": "chat", "enabled": true, "socket": "/s"}"#.into()
            }))
        );
        assert_eq!(parse(&["nope"]), Some(Err(stop(USAGE))));
    }

    #[test]
    fn usage_lines_are_the_shell_s() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/src/talk-and-workspace.sh");
        let src = std::fs::read_to_string(&path).expect("talk-and-workspace.sh");
        for line in [USAGE, LOG_USAGE, TARGET_USAGE, ROW_USAGE] {
            let shell = line.replace(k2_client::brand::CLI_NAME, "k2");
            assert!(src.contains(&shell), "no longer the shell's: {shell}");
        }
    }
}
