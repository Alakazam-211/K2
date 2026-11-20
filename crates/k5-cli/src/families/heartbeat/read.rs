//! The heartbeat readers: each Python block of `cli/src/remind.sh` and
//! `cli/src/heartbeat.sh` on what it reads, as [`Shown`].

use super::py::{
    dget_or, dget_v, dumps_indent, has, hashable, index, int, is, iter, join, left, len, loads,
    loads_value, or, plus, repr_head, stamp, str_plus, stripped, Py, R,
};
use super::when;
use crate::exit::Exit;
use crate::families::{ljust, s, Shown};
use crate::out::json::Value;
use crate::out::py;
use k2_client::{cli_name, product_name};

/// The codes `cmd_heartbeat_list` refuses with.
fn gated(code: &Value) -> bool {
    ["owner_only", "forbidden", "gated"]
        .iter()
        .any(|w| is(code, w))
}

/// `hb_zone(r)`.
fn zone(r: &Value) -> R<Value> {
    Ok(or(
        or(dget_v(r, "zoneLabel")?, dget_v(r, "tz")?),
        s("server clock"),
    ))
}

/// `hb_next(r)`.
fn next(r: &Value) -> R<String> {
    let v = dget_v(r, "nextFireAt")?;
    if !py::truthy(&v) {
        return Ok("-".into());
    }
    when::local_minute(&v)
}

/// `hb_waiting(r)`.
fn waiting(r: &Value) -> R<String> {
    let reason = dget_v(r, "waitReason")?;
    if !py::truthy(&reason) {
        return Ok("-".into());
    }
    let detail = dget_v(r, "waitDetail")?;
    let own = ["wakeup_empty", "schedule_error", "wakeup_waiting"]
        .iter()
        .any(|w| is(&reason, w));
    if py::truthy(&detail) && !own {
        return Ok(format!("{}: {}", py::str_of(&reason), py::str_of(&detail)));
    }
    Ok(py::str_of(&reason))
}

/// One row's Schedule column.
fn schedule(spec: &Value) -> R<String> {
    let mut parts = Vec::new();
    if has(spec, "time")? {
        parts.push(format!("at {}", py::str_of(&index(spec, "time")?)));
    }
    if has(spec, "days")? {
        parts.push(format!("on {}", join(&index(spec, "days")?, ", ")?));
    }
    if has(spec, "days_of_month")? {
        let items = iter(&index(spec, "days_of_month")?)?;
        let words: Vec<String> = items.iter().map(py::str_of).collect();
        parts.push(format!("day(s) {}", words.join(",")));
    }
    if has(spec, "months")? {
        parts.push(format!("in {}", join(&index(spec, "months")?, ",")?));
    }
    let window = |spec: &Value| -> R<String> {
        Ok(format!(
            "{}-{}",
            py::str_of(&dget_or(spec, "start", s("00:00"))?),
            py::str_of(&dget_or(spec, "end", s("23:59"))?)
        ))
    };
    let start_or_end = |spec: &Value| -> R<bool> {
        Ok(py::truthy(&dget_v(spec, "start")?) || py::truthy(&dget_v(spec, "end")?))
    };
    if has(spec, "every_seconds")? {
        let es = int(&index(spec, "every_seconds")?)?;
        let every = if es != 0 && es.rem_euclid(3600) == 0 {
            format!("{}h", es.div_euclid(3600))
        } else if es != 0 && es.rem_euclid(60) == 0 {
            format!("{}m", es.div_euclid(60))
        } else {
            format!("{es}s")
        };
        let mut hourly = format!("every {every}");
        if start_or_end(spec)? {
            hourly.push_str(&format!(", {}", window(spec)?));
        }
        parts.push(hourly);
    } else if start_or_end(spec)? {
        parts.push(window(spec)?);
    }
    let sched = parts.join(" ");
    Ok(if sched.is_empty() { "-".into() } else { sched })
}

/// `cmd_heartbeat_list`'s table, on what the reader read.
pub fn list(raw: &str) -> Shown {
    let mut p = Py::default();
    let r = list_rows(&mut p, raw);
    p.caught(r)
}

fn list_rows(p: &mut Py, raw: &str) -> R<Exit> {
    let rows = loads(raw)?;
    if let Value::Object(_) = rows {
        let err = dget_or(&rows, "error", rows.clone())?;
        if let Value::Object(_) = err {
            let code = or(dget_v(&err, "code")?, s("error"));
            let mut hint = or(
                or(dget_v(&err, "hint")?, dget_v(&err, "message")?),
                code.clone(),
            );
            if gated(&code) && !has(&hint, "ask your human")? {
                let head = if py::truthy(&hint) {
                    plus(&hint, " \u{2014} ")?
                } else {
                    String::new()
                };
                hint = s(&format!(
                    "{head}requires owner/admin \u{2014} ask your human"
                ));
            }
            p.err(format!("Error: {}", py::str_of(&hint)));
            return Ok(if gated(&code) || is(&code, "other_workspace") {
                Exit::Refused
            } else {
                Exit::Error
            });
        }
        p.err(format!("Error: {}", py::str_of(&err)));
        return Ok(if py::str_of(&err).contains("workspace not found") {
            Exit::NotFound
        } else {
            Exit::Error
        });
    }
    if !py::truthy(&rows) {
        p.out("No heartbeats configured for this workspace.");
        return Ok(Exit::Ok);
    }
    let rows = iter(&rows)?;
    let mut zw = 4;
    for r in &rows {
        zw = zw.max(len(&zone(r)?)?);
    }
    p.out(format!(
        "{} {} {} {} {} {} {} Waiting",
        ljust("Name", 20),
        ljust("Frequency", 10),
        ljust("Schedule", 30),
        ljust("Zone", zw),
        ljust("Last Fired", 22),
        ljust("Enabled", 8),
        ljust("Next", 17)
    ));
    p.out("-".repeat(141 + zw));
    for r in &rows {
        let spec = loads_value(&dget_or(r, "specJson", s("{}"))?)?;
        let sched = schedule(&spec)?;
        let last = stamp(&or(dget_v(r, "lastFired")?, s("never")))?;
        let en = if py::truthy(&dget_v(r, "enabled")?) {
            if py::truthy(&dget_v(r, "pausedUntil")?) {
                "paused"
            } else {
                "yes"
            }
        } else if py::truthy(&dget_v(r, "disabledReason")?) {
            "auto-off"
        } else {
            "no"
        };
        let name = left(&dget_or(r, "name", s("?"))?, 20)?;
        let freq = left(&dget_or(r, "frequency", s("?"))?, 10)?;
        let z = left(&zone(r)?, zw)?;
        let nx = next(r)?;
        let w = waiting(r)?;
        p.out(format!(
            "{name} {freq} {} {z} {} {} {} {w}",
            ljust(&sched, 30),
            ljust(&last, 22),
            ljust(en, 8),
            ljust(&nx, 17)
        ));
        let tz = dget_v(r, "tz")?;
        if py::truthy(&tz) {
            let tzv = index(r, "tz")?;
            let at = when::hb_when(Some(&dget_v(r, "nextFireAt")?), Some(&tzv))?;
            p.out(format!("    time zone {}: next {at}", py::str_of(&tzv)));
        }
        let wr = dget_v(r, "waitReason")?;
        if is(&wr, "wakeup_empty") {
            p.out("    waiting: WAKEUP.md is empty (needs instructions)");
        } else if is(&wr, "schedule_error") {
            let why = or(
                or(dget_v(r, "waitDetail")?, dget_v(r, "scheduleError")?),
                s("?"),
            );
            p.out(format!("    invalid schedule: {}", py::str_of(&why)));
        } else if is(&wr, "wakeup_waiting") {
            let why = or(dget_v(r, "waitDetail")?, s("waiting for WAKEUP.md"));
            p.out(format!("    {}", py::str_of(&why)));
        }
        let pu = dget_v(r, "pausedUntil")?;
        if py::truthy(&dget_v(r, "enabled")?) && py::truthy(&pu) {
            let shown = when::local_minute(&pu)?;
            p.out(format!(
                "    paused until {shown} (LLM usage limit): it resumes by itself"
            ));
        }
        let dr = dget_v(r, "disabledReason")?;
        if !py::truthy(&dget_v(r, "enabled")?) && py::truthy(&dr) {
            hashable(&dr)?;
            let why = match &dr {
                d if is(d, "failures") => "its fires failed too many times in a row".to_string(),
                d if is(d, "wakeup_missing") => format!(
                    "WAKEUP.md could not be read (an older {} did this; {} now waits and retries instead)",
                    product_name!(),
                    product_name!()
                ),
                d if is(d, "wakeup_empty") => "WAKEUP.md was empty (add instructions first)".into(),
                d => py::str_of(d),
            };
            let since = dget_v(r, "waitSince")?;
            let mut at = String::new();
            if py::truthy(&since) {
                at = match when::parse(&since)? {
                    Some(t) => format!(" (off since {})", when::here(&t, "%Y-%m-%d %H:%M")?.0),
                    None => format!(" (off since {})", py::str_of(&since)),
                };
            }
            p.out(format!(
                "    auto-disabled by {}{at}: {why}",
                product_name!()
            ));
            p.out(format!(
                "    it stays off until it is turned on: {} heartbeat enable {}",
                cli_name!(),
                py::str_of(&dget_or(r, "name", s("?"))?)
            ));
        }
    }
    Ok(Exit::Ok)
}

/// `cmd_heartbeat_list_archived`'s table.
pub fn archived(raw: &str) -> Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let rows = loads(raw)?;
        if let Value::Object(_) = rows {
            let err = dget_or(&rows, "error", rows.clone())?;
            p.err(format!("Error: {}", py::str_of(&err)));
            return Ok(Exit::Error);
        }
        if !py::truthy(&rows) {
            p.out("No archived heartbeats in this workspace.");
            return Ok(Exit::Ok);
        }
        p.out(format!(
            "{} {} {} {}",
            ljust("NAME", 20),
            ljust("FREQ", 10),
            ljust("ARCHIVED", 22),
            ljust("LAST FIRED", 22)
        ));
        for r in iter(&rows)? {
            let cell = |key: &str| -> R<String> {
                let v = stamp(&or(dget_v(&r, key)?, s("")))?;
                Ok(if v.is_empty() { "\u{2014}".into() } else { v })
            };
            let a = cell("archivedAt")?;
            let l = cell("lastFired")?;
            p.out(format!(
                "{} {} {} {}",
                left(&dget_or(&r, "name", s("?"))?, 20)?,
                left(&dget_or(&r, "frequency", s("?"))?, 10)?,
                ljust(&a, 22),
                ljust(&l, 22)
            ));
        }
        Ok(Exit::Ok)
    })();
    p.caught(r)
}

/// The readers of `remove`, `unarchive`, `rename` and
/// `use-pinned-session`: `success` prints `line`, else the error.
pub fn success(raw: &str, line: &str) -> Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let d = loads(raw)?;
        if py::truthy(&dget_v(&d, "success")?) {
            p.out(line);
            return Ok(Exit::Ok);
        }
        let e = dget_or(&d, "error", s("unknown"))?;
        p.err(format!("Error: {}", py::str_of(&e)));
        Ok(Exit::Error)
    })();
    p.caught(r)
}

/// `cmd_heartbeat_fire`'s reader.
pub fn fire(raw: &str) -> Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let d = loads(raw)?;
        if has(&d, "error")? {
            let e = index(&d, "error")?;
            p.err(format!("Error: {}", py::str_of(&e)));
            return Ok(Exit::Error);
        }
        let decision = dget_or(&d, "decision", s("?"))?;
        let name = dget_or(&d, "name", s("?"))?;
        let agent = dget_or(&d, "agent", s("?"))?;
        if py::truthy(&dget_v(&d, "success")?) {
            let tid = dget_or(&d, "terminalId", s("?"))?;
            p.out(format!(
                "Fired heartbeat: {} (agent: {})",
                py::str_of(&name),
                py::str_of(&agent)
            ));
            p.out(format!("  terminal: {}", py::str_of(&tid)));
            p.out("  audit:    decision=fired (manual fire via CLI)");
            return Ok(Exit::Ok);
        }
        let reason = dget_or(&d, "reason", s("unknown"))?;
        p.err(format!(
            "Did not fire \"{}\": {}",
            py::str_of(&name),
            py::str_of(&decision)
        ));
        p.err(format!("  reason: {}", py::str_of(&reason)));
        Ok(Exit::Error)
    })();
    p.caught(r)
}

/// The start the restart and enable readers share: the answer as a dict,
/// or the shown end.
fn reply_dict(p: &mut Py, raw: &str) -> Result<Value, Exit> {
    let Ok(d) = loads(raw) else {
        p.err(format!(
            "Error: the daemon answered something that is not JSON: {}",
            repr_head(raw, 300)
        ));
        return Err(Exit::Error);
    };
    if !matches!(d, Value::Object(_)) {
        p.err(format!("Error: unexpected reply: {}", repr_head(raw, 300)));
        return Err(Exit::Error);
    }
    Ok(d)
}

const REFUSED: &[&str] = &[
    "other_workspace",
    "admin_only",
    "owner_only",
    "forbidden",
    "wrong_credential",
    "invalid_token",
];

/// `cmd_heartbeat_restart`'s reader.
pub fn restart(raw: &str, name: &str, json: bool) -> Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let d = match reply_dict(&mut p, raw) {
            Ok(d) => d,
            Err(exit) => return Ok(exit),
        };
        let err = dget_v(&d, "error")?;
        if is(&err, "no_such_heartbeat") {
            let m = or(
                dget_v(&d, "message")?,
                s(&format!("no heartbeat named {name}")),
            );
            p.err(format!(
                "Error (no_such_heartbeat): {}. List them with: {} heartbeat schedule list",
                py::str_of(&m),
                cli_name!()
            ));
            return Ok(Exit::Error);
        }
        if let Value::Object(_) = err {
            let code = or(dget_v(&err, "code")?, s("error"));
            let hint = or(
                or(dget_v(&err, "hint")?, dget_v(&err, "message")?),
                code.clone(),
            );
            p.err(format!(
                "Error ({}): {}",
                py::str_of(&code),
                py::str_of(&hint)
            ));
            let refused = REFUSED
                .iter()
                .chain(&["role_required"])
                .any(|w| is(&code, w));
            return Ok(if refused { Exit::Refused } else { Exit::Error });
        }
        if is(&err, "role_required") {
            let m = or(dget_v(&d, "message")?, err.clone());
            p.err(format!("Error (role_required): {}", py::str_of(&m)));
            return Ok(Exit::Refused);
        }
        if py::truthy(&err) {
            let e = py::str_of(&err);
            p.err(format!("Error: {e}"));
            if e.starts_with("not_a_heartbeat") {
                return Ok(Exit::Usage);
            }
            return Ok(if e.contains("workspace not found") {
                Exit::NotFound
            } else {
                Exit::Error
            });
        }
        if json {
            p.out(dumps_indent(&d)?);
            return Ok(Exit::Ok);
        }
        let hb = py::str_of(&or(dget_v(&d, "name")?, s(name)));
        p.out(format!("Restarted heartbeat session: {hb}"));
        if py::truthy(&dget_v(&d, "stopped")?) {
            p.out(format!(
                "  stopped session: {}",
                py::str_of(&dget_v(&d, "terminalId")?)
            ));
        } else {
            p.out("  no session was running");
        }
        if py::truthy(&dget_v(&d, "clearedSessionId")?) {
            p.out(format!(
                "  cleared saved conversation: {}",
                py::str_of(&index(&d, "clearedSessionId")?)
            ));
        }
        p.out(format!(
            "The next fire starts a new session. To fire it now: {} heartbeat signal fire {hb}",
            cli_name!()
        ));
        Ok(Exit::Ok)
    })();
    p.raised(r, 2)
}

/// `cmd_heartbeat_enable_disable`'s reader.
pub fn enable(raw: &str, name: &str, on: bool, json: bool) -> Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let verb = if on { "Enabled" } else { "Disabled" };
        let d = match reply_dict(&mut p, raw) {
            Ok(d) => d,
            Err(exit) => return Ok(exit),
        };
        let err = dget_v(&d, "error")?;
        if is(&err, "no_such_heartbeat") {
            let m = or(
                dget_v(&d, "message")?,
                s(&format!("no heartbeat named {name}")),
            );
            p.err(format!("Error (no_such_heartbeat): {}", py::str_of(&m)));
            return Ok(Exit::Error);
        }
        if let Value::Object(_) = err {
            let code = or(dget_v(&err, "code")?, s("error"));
            let hint = or(dget_v(&err, "hint")?, code.clone());
            p.err(format!(
                "Error ({}): {}",
                py::str_of(&code),
                py::str_of(&hint)
            ));
            let refused = REFUSED.iter().any(|w| is(&code, w));
            return Ok(if refused { Exit::Refused } else { Exit::Error });
        }
        if py::truthy(&err) {
            let e = py::str_of(&err);
            p.err(format!("Error: {e}"));
            return Ok(if e.contains("workspace not found") {
                Exit::NotFound
            } else {
                Exit::Error
            });
        }
        if json {
            p.out(dumps_indent(&d)?);
            return Ok(Exit::Ok);
        }
        let ws = dget_v(&d, "workspace")?;
        if py::truthy(&ws) {
            let hb = or(dget_v(&d, "heartbeat")?, s(name));
            p.out(format!(
                "{verb} heartbeat: {} (workspace {})",
                py::str_of(&hb),
                py::str_of(&index(&d, "workspace")?)
            ));
        } else {
            p.out(format!("{verb} heartbeat: {name}"));
        }
        Ok(Exit::Ok)
    })();
    p.raised(r, 2)
}

/// `cmd_heartbeat_session`'s read: where a fire's wake lands.
pub fn session_show(raw: &str, name: &str) -> Shown {
    let mut p = Py::default();
    let rows = match loads(raw) {
        Ok(rows) => rows,
        Err(e) => return p.caught(Err(e)),
    };
    let r = (|| -> R<Exit> {
        let mut row = None;
        for r in iter(&rows)? {
            if is(&dget_v(&r, "name")?, name) {
                row = Some(r);
                break;
            }
        }
        let Some(row) = row else {
            p.err(format!("No heartbeat named \"{name}\" in this workspace."));
            return Ok(Exit::Error);
        };
        if py::truthy(&dget_v(&row, "useWorkspaceSession")?) {
            p.out("pinned  (delivers into the workspace pinned chat)");
        } else if py::truthy(&dget_v(&row, "lastSessionId")?) {
            let provider = or(dget_v(&row, "sessionProvider")?, s("default-agent"));
            p.out(format!(
                "{}:{}",
                py::str_of(&provider),
                py::str_of(&index(&row, "lastSessionId")?)
            ));
        } else {
            p.out("auto  (own session - new on next fire)");
        }
        Ok(Exit::Ok)
    })();
    p.raised(r, 8)
}

/// `cmd_heartbeat_session`'s write.
pub fn session_set(raw: &str, name: &str) -> Shown {
    let mut p = Py::default();
    let d = match loads(raw) {
        Ok(d) => d,
        Err(e) => return p.caught(Err(e)),
    };
    let r = (|| -> R<Exit> {
        if !py::truthy(&dget_v(&d, "success")?) {
            let e = dget_or(&d, "error", s("unknown"))?;
            p.err(format!("Error: {}", py::str_of(&e)));
            return Ok(Exit::Error);
        }
        let mode = dget_v(&d, "mode")?;
        if is(&mode, "pinned") {
            p.out(format!(
                "Heartbeat \"{name}\" now delivers into the workspace pinned chat."
            ));
        } else if is(&mode, "auto") {
            p.out(format!(
                "Heartbeat \"{name}\" now uses its own session (new on next fire)."
            ));
        } else {
            p.out(format!(
                "Heartbeat \"{name}\" now delivers into {}:{}.",
                py::str_of(&dget_v(&d, "provider")?),
                py::str_of(&dget_v(&d, "sessionId")?)
            ));
        }
        Ok(Exit::Ok)
    })();
    p.raised(r, 12)
}

/// What `cmd_heartbeat_edit` changed.
pub struct Edited<'a> {
    pub name: &'a str,
    pub freq: &'a str,
    pub instructions: bool,
    pub tz: bool,
}

/// `cmd_heartbeat_edit`'s reader.
pub fn edit(raw: &str, e: &Edited) -> Shown {
    let mut p = Py::default();
    let d = match loads(raw) {
        Ok(d) => d,
        Err(x) => return p.caught(Err(x)),
    };
    let r = (|| -> R<Exit> {
        if !py::truthy(&dget_v(&d, "success")?) {
            let err = dget_or(&d, "error", s("unknown"))?;
            let text = py::str_of(&err);
            if text.contains("frequency") && e.freq.is_empty() && !e.instructions {
                p.err("Error: this daemon is older than heartbeat edit --tz. Update the host.");
            } else if text.contains("frequency") && e.freq.is_empty() {
                p.err("Error: this daemon is older than heartbeat edit --instructions without a schedule.");
                p.err("       Update the host, or pass the schedule flags too.");
            } else {
                p.err(format!("Error: {text}"));
            }
            return Ok(Exit::Error);
        }
        if !e.freq.is_empty() {
            p.out(format!("Updated heartbeat: {} ({})", e.name, e.freq));
        } else if e.tz {
            p.out(format!("Updated heartbeat: {}", e.name));
        }
        if e.instructions {
            p.out(format!(
                "Wake instructions written by the daemon: {}",
                e.name
            ));
        }
        Ok(Exit::Ok)
    })();
    p.raised(r, 6)
}

/// `_hb_tz_report`: `strict` when `--tz` was given.
pub fn tz_report(raw: &str, strict: bool) -> Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let d = loads(raw)?;
        if !matches!(d, Value::Object(_)) || !has(&d, "tz")? {
            if !strict {
                return Ok(Exit::Ok);
            }
            p.err("Error: this daemon is older than --tz: the schedule still follows the server clock. Update the host.");
            return Ok(Exit::Error);
        }
        let tz = dget_v(&d, "tz")?;
        let label = or(or(dget_v(&d, "zoneLabel")?, tz.clone()), s("server clock"));
        let head = str_plus("Time zone: ", &label)?;
        if py::truthy(&tz) && is(&dget_v(&d, "tzFrom")?, "owner") {
            p.out(format!("{head} (the owner's timezone, the default for new schedules; --tz server for the server clock)"));
        } else if py::truthy(&tz) {
            p.out(format!(
                "{head} (its times are read there, wherever the server is)"
            ));
        } else {
            p.out(head);
        }
        let at = when::hb_when(Some(&dget_v(&d, "nextFireAt")?), Some(&tz))?;
        p.out(format!("Next fire: {at}"));
        Ok(Exit::Ok)
    })();
    p.raised(r, 23)
}

/// `waiting += tail`: a string grows, a list takes each character.
fn add(waiting: &mut Value, tail: &str) -> R<()> {
    match waiting {
        Value::Str(_) => *waiting = s(&(py::str_of(waiting) + tail)),
        Value::Array(items) => items.extend(tail.chars().map(|c| s(&c.to_string()))),
        other => {
            return Err(format!(
                "TypeError: unsupported operand type(s) for +=: '{}' and 'str'",
                py::type_name(other)
            ))
        }
    }
    Ok(())
}

/// What `cmd_heartbeat_show` shows.
pub struct Shows<'a> {
    pub name: &'a str,
    pub json: bool,
    pub ws: &'a str,
}

/// `cmd_heartbeat_show`'s reader.
pub fn show(raw: &str, w: &Shows) -> Shown {
    let mut p = Py::default();
    let row = match loads(raw) {
        Ok(row) => row,
        Err(e) => return p.caught(Err(e)),
    };
    let r = (|| -> R<Exit> {
        let where_ = if w.ws.is_empty() {
            "this workspace".to_string()
        } else {
            format!("workspace {}", w.ws)
        };
        if !matches!(row, Value::Object(_)) {
            p.err(format!("Error: unexpected reply: {}", py::str_of(&row)));
            return Ok(Exit::Error);
        }
        let err = dget_v(&row, "error")?;
        if is(&err, "no_such_heartbeat") {
            p.err(format!("No heartbeat named \"{}\" in {where_}.", w.name));
            return Ok(Exit::Error);
        }
        if let Value::Object(_) = err {
            let code = dget_v(&err, "code")?;
            let hint = or(dget_v(&err, "hint")?, code.clone());
            p.err(format!(
                "Error ({}): {}",
                py::str_of(&code),
                py::str_of(&hint)
            ));
            return Ok(if is(&code, "other_workspace") {
                Exit::Refused
            } else {
                Exit::Error
            });
        }
        if py::truthy(&err) && py::str_of(&err).contains("workspace not found") {
            p.err(format!("Error: {}", py::str_of(&err)));
            return Ok(Exit::NotFound);
        }
        if py::truthy(&err) {
            p.err(format!(
                "Error: {} (an older host has no heartbeat show; update it)",
                py::str_of(&err)
            ));
            return Ok(Exit::Error);
        }
        if w.json {
            p.out(dumps_indent(&row)?);
            return Ok(Exit::Ok);
        }
        let spec = loads_value(&or(dget_v(&row, "specJson")?, s("{}")))?;
        let yes = |v: Value| if py::truthy(&v) { "yes" } else { "no" };
        let enabled = yes(dget_v(&row, "enabled")?);
        let last = or(dget_v(&row, "lastFired")?, s("never"));
        let created = or(dget_v(&row, "createdAt")?, s("?"));
        let pinned = yes(dget_v(&row, "useWorkspaceSession")?);
        let f = |k: &str| -> R<String> { Ok(py::str_of(&dget_v(&row, k)?)) };
        p.out(format!("Name:           {}", f("name")?));
        if !w.ws.is_empty() {
            p.out(format!("Workspace:      {}", w.ws));
        }
        p.out(format!("Frequency:      {}", f("frequency")?));
        p.out(format!("Enabled:        {enabled}"));
        p.out(format!("Pinned chat:    {pinned}"));
        p.out(format!("Last fired:     {}", py::str_of(&last)));
        let nf = or(dget_v(&row, "nextFireAt")?, s("-"));
        p.out(format!("Next fire:      {}", py::str_of(&nf)));
        let tz = dget_v(&row, "tz")?;
        let label = or(
            or(dget_v(&row, "zoneLabel")?, tz.clone()),
            s("server clock"),
        );
        p.out(format!("Time zone:      {}", py::str_of(&label)));
        if py::truthy(&tz) {
            let at = when::hb_when(Some(&dget_v(&row, "nextFireAt")?), Some(&tz))?;
            p.out(format!("Next in zone:   {at}"));
        }
        let mut waiting = or(dget_v(&row, "waitReason")?, s("-"));
        let detail = dget_v(&row, "waitDetail")?;
        if py::truthy(&detail) {
            add(&mut waiting, &format!(" ({})", py::str_of(&detail)))?;
        }
        let since = dget_v(&row, "waitSince")?;
        if py::truthy(&since) {
            add(&mut waiting, &format!(" since {}", py::str_of(&since)))?;
        }
        let waiting = py::str_of(&waiting);
        p.out(format!("Waiting:        {waiting}"));
        p.out(format!("Wakeup file:    {}", f("wakeupPath")?));
        p.out(format!("Created:        {}", py::str_of(&created)));
        p.out("Spec:");
        let Value::Object(pairs) = &spec else {
            return Err(format!(
                "AttributeError: '{}' object has no attribute 'items'",
                py::type_name(&spec)
            ));
        };
        for (k, v) in pairs {
            p.out(format!(
                "  {}: {}",
                py::str_of(&Value::Str(k.clone())),
                py::str_of(v)
            ));
        }
        p.out("Instructions:");
        let body = or(dget_v(&row, "instructions")?, s(""));
        if !stripped(&body)?.is_empty() {
            for line in crate::out::pylib::ops::splitlines(&py::str_of(&body)) {
                p.out(format!("  {line}"));
            }
        } else {
            p.out("  (empty \u{2014} the heartbeat waits until instructions are written)");
        }
        Ok(Exit::Ok)
    })();
    p.raised(r, 30)
}

/// The wakeup path `cmd_heartbeat_wakeup` reads from the list: `Ok`
/// with what Python printed, or the reader's end.
pub fn wakeup_path(raw: &str, name: &str) -> Result<String, Shown> {
    let mut p = Py::default();
    let mut path = String::new();
    let r = (|| -> R<Exit> {
        let rows = loads(raw)?;
        let mut row = None;
        for r in iter(&rows)? {
            if is(&dget_v(&r, "name")?, name) {
                row = Some(r);
                break;
            }
        }
        let Some(row) = row else {
            p.err(format!("No heartbeat named \"{name}\" in this workspace."));
            return Ok(Exit::Error);
        };
        path = py::str_of(&or(dget_v(&row, "wakeupPath")?, s("")));
        Ok(Exit::Ok)
    })();
    let shown = p.caught(r);
    if shown.exit == Exit::Ok {
        Ok(path)
    } else {
        Err(shown)
    }
}

/// `cmd_heartbeat_status`'s first reader: the row's wait, before the
/// history.
pub fn status_row(raw: &str, name: &str) -> Shown {
    let mut p = Py::default();
    let rows = match loads(raw) {
        Ok(rows) => rows,
        Err(e) => return p.caught(Err(e)),
    };
    let r = (|| -> R<Exit> {
        if let Value::Object(_) = rows {
            let e = dget_or(&rows, "error", rows.clone())?;
            p.err(format!("Error: {}", py::str_of(&e)));
            return Ok(Exit::Error);
        }
        let mut row = None;
        for r in iter(&rows)? {
            if is(&dget_v(&r, "name")?, name) {
                row = Some(r);
                break;
            }
        }
        let Some(row) = row else {
            p.err(format!("No heartbeat named \"{name}\" in this workspace."));
            return Ok(Exit::Error);
        };
        if py::truthy(&dget_v(&row, "waitReason")?) {
            let detail = if py::truthy(&dget_v(&row, "waitDetail")?) {
                format!(" ({})", py::str_of(&index(&row, "waitDetail")?))
            } else {
                String::new()
            };
            let nf = or(dget_v(&row, "nextFireAt")?, s("-"));
            p.out(format!(
                "Next fire: {}   Waiting: {}{detail}",
                py::str_of(&nf),
                py::str_of(&index(&row, "waitReason")?)
            ));
            p.out("");
        }
        Ok(Exit::Ok)
    })();
    p.raised(r, 9)
}

/// `who(a)` in `cmd_heartbeat_status`.
fn who(a: &Value) -> R<String> {
    if !py::truthy(a) {
        return Ok("scheduler".into());
    }
    if is(a, "owner-token") {
        return Ok("owner".into());
    }
    let Value::Str(_) = a else {
        return Err(format!(
            "AttributeError: '{}' object has no attribute 'startswith'",
            py::type_name(a)
        ));
    };
    let text = py::str_of(a);
    for (pre, words) in [
        ("user:", "user"),
        ("agent:", "agent"),
        ("app:", "app user"),
        ("app-token:", "app token"),
    ] {
        if let Some(rest) = text.strip_prefix(pre) {
            return Ok(format!("{words} {rest}"));
        }
    }
    Ok(text)
}

/// `cmd_heartbeat_status`'s history.
pub fn status_history(raw: &str, name: &str) -> Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let rows = loads(raw)?;
        if !py::truthy(&rows) {
            p.out(format!("No fire history for heartbeat \"{name}\" yet."));
            return Ok(Exit::Ok);
        }
        p.out(format!(
            "Last {} fire(s) for heartbeat \"{name}\":",
            len(&rows)?
        ));
        p.out(format!(
            "{} {} {} Reason",
            ljust("Time", 22),
            ljust("Decision", 22),
            ljust("By", 22)
        ));
        p.out("-".repeat(110));
        for r in iter(&rows)? {
            let t = stamp(&or(dget_v(&r, "firedAt")?, s("")))?;
            let d = left(&dget_or(&r, "decision", s("?"))?, 22)?;
            let by = who(&dget_v(&r, "actor")?)?;
            let reason = or(dget_or(&r, "reason", s(""))?, s(""));
            p.out(format!(
                "{} {d} {} {}",
                ljust(&t, 22),
                ljust(&by, 22),
                py::str_of(&reason)
            ));
        }
        Ok(Exit::Ok)
    })();
    p.caught(r)
}

/// `cmd_heartbeat_log`'s reader.
pub fn log(raw: &str) -> Shown {
    let mut p = Py::default();
    let fail = |p: &mut Py, m: String, exit: Exit| -> R<Exit> {
        p.err(format!("Error: {} heartbeat log: {m}", cli_name!()));
        Ok(exit)
    };
    let r = (|| -> R<Exit> {
        let server = format!("{}'s server", product_name!());
        if raw.trim_matches(crate::families::py_space).is_empty() {
            return fail(
                &mut p,
                format!(
                    "{server} gave no answer on /cli/heartbeat-log; run {} daemon status",
                    cli_name!()
                ),
                Exit::Error,
            );
        }
        let Ok(rows) = loads(raw) else {
            return fail(
                &mut p,
                format!("{server} answered /cli/heartbeat-log with something that is not JSON"),
                Exit::Error,
            );
        };
        if let Value::Object(_) = rows {
            let e = dget_v(&rows, "error")?;
            if py::truthy(&e) {
                let mut err = index(&rows, "error")?;
                if let Value::Object(_) = err {
                    err = or(dget_v(&err, "hint")?, dget_v(&err, "code")?);
                }
                if let Value::Object(_) = dget_v(&rows, "permission")? {
                    return fail(
                        &mut p,
                        format!("{server} refused /cli/heartbeat-log: {}", py::str_of(&err)),
                        Exit::Refused,
                    );
                }
                return fail(
                    &mut p,
                    format!(
                        "{server} answered /cli/heartbeat-log with an error: {}",
                        py::str_of(&err)
                    ),
                    Exit::Error,
                );
            }
        }
        let list = match &rows {
            Value::Array(items) if items.iter().all(|r| matches!(r, Value::Object(_))) => {
                items.clone()
            }
            other => return fail(
                &mut p,
                format!(
                    "{server} answered /cli/heartbeat-log with a {}, not a list of heartbeat rows",
                    py::type_name(other)
                ),
                Exit::Error,
            ),
        };
        if list.is_empty() {
            p.out("No heartbeat activity recorded yet.");
            return Ok(Exit::Ok);
        }
        p.out(format!(
            "{} {} {} {} Reason",
            ljust("Fired At", 22),
            ljust("Agent", 20),
            ljust("Decision", 22),
            ljust("Inbox", 8)
        ));
        p.out("-".repeat(100));
        for r in &list {
            let fired = stamp(&dget_or(r, "firedAt", s("?"))?)?;
            let agent = or(dget_v(r, "agentName")?, s("(project)"));
            let decision = dget_or(r, "decision", s("?"))?;
            let count = or(dget_v(r, "inboxCount")?, Value::Int("0".into()));
            let pri = or(dget_v(r, "inboxPriority")?, s(""));
            let tail = if py::truthy(&pri) {
                str_plus(" ", &pri)?
            } else {
                String::new()
            };
            let inbox = format!("{}{tail}", py::str_of(&count));
            let reason = or(dget_v(r, "reason")?, s(""));
            p.out(format!(
                "{} {} {} {} {}",
                ljust(&fired, 22),
                left(&agent, 20)?,
                left(&decision, 22)?,
                ljust(&inbox, 8),
                py::str_of(&reason)
            ));
        }
        Ok(Exit::Ok)
    })();
    p.raised(r, 30)
}

/// `hb_warn_if_transport_down`'s reader: lines on stderr, or none.
pub fn transport_warning(raw: &str) -> String {
    let Ok(d) = loads(raw) else {
        return String::new();
    };
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let installed = dget_v(&d, "transportInstalled")?;
        let stale = dget_v(&d, "staleSecs")?;
        if installed == Value::Bool(false) {
            p.err("");
            p.err("WARNING: no schedule transport is installed on this machine (launchd/cron).");
            p.err("         This heartbeat will NOT fire until the daemon installs it \u{2014}");
            p.err(format!(
                "         check `{} daemon status` and Settings \u{2192} Logs \u{2192} Heartbeats.",
                cli_name!()
            ));
            return Ok(Exit::Ok);
        }
        if stale == Value::Null {
            return Ok(Exit::Ok);
        }
        let over = |v: &Value, n: i128| -> R<bool> {
            match v {
                Value::Int(d) => Ok(d.parse::<i128>().map_or(!d.starts_with('-'), |x| x > n)),
                Value::Bool(b) => Ok(i128::from(*b) > n),
                Value::Float(f) => Ok(*f > n as f64),
                other => Err(format!(
                    "TypeError: '>' not supported between instances of '{}' and 'int'",
                    py::type_name(other)
                )),
            }
        };
        if !over(&stale, 900)? {
            return Ok(Exit::Ok);
        }
        let enabled = dget_or(&d, "enabledCount", Value::Int("0".into()))?;
        if !over(&enabled, 0)? {
            return Ok(Exit::Ok);
        }
        let minutes = match &stale {
            Value::Float(f) => py::str_of(&Value::Float((f / 60.0).floor())),
            Value::Bool(b) => (i128::from(*b) / 60).to_string(),
            v => match py::str_of(v).parse::<i128>() {
                Ok(n) => n.div_euclid(60).to_string(),
                Err(_) => py::str_of(v),
            },
        };
        p.err("");
        p.err(format!(
            "WARNING: the heartbeat scheduler has not ticked for {minutes} minutes."
        ));
        p.err(format!(
            "         Scheduled fires are not being delivered \u{2014} check `{} daemon status`.",
            cli_name!()
        ));
        Ok(Exit::Ok)
    })();
    p.raised(r, 7).stderr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_table() {
        let rows = r#"[{"name":"daily","frequency":"daily","specJson":"{\"time\":\"07:00\"}","enabled":true,"lastFired":null}]"#;
        let shown = list(rows);
        assert_eq!(shown.exit, Exit::Ok, "{shown:?}");
        let lines: Vec<&str> = shown.stdout.lines().collect();
        assert!(lines[0].starts_with("Name                 Frequency  Schedule"));
        assert_eq!(lines[1].len(), 141 + 12);
        assert!(
            lines[2].starts_with("daily                daily      at 07:00"),
            "{}",
            lines[2]
        );
    }

    #[test]
    fn the_list_errors() {
        let shown = list(r#"{"error":{"code":"owner_only","hint":"no"}}"#);
        assert_eq!(shown.exit, Exit::Refused);
        assert_eq!(
            shown.stderr,
            "Error: no \u{2014} requires owner/admin \u{2014} ask your human\n"
        );
        assert_eq!(
            list("[]").stdout,
            "No heartbeats configured for this workspace.\n"
        );
        assert_eq!(list("5").stderr, "Error: 'int' object is not iterable\n");
    }
}
