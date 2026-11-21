//! `k5 hooks status|install|uninstall`, and `hooks` with no verb or one it
//! has no arm for, as the shell CLI's `hooks` arm answers them
//! (`cli/src/router.sh`; `cmd_hooks_status` and `cmd_hooks_install` in
//! `cli/src/remind.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]). `hooks` is an open tool.
//! 2. No verb, or one with no arm: the four usage lines
//!    (`cli/text/help/hooks.txt`) on stderr, exit 1.
//! 3. `status [--limit|-n N] [--json]`: a `--limit` or `-n` with no word
//!    after it, or one that is not digits only, is `Error: <flag> needs a
//!    number` on stderr, exit 2, nothing sent. Otherwise one `cli_request
//!    /cli/hooks/status limit=<N>` over TCP (20 by default; digits, so its
//!    `urlencode` is the number). `install` and `uninstall` `[--json]`: one
//!    `cli_post_json /cli/hooks/install` over TCP with `{}` or
//!    `{"remove":true}`. Other words are skipped.
//! 4. The answer as the shell holds it in `raw=$(…)`: NULs and trailing
//!    newlines dropped. `--json` echoes it. Otherwise the shell's Python
//!    renders it. `status` ([`status_text`]) has its stderr thrown away;
//!    where that Python raises, what it printed before is followed by
//!    `raw` and a newline (`|| echo "$raw"`), exit 0. `install`
//!    ([`install_out`]): an answer that is not a JSON object prints as it
//!    came; one with an `error` key is `Error: <the error>` on stderr and
//!    exit 1, or 3 with a `permission` object; a report it can't lay out
//!    ([`install_text`]) is its lines, then the answer as it came, exit 0.
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! A request with no answer is the transport's line and exit 1; any other
//! failure ends with the code the shell's call returns, with nothing
//! printed.
//!
//! The shell answers ([`run`] returns `None`): `hooks emit` (whole), `-h`
//! or `--help` (the front sends those on: the hooks page, or emit's), a
//! word that is not UTF-8, a port curl won't take, and a `python3` the
//! binary can't tell.

use super::{curl_port, without_nuls};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::pylib::json::loads;
use crate::out::pylib::ops::py_eq;
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::{Call, Channel, Error};
use k2_client::{cli_name, product_name};
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["hooks"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

pub const STATUS: &str = "/cli/hooks/status";
pub const INSTALL: &str = "/cli/hooks/install";

/// The `*)` arm's lines, on stderr, exit 1.
pub const USAGE: [&str; 4] = [
    concat!("Usage: ", cli_name!(), " hooks status [--limit N] [--json]"),
    concat!(
        "       ",
        cli_name!(),
        " hooks install [--json]     re-run the daemon's hook installer now"
    ),
    concat!(
        "       ",
        cli_name!(),
        " hooks uninstall [--json]   remove ",
        product_name!(),
        "'s entries from the agent CLI configs"
    ),
    concat!(
        "       ",
        cli_name!(),
        " hooks emit <event> [--json '<obj>']  report a custom program's lifecycle (",
        cli_name!(),
        " study hooks)"
    ),
];

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// No verb or one with no arm.
    Usage,
    /// `status`: the limit (digits), and `--json`.
    Status { limit: String, json: bool },
    /// `status` with a `--limit` or `-n` (the flag as typed) that has no
    /// word after it, or one that is not digits only.
    NeedsNumber(String),
    /// `install` (false) or `uninstall` (true), and `--json`.
    Install { remove: bool, json: bool },
}

/// The words after `hooks`. `None`: the shell's.
pub fn parse(words: &[&str]) -> Option<Ask> {
    let (verb, rest) = match words.split_first() {
        Some((v, r)) => (*v, r),
        None => ("", &[][..]),
    };
    let json = rest.contains(&"--json");
    match verb {
        "status" => {
            // The shell's loop, left to right: the first bad limit stops it.
            let mut limit = "20".to_string();
            let mut json = false;
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    flag @ ("--limit" | "-n") => match rest.get(i + 1) {
                        Some(v) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => {
                            limit = (*v).to_string();
                            i += 2;
                        }
                        _ => return Some(Ask::NeedsNumber(flag.to_string())),
                    },
                    "--json" => {
                        json = true;
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            Some(Ask::Status { limit, json })
        }
        "install" => Some(Ask::Install {
            remove: false,
            json,
        }),
        "uninstall" => Some(Ask::Install { remove: true, json }),
        "emit" => None,
        _ => Some(Ask::Usage),
    }
}

/// `k5 hooks ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let ask = parse(&words)?;
    if !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    if ask == Ask::Usage {
        for l in USAGE {
            out::stderr_line(l);
        }
        return Some(Exit::Error);
    }
    if let Ask::NeedsNumber(flag) = &ask {
        out::stderr_line(&format!("Error: {flag} needs a number"));
        return Some(Exit::Usage);
    }
    let mut settings = settings;
    settings.port = Some(settings.port_text.as_deref().and_then(curl_port)?);
    Some(send(args, ask, settings))
}

fn send(args: &[OsString], ask: Ask, settings: Settings) -> Exit {
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let passport = client.passport().is_some();
    let mut closing = Closing::default();
    let query = match &ask {
        Ask::Status { limit, .. } => format!("limit={limit}"),
        _ => String::new(),
    };
    let pairs = [query.as_str()];
    let (endpoint, call, json, status) = match &ask {
        Ask::Status { json, .. } => (STATUS, Call::Request(&pairs), *json, true),
        Ask::Install { remove, json } => {
            let body: &[u8] = if *remove { b"{\"remove\":true}" } else { b"{}" };
            (INSTALL, Call::PostJson(body), *json, false)
        }
        Ask::Usage | Ask::NeedsNumber(_) => unreachable!("answered before"),
    };
    let exit = match client.call(Channel::Tcp, endpoint, call) {
        Ok(r) => {
            closing.answer(&r, passport);
            let mut raw = without_nuls(&r.body);
            while raw.last() == Some(&b'\n') {
                raw.pop();
            }
            let (text, error) = if json {
                (echo(&raw), None)
            } else if status {
                (rendered(&raw), None)
            } else {
                install_out(&raw)
            };
            match out::write_flushed(&mut std::io::stdout().lock(), &text) {
                Ok(()) => match error {
                    Some((line, exit)) => {
                        out::stderr_line(&line);
                        exit
                    }
                    None => Exit::Ok,
                },
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Err(e @ Error::NoAnswer(_)) => {
            out::stderr_line(&e.to_string());
            Exit::Error
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    };
    closing.flush(&mut client, exit);
    exit
}

/// `echo "$raw"`: bash's echo reads a word of only `-` and `n`, `e` or
/// `E` as its options (no newline after `-n`).
pub fn echo(raw: &[u8]) -> Vec<u8> {
    if raw.len() > 1 && raw[0] == b'-' && raw[1..].iter().all(|b| matches!(b, b'n' | b'e' | b'E')) {
        return if raw.contains(&b'n') {
            Vec::new()
        } else {
            b"\n".to_vec()
        };
    }
    let mut out = raw.to_vec();
    out.push(b'\n');
    out
}

/// `status`' `echo "$raw" | python3 -c "…" 2>/dev/null || echo "$raw"`:
/// the Python's lines, or what it printed before it stopped, then the raw
/// answer.
pub fn rendered(raw: &[u8]) -> Vec<u8> {
    let mut printed = String::new();
    let ok = std::str::from_utf8(raw).ok().and_then(|text| {
        let data = loads(&format!("{text}\n")).ok()?;
        status_text(&data, now(), &mut printed).ok()
    });
    let mut out = printed.into_bytes();
    if ok.is_none() {
        out.extend(echo(raw));
    }
    out
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The Python stopped (it raised, or exited 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped;

type R<T> = Result<T, Stopped>;

/// `x.get(key)` on `x`, which must be a dict.
fn get<'a>(x: &'a Value, key: &str) -> R<Option<&'a Value>> {
    match x {
        Value::Object(pairs) => Ok(super::get(pairs, key)),
        _ => Err(Stopped),
    }
}

/// `x.get(key, default)` as a value.
fn get_or(x: &Value, key: &str, default: Value) -> R<Value> {
    Ok(get(x, key)?.cloned().unwrap_or(default))
}

/// `x.get(key) or default`.
fn get_truthy(x: &Value, key: &str, default: Value) -> R<Value> {
    Ok(get(x, key)?
        .filter(|v| py::truthy(v))
        .cloned()
        .unwrap_or(default))
}

fn s(text: &str) -> Value {
    Value::Str(text.into())
}

fn empty() -> Value {
    Value::Object(Vec::new())
}

/// `f'{x}'`, as `print` can write it: a string with a lone surrogate
/// can't be written, and the print raises.
fn str_of(v: &Value) -> R<String> {
    match v {
        Value::Str(t) => t.as_str().map(str::to_string).ok_or(Stopped),
        other => Ok(py::str_of(other)),
    }
}

/// `f'{x:<n}'` (`right`: `>n`) for the values JSON gives; the types whose
/// `__format__` refuses a spec raise.
fn fmt(v: &Value, width: usize, right: bool) -> R<String> {
    let text = match v {
        Value::Str(_) | Value::Int(_) | Value::Float(_) => str_of(v)?,
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        _ => return Err(Stopped),
    };
    let n = text.chars().count();
    let pad = " ".repeat(width.saturating_sub(n));
    Ok(if right {
        format!("{pad}{text}")
    } else {
        format!("{text}{pad}")
    })
}

/// `len(x)`.
fn len(v: &Value) -> R<usize> {
    match v {
        Value::Str(t) => Ok(t.code_points().len()),
        Value::Array(a) => Ok(a.len()),
        Value::Object(o) => Ok(o.len()),
        _ => Err(Stopped),
    }
}

/// The shell's `ago(ms)`.
fn ago(ms: Option<&Value>, now: f64) -> R<String> {
    let Some(ms) = ms.filter(|m| py::truthy(m)) else {
        return Ok("never".into());
    };
    let ms = match ms {
        Value::Int(d) => d.parse::<f64>().map_err(|_| Stopped)?,
        Value::Float(f) => *f,
        Value::Bool(_) => 1.0,
        _ => return Err(Stopped),
    };
    if ms.is_infinite() {
        return Err(Stopped);
    }
    let d = now - ms / 1000.0;
    if !d.is_finite() {
        return Err(Stopped);
    }
    let s = d.trunc().max(0.0) as u64;
    Ok(if s < 120 {
        format!("{s}s ago")
    } else if s < 7200 {
        format!("{}m ago", s / 60)
    } else {
        format!("{}h ago", s / 3600)
    })
}

fn line(out: &mut String, text: &str) {
    out.push_str(text);
    out.push('\n');
}

/// `cmd_hooks_status`' Python: its lines onto `out`, until it raises.
pub fn status_text(data: &Value, now: f64, out: &mut String) -> R<()> {
    let inj = get_or(data, "injections", empty())?;
    let script = get_or(&inj, "notify_script", empty())?;
    let clis = get_or(data, "clis", empty())?;
    let port = str_of(get(data, "port")?.unwrap_or(&Value::Null))?;
    line(
        out,
        &format!(concat!(product_name!(), " hook server port: {}"), port),
    );
    let path = str_of(&get_or(&script, "path", s("?"))?)?;
    let exists = str_of(&get_or(&script, "exists", Value::Bool(false))?)?;
    let version = str_of(&get_truthy(&script, "version", s("?"))?)?;
    line(
        out,
        &format!("Notify script: {path}  exists={exists}  version={version}"),
    );
    if let Some(owner) = get(data, "ownerCheck")?.filter(|o| py::truthy(o)) {
        let note = if py_eq(owner, &s("conversation")) {
            "  (Windows: conversation id only, no process check)"
        } else {
            ""
        };
        line(out, &format!("Owner check: {}{note}", str_of(owner)?));
    }
    let last = get_truthy(data, "lastInstall", empty())?;
    if let Some(skipped) = get(&last, "skipped")?.filter(|v| py::truthy(v)) {
        line(
            out,
            &format!("Last install pass: skipped ({})", str_of(skipped)?),
        );
    } else if let Some(r) = get(&last, "report")?.filter(|v| py::truthy(v)) {
        let action = str_of(get(r, "action")?.unwrap_or(&Value::Null))?;
        let trigger = str_of(get(&last, "trigger")?.unwrap_or(&Value::Null))?;
        let on = get(&last, "agentHooksSetting")?.is_some_and(py::truthy);
        line(
            out,
            &format!(
                "Last install pass: {action} ({trigger}), agentHooks setting {}",
                if on { "on" } else { "off" }
            ),
        );
    }
    line(out, "");
    line(out, "Config injections:");
    for key in ["claude", "cursor", "gemini"] {
        let c = get_or(&inj, key, empty())?;
        let d = get_or(&clis, key, empty())?;
        let unreadable = get(&c, "configUnreadable")?.is_some_and(py::truthy);
        let tag = if unreadable {
            "!! "
        } else if get(&c, "injected")?.is_some_and(py::truthy) {
            "OK "
        } else if get(&c, "exists")?.is_some_and(py::truthy) {
            "NO "
        } else {
            "-- "
        };
        let mut extra = Vec::new();
        if key == "claude" {
            if let Some(v) = get(&d, "detectedVersion")?.filter(|v| py::truthy(v)) {
                let set = str_of(&get_truthy(&d, "eventSet", s("?"))?)?;
                extra.push(format!("claude {} ({set} set)", str_of(v)?));
            }
        }
        if get(&c, "configUnreadable")?.is_some_and(py::truthy) {
            extra.push("config_unreadable: left untouched".into());
        }
        let mut events = get(&d, "installedEvents")?.filter(|v| py::truthy(v));
        if events.is_none() {
            events = get(&c, "events")?.filter(|v| py::truthy(v));
        }
        if let Some(events) = events {
            extra.push(format!("{} events", len(events)?));
        }
        extra.push(format!("last event {}", ago(get(&d, "lastEventMs")?, now)?));
        let path = str_of(&get_or(&c, "path", s("?"))?)?;
        line(
            out,
            &format!("  [{tag}] {key:<8} {path}  {}", extra.join("; ")),
        );
    }
    let sessions = get_truthy(data, "sessions", empty())?;
    if py::truthy(&sessions) {
        line(out, "");
        line(out, "Live sessions:");
        line(
            out,
            &format!(
                "  {:<38} {:<16} {:<10} {:>8} {:>8} {:>8} {:>6}  Last",
                "Session", "Owner", "Evidence", "Accepted", "Foreign", "Unknown", "Parse"
            ),
        );
        let Value::Object(pairs) = &sessions else {
            return Err(Stopped);
        };
        for (sid, e) in pairs {
            let o = get_truthy(e, "owner", empty())?;
            let proc = get_truthy(&o, "proc", empty())?;
            let who = match get(&proc, "pid")?.filter(|v| py::truthy(v)) {
                Some(pid) => s(&format!("pid {}", str_of(pid)?)),
                None => get_truthy(&o, "via", s("none"))?,
            };
            let c = get_truthy(e, "counters", empty())?;
            let ev = get_truthy(e, "evidence", s("-"))?;
            let sid = fmt(&Value::Str(sid.clone()), 38, false)?;
            let who = fmt(&who, 16, false)?;
            let ev = fmt(&ev, 10, false)?;
            let zero = || Value::Int("0".into());
            let accepted = fmt(&get_or(&c, "accepted", zero())?, 8, true)?;
            let foreign = fmt(&get_or(&c, "foreign", zero())?, 8, true)?;
            let unknown = fmt(&get_or(&c, "unknownPane", zero())?, 8, true)?;
            let parse = fmt(&get_or(&c, "parseError", zero())?, 6, true)?;
            let last = ago(get(e, "lastEnvelopeMs")?, now)?;
            line(
                out,
                &format!("  {sid} {who} {ev} {accepted} {foreign} {unknown} {parse}  {last}"),
            );
        }
    }
    if let Some(totals) = get(data, "totals")?.filter(|v| py::truthy(v)) {
        let zero = || Value::Int("0".into());
        let t = |k: &str| -> R<String> { str_of(&get_or(totals, k, zero())?) };
        line(
            out,
            &format!(
                "Totals: accepted {}, foreign {}, unknown pane {}, parse errors {}",
                t("accepted")?,
                t("foreign")?,
                t("unknownPane")?,
                t("parseError")?
            ),
        );
    }
    line(out, "");
    let events = get_or(data, "recent_events", Value::Array(Vec::new()))?;
    let cap = get_or(data, "recent_events_cap", Value::Int("50".into()))?;
    if !py::truthy(&events) {
        line(
            out,
            &format!(
                "Recent events (0 of last {}): none received yet \u{2014} launch a Claude session and run a prompt.",
                str_of(&cap)?
            ),
        );
        return Ok(());
    }
    line(
        out,
        &format!(
            "Recent events (showing last {} of {}):",
            len(&events)?,
            str_of(&cap)?
        ),
    );
    line(
        out,
        &format!(
            "{:<20} {:<13} {:<22} {:<14} Pane",
            "Time", "Verdict", "Raw", "Tool"
        ),
    );
    line(out, &"-".repeat(100));
    let Value::Array(items) = &events else {
        // A string's characters and a dict's keys have no `.get`.
        return Err(Stopped);
    };
    for e in items {
        let ts = match get_truthy(e, "timestamp", s(""))? {
            Value::Str(t) => {
                let t = t.as_str().ok_or(Stopped)?;
                t.chars().take(19).collect::<String>().replace('T', " ")
            }
            _ => return Err(Stopped),
        };
        let mut verdict = get(e, "verdict")?.filter(|v| py::truthy(v));
        if verdict.is_none() {
            verdict = get(e, "canonical")?.filter(|v| py::truthy(v));
        }
        let verdict = verdict.cloned().unwrap_or_else(|| s("(unmatched)"));
        let raw = get_truthy(e, "raw_event", s(""))?;
        let tool = get_truthy(e, "tool_name", s(""))?;
        let pane = get_truthy(e, "pane_id", s(""))?;
        let ts = fmt(&s(&ts), 20, false)?;
        let verdict = fmt(&verdict, 13, false)?;
        let raw = fmt(&raw, 22, false)?;
        let tool = fmt(&tool, 14, false)?;
        let pane = str_of(&pane)?;
        line(out, &format!("{ts} {verdict} {raw} {tool} {pane}"));
    }
    Ok(())
}

/// `cmd_hooks_install`' Python on `raw` (`echo "$raw"` is its input):
/// what it prints on stdout, and its `Error:` line with the exit code
/// when the answer has an `error` key. An answer that is not UTF-8, not
/// JSON or not an object prints as it came (`as_it_came`); so does the
/// rest of one whose report it can't lay out, after the lines it made.
pub fn install_out(raw: &[u8]) -> (Vec<u8>, Option<(String, Exit)>) {
    let input = echo(raw);
    let data = std::str::from_utf8(&input)
        .ok()
        .and_then(|text| loads(text).ok());
    let Some(data @ Value::Object(pairs)) = &data else {
        return (input, None);
    };
    if let Some(err) = super::get(pairs, "error") {
        // `err.get('hint') or err.get('code')` for an object; `f'{err}'`,
        // on stderr, where a lone surrogate is written `\udXXX`.
        let err = match err {
            Value::Object(e) => super::get(e, "hint")
                .filter(|h| py::truthy(h))
                .or_else(|| super::get(e, "code"))
                .cloned()
                .unwrap_or(Value::Null),
            other => other.clone(),
        };
        let refused = matches!(super::get(pairs, "permission"), Some(Value::Object(_)));
        let exit = if refused { Exit::Refused } else { Exit::Error };
        return (
            Vec::new(),
            Some((format!("Error: {}", py::str_of(&err)), exit)),
        );
    }
    let mut printed = String::new();
    let ok = install_text(data, &mut printed).is_ok();
    let mut out = printed.into_bytes();
    if !ok {
        out.extend(input);
    }
    (out, None)
}

/// `cmd_hooks_install`' Python inside its `try`, for an object with no
/// `error` key: its lines onto `out`, until it raises.
pub fn install_text(data: &Value, out: &mut String) -> R<()> {
    if let Some(skipped) = get(data, "skipped")?.filter(|v| py::truthy(v)) {
        line(out, &format!("Hooks not changed: {}.", str_of(skipped)?));
        return Ok(());
    }
    let r = get_truthy(data, "report", empty())?;
    let action = get_or(&r, "action", s("?"))?;
    let script = get_truthy(&r, "script", empty())?;
    let state = str_of(&get_or(&script, "state", s("?"))?)?;
    let path = str_of(&get_or(&script, "path", s("?"))?)?;
    line(
        out,
        &format!("Hook {}: notify.sh {state} ({path})", str_of(&action)?),
    );
    if py_eq(&action, &s("install")) {
        let version = str_of(&get_truthy(&r, "claudeVersion", s("version unknown"))?)?;
        let set = str_of(&get_or(&r, "claudeEventSet", s("?"))?)?;
        line(out, &format!("Claude {version}: {set} event set"));
    }
    let setting = get_or(data, "agentHooksSetting", Value::Bool(true))?;
    if !py::truthy(&setting) && py_eq(&action, &s("remove")) {
        line(
            out,
            concat!(
                "The agentHooks setting is off, so ",
                product_name!(),
                " hooks were removed."
            ),
        );
    }
    let clis = get_or(&r, "clis", Value::Array(Vec::new()))?;
    let items = super::py_iter(&clis, 0).map_err(|_| Stopped)?;
    for c in items {
        let note = if py_eq(
            get(&c, "state")?.unwrap_or(&Value::Null),
            &s("config_unreadable"),
        ) {
            "  (did not parse; left untouched)".to_string()
        } else if let Some(e) = get(&c, "error")?.filter(|v| py::truthy(v)) {
            format!("  ({})", str_of(e)?)
        } else {
            String::new()
        };
        let cli = fmt(&get_or(&c, "cli", s("?"))?, 8, false)?;
        let state = fmt(&get_or(&c, "state", s("?"))?, 18, false)?;
        let path = str_of(&get_or(&c, "path", s("?"))?)?;
        line(out, &format!("  {cli} {state} {path}{note}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src");
        let mut text = String::new();
        for f in ["remind.sh", "router.sh"] {
            text.push_str(
                &std::fs::read_to_string(root.join(f)).unwrap_or_else(|e| panic!("read {f}: {e}")),
            );
        }
        text
    }

    /// The arm, the requests and the reader's fallback as the shell has
    /// them; the binary's lines are checked against bash in
    /// `tests/hooks_parity.rs`.
    #[test]
    fn the_shell_s_arm_and_requests() {
        let text = source();
        for shell in [
            r#"status)    shift 2; cmd_hooks_status "$@" ;;"#,
            r#"install)   shift 2; cmd_hooks_install 0 "$@" ;;"#,
            r#"uninstall) shift 2; cmd_hooks_install 1 "$@" ;;"#,
            r#"emit)      shift 2; cmd_hooks_emit "$@" ;;"#,
            r#"*)         _hooks_usage >&2; exit 1 ;;"#,
            r#"--limit|-n)"#,
            r#"''|*[!0123456789]*) echo "Error: $1 needs a number" >&2; exit 2 ;;"#,
            r#"raw=$(cli_request "/cli/hooks/status" "limit=$(urlencode "$limit")")"#,
            r#"[ "$remove" = "1" ] && body='{"remove":true}'"#,
            r#"raw=$(cli_post_json "/cli/hooks/install" "$body")"#,
            r#"" 2>/dev/null || echo "$raw""#,
            "        err = err.get('hint') or err.get('code')",
            "    sys.exit(3 if isinstance(data.get('permission'), dict) else 1)",
            r#"" || rc=$?"#,
        ] {
            assert!(text.contains(shell), "the shell no longer has {shell:?}");
        }
        let page =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/text/help/hooks.txt");
        let page = std::fs::read_to_string(&page).unwrap_or_else(|e| panic!("read hooks.txt: {e}"));
        let lines: Vec<String> = USAGE
            .iter()
            .map(|l| l.replace(cli_name!(), "k2").replace(product_name!(), "K2"))
            .collect();
        assert_eq!(
            page.lines().collect::<Vec<_>>(),
            lines,
            "the usage lines moved"
        );
    }

    #[test]
    fn the_words() {
        assert_eq!(parse(&[]), Some(Ask::Usage));
        assert_eq!(parse(&["nope"]), Some(Ask::Usage));
        assert_eq!(parse(&["emit", "Stop"]), None);
        assert_eq!(
            parse(&["status", "-n", "5", "--json"]),
            Some(Ask::Status {
                limit: "5".into(),
                json: true
            })
        );
        assert_eq!(
            parse(&["status", "--limit", "--json"]),
            Some(Ask::NeedsNumber("--limit".into()))
        );
        assert_eq!(
            parse(&["status", "--limit"]),
            Some(Ask::NeedsNumber("--limit".into()))
        );
        assert_eq!(
            parse(&["status", "-n", "5", "--limit", "a b"]),
            Some(Ask::NeedsNumber("--limit".into()))
        );
        assert_eq!(
            parse(&["status", "-n", "", "--limit"]),
            Some(Ask::NeedsNumber("-n".into()))
        );
        assert_eq!(
            parse(&["status", "--json", "-n", "007"]),
            Some(Ask::Status {
                limit: "007".into(),
                json: true
            })
        );
        assert_eq!(
            parse(&["uninstall", "x", "--json"]),
            Some(Ask::Install {
                remove: true,
                json: true
            })
        );
    }

    #[test]
    fn install_s_reader() {
        let err = |raw: &str| install_out(raw.as_bytes()).1;
        assert_eq!(
            err(r#"{"error":"boom"}"#),
            Some(("Error: boom".into(), Exit::Error))
        );
        assert_eq!(
            err(r#"{"error":{"code":"c","hint":""}}"#),
            Some(("Error: c".into(), Exit::Error))
        );
        assert_eq!(
            err(r#"{"error":{"hint":"h"},"permission":{}}"#),
            Some(("Error: h".into(), Exit::Refused))
        );
        assert_eq!(
            err(r#"{"error":{},"permission":[]}"#),
            Some(("Error: None".into(), Exit::Error))
        );
        assert_eq!(
            err(r#"{"error":"a \ud800"}"#),
            Some(("Error: a \\ud800".into(), Exit::Error))
        );
        assert_eq!(
            install_out(br#""an error""#),
            (b"\"an error\"\n".to_vec(), None)
        );
        assert_eq!(install_out(b"-n"), (Vec::new(), None));
        assert_eq!(install_out(b"x\xff"), (b"x\xff\n".to_vec(), None));
        assert_eq!(
            install_out(br#"{"report":{"clis":[{"cli":null}]}}"#),
            (
                b"Hook ?: notify.sh ? (?)\n{\"report\":{\"clis\":[{\"cli\":null}]}}\n".to_vec(),
                None
            )
        );
    }

    #[test]
    fn echo_reads_its_options() {
        assert_eq!(echo(b"-n"), b"");
        assert_eq!(echo(b"-eE"), b"\n");
        assert_eq!(echo(b"-"), b"-\n");
        assert_eq!(echo(b"-x"), b"-x\n");
        assert_eq!(echo(b""), b"\n");
    }

    #[test]
    fn ago_as_the_shell_counts() {
        assert_eq!(ago(None, 1000.0), Ok("never".into()));
        assert_eq!(
            ago(Some(&Value::Int("0".into())), 1000.0),
            Ok("never".into())
        );
        assert_eq!(
            ago(Some(&Value::Int("999000".into())), 1000.0),
            Ok("1s ago".into())
        );
        assert_eq!(
            ago(Some(&Value::Int("1000000".into())), 7000.0),
            Ok("100m ago".into())
        );
        assert_eq!(
            ago(Some(&Value::Int("1000000".into())), 8200.0),
            Ok("2h ago".into())
        );
        assert_eq!(
            ago(Some(&Value::Int("9999999".into())), 1000.0),
            Ok("0s ago".into())
        );
        assert_eq!(ago(Some(&s("x")), 1000.0), Err(Stopped));
    }
}
