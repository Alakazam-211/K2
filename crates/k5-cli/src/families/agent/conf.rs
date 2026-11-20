//! `_agent_conf_fetch`, and the two verbs that only print the conf:
//! `cmd_agent_conf` and `cmd_agent_get` (`cli/src/agent.sh`).

use super::{abs_user, at, cli, cut, dumps, err_json, head, obj, product, raised, s, str_or};
use crate::families::connections::held;
use crate::families::{py_get, py_iter, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::pylib::{json as pyjson, ops};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::client::{Call, Channel, Error, LOCAL_TRANSPORT_DENIED, TMPDIR_UNWRITABLE};
use crate::transport::encode::quote;
use crate::transport::Client;

/// The fetch block's lines (`<stdin>`).
mod line {
    pub const ERROR: u32 = 10;
    pub const EXIT4: u32 = 15;
    pub const CONF_PERSONA: u32 = 14;
    pub const CONF_CONNS: u32 = 31;
    pub const CONF_PROJECTS: u32 = 36;
    pub const CONF_LIVE: u32 = 40;
    pub const CONF_UPTIME: u32 = 44;
    pub const CONF_CHARTER: u32 = 66;
    pub const GET_PERSONA: u32 = 13;
    pub const GET_PROJECTS: u32 = 38;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfArgs {
    pub token: String,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetArgs {
    pub name: String,
    pub key: String,
    pub json: bool,
}

/// `cmd_agent_conf`'s words.
pub fn parse_conf(rest: &[&str]) -> Result<ConfArgs, String> {
    let usage = format!("usage: {} agent conf [<name>] [--json]", cli());
    let mut a = ConfArgs {
        token: String::new(),
        json: false,
    };
    for w in rest {
        if *w == "--json" {
            a.json = true;
        } else if w.starts_with('-') {
            return Err(format!("unknown flag '{w}' \u{2014} {usage}"));
        } else if a.token.is_empty() {
            a.token = (*w).to_string();
        } else {
            return Err(format!("unexpected extra argument '{w}' \u{2014} {usage}"));
        }
    }
    Ok(a)
}

/// The keys `agent get` takes.
const KEYS: &[&str] = &["mode", "enabled", "name", "path", "persona", "projects"];

/// `cmd_agent_get`'s words.
pub fn parse_get(rest: &[&str]) -> Result<GetArgs, String> {
    let cli = cli();
    let mut a = GetArgs {
        name: String::new(),
        key: String::new(),
        json: false,
    };
    for w in rest {
        if *w == "--json" {
            a.json = true;
        } else if w.starts_with('-') {
            return Err(format!(
                "unknown flag '{w}' \u{2014} usage: {cli} agent get <name> <key> [--json]"
            ));
        } else if a.name.is_empty() {
            a.name = (*w).to_string();
        } else if a.key.is_empty() {
            a.key = (*w).to_string();
        } else {
            return Err(format!(
                "unexpected extra argument '{w}' \u{2014} usage: {cli} agent get <name> <key>"
            ));
        }
    }
    if a.name.is_empty() || a.key.is_empty() {
        return Err(format!(
            "usage: {cli} agent get <name> <key> \u{2014} keys: mode, enabled, name, path, persona"
        ));
    }
    if !KEYS.contains(&a.key.as_str()) {
        return Err(format!(
            "unknown key '{}' \u{2014} valid keys: mode, enabled, name, path, persona, projects",
            a.key
        ));
    }
    Ok(a)
}

/// The `daemon_unreachable` line of `_agent_conf_fetch` and `agent list`.
pub fn unreachable_line() -> String {
    err_json(
        "daemon_unreachable",
        &format!(
            "no response from the {} daemon (is it running?)",
            product()
        ),
    )
}

/// What `$(…)` held of a `cli_request` answer (empty: `resp=""`).
/// `denied`: the call ended with the local-refusal code (75 or 76).
pub struct Held {
    pub raw: Vec<u8>,
    pub denied: bool,
}

/// `resp=$(cli_request <endpoint> <args>)`, over TCP: what it held, or
/// the exit after the no-answer line.
pub fn request(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    args: &[&str],
) -> Result<Held, Exit> {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, Call::Request(args)) {
        Ok(r) => {
            closing.answer(&r, passport);
            Ok(Held {
                raw: held(&r.body),
                denied: false,
            })
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Err(Exit::Error)
        }
        Err(e) => Ok(Held {
            raw: Vec::new(),
            denied: matches!(e.curl_code(), LOCAL_TRANSPORT_DENIED | TMPDIR_UNWRITABLE),
        }),
    }
}

/// `_agent_conf_fetch <token>`: the conf, or the exit after its lines.
pub fn fetch(client: &mut Client, closing: &mut Closing, token: &str) -> Result<Value, Exit> {
    let pwd;
    let token = if token.is_empty() {
        pwd = crate::daemon::logical_cwd(std::env::var_os("PWD"))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        pwd.as_str()
    } else {
        token
    };
    let q = if token.contains('/') {
        abs_user(token)
    } else {
        token.to_string()
    };
    let arg = format!("q={}", quote(&q));
    // `conf=$(_agent_conf_fetch …)` holds `resp=$(cli_request …)`: with no
    // answer, its line, then the fetch goes on with an empty answer before
    // the main shell's trap ends the command.
    let held = match request(client, closing, super::CONF, &[&arg]) {
        Ok(h) => h,
        Err(_) => Held {
            raw: Vec::new(),
            denied: false,
        },
    };
    if held.raw.is_empty() {
        out::stderr_line(&unreachable_line());
        return Err(Exit::Error);
    }
    reader(&String::from_utf8_lossy(&held.raw)).map_err(|shown| shown.print())
}

/// `_agent_conf_fetch`'s Python on the answer `raw`.
pub fn reader(raw: &str) -> Result<Value, Shown> {
    let Ok(d) = pyjson::loads(raw) else {
        return Err(Shown::err(
            err_json(
                "bad_response",
                &format!(
                    "daemon returned an unparseable response: {}",
                    py::repr(&s(&cut(raw, 200)))
                ),
            ) + "\n",
            Exit::Error,
        ));
    };
    let err = match py_get(&d, "error", line::ERROR) {
        Ok(e) => e.cloned().unwrap_or(Value::Null),
        Err(r) => return Err(raised(String::new(), String::new(), &r)),
    };
    if err != Value::Null {
        let err = match err {
            Value::Str(_) => obj(vec![("code", s("error")), ("hint", err)]),
            other => other,
        };
        let line = dumps(&obj(vec![("error", err.clone())])) + "\n";
        return Err(match py_get(&err, "code", line::EXIT4) {
            Ok(code) => {
                let found = code.is_some_and(|c| {
                    ops::py_eq(c, &s("not_found")) || ops::py_eq(c, &s("ambiguous"))
                });
                Shown::err(line, if found { Exit::NotFound } else { Exit::Error })
            }
            Err(r) => raised(String::new(), line, &r),
        });
    }
    if !at(&d, "ok").is_some_and(py::truthy) {
        return Err(Shown::err(
            err_json(
                "error",
                &format!(
                    "unexpected daemon response: {}",
                    py::repr(&s(&cut(raw, 200)))
                ),
            ) + "\n",
            Exit::Error,
        ));
    }
    Ok(d)
}

/// `cmd_agent_conf`.
pub fn conf(client: &mut Client, closing: &mut Closing, a: &ConfArgs) -> Exit {
    let d = match fetch(client, closing, &a.token) {
        Ok(d) => d,
        Err(exit) => return exit,
    };
    if a.json {
        return Shown::out(dumps(&d) + "\n").print();
    }
    conf_printer(&d, now()).print()
}

/// `time.time()`.
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The conf printer's `ago(ts)`.
fn ago(secs: f64) -> String {
    let s = secs.trunc() as i64;
    if s < 60 {
        "just now".to_string()
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 172_800 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86400)
    }
}

/// The persona's `" (<n> lines, updated <ago>)"`, or nothing when the
/// file can't be read (`except OSError`).
fn persona_detail(path: &str, now: f64) -> String {
    let Ok(meta) = std::fs::metadata(path) else {
        return String::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64());
    format!(
        " ({} lines, updated {})",
        count_lines(&bytes),
        ago(now - mtime)
    )
}

/// `sum(1 for _ in open(p, encoding="utf-8", errors="replace"))`: the
/// lines as Python's text mode splits them (`\n`, `\r\n` and `\r`).
fn count_lines(bytes: &[u8]) -> usize {
    let text = String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let ends = text.matches('\n').count();
    if text.is_empty() || text.ends_with('\n') {
        ends
    } else {
        ends + 1
    }
}

/// What `os.stat(persona)` and `persona + detail` raise for a persona that
/// is not a string.
fn not_a_path(v: &Value, line: u32) -> Raise {
    match v {
        Value::Int(_) | Value::Bool(_) => Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                py::type_name(v)
            ),
        ),
        other => Raise::at(
            line,
            format!(
                "TypeError: stat: path should be string, bytes, os.PathLike or integer, not {}",
                py::type_name(other)
            ),
        ),
    }
}

/// `int(x)` of a number, or what it raises.
fn int_of(v: &Value, line: u32) -> Result<i128, Raise> {
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Int(d) => d.parse::<i128>().map_err(|_| {
            Raise::at(
                line,
                "OverflowError: the uptime is too large for this printer",
            )
        }),
        Value::Float(f) if f.is_nan() => Err(Raise::at(
            line,
            "ValueError: cannot convert float NaN to integer",
        )),
        Value::Float(f) if f.is_infinite() => Err(Raise::at(
            line,
            "OverflowError: cannot convert float infinity to integer",
        )),
        Value::Float(f) => Ok(f.trunc() as i128),
        _ => Ok(0),
    }
}

/// `cmd_agent_conf`'s printer.
pub fn conf_printer(d: &Value, now: f64) -> Shown {
    let mut stdout = String::new();
    match conf_lines(d, now, &mut stdout) {
        Ok(()) => Shown::out(stdout),
        Err(r) => raised(stdout, String::new(), &r),
    }
}

fn conf_lines(d: &Value, now: f64, out: &mut String) -> Result<(), Raise> {
    let persona_line = match at(d, "personaPath") {
        Some(p) if py::truthy(p) => match p {
            Value::Str(t) => {
                let path = py::str_of(&Value::Str(t.clone()));
                let detail = persona_detail(&path, now);
                path + &detail
            }
            other => return Err(not_a_path(other, line::CONF_PERSONA)),
        },
        _ => "(none)".to_string(),
    };
    let conns = or_list(at(d, "connections"));
    let mut bits = Vec::new();
    for c in py_iter(&conns, line::CONF_CONNS)? {
        let peer = str_or(py_get(&c, "peer", line::CONF_CONNS)?, "?");
        let remote = py_get(&c, "remote", line::CONF_CONNS)?.is_some_and(py::truthy);
        bits.push(format!(
            "{peer} ({})",
            if remote { "remote" } else { "bidirectional" }
        ));
    }
    let connects = none_if_empty(bits.join(", "));
    let mut bits = Vec::new();
    for p in py_iter(&or_list(at(d, "projects")), line::CONF_PROJECTS)? {
        let name = str_or(py_get(&p, "name", line::CONF_PROJECTS)?, "?");
        let poc = py_get(&p, "isPoc", line::CONF_PROJECTS)?.is_some_and(py::truthy);
        bits.push(format!("{name}{}", if poc { " (PoC)" } else { "" }));
    }
    let projects = none_if_empty(bits.join(", "));
    let live = match at(d, "live") {
        Some(v) if py::truthy(v) => v.clone(),
        _ => obj(vec![]),
    };
    let live_line = if py_get(&live, "active", line::CONF_LIVE)?.is_some_and(py::truthy) {
        let id = py_get(&live, "sessionId", line::CONF_LIVE)?
            .cloned()
            .unwrap_or_else(|| s(""));
        let mut l = format!("yes \u{2014} session {}", head(&id, 8));
        if let Some(up @ (Value::Int(_) | Value::Float(_) | Value::Bool(_))) =
            py_get(&live, "uptimeSec", line::CONF_UPTIME)?
        {
            let n = int_of(up, line::CONF_UPTIME)?;
            let (h, m) = (n.div_euclid(3600), n.rem_euclid(3600).div_euclid(60));
            if h != 0 {
                l.push_str(&format!(" ({h}h {m}m)"));
            } else {
                l.push_str(&format!(" ({m}m)"));
            }
        }
        l
    } else {
        "no".to_string()
    };
    let enabled = at(d, "enabled").is_some_and(py::truthy);
    for (label, value) in [
        ("agent    ", str_or(at(d, "name"), "?")),
        ("path     ", str_or(at(d, "path"), "?")),
        ("mode     ", str_or(at(d, "mode"), "?")),
        ("enabled  ", (if enabled { "true" } else { "false" }).to_string()),
        ("persona  ", persona_line),
        ("connects ", connects),
        ("projects ", projects),
        ("live     ", live_line),
    ] {
        out.push_str(&format!("{label}: {value}\n"));
    }
    match at(d, "charterProblems") {
        None | Some(Value::Null) => {}
        Some(p) if !py::truthy(p) => out.push_str("charter  : ok\n"),
        Some(p) => {
            let mut bits = Vec::new();
            for pr in py_iter(p, line::CONF_CHARTER)? {
                let problem = py_get(&pr, "problem", line::CONF_CHARTER)?;
                let file = str_or(py_get(&pr, "file", line::CONF_CHARTER)?, "None");
                if problem.is_some_and(|v| ops::py_eq(v, &s("dangling"))) {
                    let target = str_or(py_get(&pr, "target", line::CONF_CHARTER)?, "None");
                    bits.push(format!("{file} dangling \u{2192} {target}"));
                } else {
                    bits.push(format!("{file} missing"));
                }
            }
            out.push_str(&format!(
                "charter  : PROBLEM \u{2014} {} (fix: cd {} && {} agent context regen)\n",
                bits.join("; "),
                str_or(at(d, "path"), "."),
                cli()
            ));
        }
    }
    Ok(())
}

/// `x or []`.
fn or_list(v: Option<&Value>) -> Value {
    match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => Value::Array(Vec::new()),
    }
}

/// `", ".join(…) or "(none)"`.
fn none_if_empty(joined: String) -> String {
    if joined.is_empty() {
        "(none)".to_string()
    } else {
        joined
    }
}

/// `cmd_agent_get`.
pub fn get(client: &mut Client, closing: &mut Closing, a: &GetArgs) -> Exit {
    let d = match fetch(client, closing, &a.name) {
        Ok(d) => d,
        Err(exit) => return exit,
    };
    get_printer(&d, &a.key, a.json).print()
}

/// `cmd_agent_get`'s Python on the conf `d`.
pub fn get_printer(d: &Value, key: &str, as_json: bool) -> Shown {
    let value = match key {
        "persona" => {
            let p = at(d, "personaPath").filter(|p| py::truthy(p));
            let Some(p) = p else {
                let name = str_or(at(d, "name"), "None");
                return Shown::err(
                    err_json(
                        "no_persona",
                        &format!(
                            "{name} has no persona file yet \u{2014} set one with: {} agent set {name} --persona <file.md>",
                            cli()
                        ),
                    ) + "\n",
                    Exit::Error,
                );
            };
            let Value::Str(t) = p else {
                return raised(
                    String::new(),
                    String::new(),
                    &Raise::at(
                        line::GET_PERSONA,
                        format!(
                            "TypeError: expected str, bytes or os.PathLike object, not {}",
                            py::type_name(p)
                        ),
                    ),
                );
            };
            let path = py::str_of(&Value::Str(t.clone()));
            match std::fs::read(&path) {
                Ok(bytes) => s(&String::from_utf8_lossy(&bytes)
                    .replace("\r\n", "\n")
                    .replace('\r', "\n")),
                Err(e) => {
                    let f = pyjson::os_error(&e, &path);
                    return Shown::err(
                        err_json("persona_unreadable", &f.message) + "\n",
                        Exit::Error,
                    );
                }
            }
        }
        "projects" => or_list(at(d, "projects")),
        other => at(d, other).cloned().unwrap_or(Value::Null),
    };
    if as_json {
        return Shown::out(
            dumps(&obj(vec![
                ("ok", Value::Bool(true)),
                ("key", s(key)),
                ("value", value),
            ])) + "\n",
        );
    }
    match (key, &value) {
        ("persona", v) => Shown::out(py::str_of(v)),
        ("projects", v) => {
            let mut stdout = String::new();
            let items = match py_iter(v, line::GET_PROJECTS) {
                Ok(items) => items,
                Err(r) => return raised(stdout, String::new(), &r),
            };
            for p in items {
                match py_get(&p, "name", line::GET_PROJECTS) {
                    Ok(name) => {
                        stdout.push_str(&str_or(name, "?"));
                        stdout.push('\n');
                    }
                    Err(r) => return raised(stdout, String::new(), &r),
                }
            }
            Shown::out(stdout)
        }
        (_, Value::Bool(true)) => Shown::out("true\n"),
        (_, Value::Bool(false)) => Shown::out("false\n"),
        (_, v) => Shown::out(py::str_of(v) + "\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::out::json::parse;

    #[test]
    fn the_lines_python_counts() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a"), 1);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b"a\r\nb\rc"), 3);
    }

    #[test]
    fn ago_as_the_printer_says_it() {
        assert_eq!(ago(-5.0), "just now");
        assert_eq!(ago(59.9), "just now");
        assert_eq!(ago(61.0), "1m ago");
        assert_eq!(ago(7200.0), "2h ago");
        assert_eq!(ago(172_800.0), "2d ago");
    }

    #[test]
    fn a_not_found_is_exit_4() {
        let r = reader(r#"{"error":{"code":"not_found","hint":"no agent 'x'"}}"#).unwrap_err();
        assert_eq!(r.exit, Exit::NotFound);
        assert_eq!(
            r.stderr,
            "{\"error\": {\"code\": \"not_found\", \"hint\": \"no agent 'x'\"}}\n"
        );
        let r = reader(r#"{"error":"boom"}"#).unwrap_err();
        assert_eq!(r.exit, Exit::Error);
        let r = reader("[1]").unwrap_err();
        assert!(r.stderr.ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
    }

    #[test]
    fn the_conf_lines() {
        let d = parse(
            r#"{"ok":true,"name":"qa","path":"/srv/qa","mode":"agent","enabled":true,
            "connections":[{"peer":"ops"},{"peer":"far","remote":true}],
            "projects":[{"name":"P","isPoc":true}],
            "live":{"active":true,"sessionId":"0123456789","uptimeSec":3725.5},
            "charterProblems":[{"problem":"dangling","file":"CLAUDE.md","target":"AGENTS.md"},{"file":"x"}]}"#,
        )
        .unwrap();
        let shown = conf_printer(&d, 0.0);
        assert_eq!(
            shown.stdout,
            format!(
                "agent    : qa\npath     : /srv/qa\nmode     : agent\nenabled  : true\npersona  : (none)\nconnects : ops (bidirectional), far (remote)\nprojects : P (PoC)\nlive     : yes \u{2014} session 01234567 (1h 2m)\ncharter  : PROBLEM \u{2014} CLAUDE.md dangling \u{2192} AGENTS.md; x missing (fix: cd /srv/qa && {} agent context regen)\n",
                cli()
            )
        );
    }

    #[test]
    fn get_prints_python_values() {
        let d = parse(r#"{"ok":true,"name":"qa","enabled":false,"mode":null,"projects":[{"name":"a"},{}]}"#).unwrap();
        assert_eq!(get_printer(&d, "enabled", false).stdout, "false\n");
        assert_eq!(get_printer(&d, "mode", false).stdout, "None\n");
        assert_eq!(get_printer(&d, "projects", false).stdout, "a\n?\n");
        assert_eq!(
            get_printer(&d, "enabled", true).stdout,
            "{\"ok\": true, \"key\": \"enabled\", \"value\": false}\n"
        );
        let e = get_printer(&d, "persona", false);
        assert_eq!(e.exit, Exit::Error);
        assert!(e.stderr.contains("no_persona"));
    }
}
