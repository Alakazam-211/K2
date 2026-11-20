//! `k5 fed peers` and `k5 fed outbox`: their readers, as the shell's
//! Python prints them (`cli/src/federation.sh`), Python 3.12's traceback
//! included where it raises on an answer of a shape it doesn't expect.

use super::super::{get, py_get, py_iter, raised, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{self, Text, Value};
use crate::out::py;
use k2_client::cli_name;
use k2_client::hosts::ZONE_SUFFIX;
use std::cmp::Ordering;

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// `x or ''`.
fn or_empty(v: Option<&Value>) -> Value {
    match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => s(""),
    }
}

/// `v.<name>()` on a value that must be a str.
fn as_str<'a>(v: &'a Value, name: &str, at: &dyn Fn(String) -> Raise) -> Result<&'a Text, Raise> {
    match v {
        Value::Str(t) => Ok(t),
        other => Err(at(format!(
            "AttributeError: '{}' object has no attribute '{name}'",
            py::type_name(other)
        ))),
    }
}

/// `v.strip()`.
fn strip(v: &Value, at: &dyn Fn(String) -> Raise) -> Result<String, Raise> {
    let t = as_str(v, "strip", at)?;
    Ok(py::str_of(&Value::Str(t.clone()))
        .trim_matches(super::super::py_space)
        .to_string())
}

/// `v[:n]`.
fn head(v: &Value, n: usize, at: &dyn Fn(String) -> Raise) -> Result<Value, Raise> {
    match v {
        Value::Str(t) => {
            let cps = t.code_points();
            let cut: Vec<u32> = cps.into_iter().take(n).collect();
            Ok(Value::Str(points(&cut)))
        }
        Value::Array(items) => Ok(Value::Array(items.iter().take(n).cloned().collect())),
        Value::Object(_) => Err(at("TypeError: unhashable type: 'slice'".into())),
        other => Err(at(format!(
            "TypeError: '{}' object is not subscriptable",
            py::type_name(other)
        ))),
    }
}

fn points(cps: &[u32]) -> Text {
    if let Some(s) = cps
        .iter()
        .map(|c| char::from_u32(*c))
        .collect::<Option<String>>()
    {
        return Text::from(s);
    }
    let lit: String = cps.iter().map(|c| format!("\\u{c:04x}")).collect();
    match json::parse(&format!("\"{lit}\"")) {
        Ok(Value::Str(t)) => t,
        _ => Text::from(""),
    }
}

/// `str.casefold()`: lower case, with the full case folds that differ
/// from it for the letters a name is likely to hold.
pub fn casefold(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\u{df}' | '\u{1e9e}' => out.push_str("ss"),
            '\u{3c2}' => out.push('\u{3c3}'),
            '\u{17f}' => out.push('s'),
            '\u{b5}' => out.push('\u{3bc}'),
            '\u{3d0}' => out.push('\u{3b2}'),
            '\u{3d1}' => out.push('\u{3b8}'),
            '\u{3d5}' => out.push('\u{3c6}'),
            '\u{3d6}' => out.push('\u{3c0}'),
            '\u{3f0}' => out.push('\u{3ba}'),
            '\u{3f1}' => out.push('\u{3c1}'),
            '\u{3f5}' => out.push('\u{3b5}'),
            '\u{1e9b}' => out.push('\u{1e61}'),
            '\u{1fbe}' => out.push('\u{3b9}'),
            '\u{149}' => out.push_str("\u{2bc}n"),
            '\u{fb00}' => out.push_str("ff"),
            '\u{fb01}' => out.push_str("fi"),
            '\u{fb02}' => out.push_str("fl"),
            '\u{fb03}' => out.push_str("ffi"),
            '\u{fb04}' => out.push_str("ffl"),
            '\u{fb05}' | '\u{fb06}' => out.push_str("st"),
            _ => out.extend(c.to_lowercase()),
        }
    }
    out
}

/// One peer's sort key: `(_shown(p).casefold(), _host(p).casefold(),
/// p.get('fingerprint') or '')`.
fn sort_key(p: &Value) -> Result<(String, String, Value), Raise> {
    let shown_at = |line: u32| {
        move |e: String| Raise {
            frames: vec![(27, "<module>"), (27, "<lambda>"), (line, "_shown")],
            exception: e,
        }
    };
    let host_at = |line: u32| {
        move |e: String| Raise {
            frames: vec![(27, "<module>"), (27, "<lambda>"), (line, "_host")],
            exception: e,
        }
    };
    let lambda = |e: String| Raise {
        frames: vec![(27, "<module>"), (27, "<lambda>")],
        exception: e,
    };
    // _shown
    let Value::Object(pairs) = p else {
        return Err(shown_at(19)(format!(
            "AttributeError: '{}' object has no attribute 'get'",
            py::type_name(p)
        )));
    };
    let mut shown = s(&strip(&or_empty(get(pairs, "label")), &shown_at(19))?);
    if !py::truthy(&shown) {
        shown = s(&strip(&or_empty(get(pairs, "subdomain")), &shown_at(19))?);
    }
    if !py::truthy(&shown) {
        shown = head(&or_empty(get(pairs, "fingerprint")), 12, &shown_at(20))?;
    }
    let shown = casefold(&py::str_of(&Value::Str(
        as_str(&shown, "casefold", &lambda)?.clone(),
    )));
    // _host
    let base = match get(pairs, "base_url").filter(|v| py::truthy(v)) {
        Some(v) => v.clone(),
        None => or_empty(get(pairs, "baseUrl")),
    };
    let mut b = strip(&base, &host_at(22))?;
    for pre in ["http://", "https://"] {
        if b.to_lowercase().starts_with(pre) {
            b = b.chars().skip(pre.len()).collect();
        }
    }
    let mut host = b.split('/').next().unwrap_or("").to_string();
    if host.is_empty() {
        host = strip(&or_empty(get(pairs, "subdomain")), &host_at(26))?;
    }
    Ok((shown, casefold(&host), or_empty(get(pairs, "fingerprint"))))
}

/// `a < b` for the keys' last items, or Python's `TypeError`.
fn less(a: &Value, b: &Value) -> Result<bool, String> {
    let num = |v: &Value| match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Int(d) => d.parse::<f64>().ok(),
        Value::Float(f) => Some(*f),
        _ => None,
    };
    if a == b {
        return Ok(false);
    }
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => Ok(x < y),
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => Ok(x < y),
            _ => Err(format!(
                "TypeError: '<' not supported between instances of '{}' and '{}'",
                py::type_name(a),
                py::type_name(b)
            )),
        },
    }
}

fn key_less(a: &(String, String, Value), b: &(String, String, Value)) -> Result<bool, String> {
    match a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)) {
        Ordering::Less => Ok(true),
        Ordering::Greater => Ok(false),
        Ordering::Equal => less(&a.2, &b.2),
    }
}

/// `sorted(items, key=…)` as CPython 3.12's list sort compares (a run,
/// then binary insertion): the order, or the comparison's `TypeError`.
fn sorted(items: Vec<Value>) -> Result<Vec<Value>, Raise> {
    let mut keyed = Vec::with_capacity(items.len());
    for p in items {
        keyed.push((sort_key(&p)?, p));
    }
    let at = |e: String| Raise::at(27, e);
    let n = keyed.len();
    if n < 2 {
        return Ok(keyed.into_iter().map(|(_, p)| p).collect());
    }
    // count_run
    let mut run = 2;
    if key_less(&keyed[1].0, &keyed[0].0).map_err(at)? {
        while run < n && key_less(&keyed[run].0, &keyed[run - 1].0).map_err(at)? {
            run += 1;
        }
        keyed[..run].reverse();
    } else {
        while run < n && !key_less(&keyed[run].0, &keyed[run - 1].0).map_err(at)? {
            run += 1;
        }
    }
    // binarysort
    for start in run..n {
        let (mut l, mut r) = (0, start);
        while l < r {
            let p = l + ((r - l) >> 1);
            if key_less(&keyed[start].0, &keyed[p].0).map_err(at)? {
                r = p;
            } else {
                l = p + 1;
            }
        }
        let item = keyed.remove(start);
        keyed.insert(l, item);
    }
    Ok(keyed.into_iter().map(|(_, p)| p).collect())
}

/// `peers`' reader, on what the call printed.
pub fn peers_reader(raw: &[u8]) -> Shown {
    let cli = cli_name!();
    let Some(d) = std::str::from_utf8(raw)
        .ok()
        .and_then(|t| json::parse(t).ok())
    else {
        return Shown::err(
            format!("{cli} fed peers: could not parse response\n"),
            Exit::Error,
        );
    };
    if let Value::Object(pairs) = &d {
        if let Some(err) = get(pairs, "error").filter(|e| py::truthy(e)) {
            let shown = match err {
                Value::Object(e) => {
                    let pick = [get(e, "hint"), get(e, "code")]
                        .into_iter()
                        .flatten()
                        .find(|v| py::truthy(v));
                    py::str_of(pick.unwrap_or(err))
                }
                other => py::str_of(other),
            };
            return Shown::err(format!("{cli} fed peers: {shown}\n"), Exit::Error);
        }
    }
    let peers = match &d {
        Value::Object(pairs) => get(pairs, "peers").cloned().unwrap_or(Value::Array(vec![])),
        _ => Value::Array(vec![]),
    };
    let mut out = String::new();
    let r = (|| -> Result<(), Raise> {
        let peers = sorted(py_iter(&peers, 27)?)?;
        if peers.is_empty() {
            out.push_str("No paired servers yet.\n");
            out.push_str(&format!("Pair with: {cli} fed pair-start / {cli} fed confirm (or Settings \u{2192} Network \u{2192} Connected Servers \u{2192} Pair a server).\n"));
            return Ok(());
        }
        out.push_str(&format!("Peers ({}):\n", peers.len()));
        for p in &peers {
            let Value::Object(pairs) = p else {
                unreachable!("the sort key read it as a dict")
            };
            let sub = strip(&or_empty(get(pairs, "subdomain")), &|e| Raise::at(34, e))?;
            let base = match get(pairs, "base_url").filter(|v| py::truthy(v)) {
                Some(v) => v.clone(),
                None => or_empty(get(pairs, "baseUrl")),
            };
            let base = strip(&base, &|e| Raise::at(35, e))?;
            let host = if !base.is_empty() {
                base
            } else if sub.starts_with("http://")
                || sub.starts_with("https://")
                || sub.contains(':')
                || sub.ends_with(".ts.net")
            {
                sub
            } else if !sub.is_empty() && !sub.ends_with(ZONE_SUFFIX) {
                format!("{sub}{ZONE_SUFFIX}")
            } else if sub.is_empty() {
                "\u{2014}".to_string()
            } else {
                sub
            };
            let label = or_empty(get(pairs, "label"));
            let trust = match get(pairs, "trust").filter(|v| py::truthy(v)) {
                Some(v) => v.clone(),
                None => s("?"),
            };
            let fp = head(&or_empty(get(pairs, "fingerprint")), 16, &|e| {
                Raise::at(46, e)
            })?;
            let extra = if py::truthy(&label) {
                format!("  label={}", py::str_of(&label))
            } else {
                String::new()
            };
            out.push_str(&format!(
                "  {host}  trust={}  fp={}\u{2026}{extra}\n",
                py::str_of(&trust),
                py::str_of(&fp)
            ));
            if get(pairs, "repair_recommended").is_some_and(py::truthy) {
                out.push_str(&format!("      older pairing code: messages still work; pairing again recommended (Settings \u{2192} Network \u{2192} Connected Servers \u{2192} Pair again, or {cli} fed pair-start)\n"));
            }
            let n = match get(pairs, "address_notice").filter(|v| py::truthy(v)) {
                Some(v) => v.clone(),
                None => Value::Object(vec![]),
            };
            let st = py_get(&n, "status", 53)?.cloned();
            let tgt = match py_get(&n, "target", 54)?.filter(|v| py::truthy(v)) {
                Some(v) => v.clone(),
                None => Value::Object(vec![]),
            };
            let first = or_empty(py_get(&tgt, "subdomain", 55)?);
            let new_addr = if py::truthy(&first) {
                first
            } else {
                or_empty(py_get(&tgt, "base_url", 55)?)
            };
            let Value::Object(np) = &n else {
                unreachable!("n.get passed")
            };
            if st == Some(s("pending")) {
                let err = get(np, "last_error").cloned().unwrap_or(Value::Null);
                let attempts = get(np, "attempts")
                    .cloned()
                    .unwrap_or(Value::Int("0".into()));
                let tail = if py::truthy(&err) {
                    format!("; last error: {}", py::str_of(&err))
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "      address notice: pending -> {} (attempts {}{tail})\n",
                    py::str_of(&new_addr),
                    py::str_of(&attempts)
                ));
            } else if st == Some(s("failed")) {
                let reason = match get(np, "failed_reason").filter(|v| py::truthy(v)) {
                    Some(v) => v.clone(),
                    None => s("?"),
                };
                out.push_str(&format!(
                    "      address notice: FAILED -> {} ({}): {}\n",
                    py::str_of(&new_addr),
                    py::str_of(&reason),
                    py::str_of(&or_empty(get(np, "last_error")))
                ));
            }
        }
        Ok(())
    })();
    match r {
        Ok(()) => Shown::out(out),
        Err(e) => raised(out, String::new(), &e),
    }
}

/// `'error' in d`, or the `TypeError` at line 6.
fn has_error(d: &Value) -> Result<bool, Raise> {
    match d {
        Value::Object(pairs) => Ok(get(pairs, "error").is_some()),
        Value::Array(items) => Ok(items.contains(&s("error"))),
        Value::Str(t) => Ok(py::str_of(&Value::Str(t.clone())).contains("error")),
        other => Err(Raise::at(
            6,
            format!(
                "TypeError: argument of type '{}' is not iterable",
                py::type_name(other)
            ),
        )),
    }
}

/// `p.get(key, default)` on a value that must be a dict.
fn get_or(p: &Value, key: &str, default: Value, line: u32) -> Result<Value, Raise> {
    Ok(py_get(p, key, line)?.cloned().unwrap_or(default))
}

/// `outbox`'s reader, on what curl printed.
pub fn outbox_reader(raw: &[u8]) -> Shown {
    let cli = cli_name!();
    let Some(d) = std::str::from_utf8(raw)
        .ok()
        .and_then(|t| json::parse(t).ok())
    else {
        return Shown::err(
            format!("{cli} fed outbox: unexpected response (is \"Connect agents across servers\" on?)\n"),
            Exit::Error,
        );
    };
    let mut out = String::new();
    let r = (|| -> Result<Exit, Raise> {
        if has_error(&d)? {
            let Value::Object(pairs) = &d else {
                let index = if matches!(d, Value::Array(_)) {
                    "list indices must be integers or slices, not str"
                } else {
                    "string indices must be integers, not 'str'"
                };
                return Err(Raise::at(7, format!("TypeError: {index}")));
            };
            let err = get(pairs, "error").cloned().unwrap_or(Value::Null);
            out = format!("{cli} fed outbox: {}\n", py::str_of(&err));
            return Ok(Exit::Error);
        }
        let peers = get_or(&d, "peers", Value::Array(vec![]), 8)?;
        if !py::truthy(&peers) {
            out.push_str("outbox: empty (no queued or dead-lettered cross-server messages)\n");
            return Ok(Exit::Ok);
        }
        for p in py_iter(&peers, 12)? {
            let mut who = get_or(&p, "label", Value::Null, 13)?;
            if !py::truthy(&who) {
                who = get_or(&p, "subdomain", Value::Null, 13)?;
            }
            if !py::truthy(&who) {
                let fp = get_or(&p, "fingerprint", s(""), 13)?;
                who = head(&fp, 12, &|e| Raise::at(13, e))?;
            }
            let queued = get_or(&p, "queued", Value::Int("0".into()), 14)?;
            let mut line = format!("{}: {} queued", py::str_of(&who), py::str_of(&queued));
            let age = get_or(&p, "oldest_age_secs", Value::Null, 15)?;
            if age != Value::Null {
                line.push_str(&format!(" (oldest {}s)", py::str_of(&age)));
            }
            let dead = get_or(&p, "dead", Value::Int("0".into()), 18)?;
            line.push_str(&format!(", {} dead-lettered", py::str_of(&dead)));
            out.push_str(&line);
            out.push('\n');
            let letters = get_or(&p, "dead_letters", Value::Array(vec![]), 20)?;
            let letters = head(&letters, 5, &|e| Raise::at(20, e))?;
            for dl in py_iter(&letters, 20)? {
                let id = get_or(&dl, "msg_uuid", s(""), 21)?;
                let id = head(&id, 8, &|e| Raise::at(21, e))?;
                let reason = get_or(&dl, "reason", s(""), 21)?;
                out.push_str(&format!(
                    "  dead {}\u{2026} \u{2014} {}\n",
                    py::str_of(&id),
                    py::str_of(&reason)
                ));
            }
        }
        let total = get_or(&d, "total_queued", Value::Int("0".into()), 22)?;
        out.push_str(&format!("total queued: {}\n", py::str_of(&total)));
        Ok(Exit::Ok)
    })();
    match r {
        Ok(Exit::Ok) => Shown::out(out),
        Ok(exit) => Shown {
            stdout: String::new(),
            stderr: out,
            exit,
        },
        Err(e) => raised(out, String::new(), &e),
    }
}
