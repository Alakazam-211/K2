//! `publish`'s verbs that ask only this computer's daemon
//! (`cli/src/connect.sh`): `client-ip` (`cmd_publish_client_ip`), `status
//! <name>` (`_publish_app_status`), and the words `secret`, `secrets` and
//! `env` read before they hand over to `vault grant`, `vault revoke` and
//! `vault service` (`crate::families::vault`).
//!
//! Each reader gets the answer as the shell's `$(…)` holds it and prints
//! what its Python prints; where that Python raises, a Python 3.12
//! traceback (no source lines) and exit 1.

use super::read::{dumps, obj, s};
use super::{Act, Req};
use crate::exit::Exit;
use crate::families::{get, py_iter, py_space, raised, Raise, Shown};
use crate::out::json::{self, Options, Value};
use crate::out::py;
use crate::out::pylib::json::loads;
use crate::out::pylib::ops;
use k2_client::brand::{CLI_NAME as CLI, PRODUCT_NAME as PRODUCT};

/// `/cli/publish/client-ip/list`.
pub const CLIENT_IP_LIST: &str = "/cli/publish/client-ip/list";
/// `/cli/publish/client-ip`.
pub const CLIENT_IP: &str = "/cli/publish/client-ip";

/// Python's `str.strip()`.
pub fn strip(t: &str) -> &str {
    t.trim_matches(py_space)
}

/// `cmd_publish_client_ip`'s words.
pub fn parse_client_ip(rest: &[&str]) -> Act {
    let usage = format!("usage: {CLI} publish client-ip [<label> on|off] [--json]");
    let mut json = false;
    let mut label = String::new();
    let mut state = String::new();
    for w in rest {
        match *w {
            "--json" => json = true,
            f if f.starts_with('-') => {
                return Act::Lines(
                    format!("unknown flag '{f}' \u{2014} {usage}\n"),
                    Exit::Usage,
                )
            }
            a if label.is_empty() => label = a.to_string(),
            a if state.is_empty() => state = a.to_string(),
            a => {
                return Act::Lines(
                    format!("unexpected argument '{a}' \u{2014} {usage}\n"),
                    Exit::Usage,
                )
            }
        }
    }
    if label.is_empty() {
        return Act::ClientIp {
            endpoint: CLIENT_IP_LIST,
            call: Req::Get(Vec::new()),
            json,
        };
    }
    let on = match state.as_str() {
        "on" | "true" => "true",
        "off" | "false" => "false",
        _ => return Act::Lines(format!("{usage}\n"), Exit::Usage),
    };
    // `"on"` is the environment's text, a string.
    let body = dumps(&obj(vec![("label", s(&label)), ("on", s(on))]));
    Act::ClientIp {
        endpoint: CLIENT_IP,
        call: Req::Post(body),
        json,
    }
}

/// `cmd_publish_client_ip`'s Python on `raw`.
pub fn client_ip(raw: &str, as_json: bool) -> Shown {
    let who = format!("{CLI} publish client-ip");
    let d = match loads(raw) {
        Ok(d) => d,
        Err(_) => {
            let r = strip(raw);
            let r = if r.is_empty() { "(empty)" } else { r };
            return Shown::err(
                format!("{who}: no answer from the daemon: {r}\n"),
                Exit::Error,
            );
        }
    };
    let pairs = match &d {
        Value::Object(p) => p,
        _ => return Shown::err(format!("{who}: {raw}\n"), Exit::Usage),
    };
    if let Some(e) = get(pairs, "error").filter(|e| py::truthy(e)) {
        return Shown::err(format!("{who}: {}\n", py::str_of(e)), Exit::Usage);
    }
    if as_json {
        return Shown::out(format!("{}\n", dumps(&d)));
    }
    let mut out = String::new();
    if let Some(label) = get(pairs, "label") {
        let on = match ops::index(&d, "clientIp", 16) {
            Ok(v) => py::truthy(v),
            Err(r) => return raised(out, String::new(), &r),
        };
        out.push_str(&format!(
            "{}: client IP {}\n",
            py::str_of(label),
            if on {
                "on (HTTP/1.1 rewrite, X-K2-Client-IP)"
            } else {
                "off (raw splice)"
            }
        ));
    }
    let labels = get(pairs, "labels").filter(|l| py::truthy(l));
    let list = match labels {
        Some(l) => match ops::join(l, ", ", 18) {
            Ok(t) => t,
            Err(r) => return raised(out, String::new(), &r),
        },
        None => "none (all raw)".to_string(),
    };
    out.push_str(&format!(
        "Nested services with the client-IP rewrite: {list}\n"
    ));
    out.push_str(&format!(
        "Apps: on unless the manifest says host.clientIp: false (not listed here). The App gets X-K2-Client-IP only when {PRODUCT} knows the visitor: {CLI} tunnel status (Visitor IPs).\n"
    ));
    Shown::out(out)
}

/// Python's `int(v)` on a JSON value: the number, or the exception's
/// last line.
pub fn py_int(v: &Value) -> Result<i128, String> {
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Int(d) => Ok(d.parse().unwrap_or(if d.starts_with('-') {
            i128::MIN
        } else {
            i128::MAX
        })),
        Value::Float(f) if f.is_nan() => {
            Err("ValueError: cannot convert float NaN to integer".to_string())
        }
        Value::Float(f) if f.is_infinite() => {
            Err("OverflowError: cannot convert float infinity to integer".to_string())
        }
        Value::Float(f) => Ok(f.trunc() as i128),
        Value::Str(t) => {
            let text = py::str_of(v);
            int_text(&text).ok_or_else(|| {
                format!(
                    "ValueError: invalid literal for int() with base 10: {}",
                    py::repr(&Value::Str(t.clone()))
                )
            })
        }
        other => Err(format!(
            "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py::type_name(other)
        )),
    }
}

/// Python's `int(text)` in base 10 (ASCII digits): spaces around, a sign,
/// one `_` between digits.
pub fn int_text(text: &str) -> Option<i128> {
    let t = strip(text);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let ok = !digits.is_empty()
        && !digits.starts_with('_')
        && !digits.ends_with('_')
        && !digits.contains("__")
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'_');
    if !ok {
        return None;
    }
    let plain: String = digits.chars().filter(|c| *c != '_').collect();
    let n: i128 = plain.parse().unwrap_or(i128::MAX);
    Some(if neg { -n } else { n })
}

/// `_publish_app_status`'s Python on `raw` (`/cli/publish/list`'s answer)
/// for the App `name`.
pub fn app_status(raw: &str, name: &str, as_json: bool) -> Shown {
    let name = strip(name).to_lowercase();
    let d = match loads(raw) {
        Ok(d) => d,
        Err(_) => {
            return Shown::err(
                format!("Error: no usable answer from the {PRODUCT} daemon on this computer\n"),
                Exit::Error,
            )
        }
    };
    let pairs = match &d {
        Value::Object(p) if get(p, "error").is_none() => p,
        _ => {
            let e = match &d {
                Value::Object(p) => get(p, "error").filter(|e| py::truthy(e)),
                _ => None,
            };
            let text = match e {
                Some(e) => py::str_of(e),
                None => raw.chars().take(200).collect(),
            };
            return Shown::err(format!("Error: {text}\n"), Exit::Error);
        }
    };
    let services = match get(pairs, "services").filter(|v| py::truthy(v)) {
        Some(v) => match py_iter(v, 14) {
            Ok(items) => items,
            Err(r) => return raised(String::new(), String::new(), &r),
        },
        None => Vec::new(),
    };
    let svc = services.iter().find_map(|s| match s {
        Value::Object(p) => {
            let n = get(p, "name").map_or_else(String::new, py::str_of);
            (n.to_lowercase() == name).then_some(p)
        }
        _ => None,
    });
    let field = |p: &[(json::Text, Value)], k: &str| get(p, k).cloned().unwrap_or(Value::Null);
    let (out, exit) = match svc {
        None => (
            obj(vec![
                ("name", s(&name)),
                ("status", s("missing")),
                ("exit", Value::Int("1".into())),
                (
                    "suggestion",
                    s(&format!(
                        "no published App named {name} in this workspace: {CLI} publish ps lists them"
                    )),
                ),
            ]),
            1,
        ),
        Some(p) => {
            let st = match get(p, "status").filter(|v| py::truthy(v)) {
                Some(v) => py::str_of(v),
                None => String::new(),
            };
            let mut st_label = st.clone();
            let code = get(p, "lastExitCode").filter(|v| **v != Value::Null);
            let truthy = |k: &str| get(p, k).is_some_and(py::truthy);
            let (exit, sug) = match st.as_str() {
                "running" => {
                    let at = if truthy("url") {
                        format!(" at {}", py::str_of(&field(p, "url")))
                    } else {
                        String::new()
                    };
                    (0, format!("it is up{at}"))
                }
                "starting" => {
                    if let Some(left) = get(p, "startingLeftMs").filter(|v| **v != Value::Null) {
                        let n = match py_int(left) {
                            Ok(n) => n,
                            Err(e) => {
                                return raised(String::new(), String::new(), &Raise::at(28, e))
                            }
                        };
                        let secs = n.div_euclid(1000).max(0);
                        let (m, sec) = (secs / 60, secs % 60);
                        let left_s = if m != 0 && sec != 0 {
                            format!("{m}m{sec:02}s")
                        } else if m != 0 {
                            format!("{m}m")
                        } else {
                            format!("{sec}s")
                        };
                        st_label = format!(
                            "starting (waiting for port {}, {left_s} left)",
                            py::str_of(&field(p, "port"))
                        );
                    }
                    let mut sug = format!(
                        "still starting: run {CLI} publish status {name} again in a few seconds"
                    );
                    if truthy("hostnamePending") {
                        sug.push_str(" (the public URL is published once the port answers)");
                    }
                    (2, sug)
                }
                "stopped" => (1, format!("it is stopped: {CLI} publish start {name}")),
                "unhealthy" => {
                    let target = match get(p, "target").filter(|v| py::truthy(v)) {
                        Some(t) => py::str_of(t),
                        None => "its port".to_string(),
                    };
                    (
                        1,
                        format!("the process runs but nothing answers on its port: check it listens on {target} ({CLI} publish logs {name})"),
                    )
                }
                _ => {
                    let with = code.map_or_else(String::new, |c| {
                        format!(" with code {}", py::str_of(c))
                    });
                    (
                        1,
                        format!("it exited{with}: read {CLI} publish logs {name}, fix it, check the code with {CLI} app validate {name}, then {CLI} publish start {name}"),
                    )
                }
            };
            (
                obj(vec![
                    ("name", s(&name)),
                    ("status", s(&st)),
                    ("desired", field(p, "desired")),
                    ("error", field(p, "error")),
                    ("lastExitCode", field(p, "lastExitCode")),
                    ("url", field(p, "url")),
                    ("suggestion", s(&sug)),
                    ("exit", Value::Int(exit.to_string())),
                    ("probeTimeoutMs", field(p, "probeTimeoutMs")),
                    ("probeTimeoutEffectiveMs", field(p, "probeTimeoutEffectiveMs")),
                    ("startingLeftMs", field(p, "startingLeftMs")),
                    ("hostnamePending", Value::Bool(truthy("hostnamePending"))),
                    ("statusText", s(&st_label)),
                ]),
                exit,
            )
        }
    };
    let exit = match exit {
        0 => Exit::Ok,
        2 => Exit::Usage,
        _ => Exit::Error,
    };
    let text = if as_json {
        let opts = Options {
            indent: Some(2),
            ..Options::default()
        };
        json::to_string(&out, &opts).expect("a value from JSON always prints") + "\n"
    } else {
        let Value::Object(o) = &out else {
            unreachable!("an object")
        };
        let label = get(o, "statusText")
            .filter(|v| py::truthy(v))
            .or_else(|| get(o, "status"))
            .map_or_else(String::new, py::str_of);
        let mut line = format!("{name}: {label}");
        if let Some(e) = get(o, "error").filter(|v| py::truthy(v)) {
            line.push_str(&format!(" ({})", py::str_of(e)));
        }
        let sug = get(o, "suggestion").map_or_else(String::new, py::str_of);
        format!("{line}\n  suggestion: {sug}\n")
    };
    Shown {
        stdout: text,
        stderr: String::new(),
        exit,
    }
}

/// `cmd_publish`'s `secret|secrets)` arm: the vault verb and its words,
/// or what it prints itself.
pub fn parse_secret(rest: &[&str]) -> Act {
    let act = rest.first().copied().unwrap_or("");
    let label = rest.get(1).copied().unwrap_or("");
    let sname = rest.get(2).copied().unwrap_or("");
    if act.is_empty() {
        return Act::Page {
            key: "publish/secret",
            stderr: true,
            exit: Exit::Usage,
        };
    }
    if act == "help" {
        return Act::Page {
            key: "publish/secret",
            stderr: false,
            exit: Exit::Ok,
        };
    }
    let line = |l: String| Act::Lines(format!("{l}\n"), Exit::Usage);
    let tail = || -> Vec<String> { rest.iter().skip(3).map(|w| (*w).to_string()).collect() };
    match act {
        "add" => {
            if label.is_empty() || sname.is_empty() {
                return line(format!("usage: {CLI} publish secret add <service> NAME [--as file|env] [--env-name X] [--scope \u{2026}]"));
            }
            let mut words = vec![
                sname.to_string(),
                "--to".to_string(),
                format!("service:{label}"),
            ];
            words.extend(tail());
            Act::Vault {
                sub: "grant",
                words,
            }
        }
        "remove" | "rm" => {
            if label.is_empty() || sname.is_empty() {
                return line(format!(
                    "usage: {CLI} publish secret remove <service> NAME [--scope \u{2026}]"
                ));
            }
            let mut words = vec![
                sname.to_string(),
                "--from".to_string(),
                format!("service:{label}"),
            ];
            words.extend(tail());
            Act::Vault {
                sub: "revoke",
                words,
            }
        }
        "list" | "ls" => {
            if label.is_empty() {
                return line(format!("usage: {CLI} publish secret list <service>"));
            }
            Act::Vault {
                sub: "service",
                words: vec![label.to_string()],
            }
        }
        _ => line(format!(
            "usage: {CLI} publish secret add|remove|list <service> \u{2026}"
        )),
    }
}

/// `cmd_publish`'s `env)` arm.
pub fn parse_env(rest: &[&str]) -> Act {
    if rest.first().is_none_or(|w| w.is_empty()) {
        return Act::Lines(
            format!("usage: {CLI} publish env <service> [clean|legacy] [--env-file .env.k2]\n"),
            Exit::Usage,
        );
    }
    Act::Vault {
        sub: "service",
        words: rest.iter().map(|w| (*w).to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_reads_as_python_s() {
        assert_eq!(int_text(" 12 "), Some(12));
        assert_eq!(int_text("-1_000"), Some(-1000));
        assert_eq!(int_text("1__0"), None);
        assert_eq!(int_text("1.5"), None);
        assert_eq!(py_int(&Value::Float(-2.7)), Ok(-2));
        assert_eq!(
            py_int(&s("x")),
            Err("ValueError: invalid literal for int() with base 10: 'x'".to_string())
        );
        assert!(py_int(&Value::Array(Vec::new()))
            .unwrap_err()
            .ends_with("not 'list'"));
    }

    #[test]
    fn the_secret_words() {
        assert_eq!(
            parse_secret(&["add", "api", "K", "--as", "file"]),
            Act::Vault {
                sub: "grant",
                words: ["K", "--to", "service:api", "--as", "file"]
                    .map(String::from)
                    .to_vec()
            }
        );
        assert_eq!(
            parse_secret(&["ls", "api", "extra"]),
            Act::Vault {
                sub: "service",
                words: vec!["api".to_string()]
            }
        );
        assert!(matches!(parse_secret(&["add", "api"]), Act::Lines(_, Exit::Usage)));
        assert!(matches!(parse_env(&[""]), Act::Lines(_, Exit::Usage)));
    }

    #[test]
    fn app_status_reads_as_the_shell_s_python() {
        let raw = r#"{"services":[{"name":"Web","status":"starting","startingLeftMs":125000,"port":3000}]}"#;
        let shown = app_status(raw, " web ", false);
        assert_eq!(shown.exit, Exit::Usage);
        assert_eq!(
            shown.stdout,
            format!("web: starting (waiting for port 3000, 2m05s left)\n  suggestion: still starting: run {CLI} publish status web again in a few seconds\n")
        );
    }
}
