//! `k5 root-helper status|update|rollback|install-command`: the CLI
//! forms of the root helper, as the shell CLI's `cmd_root_helper`
//! answers them (`cli/src/root-helper.sh`). Only the command line is
//! ported: each verb is one `_sidecar_send` to the daemon, which owns the
//! helper and decides every request (it refuses `update` and `rollback`
//! for any passport); nothing here touches the helper itself.
//!
//! Each answer goes through `_root_helper_render`: no answer is
//! `<cli> root-helper: daemon unreachable`, exit 1; `--json` prints the
//! body (exit 1 on a status that is not 200); an error is one line,
//! exit 1; else its Python's lines ([`render`]). `install-command` prints
//! the daemon's `installCommand`.
//!
//! The shell answers these itself ([`run`] returns `None`): any `-h` or
//! `--help` (the front's), `--from` as the last word (`shift 2` out of
//! range), a `--from` folder the binary can't resolve as bash's `cd`
//! would, a word that is not UTF-8, a port curl won't take, and a
//! `python3` the binary can't tell.

use super::heartbeat::py::{dget_or, dget_v, has, hashable, iter, or, Py, R};
use super::workspace::register::{self, Verb as RegisterVerb};
use super::{curl_port, s, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::Error;
use k2_client::{cli_name, product_name};
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["root-helper"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

pub const STATUS: &str = "/cli/root-helper/status";
pub const UPDATE: &str = "/cli/root-helper/update";
pub const ROLLBACK: &str = "/cli/root-helper/rollback";

/// What a command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    Help,
    /// A line on stderr, exit 2.
    Usage(String),
    Status { json: bool, events: bool },
    Update { json: bool, body: String },
    Rollback { json: bool },
    InstallCommand { json: bool },
}

/// The words after `root-helper`. `resolve` is `cd "$from" && pwd`
/// (`None`: the shell's).
pub fn parse(words: &[&str], resolve: &dyn Fn(&str) -> Option<String>) -> Option<Form> {
    let c = cli_name!();
    let sub = words.first().copied().unwrap_or("status");
    if matches!(sub, "help" | "--help" | "-h") {
        return Some(Form::Help);
    }
    let rest = words.get(1..).unwrap_or_default();
    let (mut json, mut events, mut matched) = (false, false, false);
    let (mut from, mut version) = (String::new(), String::new());
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--json" => json = true,
            "--events" => events = true,
            "--from" => {
                from = rest.get(i + 1)?.to_string();
                i += 1;
            }
            "--version" => {
                version = rest.get(i + 1).copied().unwrap_or("").to_string();
                if version.is_empty() {
                    return Some(Form::Usage("usage: --version needs x.y.z".into()));
                }
                i += 1;
            }
            "--match-installed" => matched = true,
            "-h" | "--help" => return Some(Form::Help),
            a => {
                return Some(Form::Usage(format!(
                    "usage: unknown argument '{a}' ({c} root-helper --help)"
                )))
            }
        }
        i += 1;
    }
    Some(match sub {
        "status" => Form::Status { json, events },
        "update" => {
            if !version.is_empty() && matched {
                return Some(Form::Usage(
                    "usage: give --version or --match-installed, not both".into(),
                ));
            }
            if !from.is_empty() && !from.starts_with('/') {
                if std::env::var_os("CDPATH").is_some() {
                    return None;
                }
                if !std::path::Path::new(&from).is_dir() {
                    return Some(Form::Usage("--from: no such directory".into()));
                }
                from = resolve(&from)?;
            }
            let mut b: Vec<(Text, Value)> = Vec::new();
            if !from.is_empty() {
                b.push((Text::from("fromDir"), s(&from)));
            }
            if !version.is_empty() {
                b.push((Text::from("version"), s(&version)));
            }
            if matched {
                b.push((Text::from("matchInstalled"), Value::Bool(true)));
            }
            let body = json::to_string(&Value::Object(b), &Options::default()).ok()?;
            Form::Update { json, body }
        }
        "rollback" => Form::Rollback { json },
        "install-command" => Form::InstallCommand { json },
        other => Form::Usage(format!(
            "usage: unknown root-helper verb '{other}' (status|update|rollback|install-command)"
        )),
    })
}

/// `k5 root-helper ...`: `args` is the whole command line after
/// `argv[0]`. `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let pwd = daemon::logical_cwd(std::env::var_os("PWD")).ok()?;
    let pwd = pwd.to_str()?.to_string();
    let cdpath = std::env::var_os("CDPATH").is_some();
    let resolve = |w: &str| register::path(RegisterVerb::Open, w, &pwd, cdpath);
    let form = parse(&words, &resolve)?;
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
    Some(run_with(args, form, settings))
}

fn run_with(args: &[OsString], form: Form, mut settings: Settings) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    match &form {
        Form::Help => {
            let text = crate::text::help("root-helper").unwrap_or_default();
            return Shown::out(text).print();
        }
        Form::Usage(line) => {
            out::stderr_line(line);
            return Exit::Usage;
        }
        _ => {}
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let mut send = |post: bool, path: &str, payload: &str| -> Result<Option<(String, Vec<u8>)>, Exit> {
        let passport = client.passport().is_some();
        let r = if post {
            client.sidecar_post(path, payload)
        } else {
            client.sidecar_get(path, payload)
        };
        match r {
            Ok(r) => {
                closing.answer(&r, passport);
                Ok(Some((format!("{:03}", r.status), super::without_nuls(&r.body))))
            }
            Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
                out::stderr_line(&e.to_string());
                Err(Exit::Error)
            }
            Err(_) => Ok(None),
        }
    };
    let exit = match form {
        Form::Status { json, events } => {
            let q = if events { "refresh=1&events=1" } else { "refresh=1" };
            match send(false, STATUS, q) {
                Ok(a) => shown(json, a),
                Err(e) => e,
            }
        }
        Form::Update { json, body } => match send(true, UPDATE, &body) {
            Ok(a) => shown(json, a),
            Err(e) => e,
        },
        Form::Rollback { json } => match send(true, ROLLBACK, "{}") {
            Ok(a) => shown(json, a),
            Err(e) => e,
        },
        Form::InstallCommand { json } => match send(false, STATUS, "") {
            Ok(a) if json => shown(true, a),
            Ok(a) => install_command(a),
            Err(e) => e,
        },
        Form::Help | Form::Usage(_) => Exit::Ok,
    };
    closing.flush(&mut client, exit);
    exit
}

fn unreachable() -> Exit {
    out::stderr_line(&format!("{} root-helper: daemon unreachable", cli_name!()));
    Exit::Error
}

/// `_root_helper_render <as_json> <raw>`.
fn shown(json: bool, answer: Option<(String, Vec<u8>)>) -> Exit {
    let Some((code, body)) = answer.filter(|(c, _)| c != "000") else {
        return unreachable();
    };
    if json {
        let mut b = body;
        b.push(b'\n');
        if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &b) {
            return out::after_stdout_error(&e);
        }
        return if code == "200" { Exit::Ok } else { Exit::Error };
    }
    let raw = String::from_utf8_lossy(&body).into_owned();
    if let Some(exit) = fail_if_error(&raw, &code) {
        return exit;
    }
    render(&raw).print()
}

/// `install-command`'s lines.
fn install_command(answer: Option<(String, Vec<u8>)>) -> Exit {
    let Some((code, body)) = answer.filter(|(c, _)| c != "000") else {
        return unreachable();
    };
    let raw = String::from_utf8_lossy(&body).into_owned();
    if let Some(exit) = fail_if_error(&raw, &code) {
        return exit;
    }
    let d = json::parse(&raw).unwrap_or(Value::Null);
    let v = dget_v(&d, "installCommand").unwrap_or(Value::Null);
    let mut cmd = if py::truthy(&v) { py::str_of(&v) } else { String::new() };
    while cmd.ends_with('\n') {
        cmd.pop();
    }
    if cmd.is_empty() {
        out::stderr_line(&format!(
            "{} root-helper: error: the daemon gave no install command",
            cli_name!()
        ));
        return Exit::Error;
    }
    match out::stdout(&format!("{cmd}\n")) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// `_root_helper_fail_if_error`'s line: `Some(exit)` when it printed one.
fn fail_if_error(raw: &str, code: &str) -> Option<Exit> {
    let mut line = error_line(raw, code);
    while line.ends_with('\n') {
        line.pop();
    }
    if line.is_empty() {
        return None;
    }
    out::stderr_line(&format!("{} root-helper: error: {line}", cli_name!()));
    Some(Exit::Error)
}

/// The Python of `_root_helper_fail_if_error`: the message, or empty.
pub fn error_line(raw: &str, code: &str) -> String {
    let d = json::parse(raw).ok();
    let e = match &d {
        Some(v @ Value::Object(_)) => dget_v(v, "error").unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let strip = |t: &str| -> String {
        t.trim_matches(super::py_space).chars().take(200).collect()
    };
    match &e {
        Value::Object(_) => {
            let c = or(dget_v(&e, "code").unwrap_or(Value::Null), s("error"));
            let mut m = py::str_of(&c);
            let hint = dget_v(&e, "hint").unwrap_or(Value::Null);
            if py::truthy(&hint) {
                m.push_str(&format!(": {}", py::str_of(&hint)));
            }
            m
        }
        Value::Null if code != "200" => {
            let t = strip(raw);
            if t.is_empty() {
                format!("HTTP {code}")
            } else {
                format!("HTTP {code}: {t}")
            }
        }
        Value::Null if !matches!(d, Some(Value::Object(_))) => {
            let t = strip(raw);
            format!(
                "unexpected answer from the daemon: {}",
                if t.is_empty() { "(empty)".to_string() } else { t }
            )
        }
        Value::Null => String::new(),
        other => py::str_of(other),
    }
}

fn st(v: &Value) -> String {
    py::str_of(v)
}

/// `print(a, b, ...)`.
fn words(parts: &[String]) -> String {
    parts.join(" ")
}

/// `{...}.get(key, default)` where the key must be hashable.
fn pick(table: &[(&str, &str)], key: &Value, default: String) -> R<String> {
    hashable(key)?;
    Ok(table
        .iter()
        .find(|(k, _)| *key == s(k))
        .map_or(default, |(_, v)| v.to_string()))
}

/// `time.strftime("%Y-%m-%d %H:%M UTC", time.gmtime(at))`.
fn gm(at: &Value) -> R<String> {
    let secs = match at {
        Value::Int(d) => d
            .parse::<i64>()
            .map_err(|_| "OverflowError: timestamp out of range for platform time_t".to_string())?,
        Value::Float(f) if f.is_finite() => f.floor() as i64,
        Value::Bool(b) => i64::from(*b),
        other => {
            return Err(format!(
                "TypeError: '{}' object cannot be interpreted as an integer",
                py::type_name(other)
            ))
        }
    };
    let (y, m, d) = super::heartbeat::when::civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    Ok(format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60
    ))
}

/// `_root_helper_render`'s Python on a 200 answer with no error.
pub fn render(raw: &str) -> Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let mut d = super::heartbeat::py::loads(raw)?;
        let h = dget_v(&d, "heal")?;
        if h != Value::Null && py::truthy(&dget_v(&d, "target")?) {
            p.out(format!(
                "helper for : v{}",
                st(&super::heartbeat::py::index(&d, "target")?)
            ));
        }
        for w in iter(&or(dget_v(&d, "warnings")?, Value::Array(Vec::new())))? {
            p.out(format!("warning    : {}", st(&w)));
        }
        if h != Value::Null {
            let Value::Object(pairs) = &h else {
                return Err(format!(
                    "AttributeError: '{}' object has no attribute 'get'",
                    py::type_name(&h)
                ));
            };
            let rest: Vec<(Text, Value)> = pairs
                .iter()
                .filter(|(k, _)| *k != Text::from("result"))
                .cloned()
                .collect();
            let tail = if rest.is_empty() {
                String::new()
            } else {
                st(&Value::Object(rest))
            };
            p.out(words(&[
                "heal       :".into(),
                st(&dget_v(&h, "result")?),
                tail,
            ]));
            let dr = or(dget_v(&d, "dropin")?, Value::Object(Vec::new()));
            let res = dget_v(&dr, "result").or_else(|e| {
                if py::truthy(&dr) {
                    Err(e)
                } else {
                    Ok(Value::Null)
                }
            })?;
            if py::truthy(&dr) && !is_any(&res, &["not_applicable", "opted_out"]) {
                let word = pick(
                    &[
                        ("installed", "written now (through the helper; systemd reloaded)"),
                        ("current", "current"),
                        ("stale", "still the older helper drop-in"),
                        ("failed", "not written"),
                    ],
                    &res,
                    st(&res),
                )?;
                let unit = or(dget_v(&dr, "unit")?, s("unit"));
                let mut line = format!("{} {word}", st(&unit));
                let reason = dget_v(&dr, "reason")?;
                if py::truthy(&reason) {
                    line.push_str(&format!(": {}", st(&reason)));
                }
                p.out(format!("unit dropin: {line}"));
                if py::truthy(&dget_v(&dr, "restartNeeded")?) {
                    p.out(format!(
                        "restart    : once, when convenient: {}",
                        st(&dget_v(&dr, "restart")?)
                    ));
                    p.out(format!("             ({})", st(&dget_v(&dr, "why")?)));
                }
            }
            d = or(dget_v(&d, "status")?, Value::Object(Vec::new()));
        }
        if has(&d, "state")? {
            p.out(format!(
                "state      : {} ({})",
                st(&dget_v(&d, "state")?),
                st(&dget_v(&d, "path")?)
            ));
            p.out(format!(
                "version    : {}  protocol {}  daemon {}",
                st(&dget_v(&d, "version")?),
                st(&dget_v(&d, "protocol")?),
                st(&dget_v(&d, "daemonVersion")?)
            ));
            let lg = or(dget_v(&d, "legacy")?, Value::Object(Vec::new()));
            let pg = if py::truthy(&dget_v(&lg, "pg")?) { "present" } else { "absent" };
            p.out(format!(
                "legacy     : mail {} \u{b7} pg {pg}",
                st(&dget_v(&lg, "mail")?)
            ));
            p.out(format!("setup      : {}", st(&dget_v(&d, "rootSetup")?)));
            let ah = or(dget_v(&d, "autoHeal")?, Value::Object(Vec::new()));
            let last = dget_v(&ah, "lastResult")?;
            if py::truthy(&last) {
                p.out(format!(
                    "auto-heal  : {} {}",
                    st(&dget_v(&last, "result")?),
                    st(&dget_or(&last, "reason", s(""))?)
                ));
            } else if py::truthy(&dget_v(&ah, "superseded")?) {
                let sp = dget_v(&ah, "superseded")?;
                let at = if py::truthy(&dget_v(&sp, "at")?) {
                    gm(&dget_v(&sp, "at")?)?
                } else {
                    "?".into()
                };
                p.out(format!(
                    "auto-heal  : earlier attempt ({at}) said {}: {}; {}",
                    st(&dget_v(&sp, "result")?),
                    st(&dget_or(&sp, "reason", s(""))?),
                    st(&dget_or(&sp, "why", s(""))?)
                ));
            }
            let c = or(dget_v(&d, "cli")?, Value::Object(Vec::new()));
            if py::truthy(&c) {
                let state = dget_v(&c, "state")?;
                let version = dget_v(&c, "version")?;
                let v = if py::truthy(&version) {
                    format!("v{}", st(&version))
                } else {
                    String::new()
                };
                let mut line = format!("{} {v}", st(&dget_v(&c, "path")?));
                if super::heartbeat::py::is(&state, "stale") {
                    let owned = if py::truthy(&dget_v(&c, "rootOwned")?) {
                        "root-owned"
                    } else {
                        "not root-owned"
                    };
                    line.push_str(&format!(
                        " ({owned}) \u{2014} older than the daemon (v{}); auto-heal refreshes it through the root helper",
                        st(&dget_v(&c, "daemonCliVersion")?)
                    ));
                } else if super::heartbeat::py::is(&state, "current") {
                    line.push_str(" \u{2014} current");
                } else {
                    let not_k2 = format!("not the {} CLI", product_name!());
                    let word = pick(
                        &[
                            ("absent", "not installed there"),
                            ("symlink", "a symlink (the app or a package owns it)"),
                            ("not_k2", &not_k2),
                        ],
                        &state,
                        st(&state),
                    )?;
                    line.push_str(&format!(" \u{2014} {word}"));
                }
                p.out(format!("cli        : {}", line.replace("  ", " ")));
                let lr = or(dget_v(&c, "lastResult")?, Value::Object(Vec::new()));
                let lres = if py::truthy(&lr) {
                    dget_v(&lr, "result")?
                } else {
                    Value::Null
                };
                if py::truthy(&lr) && !is_any(&lres, &["current", "not_managed"]) {
                    let reason = dget_v(&lr, "reason")?;
                    let why = if py::truthy(&reason) {
                        st(&reason)
                    } else if py::truthy(&dget_v(&lr, "to")?) {
                        format!(
                            "{} -> {}",
                            st(&dget_v(&lr, "from")?),
                            st(&dget_v(&lr, "to")?)
                        )
                    } else {
                        String::new()
                    };
                    p.out(words(&["cli refresh:".into(), st(&lres), why]));
                }
            }
            if py::truthy(&dget_v(&d, "plainSudoRestartUsed")?) {
                p.out("note       : a Stalwart restart used plain sudo systemctl (no helper)");
            }
            if super::heartbeat::py::is(&dget_v(&d, "rootSetup")?, "copy-command") {
                p.out(format!(
                    "one root step (an admin SSH session, never a {} terminal):",
                    product_name!()
                ));
                p.out(format!("  {}", st(&dget_v(&d, "installCommand")?)));
            }
            let le = or(dget_v(&d, "lastEvents")?, Value::Object(Vec::new()));
            let ev = or(dget_v(&le, "events")?, Value::Array(Vec::new()));
            let items = iter(&ev)?;
            let start = items.len().saturating_sub(20);
            for e in &items[start..] {
                let x = or(dget_v(e, "event")?, Value::Object(Vec::new()));
                let caller = or(dget_v(&x, "caller")?, Value::Object(Vec::new()));
                p.out(words(&[
                    "  event".into(),
                    st(&dget_v(e, "at")?),
                    st(&dget_v(&x, "verb")?),
                    st(&dget_v(&x, "result")?),
                    st(&dget_v(&caller, "pid")?),
                ]));
            }
        }
        Ok(Exit::Ok)
    })();
    p.raised(r, 4)
}

/// `x in (...)` against strings.
fn is_any(x: &Value, words: &[&str]) -> bool {
    words.iter().any(|w| super::heartbeat::py::is(x, w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(w: &str) -> Option<String> {
        Some(w.to_string())
    }

    #[test]
    fn the_forms() {
        assert_eq!(
            parse(&[], &same),
            Some(Form::Status {
                json: false,
                events: false
            })
        );
        assert_eq!(parse(&["help"], &same), Some(Form::Help));
        assert_eq!(parse(&["update", "--from"], &same), None);
        assert_eq!(
            parse(&["update", "--version", "1.2.3", "--match-installed"], &same),
            Some(Form::Usage(
                "usage: give --version or --match-installed, not both".into()
            ))
        );
        assert_eq!(
            parse(&["update", "--from", "/opt/h", "--match-installed"], &same),
            Some(Form::Update {
                json: false,
                body: r#"{"fromDir": "/opt/h", "matchInstalled": true}"#.into()
            })
        );
        assert!(matches!(parse(&["nope"], &same), Some(Form::Usage(_))));
    }

    #[test]
    fn the_error_line() {
        assert_eq!(error_line(r#"{"error":"route not found"}"#, "404"), "route not found");
        assert_eq!(error_line("  ", "502"), "HTTP 502");
        assert_eq!(error_line("[]", "200"), "unexpected answer from the daemon: []");
        assert_eq!(error_line(r#"{"ok":true}"#, "200"), "");
        assert_eq!(gm(&Value::Int("86460".into())).unwrap(), "1970-01-02 00:01 UTC");
    }
}
