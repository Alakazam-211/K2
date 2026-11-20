//! How an App's people sign in, as the shell CLI answers it: `app auth`
//! (`cmd_app_auth`, `cli/src/app-auth.sh`: the sign-in door's state, `door
//! on|off` and `origins`) and `app login settings` (`cmd_app_invite_settings`,
//! `cli/src/app-people.sh`: invite and reset links, the App's own mail
//! and its device cap). Each is the shell's word loop, one request, then
//! `--json` as it came, else the shell's Python line for line.
//!
//! The shell answers these itself (`None`): `-h`, `--help` (and `auth
//! help`) anywhere; a body file (`--invite-body-file`,
//! `--reset-body-file`) that can't be read, holds a NUL or is not UTF-8.

use super::call::{Cj, Ctx};
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::skin_token::py::{dumps, obj, strip};
use crate::families::{py_space, Raise};
use crate::out::json::{Text, Value};
use crate::out::pylib::ops::py_eq;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const AUTH_HELP: &str = concat!(" (", cli_name!(), " app auth --help)");
    pub const ORIGIN: &str = concat!("--origin goes with ", cli_name!(), " app auth origins");
    pub const CLEAR: &str = concat!("--clear goes with ", cli_name!(), " app auth origins");
    pub const DOOR: &str = concat!(
        "usage: ",
        cli_name!(),
        " app auth door on|off [<app>] (",
        cli_name!(),
        " app auth --help)"
    );
    pub const ORIGINS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app auth origins <app> [--origin O]\u{2026} [--clear] (",
        cli_name!(),
        " app auth --help)"
    );
    pub const BOTH: &str = concat!(
        "--origin and --clear don't go together (",
        cli_name!(),
        " app auth --help)"
    );
    pub const LOGIN: &str = concat!(
        "usage: ",
        cli_name!(),
        " app login settings <app> [--invite-ttl 72h] [--reset-ttl 24h] [--set-password-url <url>] [--invite-subject <s>] [--invite-body-file <f>] [--reset-subject <s>] [--reset-body-file <f>] [--max-devices <n>] [--default <field>]"
    );
    pub const SETTINGS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app login settings <app> [flags]   (",
        cli_name!(),
        " app login settings --help)"
    );
    pub const MAX_DEVICES: &str = concat!(
        cli_name!(),
        " app login settings: --max-devices is a whole number from 1 to 20 (not '"
    );
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `auth [<app>]`, or `auth origins <app>` with no change: the GET.
    AuthStatus { app: String, json: bool },
    /// `auth door on|off [<app>]`.
    AuthDoor {
        onoff: String,
        app: String,
        json: bool,
    },
    /// `auth origins <app> --origin …|--clear`: the body.
    AuthOrigins { body: String, json: bool },
    /// `login settings <app> …`: the body.
    Login { body: String, json: bool },
}

fn help(words: &[&str]) -> bool {
    words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

/// The words after `auth`. `None`: the shell's.
pub fn parse_auth(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) || words.first() == Some(&"help") {
        return None;
    }
    Some(auth(words))
}

fn auth(words: &[&str]) -> Result<Ask, Stop> {
    let (sub, mut rest) = match words.first().copied() {
        Some(s @ ("door" | "origins")) => (s, &words[1..]),
        Some(s) if s.starts_with('-') && s != "--json" => {
            return Err(Stop::usage(format!(
                "Unknown flag: {s}{}",
                usage::AUTH_HELP
            )))
        }
        _ => ("status", words),
    };
    let (mut app, mut json, mut onoff, mut origins, mut clear) =
        (String::new(), false, String::new(), String::new(), false);
    while let Some(w) = rest.first().copied() {
        match w {
            "--json" => json = true,
            "--origin" => {
                if sub != "origins" {
                    return Err(Stop::usage(usage::ORIGIN));
                }
                origins.push_str(rest.get(1).copied().unwrap_or(""));
                origins.push('\n');
                if rest.len() < 2 {
                    return Err(Stop::silent(Exit::Error));
                }
                rest = &rest[2..];
                continue;
            }
            "--clear" => {
                if sub != "origins" {
                    return Err(Stop::usage(usage::CLEAR));
                }
                clear = true;
            }
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!(
                    "Unknown flag: {w}{}",
                    usage::AUTH_HELP
                )))
            }
            "on" | "off" if sub == "door" && onoff.is_empty() => onoff = w.to_string(),
            _ if app.is_empty() => app = w.to_string(),
            _ => {
                return Err(Stop::usage(format!(
                    "Unexpected argument: {w}{}",
                    usage::AUTH_HELP
                )))
            }
        }
        rest = &rest[1..];
    }
    match sub {
        "door" => {
            if onoff.is_empty() {
                return Err(Stop::usage(usage::DOOR));
            }
            Ok(Ask::AuthDoor { onoff, app, json })
        }
        "origins" => {
            if app.is_empty() {
                return Err(Stop::usage(usage::ORIGINS));
            }
            if origins.is_empty() && !clear {
                return Ok(Ask::AuthStatus { app, json });
            }
            if !origins.is_empty() && clear {
                return Err(Stop::usage(usage::BOTH));
            }
            let list: Vec<Value> = origins
                .split('\n')
                .map(strip)
                .filter(|x| !x.is_empty())
                .map(|x| Value::Str(Text::from(x)))
                .collect();
            let body = dumps(&obj(vec![
                ("app", Value::Str(Text::from(app.as_str()))),
                ("origins", Value::Array(list)),
            ]));
            Ok(Ask::AuthOrigins { body, json })
        }
        _ => Ok(Ask::AuthStatus { app, json }),
    }
}

/// The words after `login`. `None`: the shell's.
pub fn parse_login(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    if words.first() != Some(&"settings") {
        return Some(Err(Stop::usage(usage::LOGIN)));
    }
    login(&words[1..])
}

/// `_read_body_file <path>`, as `$(…)` holds it: `Some(Err)` for its
/// line, `None` where the shell's `cat` would fail or the text is not
/// what the binary can hand on as the shell does.
fn body_file(path: &str) -> Option<Result<String, Stop>> {
    if !std::path::Path::new(path).is_file() {
        return Some(Err(Stop::usage(format!("no such file: {path}"))));
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let mut text = String::from_utf8(bytes).ok()?;
    while text.ends_with('\n') {
        text.pop();
    }
    Some(Ok(text))
}

fn login(words: &[&str]) -> Option<Result<Ask, Stop>> {
    const KEYS: [(&str, &str); 7] = [
        ("--invite-ttl", "inviteTtl"),
        ("--reset-ttl", "resetTtl"),
        ("--set-password-url", "setPasswordUrl"),
        ("--invite-subject", "inviteSubject"),
        ("--invite-body-file", "inviteBody"),
        ("--reset-subject", "resetSubject"),
        ("--reset-body-file", "resetBody"),
    ];
    let mut values: [String; 7] = Default::default();
    let (mut app, mut json, mut max, mut defaults) =
        (String::new(), false, String::new(), String::new());
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        let v = rest.get(1).copied().unwrap_or("");
        if let Some(i) = KEYS.iter().position(|(flag, _)| *flag == w) {
            values[i] = if w.ends_with("-body-file") {
                match body_file(v)? {
                    Ok(t) => t,
                    Err(stop) => return Some(Err(stop)),
                }
            } else {
                v.to_string()
            };
        } else {
            match w {
                "--max-devices" => max = v.to_string(),
                "--default" => {
                    defaults.push(' ');
                    defaults.push_str(v);
                }
                "--json" => {
                    json = true;
                    rest = &rest[1..];
                    continue;
                }
                _ if w.starts_with('-') => {
                    return Some(Err(Stop::usage(format!("Unknown flag: {w}"))))
                }
                _ => {
                    app = w.to_string();
                    rest = &rest[1..];
                    continue;
                }
            }
        }
        if rest.len() < 2 {
            return Some(Err(Stop::silent(Exit::Error)));
        }
        rest = &rest[2..];
    }
    if app.is_empty() {
        return Some(Err(Stop::usage(usage::SETTINGS)));
    }
    if !(max.is_empty() || (max.len() <= 2 && max.bytes().all(|b| b.is_ascii_digit()))) {
        return Some(Err(Stop::usage(format!("{}{max})", usage::MAX_DEVICES))));
    }
    let mut b: Vec<(Text, Value)> = vec![(Text::from("app"), Value::Str(Text::from(app.as_str())))];
    for (i, (_, key)) in KEYS.iter().enumerate() {
        if !values[i].is_empty() {
            b.push((Text::from(*key), Value::Str(Text::from(values[i].as_str()))));
        }
    }
    if !max.is_empty() {
        let digits = max.trim_start_matches('0');
        b.push((
            Text::from("maxDevices"),
            Value::Int(if digits.is_empty() { "0" } else { digits }.to_string()),
        ));
    }
    for f in defaults.split(py_space).filter(|f| !f.is_empty()) {
        let key = Text::from(f);
        match b.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = Value::Null,
            None => b.push((key, Value::Null)),
        }
    }
    Some(Ok(Ask::Login {
        body: dumps(&Value::Object(b)),
        json,
    }))
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    let (body, json) = match ask {
        Ask::AuthStatus { app, json } => {
            let q = if app.is_empty() {
                String::new()
            } else {
                format!("app={}", quote(app))
            };
            (ctx.call(false, "/cli/app/auth", &q)?, *json)
        }
        Ask::AuthDoor { onoff, app, json } => {
            let b = Cj::new().s("door", onoff).s("app", app).body();
            (post_200(ctx, "/cli/app/auth/set", &b)?, *json)
        }
        Ask::AuthOrigins { body, json } => (post_200(ctx, "/cli/app/auth/set", body)?, *json),
        Ask::Login { body, json } => {
            let body = ctx.call(true, "/cli/skin/access/invites", body)?;
            return Ok(super::access::answer(&body, *json, login_lines));
        }
    };
    Ok(super::access::answer(&body, json, auth_lines))
}

/// `_app_rt_post`, then `[ "$_CP_STATUS" = "200" ] || _compute_fail`.
fn post_200(ctx: &mut Ctx, path: &str, body: &str) -> Result<Vec<u8>, Exit> {
    let s = ctx.send(true, path, body, false)?;
    if s.ok() {
        Ok(s.body)
    } else {
        Err(ctx.fail(&s))
    }
}

/// `cmd_app_auth`'s Python.
fn auth_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let b = py::or(py::dget(&v, "box", 4)?, Value::Object(Vec::new()));
    let on = py::truthy(&py::dget(&b, "on", 5)?);
    let mut line = format!(
        "sign-in door on this server: {}",
        if on { "on" } else { "off" }
    );
    if py::truthy(&py::dget(&b, "envOverride", 6)?) {
        line.push_str(&format!(
            " (K2_APP_AUTH_DOOR={} on the daemon wins over the stored switch)",
            py::s(&py::dget(&b, "envOverride", 7)?)
        ));
    } else if !py::truthy(&py::dget(&b, "on", 8)?) {
        line.push_str(concat!(
            " (the owner turns it on: ",
            cli_name!(),
            " app auth door on)"
        ));
    }
    o.line(&line);
    let a = py::dget(&v, "app", 11)?;
    if py::truthy(&a) {
        let name = py::or(py::dget(&a, "name", 13)?, py::dget(&a, "app", 13)?);
        let door = py::dget(&a, "door", 13)?;
        let off = if py::truthy(&py::dget(&a, "appOff", 13)?) {
            " (turned off for this App)"
        } else {
            ""
        };
        o.line(&format!("{}: door {}{off}", py::s(&name), py::s(&door)));
        let origins = py::or(py::dget(&a, "origins", 14)?, py::empty_list());
        let origins = if py::truthy(&origins) {
            py::join(", ", &origins, 15)?
        } else {
            "-".to_string()
        };
        o.line(&format!("  extra page origins:    {origins}"));
        let po = py::or(py::dget(&a, "pairingOrigins", 16)?, py::empty_list());
        if py::truthy(&po) {
            o.line(&format!("  from phone pairing:    {}", py::join(", ", &po, 18)?));
        }
        let hosts = py::or(py::dget(&a, "hosts", 19)?, py::empty_list());
        let hosts = py::join(", ", &hosts, 19)?;
        let hosts = if hosts.is_empty() {
            "- (no public host yet)".to_string()
        } else {
            hosts
        };
        o.line(&format!("  hosts:                 {hosts}"));
        if py::truthy(&py::dget(&a, "labelNote", 20)?) {
            o.line(&py::concat("  note: ", &py::idx(&a, "labelNote", 21)?, 21)?);
        }
        if py::truthy(&py::dget(&a, "notRuntime", 22)?) {
            o.line(concat!(
                "  note: not a runtime App: ",
                product_name!(),
                " answers /.k2/auth/* only in front of a runtime App (",
                cli_name!(),
                " app deploy)"
            ));
        }
    }
    let warnings = py::or(py::dget(&v, "warnings", 24)?, py::empty_list());
    for w in py::iter(&warnings, 24)? {
        o.line(&py::concat("note: ", &w, 25)?);
    }
    Ok(Exit::Ok)
}

/// `int(n or 0)` in `life`, as an `i128`.
fn int_or_zero(n: &Value, line: u32) -> Result<i128, Raise> {
    let n = py::or(n.clone(), Value::Int("0".into()));
    let digits = crate::families::feedback::py_int(&n).map_err(|e| Raise::inside(10, line, "life", e))?;
    digits.parse().map_err(|_| {
        Raise::inside(
            10,
            line,
            "life",
            "OverflowError: Python int too large to convert to C long",
        )
    })
}

/// `life(n, own)`.
fn life(n: &Value, own: &Value) -> Result<String, Raise> {
    let n = int_or_zero(n, 6)?;
    let t = if n.rem_euclid(86_400) == 0 && n >= 4 * 86_400 {
        format!("{} days", n.div_euclid(86_400))
    } else if n.rem_euclid(3600) == 0 {
        format!("{} hours", n.div_euclid(3600))
    } else {
        format!("{} minutes", n.div_euclid(60))
    };
    Ok(if py::truthy(own) {
        t
    } else {
        format!("{t} (default)")
    })
}

/// `cmd_app_invite_settings`' Python.
fn login_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let s = py::or(py::dget(&v, "settings", 4)?, Value::Object(Vec::new()));
    o.line(&format!("App {}", py::s(&py::dget(&v, "app", 9)?)));
    let invite = life(
        &py::dget(&s, "inviteTtlSecs", 10)?,
        &py::dget(&s, "inviteTtlSet", 10)?,
    )?;
    o.line(&format!("  invite links live {invite}"));
    let reset = life(
        &py::dget(&s, "resetTtlSecs", 11)?,
        &py::dget(&s, "resetTtlSet", 11)?,
    )?;
    o.line(&format!("  reset links live  {reset}"));
    let page = py::or(
        py::dget(&s, "setPasswordUrl", 12)?,
        py::text("the box front door /reset (default)"),
    );
    o.line(&format!("  set-password page {}", py::s(&page)));
    for k in ["invite", "reset"] {
        let t = py::or(py::dget(&s, k, 14)?, Value::Object(Vec::new()));
        let own = py::or(py::dget(&t, "subject", 15)?, py::dget(&t, "body", 15)?);
        let which = if py::truthy(&own) {
            "the App template"
        } else {
            concat!(product_name!(), " template")
        };
        o.line(&format!("  {k} mail: {which}"));
    }
    let max = py::dget(&v, "maxDevices", 16)?;
    let note = if py_eq(&max, &Value::Int("20".into())) {
        " (default)"
    } else {
        ""
    };
    o.line(&format!("  devices per person: {}{note}", py::s(&max)));
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_auth_words_as_the_shell_reads_them() {
        assert_eq!(
            auth(&["--json"]),
            Ok(Ask::AuthStatus {
                app: String::new(),
                json: true
            })
        );
        assert_eq!(
            auth(&["off"]),
            Ok(Ask::AuthStatus {
                app: "off".into(),
                json: false
            })
        );
        assert_eq!(
            auth(&["door", "acme", "on"]),
            Ok(Ask::AuthDoor {
                onoff: "on".into(),
                app: "acme".into(),
                json: false
            })
        );
        assert_eq!(auth(&["door", "on", "off", "x"]).unwrap_err().exit, Exit::Usage);
        assert_eq!(auth(&["--zap"]).unwrap_err().exit, Exit::Usage);
        assert_eq!(auth(&["acme", "--origin", "x"]), Err(Stop::usage(usage::ORIGIN)));
        assert_eq!(
            auth(&["origins", "a", "--origin", " x ", "--origin", ""]),
            Ok(Ask::AuthOrigins {
                body: r#"{"app": "a", "origins": ["x"]}"#.into(),
                json: false
            })
        );
        assert_eq!(
            auth(&["origins", "a", "--origin", "x", "--clear"]),
            Err(Stop::usage(usage::BOTH))
        );
        assert_eq!(auth(&["origins", "a", "--origin"]), Err(Stop::silent(Exit::Error)));
    }

    #[test]
    fn the_login_words_as_the_shell_reads_them() {
        let body = |w: &[&str]| match login(w) {
            Some(Ok(Ask::Login { body, .. })) => body,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            body(&["acme", "--invite-ttl", "48h", "--max-devices", "05", "--default", "inviteTtl resetBody", "--default", "app"]),
            r#"{"app": null, "inviteTtl": null, "maxDevices": 5, "resetBody": null}"#
        );
        assert_eq!(
            login(&["acme", "--max-devices", "100"]),
            Some(Err(Stop::usage(format!("{}100)", usage::MAX_DEVICES))))
        );
        assert_eq!(
            login(&["acme", "--invite-body-file"]),
            Some(Err(Stop::usage("no such file: ")))
        );
        assert_eq!(login(&["acme", "--reset-ttl"]), Some(Err(Stop::silent(Exit::Error))));
        assert_eq!(login(&[]), Some(Err(Stop::usage(usage::SETTINGS))));
    }

    #[test]
    fn link_lives_as_python_words_them() {
        let i = |n: i64| Value::Int(n.to_string());
        assert_eq!(life(&i(259_200), &Value::Bool(true)).unwrap(), "72 hours");
        assert_eq!(life(&i(345_600), &Value::Null).unwrap(), "4 days (default)");
        assert_eq!(life(&Value::Null, &Value::Bool(true)).unwrap(), "0 hours");
        assert_eq!(life(&i(90), &Value::Bool(true)).unwrap(), "1 minutes");
        assert_eq!(life(&i(-90), &Value::Bool(true)).unwrap(), "-2 minutes");
        assert!(life(&py::text("x"), &Value::Null).is_err());
    }
}
