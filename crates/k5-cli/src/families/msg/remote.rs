//! `k5 msg <agent>::<host> "text"` (and the older `<agent>@<host>`): a
//! message to an agent on another server, over federation, as the shell's
//! `_send_remote_msg` (`cli/src/remote-msg.sh`) sends it.
//!
//! After the send key and the wire-form check ([`super::send`]), in the
//! shell's order:
//!
//! 1. `_postal_send_teach`: a Postal address is Postal mail with the text
//!    alone ([`super::postal`]).
//! 2. An empty agent (`@host`): one line, exit 2.
//! 3. `<agent>/<chat>`: the chat's name slugged as the shell's Python does
//!    it; a bad one is one line, exit 2.
//! 4. The `@` form's hint.
//! 5. `/cli/federation/peers` (`cli_send`), and its reader ([`peers`]):
//!    the peer's fingerprint and trust, or the line and exit 2 or 3.
//! 6. `/cli/federation/peer-roster?peer=<fp>`, and its reader ([`roster`]):
//!    the agent's workspace id, or the line and exit 2 or 3.
//! 7. `/cli/federation/send`, sent once by its key
//!    ([`super::send_once::send`]), after the refusal notes so far are
//!    dropped (`_permission_note_reset`).
//! 8. The replay note, then the answer's reader ([`sent`]): delivered
//!    (exit 0), queued (exit 0, two lines), the gate's JSON line (exit 3),
//!    declined or an error (exit 2), anything else (exit 1).
//!
//! A request that fails ends the command with curl's code (`set -e` on
//! `x=$(cli_send …)`); a reader that raises ends it with its traceback and
//! exit 1, as `x=$(… | python3 …)` does under `set -o pipefail`.

use super::super::{py_space, traceback, Raise};
use super::py::{loads, s};
use super::send_once::{self, Key};
use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::out::py::{str_of, truthy, type_name};
use crate::out::{self};
use crate::prelude::Closing;
use crate::transport::encode::quote;
use crate::transport::{Call, Client, Error};
use k2_client::hosts::ZONE_SUFFIX;
use k2_client::{cli_name, product_name};

/// The peers.
pub const PEERS: &str = "/cli/federation/peers";
/// A peer's roster.
pub const ROSTER: &str = "/cli/federation/peer-roster";
/// The send.
pub const SEND: &str = "/cli/federation/send";

/// `_split_remote_target` for an address on another server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// `_BARE_TARGET`.
    pub agent: String,
    /// `_REMOTE_HOST`, never empty.
    pub host: String,
    /// `_USED_LEGACY_AT`.
    pub legacy_at: bool,
}

/// `_split_remote_target raw`: `None` for a local address or the wire
/// form.
pub fn split(raw: &str) -> Option<Target> {
    if let Some(i) = raw.find("::") {
        let (left, right) = (&raw[..i], &raw[i + 2..]);
        if right.contains("::") || left.is_empty() || !right.contains('.') {
            return None;
        }
        return Some(Target {
            agent: left.to_string(),
            host: right.to_string(),
            legacy_at: false,
        });
    }
    let (agent, host) = raw.rsplit_once('@')?;
    (!host.is_empty()).then(|| Target {
        agent: agent.to_string(),
        host: host.to_string(),
        legacy_at: true,
    })
}

/// No agent before `@`.
pub fn empty_agent() -> String {
    concat!(
        cli_name!(),
        " msg: empty agent name in '<agent>::<host>' target."
    )
    .to_string()
}

/// A bad `<agent>/<chat>`.
pub fn bad_chat() -> String {
    format!(
        concat!(
            cli_name!(),
            " msg: a chat on another server is <agent>/<chat>::<host> (one slash), e.g. ada/notes::box.example{}."
        ),
        ZONE_SUFFIX
    )
}

/// The `@` form's hint.
pub fn at_hint(agent: &str, host: &str) -> String {
    format!(
        concat!(
            "hint: prefer `",
            cli_name!(),
            " msg {}::{}` — '@' federated addresses are legacy compat (not email)."
        ),
        agent, host
    )
}

/// The chat's name as `python3 -c 'print("-".join(p for p in
/// s.strip().lower().split() if p))'` prints it, held by `$(…)`.
pub fn slug(chat: &str) -> String {
    let lower = chat.trim_matches(py_space).to_lowercase();
    lower
        .split(py_space)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Splits `agent` at its first `/` into the agent and the chat's slug, or
/// the bad-chat line.
pub fn chat(agent: &str) -> Result<(String, Option<String>), String> {
    let Some((a, c)) = agent.split_once('/') else {
        return Ok((agent.to_string(), None));
    };
    let (a, c) = (a.to_string(), slug(c));
    if a.is_empty() || c.is_empty() || c.contains('/') || c.contains(':') {
        return Err(bad_chat());
    }
    Ok((a, Some(c)))
}

/// What a reader printed, held by `$(…)`, or how it raised.
pub type Read = Result<String, Raise>;

fn no_attr(line: u32, v: &Value, name: &str) -> Raise {
    Raise::no_attribute(line, v, name)
}

/// `v.get(key)` on a dict, `None` (JSON null) when missing; the
/// `AttributeError` at `line` on anything else.
fn get(v: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Object(pairs) => Ok(super::super::get(pairs, key)
            .cloned()
            .unwrap_or(Value::Null)),
        other => Err(no_attr(line, other, "get")),
    }
}

/// `v.get(key, default)`.
fn get_or(v: &Value, key: &str, default: Value, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Object(pairs) => Ok(super::super::get(pairs, key).cloned().unwrap_or(default)),
        other => Err(no_attr(line, other, "get")),
    }
}

fn or(a: Value, b: Value) -> Value {
    if truthy(&a) {
        a
    } else {
        b
    }
}

/// `v.strip()` / `v.lower()` on a value that may not be a string.
fn text_of(v: &Value, line: u32, method: &str) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(str_of(v)),
        other => Err(no_attr(line, other, method)),
    }
}

fn strip(t: &str) -> String {
    t.trim_matches(py_space).to_string()
}

/// The peers reader's `bare(h)`, called from `from`.
fn bare(h: &Value, from: u32) -> Result<String, Raise> {
    let h = or(h.clone(), s(""));
    let t = text_of(&h, 6, "strip").map_err(|r| inside(from, 6, "bare", r))?;
    Ok(bare_str(&t))
}

fn bare_str(h: &str) -> String {
    let mut h = strip(h).to_lowercase();
    let doubled = format!("{ZONE_SUFFIX}{ZONE_SUFFIX}");
    while h.ends_with(&doubled) {
        h.truncate(h.len() - ZONE_SUFFIX.len());
    }
    if h.ends_with(ZONE_SUFFIX) {
        h.truncate(h.len() - ZONE_SUFFIX.len());
    }
    h
}

fn inside(from: u32, line: u32, name: &'static str, r: Raise) -> Raise {
    Raise::inside(from, line, name, r.exception)
}

/// The peers reader's `host_port(u)`, called from `from`.
fn host_port(u: &Value, from: u32) -> Result<String, Raise> {
    let u = or(u.clone(), s(""));
    let u = strip(&text_of(&u, 28, "strip").map_err(|r| inside(from, 28, "host_port", r))?);
    Ok(host_port_str(&u))
}

fn host_port_str(u: &str) -> String {
    let lower = u.to_lowercase();
    let rest = if lower.starts_with("https://") {
        u.chars().skip(8).collect::<String>()
    } else if lower.starts_with("http://") {
        u.chars().skip(7).collect::<String>()
    } else {
        u.to_string()
    };
    rest.split('/').next().unwrap_or_default().to_string()
}

fn ok_line(p: &Value) -> Read {
    Ok(format!(
        "OK\t{}\t{}",
        str_of(&get_or(p, "fingerprint", s(""), 40)?),
        str_of(&get_or(p, "trust", s(""), 40)?)
    ))
}

/// The peers reader: what it prints for `host` and the answer `raw`.
pub fn peers(raw: &[u8], host: &str) -> Read {
    let host = strip(host).to_lowercase();
    let want = bare_str(&host);
    let Some(d) = loads(raw) else {
        return Ok("ERR\tparse".to_string());
    };
    let list = match &d {
        Value::Object(pairs) => match super::super::get(pairs, "peers") {
            Some(Value::Array(list)) => Some(list.clone()),
            _ => None,
        },
        _ => None,
    };
    let Some(list) = list else {
        let Value::Object(_) = &d else {
            return Ok("NOFED".to_string());
        };
        let err = get(&d, "error", 19)?;
        if let Value::Object(_) = &err {
            let code = get(&err, "code", 20)?;
            if ["forbidden", "not_connected"]
                .iter()
                .any(|c| super::py::eq(&code, &s(c)))
            {
                let v = or(or(get(&err, "hint", 21)?, code), s(""));
                return Ok(format!("FORBIDDEN\t{}", str_of(&v)));
            }
        }
        if truthy(&err) {
            let v = match &err {
                Value::Object(_) => or(get(&err, "hint", 24)?, err.clone()),
                _ => err.clone(),
            };
            return Ok(format!("NOFED\t{}", str_of(&v)));
        }
        return Ok("NOFED".to_string());
    };
    for p in &list {
        let sub = or(get(p, "subdomain", 35)?, s(""));
        let label = or(get(p, "label", 36)?, s(""));
        let base = or(or(get(p, "base_url", 37)?, get(p, "baseUrl", 37)?), s(""));
        let hp = host_port(&base, 38)?;
        if !want.is_empty() && bare(&sub, 39)? == want {
            return ok_line(p);
        }
        let sub_lower =
            |line| -> Result<String, Raise> { Ok(text_of(&sub, line, "lower")?.to_lowercase()) };
        if !host.is_empty() && truthy(&sub) {
            let l = sub_lower(43)?;
            if l == host
                || l == format!("{host}{ZONE_SUFFIX}")
                || l == format!("{want}{ZONE_SUFFIX}")
            {
                return ok_line(p);
            }
        }
        if !host.is_empty()
            && (sub_lower(47)? == host || hp.to_lowercase() == host || hp.to_lowercase() == want)
        {
            return ok_line(p);
        }
        if !want.is_empty() && bare(&label, 50)? == want {
            return ok_line(p);
        }
    }
    let mut known = Vec::new();
    for p in &list {
        let base = or(or(get(p, "base_url", 56)?, get(p, "baseUrl", 56)?), s(""));
        let base = strip(&text_of(&base, 56, "strip")?);
        let sub = or(get(p, "subdomain", 57)?, s(""));
        let sub = strip(&text_of(&sub, 57, "strip")?);
        if !base.is_empty() {
            let hp = host_port(&s(&base), 59)?;
            known.push(if hp.is_empty() { base } else { hp });
        } else if !sub.is_empty() {
            if sub.contains(':') || sub.ends_with(".ts.net") || sub.ends_with(ZONE_SUFFIX) {
                known.push(sub);
            } else {
                known.push(format!("{sub}{ZONE_SUFFIX}"));
            }
        } else {
            let label = get(p, "label", 65)?;
            if truthy(&label) {
                known.push(str_of(&label));
            }
        }
    }
    Ok(format!("NONE\t{}", known.join(", ")))
}

/// The roster reader's `slugify(s)`, from `norm` called at `from`.
fn slugify(v: &Value, from: &[(u32, &'static str)]) -> Result<String, Raise> {
    if !truthy(v) {
        return Ok(String::new());
    }
    let raise = |line: u32, exception: String| {
        let mut frames = from.to_vec();
        frames.push((37, "norm"));
        frames.push((line, "slugify"));
        Raise { frames, exception }
    };
    let items: Vec<Value> = match v {
        Value::Str(_) => str_of(v).chars().map(|c| s(&c.to_string())).collect(),
        Value::Array(a) => a.clone(),
        Value::Object(pairs) => pairs.iter().map(|(k, _)| Value::Str(k.clone())).collect(),
        other => {
            return Err(raise(
                23,
                format!("TypeError: '{}' object is not iterable", type_name(other)),
            ))
        }
    };
    for ch in &items {
        let Value::Str(_) = ch else {
            return Err(raise(
                24,
                format!(
                    "TypeError: 'in <string>' requires string as left operand, not {}",
                    type_name(ch)
                ),
            ));
        };
        let c = str_of(ch);
        let n = c.chars().count();
        if c.is_empty() || "/:\\".contains(c.as_str()) {
            return slug_stripped(v, &raise);
        }
        if c == "\0" {
            return slug_stripped(v, &raise);
        }
        if n != 1 {
            return Err(raise(
                24,
                format!("TypeError: ord() expected a character, but string of length {n} found"),
            ));
        }
        if (c.chars().next().unwrap_or('\0') as u32) < 32 {
            return slug_stripped(v, &raise);
        }
    }
    let Value::Str(_) = v else {
        return Err(raise(
            26,
            format!(
                "AttributeError: '{}' object has no attribute 'strip'",
                type_name(v)
            ),
        ));
    };
    let mut t = strip(&str_of(v)).to_lowercase().replace('_', "-");
    t = t
        .split(py_space)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    while t.contains("--") {
        t = t.replace("--", "-");
    }
    t = t.trim_matches('-').to_string();
    t = t
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect();
    while t.contains("--") {
        t = t.replace("--", "-");
    }
    Ok(t.trim_matches('-').to_string())
}

/// `return (s or "").strip().lower()` at line 25.
fn slug_stripped(v: &Value, raise: &dyn Fn(u32, String) -> Raise) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(strip(&str_of(v)).to_lowercase()),
        other => Err(raise(
            25,
            format!(
                "AttributeError: '{}' object has no attribute 'strip'",
                type_name(other)
            ),
        )),
    }
}

/// The roster reader's `norm(s)`, called from the frames `from`.
fn norm(v: &Value, from: &[(u32, &'static str)]) -> Result<String, Raise> {
    let g = slugify(v, from)?;
    if !g.is_empty() {
        return Ok(g);
    }
    let v = or(v.clone(), s(""));
    match &v {
        Value::Str(_) => Ok(strip(&str_of(&v)).to_lowercase()),
        other => {
            let mut frames = from.to_vec();
            frames.push((38, "norm"));
            Err(Raise {
                frames,
                exception: format!(
                    "AttributeError: '{}' object has no attribute 'strip'",
                    type_name(other)
                ),
            })
        }
    }
}

const MODULE: &str = "<module>";
const GENEXPR: &str = "<genexpr>";

/// What `for x in v` walks, or the `TypeError` at `line`.
fn iterate(v: &Value, line: u32) -> Result<Vec<Value>, Raise> {
    match v {
        Value::Str(_) => Ok(str_of(v).chars().map(|c| s(&c.to_string())).collect()),
        Value::Array(a) => Ok(a.clone()),
        Value::Object(pairs) => Ok(pairs.iter().map(|(k, _)| Value::Str(k.clone())).collect()),
        other => Err(Raise::at(
            line,
            format!("TypeError: '{}' object is not iterable", type_name(other)),
        )),
    }
}

/// `x not in seen` for a set of what `json.loads` gave, or the
/// `TypeError` for a list or a dict.
fn hashable(v: &Value, line: u32) -> Result<(), Raise> {
    match v {
        Value::Array(_) | Value::Object(_) => Err(Raise::at(
            line,
            format!("TypeError: unhashable type: '{}'", type_name(v)),
        )),
        _ => Ok(()),
    }
}

/// `", ".join(values)` at `line` inside a generator expression.
fn join_str(values: &[Value], line: u32) -> Result<String, Raise> {
    let mut out = Vec::new();
    for (i, v) in values.iter().enumerate() {
        let Value::Str(_) = v else {
            return Err(Raise::at(
                line,
                format!(
                    "TypeError: sequence item {i}: expected str instance, {} found",
                    type_name(v)
                ),
            ));
        };
        out.push(str_of(v));
    }
    Ok(out.join(", "))
}

/// The roster reader: what it prints for `agent` and the answer `raw`.
pub fn roster(raw: &[u8], agent: &str) -> Read {
    let Some(d) = loads(raw) else {
        return Ok("ERR\tparse".to_string());
    };
    let Value::Object(pairs) = &d else {
        return Ok("ERR\t".to_string());
    };
    if let Some(err) = super::super::get(pairs, "error") {
        if let Value::Object(_) = err {
            let code = get(err, "code", 12)?;
            if ["forbidden", "not_connected"]
                .iter()
                .any(|c| super::py::eq(&code, &s(c)))
            {
                let v = or(or(get(err, "hint", 13)?, code), s(""));
                return Ok(format!("FORBIDDEN\t{}", str_of(&v)));
            }
            let v = or(
                or(get(err, "hint", 15)?, get(err, "code", 15)?),
                err.clone(),
            );
            return Ok(format!("ERR\t{}", str_of(&v)));
        }
        return Ok(format!("ERR\t{}", str_of(err)));
    }
    let roster = or(get(&d, "roster", 17)?, Value::Object(Vec::new()));
    let agents = or(get(&roster, "agents", 18)?, Value::Array(Vec::new()));
    let want = norm(&s(agent), &[(40, MODULE)])?;
    let mut matches = Vec::new();
    for a in iterate(&agents, 42)? {
        let handle = or(get(&a, "agent", 43)?, s(""));
        let handle = strip(&text_of(&handle, 43, "strip")?);
        let aliases = or(get(&a, "aliases", 44)?, Value::Array(Vec::new()));
        let wname = or(get(&a, "workspace_name", 45)?, s(""));
        let mut hit = norm(&s(&handle), &[(46, MODULE)])? == want;
        if !hit {
            for x in iterate(&aliases, 46)? {
                if norm(&x, &[(46, MODULE), (46, GENEXPR)])? == want {
                    hit = true;
                    break;
                }
            }
        }
        if !hit {
            let w = text_of(&wname, 47, "strip")?;
            if !strip(&w).is_empty() && norm(&wname, &[(47, MODULE)])? == want {
                hit = true;
            }
        }
        if hit {
            matches.push(a);
        }
    }
    let mut ids: Vec<Value> = Vec::new();
    let mut seen: Vec<Value> = Vec::new();
    for a in &matches {
        let wid = or(get(a, "workspace_id", 55)?, s(""));
        hashable(&wid, 56)?;
        if !seen.iter().any(|x| super::py::eq(x, &wid)) {
            seen.push(wid);
            ids.push(a.clone());
        }
    }
    if ids.is_empty() {
        let names: Vec<Value> = iterate(&agents, 60)?
            .iter()
            .map(|a| get_or(a, "agent", s("?"), 60))
            .collect::<Result<_, _>>()?;
        let names = join_str(&names, 60).map_err(|r| Raise {
            frames: vec![(60, MODULE)],
            exception: r.exception,
        })?;
        return Ok(format!("NONE\t{names}"));
    }
    if ids.len() > 1 {
        let info: Vec<String> = ids
            .iter()
            .map(|a| {
                Ok(format!(
                    "{} ({})",
                    str_of(&get_or(a, "workspace_name", s("?"), 63)?),
                    str_of(&get_or(a, "workspace_id", s("?"), 63)?)
                ))
            })
            .collect::<Result<_, Raise>>()?;
        return Ok(format!("AMBIG\t{}", info.join("; ")));
    }
    let handle = or(get(&ids[0], "agent", 67)?, s(""));
    let handle = strip(&text_of(&handle, 67, "strip")?);
    let note = if !handle.is_empty() && norm(&s(&handle), &[(68, MODULE)])? != want {
        handle
    } else {
        String::new()
    };
    let wid = or(get_or(&ids[0], "workspace_id", s(""), 69)?, s(""));
    Ok(format!("OK\t{}\t{}", str_of(&wid), note))
}

/// The send's reader: what it prints for the answer `raw`.
pub fn sent(raw: &[u8]) -> Read {
    let Some(d) = loads(raw) else {
        return Ok("RAW".to_string());
    };
    let Value::Object(_) = &d else {
        return Ok("RAW".to_string());
    };
    let err = get(&d, "error", 9)?;
    if let Value::Object(_) = &err {
        let code = or(get(&err, "code", 11)?, s(""));
        let hint = or(get(&err, "hint", 12)?, code.clone());
        if ["not_connected", "gated", "forbidden"]
            .iter()
            .any(|c| super::py::eq(&code, &s(c)))
        {
            return Ok(format!("GATE\t{}\t{}", str_of(&code), str_of(&hint)));
        }
        return Ok(format!("ERROR\t{}", str_of(&or(or(hint, code), err))));
    }
    if !matches!(err, Value::Null) {
        return Ok(format!("ERROR\t{}", str_of(&err)));
    }
    let st = get_or(&d, "status", s(""), 19)?;
    if super::py::eq(&st, &s("sent")) {
        return Ok("SENT".to_string());
    }
    if super::py::eq(&st, &s("queued")) {
        let why = or(or(get(&d, "reason", 24)?, get(&d, "hint", 24)?), s(""));
        let why = match &why {
            Value::Str(_) => {
                let t = str_of(&why);
                s(t.strip_prefix("delivery deferred to retry: ").unwrap_or(&t))
            }
            other => return Err(no_attr(25, other, "startswith")),
        };
        let words: Vec<String> = str_of(&why)
            .split(py_space)
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect();
        return Ok(format!("QUEUED\t{}", words.join(" ")));
    }
    if super::py::eq(&st, &s("declined")) {
        let v = or(
            or(get(&d, "hint", 29)?, get(&d, "reason", 29)?),
            s("declined by peer"),
        );
        return Ok(format!("DECLINED\t{}", str_of(&v)));
    }
    Ok("OTHER".to_string())
}

/// `IFS=$'\t' read -r a b c <<<"$line"`: the first line split at runs of
/// tabs (tabs at either end dropped), the third field the rest.
pub fn read3(text: &str) -> (String, String, String) {
    let line = text.split('\n').next().unwrap_or_default();
    let mut rest = line.trim_matches('\t');
    let mut take = || {
        let (head, tail) = rest.split_once('\t').unwrap_or((rest, ""));
        rest = tail.trim_start_matches('\t');
        head.to_string()
    };
    let a = take();
    let b = take();
    (a, b, rest.to_string())
}

/// `"${v%%$'\t'*}"` and `"${v#*$'\t'}"`, the second empty when there is
/// no tab.
pub fn cut(v: &str) -> (String, String) {
    match v.split_once('\t') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (v.to_string(), String::new()),
    }
}

/// The gate's JSON line: `json.dumps({"error": {"code": code, "hint":
/// hint}}, ensure_ascii=False, separators=(",", ":"))`.
pub fn gate_line(code: &str, hint: &str) -> String {
    let v = Value::Object(vec![(
        "error".into(),
        Value::Object(vec![("code".into(), s(code)), ("hint".into(), s(hint))]),
    )]);
    json::to_string(
        &v,
        &json::Options {
            compact: true,
            ensure_ascii: false,
            ..json::Options::default()
        },
    )
    .unwrap_or_default()
}

/// The send's body, as `json.dumps` writes it.
pub fn body(to: &str, text: &str, from_workspace: &str, key: &str) -> String {
    let v = Value::Object(vec![
        ("to".into(), s(to)),
        ("text".into(), s(text)),
        ("from_workspace".into(), s(from_workspace)),
        ("idempotency_key".into(), s(key)),
    ]);
    super::py::dumps(&v)
}

/// The lines for a peer the reader didn't find (`NONE`).
pub fn no_peer(host: &str, known: &str) -> String {
    if known.is_empty() {
        format!(
            concat!(
                "no paired peer for {0} — pair it first (",
                cli_name!(),
                " fed pair-start / confirm), or check the host.\n",
                "hint: run `",
                cli_name!(),
                " fed peers` to list trusted servers on this machine.\n"
            ),
            host
        )
    } else {
        format!(
            concat!(
                "no paired peer matching {0} — known peers: {1}\n",
                "hint: run `",
                cli_name!(),
                " fed peers` to inspect trust; if you received a message from this host but have no pin, complete pairing (",
                cli_name!(),
                " fed pair-start / confirm, or Settings → Network → Connected Servers → Pair a server) so replies can go back.\n"
            ),
            host, known
        )
    }
}

/// Federation off, or the peers' answer unreadable (`NOFED`, `ERR`).
pub fn no_fed(host: &str) -> String {
    format!(
        concat!(
            "\"Connect agents across servers\" is off or {} can't be reached: turn it on (",
            cli_name!(),
            " fed enable, or Settings → Network → Tunnel → Policies) and pair first (",
            cli_name!(),
            " fed pair-start)."
        ),
        host
    )
}

/// No agent of that name on the peer.
pub fn no_agent(agent: &str, host: &str, names: &str) -> String {
    let first = if names.is_empty() {
        format!("no agent \"{agent}\" on {host} (its roster is empty).")
    } else {
        format!("no agent \"{agent}\" on {host}. Available: {names}.")
    };
    format!(
        concat!(
            "{}\n",
            "hint: if you were connected to it before, its workspace may have been removed there. Handles are never reused; `",
            cli_name!(),
            " connections list` marks such connections GONE.\n"
        ),
        first
    )
}

/// Queued: the two lines.
pub fn queued(shown: &str, host: &str, why: &str) -> String {
    let why = if why.is_empty() {
        "the peer is briefly unreachable"
    } else {
        why
    };
    format!(
        concat!(
            "QUEUED (not delivered yet) for {}::{}: {}\n",
            "  ",
            product_name!(),
            " retries it from this server's outbox (next try in about 30 s). The owner can watch it with: ",
            cli_name!(),
            " fed outbox\n"
        ),
        shown, host, why
    )
}

fn err(text: &str) {
    let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
}

/// `echo "$v" >&2`, as bash's `echo` prints a value that is only its own
/// flags.
fn echo_err(v: &str) {
    let _ = out::write_flushed(
        &mut std::io::stderr().lock(),
        &super::send::echo(v.as_bytes()),
    );
}

fn line(text: &str) {
    out::stderr_line(text);
}

/// `x=$(cli_send cli_request <endpoint> <args>)`: what `$x` holds, or the
/// command's end.
fn request(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    args: &[&str],
) -> Result<String, Exit> {
    let passport = client.passport().is_some();
    match client.send(endpoint, Call::Request(args)) {
        Ok(r) => {
            closing.answer(&r, passport);
            Ok(String::from_utf8_lossy(&send_once::held(&r.body)).into_owned())
        }
        Err(Error::NoAnswer(n)) => {
            line(&n.to_string());
            Err(Exit::Error)
        }
        Err(e) => Err(Exit::from_curl(e.curl_code())),
    }
}

/// A reader's print, held by `$(…)`; a raise prints its traceback and ends
/// the command (exit 1).
fn held(read: Read) -> Result<String, Exit> {
    match read {
        Ok(text) => Ok(text.replace('\0', "")),
        Err(r) => {
            err(&traceback(&r.frames, &r.exception));
            Err(Exit::Error)
        }
    }
}

/// Steps 2 to 8 of the module's notes. `project`: `$PROJECT`.
pub fn send(
    client: &mut Client,
    closing: &mut Closing,
    target: &Target,
    text: &str,
    project: &str,
    key: &Key,
) -> Exit {
    match send_with(client, closing, target, text, project, key) {
        Ok(e) | Err(e) => e,
    }
}

fn send_with(
    client: &mut Client,
    closing: &mut Closing,
    target: &Target,
    text: &str,
    project: &str,
    key: &Key,
) -> Result<Exit, Exit> {
    let host = target.host.as_str();
    if target.agent.is_empty() {
        line(&empty_agent());
        return Ok(Exit::Usage);
    }
    let (agent, chat) = match chat(&target.agent) {
        Ok(v) => v,
        Err(l) => {
            line(&l);
            return Ok(Exit::Usage);
        }
    };
    if target.legacy_at {
        line(&at_hint(&agent, host));
    }
    let raw = request(client, closing, PEERS, &[])?;
    let (kind, fp, trust) = read3(&held(peers(raw.as_bytes(), host))?);
    match kind.as_str() {
        "FORBIDDEN" => {
            line(if fp.is_empty() { "forbidden" } else { &fp });
            return Ok(Exit::Refused);
        }
        "NOFED" | "ERR" => {
            line(&no_fed(host));
            if !fp.is_empty() && kind == "NOFED" {
                line(&format!("detail: {fp}"));
            }
            return Ok(Exit::Usage);
        }
        "NONE" => {
            err(&no_peer(host, &fp));
            return Ok(Exit::Usage);
        }
        "OK" => {}
        _ => {
            line(concat!(
                cli_name!(),
                " msg: could not read federation peers from the local daemon."
            ));
            return Ok(Exit::Usage);
        }
    }
    if fp.is_empty() {
        line(no_peer(host, "").lines().next().unwrap_or_default());
        return Ok(Exit::Usage);
    }
    let trust_lc = trust.to_ascii_lowercase();
    if !trust_lc.is_empty() && trust_lc != "trusted" {
        line(&format!(
            concat!(
                "peer {} is not trusted yet (state={}) — confirm the pairing (",
                cli_name!(),
                " fed confirm)."
            ),
            host, trust
        ));
        return Ok(Exit::Usage);
    }
    let peer = format!("peer={}", quote(&fp));
    let roster_json = request(client, closing, ROSTER, &[peer.as_str()])?;
    let (rkind, rdetail) = cut(&held(roster(roster_json.as_bytes(), &agent))?);
    match rkind.as_str() {
        "FORBIDDEN" => {
            let hint = if rdetail.is_empty() {
                "forbidden"
            } else {
                &rdetail
            };
            line(&gate_line("forbidden", hint));
            return Ok(Exit::Refused);
        }
        "ERR" => {
            let why = if rdetail.is_empty() {
                &roster_json
            } else {
                &rdetail
            };
            line(&format!("could not reach {host}'s roster: {why}"));
            return Ok(Exit::Usage);
        }
        "NONE" => {
            err(&no_agent(&agent, host, &rdetail));
            return Ok(Exit::Usage);
        }
        "AMBIG" => {
            line(&format!("ambiguous agent \"{agent}\" on {host} — multiple workspaces expose it: {rdetail}. Disambiguate on the host."));
            return Ok(Exit::Usage);
        }
        "OK" => {}
        _ => {
            line(&format!("could not parse {host}'s roster."));
            return Ok(Exit::Usage);
        }
    }
    let (wsid, note_handle) = cut(&rdetail);
    if wsid.is_empty() {
        line(&format!(
            "no resolvable workspace for agent \"{agent}\" on {host}."
        ));
        return Ok(Exit::Usage);
    }
    let (wire_agent, shown) = match &chat {
        Some(c) => (format!("{agent}/{c}"), format!("{agent}/{c}")),
        None => (agent.clone(), agent.clone()),
    };
    let body = body(
        &format!("{fp}::{wsid}::{wire_agent}"),
        text,
        project,
        &key.key,
    );
    closing.reset_note();
    let soft = send_once::send(client, closing, key, SEND, Call::PostJson(body.as_bytes()))?;
    if soft.rc != 0 {
        return Err(Exit::from_curl(soft.rc));
    }
    let resp = String::from_utf8_lossy(&soft.out).into_owned();
    if let Some(n) = send_once::replay_note(&soft.out) {
        line(&n);
    }
    let (skind, sdetail) = cut(&held(sent(&soft.out))?);
    let note = || {
        if !note_handle.is_empty() {
            let (old, new) = (format!("{agent}::{host}"), format!("{note_handle}::{host}"));
            if old != new {
                line(&format!("note: {old} is {new}"));
            }
        }
    };
    Ok(match skind.as_str() {
        "SENT" => {
            note();
            if let Err(e) = out::write_flushed(
                &mut std::io::stdout().lock(),
                format!("delivered to {shown}::{host} (federation)\n").as_bytes(),
            ) {
                return Ok(out::after_stdout_error(&e));
            }
            Exit::Ok
        }
        "QUEUED" => {
            note();
            err(&queued(&shown, host, &sdetail));
            Exit::Ok
        }
        "GATE" => {
            let (code, rest) = cut(&sdetail);
            let hint = if sdetail.contains('\t') {
                rest
            } else {
                code.clone()
            };
            line(&gate_line(&code, &hint));
            Exit::Refused
        }
        "DECLINED" => {
            note();
            echo_err(if sdetail.is_empty() { &resp } else { &sdetail });
            Exit::Usage
        }
        "ERROR" => {
            echo_err(if sdetail.is_empty() { &resp } else { &sdetail });
            Exit::Usage
        }
        _ => {
            line(&format!(
                concat!(cli_name!(), " msg: unexpected daemon response: {}"),
                resp
            ));
            Exit::Error
        }
    })
}
