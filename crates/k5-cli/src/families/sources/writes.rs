//! `propose`, `add`, `edit`, `try`, `approve`, `secret`, `pin`, `unpin`
//! and `trigger` (`cli/src/sources.sh`, B1 and B2): their words, their
//! requests and the shell's Python that lays out each answer.
//!
//! `approve` and `secret` outside a K2 terminal ask the person for a
//! vault confirm ([`confirm::confirm_token`]); the token goes from that
//! answer into the next request's body and nowhere else. Inside a K2
//! terminal (a `HOOK_TOKEN` set) no confirm is asked for.

use super::spec::{self, Fail, Mode};
use super::{
    at, call, concat, idx, load, print_json, push_line, Ask, Stop, CLI,
};
use crate::confirm::{self, Confirm};
use crate::exit::Exit;
use crate::families::{py_get, py_iter, py_left, raised, s, Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::out::pylib::{json as pyjson, ops};
use crate::prelude::Closing;
use crate::transport::encode::quote;
use crate::transport::Client;
use k2_client::brand::PRODUCT_NAME;
use std::path::Path;

/// `approve`'s words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approve {
    pub names: Vec<String>,
    pub secret: String,
    pub viewers: String,
    pub access: String,
    pub keep: String,
    pub use_anyway: bool,
    pub json: bool,
}

/// `secret`'s words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bind {
    pub names: Vec<String>,
    pub secret: String,
    pub none: bool,
    pub use_anyway: bool,
    pub json: bool,
}

/// `trigger`'s words.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Trigger {
    pub sub: String,
    pub id: String,
    pub source: String,
    /// `_sources_b2_on`'s JSON text.
    pub on: String,
    pub agent: String,
    pub note: String,
    pub no_note: bool,
    pub cooldown: String,
    pub cap: String,
    pub limit: String,
    pub json: bool,
}

fn usage<T>(line: impl Into<String>) -> Result<T, Stop> {
    Err(Stop::Usage(line.into()))
}

/// A spec failure as the verb's caller takes it: `|| exit <code>` after
/// a traceback, `exit 2` after a `die`.
fn spec_stop(f: Fail, traceback_exit: Exit) -> Stop {
    match f {
        Fail::Die(m) => Stop::Usage(format!("Error: {m}")),
        Fail::Raise(r) => Stop::Raised(r.text(), traceback_exit),
        Fail::Shell => Stop::Shell,
    }
}

/// `propose` or `add`: `_sources_spec_json full "$@" || exit 2`.
pub fn create(route: &'static str, words: &[String]) -> Result<Ask, Stop> {
    let body = spec::spec_json(Mode::Full, words).map_err(|f| spec_stop(f, Exit::Usage))?;
    Ok(Ask::Create { route, body })
}

/// `edit <name> [flags]`.
pub fn edit(words: &[String]) -> Result<Ask, Stop> {
    let (mut name, mut rest) = (String::new(), Vec::new());
    let mut i = 0;
    while i < words.len() {
        let a = &words[i];
        if a.starts_with("--") {
            rest.push(a.clone());
            if !matches!(a.as_str(), "--json" | "--yes") && i + 1 < words.len() {
                rest.push(words[i + 1].clone());
                i += 1;
            }
        } else if name.is_empty() {
            name = a.clone();
        } else {
            rest.push(a.clone());
        }
        i += 1;
    }
    if name.is_empty() {
        return usage(format!("Error: usage: {CLI} sources edit <name> [flags]"));
    }
    let set = spec::spec_json(Mode::Set, &rest).map_err(|f| spec_stop(f, Exit::Usage))?;
    let Ok(Value::Object(mut set)) = json::parse(&set) else {
        return Err(Stop::Shell);
    };
    set.retain(|(k, _)| *k != Text::from("name"));
    let body = dumps(&obj(vec![("name", s(&name)), ("set", Value::Object(set))]));
    Ok(Ask::Edit { body })
}

/// `try`.
pub fn try_(words: &[String]) -> Result<Ask, Stop> {
    let (mut name, mut depth, mut json) = (String::new(), String::new(), false);
    let mut draft: Vec<String> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        match words[i].as_str() {
            "--json" => json = true,
            "--tree-depth" => {
                match words.get(i + 1).filter(|v| !v.is_empty()) {
                    Some(v) => depth = v.clone(),
                    None => return usage("usage: --tree-depth N"),
                }
                i += 1;
            }
            "--name" => {
                let v = words.get(i + 1).ok_or(Stop::Shift)?;
                draft.push("--name".into());
                draft.push(v.clone());
                name = v.clone();
                i += 1;
            }
            other => draft.push(other.to_string()),
        }
        i += 1;
    }
    let body = if draft.len() == 2 && !name.is_empty() {
        dumps(&obj(vec![("name", s(&name))]))
    } else {
        if draft.is_empty() {
            return usage(format!(
                "usage: {CLI} sources try --url URL [flags\u{2026}] | --from file.json | --name <saved>"
            ));
        }
        spec::spec_json(Mode::Draft, &draft).map_err(|f| spec_stop(f, Exit::Error))?
    };
    Ok(Ask::Try { body, depth, json })
}

/// `approve <name>… [flags]`.
pub fn approve(words: &[String]) -> Result<Ask, Stop> {
    let mut a = Approve {
        names: Vec::new(),
        secret: String::new(),
        viewers: String::new(),
        access: String::new(),
        keep: String::new(),
        use_anyway: false,
        json: false,
    };
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        match w {
            "--secret" => {
                match words.get(i + 1).filter(|v| !v.is_empty()) {
                    Some(v) => a.secret = v.clone(),
                    None => return usage("usage: --secret NAME"),
                }
                i += 1;
            }
            "--viewers" | "--agent-access" | "--keep" => {
                let v = words.get(i + 1).ok_or(Stop::Shift)?.clone();
                match w {
                    "--viewers" => a.viewers = v,
                    "--agent-access" => a.access = v,
                    _ => a.keep = v,
                }
                i += 1;
            }
            "--use-anyway" => a.use_anyway = true,
            "--json" => a.json = true,
            _ if w.starts_with('-') => return usage(format!("Error: unknown flag '{w}'")),
            _ => a.names.push(w.to_string()),
        }
        i += 1;
    }
    if a.names.is_empty() {
        return usage(format!("usage: {CLI} sources approve <name>\u{2026} [--secret NAME] [--viewers member|admin] [--agent-access values|shape] [--keep values|states] [--use-anyway]"));
    }
    Ok(Ask::Approve(a))
}

/// `secret <name>… NAME|--none [--use-anyway]`.
pub fn secret(words: &[String]) -> Result<Ask, Stop> {
    let (mut pos, mut none, mut use_anyway, mut json) = (Vec::new(), false, false, false);
    for w in words {
        match w.as_str() {
            "--none" => none = true,
            "--use-anyway" => use_anyway = true,
            "--json" => json = true,
            _ if w.starts_with('-') => return usage(format!("Error: unknown flag '{w}'")),
            _ => pos.push(w.clone()),
        }
    }
    let (mut secret, mut names) = (String::new(), Vec::new());
    if none {
        names = pos;
    } else if pos.len() >= 2 {
        secret = pos.pop().unwrap_or_default();
        names = pos;
    }
    if names.is_empty() {
        return usage(format!(
            "usage: {CLI} sources secret <name>\u{2026} NAME|--none [--use-anyway]"
        ));
    }
    Ok(Ask::Bind(Bind {
        names,
        secret,
        none,
        use_anyway,
        json,
    }))
}

/// `pin` or `unpin` `<name> [--json]`.
pub fn pin(verb: &'static str, words: &[String]) -> Result<Ask, Stop> {
    let name = words.first().cloned().unwrap_or_default();
    if name.is_empty() || name.starts_with('-') {
        return usage(format!("usage: {CLI} sources {verb} <name>"));
    }
    Ok(Ask::Pin {
        verb,
        name,
        json: words.get(1).is_some_and(|w| w == "--json"),
    })
}

/// `_sources_b2_on "$v"`: the trigger's `on` as JSON text.
fn on_json(v: &str) -> Result<String, Stop> {
    let kind = v.split(':').next().unwrap_or_default();
    let whole = kind.len() == v.len();
    match kind {
        "went_crit" | "went_warn" | "recovered" | "item_went_crit" => {
            if !whole {
                return usage(format!("usage: --on {kind} (no duration)"));
            }
            Ok(format!("{{\"kind\":\"{kind}\"}}"))
        }
        "stale_for" => {
            if whole {
                return usage("usage: --on stale_for:30m");
            }
            let secs = super::b2_secs(&v[kind.len() + 1..])?;
            Ok(format!("{{\"kind\":\"stale_for\",\"secs\":{secs}}}"))
        }
        _ => usage("usage: --on went_crit|went_warn|recovered|item_went_crit|stale_for:30m"),
    }
}

/// `trigger [<sub>] …`. `None`: the trigger page.
pub fn trigger(words: &[String]) -> Result<Option<Ask>, Stop> {
    let sub = words.first().cloned().unwrap_or_default();
    if matches!(sub.as_str(), "" | "help" | "--help" | "-h") {
        return Ok(None);
    }
    let mut t = Trigger {
        sub: sub.clone(),
        ..Trigger::default()
    };
    let mut rest = &words[1..];
    if matches!(
        sub.as_str(),
        "show" | "log" | "edit" | "approve" | "pause" | "resume" | "remove" | "rm" | "test"
    ) {
        let id = rest.first().cloned().unwrap_or_default();
        if id.is_empty() || id.starts_with('-') {
            return usage(format!("usage: {CLI} sources trigger {sub} <id>"));
        }
        t.id = id;
        rest = &rest[1..];
    }
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i].as_str();
        let value = || rest.get(i + 1).cloned().ok_or(Stop::Shift);
        match a {
            "--json" => t.json = true,
            "--no-note" => t.no_note = true,
            "--on" => {
                t.on = on_json(rest.get(i + 1).map_or("", String::as_str))?;
                value()?;
                i += 1;
            }
            "--cooldown" => {
                t.cooldown = super::b2_secs(rest.get(i + 1).map_or("", String::as_str))?;
                value()?;
                i += 1;
            }
            "--source" | "--agent" | "--note" | "--daily-cap" | "--limit" => {
                let v = value()?;
                match a {
                    "--source" => t.source = v,
                    "--agent" => t.agent = v,
                    "--note" => t.note = v,
                    "--daily-cap" => t.cap = v,
                    _ => t.limit = v,
                }
                i += 1;
            }
            _ => {
                return usage(format!(
                    "Error: unknown flag '{a}' ({CLI} sources trigger --help)"
                ))
            }
        }
        i += 1;
    }
    Ok(Some(Ask::Trigger(t)))
}

fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).unwrap_or_default()
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `urllib.parse.urlencode` of the pairs whose value isn't empty.
fn query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{}={}", plus(k), plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn plus(v: &str) -> String {
    quote(v).replace("%20", "+")
}

/// `_sources_in_k2`: a session token is set.
fn in_k2() -> bool {
    k2_client::env_names::first_nonempty_os_in(k2_client::env_names::HOOK_TOKEN, |n| {
        std::env::var_os(n)
    })
    .is_some()
}

/// `_compute_json K2_CJ_<key>=<value> …`: the pairs in order, empty ones
/// left out; a key ending `_n` is a number (`float` with a `.`, else
/// `int`), `_j` is JSON. `Err`: Python's exception.
fn compute_json(pairs: &[(&str, &str)]) -> Result<String, Raise> {
    let mut out: Vec<(Text, Value)> = Vec::new();
    for (k, v) in pairs {
        if v.is_empty() {
            continue;
        }
        let (key, value) = if let Some(key) = k.strip_suffix("_n") {
            let n = if v.contains('.') {
                ops::float(v).map(Value::Float)
            } else {
                spec::py_int(v).ok().flatten()
            };
            let n = n.ok_or_else(|| {
                let what = if v.contains('.') {
                    "could not convert string to float"
                } else {
                    "invalid literal for int() with base 10"
                };
                Raise::at(14, format!("ValueError: {what}: {}", py::repr(&s(v))))
            })?;
            (key, n)
        } else if let Some(key) = k.strip_suffix("_j") {
            let j = pyjson::loads(v).map_err(|f| Raise::at(18, f.last_line()))?;
            (key, j)
        } else {
            (*k, s(v))
        };
        let key = Text::from(key);
        match out.iter_mut().find(|(x, _)| *x == key) {
            Some((_, old)) => *old = value,
            None => out.push((key, value)),
        }
    }
    Ok(dumps(&Value::Object(out)))
}

/// What the confirm needs, besides the client.
pub struct Vault<'a> {
    pub home: &'a Path,
    pub closing: &'a mut Closing,
}

/// The new verbs, from their ask to their exit. `Err` ends the command.
pub fn go(c: &mut Client, ask: Ask, json_any: bool, v: Vault<'_>) -> Result<Exit, Exit> {
    match ask {
        Ask::Create { route, body } => {
            let sent = call(c, true, &format!("/cli/sources/{route}"), &body, json_any)?;
            if json_any {
                return Ok(Shown::out(print_json(&sent.resp)).print());
            }
            Ok(reader(&sent.resp, created).print())
        }
        Ask::Edit { body } => {
            let sent = call(c, true, "/cli/sources/edit", &body, json_any)?;
            if json_any {
                return Ok(Shown::out(print_json(&sent.resp)).print());
            }
            Ok(reader(&sent.resp, edited).print())
        }
        Ask::Try { body, depth, json } => {
            let sent = call(c, true, "/cli/sources/try", &body, json)?;
            if json {
                return Ok(Shown::out(print_json(&sent.resp)).print());
            }
            Ok(tried(&sent.resp, &depth).print())
        }
        Ask::Approve(a) => approve_go(c, a, v),
        Ask::Bind(b) => bind_go(c, b, v),
        Ask::Pin { verb, name, json } => {
            let body = compute_json(&[("name", name.as_str())]).unwrap_or_default();
            let sent = call(c, true, &format!("/cli/sources/{verb}"), &body, json)?;
            if json {
                return Ok(Shown::out(print_json(&sent.resp)).print());
            }
            Ok(reader(&sent.resp, pinned).print())
        }
        Ask::Trigger(t) => trigger_go(c, t),
        _ => Err(Exit::Error),
    }
}

/// A Python reader on the body: `json.load` at line 3, then `lines`.
fn reader(body: &[u8], lines: Lines) -> Shown {
    let mut o = String::new();
    match load(body, 3).and_then(|v| lines(&v, &mut o)) {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

/// `print("%s: %s" % (d["name"], d["status"]))`.
fn name_status(d: &Value, l: u32) -> Result<(Value, Value), Raise> {
    let name = idx(d, "name", l)?;
    let status = idx(d, "status", l)?;
    Ok((name, status))
}

/// `for w in d.get("warnings") or []: print("warning: " + w["hint"])`.
fn warnings(d: &Value, o: &mut String, l: u32) -> Result<(), Raise> {
    let ws = at(d, "warnings", l)?;
    if !py::truthy(&ws) {
        return Ok(());
    }
    for w in py_iter(&ws, l)? {
        let hint = idx(&w, "hint", l + 1)?;
        push_line(o, &concat(&s("warning: "), &hint, l + 1)?);
    }
    Ok(())
}

/// `_sources_create`'s reader.
fn created(d: &Value, o: &mut String) -> Result<(), Raise> {
    let approval = at(d, "approval", 4)?;
    let how = match &approval {
        Value::Array(_) | Value::Object(_) => {
            return Err(Raise::at(
                4,
                format!("TypeError: unhashable type: '{}'", py::type_name(&approval)),
            ))
        }
        Value::Str(t) => match t.as_str() {
            Some("one_click") => format!("one click in Settings \u{2192} Gardens \u{2192} a Garden \u{2192} External Data (or {CLI} sources approve)"),
            Some("needs_confirm") => format!("a person binds the secret with a confirm (Settings, or {CLI} sources approve --secret)"),
            Some("owner_only") => "the server owner approves it".to_string(),
            _ => String::new(),
        },
        _ => String::new(),
    };
    let (name, status) = name_status(d, 7)?;
    push_line(o, &format!("{}: {}", py::str_of(&name), py::str_of(&status)));
    let status = idx(d, "status", 8)?;
    if ops::py_eq(&status, &s("proposed")) && !how.is_empty() {
        push_line(o, &format!("Waiting for approval: {how}"));
    }
    warnings(d, o, 10)
}

/// `cmd_sources_edit`'s reader.
fn edited(d: &Value, o: &mut String) -> Result<(), Raise> {
    let (name, status) = name_status(d, 3)?;
    push_line(o, &format!("{}: {}", py::str_of(&name), py::str_of(&status)));
    if py::truthy(&at(d, "unbound", 4)?) {
        push_line(
            o,
            "The secret was unbound; a person approves it again with a confirm.",
        );
    }
    warnings(d, o, 6)
}

/// `str.count("/")` (a list counts its items equal to `"/"`).
fn count_slashes(p: &Value, l: u32) -> Result<usize, Raise> {
    match p {
        Value::Str(_) => Ok(py::str_of(p).matches('/').count()),
        Value::Array(items) => Ok(items.iter().filter(|x| ops::py_eq(x, &s("/"))).count()),
        other => Err(Raise::no_attribute(l, other, "count")),
    }
}

/// `try`'s reader, `depth` as `sys.argv[1]`.
fn tried(body: &[u8], depth: &str) -> Shown {
    let mut o = String::new();
    let r = (|| -> Result<(), Raise> {
        let depth: i128 = if depth.is_empty() {
            99
        } else {
            match spec::py_int(depth) {
                Ok(Some(Value::Int(d))) => d.parse().unwrap_or(i128::MAX),
                _ => {
                    return Err(Raise::at(
                        3,
                        format!(
                            "ValueError: invalid literal for int() with base 10: {}",
                            py::repr(&s(depth))
                        ),
                    ))
                }
            }
        };
        let v = load(body, 4)?;
        let head = if py::truthy(&at(&v, "ok", 5)?) {
            "ok".to_string()
        } else {
            let reason = at(&v, "reason", 5)?;
            let reason = if py::truthy(&reason) { reason } else { s("error") };
            format!("failed: {}", py::str_of(&reason))
        };
        let status = at(&v, "status", 6)?;
        let ms = at(&v, "elapsedMs", 6)?;
        let cut = if py::truthy(&at(&v, "truncated", 6)?) {
            " \u{b7} truncated"
        } else {
            ""
        };
        push_line(
            &mut o,
            &format!(
                "{head} \u{b7} HTTP {} \u{b7} {} ms{cut}",
                py::str_of(&status),
                py::str_of(&ms)
            ),
        );
        let targets = at(&v, "targets", 7)?;
        if py::truthy(&targets) {
            for t in py_iter(&targets, 7)? {
                let key = at(&t, "key", 8)?;
                let st = at(&t, "status", 8)?;
                let reason = at(&t, "reason", 8)?;
                let reason = if py::truthy(&reason) { reason } else { s("") };
                let ms = at(&t, "elapsedMs", 8)?;
                push_line(
                    &mut o,
                    &format!(
                        "  target {} {} {} {} ms",
                        py_left(&key, 20),
                        py::str_of(&st),
                        py::str_of(&reason),
                        py::str_of(&ms)
                    ),
                );
            }
        }
        let ws = at(&v, "warnings", 9)?;
        if py::truthy(&ws) {
            for w in py_iter(&ws, 9)? {
                let code = at(&w, "code", 10)?;
                let hint = at(&w, "hint", 10)?;
                push_line(
                    &mut o,
                    &format!("warning: {}: {}", py::str_of(&code), py::str_of(&hint)),
                );
            }
        }
        if py::truthy(&at(&v, "rootKey", 11)?) {
            let rk = idx(&v, "rootKey", 12)?;
            let hint = at(&rk, "hint", 12)?;
            push_line(&mut o, &format!("warning: {}", py::str_of(&hint)));
        }
        let tree = at(&v, "tree", 13)?;
        let tree = if py::truthy(&tree) { tree } else { Value::Array(Vec::new()) };
        if py::truthy(&tree) {
            push_line(&mut o, "shape (pointer \u{b7} type \u{b7} sample):");
        }
        for n in py_iter(&tree, 16)? {
            let p = at(&n, "pointer", 17)?;
            let p = if py::truthy(&p) { p } else { s("/") };
            if count_slashes(&p, 18)? as i128 > depth {
                continue;
            }
            let mut extra = String::new();
            if !matches!(at(&n, "len", 21)?, Value::Null) {
                extra = format!(" len {}", py::str_of(&idx(&n, "len", 22)?));
            }
            if ops::py_in(&s("sample"), &n, 23)? {
                let sample = idx(&n, "sample", 24)?;
                let text = json::to_string(
                    &sample,
                    &Options {
                        ensure_ascii: false,
                        ..Options::default()
                    },
                )
                .unwrap_or_default();
                extra = format!(" {}", text.chars().take(80).collect::<String>());
            }
            let ty = at(&n, "type", 25)?;
            push_line(
                &mut o,
                &format!("  {} \u{b7} {}{extra}", py::str_of(&p), py::str_of(&ty)),
            );
        }
        if py::truthy(&at(&v, "treeTruncated", 26)?) {
            push_line(&mut o, "  \u{2026} (the tree stops at 500 nodes)");
        }
        let view = at(&v, "view", 28)?;
        if py::truthy(&view) {
            let display = at(&view, "display", 30)?;
            let mut parts = Vec::new();
            for k in ["ok", "warn", "crit", "unknown"] {
                let counts = py_get(&view, "counts", 30)?
                    .cloned()
                    .unwrap_or(Value::Object(Vec::new()));
                let n = at(&counts, k, 30)?;
                parts.push(format!("{k} {}", py::str_of(&n)));
            }
            push_line(
                &mut o,
                &format!(
                    "would read: {} \u{b7} {}",
                    py::str_of(&display),
                    parts.join(", ")
                ),
            );
        }
        push_line(
            &mut o,
            &format!("Nothing was saved. Propose it with: {CLI} sources propose \u{2026}"),
        );
        Ok(())
    })();
    match r {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

fn pinned(v: &Value, o: &mut String) -> Result<(), Raise> {
    let name = idx(v, "name", 4)?;
    let what = if py::truthy(&at(v, "pinned", 4)?) {
        "pinned (agents can no longer edit or remove it)"
    } else {
        "not pinned"
    };
    push_line(o, &format!("{}: {what}", py::str_of(&name)));
    Ok(())
}

/// `printf '%s' "$x"` into a `$(…)`: the value's text, trailing newlines
/// gone.
fn sub_out(text: &str) -> String {
    text.trim_end_matches('\n').to_string()
}

/// `IFS=$'\t' read -r a b c d e <<<"$line"`: tabs are IFS white space, so
/// runs of them are one break and none lead or trail.
fn read_tabs(line: &str, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line.trim_matches('\t');
    while out.len() + 1 < n {
        match rest.split_once('\t') {
            Some((a, b)) => {
                out.push(a.to_string());
                rest = b.trim_start_matches('\t');
            }
            None => {
                out.push(rest.to_string());
                rest = "";
                break;
            }
        }
    }
    if out.len() + 1 == n {
        out.push(rest.to_string());
    }
    while out.len() < n {
        out.push(String::new());
    }
    out
}

/// The approve plan's Python: `missing\t<names>` or
/// `ok\t<items>\t<hash>\t<first id>\t<1 when a secret is bound>`.
fn plan(picked: &str, body: &[u8], joined: &str) -> Result<String, Raise> {
    let v = load(body, 4)?;
    let want: Vec<&str> = joined.split(',').collect();
    let sources = at(&v, "sources", 6)?;
    let sources = if py::truthy(&sources) { sources } else { Value::Array(Vec::new()) };
    let mut rows: Vec<(String, Value)> = Vec::new();
    for row in py_iter(&sources, 6)? {
        let key = idx(&row, "name", 6)?;
        match &key {
            Value::Array(_) | Value::Object(_) => {
                return Err(Raise::at(
                    6,
                    format!("TypeError: unhashable type: '{}'", py::type_name(&key)),
                ))
            }
            Value::Str(t) => {
                let k = py::str_of(&Value::Str(t.clone()));
                rows.retain(|(x, _)| *x != k);
                rows.push((k, row));
            }
            _ => {}
        }
    }
    let find = |n: &str| rows.iter().find(|(k, _)| k == n).map(|(_, r)| r.clone());
    let missing: Vec<&str> = want.iter().copied().filter(|n| find(*n).is_none()).collect();
    if !missing.is_empty() {
        return Ok(format!("missing\t{}", missing.join(",")));
    }
    let (mut items, mut pairs) = (Vec::new(), Vec::new());
    for n in &want {
        let row = find(*n).unwrap_or(Value::Null);
        let mut sec = at(&row, "urlSecret", 13)?;
        if !py::truthy(&sec) {
            sec = if picked.is_empty() { at(&row, "secret", 13)? } else { s(picked) };
        }
        let hash = idx(&row, "specHash", 14)?;
        let mut it = vec![(Text::from("name"), s(n)), (Text::from("specHash"), hash)];
        if py::truthy(&sec) {
            let id = idx(&row, "id", 17)?;
            pairs.push(format!("{}:{}", py::str_of(&id), py::str_of(&sec)));
            it.push((Text::from("secret"), sec));
        }
        items.push(Value::Object(it));
    }
    pairs.sort();
    let h = if pairs.is_empty() {
        String::new()
    } else {
        hex(&crate::families::fed::pubkey::sha256(pairs.join("\n").as_bytes()))
    };
    let first = find(want[0]).unwrap_or(Value::Null);
    let id = idx(&first, "id", 22)?;
    Ok(format!(
        "ok\t{}\t{h}\t{}\t{}",
        dumps(&Value::Array(items)),
        py::str_of(&id),
        if pairs.is_empty() { "" } else { "1" }
    ))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `tok="$(_sources_b2_confirm <action> <id> <label>)" || exit 3`.
fn confirm_tok(
    c: &mut Client,
    v: &mut Vault<'_>,
    action: &str,
    id: &str,
    label: &str,
) -> Result<String, Exit> {
    if in_k2() {
        return Ok(String::new());
    }
    let passport = c.passport().is_some();
    let sock = confirm::sock_path(v.home);
    let mut cf = Confirm {
        client: c,
        sock: &sock,
        closing: &mut *v.closing,
        passport,
    };
    confirm::confirm_token(&mut cf, action, id, label)
        .map(|t| sub_out(&t))
        .ok_or(Exit::Refused)
}

fn approve_go(c: &mut Client, a: Approve, mut v: Vault<'_>) -> Result<Exit, Exit> {
    let joined = a.names.join(",");
    let sent = call(
        c,
        false,
        "/cli/sources/pending",
        &query(&[("names", joined.as_str())]),
        a.json,
    )?;
    let line = plan(&a.secret, &sent.resp, &joined)
        .map_err(|r| raised(String::new(), String::new(), &r).print())?;
    let f = read_tabs(&line, 5);
    if f[0] == "missing" {
        crate::out::stderr_line(&format!(
            "Error: source_not_found: not waiting for approval: {} ({CLI} sources pending)",
            f[1]
        ));
        return Err(Exit::NotFound);
    }
    let items = match json::parse(&f[1]) {
        Ok(Value::Array(items)) => items,
        _ => return Err(Exit::Error),
    };
    let (hash, first_id, needs) = (&f[2], &f[3], !f[4].is_empty());
    let sent = if a.names.len() == 1 {
        let tok = if needs {
            confirm_tok(c, &mut v, "source-secret", first_id, &a.names[0])?
        } else {
            String::new()
        };
        let Some(Value::Object(mut it)) = items.into_iter().next() else {
            return Err(Exit::Error);
        };
        for (k, val) in [
            ("confirmToken", tok.as_str()),
            ("viewers", a.viewers.as_str()),
            ("agentAccess", a.access.as_str()),
            ("keep", a.keep.as_str()),
        ] {
            if !val.is_empty() {
                set(&mut it, k, s(val));
            }
        }
        if a.use_anyway {
            set(&mut it, "useAnyway", Value::Bool(true));
        }
        let body = dumps(&Value::Object(it));
        call(c, true, "/cli/sources/approve", &body, a.json)?
    } else {
        if !(a.viewers.is_empty() && a.access.is_empty() && a.keep.is_empty()) {
            crate::out::stderr_line(
                "usage: --viewers/--agent-access/--keep apply to one source at a time",
            );
            return Err(Exit::Usage);
        }
        let tok = if needs {
            let label = format!("{} sources", a.names.len());
            confirm_tok(c, &mut v, "source-batch", hash, &label)?
        } else {
            String::new()
        };
        let mut b = vec![
            (Text::from("items"), Value::Array(items)),
            (Text::from("triggers"), Value::Array(Vec::new())),
        ];
        if !tok.is_empty() {
            b.push((Text::from("confirmToken"), s(&tok)));
        }
        if a.use_anyway {
            b.push((Text::from("useAnyway"), Value::Bool(true)));
        }
        let body = dumps(&Value::Object(b));
        call(c, true, "/cli/sources/batch-approve", &body, a.json)?
    };
    if a.json {
        return Ok(Shown::out(print_json(&sent.resp)).print());
    }
    Ok(reader(&sent.resp, approved).print())
}

fn set(d: &mut Vec<(Text, Value)>, k: &str, v: Value) {
    let key = Text::from(k);
    match d.iter_mut().find(|(x, _)| *x == key) {
        Some((_, old)) => *old = v,
        None => d.push((key, v)),
    }
}

fn approved(v: &Value, o: &mut String) -> Result<(), Raise> {
    if ops::py_in(&s("approved"), v, 4)? {
        let list = idx(v, "approved", 5)?;
        push_line(o, &format!("approved: {}", ops::join(&list, ", ", 5)?));
    } else {
        let name = at(v, "name", 7)?;
        let bound = if py::truthy(&at(v, "secretBound", 7)?) {
            " \u{b7} secret bound"
        } else {
            ""
        };
        push_line(o, &format!("{}: active{bound}", py::str_of(&name)));
    }
    Ok(())
}

/// `sid="$(… json.load(sys.stdin)["source"].get("id") or "")"`.
fn source_id(body: &[u8]) -> Result<String, Raise> {
    let v = load(body, 1)?;
    let src = idx(&v, "source", 1)?;
    let id = at(&src, "id", 1)?;
    Ok(if py::truthy(&id) {
        sub_out(&py::str_of(&id))
    } else {
        String::new()
    })
}

/// `"$(printf '%s\n' "${names[@]}")"`, then `.splitlines()`.
fn name_lines(names: &[String]) -> Vec<String> {
    let mut text = String::new();
    for n in names {
        text.push_str(n);
        text.push('\n');
    }
    ops::splitlines(text.trim_end_matches('\n'))
}

fn bind_go(c: &mut Client, b: Bind, mut v: Vault<'_>) -> Result<Exit, Exit> {
    let tb = |r: Raise| raised(String::new(), String::new(), &r).print();
    let anyway = |d: &mut Vec<(Text, Value)>| {
        if b.use_anyway {
            d.push((Text::from("useAnyway"), Value::Bool(true)));
        }
    };
    let multi = b.names.len() > 1;
    let body = if multi {
        let names = Value::Array(name_lines(&b.names).iter().map(|n| s(n)).collect());
        if b.none || in_k2() {
            let secret = if b.secret.is_empty() { Value::Null } else { s(&b.secret) };
            let mut d = vec![(Text::from("names"), names), (Text::from("secret"), secret)];
            anyway(&mut d);
            d
        } else {
            let mut ids = String::new();
            for n in &b.names {
                let sent = call(c, false, "/cli/sources/show", &query(&[("name", n.as_str())]), b.json)?;
                let sid = source_id(&sent.resp).map_err(tb)?;
                if sid.is_empty() {
                    crate::out::stderr_line(&format!(
                        "Error: the daemon didn't name the source id for {n} (run this outside {PRODUCT_NAME})"
                    ));
                    return Err(Exit::Refused);
                }
                ids.push_str(&sid);
                ids.push('\n');
            }
            let mut lines: Vec<String> = ops::splitlines(&ids)
                .into_iter()
                .filter(|i| !i.is_empty())
                .map(|i| format!("{i}:{}", b.secret))
                .collect();
            lines.sort();
            let hash = hex(&crate::families::fed::pubkey::sha256(lines.join("\n").as_bytes()));
            let label = format!("{} sources", b.names.len());
            let tok = confirm_tok(c, &mut v, "source-batch", &hash, &label)?;
            let mut d = vec![
                (Text::from("names"), names),
                (Text::from("secret"), s(&b.secret)),
                (Text::from("confirmToken"), s(&tok)),
            ];
            anyway(&mut d);
            d
        }
    } else {
        let name = &b.names[0];
        if b.none {
            vec![(Text::from("name"), s(name)), (Text::from("secret"), Value::Null)]
        } else if in_k2() {
            let mut d = vec![(Text::from("name"), s(name)), (Text::from("secret"), s(&b.secret))];
            anyway(&mut d);
            d
        } else {
            let sent = call(c, false, "/cli/sources/show", &query(&[("name", name.as_str())]), b.json)?;
            let sid = source_id(&sent.resp).map_err(tb)?;
            if sid.is_empty() {
                crate::out::stderr_line(
                    &format!("Error: the daemon didn't name the source id (run this outside {PRODUCT_NAME})"),
                );
                return Err(Exit::Refused);
            }
            let tok = confirm_tok(c, &mut v, "source-secret", &sid, name)?;
            let mut d = vec![
                (Text::from("name"), s(name)),
                (Text::from("secret"), s(&b.secret)),
                (Text::from("confirmToken"), s(&tok)),
            ];
            anyway(&mut d);
            d
        }
    };
    let sent = call(c, true, "/cli/sources/secret", &dumps(&Value::Object(body)), b.json)?;
    if b.json {
        return Ok(Shown::out(print_json(&sent.resp)).print());
    }
    let lines = if multi { bound_many } else { bound_one };
    Ok(reader(&sent.resp, lines).print())
}

fn bound_many(v: &Value, o: &mut String) -> Result<(), Raise> {
    let names = ops::join(&idx(v, "names", 4)?, ", ", 4)?;
    let what = if py::truthy(&at(v, "secretBound", 4)?) {
        concat(&s("use "), &idx(v, "secret", 4)?, 4)?
    } else {
        "no secret".to_string()
    };
    push_line(o, &format!("{names}: {what}"));
    Ok(())
}

fn bound_one(v: &Value, o: &mut String) -> Result<(), Raise> {
    let name = idx(v, "name", 4)?;
    let what = if py::truthy(&at(v, "secretBound", 4)?) {
        concat(&s("uses "), &idx(v, "secret", 4)?, 4)?
    } else {
        "no secret".to_string()
    };
    push_line(o, &format!("{}: {what}", py::str_of(&name)));
    Ok(())
}

/// `time.strftime(fmt, time.gmtime(t))` for `%Y-%m-%d %H:%M UTC` (full)
/// or `%m-%d %H:%M`.
fn gm(t: &Value, full: bool, l: u32) -> Result<String, Raise> {
    let secs: i64 = match t {
        Value::Bool(b) => i64::from(*b),
        Value::Int(d) => d.parse().map_err(|_| {
            Raise::at(l, "OverflowError: timestamp out of range for platform time_t")
        })?,
        Value::Float(f) if f.is_nan() => {
            return Err(Raise::at(l, "ValueError: Invalid value NaN (not a number)"))
        }
        Value::Float(f) if f.is_infinite() => {
            return Err(Raise::at(
                l,
                "OverflowError: cannot convert float infinity to integer",
            ))
        }
        Value::Float(f) => f.floor() as i64,
        other => {
            return Err(Raise::at(
                l,
                format!(
                    "TypeError: '{}' object cannot be interpreted as an integer",
                    py::type_name(other)
                ),
            ))
        }
    };
    if !(-62_135_596_800..=253_402_300_799).contains(&secs) {
        return Err(Raise::at(l, "ValueError: year is out of range"));
    }
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let hm = format!("{:02}:{:02}", rem / 3600, rem % 3600 / 60);
    Ok(if full {
        format!("{y:04}-{m:02}-{d:02} {hm} UTC")
    } else {
        format!("{m:02}-{d:02} {hm}")
    })
}

/// `x[:n]`.
fn head(v: &Value, n: usize, l: u32) -> Result<String, Raise> {
    match v {
        Value::Str(t) => Ok(py::str_of(&Value::Str(t.clone())).chars().take(n).collect()),
        Value::Array(items) => Ok(py::str_of(&Value::Array(
            items.iter().take(n).cloned().collect(),
        ))),
        Value::Object(_) => Err(Raise::at(l, "TypeError: unhashable type: 'slice'")),
        other => Err(Raise::at(
            l,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `_sources_b2_trigger_line`.
fn trigger_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let rows = if ops::py_in(&s("triggers"), v, 4)? {
        at(v, "triggers", 4)?
    } else {
        Value::Array(vec![idx(v, "trigger", 4)?])
    };
    if !py::truthy(&rows) {
        push_line(o, "No triggers.");
        return Ok(());
    }
    for t in py_iter(&rows, 7)? {
        let on_v = idx(&t, "on", 8)?;
        let kind = idx(&on_v, "kind", 8)?;
        let secs = if py::truthy(&at(&on_v, "secs", 8)?) {
            s(&format!(":{}s", py::str_of(&idx(&on_v, "secs", 8)?)))
        } else {
            s("")
        };
        let on = concat(&kind, &secs, 8)?;
        let a = idx(&t, "agent", 9)?;
        let missing = if py::truthy(&at(&a, "missing", 10)?) {
            " (missing workspace)"
        } else {
            ""
        };
        let who = concat(&at(&a, "handle", 10)?, &s(missing), 10)?;
        let mut last = String::new();
        if py::truthy(&at(&t, "lastFireAt", 12)?) {
            last = format!(
                " \u{b7} last fire {}",
                gm(&idx(&t, "lastFireAt", 13)?, true, 13)?
            );
        }
        let id = head(&idx(&t, "id", 15)?, 8, 15)?;
        let source = idx(&t, "source", 15)?;
        let status = idx(&t, "status", 15)?;
        let cool = idx(&t, "cooldownSecs", 15)?;
        let cap = idx(&t, "dailyCap", 15)?;
        let today = py_get(&t, "firesToday", 15)?
            .cloned()
            .unwrap_or(Value::Int("0".into()));
        push_line(
            o,
            &format!(
                "{id}  {} {} \u{2192} {} {} cooldown {}s \u{b7} cap {}/day \u{b7} {} today{last}",
                py_left(&source, 14),
                py_left(&s(&on), 18),
                py_left(&s(&who), 20),
                py_left(&status, 13),
                py::str_of(&cool),
                py::str_of(&cap),
                py::str_of(&today)
            ),
        );
        if py::truthy(&at(&t, "note", 16)?) {
            push_line(
                o,
                &format!("          note: {}", py::str_of(&idx(&t, "note", 17)?)),
            );
        }
    }
    Ok(())
}

/// `_sources_b2_fires`.
fn fire_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let fires = at(v, "fires", 4)?;
    if !py::truthy(&fires) {
        push_line(o, "No fires yet.");
        return Ok(());
    }
    for f in py_iter(&fires, 7)? {
        let t = gm(&idx(&f, "at", 8)?, false, 8)?;
        let mut what = idx(&f, "outcome", 9)?;
        let add = |what: &Value, more: String, l: u32| -> Result<Value, Raise> {
            Ok(s(&concat(what, &s(&more), l)?))
        };
        if py::truthy(&at(&f, "suppressReason", 10)?) {
            let r = py::str_of(&idx(&f, "suppressReason", 10)?);
            what = add(&what, format!(" ({r})"), 10)?;
        }
        if py::truthy(&at(&f, "detail", 11)?) {
            let d = py::str_of(&idx(&f, "detail", 11)?);
            what = add(&what, format!(" [{d}]"), 11)?;
        }
        if py::truthy(&at(&f, "woke", 12)?) {
            what = add(&what, " \u{b7} woke".to_string(), 12)?;
        }
        let incident = head(&idx(&f, "incidentId", 13)?, 12, 13)?;
        let item = if py::truthy(&at(&f, "itemKey", 13)?) {
            concat(&s(" item "), &idx(&f, "itemKey", 13)?, 13)?
        } else {
            String::new()
        };
        push_line(
            o,
            &format!("{t}  {} incident {incident}{item}", py_left(&what, 36)),
        );
    }
    Ok(())
}

/// The reader after approve, pause, resume, remove and test.
fn trigger_done(v: &Value, o: &mut String) -> Result<(), Raise> {
    if ops::py_in(&s("outcome"), v, 4)? {
        let woke = if py::truthy(&at(v, "woke", 5)?) {
            " (woke the agent)"
        } else {
            ""
        };
        push_line(
            o,
            &format!("test wake: {}{woke}", py::str_of(&idx(v, "outcome", 5)?)),
        );
    } else if py::truthy(&at(v, "removed", 6)?) {
        push_line(o, &format!("trigger {} removed", py::str_of(&idx(v, "id", 7)?)));
    } else {
        let st = at(v, "status", 9)?;
        push_line(
            o,
            &format!("trigger {}: {}", py::str_of(&idx(v, "id", 9)?), py::str_of(&st)),
        );
    }
    Ok(())
}

/// The reader after propose and add.
fn trigger_made(v: &Value, o: &mut String) -> Result<(), Raise> {
    let id = idx(v, "id", 4)?;
    let status = idx(v, "status", 4)?;
    push_line(
        o,
        &format!("trigger {}: {}", py::str_of(&id), py::str_of(&status)),
    );
    if ops::py_eq(&idx(v, "status", 5)?, &s("proposed")) {
        let short = head(&idx(v, "id", 6)?, 8, 6)?;
        push_line(o, &format!("A person approves it in Settings \u{2192} Gardens \u{2192} a Garden \u{2192} External Data (or {CLI} sources trigger approve {short})."));
    }
    Ok(())
}

/// The id a short prefix names in the list, or `""`.
fn full_id(body: &[u8], prefix: &str) -> Result<String, Raise> {
    let v = load(body, 3)?;
    let list = at(&v, "triggers", 3)?;
    let list = if py::truthy(&list) { list } else { Value::Array(Vec::new()) };
    let mut m = Vec::new();
    for t in py_iter(&list, 3)? {
        let id = idx(&t, "id", 3)?;
        let starts = match &id {
            Value::Str(_) => py::str_of(&id).starts_with(prefix),
            other => return Err(Raise::no_attribute(3, other, "startswith")),
        };
        if starts {
            m.push(idx(&t, "id", 3)?);
        }
    }
    Ok(if m.len() == 1 {
        sub_out(&py::str_of(&m[0]))
    } else {
        String::new()
    })
}

/// A reader's lines.
type Lines = fn(&Value, &mut String) -> Result<(), Raise>;

fn trigger_go(c: &mut Client, mut t: Trigger) -> Result<Exit, Exit> {
    let tb = |r: Raise| raised(String::new(), String::new(), &r).print();
    if !t.id.is_empty() && t.id.chars().count() < 36 {
        let sent = call(c, false, "/cli/sources/triggers/list", "", t.json)?;
        let full = full_id(&sent.resp, &t.id).map_err(tb)?;
        if full.is_empty() {
            crate::out::stderr_line(&format!(
                "Error: trigger_not_found: no single trigger starts with '{}' ({CLI} sources trigger list)",
                t.id
            ));
            return Err(Exit::NotFound);
        }
        t.id = full;
    }
    let shown = |sent: &super::Sent, lines: &[Lines]| {
        if t.json {
            return Shown::out(print_json(&sent.resp)).print();
        }
        for l in lines {
            let e = reader(&sent.resp, *l).print();
            if e != Exit::Ok {
                return e;
            }
        }
        Exit::Ok
    };
    match t.sub.as_str() {
        "list" | "ls" => {
            let q = query(&[("source", t.source.as_str())]);
            let sent = call(c, false, "/cli/sources/triggers/list", &q, t.json)?;
            Ok(shown(&sent, &[trigger_lines as Lines]))
        }
        "show" => {
            let q = query(&[("id", t.id.as_str())]);
            let sent = call(c, false, "/cli/sources/triggers/show", &q, t.json)?;
            Ok(shown(&sent, &[trigger_lines as Lines, fire_lines]))
        }
        "log" => {
            let q = query(&[("id", t.id.as_str()), ("limit", t.limit.as_str())]);
            let sent = call(c, false, "/cli/sources/triggers/log", &q, t.json)?;
            Ok(shown(&sent, &[fire_lines as Lines]))
        }
        "propose" | "add" => {
            if t.source.is_empty() || t.on.is_empty() {
                crate::out::stderr_line(&format!(
                    "usage: {CLI} sources trigger {} --source NAME --on KIND [--agent HANDLE]",
                    t.sub
                ));
                return Err(Exit::Usage);
            }
            // A bad number: Python's traceback, then the POST goes with an
            // empty body (the `$(…)` is an argument, not an assignment).
            let body = compute_json(&[
                ("source", t.source.as_str()),
                ("on_j", t.on.as_str()),
                ("agent", t.agent.as_str()),
                ("note", t.note.as_str()),
                ("cooldownSecs_n", t.cooldown.as_str()),
                ("dailyCap_n", t.cap.as_str()),
            ])
            .unwrap_or_else(|r| {
                let _ = crate::out::write_flushed(&mut std::io::stderr().lock(), r.text().as_bytes());
                String::new()
            });
            let route = format!("/cli/sources/triggers/{}", t.sub);
            let sent = call(c, true, &route, &body, t.json)?;
            Ok(shown(&sent, &[trigger_made as Lines]))
        }
        "edit" => {
            let set = compute_json(&[
                ("on_j", t.on.as_str()),
                ("agent", t.agent.as_str()),
                ("note", t.note.as_str()),
                ("cooldownSecs_n", t.cooldown.as_str()),
                ("dailyCap_n", t.cap.as_str()),
            ])
            .map_err(tb)?;
            let Ok(Value::Object(mut set)) = json::parse(&set) else {
                return Err(Exit::Error);
            };
            if t.no_note {
                self::set(&mut set, "note", Value::Null);
            }
            let body = dumps(&obj(vec![("id", s(&t.id)), ("set", Value::Object(set))]));
            let sent = call(c, true, "/cli/sources/triggers/edit", &body, t.json)?;
            Ok(shown(&sent, &[trigger_lines as Lines]))
        }
        "approve" | "pause" | "resume" | "remove" | "rm" | "test" => {
            let verb = if t.sub == "rm" { "remove" } else { t.sub.as_str() };
            let body = compute_json(&[("id", t.id.as_str())]).unwrap_or_default();
            let route = format!("/cli/sources/triggers/{verb}");
            let sent = call(c, true, &route, &body, t.json)?;
            Ok(shown(&sent, &[trigger_done as Lines]))
        }
        other => {
            crate::out::stderr_line(&format!(
                "Error: unknown {CLI} sources trigger command '{other}' ({CLI} sources trigger --help)"
            ));
            Err(Exit::Usage)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_splits_on_runs_of_tabs() {
        assert_eq!(read_tabs("ok\t[1]\th\tid\t1", 5), ["ok", "[1]", "h", "id", "1"]);
        // No secret: the empty hash and flag fold away.
        assert_eq!(read_tabs("ok\t[1]\t\tid\t", 5), ["ok", "[1]", "id", "", ""]);
        assert_eq!(read_tabs("missing\ta,b", 5), ["missing", "a,b", "", "", ""]);
    }

    #[test]
    fn utc_as_gmtime_gives_it() {
        let t = |v: &str| gm(&Value::Int(v.into()), true, 1).unwrap();
        assert_eq!(t("0"), "1970-01-01 00:00 UTC");
        assert_eq!(t("1763600000"), "2025-11-20 00:53 UTC");
        assert_eq!(gm(&Value::Float(59.9), false, 1).unwrap(), "01-01 00:00");
        assert_eq!(gm(&Value::Int("-1".into()), false, 1).unwrap(), "12-31 23:59");
    }

    #[test]
    fn compute_json_as_python_prints_it() {
        assert_eq!(
            compute_json(&[("source", "w"), ("on_j", r#"{"kind":"went_crit"}"#), ("agent", ""), ("cooldownSecs_n", "007")]).unwrap(),
            r#"{"source": "w", "on": {"kind": "went_crit"}, "cooldownSecs": 7}"#
        );
        assert!(compute_json(&[("dailyCap_n", "x")]).is_err());
        assert!(compute_json(&[("on_j", r#"{"secs":007}"#)]).is_err());
    }

    #[test]
    fn the_names_split_as_splitlines_does() {
        let w = |l: &[&str]| l.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(name_lines(&w(&["a", "b"])), ["a", "b"]);
        assert_eq!(name_lines(&w(&["a", ""])), ["a"]);
        assert_eq!(name_lines(&w(&["", "b"])), ["", "b"]);
    }
}
