//! The shell's Python printers for `cmd_skin`'s answers, line for line,
//! each raising where Python 3.12 raises (`File "<string>", line N`).
//! Every printer reads the answer `_skin_die_if_err` already passed.

use super::super::skin_token::py::{self, at, at_or, dict, join, load, or, plus_str, py_in, s};
use super::super::{py_iter, py_left, py_len, raised, Raise, Shown};
use super::Reader;
use crate::exit::Exit;
use crate::families::activity;
use crate::out::json::Value;
use crate::out::py as pyout;
use k2_client::{cli_name, product_name};

/// What a printer wrote before it ended: stdout and stderr.
#[derive(Default)]
struct Out {
    o: String,
    e: String,
}

impl Out {
    fn line(&mut self, text: impl AsRef<str>) {
        self.o.push_str(text.as_ref());
        self.o.push('\n');
    }

    fn err(&mut self, text: impl AsRef<str>) {
        self.e.push_str(text.as_ref());
        self.e.push('\n');
    }
}

/// How a printer ends without raising: `sys.exit(n)` or the end.
type Ends = Result<Exit, Raise>;

/// `d=json.load(sys.stdin)` at line 3, then `body`.
fn printed(raw: &str, body: impl FnOnce(&Value, &mut Out) -> Ends) -> Shown {
    let mut out = Out::default();
    let r = load(raw, 3).and_then(|d| body(&d, &mut out));
    match r {
        Ok(exit) => Shown {
            stdout: out.o,
            stderr: out.e,
            exit,
        },
        Err(r) => raised(out.o, out.e, &r),
    }
}

fn st(v: &Value) -> String {
    pyout::str_of(v)
}

fn truthy(v: &Value) -> bool {
    pyout::truthy(v)
}

/// `d.get(k)` on a dict, `None` as Python's `None`.
fn g(d: &Value, k: &str) -> Value {
    at(d, k).cloned().unwrap_or(Value::Null)
}

/// `a or b` for two `get`s.
fn or2(d: &Value, a: &str, b: &str) -> Value {
    or(at(d, a), g(d, b))
}

/// `x or y`.
fn orv(x: Value, y: Value) -> Value {
    if truthy(&x) {
        x
    } else {
        y
    }
}

/// `v.get(…)` where Python needs a dict.
fn need(v: &Value, line: u32) -> Result<&Value, Raise> {
    dict(v, line)
}

/// `", ".join(x) if x else "—"`-style: the join, or `none`.
fn joined(sep: &str, x: &Value, none: &str, line: u32) -> Result<String, Raise> {
    if truthy(x) {
        join(sep, x, line)
    } else {
        Ok(none.to_string())
    }
}

/// `x[:n]`, or the exception at `line`.
fn head(v: &Value, n: usize, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Str(t) => Ok(match t.as_str() {
            Some(text) => s(&text.chars().take(n).collect::<String>()),
            None => {
                // A lone surrogate: rebuild the string from its escapes.
                let mut lit = String::from("\"");
                for cp in t.code_points().into_iter().take(n) {
                    if cp > 0xffff {
                        let c = cp - 0x10000;
                        lit.push_str(&format!(
                            "\\u{:04x}\\u{:04x}",
                            0xd800 + (c >> 10),
                            0xdc00 + (c & 0x3ff)
                        ));
                    } else {
                        lit.push_str(&format!("\\u{cp:04x}"));
                    }
                }
                lit.push('"');
                crate::out::json::parse(&lit).expect("escaped code points are JSON")
            }
        }),
        Value::Array(items) => Ok(Value::Array(items.iter().take(n).cloned().collect())),
        Value::Object(_) => Err(Raise::at(line, format!("KeyError: slice(None, {n}, None)"))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                pyout::type_name(other)
            ),
        )),
    }
}

/// `time.strftime("%Y-%m-%d %H:%M", time.localtime(x))`.
fn minute(x: &Value) -> Result<String, String> {
    let full = activity::stamp(x, 0)?;
    Ok(full[..full.len() - 3].to_string())
}

/// `t(x)`: the shell's `def t(x)` at line 4, called from `from`.
fn t(x: &Value, from: u32) -> Result<String, Raise> {
    if !truthy(x) {
        return Ok("\u{2014}".into());
    }
    minute(x).map_err(|e| Raise::inside(from, 4, "t", e))
}

/// Python's `a == b` for JSON values.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    fn num(v: &Value) -> Option<Result<String, f64>> {
        match v {
            Value::Bool(b) => Some(Ok(if *b { "1" } else { "0" }.into())),
            Value::Int(d) => Some(Ok(d.clone())),
            Value::Float(f) => Some(Err(*f)),
            _ => None,
        }
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && py_eq(v, v2)))
        }
        _ => match (num(a), num(b)) {
            (Some(Ok(x)), Some(Ok(y))) => x == y,
            (Some(Err(x)), Some(Err(y))) => x == y,
            (Some(Ok(i)), Some(Err(f))) | (Some(Err(f)), Some(Ok(i))) => {
                f.is_finite() && f.fract() == 0.0 && format!("{f:.0}") == i
            }
            _ => false,
        },
    }
}

/// The "Waiting:" lines: `w=d.get("waitingForReset") or {}` at `line`,
/// `w.get("rooms")` at the next, the print at the one after.
fn waiting(d: &Value, line: u32, o: &mut Out) -> Result<(), Raise> {
    let w = orv(g(d, "waitingForReset"), Value::Object(vec![]));
    let w = need(&w, line + 1)?;
    if truthy(&g(w, "rooms")) {
        let rooms = join(", ", &g(w, "rooms"), line + 2)?;
        o.line(format!(
            "Waiting: everything else works now, but they can't use room {rooms} until they choose their own password, because you set this login up: first sign-in with a temporary password, an invite ({}), or your App's forgot-password flow.",
            st(&g(w, "ask"))
        ));
    }
    Ok(())
}

/// `_skin_exit_if_held`: nothing and 0, or the held lines and 2.
pub fn held(raw: &str) -> Shown {
    let src = if raw.is_empty() { "{}" } else { raw };
    let Ok(d) = crate::out::json::parse(src) else {
        return Shown::out("");
    };
    if !matches!(d, Value::Object(_)) || g(&d, "status") != s("held") {
        return Shown::out("");
    }
    let mut o = Out::default();
    let r = (|| -> Result<(), Raise> {
        let req = orv(g(&d, "request"), Value::Object(vec![]));
        let req = need(&req, 10)?;
        o.line(format!(
            "Held for the owner (request {}): {}",
            st(&orv(g(req, "id"), s("?"))),
            st(&orv(g(req, "summary"), s("")))
        ));
        for (key, line) in [("hint", 12), ("afterApproval", 14)] {
            if truthy(&g(&d, key)) {
                o.line(format!("  {}", py::str_plus(&g(&d, key), line)?));
            }
        }
        Ok(())
    })();
    match r {
        Ok(()) => Shown {
            stdout: o.o,
            stderr: String::new(),
            exit: Exit::Usage,
        },
        Err(r) => raised(o.o, String::new(), &r),
    }
}

/// `role room`'s own held check.
pub fn room_held(raw: &str) -> Shown {
    printed(raw, |d, o| {
        if !matches!(d, Value::Object(_)) || g(d, "status") != s("held") {
            return Ok(Exit::Ok);
        }
        let req = orv(g(d, "request"), Value::Object(vec![]));
        let req = need(&req, 7)?;
        o.line(format!(
            "Held for the owner (request {}): {}",
            st(&orv(g(req, "id"), s("?"))),
            st(&orv(g(req, "summary"), s("")))
        ));
        if truthy(&g(d, "hint")) {
            o.line(format!("  {}", py::str_plus(&g(d, "hint"), 9)?));
        }
        Ok(Exit::Usage)
    })
}

/// The printer for `reader`.
pub fn read(reader: &Reader, raw: &str) -> Shown {
    match reader {
        Reader::UserAdd { app } => user_add(raw, app),
        Reader::UserShow => user_show(raw),
        Reader::UserImage => one(raw, |d| {
            let fmt = if truthy(&g(d, "cleared")) {
                "Picture cleared for "
            } else {
                ""
            };
            let name = st(&g(d, "username"));
            Ok(if fmt.is_empty() {
                format!("{name} had no picture")
            } else {
                format!("{fmt}{name}")
            })
        }),
        Reader::UserEnded => one(raw, |d| {
            let ended = at_or(d, "ended", Value::Int("0".into()));
            let n = super::super::py_d(&ended, 4)?;
            Ok(format!(
                "Ended {n} session(s) of {} in App {}",
                st(&g(d, "username")),
                st(&g(d, "app"))
            ))
        }),
        Reader::UserSessions => user_sessions(raw),
        Reader::UserSessionsJson => sessions_json(raw),
        Reader::UserInvite => user_invite(raw),
        Reader::UserReset => user_reset(raw),
        Reader::UserUnlock => printed(raw, |d, o| {
            let d = need(d, 4)?;
            let key = st(&at_or(d, "username", s("")));
            if g(d, "cleared") == Value::Bool(true) {
                o.line(format!("Lock cleared for {key}"));
            } else {
                o.line(format!("No lock for {key}"));
            }
            Ok(Exit::Ok)
        }),
        Reader::UserEmail => printed(raw, |d, o| {
            let d = need(d, 4)?;
            let em = orv(g(d, "email"), s(""));
            let what = if truthy(&em) {
                format!("set to {}", st(&em))
            } else {
                "cleared".into()
            };
            o.line(format!(
                "Skin email {what} for {}",
                st(&at_or(d, "username", s("")))
            ));
            Ok(Exit::Ok)
        }),
        Reader::UserFullName => printed(raw, |d, o| {
            let d = need(d, 4)?;
            let full = orv(or2(d, "fullName", "full_name"), s(""));
            let what = if truthy(&full) {
                format!("set to {}", st(&full))
            } else {
                "cleared".into()
            };
            o.line(format!(
                "Skin full name {what} for {}",
                st(&at_or(d, "username", s("")))
            ));
            Ok(Exit::Ok)
        }),
        Reader::UserAgent => printed(raw, |d, o| {
            let d = need(d, 4)?;
            let ws = orv(or2(d, "agentHandle", "agentWorkspace"), s(""));
            let name = st(&at_or(d, "username", s("")));
            if truthy(&ws) {
                o.line(format!(
                    "App user {name} is now the agent of {} (db actor kind: agent)",
                    st(&ws)
                ));
            } else {
                o.line(format!(
                    "App user {name} is a person again (db actor kind: guest)"
                ));
            }
            Ok(Exit::Ok)
        }),
        Reader::UserRemove => one(raw, |d| {
            Ok(if truthy(&g(d, "success")) {
                "Skin user removed."
            } else {
                "No matching skin user."
            }
            .into())
        }),
        Reader::UserRooms => printed(raw, |d, o| {
            let d = need(d, 4)?;
            let name = st(&at_or(d, "username", s("")));
            if truthy(&g(d, "app")) {
                let handles = orv(g(d, "roomHandles"), Value::Array(vec![]));
                let h = joined(", ", &handles, "(none)", 6)?;
                o.line(format!(
                    "App user {name} rooms on {}: {h}",
                    st(&g(d, "app"))
                ));
            } else {
                let handles = orv(
                    or2(d, "defaultRoomHandles", "default_room_handles"),
                    Value::Array(vec![]),
                );
                let h = joined(", ", &handles, "(none)", 9)?;
                o.line(format!("Skin user {name} default rooms: {h}"));
            }
            waiting(d, 10, o)?;
            Ok(Exit::Ok)
        }),
        Reader::UserRole => printed(raw, |d, o| {
            let d = need(d, 4)?;
            let role = orv(or2(d, "roleName", "role_name"), s("\u{2014}"));
            o.line(format!(
                "Skin user {} role: {}",
                st(&at_or(d, "username", s(""))),
                st(&role)
            ));
            waiting(d, 5, o)?;
            Ok(Exit::Ok)
        }),
        Reader::UserUnassign => one(raw, |d| {
            Ok(format!(
                "Skin user {} unassigned.",
                st(&at_or(d, "username", s("")))
            ))
        }),
        Reader::RoleCreate => role_access(raw, "Skin role created: ", true),
        Reader::RoleUpdate => role_access(raw, "Skin role updated: ", false),
        Reader::RoleList => role_list(raw),
        Reader::RoleRoom => role_room(raw),
        Reader::RolePaths { handle } => role_paths(raw, handle),
        Reader::RoleRemove => one(raw, |d| {
            Ok(if truthy(&g(d, "success")) {
                "Skin role removed."
            } else {
                "No matching skin role."
            }
            .into())
        }),
        Reader::RoleAdd => role_add(raw),
        Reader::GrantList => grant_list(raw),
        Reader::GrantAdd => grant_add(raw),
        Reader::GrantRm => printed(raw, |d, o| {
            let d = need(d, 4)?;
            o.line(if truthy(&g(d, "success")) {
                "Grant removed."
            } else {
                "No matching grant."
            });
            let cascaded = orv(g(d, "cascaded"), Value::Array(vec![]));
            for c in py_iter(&cascaded, 5)? {
                let c = need(&c, 6)?;
                let f = |k: &str| st(&at_or(c, k, s("")));
                o.line(format!(
                    "  also removed {} grant {} ({} {}, scope {})",
                    f("kind"),
                    f("id"),
                    f("subjectKind"),
                    f("subjectId"),
                    f("scope")
                ));
            }
            Ok(Exit::Ok)
        }),
        Reader::GrantSet => one(raw, |d| {
            Ok(format!(
                "Grant {} scope {}",
                st(&orv(g(d, "id"), s(""))),
                st(&orv(g(d, "scope"), s("")))
            ))
        }),
        Reader::GrantRole { role } => printed(raw, |d, o| {
            let d = need(d, 4)?;
            let id = st(&orv(g(d, "id"), s("")));
            if truthy(&g(d, "changed")) {
                o.line(format!(
                    "Grant {id}  now role {role} ({} on {}). Live logins kept; they have the new role now.",
                    st(&orv(g(d, "subjectId"), s(""))),
                    st(&orv(g(d, "targetId"), s("")))
                ));
            } else {
                o.line(format!(
                    "Grant {id} already has role {role}. Nothing changed."
                ));
            }
            Ok(Exit::Ok)
        }),
        Reader::FrontDoor => front_door(raw),
        Reader::Hydra => printed(raw, |d, o| {
            let d = need(d, 4)?;
            o.line(format!("supported: {}", st(&g(d, "supported"))));
            o.line(format!("enabled:   {}", st(&g(d, "enabled"))));
            o.line(format!("running:   {}", st(&g(d, "running"))));
            o.line(format!("public:    {}", st(&orv(g(d, "publicUrl"), s("")))));
            o.line(format!("admin:     {}", st(&orv(g(d, "adminUrl"), s("")))));
            if truthy(&g(d, "hint")) {
                o.line(format!("hint:      {}", st(&g(d, "hint"))));
            }
            Ok(Exit::Ok)
        }),
    }
}

/// A printer of one line at line 4, `d.get` first.
fn one(raw: &str, line: impl FnOnce(&Value) -> Result<String, Raise>) -> Shown {
    printed(raw, |d, o| {
        let d = need(d, 4)?;
        o.line(line(d)?);
        Ok(Exit::Ok)
    })
}

fn user_add(raw: &str, app: &str) -> Shown {
    printed(raw, |d, o| {
        let name = st(&at_or(need(d, if app.is_empty() { 8 } else { 6 })?, "username", s("")));
        if app.is_empty() {
            o.line(format!("Skin user added: {name}"));
        } else {
            o.line(format!(
                "App user added: {name} (App {app} only; no box login)"
            ));
        }
        let pw = g(d, "generatedPassword");
        if truthy(&pw) {
            o.line(format!(
                concat!(
                    "Temporary password (shown once, ",
                    product_name!(),
                    " keeps no copy): {}"
                ),
                st(&pw)
            ));
            o.line(concat!(
                "Give it to them privately, never in a Thread, chat or file others can read. At their first sign-in ",
                product_name!(),
                " asks them to choose their own."
            ));
            o.line(concat!(
                "To handle no password at all: ",
                cli_name!(),
                " app user invite <username> --app <app> mails them a link."
            ));
        }
        waiting(d, 14, o)?;
        Ok(Exit::Ok)
    })
}

fn user_show(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = need(d, 5)?;
        let mut full = orv(g(d, "fullName"), s("\u{2014}"));
        if truthy(&g(d, "nameSetBySelf")) && truthy(&g(d, "fullName")) {
            full = s(&format!(
                "{} (set by {})",
                st(&full),
                st(&g(d, "username"))
            ));
        }
        o.line(format!(
            "{} in App {}",
            st(&g(d, "username")),
            st(&g(d, "app"))
        ));
        o.line(format!("  full name     {}", st(&full)));
        let dash = || s("\u{2014}");
        o.line(format!(
            "  display name  {}",
            st(&orv(g(d, "displayName"), dash()))
        ));
        o.line(format!("  email         {}", st(&orv(g(d, "email"), dash()))));
        let picture = if truthy(&g(d, "imageHash")) {
            format!("yes ({}\u{2026})", st(&head(&g(d, "imageHash"), 12, 12)?))
        } else {
            "\u{2014}".into()
        };
        o.line(format!("  picture       {picture}"));
        o.line(format!("  role here     {}", st(&orv(g(d, "role"), dash()))));
        let made = t(&g(d, "createdAt"), 14)?;
        let last = t(&g(d, "lastSignInAt"), 14)?;
        o.line(format!("  made          {made}    last sign-in {last}"));
        let set = orv(g(d, "settings"), Value::Object(vec![]));
        let set = need(&set, 16)?;
        let on = |k: &str| if truthy(&g(set, k)) { "on" } else { "off" };
        o.line(format!(
            "  may edit      names {}, picture {}",
            on("editNames"),
            on("editImage")
        ));
        let rows = orv(g(d, "sessions"), Value::Array(vec![]));
        o.line(format!("  sessions      {}", py_len(&rows, 18)?));
        for r in py_iter(&rows, 19)? {
            let r = need(&r, 20)?;
            let client = head(&orv(g(r, "client"), dash()), 24, 20)?;
            let when = t(&or2(r, "lastSeenAt", "createdAt"), 20)?;
            o.line(format!(
                "    {} {} {} last {when}{}",
                py_left(&g(r, "id"), 38),
                py_left(&g(r, "kind"), 7),
                py_left(&client, 24),
                push(r)
            ));
        }
        Ok(Exit::Ok)
    })
}

/// `"  push:%s" % r.get("pushPlatform") if r.get("push") else ""`.
fn push(r: &Value) -> String {
    if truthy(&g(r, "push")) {
        format!("  push:{}", st(&g(r, "pushPlatform")))
    } else {
        String::new()
    }
}

fn user_sessions(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = need(d, 5)?;
        let rows = orv(g(d, "sessions"), Value::Array(vec![]));
        if !truthy(&rows) {
            o.line(format!(
                "{} has no live sessions in App {}",
                st(&g(d, "username")),
                st(&g(d, "app"))
            ));
            return Ok(Exit::Ok);
        }
        for r in py_iter(&rows, 8)? {
            let r = need(&r, 9)?;
            let client = head(&orv(g(r, "client"), s("\u{2014}")), 24, 9)?;
            let since = t(&g(r, "createdAt"), 9)?;
            let last = t(&g(r, "lastSeenAt"), 9)?;
            o.line(format!(
                "{} {} {} since {since}  last {last}{}",
                py_left(&g(r, "id"), 38),
                py_left(&g(r, "kind"), 7),
                py_left(&client, 24),
                push(r)
            ));
        }
        Ok(Exit::Ok)
    })
}

/// `user sessions --json`: one line of Python.
fn sessions_json(raw: &str) -> Shown {
    let r = load(raw, 1).and_then(|d| {
        let d = need(&d, 1)?.clone();
        let rows = orv(g(&d, "sessions"), Value::Array(vec![]));
        Ok(py::dumps(&py::obj(vec![
            ("ok", Value::Bool(true)),
            ("sessions", rows),
        ])))
    });
    match r {
        Ok(line) => Shown::out(line + "\n"),
        Err(r) => raised(String::new(), String::new(), &r),
    }
}

/// `x // 3600` for the value Python reads, or the `TypeError` at `line`.
fn hours(v: &Value, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Bool(_) => Ok(Value::Int("0".into())),
        Value::Int(digits) => Ok(Value::Int(int_floor_div(digits, 3600))),
        Value::Float(f) => Ok(Value::Float(float_floor_div(*f, 3600.0))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for //: '{}' and 'int'",
                pyout::type_name(other)
            ),
        )),
    }
}

/// Floor division of a decimal integer by a small positive divisor.
fn int_floor_div(digits: &str, by: u64) -> String {
    let (neg, mag) = match digits.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, digits),
    };
    let mut q = String::new();
    let mut rem: u64 = 0;
    for c in mag.bytes() {
        rem = rem * 10 + u64::from(c - b'0');
        let d = rem / by;
        rem %= by;
        if !(q.is_empty() && d == 0) {
            q.push(char::from(b'0' + d as u8));
        }
    }
    if q.is_empty() {
        q.push('0');
    }
    if !neg {
        return q;
    }
    if rem != 0 {
        q = add_one(&q);
    }
    if q == "0" {
        q
    } else {
        format!("-{q}")
    }
}

fn add_one(digits: &str) -> String {
    let mut b: Vec<u8> = digits.bytes().collect();
    let mut i = b.len();
    loop {
        if i == 0 {
            b.insert(0, b'1');
            break;
        }
        i -= 1;
        if b[i] == b'9' {
            b[i] = b'0';
        } else {
            b[i] += 1;
            break;
        }
    }
    String::from_utf8(b).expect("digits")
}

/// CPython's `float.__floordiv__`.
fn float_floor_div(vx: f64, wx: f64) -> f64 {
    let m = vx % wx;
    let mut div = (vx - m) / wx;
    if m != 0.0 && ((wx < 0.0) != (m < 0.0)) {
        div -= 1.0;
    }
    if div != 0.0 {
        let mut f = div.floor();
        if div - f > 0.5 {
            f += 1.0;
        }
        f
    } else {
        (0.0f64).copysign(vx / wx)
    }
}

fn user_invite(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = need(d, 4)?;
        let how = g(d, "delivered");
        if how == s("sent") {
            let h = hours(&at_or(d, "ttlSecs", Value::Int("0".into())), 6)?;
            o.line(format!(
                "Invite sent to {} from {} (the link works once; it expires in {} hours)",
                st(&g(d, "sentTo")),
                st(&g(d, "from")),
                st(&orv(h, s("<1")))
            ));
        } else if how == s("held") {
            o.line(st(&orv(g(d, "hint"), s("Invite held for approval."))));
        } else {
            o.line(format!(
                "No mail path on this box: give {} this one-time link yourself (shown once):\n{}",
                st(&g(d, "username")),
                st(&g(d, "link"))
            ));
        }
        waiting(d, 11, o)?;
        Ok(Exit::Ok)
    })
}

fn user_reset(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = need(d, 4)?;
        let how = g(d, "delivered");
        if at(d, "revoked").is_some() {
            o.line(format!(
                "Took back {} unused link(s) for {} on that App",
                st(&g(d, "revoked")),
                st(&g(d, "username"))
            ));
        } else if how == s("sent") {
            o.line(format!(
                "Reset link sent to {} (works once; when it is used, {} gets everything their role names)",
                st(&g(d, "sentTo")),
                st(&g(d, "username"))
            ));
        } else if how == s("held") {
            o.line(st(&orv(g(d, "hint"), s("Reset link held for approval."))));
        } else {
            o.line(format!(
                "One-time reset link for {} (shown once; give it to them yourself):\n{}",
                st(&g(d, "username")),
                st(&g(d, "link"))
            ));
        }
        Ok(Exit::Ok)
    })
}

/// A dict key Python can hash, or the `TypeError` at `line`.
fn hashable(v: &Value, line: u32) -> Result<(), Raise> {
    match v {
        Value::Array(_) | Value::Object(_) => Err(Raise::at(
            line,
            format!("TypeError: unhashable type: '{}'", pyout::type_name(v)),
        )),
        _ => Ok(()),
    }
}

/// `sys.stdin.readline()`: up to and with the next newline.
fn readline(rest: &mut &str) -> String {
    match rest.find('\n') {
        Some(i) => {
            let line = rest[..=i].to_string();
            *rest = &rest[i + 1..];
            line
        }
        None => {
            let line = rest.to_string();
            *rest = "";
            line
        }
    }
}

/// `user list`'s printer, on both answers, each with a newline after it.
pub fn user_list(stream: &str) -> Shown {
    let mut rest = stream;
    let mut out = Out::default();
    let r = (|| -> Result<(), Raise> {
        let o = &mut out;
        let first = readline(&mut rest);
        let first = if first.is_empty() { "{}".into() } else { first };
        let doc = load(&first, 3)?;
        let users = at_or(need(&doc, 3)?, "users", Value::Array(vec![]));
        let second = readline(&mut rest);
        let second = if second.is_empty() { "{}".into() } else { second };
        let doc = load(&second, 4)?;
        let mut roles: Vec<(Value, Value)> = Vec::new();
        for r in py_iter(&at_or(need(&doc, 4)?, "roles", Value::Array(vec![])), 4)? {
            let key = orv(g(need(&r, 4)?, "name"), s(""));
            hashable(&key, 4)?;
            match roles.iter_mut().find(|(k, _)| py_eq(k, &key)) {
                Some(slot) => slot.1 = r.clone(),
                None => roles.push((key, r.clone())),
            }
        }
        if !truthy(&users) {
            o.line("No skin users.");
            return Ok(());
        }
        let dash = || s("\u{2014}");
        for u in py_iter(&users, 7)? {
            let u = need(&u, 8)?;
            let name = orv(g(u, "username"), s(""));
            let mut role = orv(or2(u, "roleName", "role_name"), dash());
            let handles = if truthy(&or2(u, "roleName", "role_name")) {
                hashable(&role, 11)?;
                let r = roles
                    .iter()
                    .find(|(k, _)| py_eq(k, &role))
                    .map(|(_, v)| v.clone())
                    .unwrap_or(Value::Null);
                let r = orv(r, Value::Object(vec![]));
                let r = need(&r, 12)?;
                orv(or2(r, "roomHandles", "room_handles"), Value::Array(vec![]))
            } else {
                orv(
                    or2(u, "defaultRoomHandles", "default_room_handles"),
                    Value::Array(vec![]),
                )
            };
            let mut rooms = joined(", ", &handles, "\u{2014}", 15)?;
            if role == dash() && rooms == "\u{2014}" {
                let mut gs = Vec::new();
                for gr in py_iter(&orv(g(u, "appGrants"), Value::Array(vec![])), 18)? {
                    let gd = need(&gr, 18)?;
                    if truthy(&g(gd, "role")) || truthy(&g(gd, "rooms")) {
                        gs.push(gr.clone());
                    }
                }
                if !gs.is_empty() {
                    let parts: Vec<String> = gs
                        .iter()
                        .filter(|gr| truthy(&g(gr, "role")))
                        .map(|gr| {
                            format!(
                                "{} (App {})",
                                st(&g(gr, "role")),
                                st(&or2(gr, "app", "appServiceId"))
                            )
                        })
                        .collect();
                    role = orv(s(&parts.join(", ")), dash());
                    let mut rh: Vec<Value> = Vec::new();
                    for gr in &gs {
                        for h in py_iter(&orv(g(gr, "rooms"), Value::Array(vec![])), 23)? {
                            if !rh.iter().any(|x| py_eq(x, &h)) {
                                rh.push(h);
                            }
                        }
                    }
                    let rh = Value::Array(rh);
                    rooms = joined(", ", &rh, "\u{2014}", 26)?;
                }
            }
            let email = orv(g(u, "email"), dash());
            let mut full = orv(or2(u, "fullName", "full_name"), dash());
            if truthy(&g(u, "nameSetBySelf")) && full != dash() {
                full = s(&format!("{} (set by {})", st(&full), st(&name)));
            }
            let agent = orv(or2(u, "agentHandle", "agentWorkspace"), s(""));
            let mut tail = if truthy(&agent) {
                format!("  agent:{}", st(&agent))
            } else {
                String::new()
            };
            let test = g(u, "test");
            if truthy(&test) {
                let tt = need(&test, 35)?;
                let app = or2(tt, "app", "appServiceId");
                let when = if truthy(&g(tt, "expired")) {
                    "expired".to_string()
                } else {
                    let at = orv(g(tt, "expiresAt"), Value::Int("0".into()));
                    format!("ends {}", minute(&at).map_err(|e| Raise::at(35, e))?)
                };
                tail.push_str(&format!("  [test: App {}, {when}]", st(&app)));
            }
            if truthy(&g(u, "tiedTo")) {
                tail.push_str(&format!(
                    "  [{}: set up by {}'s agent]",
                    if truthy(&g(u, "pendingReset")) {
                        "waiting for reset"
                    } else {
                        "tied"
                    },
                    st(&g(u, "tiedTo"))
                ));
            }
            o.line(format!(
                "{}  {}  {}  {rooms}  {}{tail}",
                st(&name),
                st(&full),
                st(&role),
                st(&email)
            ));
        }
        Ok(())
    })();
    match r {
        Ok(()) => Shown::out(out.o),
        Err(r) => raised(out.o, String::new(), &r),
    }
}

/// `", ".join(a.get("caps") or []) or "thread-only"`.
fn caps(a: &Value, sep: &str, line: u32) -> Result<String, Raise> {
    let c = join(sep, &orv(g(a, "caps"), Value::Array(vec![])), line)?;
    Ok(if c.is_empty() {
        "thread-only".into()
    } else {
        c
    })
}

/// `role create` (`created`) and `role update`.
fn role_access(raw: &str, head_text: &str, created: bool) -> Shown {
    printed(raw, |d, o| {
        let d = need(d, 4)?;
        let access = orv(or2(d, "roomAccess", "room_access"), Value::Array(vec![]));
        let name = if created {
            st(&at_or(d, "name", s("")))
        } else {
            st(&orv(g(d, "name"), s("")))
        };
        if created {
            if !truthy(&access) {
                o.line(format!("{head_text}{name}  (none)"));
                return Ok(Exit::Ok);
            }
            o.line(format!("{head_text}{name}"));
        } else {
            o.line(format!("{head_text}{name}"));
            if !truthy(&access) {
                o.line("  (none)");
                return Ok(Exit::Ok);
            }
        }
        for a in py_iter(&access, 9)? {
            let a = need(&a, 10)?;
            let c = caps(a, ", ", 10)?;
            o.line(format!("  {}  {c}", st(&orv(g(a, "handle"), s("")))));
        }
        Ok(Exit::Ok)
    })
}

fn role_list(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let roles = at_or(need(d, 4)?, "roles", Value::Array(vec![]));
        if !truthy(&roles) {
            o.line("No skin roles.");
            return Ok(Exit::Ok);
        }
        for r in py_iter(&roles, 7)? {
            let r = need(&r, 8)?;
            o.line(st(&orv(g(r, "name"), s(""))));
            let access = orv(or2(r, "roomAccess", "room_access"), Value::Array(vec![]));
            if !truthy(&access) {
                o.line("  (none)");
                continue;
            }
            for a in py_iter(&access, 13)? {
                let a = need(&a, 14)?;
                let c = caps(a, ",", 14)?;
                o.line(format!("  {}  {c}", st(&orv(g(a, "handle"), s("")))));
                let f = orv(g(a, "folders"), Value::Object(vec![]));
                let f = need(&f, 17)?;
                for (key, label, line) in [("read", "read:  ", 18), ("write", "write: ", 20)] {
                    if truthy(&g(f, key)) {
                        o.line(format!("      {label} {}", join(", ", &g(f, key), line)?));
                    }
                }
                if truthy(&g(f, "uploadDir")) {
                    o.line(format!("      upload: {}", st(&g(f, "uploadDir"))));
                }
                if truthy(&g(f, "hide")) {
                    o.line(format!("      hidden: {}", join(", ", &g(f, "hide"), 24)?));
                }
            }
        }
        Ok(Exit::Ok)
    })
}

/// `[w + suffix for w in (x or [])]` at `line`.
fn suffixed(x: &Value, suffix: &str, line: u32) -> Result<Vec<Value>, Raise> {
    py_iter(&orv(x.clone(), Value::Array(vec![])), line)?
        .iter()
        .map(|w| {
            let w = plus_str(w, line).map_err(|_| {
                Raise::at(
                    line,
                    match w {
                        Value::Array(_) => {
                            "TypeError: can only concatenate list (not \"str\") to list".to_string()
                        }
                        other => format!(
                            "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                            pyout::type_name(other)
                        ),
                    },
                )
            })?;
            Ok(s(&(w + suffix)))
        })
        .collect()
}

/// `a + b` where `b` is a list, at `line`.
fn plus_list(a: Value, b: Vec<Value>, line: u32) -> Result<Value, Raise> {
    match a {
        Value::Array(mut items) => {
            items.extend(b);
            Ok(Value::Array(items))
        }
        Value::Str(_) => Err(Raise::at(
            line,
            "TypeError: can only concatenate str (not \"list\") to str",
        )),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'list'",
                pyout::type_name(&other)
            ),
        )),
    }
}

fn role_room(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = need(d, 4)?;
        o.line(format!("Skin role {}", st(&orv(g(d, "name"), s("")))));
        let access = orv(or2(d, "roomAccess", "room_access"), Value::Array(vec![]));
        if !truthy(&access) {
            o.line("  (none)");
            return Ok(Exit::Ok);
        }
        for a in py_iter(&access, 9)? {
            let a = need(&a, 10)?;
            let c = caps(a, ", ", 10)?;
            o.line(format!("  {}  {c}", st(&orv(g(a, "handle"), s("")))));
            let f = orv(g(a, "folders"), Value::Object(vec![]));
            let f = need(&f, 13)?;
            if ["read", "write", "upload", "download"]
                .iter()
                .any(|k| truthy(&g(f, k)))
            {
                let read = orv(g(f, "read"), Value::Array(vec![]));
                let w = suffixed(&g(f, "write"), " (write)", 14)?;
                let all = plus_list(read, w, 14)?;
                let u = suffixed(&g(f, "upload"), " (drop)", 14)?;
                let all = plus_list(all, u, 14)?;
                let x = suffixed(&g(f, "download"), " (save)", 14)?;
                let all = plus_list(all, x, 14)?;
                o.line(format!("      folders: {}", join(", ", &all, 14)?));
            }
            if truthy(&g(f, "hide")) {
                o.line(format!("      hidden: {}", join(", ", &g(f, "hide"), 16)?));
            }
        }
        Ok(Exit::Ok)
    })
}

fn role_paths(raw: &str, want: &str) -> Shown {
    printed(raw, |d, o| {
        let d = need(d, 5)?;
        o.line(format!("Skin role {}", st(&orv(g(d, "name"), s("")))));
        let access = orv(or2(d, "roomAccess", "room_access"), Value::Array(vec![]));
        for a in py_iter(&access, 6)? {
            let a = need(&a, 7)?;
            if !py_eq(&g(a, "handle"), &s(want)) {
                continue;
            }
            let f = orv(g(a, "folders"), Value::Object(vec![]));
            let c = caps(a, ", ", 10)?;
            o.line(format!("  {}  {c}", st(&orv(g(a, "handle"), s("")))));
            let f = need(&f, 12)?;
            if !["read", "write", "upload"].iter().any(|k| truthy(&g(f, k))) {
                o.line("      folders: whole room");
            }
            let lines: [(&str, &str, &str, u32); 3] = [
                ("read", "read:   ", "", 15),
                ("write", "write:  ", "", 17),
                (
                    "upload",
                    "drop:   ",
                    " (files:upload: add only, never list or read)",
                    19,
                ),
            ];
            for (key, label, tail, line) in lines {
                if truthy(&g(f, key)) {
                    o.line(format!("      {label}{}{tail}", join(", ", &g(f, key), line)?));
                }
            }
            if truthy(&g(f, "uploadDir")) {
                o.line(format!("      upload: {}", st(&g(f, "uploadDir"))));
            }
            if truthy(&g(f, "download")) {
                o.line(format!(
                    "      save:   {} (files:download: may save these)",
                    join(", ", &g(f, "download"), 23)?
                ));
            }
            if truthy(&g(f, "hide")) {
                o.line(format!(
                    "      hidden: {} (never listed or read, even inside a folder)",
                    join(", ", &g(f, "hide"), 25)?
                ));
            }
        }
        Ok(Exit::Ok)
    })
}

fn role_add(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = need(d, 4)?;
        let r = orv(g(d, "role"), Value::Object(vec![]));
        let r = need(&r, 5)?;
        o.line(format!(
            "Role {} made from {} (edit it like any role: {} app role room|paths|update {})",
            st(&g(r, "name")),
            st(&g(d, "template")),
            cli_name!(),
            st(&g(r, "name"))
        ));
        for line in py_iter(&orv(g(r, "roomAccess"), Value::Array(vec![])), 6)? {
            let line = need(&line, 7)?;
            let c = join(", ", &orv(g(line, "caps"), Value::Array(vec![])), 7)?;
            o.line(format!("  {} {c}", py_left(&g(line, "handle"), 20)));
        }
        Ok(Exit::Ok)
    })
}

/// `"<text>" + v`, at `line`.
fn cat(text: &str, v: &Value, line: u32) -> Result<String, Raise> {
    Ok(format!("{text}{}", py::str_plus(v, line)?))
}

fn grant_list(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = need(d, 4)?;
        let grants = orv(g(d, "grants"), Value::Array(vec![]));
        if !truthy(&grants) {
            o.line(if truthy(&g(d, "scope")) {
                "No logins on your Apps."
            } else {
                "No grants."
            });
            return Ok(Exit::Ok);
        }
        for gr in py_iter(&grants, 8)? {
            if truthy(&g(d, "scope")) {
                let gr = need(&gr, 10)?;
                let what = if truthy(&g(gr, "role")) {
                    cat("role ", &g(gr, "role"), 10)?
                } else {
                    format!(
                        "rooms {}",
                        join(",", &orv(g(gr, "rooms"), Value::Array(vec![])), 10)?
                    )
                };
                let enabled = at_or(gr, "enabled", Value::Bool(true));
                o.line(format!(
                    "{}  {} on {}  {what}{}",
                    st(&orv(g(gr, "id"), s(""))),
                    st(&orv(or2(gr, "username", "subjectId"), s(""))),
                    st(&orv(or2(gr, "app", "targetId"), s(""))),
                    if truthy(&enabled) { "" } else { "  [off]" }
                ));
                continue;
            }
            let gr = need(&gr, 13)?;
            let role = orv(or2(gr, "roleId", "role_id"), s(""));
            let scope = orv(g(gr, "scope"), s(""));
            let mut extra = if truthy(&role) {
                cat("role ", &role, 15)?
            } else if truthy(&scope) {
                cat("scope ", &scope, 15)?
            } else {
                String::new()
            };
            let hs = g(gr, "hookState");
            if truthy(&hs) {
                let err = if truthy(&g(gr, "hookError")) {
                    cat(": ", &g(gr, "hookError"), 18)?
                } else {
                    String::new()
                };
                extra.push_str(&format!("  [{}{err}]", st(&hs)));
            }
            o.line(format!(
                "{}  {} {}  {} {}  {extra}",
                st(&orv(g(gr, "id"), s(""))),
                st(&orv(or2(gr, "subjectKind", "subject_kind"), s(""))),
                st(&orv(or2(gr, "subjectId", "subject_id"), s(""))),
                st(&orv(g(gr, "kind"), s(""))),
                st(&orv(or2(gr, "targetId", "target_id"), s(""))),
            ));
        }
        Ok(Exit::Ok)
    })
}

fn grant_add(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = need(d, 5)?;
        let chat = g(d, "kind") == s("chat");
        let scope = if chat && truthy(&g(d, "scope")) {
            cat("  scope ", &g(d, "scope"), 10)?
        } else {
            String::new()
        };
        o.line(format!(
            "Grant {}  {} {}  {} {}{scope}",
            st(&orv(g(d, "id"), s(""))),
            st(&orv(g(d, "subjectKind"), s(""))),
            st(&orv(g(d, "subjectId"), s(""))),
            st(&orv(g(d, "kind"), s(""))),
            st(&orv(g(d, "targetId"), s(""))),
        ));
        if chat {
            o.line(concat!(
                "Queued for chat. Check with: ",
                cli_name!(),
                " app grant hooks"
            ));
        }
        Ok(Exit::Ok)
    })
}

fn front_door(raw: &str) -> Shown {
    printed(raw, |d, o| {
        let d = need(d, 4)?;
        let err = g(d, "error");
        o.line(format!("mode:     {}", st(&orv(g(d, "mode"), s("-")))));
        o.line(format!(
            "url:      {}",
            st(&orv(or2(d, "url", "connectUrl"), s("(none)")))
        ));
        o.line(format!("listen:   {}", st(&orv(g(d, "listen"), s("-")))));
        o.line(format!("applied:  {}", st(&g(d, "applied"))));
        let c = orv(g(d, "caddy"), Value::Object(vec![]));
        let c = need(&c, 10)?;
        if truthy(&g(c, "missing")) {
            o.line("caddy:    missing \u{2014} brew install caddy  or  sudo apt install caddy");
        } else {
            o.line(format!(
                "caddy:    running={} pid={}",
                st(&g(c, "running")),
                st(&g(c, "pid"))
            ));
        }
        let n = orv(g(d, "nested"), Value::Object(vec![]));
        let n = need(&n, 15)?;
        o.line(format!(
            "nested:   {} registered={}",
            st(&orv(or2(n, "host", "label"), s("-"))),
            st(&g(n, "registered"))
        ));
        if truthy(&g(d, "hint")) {
            o.line(format!("hint:     {}", st(&g(d, "hint"))));
        }
        if let Value::Str(_) = err {
            if !py::strip(&st(&err)).is_empty() {
                o.err(format!("error:    {}", st(&err)));
            }
        }
        Ok(Exit::Ok)
    })
}

/// A string error that names a token, "forbidden" or the owner: 3, else 1.
fn str_err_exit(err: &str) -> Exit {
    let low = err.to_lowercase();
    if ["token", "forbidden", "owner"].iter().any(|w| low.contains(w)) {
        Exit::Refused
    } else {
        Exit::Error
    }
}

const OWNER_CODES: [&str; 3] = ["owner_only", "forbidden", "gated"];

fn owner_code(code: &Value) -> bool {
    OWNER_CODES.iter().any(|c| py_eq(code, &s(c)))
}

/// `front-door connect|direct --json`: the exit the shell's Python picks
/// after the answer is printed.
pub fn front_door_json_exit(raw: &str) -> Shown {
    let Ok(d) = crate::out::json::parse(raw) else {
        return Shown::err("", Exit::Error);
    };
    let r = (|| -> Ends {
        let d = need(&d, 7)?;
        let err = g(d, "error");
        if let Value::Object(_) = err {
            let code = orv(g(&err, "code"), s(""));
            return Ok(if owner_code(&code) {
                Exit::Refused
            } else {
                Exit::Error
            });
        }
        if let Value::Str(_) = err {
            let e = st(&err);
            if !py::strip(&e).is_empty() && !truthy(&g(d, "applied")) {
                return Ok(str_err_exit(&e));
            }
        }
        Ok(Exit::Ok)
    })();
    match r {
        Ok(exit) => Shown::err("", exit),
        Err(r) => raised(String::new(), String::new(), &r),
    }
}

/// `front-door connect|direct`'s printer.
pub fn front_door_set(raw: &str) -> Shown {
    let d = match load(raw, 4) {
        Ok(d) => d,
        Err(r) => {
            let why = r
                .exception
                .strip_prefix("json.decoder.JSONDecodeError: ")
                .unwrap_or(&r.exception)
                .to_string();
            return Shown::err(format!("Error: {why}\n"), Exit::Error);
        }
    };
    let mut o = Out::default();
    let r = (|| -> Ends {
        let d = need(&d, 7)?;
        let err = g(d, "error");
        if let Value::Object(_) = err {
            let code = orv(g(&err, "code"), s("error"));
            let mut hint = orv(g(&err, "hint"), orv(g(&err, "message"), code.clone()));
            let owner = owner_code(&code);
            if owner && !py_in("ask your human", &hint, 11)? {
                let head = if truthy(&hint) {
                    plus_str(&hint, 12)? + " \u{2014} "
                } else {
                    String::new()
                };
                hint = s(&(head + "requires owner/admin \u{2014} ask your human"));
            }
            o.err(format!("Error: {}", st(&hint)));
            return Ok(if owner { Exit::Refused } else { Exit::Error });
        }
        let text = match &err {
            Value::Str(_) => Some(st(&err)),
            _ => None,
        };
        let live = text.as_deref().filter(|e| !py::strip(e).is_empty());
        let applied = truthy(&g(d, "applied"));
        if let Some(e) = live {
            if !applied && !truthy(&g(d, "mode")) {
                o.err(format!("Error: {e}"));
                return Ok(str_err_exit(e));
            }
            if ["caddy_missing", "bind_failed", "pro_required", "no_token"]
                .iter()
                .any(|w| e.contains(w))
                && !applied
            {
                o.err(format!("Error: {e}"));
                return Ok(Exit::Error);
            }
        }
        o.line(format!(
            "Front door: {}  {}",
            st(&at_or(d, "mode", s("?"))),
            st(&orv(or2(d, "url", "connectUrl"), s("(no url yet)")))
        ));
        o.line(format!(
            "listen:  {}  applied={}",
            st(&orv(g(d, "listen"), s("-"))),
            st(&g(d, "applied"))
        ));
        if truthy(&g(d, "hint")) {
            o.line(st(&g(d, "hint")));
        }
        if let Some(e) = live {
            o.err(e);
            if !applied {
                return Ok(Exit::Error);
            }
        }
        Ok(Exit::Ok)
    })();
    match r {
        Ok(exit) => Shown {
            stdout: o.o,
            stderr: o.e,
            exit,
        },
        Err(r) => raised(o.o, o.e, &r),
    }
}
