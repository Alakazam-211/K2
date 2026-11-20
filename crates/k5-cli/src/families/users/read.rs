//! The shell's readers for `k5 users` (each a `python3 -c` block in
//! `cmd_users`), and the bodies its Python builds. Each reader takes the
//! answer as `printf '%s' "$resp"` hands it on and prints what the Python
//! prints: stdout, stderr, the exit, and Python's traceback where it
//! raises on an answer of a shape it doesn't expect.

use super::{s, str_opt, Add};
use crate::exit::Exit;
use crate::families::tunnel::Out;
use crate::families::{get, ljust, py_get, py_iter, py_space, Raise, Shown};
use crate::out::json::{self, Options, Value};
use crate::out::py;
use crate::prompt::Secret;
use k2_client::{cli_name, product_name};

/// The answer as the reader's `sys.stdin.read()` has it.
pub fn text(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

/// `raw[:200]`: the first 200 code points.
pub fn head(t: &str) -> String {
    t.chars().take(200).collect()
}

/// `json.loads(raw) if raw.strip() else {}`; `None` where it raises.
pub fn loads(t: &str) -> Option<Value> {
    if t.trim_matches(py_space).is_empty() {
        return Some(Value::Object(Vec::new()));
    }
    json::parse(t).ok()
}

/// `sys.stderr.write("<prefix>%r\n" % (raw[:200],)); sys.exit(1)`.
pub fn unparseable(prefix: &str, t: &str) -> Shown {
    Shown::err(format!("{prefix}{}\n", py::repr(&s(&head(t)))), Exit::Error)
}

const UNPARSEABLE: &str = "Error: daemon returned an unparseable response: ";

/// `x or default`, as a value.
pub fn or(x: Option<&Value>, default: Value) -> Value {
    match x {
        Some(v) if py::truthy(v) => v.clone(),
        _ => default,
    }
}

/// `a or b` where `b` is a `.get` too: `b`'s value, or `None`.
pub fn or_get(a: Option<&Value>, b: Option<&Value>) -> Value {
    or(a, b.cloned().unwrap_or(Value::Null))
}

fn truthy(x: Option<&Value>) -> bool {
    x.is_some_and(py::truthy)
}

/// `v + " "` and the like: `v` must be a string.
pub fn plus(v: &Value, tail: &str, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(format!("{}{tail}", py::str_of(v))),
        Value::Array(_) => Err(Raise::at(
            line,
            "TypeError: can only concatenate list (not \"str\") to list",
        )),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                py::type_name(other)
            ),
        )),
    }
}

/// `"<text>" + v`: `v` must be a string.
pub fn str_plus(text: &str, v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(format!("{text}{}", py::str_of(v))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// `for x in (v or [])`.
fn items(v: Option<&Value>, line: u32) -> Result<Vec<Value>, Raise> {
    match v {
        Some(x) if py::truthy(x) => py_iter(x, line),
        _ => Ok(Vec::new()),
    }
}

/// `", ".join(str(a.get(field) or (a.get("id", "?") + " (gone)")) for a in
/// (list or [])) or "none"`, or with `gone` off `str(a.get(field) or
/// a.get("id"))`.
pub fn shared(list: Option<&Value>, field: &str, gone: bool, line: u32) -> Result<String, Raise> {
    let mut names = Vec::new();
    for a in items(list, line)? {
        let named = py_get(&a, field, line)?;
        let name = if truthy(named) {
            py::str_of(named.unwrap_or(&Value::Null))
        } else if gone {
            let id = py_get(&a, "id", line)?.cloned().unwrap_or_else(|| s("?"));
            plus(&id, " (gone)", line)?
        } else {
            str_opt(py_get(&a, "id", line)?)
        };
        names.push(name);
    }
    let joined = names.join(", ");
    Ok(if joined.is_empty() {
        "none".into()
    } else {
        joined
    })
}

/// `[a for a in env.split("\n") if a.strip()]`.
pub fn kept(lines: &str) -> Vec<String> {
    lines
        .split('\n')
        .filter(|a| !a.trim_matches(py_space).is_empty())
        .map(str::to_string)
        .collect()
}

/// [`kept`] as a JSON list.
pub fn listed(lines: &str) -> Value {
    Value::Array(kept(lines).iter().map(|a| s(a)).collect())
}

/// `add`'s body, as its Python builds it from the password on stdin.
pub fn add_body(a: &Add, pw: &Secret) -> String {
    // Only member and voice_chat ride the add (`body_role`).
    let role = match a.role.as_str() {
        r @ ("member" | "voice_chat") => r,
        _ => "",
    };
    let mut b = vec![
        ("username", s(&a.username)),
        ("password", super::pw_value(pw)),
    ];
    if role == "voice_chat" {
        if a.keep {
            b.push(("mustChangePassword", Value::Bool(false)));
        }
    } else {
        b.push(("mustChangePassword", Value::Bool(!a.keep && a.fixed != "1")));
    }
    if !role.is_empty() {
        b.push(("role", s(role)));
    }
    for (key, list) in [
        ("shareAgents", &a.agents),
        ("shareGardens", &a.gardens),
        ("shareSources", &a.sources),
    ] {
        if !kept(list).is_empty() {
            b.push((key, listed(list)));
        }
    }
    match a.fixed {
        "1" => b.push(("fixedPassword", Value::Bool(true))),
        "0" => b.push(("fixedPassword", Value::Bool(false))),
        _ => {}
    }
    if a.force {
        b.push(("force", Value::Bool(true)));
    }
    crate::confirm::dumps(&crate::confirm::object(b))
}

/// `_users_post_checked`'s first reader: the secret id of an owner-login
/// check the daemon asks for, or empty.
pub fn presence_secret(resp: &[u8]) -> String {
    let t = text(resp);
    let d = json::parse(if t.is_empty() { "{}" } else { &t }).unwrap_or(Value::Object(Vec::new()));
    let Value::Object(pairs) = &d else {
        return String::new();
    };
    let (Some(Value::Object(e)), Some(Value::Object(c))) =
        (get(pairs, "error"), get(pairs, "confirm"))
    else {
        return String::new();
    };
    let is = |p: &[(json::Text, Value)], k: &str, want: &str| matches!(get(p, k), Some(Value::Str(t)) if t.as_str() == Some(want));
    if !is(e, "code", "vault_presence_required") || !is(c, "action", super::OWNER_LOGIN) {
        return String::new();
    }
    let sid = py::str_of(&or(get(c, "secretId"), s("")));
    sid.trim_end_matches('\n').to_string()
}

/// The readers' common end: what was printed, and the exit or the
/// traceback.
fn done(o: Out, r: Result<Exit, Raise>) -> Shown {
    o.done(r)
}

/// `list [--json]`'s reader. `agent`: the caller has an agent passport.
pub fn list(raw: &[u8], as_json: bool, agent: bool) -> Shown {
    let t = text(raw);
    let Some(d) = loads(&t) else {
        return unparseable(UNPARSEABLE, &t);
    };
    let Value::Object(pairs) = &d else {
        return Shown::err("Error: unexpected response from /cli/users\n", Exit::Error);
    };
    let mut o = Out::default();
    let r = list_body(&mut o, pairs, as_json, agent);
    done(o, r)
}

const REFUSED_WORDS: [&str; 7] = [
    "owner",
    "admin",
    "forbidden",
    "token",
    "not allowed",
    "role_required",
    "gated",
];

fn list_body(
    o: &mut Out,
    pairs: &[(json::Text, Value)],
    as_json: bool,
    agent: bool,
) -> Result<Exit, Raise> {
    if let Some(err) = get(pairs, "error").filter(|e| **e != Value::Null) {
        let (text, code) = match err {
            Value::Object(ep) => {
                let text = or(get(ep, "hint"), or(get(ep, "code"), s(&py::str_of(err))));
                (text, get(ep, "code").cloned().unwrap_or_else(|| s("")))
            }
            other => (s(&py::str_of(other)), s("")),
        };
        let low = str_plus(&plus(&code, " ", 15)?, &text, 15)?.to_lowercase();
        let refused = REFUSED_WORDS.iter().any(|w| low.contains(w));
        let shown = py::str_of(&text);
        if refused && agent {
            o.err(&format!(
                "Error: owner_only: Owner/admin only; agents use `{} connections list --users`. (daemon: {shown})",
                cli_name!()
            ));
            return Ok(Exit::Refused);
        }
        o.err(&format!("Error: {shown}"));
        return Ok(if refused { Exit::Refused } else { Exit::Error });
    }
    let users = or(get(pairs, "users"), Value::Array(Vec::new()));
    if as_json {
        let mut out = Vec::new();
        for u in py_iter(&users, 52)? {
            let Value::Object(up) = &u else { continue };
            let at = |k: &str| get(up, k);
            let username = py::str_of(&or(at("username"), s("")));
            let field = |k: &str| at(k).cloned().unwrap_or(Value::Null);
            let shares = or(
                at("shares"),
                crate::confirm::object(vec![
                    ("agents", Value::Array(Vec::new())),
                    ("gardens", Value::Array(Vec::new())),
                ]),
            );
            out.push(crate::confirm::object(vec![
                ("username", s(&username)),
                ("role", or(at("role"), s("member"))),
                ("disabled", Value::Bool(truthy(at("disabled")))),
                ("wasViewer", Value::Bool(truthy(at("wasViewer")))),
                ("canClone", field("canClone")),
                ("createdAt", or_get(at("createdAt"), at("created_at"))),
                ("firstName", field("firstName")),
                ("lastName", field("lastName")),
                ("displayName", or(at("displayName"), s(&username))),
                (
                    "mustChangePassword",
                    Value::Bool(at("mustChangePassword") == Some(&Value::Bool(true))),
                ),
                ("shares", shares),
            ]));
        }
        let mut doc = vec![("ok", Value::Bool(true)), ("users", Value::Array(out))];
        if truthy(get(pairs, "doctor")) {
            doc.push((
                "doctor",
                get(pairs, "doctor").cloned().unwrap_or(Value::Null),
            ));
        }
        let opts = Options {
            compact: true,
            ensure_ascii: false,
            ..Options::default()
        };
        o.out(&json::to_string(&crate::confirm::object(doc), &opts).unwrap_or_default());
        return Ok(Exit::Ok);
    }
    if !py::truthy(&users) {
        o.out("No connect users yet.");
        o.out(&format!(
            "The owner makes the first owner login in person: {} users add <username> --role owner --keep-password (it asks for a user check on this computer)",
            cli_name!()
        ));
        return Ok(Exit::Ok);
    }
    let all = py_iter(&users, 82)?;
    let mut rows: Vec<[String; 6]> = Vec::new();
    for u in &all {
        let Value::Object(up) = u else { continue };
        let at = |k: &str| get(up, k);
        let mut disabled = if truthy(at("disabled")) { "yes" } else { "no" }.to_string();
        if truthy(at("wasViewer")) {
            disabled.push_str(" (was viewer)");
        }
        let has_name = truthy(at("firstName")) || truthy(at("lastName"));
        let name = if has_name {
            py::str_of(&or(at("displayName"), s("")))
        } else {
            String::new()
        };
        rows.push([
            py::str_of(&or(at("username"), s("?"))),
            if name.is_empty() { "-".into() } else { name },
            py::str_of(&or(at("role"), s("member"))),
            disabled,
            if at("mustChangePassword") == Some(&Value::Bool(true)) {
                "temporary".into()
            } else {
                "-".into()
            },
            py::str_of(&or(at("createdAt"), or(at("created_at"), s("-")))),
        ]);
    }
    if rows.is_empty() {
        o.out("No connect users yet.");
        return Ok(Exit::Ok);
    }
    let head = [
        "USERNAME", "NAME", "ROLE", "DISABLED", "PASSWORD", "CREATED",
    ];
    let width = |i: usize| {
        rows.iter()
            .map(|r| r[i].chars().count())
            .chain([head[i].len()])
            .max()
            .unwrap_or(0)
    };
    let w: Vec<usize> = (0..5).map(width).collect();
    let line = |r: [&str; 6]| {
        let mut l = String::new();
        for i in 0..5 {
            l.push_str(&ljust(r[i], w[i]));
            l.push_str("  ");
        }
        l.push_str(r[5]);
        l
    };
    o.out(&line(head));
    for r in &rows {
        o.out(&line([&r[0], &r[1], &r[2], &r[3], &r[4], &r[5]]));
    }
    for u in &all {
        let Value::Object(up) = u else { continue };
        if get(up, "role") != Some(&s("voice_chat")) {
            continue;
        }
        let sh = or(get(up, "shares"), Value::Object(Vec::new()));
        let ag = shared(py_get(&sh, "agents", 117)?, "address", true, 117)?;
        let gd = shared(py_get(&sh, "gardens", 118)?, "name", true, 118)?;
        o.out(&format!(
            "  {} (Voice & Chat only) shares: agents {ag}; gardens {gd}",
            str_opt(get(up, "username"))
        ));
    }
    for l in items(get(pairs, "doctor"), 121)? {
        o.err(&format!("doctor: {}", py::str_of(&l)));
    }
    Ok(Exit::Ok)
}

/// `add`'s reader: `username` and whether `--keep-password` was given.
pub fn add(raw: &[u8], username: &str, keep: bool) -> Shown {
    let t = text(raw);
    let Some(d) = loads(&t) else {
        return unparseable(UNPARSEABLE, &t);
    };
    let mut o = Out::default();
    let r = add_body_read(&mut o, &d, &t, username, keep);
    done(o, r)
}

fn add_body_read(
    o: &mut Out,
    d: &Value,
    t: &str,
    username: &str,
    keep: bool,
) -> Result<Exit, Raise> {
    let err = match d {
        Value::Object(p) => get(p, "error").filter(|e| **e != Value::Null),
        _ => None,
    };
    if let Some(err) = err {
        let hint = match err {
            Value::Object(ep) => or(get(ep, "hint"), or_get(get(ep, "message"), get(ep, "code"))),
            other => or(py_get(d, "message", 10)?, other.clone()),
        };
        o.err(&format!("Error: {}", py::str_of(&hint)));
        if let Value::Object(ep) = err {
            for r in items(get(ep, "risks"), 12)? {
                o.err(&format!("  - {}", py::str_of(&r)));
            }
        }
        return Ok(Exit::Error);
    }
    let Value::Object(p) = d else {
        return failed(o, "users/add", t);
    };
    if !truthy(get(p, "success")) {
        return failed(o, "users/add", t);
    }
    o.out(&format!("Created user {username}."));
    if get(p, "role") == Some(&s("voice_chat")) {
        let sh = or(get(p, "shares"), Value::Object(Vec::new()));
        let ag = shared(py_get(&sh, "agents", 21)?, "address", false, 21)?;
        let gd = shared(py_get(&sh, "gardens", 22)?, "name", false, 22)?;
        o.out(&format!(
            "Voice & Chat only: joins voice rooms; shared agents: {ag}; shared Gardens: {gd}."
        ));
        for w in items(get(p, "warnings"), 24)? {
            o.err(&format!(
                "WARNING: shared anyway (--force): {}",
                py::str_of(&w)
            ));
        }
        if let Some(n) = get(p, "notice").filter(|n| py::truthy(n)) {
            o.err(&format!("WARNING: {}", py::str_of(n)));
        }
    }
    if get(p, "fixedPassword") == Some(&Value::Bool(true)) {
        o.out("The password is fixed: the person cannot change it.");
    }
    let temp = match get(p, "mustChangePassword") {
        None | Some(Value::Null) => !keep,
        Some(v) => *v == Value::Bool(true),
    };
    if temp {
        o.out(&format!(
            "The password is temporary: the first time {username} signs in, {} asks them to choose their own.",
            product_name!()
        ));
    } else {
        o.out("The password is kept (not temporary).");
    }
    Ok(Exit::Ok)
}

/// `Error: <what> failed: <raw[:200] or "empty response">`, exit 1.
fn failed(o: &mut Out, what: &str, t: &str) -> Result<Exit, Raise> {
    let h = head(t);
    o.err(&format!(
        "Error: {what} failed: {}",
        if h.is_empty() { "empty response" } else { &h }
    ));
    Ok(Exit::Error)
}

/// The reader of `add`'s set-role for an owner or admin.
pub fn add_role(raw: &[u8], username: &str, role: &str) -> Shown {
    let t = text(raw);
    let Some(d) = loads(&t) else {
        return unparseable("Error: set-role returned unparseable response: ", &t);
    };
    let mut o = Out::default();
    let err = match &d {
        Value::Object(p) => get(p, "error").filter(|e| **e != Value::Null),
        _ => None,
    };
    if let Some(err) = err {
        let hint = match err {
            Value::Object(ep) => str_opt(get(ep, "hint")),
            other => py::str_of(other),
        };
        o.err(&format!("Error setting role: {hint}"));
        return done(o, Ok(Exit::Error));
    }
    let ok = matches!(&d, Value::Object(p) if truthy(get(p, "success")));
    if !ok {
        let r = failed(&mut o, "set-role", &t);
        return done(o, r);
    }
    o.out(&format!("Role set to {role} for {username}."));
    done(o, Ok(Exit::Ok))
}

/// `set-role`'s reader.
pub fn set_role(raw: &[u8], user: &str, role: &str) -> Shown {
    let t = text(raw);
    let Some(d) = loads(&t) else {
        return unparseable(UNPARSEABLE, &t);
    };
    let mut o = Out::default();
    if let Value::Object(p) = &d {
        if truthy(get(p, "success")) {
            let live = get(p, "live");
            let tail = if truthy(live) {
                " (applied now, no sign-out)"
            } else if live == Some(&Value::Bool(false)) {
                " (they sign in again)"
            } else {
                ""
            };
            let from = get(p, "from").cloned().unwrap_or_else(|| s("?"));
            let to = get(p, "role").cloned().unwrap_or_else(|| s(role));
            o.out(&format!(
                "{user}: {} -> {}{tail}",
                py::str_of(&from),
                py::str_of(&to)
            ));
            return done(o, Ok(Exit::Ok));
        }
    }
    let e = match &d {
        Value::Object(p) => get(p, "error").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let message = match &d {
        Value::Object(p) => get(p, "message"),
        _ => None,
    };
    let from_e = match &e {
        Value::Object(ep) => or_get(get(ep, "hint"), get(ep, "code")),
        other => other.clone(),
    };
    let h = head(&t);
    let last = s(if h.is_empty() {
        "empty response from daemon"
    } else {
        &h
    });
    let msg = or(message, or(Some(&from_e), last));
    o.err(&format!("Error: {}", py::str_of(&msg)));
    let refused = matches!(&e, Value::Str(c) if matches!(c.as_str(), Some("role_change_not_allowed" | "role_required" | "invalid or missing token")));
    done(o, Ok(if refused { Exit::Refused } else { Exit::Error }))
}

/// `share`'s reader: the login as given and `--json`.
pub fn share(raw: &[u8], user: &str, as_json: bool) -> Shown {
    let t = text(raw);
    let user = user.trim_matches(py_space).to_lowercase();
    let Some(d) = loads(&t) else {
        return unparseable(UNPARSEABLE, &t);
    };
    let mut o = Out::default();
    let r = share_body(&mut o, &d, &user, as_json);
    done(o, r)
}

fn share_body(o: &mut Out, d: &Value, user: &str, as_json: bool) -> Result<Exit, Raise> {
    let e = match d {
        Value::Object(p) => get(p, "error").filter(|e| **e != Value::Null),
        _ => None,
    };
    if let Some(e) = e {
        let from_e = match e {
            Value::Object(ep) => or_get(get(ep, "hint"), get(ep, "code")),
            other => other.clone(),
        };
        let msg = or(py_get(d, "message", 12)?, from_e);
        o.err(&format!("Error: {}", py::str_of(&msg)));
        let code = match e {
            Value::Object(ep) => {
                for r in items(get(ep, "risks"), 14)? {
                    o.err(&format!("  - {}", py::str_of(&r)));
                }
                get(ep, "code").cloned().unwrap_or(Value::Null)
            }
            other => other.clone(),
        };
        let refused = matches!(&code, Value::Str(c) if matches!(c.as_str(), Some("role_required" | "invalid or missing token" | "Invalid or missing auth token")));
        return Ok(if refused { Exit::Refused } else { Exit::Error });
    }
    for w in items(py_get(d, "warnings", 17)?, 17)? {
        o.err(&format!(
            "WARNING: shared anyway (--force): {}",
            py::str_of(&w)
        ));
    }
    if let Some(n) = py_get(d, "notice", 19)?.filter(|n| py::truthy(n)) {
        o.err(&format!("WARNING: {}", py::str_of(n)));
    }
    let empty = || {
        crate::confirm::object(vec![
            ("agents", Value::Array(Vec::new())),
            ("gardens", Value::Array(Vec::new())),
        ])
    };
    // `d` is a dict here: `.get` above would have raised.
    let Value::Object(p) = d else {
        return Ok(Exit::Error);
    };
    let shares = if get(p, "users").is_some() {
        let mut row = None;
        for u in items(get(p, "users"), 22)? {
            if let Value::Object(up) = &u {
                if get(up, "username") == Some(&s(user)) {
                    row = Some(u.clone());
                    break;
                }
            }
        }
        let Some(Value::Object(rp)) = row else {
            o.err(&format!("Error: no login {user}"));
            return Ok(Exit::Error);
        };
        let role = get(&rp, "role");
        if role != Some(&s("voice_chat")) {
            o.err(&format!(
                "Error: {user} is {}; shares are only for a voice_chat login",
                str_opt(role)
            ));
            return Ok(Exit::Error);
        }
        or(get(&rp, "shares"), empty())
    } else {
        or(get(p, "shares"), empty())
    };
    if as_json {
        let opts = Options {
            compact: true,
            ..Options::default()
        };
        let doc = crate::confirm::object(vec![
            ("ok", Value::Bool(true)),
            ("username", s(user)),
            ("shares", shares),
        ]);
        o.out(&json::to_string(&doc, &opts).unwrap_or_default());
        return Ok(Exit::Ok);
    }
    let ag = shared(py_get(&shares, "agents", 36)?, "address", true, 36)?;
    let gd = shared(py_get(&shares, "gardens", 37)?, "name", true, 37)?;
    let mut sr = Vec::new();
    for x in items(py_get(&shares, "sources", 38)?, 38)? {
        let name = py_get(&x, "name", 38)?.cloned().unwrap_or_else(|| s("?"));
        let gone = truthy(py_get(&x, "missing", 38)?);
        sr.push(format!(
            "{}{}",
            py::str_of(&name),
            if gone { " (gone)" } else { "" }
        ));
    }
    let sr = if sr.is_empty() {
        "none".to_string()
    } else {
        sr.join(", ")
    };
    o.out(&format!(
        "{user} (Voice & Chat only)\n  agents:  {ag}\n  gardens: {gd}\n  sources: {sr}"
    ));
    Ok(Exit::Ok)
}

/// `set-password`'s reader.
pub fn set_password(raw: &[u8], username: &str) -> Shown {
    let t = text(raw);
    let Some(d) = loads(&t) else {
        return unparseable(UNPARSEABLE, &t);
    };
    let mut o = Out::default();
    let err = match &d {
        Value::Object(p) => get(p, "error").filter(|e| **e != Value::Null),
        _ => None,
    };
    if let Some(err) = err {
        let hint = match err {
            Value::Object(ep) => str_opt(get(ep, "hint")),
            other => py::str_of(other),
        };
        o.err(&format!("Error: {hint}"));
        let low = hint.to_lowercase();
        let refused = ["token", "forbidden", "may not"]
            .iter()
            .any(|w| low.contains(w));
        return done(o, Ok(if refused { Exit::Refused } else { Exit::Error }));
    }
    let ok = matches!(&d, Value::Object(p) if truthy(get(p, "success")));
    if !ok {
        let r = failed(&mut o, "set-password", &t);
        return done(o, r);
    }
    o.out(&format!(
        "Password set for {username}. They are signed out everywhere."
    ));
    let temp =
        matches!(&d, Value::Object(p) if get(p, "mustChangePassword") == Some(&Value::Bool(true)));
    if temp {
        o.out(&format!(
            "It is temporary: the next time {username} signs in, {} asks them to choose their own.",
            product_name!()
        ));
    } else {
        o.out("The password is kept (not temporary).");
    }
    done(o, Ok(Exit::Ok))
}

/// `audit`'s reader (without `--json`).
pub fn audit(raw: &[u8]) -> Shown {
    let t = text(raw);
    let Some(d) = loads(&t) else {
        return unparseable(UNPARSEABLE, &t);
    };
    let Value::Object(p) = &d else {
        return Shown::err(
            "Error: unexpected response from /cli/users/audit\n",
            Exit::Error,
        );
    };
    let mut o = Out::default();
    if let Some(err) = get(p, "error").filter(|e| **e != Value::Null) {
        let hint = match err {
            Value::Object(ep) => str_opt(get(ep, "hint")),
            other => py::str_of(other),
        };
        o.err(&format!("Error: {hint}"));
        return done(o, Ok(Exit::Error));
    }
    let events = or(get(p, "events"), Value::Array(Vec::new()));
    if !py::truthy(&events) {
        let path = or(get(p, "path"), s("~/.k2/auth-audit.jsonl"));
        o.out(&format!(
            "No auth-audit records yet ({}).",
            py::str_of(&path)
        ));
        return done(o, Ok(Exit::Ok));
    }
    let all = match py_iter(&events, 24) {
        Ok(a) => a,
        Err(r) => return done(o, Err(r)),
    };
    let hdr = ["TS", "EVENT", "OUTCOME", "USER", "INGRESS", "IP", "CLIENT"];
    let keys = ["ts", "event", "outcome", "user", "ingress", "ip", "client"];
    let mut rows: Vec<Vec<String>> = Vec::new();
    for e in &all {
        let Value::Object(ep) = e else { continue };
        rows.push(
            keys.iter()
                .map(|k| py::str_of(&or(get(ep, k), s("-"))))
                .collect(),
        );
    }
    let w: Vec<usize> = (0..hdr.len())
        .map(|i| {
            rows.iter()
                .map(|r| r[i].chars().count())
                .chain([hdr[i].len()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |r: &[String]| {
        let cells: Vec<String> = r.iter().enumerate().map(|(i, c)| ljust(c, w[i])).collect();
        cells.join("  ").trim_end_matches(py_space).to_string()
    };
    o.out(&line(&hdr.map(String::from)));
    for r in &rows {
        o.out(&line(r));
    }
    done(o, Ok(Exit::Ok))
}

/// `set-can-clone`'s reader.
pub fn can_clone(raw: &[u8], user: &str, on: bool) -> Shown {
    let t = text(raw);
    let d = loads(&t).unwrap_or_else(|| crate::confirm::object(vec![("error", s(&head(&t)))]));
    let mut o = Out::default();
    let success = match py_get(&d, "success", 8) {
        Ok(v) => truthy(v),
        Err(r) => return done(o, Err(r)),
    };
    if success {
        o.out(&format!(
            "{user}: Can clone {}",
            if on { "on" } else { "off" }
        ));
        return done(o, Ok(Exit::Ok));
    }
    let Value::Object(p) = &d else {
        return done(o, Ok(Exit::Error));
    };
    let e = or(get(p, "error"), s("empty response from daemon"));
    let shown = match &e {
        Value::Object(ep) => str_opt(get(ep, "hint")),
        other => py::str_of(other),
    };
    o.err(&format!("Error: {shown}"));
    done(o, Ok(Exit::Refused))
}

/// `unlock`'s reader.
pub fn unlock(raw: &[u8], as_json: bool) -> Shown {
    let t = text(raw);
    if t.trim_matches(py_space).is_empty() {
        return Shown::err(
            "Error: empty response from daemon (is it running?)\n",
            Exit::Error,
        );
    }
    let Ok(d) = json::parse(&t) else {
        return unparseable(UNPARSEABLE, &t);
    };
    let Value::Object(p) = &d else {
        return Shown::err(
            "Error: unexpected response from /cli/users/unlock\n",
            Exit::Error,
        );
    };
    let mut err = get(p, "error").cloned().unwrap_or(Value::Null);
    if err == Value::Null && get(p, "ok") == Some(&Value::Bool(false)) {
        err = d.clone();
    }
    if err != Value::Null {
        let hint = match &err {
            Value::Object(ep) => or(
                get(ep, "hint"),
                or(get(ep, "message"), or(get(ep, "code"), s("error"))),
            ),
            other => other.clone(),
        };
        return Shown::err(format!("Error: {}\n", py::str_of(&hint)), Exit::Error);
    }
    if get(p, "ok") != Some(&Value::Bool(true)) {
        let h = head(&t);
        return Shown::err(
            format!(
                "Error: {}\n",
                if h.is_empty() { "empty response" } else { &h }
            ),
            Exit::Error,
        );
    }
    if as_json {
        let mut out = t.clone();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        return Shown::out(out);
    }
    let key = py::str_of(&or(get(p, "username"), s("")));
    if get(p, "cleared") == Some(&Value::Bool(true)) {
        Shown::out(format!("Lock cleared for {key}\n"))
    } else {
        Shown::out(format!("No lock for {key}\n"))
    }
}
