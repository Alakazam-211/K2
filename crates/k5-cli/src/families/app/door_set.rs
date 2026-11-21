//! More of an App's door, as the shell CLI answers it
//! (`cli/src/app-door.sh`, `cli/src/app-recording.sh`): `app requests`,
//! `app quota` (and `quotas`) and `app token rotate` (and `tokens
//! rotate`). Each is the shell's word loop, one request, `--json` as it
//! came, else the shell's Python line for line.
//!
//! `app token rotate` mints a remote App pass, a long-lived credential:
//! the owner's, outside a session (in one it is refused before anything
//! is sent), over TCP. The pass is printed on stdout as the shell prints
//! it, and its notes on stderr; nothing else holds it.

use super::call::{Cj, Ctx};
use super::grace_set::{help, split_strip, walk};
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{py_len, Raise, Shown};
use crate::out::json::{self, Options, Value};
use crate::out::py as pyfmt;
use crate::out::pylib::ops::py_eq;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const REQUESTS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app requests <app-id> [--since 1h] [--until 10m] [--status 4xx|5xx|N] [--path /prefix] [--method POST] [--limit N] [--json]"
    );
    pub const QUOTA: &str = concat!(
        "usage: ",
        cli_name!(),
        " app quota <app-id> [--rps N] [--burst N] [--sockets N] [--body 8M] [--upload 100M] [--subscriptions N] [--reset]"
    );
    pub const QUOTA_NUMBERS: &str =
        "usage: --rps/--burst/--sockets/--subscriptions take whole numbers";
    pub const ROTATE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app token rotate <app-id> [--ttl 30d] [--keep-old] [--caps a,b]"
    );
}

/// `cmd_app_token_rotate`'s line in a session (exit 3).
const ROTATE_OWNER_ONLY: &str = concat!(
    "{\"error\":{\"code\":\"owner_only\",\"hint\":\"a remote App pass is a long-lived credential: the owner mints it from a terminal outside ",
    product_name!(),
    "\"}}"
);

/// `"%s: %s request%s (%s ok, %s 4xx, %s 5xx; %s answered by K2, %s closed
/// early)"`, split where the values go.
const ANSWERED_BY: &str = concat!(" answered by ", product_name!(), ", ");
/// `"No visitor request reached this App in that window. A request K2
/// never got (DNS, the relay, the"`.
const NO_ROWS: &str = concat!(
    "No visitor request reached this App in that window. A request ",
    product_name!(),
    " never got (DNS, the relay, the"
);
/// `st += " K2 " + r["k2Code"]`.
const K2_CODE: &str = concat!(" ", product_name!(), " ");
/// `"note: %s row(s) were dropped since K2 started (the log queue was
/// full)"`.
const DROPPED_TAIL: &str = concat!(
    " row(s) were dropped since ",
    product_name!(),
    " started (the log queue was full)"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The query is built (`app=…&since=…`).
    Requests {
        query: String,
        json: bool,
    },
    /// `set`: the POST's body when a flag asks for a change.
    Quota {
        app: String,
        set: Option<String>,
        json: bool,
    },
    Rotate {
        app: String,
        body: String,
        json: bool,
    },
}

/// The verbs here (`token rotate` is [`parse_rotate`]'s).
pub const VERBS: &[&str] = &["requests", "quota", "quotas"];

/// The words after the verb. `None`: a help form (the shell's).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    Some(match verb {
        "requests" => requests(words),
        "quota" | "quotas" => quota(words),
        _ => return None,
    })
}

/// The words after `token rotate`. `None`: a help form.
pub fn parse_rotate(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    Some(rotate(words))
}

fn requests(words: &[&str]) -> Result<Ask, Stop> {
    let w = walk(
        words,
        &[
            &["--since"],
            &["--until"],
            &["--status"],
            &["--path"],
            &["--method"],
            &["--limit"],
        ],
        &["--json"],
    )?;
    let app = w.last();
    if app.is_empty() {
        return Err(Stop::usage(usage::REQUESTS));
    }
    let mut query = format!("app={}", quote(&app));
    for (flag, key) in [
        ("--since", "since"),
        ("--until", "until"),
        ("--status", "status"),
        ("--path", "path_prefix"),
        ("--method", "method"),
        ("--limit", "limit"),
    ] {
        let v = w.v(flag);
        if !v.is_empty() {
            query.push_str(&format!("&{key}={}", quote(&v)));
        }
    }
    Ok(Ask::Requests {
        query,
        json: w.has("--json"),
    })
}

fn quota(words: &[&str]) -> Result<Ask, Stop> {
    let w = walk(
        words,
        &[
            &["--rps"],
            &["--burst"],
            &["--sockets"],
            &["--body"],
            &["--upload"],
            &["--subscriptions"],
            &["--reason"],
        ],
        &["--reset", "--json"],
    )?;
    let app = w.last();
    if app.is_empty() {
        return Err(Stop::usage(usage::QUOTA));
    }
    let (rps, burst, sockets, subs) = (
        w.v("--rps"),
        w.v("--burst"),
        w.v("--sockets"),
        w.v("--subscriptions"),
    );
    for n in [&rps, &burst, &sockets, &subs] {
        if !n.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Stop::usage(usage::QUOTA_NUMBERS));
        }
    }
    let any = w.has("--reset")
        || [
            "--rps",
            "--burst",
            "--sockets",
            "--body",
            "--upload",
            "--subscriptions",
        ]
        .iter()
        .any(|f| w.given(f));
    let set = any.then(|| {
        Cj::new()
            .s("app", &app)
            .n("rps", &rps)
            .n("burst", &burst)
            .n("sockets", &sockets)
            .s("bodyBytes", &w.v("--body"))
            .s("uploadBytes", &w.v("--upload"))
            .n("subscriptions", &subs)
            .b("reset", if w.has("--reset") { "1" } else { "" })
            .s("reason", &w.v("--reason"))
            .body()
    });
    Ok(Ask::Quota {
        app,
        set,
        json: w.has("--json"),
    })
}

fn rotate(words: &[&str]) -> Result<Ask, Stop> {
    let w = walk(
        words,
        &[&["--ttl"], &["--caps"]],
        &["--keep-old", "--json"],
    )?;
    let app = w.last();
    if app.is_empty() {
        return Err(Stop::usage(usage::ROTATE));
    }
    let caps = w.v("--caps");
    let caps_j = if caps.is_empty() {
        String::new()
    } else {
        json::to_string(&Value::Array(split_strip(&caps)), &Options::default())
            .unwrap_or_default()
    };
    let body = Cj::new()
        .s("app", &app)
        .s("ttl", &w.v("--ttl"))
        .b("keepOld", if w.has("--keep-old") { "1" } else { "" })
        .j("caps", &caps_j)
        .body();
    Ok(Ask::Rotate {
        app,
        body,
        json: w.has("--json"),
    })
}

/// `--json`: the answer as it came; else the printer.
fn printed(body: &[u8], json: bool, lines: fn(&[u8], &mut Out) -> Result<(), Raise>) -> Shown {
    if json {
        return raw(body);
    }
    let mut o = Out::default();
    let r = lines(body, &mut o).map(|()| Exit::Ok);
    o.done(r)
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Requests { query, json } => {
            let body = ctx.call(false, "/cli/app/requests", query)?;
            Ok(printed(&body, *json, requests_lines))
        }
        Ask::Quota { app, set, json } => {
            let body = match set {
                Some(b) => {
                    let s = ctx.send(true, "/cli/app/quota/set", b, false)?;
                    match super::grace::done(ctx, s, *json) {
                        Ok(body) => body,
                        Err(a) => return a,
                    }
                }
                None => ctx.call(false, "/cli/app/quota", &format!("app={}", quote(app)))?,
            };
            Ok(printed(&body, *json, quota_lines))
        }
        Ask::Rotate { body, json, .. } => {
            if ctx.in_session() {
                return Ok(Shown::err(format!("{ROTATE_OWNER_ONLY}\n"), Exit::Refused));
            }
            let body = ctx.call_on(true, "/cli/app/remote-pass/rotate", body, true)?;
            Ok(printed(&body, *json, rotate_lines))
        }
    }
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

/// `size(n)`, called from line `from`.
fn size(n: &Value, from: u32) -> Result<String, Raise> {
    let n = if py::truthy(n) {
        n.clone()
    } else {
        Value::Int("0".into())
    };
    let x = match &n {
        Value::Int(d) => d.parse::<f64>().unwrap_or(f64::INFINITY),
        Value::Float(f) => *f,
        Value::Bool(b) => f64::from(u8::from(*b)),
        other => {
            return Err(Raise::inside(
                from,
                9,
                "size",
                format!(
                    "TypeError: '<' not supported between instances of '{}' and 'int'",
                    pyfmt::type_name(other)
                ),
            ))
        }
    };
    if x < 1024.0 {
        return Ok(py::s(&n));
    }
    let mut x = x;
    for unit in ["K", "M", "G"] {
        x /= 1024.0;
        if x < 1024.0 || unit == "G" {
            return Ok(if x < 10.0 {
                format!("{}{unit}", py::f(x, 0, 1))
            } else if x.is_finite() {
                format!("{}{unit}", x.trunc())
            } else {
                return Err(Raise::inside(
                    from,
                    14,
                    "size",
                    "OverflowError: cannot convert float infinity to integer",
                ));
            });
        }
    }
    Ok(String::new())
}

/// `cmd_app_requests`' Python.
fn requests_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let app = py::idx(&v, "app", 4)?;
    let t = py::or(py::dget(&v, "totals", 5)?, empty_dict());
    let rows = py::or(py::dget(&v, "requests", 6)?, py::empty_list());
    let zero = || Value::Int("0".into());
    let n = py::dget_or(&t, "requests", zero(), 16)?;
    let one = py_eq(&py::dget(&t, "requests", 16)?, &Value::Int("1".into()));
    let ok = py::dget_or(&t, "ok", zero(), 16)?;
    let c4 = py::dget_or(&t, "clientErrors", zero(), 16)?;
    let c5 = py::dget_or(&t, "serverErrors", zero(), 17)?;
    let k2 = py::dget_or(&t, "k2Answered", zero(), 17)?;
    let early = py::dget_or(&t, "closedEarly", zero(), 17)?;
    o.line(&format!(
        "{}: {} request{} ({} ok, {} 4xx, {} 5xx; {}{ANSWERED_BY}{} closed early)",
        py::s(&app),
        py::s(&n),
        if one { "" } else { "s" },
        py::s(&ok),
        py::s(&c4),
        py::s(&c5),
        py::s(&k2),
        py::s(&early)
    ));
    let keep = |line: u32| -> Result<Value, Raise> {
        Ok(py::or(py::dget(&v, "keep", line)?, empty_dict()))
    };
    if !py::truthy(&rows) {
        o.line(NO_ROWS);
        let days = py::dget_or(&keep(20)?, "days", Value::Int("7".into()), 20)?;
        o.line(&format!(
            "browser) has no row; widen it with --since 1d. Rows are kept {} days.",
            py::s(&days)
        ));
    } else {
        o.line(&format!(
            "{}  {}  {}  {}  {}  {}  {}",
            crate::families::ljust("TIME", 19),
            crate::families::ljust("METHOD", 7),
            crate::families::ljust("PATH", 34),
            crate::families::ljust("STATUS", 28),
            py::rjust("IN", 6),
            py::rjust("OUT", 6),
            py::rjust("MS", 6)
        ));
        for r in py::iter(&rows, 23)? {
            let at = py::div(&py::idx(&r, "atMs", 24)?, 1000.0, 24)?;
            let at = py::local_time(&Value::Float(at), "%Y-%m-%d %H:%M:%S", 24)?;
            let mut st = if py::truthy(&py::dget(&r, "status", 25)?) {
                py::s(&py::idx(&r, "status", 25)?)
            } else {
                "-".to_string()
            };
            if py::truthy(&py::dget(&r, "k2Code", 26)?) {
                st.push_str(&py::concat(K2_CODE, &py::idx(&r, "k2Code", 27)?, 27)?);
            }
            if py::truthy(&py::dget(&r, "ws", 28)?) {
                st.push_str(&py::concat(" ws ", &py::idx(&r, "ws", 29)?, 29)?);
            }
            if py::truthy(&py::dget(&r, "closedEarly", 30)?) {
                let c = py::concat(" (", &py::idx(&r, "closedEarly", 31)?, 31)?;
                st.push_str(&c);
                st.push_str(" closed)");
            }
            let mut path = py::or(py::dget(&r, "path", 32)?, py::text("-"));
            if py_len(&path, 33)? > 34 {
                path = match py::head(&path, 33, 34)? {
                    head @ Value::Str(_) => py::text(&format!("{}~", py::s(&head))),
                    other => {
                        return Err(Raise::at(
                            34,
                            format!(
                                "TypeError: can only concatenate {} (not \"str\") to {}",
                                pyfmt::type_name(&other),
                                pyfmt::type_name(&other)
                            ),
                        ))
                    }
                };
            }
            let method = py::or(py::dget(&r, "method", 35)?, py::text("-"));
            let bytes_in = size(&py::dget(&r, "bytesIn", 35)?, 35)?;
            let bytes_out = size(&py::dget(&r, "bytesOut", 35)?, 35)?;
            let ms = py::dget(&r, "totalMs", 35)?;
            o.line(&format!(
                "{}  {}  {}  {}  {}  {}  {}",
                crate::families::ljust(&at, 19),
                py::left(&method, 7),
                py::left(&path, 34),
                crate::families::ljust(&st, 28),
                py::rjust(&bytes_in, 6),
                py::rjust(&bytes_out, 6),
                py::right(&ms, 6)
            ));
        }
        let days = py::dget_or(&keep(37)?, "days", Value::Int("7".into()), 37)?;
        let rows_per = py::dget(&keep(37)?, "rowsPerApp", 37)?;
        o.line(&format!(
            "newest first; status - means no answer came. Kept {} days, at most {} rows per App.",
            py::s(&days),
            py::s(&rows_per)
        ));
    }
    if py::truthy(&py::dget(&v, "dropped", 38)?) {
        o.line(&format!(
            "note: {}{DROPPED_TAIL}",
            py::s(&py::idx(&v, "dropped", 39)?)
        ));
    }
    Ok(())
}

/// `cmd_app_quota`'s Python.
fn quota_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let q = py::or(py::dget(&v, "quotas", 4)?, empty_dict());
    let ov = py::or(py::dget(&v, "override", 5)?, empty_dict());
    let mark = |k: &str, from: u32| -> Result<&'static str, Raise> {
        let r = py::contains(&ov, k, 7).map_err(|e| Raise::inside(from, 7, "mark", e.exception))?;
        Ok(if r { " (override)" } else { "" })
    };
    let either = |a: &str, b: &str, from: u32| -> Result<&'static str, Raise> {
        let m = mark(a, from)?;
        if m.is_empty() {
            mark(b, from)
        } else {
            Ok(m)
        }
    };
    o.line(&format!("{} quotas:", py::s(&py::idx(&v, "app", 8)?)));
    let rps = py::dget(&q, "rps", 9)?;
    let burst = py::dget(&q, "burst", 9)?;
    o.line(&format!(
        "  calls/s        {} sustained, bursts of {}{}",
        py::s(&rps),
        py::s(&burst),
        either("rps", "burst", 9)?
    ));
    let sockets = py::dget(&q, "sockets", 10)?;
    o.line(&format!(
        "  sockets        {}{}",
        py::s(&sockets),
        mark("sockets", 10)?
    ));
    let body_b = py::dget(&q, "bodyBytes", 11)?;
    let upload = py::dget(&q, "uploadBytes", 11)?;
    o.line(&format!(
        "  body           {} bytes (uploads {}){}",
        py::s(&body_b),
        py::s(&upload),
        either("bodyBytes", "uploadBytes", 11)?
    ));
    let subs = py::dget(&q, "subscriptions", 12)?;
    o.line(&format!(
        "  subscriptions  {}{}",
        py::s(&subs),
        mark("subscriptions", 12)?
    ));
    let live = py::dget(&v, "live", 13)?;
    if py::truthy(&live) {
        o.line(&format!(
            "  now            {} connections, {} subscriptions",
            py::s(&py::dget(&live, "connections", 15)?),
            py::s(&py::dget(&live, "subscriptions", 15)?)
        ));
    }
    if py::truthy(&py::dget(&v, "raised", 16)?) {
        let raised = py::join(", ", &py::idx(&v, "raised", 17)?, 17)?;
        o.line(&format!("  raised: {raised}"));
    }
    Ok(())
}

/// `cmd_app_token_rotate`'s Python: the pass on stdout, its notes on
/// stderr.
fn rotate_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    o.line(&py::s(&py::idx(&v, "pass", 4)?));
    let app = py::idx(&v, "app", 6)?;
    let prefix = py::dget(&v, "prefix", 6)?;
    let caps = py::join(", ", &py::or(py::dget(&v, "caps", 6)?, py::empty_list()), 6)?;
    let caps = if caps.is_empty() {
        "none".to_string()
    } else {
        caps
    };
    let rooms = py::join(", ", &py::or(py::dget(&v, "rooms", 6)?, py::empty_list()), 6)?;
    let expires = py::local_minutes(&py::idx(&v, "expiresAt", 7)?, 7)?;
    let revoked = py::dget(&v, "revoked", 7)?;
    o.stderr.push_str(&format!(
        "{}: remote pass {}, caps {caps}, rooms {rooms}, expires {expires}; {} old revoked\n",
        py::s(&app),
        py::s(&prefix),
        py::s(&revoked)
    ));
    if py::truthy(&py::dget(&v, "withheld", 8)?) {
        let w = py::join(", ", &py::idx(&v, "withheld", 9)?, 9)?;
        o.stderr.push_str(&format!(
            "  withheld until the owner approves them: {w}\n"
        ));
    }
    let note = py::dget_or(&v, "note", py::text(""), 10)?;
    o.stderr.push_str(&format!("  {}\n", py::s(&note)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loops_read_the_words_as_the_shell_does() {
        assert_eq!(
            parse("requests", &["a", "--path", "/x y", "--limit", "5"]),
            Some(Ok(Ask::Requests {
                query: "app=a&path_prefix=%2Fx%20y&limit=5".into(),
                json: false
            }))
        );
        assert_eq!(
            parse("quota", &["a", "--rps", "1x"]),
            Some(Err(Stop::usage(usage::QUOTA_NUMBERS)))
        );
        assert_eq!(
            parse("quota", &["a", "--reason", "r"]),
            Some(Ok(Ask::Quota {
                app: "a".into(),
                set: None,
                json: false
            }))
        );
        assert_eq!(
            parse("quota", &["a", "--rps", "007", "--reset"]),
            Some(Ok(Ask::Quota {
                app: "a".into(),
                set: Some(r#"{"app": "a", "rps": 7, "reset": true}"#.into()),
                json: false
            }))
        );
        assert_eq!(
            parse_rotate(&["a", "--caps", " x, ,y", "--keep-old"]),
            Some(Ok(Ask::Rotate {
                app: "a".into(),
                body: r#"{"app": "a", "keepOld": true, "caps": ["x", "y"]}"#.into(),
                json: false
            }))
        );
        assert!(parse_rotate(&["a", "--help"]).is_none());
    }

    #[test]
    fn sizes_read_as_the_python_does() {
        assert_eq!(size(&Value::Null, 35).unwrap(), "0");
        assert_eq!(size(&Value::Int("1023".into()), 35).unwrap(), "1023");
        assert_eq!(size(&Value::Int("2048".into()), 35).unwrap(), "2.0K");
        assert_eq!(size(&Value::Int("20480".into()), 35).unwrap(), "20K");
        assert_eq!(size(&Value::Float(5.0), 35).unwrap(), "5.0");
        assert!(size(&py::text("x"), 35).is_err());
    }
}
