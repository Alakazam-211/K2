//! The shell's Python readers in `cli/src/llm.sh`, each as it prints an
//! answer: what it prints before it raises, then the traceback and exit 1
//! (the shell runs them under `set -e`). Line numbers in a traceback are
//! the reader's own, counted from the `-c` text's first (empty) line.

use super::super::sidecar::py_slice;
use super::super::{ljust, py_d, py_get, py_iter, py_space, raised, Raise, Shown};
use crate::confirm;
use crate::out::json::{Text, Value};
use crate::out::py;
use crate::out::pylib::ops::{self, round_int, s_of};
use crate::out::pylib::time;
use k2_client::brand::{CLI_NAME as CLI, PRODUCT_NAME};

type R<T> = Result<T, Raise>;

/// What a reader printed, and its exit.
struct Out(String);

impl Out {
    fn line(&mut self, s: impl AsRef<str>) {
        self.0.push_str(s.as_ref());
        self.0.push('\n');
    }
}

fn run(f: impl FnOnce(&mut Out) -> R<()>) -> Shown {
    let mut o = Out(String::new());
    match f(&mut o) {
        Ok(()) => Shown::out(o.0),
        Err(r) => raised(o.0, String::new(), &r),
    }
}

/// `json.load(sys.stdin)` at `line`.
fn load(body: &[u8], line: u32) -> R<Value> {
    confirm::load(body).map_err(|r| Raise::at(line, r.exception))
}

/// `x or {}`.
fn or_obj(v: Option<&Value>) -> Value {
    match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => Value::Object(Vec::new()),
    }
}

/// `x or []`, walked.
fn or_items(v: Option<&Value>, line: u32) -> R<Vec<Value>> {
    match v {
        Some(x) if py::truthy(x) => py_iter(x, line),
        _ => Ok(Vec::new()),
    }
}

/// `x or "<d>"` as `%s` prints it.
fn or_s(v: Option<&Value>, d: &str) -> String {
    match v {
        Some(x) if py::truthy(x) => py::str_of(x),
        _ => d.to_string(),
    }
}

/// `x or "<d>"` as a value.
fn or_v(v: Option<&Value>, d: &str) -> Value {
    match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => Value::Str(Text::from(d)),
    }
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

/// `v == "<s>"`.
fn is(v: Option<&Value>, s: &str) -> bool {
    matches!(v, Some(Value::Str(t)) if *t == Text::from(s))
}

/// `"chat" if p.get("scopeKind") == "session" else p.get("scopeKind")`.
fn scope_word(kind: Option<&Value>) -> String {
    if is(kind, "session") {
        "chat".into()
    } else {
        s_of(kind)
    }
}

/// `float(x)`.
fn float(v: &Value, line: u32) -> R<f64> {
    match v {
        Value::Int(d) => Ok(d.parse::<f64>().unwrap_or(f64::INFINITY)),
        Value::Float(f) => Ok(*f),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Str(_) => {
            let s = py::str_of(v);
            ops::float(&s).ok_or_else(|| {
                Raise::at(
                    line,
                    format!(
                        "ValueError: could not convert string to float: {}",
                        py::repr(v)
                    ),
                )
            })
        }
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: float() argument must be a string or a real number, not '{}'",
                py::type_name(other)
            ),
        )),
    }
}

/// `int(x)`.
fn int(v: &Value, line: u32) -> R<i128> {
    let bad = |v: &Value| {
        Raise::at(
            line,
            format!(
                "ValueError: invalid literal for int() with base 10: {}",
                py::repr(v)
            ),
        )
    };
    match v {
        Value::Int(d) => d.parse::<i128>().map_err(|_| bad(v)),
        Value::Float(f) if f.is_nan() => Err(Raise::at(
            line,
            "ValueError: cannot convert float NaN to integer",
        )),
        Value::Float(f) if f.is_infinite() => Err(Raise::at(
            line,
            "OverflowError: cannot convert float infinity to integer",
        )),
        Value::Float(f) => Ok(f.trunc() as i128),
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Str(_) => {
            let s = py::str_of(v);
            let w: String = s.trim_matches(py_space).replace('_', "");
            w.parse::<i128>().map_err(|_| bad(v))
        }
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
                py::type_name(other)
            ),
        )),
    }
}

/// `time.strftime(fmt, time.localtime(int(x)))`.
fn local(v: &Value, fmt: &str, line: u32) -> R<String> {
    let secs = int(v, line)?;
    let secs = i64::try_from(secs).map_err(|_| Raise::at(line, time::RANGE))?;
    time::strftime_local(secs, fmt).map_err(|e| Raise::at(line, e))
}

/// `round(float(w.get("used", 0)) * 100)`, as `%d` prints it.
fn used_pct(w: &Value, line: u32) -> R<String> {
    let zero = Value::Int("0".into());
    let used = py_get(w, "used", line)?.unwrap_or(&zero);
    round_int(float(used, line)? * 100.0, line)
}

/// `" · ".join("%s %d%%" % (w.get("label", "?"), round(…)) for w in wins)`.
fn windows(wins: &[Value], line: u32) -> R<String> {
    let mut parts = Vec::new();
    for w in wins {
        let label = py_get(w, "label", line)?.map_or_else(|| "?".to_string(), py::str_of);
        parts.push(format!("{label} {}%", used_pct(w, line)?));
    }
    Ok(parts.join(" \u{b7} "))
}

/// `s + t` where `s` is what `u.get("status") or "-"` gave.
fn plus(s: &Value, t: &str, line: u32) -> R<String> {
    match s {
        Value::Str(_) => Ok(format!("{}{t}", py::str_of(s))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +=: '{}' and 'str'",
                py::type_name(other)
            ),
        )),
    }
}

/// `_llma_resolve`'s Python (`2>/dev/null`, in a `$(…)` with no `set
/// -e`): the id it printed, or nothing.
pub fn resolve(body: &[u8], tool: &str, key: &str) -> String {
    let find = || -> R<Option<String>> {
        let v = load(body, 5)?;
        let tools = match py_get(&v, "tools", 6)? {
            None => Vec::new(),
            Some(t) => py_iter(t, 6)?,
        };
        let key_l = key.to_lowercase();
        for t in &tools {
            if !is(py_get(t, "tool", 7)?, tool) {
                continue;
            }
            let accounts = match py_get(t, "accounts", 9)? {
                None => Vec::new(),
                Some(a) => py_iter(a, 9)?,
            };
            for a in &accounts {
                if is(py_get(a, "id", 10)?, key) {
                    return Ok(Some(py::str_of(ops::index(a, "id", 11)?)));
                }
                let label = or_v(py_get(a, "label", 10)?, "");
                let Value::Str(_) = label else {
                    return Err(Raise::no_attribute(10, &label, "lower"));
                };
                if py::str_of(&label).to_lowercase() == key_l {
                    return Ok(Some(py::str_of(ops::index(a, "id", 11)?)));
                }
            }
        }
        Ok(None)
    };
    match find() {
        Ok(Some(id)) => id.trim_end_matches('\n').to_string(),
        _ => String::new(),
    }
}

/// `ago(ts)`.
fn ago(ts: Option<&Value>, line: u32) -> R<String> {
    let Some(ts) = ts.filter(|t| py::truthy(t)) else {
        return Ok("never checked".into());
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i128;
    let d = (now - int(ts, line)?).max(0);
    Ok(if d < 60 {
        "checked just now".into()
    } else if d < 3600 {
        format!("checked {} min ago", d / 60)
    } else if d < 86400 {
        format!("checked {} h ago", d / 3600)
    } else {
        format!("checked {} d ago", d / 86400)
    })
}

/// `usage_summary(u, checked)`, called from line `from`.
fn usage_summary(u: Option<&Value>, checked: Option<&Value>, from: u32) -> R<String> {
    let inside = |r: Raise| {
        let line = r.frames.last().map_or(1, |f| f.0);
        Raise::inside(from, line, "usage_summary", r.exception)
    };
    let Some(u) = u.filter(|u| py::truthy(u)) else {
        return Ok("-".into());
    };
    let wins = or_items(py_get(u, "windows", 21).map_err(inside)?, 21).map_err(inside)?;
    let mut s = if wins.is_empty() {
        or_v(py_get(u, "status", 25).map_err(inside)?, "-")
    } else {
        Value::Str(Text::from(windows(&wins, 23).map_err(inside)?.as_str()))
    };
    let tail = format!(" ({})", ago(checked, 26).map_err(inside)?);
    s = Value::Str(Text::from(plus(&s, &tail, 26).map_err(inside)?.as_str()));
    let mut s = py::str_of(&s);
    if truthy(py_get(u, "error", 27).map_err(inside)?) {
        let e = ops::index(u, "error", 28).map_err(inside)?;
        s.push_str(&format!(" [last check failed: {}]", py::str_of(e)));
    }
    Ok(s)
}

/// `"%-<w>s" % (x or "-")[:n]`.
fn cut(v: Option<&Value>, n: usize, w: usize, line: u32) -> R<String> {
    let x = or_v(v, "-");
    Ok(ljust(&py::str_of(&py_slice(&x, n, line)?), w))
}

/// `_llma_render_list <tool>`.
pub fn list(body: &[u8], want: &str) -> Shown {
    run(|o| {
        let v = load(body, 4)?;
        let tools = match py_get(&v, "tools", 31)? {
            None => Vec::new(),
            Some(t) => py_iter(t, 31)?,
        };
        for t in &tools {
            let name = py_get(t, "tool", 32)?;
            if !want.is_empty()
                && !is(name, want)
                && !(want == "cursor-agent" && is(name, "cursor"))
            {
                continue;
            }
            let disp = match py_get(t, "display", 35)? {
                Some(d) if py::truthy(d) => py::str_of(d),
                _ => s_of(name),
            };
            if !truthy(py_get(t, "supported", 36)?) {
                o.line(format!("{disp}: no tokens yet"));
                continue;
            }
            let accts = or_items(py_get(t, "accounts", 39)?, 50)?;
            let name = s_of(name);
            if accts.is_empty() {
                if matches!(py_get(t, "subscription", 41)?, Some(Value::Bool(false))) {
                    o.line(format!(
                        "{disp}: no API tokens (add one: {CLI} llm tokens add-key {name} <label>)"
                    ));
                } else {
                    o.line(format!(
                        "{disp}: no tokens (add a subscription: {CLI} llm tokens add {name} <label>)"
                    ));
                }
                continue;
            }
            let mut since = String::new();
            if truthy(py_get(t, "switchedAt", 47)?) {
                let when = local(ops::index(t, "switchedAt", 49)?, "%Y-%m-%d %H:%M", 49)?;
                let by = if truthy(py_get(t, "switchedBy", 50)?) {
                    format!(" by {}", py::str_of(ops::index(t, "switchedBy", 50)?))
                } else {
                    String::new()
                };
                since = format!(" (server default since {when}{by})");
            }
            o.line(format!("{disp}:{since}"));
            let auto = or_obj(py_get(t, "autoNext", 52)?);
            if truthy(py_get(&auto, "on", 53)?) {
                let last = if truthy(py_get(&auto, "lastLine", 54)?) {
                    format!("; last: {}", py::str_of(ops::index(&auto, "lastLine", 54)?))
                } else {
                    String::new()
                };
                o.line(format!("  auto-rotate: on (Next by itself at 99%){last}"));
            }
            for a in &accts {
                let mark = if truthy(py_get(a, "active", 56)?) {
                    "*"
                } else {
                    " "
                };
                let line = if is(py_get(a, "kind", 57)?, "api_key") {
                    format!(
                        "  {mark} {} {} API token \u{b7} billed per token",
                        cut(py_get(a, "label", 58)?, 20, 20, 58)?,
                        ljust(&or_s(py_get(a, "state", 58)?, "-"), 12)
                    )
                } else {
                    let label = cut(py_get(a, "label", 61)?, 20, 20, 61)?;
                    let state = ljust(&or_s(py_get(a, "state", 61)?, "-"), 12);
                    let plan = cut(py_get(a, "plan", 62)?, 8, 8, 62)?;
                    let summary = usage_summary(
                        py_get(a, "usage", 63)?,
                        py_get(a, "usageCheckedAt", 63)?,
                        63,
                    )?;
                    format!("  {mark} {label} {state} {plan} {summary}")
                };
                o.line(line.trim_end_matches(py_space));
                let pins = or_items(py_get(a, "pinnedTo", 65)?, 66)?;
                if !pins.is_empty() {
                    let mut parts = Vec::new();
                    for p in &pins {
                        let kind = scope_word(py_get(p, "scopeKind", 67)?);
                        let label = match py_get(p, "label", 67)? {
                            Some(l) if py::truthy(l) => py::str_of(l),
                            _ => s_of(py_get(p, "scopeId", 67)?),
                        };
                        parts.push(format!("{kind} {label}"));
                    }
                    let in_use = if truthy(py_get(a, "inUse", 68)?) {
                        " (in use)"
                    } else {
                        ""
                    };
                    o.line(format!("      used by: {}{in_use}", parts.join(", ")));
                }
                if truthy(py_get(a, "detail", 69)?) {
                    o.line(format!(
                        "      {}",
                        py::str_of(ops::index(a, "detail", 70)?)
                    ));
                }
                if truthy(py_get(a, "signInEndsSoon", 71)?)
                    && truthy(py_get(a, "signInEndsAt", 71)?)
                {
                    let when = local(ops::index(a, "signInEndsAt", 72)?, "%Y-%m-%d %H:%M %Z", 72)?;
                    o.line(format!("      sign in again by {when}"));
                }
            }
        }
        if truthy(py_get(&v, "airgap", 73)?) {
            o.line("Air-gap is on: no sign-in, refresh or usage calls.");
        }
        for r in or_items(py_get(&v, "keychainRepairs", 75)?, 75)? {
            let parts = match py_get(&r, "partitions", 77)? {
                Some(p) if py::truthy(p) => ops::join(p, ",", 77)?,
                _ => String::new(),
            };
            let parts = if parts.is_empty() {
                "empty".into()
            } else {
                parts
            };
            o.line(format!(
                "WARN keychain: macOS asks for your password on every Claude read of \"{}\" ({}): its partition list is {parts}, without apple-tool:.",
                s_of(py_get(&r, "service", 76)?),
                s_of(py_get(&r, "holds", 77)?)
            ));
            o.line(format!(
                "     Fix once in Terminal (asks for your login password): {}",
                s_of(py_get(&r, "command", 78)?)
            ));
        }
        o.line("* = the server default token.");
        o.line(or_s(
            py_get(&v, "switchNote", 80)?,
            "Changing the server default token affects every chat that uses Server default on this server.",
        ));
        Ok(())
    })
}

/// `status`'s reader.
pub fn status(body: &[u8]) -> Shown {
    run(|o| {
        let v = load(body, 3)?;
        let a = or_obj(py_get(&v, "account", 3)?);
        let g = |k: &str, line| py_get(&a, k, line);
        let active = if truthy(g("active", 4)?) {
            " (server default)"
        } else {
            ""
        };
        o.line(format!(
            "{} {}{active}",
            s_of(g("tool", 4)?),
            s_of(g("label", 4)?)
        ));
        for k in ["state", "detail", "plan", "email", "org"] {
            if truthy(g(k, 6)?) {
                o.line(format!(
                    "  {} {}",
                    ljust(k, 8),
                    py::str_of(ops::index(&a, k, 7)?)
                ));
            }
        }
        if truthy(g("signInEndsAt", 8)?) {
            let when = local(ops::index(&a, "signInEndsAt", 10)?, "%Y-%m-%d %H:%M %Z", 10)?;
            if truthy(g("signInEndsSoon", 11)?) {
                let label = confirm::dumps(g("label", 12)?.unwrap_or(&Value::Null));
                o.line(format!(
                    "  {} sign in again by {when} ({CLI} llm tokens login {} {label})",
                    ljust("sign-in", 8),
                    s_of(g("tool", 12)?)
                ));
            } else {
                o.line(format!("  {} ends {when}", ljust("sign-in", 8)));
            }
        }
        let u = or_obj(g("usage", 15)?);
        for w in or_items(py_get(&u, "windows", 16)?, 16)? {
            o.line(format!(
                "  {} {}% used, resets {}",
                ljust(&s_of(py_get(&w, "label", 17)?), 8),
                used_pct(&w, 17)?,
                or_s(py_get(&w, "resetsAt", 17)?, "-")
            ));
        }
        Ok(())
    })
}

/// `"%s %-7s %-20s %s" % (mark, r.get("tool"), (label or "-")[:20], x)`.
fn row(r: &Value, last: String, line: u32) -> R<String> {
    let mark = if truthy(py_get(r, "active", line)?) {
        "*"
    } else {
        " "
    };
    Ok(format!(
        "{mark} {} {} {last}",
        ljust(&s_of(py_get(r, "tool", line)?), 7),
        cut(py_get(r, "label", line)?, 20, 20, line)?
    ))
}

/// `usage`'s reader.
pub fn usage(body: &[u8]) -> Shown {
    run(|o| {
        let v = load(body, 3)?;
        let rows = or_items(py_get(&v, "rows", 3)?, 3)?;
        if rows.is_empty() {
            o.line("(no tokens)");
        }
        for r in &rows {
            let u = or_obj(py_get(r, "usage", 7)?);
            let wins = or_items(py_get(&u, "windows", 8)?, 8)?;
            let summ = windows(&wins, 9)?;
            let summ = if summ.is_empty() {
                or_s(py_get(&u, "status", 9)?, "-")
            } else {
                summ
            };
            o.line(row(r, summ, 10)?);
        }
        Ok(())
    })
}

/// `refresh`'s reader.
pub fn refresh(body: &[u8]) -> Shown {
    run(|o| {
        let v = load(body, 3)?;
        for a in or_items(py_get(&v, "accounts", 3)?, 4)? {
            let state = or_s(py_get(&a, "state", 5)?, "-");
            o.line(row(&a, state, 5)?);
        }
        Ok(())
    })
}

/// `clear-default`'s reader; `tool` is `sys.argv[1]`.
pub fn clear_default(body: &[u8], tool: &str) -> Shown {
    run(|o| {
        let v = load(body, 3)?;
        let c = or_obj(py_get(&v, "cleared", 3)?);
        let tool = or_s(py_get(&c, "tool", 4)?, tool);
        if !truthy(py_get(&c, "cleared", 5)?) {
            o.line(format!("{tool} had no server default token"));
        } else if truthy(py_get(&c, "nowDefault", 7)?) {
            o.line(format!(
                "{tool} server default: {} \u{2192} {} (the subscription signed in to {tool} itself)",
                s_of(py_get(&c, "clearedLabel", 8)?),
                s_of(py_get(&c, "nowDefaultLabel", 8)?)
            ));
        } else {
            o.line(format!(
                "{tool} server default cleared (was {}): chats on Server default get no {PRODUCT_NAME} token",
                s_of(py_get(&c, "clearedLabel", 10)?)
            ));
        }
        Ok(())
    })
}

/// `{True: "yes", False: "NO", None: "-"}[x]`.
fn step_word(x: Option<&Value>, line: u32) -> R<&'static str> {
    match x {
        None | Some(Value::Null) => Ok("-"),
        Some(Value::Bool(true)) => Ok("yes"),
        Some(Value::Bool(false)) => Ok("NO"),
        Some(Value::Int(d)) if d == "1" => Ok("yes"),
        Some(Value::Int(d)) if d == "0" || d == "-0" => Ok("NO"),
        Some(Value::Float(f)) if *f == 1.0 => Ok("yes"),
        Some(Value::Float(f)) if *f == 0.0 => Ok("NO"),
        Some(other @ (Value::Array(_) | Value::Object(_))) => Err(Raise::at(
            line,
            format!("TypeError: unhashable type: '{}'", py::type_name(other)),
        )),
        Some(other) => Err(Raise::at(line, format!("KeyError: {}", py::repr(other)))),
    }
}

/// `storage`'s reader.
pub fn storage(body: &[u8]) -> Shown {
    run(|o| {
        let v = load(body, 3)?;
        let src = py_get(&v, "source", 4)?;
        let tail = if is(src, "setting") {
            String::new()
        } else if is(src, "env") {
            " (K2_LLM_ACCOUNTS_STORAGE)".into()
        } else {
            " (the default)".into()
        };
        o.line(format!(
            "saved tokens are kept in: {}{tail}",
            s_of(py_get(&v, "storage", 5)?)
        ));
        let vault = if truthy(py_get(&v, "vaultOpen", 6)?) {
            "open".to_string()
        } else {
            format!("NOT open ({})", or_s(py_get(&v, "vaultError", 6)?, "?"))
        };
        o.line(format!("{PRODUCT_NAME} vault: {vault}"));
        for k in ["banner", "nudge"] {
            if truthy(py_get(&v, k, 8)?) {
                o.line(py::str_of(ops::index(&v, k, 9)?));
            }
        }
        if let Some(s) = py_get(&v, "sync", 10)?.filter(|s| py::truthy(s)) {
            let zero = Value::Int("0".into());
            let n = |k: &str| -> R<String> { py_d(py_get(s, k, 12)?.unwrap_or(&zero), 12) };
            o.line(format!(
                "sealed {}, already sealed {}, removed kept disabled {}, waiting {}",
                n("sealed")?,
                n("unchanged")?,
                n("disabled")?,
                n("waiting")?
            ));
        }
        if let Some(e) = py_get(&v, "export", 13)?.filter(|e| py::truthy(e)) {
            if let Some(r) = py_get(e, "restored", 14)?.filter(|r| py::truthy(r)) {
                o.line(format!(
                    "wrote {} file(s) back from the vault",
                    py_d(r, 15)?
                ));
            }
        }
        let items = or_items(py_get(&v, "items", 16)?, 16)?;
        if !items.is_empty() {
            o.line(format!(
                "{} {} {} {} in step",
                ljust("tool", 7),
                ljust("token", 22),
                ljust("file", 5),
                ljust("vault", 9)
            ));
        }
        for i in &items {
            let step = step_word(py_get(i, "inStep", 20)?, 20)?;
            let file = if truthy(py_get(i, "file", 21)?) {
                "yes"
            } else {
                "no"
            };
            let removed = if truthy(py_get(i, "removed", 21)?) {
                "  (removed)"
            } else {
                ""
            };
            o.line(format!(
                "{} {} {} {} {step}{removed}",
                ljust(&s_of(py_get(i, "tool", 21)?), 7),
                cut(py_get(i, "label", 21)?, 22, 22, 21)?,
                ljust(file, 5),
                ljust(&s_of(py_get(i, "vault", 21)?), 9)
            ));
        }
        Ok(())
    })
}

/// `who(p)`, called from line `from`.
fn who(p: Option<&Value>, from: u32) -> R<String> {
    let Some(p) = p.filter(|p| py::truthy(p)) else {
        return Ok("-".into());
    };
    if is(Some(p), "owner-token") {
        return Ok("the owner".into());
    }
    match p {
        Value::Str(_) => {
            let s = py::str_of(p);
            Ok(match s.strip_prefix("login:") {
                Some(rest) => rest.to_string(),
                None => s,
            })
        }
        other => Err(Raise::inside(
            from,
            8,
            "who",
            format!(
                "AttributeError: '{}' object has no attribute 'startswith'",
                py::type_name(other)
            ),
        )),
    }
}

/// `auto-next`'s reader.
pub fn auto_next(body: &[u8]) -> Shown {
    run(|o| {
        let v = load(body, 3)?;
        for t in or_items(py_get(&v, "tools", 9)?, 9)? {
            let state = if truthy(py_get(&t, "on", 10)?) {
                let paused = if truthy(py_get(&t, "byCanAct", 11)?) {
                    ""
                } else {
                    "; paused: they can no longer change the server default"
                };
                format!(
                    "on (turned on by {}{paused})",
                    who(py_get(&t, "by", 11)?, 11)?
                )
            } else {
                "off".into()
            };
            o.line(format!(
                "{} {state}",
                ljust(&s_of(py_get(&t, "tool", 14)?), 7)
            ));
            if truthy(py_get(&t, "lastLine", 15)?) {
                o.line(format!(
                    "        last: {}",
                    py::str_of(ops::index(&t, "lastLine", 16)?)
                ));
            }
        }
        o.line("");
        o.line(or_s(py_get(&v, "note", 18)?, ""));
        Ok(())
    })
}

/// `pins`'s reader: the pins' body, a newline, the list's body.
pub fn pins(pins_body: &[u8], list_body: &[u8]) -> Shown {
    run(|o| {
        let mut both = pins_body.to_vec();
        both.push(b'\n');
        both.extend_from_slice(list_body);
        let text = String::from_utf8_lossy(&both).into_owned();
        // `sys.stdin.read().split("\n", 1)` into two names.
        let Some((pins_raw, list_raw)) = text.split_once('\n') else {
            return Err(Raise::at(
                3,
                "ValueError: not enough values to unpack (expected 2, got 1)",
            ));
        };
        let pins_v = load(pins_raw.as_bytes(), 4)?;
        let pins = or_items(py_get(&pins_v, "pins", 4)?, 4)?;
        let list_v = load(list_raw.as_bytes(), 6)?;
        let mut labels: Vec<(Value, Value)> = Vec::new();
        let tools = match py_get(&list_v, "tools", 6)? {
            None => Vec::new(),
            Some(t) => py_iter(t, 6)?,
        };
        for t in &tools {
            for a in or_items(py_get(t, "accounts", 7)?, 7)? {
                let id = py_get(&a, "id", 8)?.cloned().unwrap_or(Value::Null);
                if matches!(id, Value::Array(_) | Value::Object(_)) {
                    return Err(Raise::at(
                        8,
                        format!("TypeError: unhashable type: '{}'", py::type_name(&id)),
                    ));
                }
                let label = py_get(&a, "label", 8)?.cloned().unwrap_or(Value::Null);
                match labels.iter_mut().find(|(k, _)| ops::py_eq(k, &id)) {
                    Some((_, old)) => *old = label,
                    None => labels.push((id, label)),
                }
            }
        }
        if pins.is_empty() {
            o.line(
                "(no workspace or chat has its own token \u{2014} every chat uses Server default)",
            );
        }
        for p in &pins {
            let account = py_get(p, "accountId", 13)?;
            let key = account.cloned().unwrap_or(Value::Null);
            if matches!(key, Value::Array(_) | Value::Object(_)) {
                return Err(Raise::at(
                    13,
                    format!("TypeError: unhashable type: '{}'", py::type_name(&key)),
                ));
            }
            let found = labels
                .iter()
                .find(|(k, _)| ops::py_eq(k, &key))
                .map(|(_, l)| l)
                .filter(|l| py::truthy(l));
            let target = match found {
                Some(l) => py::str_of(l),
                None => s_of(account),
            };
            let label = match py_get(p, "label", 12)? {
                Some(l) if py::truthy(l) => Some(l),
                _ => py_get(p, "scopeId", 12)?,
            };
            o.line(format!(
                "{} {} {} -> {target}",
                ljust(&s_of(py_get(p, "tool", 12)?), 7),
                ljust(&scope_word(py_get(p, "scopeKind", 12)?), 9),
                cut(label, 28, 28, 12)?
            ));
        }
        Ok(())
    })
}

/// `pct(w)`: the chip rule, called from line `from`.
fn pct(w: &Value, from: u32) -> R<i128> {
    let inside = |r: Raise| {
        let line = r.frames.last().map_or(1, |f| f.0);
        Raise::inside(from, line, "pct", r.exception)
    };
    if truthy(py_get(w, "resetPassed", 16).map_err(inside)?) {
        return Ok(0);
    }
    // The default is worked out first, whether it's used or not.
    let zero = Value::Int("0".into());
    let used = py_get(w, "used", 18).map_err(inside)?.unwrap_or(&zero);
    let dflt = float(used, 18).map_err(inside)? * 100.0;
    let p = match py_get(w, "usedPercent", 18).map_err(inside)? {
        Some(x) => float(x, 18).map_err(inside)?,
        None => dflt,
    };
    let p = if p.is_nan() { 0.0 } else { p.clamp(0.0, 100.0) };
    Ok((p + 0.5).trunc() as i128)
}

/// `for`'s reader; `addr` is `sys.argv[1]`.
pub fn for_agent(body: &[u8], addr: &str) -> Shown {
    run(|o| {
        let v = load(body, 3)?;
        let addr = or_s(py_get(&v, "address", 4)?, addr);
        if !truthy(py_get(&v, "tool", 5)?) {
            o.line(format!("{addr}: not an LLM chat (no token)"));
            return Ok(());
        }
        let pin_scope = py_get(&v, "pinScope", 9)?;
        let pinned = if is(pin_scope, "chat") {
            "set for this chat"
        } else if is(pin_scope, "workspace") {
            "set for its workspace"
        } else if is(pin_scope, "person") {
            "set for this person"
        } else {
            "set for it"
        };
        let source = py_get(&v, "source", 10)?;
        if matches!(source, Some(Value::Array(_) | Value::Object(_))) {
            let t = py::type_name(source.unwrap_or(&Value::Null));
            return Err(Raise::at(9, format!("TypeError: unhashable type: '{t}'")));
        }
        let why = if is(source, "pinned") {
            pinned.to_string()
        } else if is(source, "resumed") {
            "the token its conversation started on".into()
        } else if is(source, "default") {
            "the server default".into()
        } else {
            or_s(source, "-")
        };
        let next = if truthy(py_get(&v, "live", 12)?) {
            ""
        } else {
            " \u{b7} next start"
        };
        o.line(format!(
            "{addr}: {} token {} ({why}){next}",
            s_of(py_get(&v, "tool", 12)?),
            or_s(py_get(&v, "label", 12)?, "-")
        ));
        if truthy(py_get(&v, "unavailable", 13)?) {
            o.line(format!(
                "  {}",
                py::str_of(ops::index(&v, "unavailable", 14)?)
            ));
        }
        let mp = py_get(&v, "maxPercent", 15)?;
        let wins = or_items(py_get(&v, "windows", 16)?, 16)?;
        match mp {
            None | Some(Value::Null) => {
                let api = if is(py_get(&v, "kind", 23)?, "api_key") {
                    " (API token: billed per token)"
                } else {
                    ""
                };
                o.line(format!("  usage: no reading{api}"));
            }
            Some(mp) => {
                let d = py_d(mp, 25)?;
                let mut parts = Vec::new();
                for w in &wins {
                    let label = py_get(w, "label", 25)?.map_or_else(|| "?".to_string(), py::str_of);
                    parts.push(format!("{label} {}%", pct(w, 25)?));
                }
                o.line(format!("  usage: {d}% ({})", parts.join(" \u{b7} ")));
                if truthy(py_get(&v, "updatedAt", 26)?) {
                    o.line(format!(
                        "  read at {}",
                        py::str_of(ops::index(&v, "updatedAt", 27)?)
                    ));
                }
            }
        }
        if let Some(ns) = py_get(&v, "nextStart", 28)?.filter(|n| py::truthy(n)) {
            o.line(format!(
                "  from its next start: {} ({})",
                or_s(py_get(ns, "accountId", 30)?, "the server default"),
                s_of(py_get(ns, "source", 30)?)
            ));
        }
        if ops::contains(&v, "refreshed", 31)? && !truthy(py_get(&v, "refreshed", 31)?) {
            let r = or_v(py_get(&v, "reason", 32)?, "-");
            if is(Some(&r), "rate_limited") {
                o.line(format!(
                    "  not re-read: read less than a minute ago (try in {}s)",
                    s_of(py_get(&v, "retryAfterSecs", 34)?)
                ));
            } else {
                o.line(format!("  not re-read ({})", py::str_of(&r)));
            }
        }
        Ok(())
    })
}

/// `pin`'s reader; `label` is `sys.argv[1]`.
pub fn pinned(body: &[u8], label: &str) -> Shown {
    run(|o| {
        let v = load(body, 3)?;
        let p = or_obj(py_get(&v, "pin", 4)?);
        let who = match py_get(&p, "label", 5)? {
            Some(l) if py::truthy(l) => py::str_of(l),
            _ => s_of(py_get(&p, "scopeId", 5)?),
        };
        o.line(format!(
            "{} {who} now uses {} token {label}",
            scope_word(py_get(&p, "scopeKind", 5)?),
            s_of(py_get(&p, "tool", 5)?)
        ));
        if truthy(py_get(&v, "note", 6)?) {
            o.line(py::str_of(ops::index(&v, "note", 7)?));
        }
        Ok(())
    })
}

/// `unpin`'s check: `json.load(sys.stdin).get("unpinned")`.
pub fn unpinned(body: &[u8]) -> R<bool> {
    let v = load(body, 1)?;
    Ok(truthy(py_get(&v, "unpinned", 1)?))
}

/// `_llma_render_switch`.
pub fn switch(body: &[u8]) -> Shown {
    run(|o| {
        let v = load(body, 3)?;
        let s = or_obj(py_get(&v, "switch", 4)?);
        let a = or_obj(py_get(&v, "account", 5)?);
        let tool = match py_get(&s, "tool", 6)? {
            Some(t) if py::truthy(t) => t.clone(),
            _ => py_get(&a, "tool", 6)?.cloned().unwrap_or(Value::Null),
        };
        let tool_s = py::str_of(&tool);
        let first = |keys: &[&str], line| -> R<String> {
            let mut last = None;
            for (i, k) in keys.iter().enumerate() {
                let src = if i == 0 { &a } else { &s };
                last = py_get(src, k, line)?;
                if truthy(last) {
                    break;
                }
            }
            Ok(s_of(last))
        };
        if truthy(py_get(&s, "appliedSaved", 7)?) {
            o.line(format!(
                "{tool_s}: put the saved sign-in of {} back; agents on Server default use it again",
                first(&["label", "toLabel", "to"], 8)?
            ));
        } else if truthy(py_get(&s, "alreadyActive", 9)?) {
            o.line(format!(
                "{tool_s}: {} is already the server default",
                first(&["label", "to"], 10)?
            ));
        } else {
            let was = if truthy(py_get(&s, "fromLabel", 12)?) {
                format!(" (was {})", py::str_of(ops::index(&s, "fromLabel", 12)?))
            } else {
                String::new()
            };
            o.line(format!(
                "{tool_s}: server default is now {}{was}",
                first(&["label", "toLabel", "to"], 13)?
            ));
            for w in or_items(py_get(&v, "warnings", 14)?, 14)? {
                o.line(format!("WARN: {}", py::str_of(&w)));
            }
            for d in or_items(py_get(&s, "adopted", 16)?, 16)? {
                let account = s_of(py_get(&d, "account", 18)?);
                let label = s_of(py_get(&d, "label", 18)?);
                if truthy(py_get(&d, "created", 17)?) {
                    o.line(format!(
                        "Saved a sign-in made outside {PRODUCT_NAME} ({account}) as the new token \"{label}\"; the token \"{}\" was left as it was.",
                        s_of(py_get(&d, "foundUnder", 18)?)
                    ));
                } else {
                    o.line(format!(
                        "Saved a sign-in made outside {PRODUCT_NAME} ({account}) to its own token \"{label}\"."
                    ));
                }
            }
            for n in or_items(py_get(&s, "needsSignIn", 21)?, 21)? {
                let n = py::str_of(&n);
                o.line(format!(
                    "\"{n}\" held another account's sign-in; it needs signing in again ({CLI} llm tokens login {tool_s} \"{n}\")."
                ));
            }
            if is(Some(&tool), "claude") && !truthy(py_get(&s, "profileSwapped", 23)?) {
                o.line("Note: ~/.claude.json was not updated, so Claude /status may still name the previous account.");
            }
            o.line("Changing the server default token affects every chat that uses Server default on this server.");
        }
        Ok(())
    })
}

/// `_llma_follow_login`'s first Python: the login's id.
pub fn login_id(body: &[u8]) -> R<String> {
    let v = load(body, 3)?;
    let l = or_obj(py_get(&v, "login", 4)?);
    Ok(or_s(py_get(&l, "loginId", 4)?, "")
        .trim_end_matches('\n')
        .to_string())
}

/// What `_llma_follow_login` reads of one `login/status` answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fields {
    pub state: String,
    pub url: String,
    pub code: String,
    pub banner: String,
    pub err: String,
    pub done: String,
    pub offer: String,
    pub tool: String,
    pub label: String,
}

/// `one(x)`.
fn one(x: Option<&Value>) -> String {
    match x {
        None | Some(Value::Null) => String::new(),
        Some(v) => py::str_of(v).replace(['\n', '\t'], " "),
    }
}

/// The status reader's line, as `IFS=$'\x1f' read -r …` splits it.
pub fn login_fields(body: &[u8]) -> R<Fields> {
    let v = load(body, 2)?;
    let l = or_obj(py_get(&v, "login", 2)?);
    let g = |k: &str| py_get(&l, k, 5);
    let flag = |x: Option<&Value>| if truthy(x) { "1" } else { "0" }.to_string();
    let joined = [
        one(g("state")?),
        one(g("url")?),
        one(g("code")?),
        one(g("banner")?),
        one(g("error")?),
        flag(g("done")?),
        flag(g("offerMakeActive")?),
        one(g("tool")?),
        one(g("label")?),
    ]
    .join("\x1f");
    // `$(…)` drops the trailing newlines; `read` takes nine fields, the
    // last one the rest of the line.
    let joined = joined.trim_end_matches('\n');
    let line = joined.split('\n').next().unwrap_or("");
    let mut parts = line.splitn(9, '\x1f').map(str::to_string);
    let mut next = || parts.next().unwrap_or_default();
    Ok(Fields {
        state: next(),
        url: next(),
        code: next(),
        banner: next(),
        err: next(),
        done: next(),
        offer: next(),
        tool: next(),
        label: next(),
    })
}
