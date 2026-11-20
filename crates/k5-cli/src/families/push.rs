//! `k5 push ...`: phones and end-to-end encrypted pushes, as the shell
//! CLI's `cmd_push` answers them (`cli/src/push.sh`).
//!
//! Each verb is one `_push_send`: a `_k2_curl` of its own over TCP with
//! `$TOKEN` as the Bearer (connect 5 s, at most 30 s; a refusal's
//! `permission` object closes the command), then `_push_render`: no
//! answer (status `000`) is `<cli> push: daemon unreachable`, exit 1;
//! `--json` prints the body and exits by the status
//! (`_sidecar_exit_for`); else the view's Python ([`render`]).
//!
//! `$TOKEN` is the session's passport, or the owner token from disk.
//! `push` is an Open tool, so in a session the shell sends the owner
//! token from disk when there is one (COMPAT-58) and the binary the
//! passport: `scripts/cli-parity.sh`'s named difference
//! `owner-token-with-passport` (PRD 13.9 #4).
//!
//! The shell answers these forms itself ([`run`] returns `None`): any
//! `-h` or `--help` (the front sends those to a page or the shell), a
//! word that is not UTF-8, a port curl won't take, and a `python3` the
//! binary can't tell.

use super::heartbeat::py::{dget_v, has, iter, or, plus, str_plus, Py, R};
use super::{curl_port, s, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::urllib::quote_plus;
use crate::transport::{Auth, OwnCurl};
use k2_client::cli_name;
use std::ffi::OsString;
use std::time::Duration;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["push"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

pub const DEVICES: &str = "/cli/push/devices";
pub const E2E: &str = "/cli/push/e2e";
pub const DEVICE_REMOVE: &str = "/cli/push/device/remove";
pub const DEVICE_E2E: &str = "/cli/push/device/e2e";
pub const LOG: &str = "/cli/push/log";

/// What a command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The help on stdout.
    Help,
    /// A line on stderr, exit 2.
    Usage(String),
    /// One request: GET (`body` `None`) or POST, and the view.
    Send {
        endpoint: String,
        body: Option<String>,
        view: &'static str,
        json: bool,
    },
}

fn usage(line: String) -> Ask {
    Ask::Usage(line)
}

/// The words after `push`.
pub fn parse(words: &[&str]) -> Ask {
    let c = cli_name!();
    let sub = words.first().copied().unwrap_or("");
    if matches!(sub, "" | "help" | "--help" | "-h") {
        return Ask::Help;
    }
    let mut json = false;
    let mut args: Vec<&str> = Vec::new();
    for a in &words[1..] {
        match *a {
            "--json" => json = true,
            "-h" | "--help" => return Ask::Help,
            a => args.push(a),
        }
    }
    let send = |endpoint: &str, body: Option<String>, view| Ask::Send {
        endpoint: endpoint.to_string(),
        body,
        view,
        json,
    };
    match sub {
        "devices" | "ls" => {
            if !args.is_empty() {
                return usage(format!("usage: {c} push devices [--json]"));
            }
            send(DEVICES, None, "devices")
        }
        "e2e" => {
            if args.is_empty() {
                return send(E2E, None, "mode");
            }
            if args.len() != 1 {
                return usage(format!(
                    "usage: {c} push e2e [off|sealed-when-supported|sealed-only]"
                ));
            }
            match args[0] {
                m @ ("off" | "sealed-when-supported" | "sealed-only") => {
                    send(E2E, Some(format!("{{\"mode\":\"{m}\"}}")), "mode")
                }
                m => usage(format!(
                    "usage: {c} push e2e off|sealed-when-supported|sealed-only (got '{m}')"
                )),
            }
        }
        "device" => {
            let first = args.first().copied().unwrap_or("");
            if first == "remove" || first == "rm" {
                if args.len() != 2 || args[1].is_empty() {
                    return usage(format!("usage: {c} push device remove <deviceId>"));
                }
                return send(
                    DEVICE_REMOVE,
                    Some(dumps(&[("deviceId", args[1])])),
                    "removed",
                );
            }
            if first != "e2e" || args.len() != 3 {
                return usage(format!(
                    "usage: {c} push device e2e <deviceId> off|sealed|default | {c} push device remove <deviceId>"
                ));
            }
            match args[2] {
                "off" | "sealed" | "default" => send(
                    DEVICE_E2E,
                    Some(dumps(&[("deviceId", args[1]), ("mode", args[2])])),
                    "override",
                ),
                m => usage(format!(
                    "usage: {c} push device e2e <deviceId> off|sealed|default (got '{m}')"
                )),
            }
        }
        "log" => {
            let mut dev = String::new();
            let mut i = 0;
            while i < args.len() {
                match args[i] {
                    "--device" => {
                        i += 1;
                        dev = args.get(i).copied().unwrap_or("").to_string();
                        if dev.is_empty() {
                            return usage(format!("usage: {c} push log [--json] [--device <id>]"));
                        }
                    }
                    a if a.starts_with("--device=") => dev = a["--device=".len()..].to_string(),
                    a => {
                        return usage(format!(
                            "usage: {c} push log [--json] [--device <id>] (got '{a}')"
                        ))
                    }
                }
                i += 1;
            }
            let endpoint = if dev.is_empty() {
                LOG.to_string()
            } else {
                format!("{LOG}?device={}", quote_plus(&dev))
            };
            send(&endpoint, None, "log")
        }
        other => usage(format!(
            "usage: unknown push verb '{other}' (devices|e2e|device e2e|device remove|log)"
        )),
    }
}

/// `json.dumps({...})` of string pairs (Python's defaults).
fn dumps(pairs: &[(&str, &str)]) -> String {
    let v = Value::Object(
        pairs
            .iter()
            .map(|(k, v)| (json::Text::from(*k), s(v)))
            .collect(),
    );
    json::to_string(&v, &json::Options::default()).unwrap_or_default()
}

/// `k5 push ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let ask = parse(&words);
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
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(args, ask, settings))
}

/// `_sidecar_exit_for <status>`.
fn exit_for(status: &str) -> Exit {
    match status {
        "200" => Exit::Ok,
        "400" | "413" => Exit::Usage,
        "403" => Exit::Refused,
        "404" => Exit::NotFound,
        _ => Exit::Error,
    }
}

fn run_with(args: &[OsString], ask: Ask, mut settings: Settings) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let (endpoint, body, view, json) = match ask {
        Ask::Help => {
            let text = crate::text::help("push").unwrap_or_default();
            return match out::stdout(text) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            };
        }
        Ask::Usage(line) => {
            out::stderr_line(&line);
            return Exit::Usage;
        }
        Ask::Send {
            endpoint,
            body,
            view,
            json,
        } => (endpoint, body, view, json),
    };
    let token = match &settings.auth {
        Auth::Passport { passport, .. } => passport.as_str().to_string(),
        Auth::Owner(t) => t.clone(),
        Auth::None => String::new(),
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let how = OwnCurl {
        post: body.is_some(),
        credential: Some(&token),
        max_time: Duration::from_secs(30),
        note: true,
    };
    let passport = client.passport().is_some();
    let answer = match client.own_curl(&endpoint, how, body.unwrap_or_default().as_bytes()) {
        Ok(r) => {
            closing.answer(&r, passport);
            Some((format!("{:03}", r.status), super::without_nuls(&r.body)))
        }
        Err(_) => None,
    };
    let exit = render(json, view, answer);
    closing.flush(&mut client, exit);
    exit
}

/// `_push_render`: `answer` is the status and the body (`None`: `000`).
fn render(json: bool, view: &str, answer: Option<(String, Vec<u8>)>) -> Exit {
    let (code, body) = match answer {
        Some((code, body)) if code != "000" => (code, body),
        _ => {
            out::stderr_line(&format!("{} push: daemon unreachable", cli_name!()));
            return Exit::Error;
        }
    };
    if json {
        let mut b = body;
        b.push(b'\n');
        if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &b) {
            return out::after_stdout_error(&e);
        }
        return exit_for(&code);
    }
    reader(&String::from_utf8_lossy(&body), &code, view).print()
}

fn exits(code: &str) -> Exit {
    match code {
        "400" | "413" => Exit::Usage,
        "403" => Exit::Refused,
        "404" => Exit::NotFound,
        _ => Exit::Error,
    }
}

/// `str(v)`.
fn st(v: &Value) -> String {
    py::str_of(v)
}

/// `_push_render`'s Python on the body.
pub fn reader(raw: &str, code: &str, view: &str) -> Shown {
    let c = cli_name!();
    let mut p = Py::default();
    let d = match crate::families::heartbeat::py::loads(raw) {
        Ok(d) => d,
        Err(_) => {
            let head: String = raw
                .trim_matches(super::py_space)
                .chars()
                .take(200)
                .collect();
            p.err(format!("{c} push: error: HTTP {code}: {head}"));
            return p.caught(Ok(Exit::Error));
        }
    };
    let r = (|| -> R<Exit> {
        let failed = code != "200" || dget_v(&d, "ok")? == Value::Bool(false);
        if failed {
            let e = dget_v(&d, "error")?;
            let m = if let Value::Object(_) = e {
                let head = or(dget_v(&e, "code")?, s("error"));
                let tail = if py::truthy(&dget_v(&e, "hint")?) {
                    str_plus(": ", &crate::families::heartbeat::py::index(&e, "hint")?)?
                } else {
                    String::new()
                };
                plus(&head, &tail)?
            } else {
                st(&or(e, s(&format!("HTTP {code}"))))
            };
            p.err(format!("{c} push: error: {m}"));
            return Ok(exits(code));
        }
        match view {
            "devices" => devices(&mut p, &d)?,
            "mode" => {
                p.out(format!("encrypted pushes: {}", st(&dget_v(&d, "mode")?)));
                if has(&d, "relaySealed")? {
                    let ready = if py::truthy(&dget_v(&d, "relaySealed")?) {
                        "ready (s1)"
                    } else {
                        "not ready: phones get today's pushes"
                    };
                    p.out(format!("relay           : {ready}"));
                }
            }
            "override" => {
                let o = or(
                    dget_v(&d, "override")?,
                    s("default (follows the server switch)"),
                );
                p.out(format!(
                    "device {} override: {}",
                    st(&dget_v(&d, "deviceId")?),
                    st(&o)
                ));
            }
            "removed" => p.out(format!(
                "removed device {} (its registration and push key)",
                st(&dget_v(&d, "deviceId")?)
            )),
            _ => log(&mut p, &d)?,
        }
        Ok(Exit::Ok)
    })();
    p.raised(r, 12)
}

/// `" · ".join(b for b in bits if b)`.
fn dot_join(bits: &[Value]) -> R<String> {
    let mut parts = Vec::new();
    for b in bits.iter().filter(|b| py::truthy(b)) {
        match b {
            Value::Str(_) => parts.push(st(b)),
            other => {
                return Err(format!(
                    "TypeError: sequence item {}: expected str instance, {} found",
                    parts.len(),
                    py::type_name(other)
                ))
            }
        }
    }
    Ok(parts.join(" \u{b7} "))
}

fn devices(p: &mut Py, d: &Value) -> R<()> {
    let sealed = if py::truthy(&dget_v(d, "relaySealed")?) {
        ""
    } else {
        "(relay not ready for encrypted pushes yet)"
    };
    p.out(format!(
        "encrypted pushes: {} {sealed}",
        st(&dget_v(d, "mode")?)
    ));
    let rows = or(dget_v(d, "devices")?, Value::Array(Vec::new()));
    if !py::truthy(&rows) {
        p.out("no phones registered");
    }
    for r in iter(&rows)? {
        let e2e = or(dget_v(&r, "e2e")?, Value::Object(Vec::new()));
        let get = |k: &str| -> R<Value> { crate::families::heartbeat::py::dget_or(&r, k, s("")) };
        let previews = if py::truthy(&dget_v(&r, "previews")?) {
            "on"
        } else {
            "off"
        };
        let mut bits = vec![
            get("deviceId")?,
            get("platform")?,
            get("username")?,
            s(&format!("previews {previews}")),
            get("status")?,
        ];
        if py::truthy(&dget_v(&r, "tokenFingerprint")?) {
            bits.push(s(&format!(
                "token {}",
                st(&dget_v(&r, "tokenFingerprint")?)
            )));
        }
        if py::truthy(&e2e) {
            bits.push(s(&format!("key {}", st(&dget_v(&e2e, "keyId")?))));
            bits.push(s(if py::truthy(&dget_v(&e2e, "active")?) {
                "active"
            } else {
                "inactive"
            }));
            if py::truthy(&dget_v(&e2e, "enforce")?) {
                bits.push(s("enforce"));
            }
            if py::truthy(&dget_v(&e2e, "override")?) {
                bits.push(s(&format!("override {}", st(&dget_v(&e2e, "override")?))));
            }
        }
        p.out(format!("  {}", dot_join(&bits)?));
    }
    Ok(())
}

/// A number Python sorts and divides: `atMs` (`or 0`).
fn at_ms(v: &Value) -> R<f64> {
    match v {
        Value::Int(d) => Ok(d.parse::<f64>().unwrap_or(f64::INFINITY)),
        Value::Float(f) => Ok(*f),
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        other => Err(format!(
            "TypeError: unsupported operand type(s) for /: '{}' and 'int'",
            py::type_name(other)
        )),
    }
}

/// `datetime.datetime.fromtimestamp(ms / 1000).strftime(...)`.
fn stamp(ms: &Value) -> R<String> {
    let secs = match ms {
        Value::Int(d) => match d.parse::<i64>() {
            Ok(n) => n.div_euclid(1000),
            Err(_) => {
                return Err("OverflowError: timestamp out of range for platform time_t".into())
            }
        },
        other => {
            let f = at_ms(other)? / 1000.0;
            if !f.is_finite() || f.abs() > 1e15 {
                return Err("ValueError: year is out of range".into());
            }
            // Microseconds, rounded half to even, as fromtimestamp does.
            let us = (f * 1e6).round_ties_even();
            (us / 1e6).floor() as i64
        }
    };
    let (y, _, _) = crate::families::heartbeat::when::civil_from_days(secs.div_euclid(86_400));
    if !(1..=9999).contains(&y) {
        return Err(format!("ValueError: year {y} is out of range"));
    }
    crate::out::pylib::time::strftime_local(secs, "%Y-%m-%d %H:%M:%S")
}

fn log(p: &mut Py, d: &Value) -> R<()> {
    let mut rows: Vec<(f64, u8, usize, String, Value, Value)> = Vec::new();
    let attempts = or(dget_v(d, "attempts")?, Value::Array(Vec::new()));
    for (i, a) in iter(&attempts)?.into_iter().enumerate() {
        let at = or(dget_v(&a, "atMs")?, Value::Int("0".into()));
        rows.push((0.0, 0, i, "send".into(), at, a));
    }
    let events = or(dget_v(d, "events")?, Value::Array(Vec::new()));
    for (i, e) in iter(&events)?.into_iter().enumerate() {
        let at = or(dget_v(&e, "atMs")?, Value::Int("0".into()));
        let what = st(&dget_v(&e, "action")?);
        rows.push((0.0, 1, i, what, at, e));
    }
    if rows.len() > 1 {
        for r in &mut rows {
            r.0 = match &r.4 {
                Value::Int(_) | Value::Float(_) | Value::Bool(_) => at_ms(&r.4)?,
                other => {
                    return Err(format!(
                        "TypeError: '<' not supported between instances of '{}' and 'int'",
                        py::type_name(other)
                    ))
                }
            };
        }
        rows.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
                .then(a.2.cmp(&b.2))
        });
    }
    if rows.is_empty() {
        let dev = dget_v(d, "device")?;
        let tail = if py::truthy(&dev) {
            format!(" for device {}", st(&dev))
        } else {
            String::new()
        };
        p.out(format!(
            "no push attempts or phone changes since the daemon started{tail}"
        ));
    }
    for (_, src, _, what, at_v, a) in rows {
        let at = stamp(&at_v)?;
        let f = |k: &str| -> R<String> { Ok(st(&dget_v(&a, k)?)) };
        if src == 1 {
            let session = or(dget_v(&a, "session")?, s("-"));
            let mut bits = vec![
                format!("reason={}", f("reason")?),
                format!("device={}", f("device")?),
                format!("platform={}", f("platform")?),
                format!("login={}", f("login")?),
                format!("session={}", st(&session)),
                format!("token={}", f("token")?),
            ];
            if py::truthy(&dget_v(&a, "app")?) {
                bits.push(format!("app={}", f("app")?));
            }
            if py::truthy(&dget_v(&a, "detail")?) {
                bits.push(f("detail")?);
            }
            p.out(format!(
                "{at}  {}  {}",
                crate::families::ljust(&what, 6),
                bits.join(" ")
            ));
            continue;
        }
        let mut bits = vec![
            format!("device={}", f("device")?),
            format!("kind={}", f("kind")?),
            format!("lane={}", f("lane")?),
            format!("bytes={}", f("bytes")?),
        ];
        if py::truthy(&dget_v(&a, "key")?) {
            bits.push(format!("key={}", f("key")?));
        }
        let status = dget_v(&a, "status")?;
        bits.push(format!(
            "status={}",
            if status == Value::Null {
                "-".to_string()
            } else {
                st(&status)
            }
        ));
        if py::truthy(&dget_v(&a, "error")?) {
            bits.push(format!("error={}", f("error")?));
        }
        bits.push(format!("path={}", f("path")?));
        bits.push(format!("relay={}", f("relay")?));
        if py::truthy(&dget_v(&a, "vendor")?) {
            bits.push(format!("vendor={}", f("vendor")?));
        }
        bits.push(format!("ms={}", f("ms")?));
        p.out(format!("{at}  send    {}", bits.join(" ")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forms() {
        assert_eq!(parse(&[]), Ask::Help);
        assert_eq!(parse(&["help"]), Ask::Help);
        assert!(matches!(parse(&["devices", "x"]), Ask::Usage(_)));
        assert_eq!(
            parse(&["device", "rm", "a\u{e9}"]),
            Ask::Send {
                endpoint: DEVICE_REMOVE.into(),
                body: Some("{\"deviceId\": \"a\\u00e9\"}".into()),
                view: "removed",
                json: false
            }
        );
        assert_eq!(
            parse(&["log", "--device", "a b", "--json"]),
            Ask::Send {
                endpoint: "/cli/push/log?device=a+b".into(),
                body: None,
                view: "log",
                json: true
            }
        );
    }

    #[test]
    fn the_views() {
        let r = reader(r#"{"mode":"off","relaySealed":false}"#, "200", "mode");
        assert_eq!(
            r.stdout,
            "encrypted pushes: off\nrelay           : not ready: phones get today's pushes\n"
        );
        let r = reader(r#"{"error":{"code":"x","hint":"y"}}"#, "403", "mode");
        assert_eq!(r.exit, Exit::Refused);
        assert!(r.stderr.ends_with("push: error: x: y\n"));
    }
}
