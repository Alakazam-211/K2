//! `_zen_print VERB AS_JSON RAW` (`cli/src/zen-guide-data.sh`): the
//! shared printer of the Zen verbs, as its Python prints.
//!
//! The answer is read as the shell hands it over: held in `$(…)` (NULs
//! and trailing newlines gone), then `json.loads`. An answer that is no
//! JSON object where the printer calls `.get` on it is Python's
//! `AttributeError` traceback. Deeper in the answer the printer reads
//! each field as Python's `%s`, `or` and `", ".join` do; a field of a
//! shape no daemon sends (a number where a list is) prints what `str()`
//! gives instead of Python's traceback.

use super::super::{raised, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::out::pylib::json as pyjson;

/// `NOT_SET_UP`.
pub const NOT_SET_UP: &str = concat!(
    "Zen isn't set up on this computer. Turn it on with the Zen toggle in the ",
    k2_client::product_name!(),
    " app's top bar."
);

/// The answer, read as Python reads `os.environ["ZEN_RAW"]`.
pub fn text_of(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

/// `raw.strip()` is empty.
pub fn blank(raw: &str) -> bool {
    raw.chars().all(super::super::py_space)
}

/// `raw[:200]`.
pub fn head200(raw: &str) -> String {
    raw.chars().take(200).collect()
}

/// `d.get(k)`, `None` on anything but a dict.
pub fn g<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    match v {
        Value::Object(pairs) => super::super::get(pairs, k),
        _ => None,
    }
}

/// `d.get(a).get(b)…` through dicts, `None` where one is missing.
pub fn gp<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for k in path {
        cur = g(cur, k)?;
    }
    Some(cur)
}

/// `str(x)` of a lookup: `None` prints `None`.
pub fn s(v: Option<&Value>) -> String {
    v.map_or_else(|| "None".to_string(), py::str_of)
}

/// `bool(x)` of a lookup.
pub fn t(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

/// `str(x or default)`.
pub fn or(v: Option<&Value>, default: &str) -> String {
    match v {
        Some(x) if py::truthy(x) => py::str_of(x),
        _ => default.to_string(),
    }
}

/// `x or []`, walked: a list's items, a string's characters, a dict's
/// keys; anything else (a number, true) as one item.
pub fn items(v: Option<&Value>) -> Vec<Value> {
    match v {
        Some(x) if py::truthy(x) => match x {
            Value::Array(a) => a.clone(),
            Value::Str(t) => t
                .code_points()
                .into_iter()
                .map(|cp| Value::Str(super::super::code_point(cp)))
                .collect(),
            Value::Object(pairs) => pairs.iter().map(|(k, _)| Value::Str(k.clone())).collect(),
            other => vec![other.clone()],
        },
        _ => Vec::new(),
    }
}

/// `", ".join(x or [])`, each item as `str()`.
pub fn join(v: Option<&Value>, sep: &str) -> String {
    items(v).iter().map(py::str_of).collect::<Vec<_>>().join(sep)
}

/// `x == "word"`.
pub fn is(v: Option<&Value>, word: &str) -> bool {
    matches!(v, Some(Value::Str(t)) if t.as_str() == Some(word))
}

/// `'%d' % x`, lenient: what Python prints for a number, else `str()`.
pub fn d(v: Option<&Value>) -> String {
    match v {
        Some(x) => py::percent_d(x).unwrap_or_else(|_| py::str_of(x)),
        None => "None".to_string(),
    }
}

/// An integer of the answer (`int(x or 0)`), lenient.
pub fn int(v: Option<&Value>) -> i128 {
    match v {
        Some(Value::Int(d)) => d.parse().unwrap_or(0),
        Some(Value::Bool(b)) => i128::from(*b),
        Some(Value::Float(f)) if f.is_finite() => f.trunc() as i128,
        Some(Value::Str(t)) => t.as_str().and_then(|s| s.trim().parse().ok()).unwrap_or(0),
        _ => 0,
    }
}

/// `json.dumps(d, separators=(",", ":"))`.
pub fn compact(v: &Value) -> String {
    json::to_string(
        v,
        &Options {
            compact: true,
            ..Options::default()
        },
    )
    .unwrap_or_default()
}

/// `json.dumps(d, indent=2)`.
pub fn indented(v: &Value) -> String {
    json::to_string(
        v,
        &Options {
            indent: Some(2),
            ..Options::default()
        },
    )
    .unwrap_or_default()
}

/// What a printer gathered: stdout, stderr and the exit.
#[derive(Debug, Default)]
pub struct Out {
    pub out: String,
    pub err: String,
}

impl Out {
    pub fn o(&mut self, line: impl AsRef<str>) {
        self.out.push_str(line.as_ref());
        self.out.push('\n');
    }

    pub fn e(&mut self, line: impl AsRef<str>) {
        self.err.push_str(line.as_ref());
        self.err.push('\n');
    }

    pub fn done(self, exit: Exit) -> Shown {
        Shown {
            stdout: self.out,
            stderr: self.err,
            exit,
        }
    }

    pub fn raise(self, r: &Raise) -> Shown {
        raised(self.out, self.err, r)
    }
}

/// The settings `_zen_print`'s Python reads besides its three arguments.
#[derive(Debug, Clone, Copy, Default)]
pub struct Env<'a> {
    /// `ZEN_STRICT=true`.
    pub strict: bool,
    /// `ZEN_SYNC_RAW`: `garden list`'s sync answer.
    pub sync_raw: Option<&'a str>,
}

/// The answer read, or what the printer says when it can't be.
pub enum Read {
    Doc(Value),
    Stop(Shown),
}

/// The printers' first lines: no answer, or no JSON.
pub fn read(raw: &str, no_answer: &str) -> Read {
    let mut o = Out::default();
    if blank(raw) {
        o.e(no_answer);
        return Read::Stop(o.done(Exit::Error));
    }
    match pyjson::loads(raw) {
        Ok(v) => Read::Doc(v),
        Err(_) => {
            o.e(format!(
                "Error: the daemon answered something that is not JSON: {}",
                head200(raw)
            ));
            Read::Stop(o.done(Exit::Error))
        }
    }
}

const NO_ANSWER: &str = concat!(
    "Error: no response from the ",
    k2_client::product_name!(),
    " daemon on this computer"
);

/// Program lines of `_zen_print`'s Python, where an `AttributeError` is
/// raised on an answer that is not a dict (each tested against the shell).
pub mod line {
    pub const SET_UP: u32 = 16;
    pub const LOCKED: u32 = 57;
    /// The first `.get` of each verb's printer.
    pub const VERBS: &[(&str, u32)] = &[
        ("validate", 139),
        ("reload", 139),
        ("reset", 207),
        ("history", 211),
        ("doctor", 222),
        ("path", 227),
        ("garden-list", 229),
        ("garden-new", 232),
        ("garden-rename", 238),
        ("garden-reorder", 241),
        ("garden-template", 244),
        ("garden-lock", 252),
        ("garden-delete", 256),
        ("theme-list", 259),
        ("theme-switch", 274),
        ("sync-list", 281),
        ("sync-change", 296),
        ("sync-preview", 304),
        ("news", 314),
        ("news-seen", 330),
        ("theme-new", 333),
        ("widget-new", 338),
        ("widget-list", 346),
    ];
}

fn no_get(at: u32, d: &Value) -> Raise {
    Raise::no_attribute(at, d, "get")
}

/// `_zen_print VERB AS_JSON RAW`.
pub fn zen_print(verb: &str, as_json: bool, raw: &[u8], env: Env<'_>) -> Shown {
    let raw = text_of(raw);
    let d = match read(&raw, NO_ANSWER) {
        Read::Doc(d) => d,
        Read::Stop(s) => return s,
    };
    let mut o = Out::default();
    let dict = matches!(d, Value::Object(_));
    let err = g(&d, "error").cloned();
    let err_ref = err.as_ref();
    let is_err = |w: &str| is(err_ref, w);
    if !dict && matches!(verb, "path" | "garden-list") && !is_err("zen_not_set_up") {
        return o.raise(&no_get(line::SET_UP, &d));
    }
    if is_err("zen_not_set_up")
        || (matches!(verb, "path" | "garden-list")
            && matches!(g(&d, "setUp"), Some(Value::Bool(false))))
    {
        if as_json {
            o.o(compact(&d));
        }
        o.e(or(g(&d, "message"), NOT_SET_UP));
        return o.done(Exit::Refused);
    }
    if is_err("garden_access_needed") || is_err("garden_delete_person") || is_err("session_gone")
    {
        if as_json {
            o.o(compact(&d));
        }
        if is_err("garden_delete_person")
            && matches!(g(&d, "card"), Some(Value::Object(_)))
            && t(gp(&d, &["card", "card"]))
        {
            let who = match g(&d, "name") {
                Some(n) if py::truthy(n) => py::str_of(n),
                _ => s(g(&d, "garden")),
            };
            o.e(format!("Asked the person to delete {who}"));
        }
        o.e(or(
            g(&d, "message"),
            concat!(
                "Ask the person: ",
                k2_client::cli_name!(),
                " zen garden access <garden> ask"
            ),
        ));
        return o.done(Exit::Refused);
    }
    if is_err("garden_locked") || (is_err("owner_only") && verb == "garden-lock") {
        if as_json {
            o.o(compact(&d));
        }
        o.e(or(
            g(&d, "message"),
            "This Garden is locked. Ask the person to unlock it in Settings → Gardens.",
        ));
        return o.done(Exit::Refused);
    }
    if let Some(e) = err_ref.filter(|e| !matches!(e, Value::Null)) {
        if as_json {
            o.o(compact(&d));
        }
        if is_err("last_garden") {
            o.e(or(g(&d, "message"), "That's your last Garden."));
        } else if is_err("garden_has_changes") {
            o.e(format!(
                "Error: Garden {} has its own changes ({}). Add --force to start it over; the file is kept in .history/.",
                s(g(&d, "garden")),
                join(g(&d, "keys"), ", ")
            ));
        } else if is_err("zen_local_only") && (verb.starts_with("sync") || verb.starts_with("news"))
        {
            o.e(concat!(
                "Error: only the person on this computer can change Garden sync and news. Ask the person: Settings → Gardens. (Inside a ",
                k2_client::product_name!(),
                " terminal this runs as that session; a person can use Settings or a terminal outside ",
                k2_client::product_name!(),
                ".)"
            ));
        } else if is_err("zen_local_only") {
            o.e(concat!(
                "Error: ",
                k2_client::cli_name!(),
                " zen talks only to the ",
                k2_client::product_name!(),
                " on this computer, and this token is not its owner."
            ));
        } else if matches!(e, Value::Object(_)) {
            let hint = match g(e, "hint") {
                Some(h) if py::truthy(h) => py::str_of(h),
                _ => match g(e, "message") {
                    Some(m) if py::truthy(m) => py::str_of(m),
                    _ => py::str_of(e),
                },
            };
            o.e(format!("Error: {hint}"));
        } else {
            let m = match g(&d, "message") {
                Some(m) if py::truthy(m) => py::str_of(m),
                _ => py::str_of(e),
            };
            o.e(format!("Error: {m}"));
        }
        return o.done(Exit::Error);
    }
    if verb == "validate" {
        if !dict {
            return o.raise(&no_get(line::LOCKED, &d));
        }
        if let Some(Value::Object(_)) = g(&d, "locked") {
            if as_json {
                o.o(indented(&d));
            }
            o.e(or(
                gp(&d, &["locked", "message"]),
                "This Garden is locked. Ask the person to unlock it in Settings → Gardens.",
            ));
            return o.done(Exit::Refused);
        }
    }
    if as_json {
        o.o(indented(&d));
        if verb == "validate" && matches!(g(&d, "ok"), Some(Value::Bool(false))) {
            return o.done(Exit::Error);
        }
        if verb == "validate" && is(g(&d, "status"), "pending") && env.strict {
            return o.done(Exit::Usage);
        }
        if verb == "validate" && env.strict && stale_runtime(&d) != 0 {
            return o.done(Exit::Usage);
        }
        return o.done(Exit::Ok);
    }
    if !dict {
        return match line::VERBS.iter().find(|(v, _)| *v == verb) {
            Some((_, at)) => o.raise(&no_get(*at, &d)),
            None => o.done(Exit::Ok),
        };
    }
    body(verb, &d, env, o)
}

/// `stale_runtime(d)`.
fn stale_runtime(d: &Value) -> i128 {
    let rt = match g(d, "runtime") {
        Some(v @ Value::Object(_)) => Some(v),
        _ => None,
    };
    int(g(d, "staleRuntime")) + rt.map_or(0, |r| int(g(r, "staleErrors")))
}

/// `diag(e, warn)`.
pub fn diag(e: &Value, warn: bool) -> String {
    let mut line = format!(
        "{}:{}:{}: {}{}",
        s(g(e, "file")),
        s(g(e, "line")),
        s(g(e, "col")),
        if warn { "warning: " } else { "" },
        s(g(e, "message"))
    );
    if t(g(e, "suggestion")) {
        line.push_str(&format!("\n  suggestion: {}", s(g(e, "suggestion"))));
    }
    line
}

/// `human(n)`: K2's limit words.
pub fn human(n: i128) -> String {
    if n >= 1_048_576 {
        let t = (n * 10 + 524_288).div_euclid(1_048_576);
        if t % 10 == 0 {
            format!("{} MB", t / 10)
        } else {
            format!("{}.{} MB", t / 10, t % 10)
        }
    } else if n >= 1024 {
        format!("{} KB", (n + 512).div_euclid(1024))
    } else {
        format!("{n} bytes")
    }
}

/// `size_line(s)`.
fn size_line(sz: Option<&Value>) -> Option<String> {
    let sz = sz?;
    let code = match g(sz, "code") {
        Some(c @ (Value::Int(_) | Value::Bool(_))) => int(Some(c)),
        _ => return None,
    };
    let warn_at = match g(sz, "codeWarnAt") {
        Some(v) => int(Some(v)),
        None => code,
    };
    let large = if code > warn_at { " (large)" } else { "" };
    let mut libs = human(int(g(sz, "libs")));
    if t(g(sz, "libsUnknown")) {
        libs.push_str(" + CDN files");
    }
    Some(format!(
        "size: code {} of {}{large}, page {} of {}, libraries {libs} (outside the limit)",
        human(code),
        human(int(g(sz, "codeMax"))),
        human(int(g(sz, "page"))),
        human(int(g(sz, "pageMax")))
    ))
}

/// The SYNC table: `ZEN_SYNC_RAW`'s gardens by id.
fn sync_rows(raw: Option<&str>) -> Vec<(Value, Value)> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let text = if raw.is_empty() { "{}" } else { raw };
    let Ok(v) = pyjson::loads(text) else {
        return Vec::new();
    };
    let mut rows: Vec<(Value, Value)> = Vec::new();
    for row in items(g(&v, "gardens")) {
        let id = g(&row, "id").cloned().unwrap_or(Value::Null);
        match rows.iter_mut().find(|(k, _)| py_eq(k, &id)) {
            Some(slot) => slot.1 = row,
            None => rows.push((id, row)),
        }
    }
    rows
}

fn py_eq(a: &Value, b: &Value) -> bool {
    crate::out::pylib::ops::py_eq(a, b)
}

fn sync_word(row: &Value) -> String {
    let p = gp(row, &["page", "mode"]);
    let th = gp(row, &["theme", "mode"]);
    let same = match (p, th) {
        (None, None) => true,
        (Some(a), Some(b)) => py_eq(a, b),
        _ => false,
    };
    if same {
        return if is(p, "synced") { "synced" } else { "own copy" }.to_string();
    }
    let word = |m: Option<&Value>| if is(m, "synced") { "synced" } else { "own copy" };
    format!("page: {} · theme: {}", word(p), word(th))
}

/// `garden_line(g)`.
pub fn garden_line(gd: &Value, sync: &[(Value, Value)]) -> String {
    let mut line = format!(
        "{} {}  {}  {}  {}",
        if t(g(gd, "theme")) { "*" } else { " " },
        d(g(gd, "index")),
        s(g(gd, "id")),
        s(g(gd, "name")),
        s(g(gd, "template"))
    );
    let id = g(gd, "id").cloned().unwrap_or(Value::Null);
    if let Some((_, row)) = sync.iter().find(|(k, _)| py_eq(k, &id)) {
        line.push_str("  ");
        line.push_str(&sync_word(row));
    }
    if t(g(gd, "locked")) {
        line.push_str("  locked (agents can't change it)");
    }
    line
}

/// `part_line(name, part)`.
fn part_line(name: &str, part: Option<&Value>) -> String {
    let empty = Value::Object(Vec::new());
    let part = match part {
        Some(p) if py::truthy(p) => p,
        _ => &empty,
    };
    if is(g(part, "mode"), "synced") {
        return format!("{name} synced");
    }
    let mut line = format!("{name} own copy of {}", s(g(part, "defaults")));
    if t(g(part, "k2Version")) {
        line.push_str(&format!(
            concat!(" (", k2_client::product_name!(), " {})"),
            s(g(part, "k2Version"))
        ));
    }
    if t(g(part, "newerDefault")) {
        line.push_str(concat!(
            ", ",
            k2_client::product_name!(),
            "'s default has changed since"
        ));
    }
    line
}

/// `x or {}`.
fn obj_or<'a>(v: Option<&'a Value>, empty: &'a Value) -> &'a Value {
    match v {
        Some(x) if py::truthy(x) => x,
        _ => empty,
    }
}

/// `"%s/%s" % (p.get("gardenName") or p.get("garden"), p.get("placement"))`.
fn placed(p: &Value) -> String {
    let name = match g(p, "gardenName") {
        Some(n) if py::truthy(n) => py::str_of(n),
        _ => s(g(p, "garden")),
    };
    format!("{name}/{}", s(g(p, "placement")))
}

/// The guide topic an error points at (FC35).
fn topic(e: &Value) -> Option<&'static str> {
    let m = or(g(e, "message"), "");
    let f = or(g(e, "file"), "");
    if m.contains(concat!(k2_client::cli_name!(), " zen guide")) {
        return None;
    }
    let any = |ws: &[&str]| ws.iter().any(|w| m.contains(w));
    Some(
        if any(&["slot = \"menu\"", "a menu", "menu '", "menu = ", "menus", "menu.label"]) {
            "menus"
        } else if any(&["slot", "band", "edge", "align", "column"]) {
            "bands"
        } else if f.starts_with("gardens/") && any(&["widget", "kind", "props"]) {
            "widgets"
        } else if f.starts_with("gardens/") && any(&["layout", "template", "schema"]) {
            "files"
        } else {
            "themes"
        },
    )
}

fn body(verb: &str, dv: &Value, env: Env<'_>, mut o: Out) -> Shown {
    let empty = Value::Object(Vec::new());
    match verb {
        "validate" | "reload" => {
            let errs = items(g(dv, "errors"));
            for e in &errs {
                o.o(diag(e, false));
            }
            for w in items(g(dv, "warnings")) {
                o.o(diag(&w, true));
            }
            if let Some(l) = size_line(g(dv, "size")) {
                o.o(l);
            }
            if let Some(ty @ Value::Object(_)) = g(dv, "types") {
                let how = if t(g(ty, "checker")) {
                    format!("checked with {}", s(g(ty, "checker")))
                } else {
                    "not checked".to_string()
                };
                let note = if t(g(ty, "note")) {
                    format!("; {}", s(g(ty, "note")))
                } else {
                    String::new()
                };
                o.o(format!("types: {how}{note}"));
            }
            if verb == "reload" {
                o.o(format!(
                    "reloaded: {}",
                    if t(g(dv, "changed")) {
                        "changed"
                    } else {
                        "no change"
                    }
                ));
            }
            let staged = items(g(dv, "files"))
                .iter()
                .any(|f| py::str_of(f).starts_with("staged"));
            if !errs.is_empty() && staged {
                o.e(format!(
                    "{} error(s) in the staged copy; nothing was written.",
                    errs.len()
                ));
                return o.done(Exit::Error);
            }
            if !errs.is_empty() {
                o.e(format!(
                    "{} error(s). Zen is showing the last good version.",
                    errs.len()
                ));
                let mut topics: Vec<&str> = Vec::new();
                for e in &errs {
                    if let Some(tp) = topic(e) {
                        if !topics.contains(&tp) {
                            topics.push(tp);
                        }
                    }
                }
                if !topics.is_empty() {
                    let list: Vec<String> = topics
                        .iter()
                        .map(|tp| format!(concat!(k2_client::cli_name!(), " zen guide {}"), tp))
                        .collect();
                    o.e(format!("Help: {}", list.join(", ")));
                }
                return o.done(Exit::Error);
            }
            if verb == "validate" {
                let files: Vec<String> = items(g(dv, "files")).iter().map(py::str_of).collect();
                let rt = match g(dv, "runtime") {
                    Some(v @ Value::Object(_)) => Some(v),
                    _ => None,
                };
                for w in items(g(dv, "widgets")) {
                    let ps: Vec<String> = items(g(&w, "placements")).iter().map(placed).collect();
                    let where_ = if ps.is_empty() {
                        "not placed".to_string()
                    } else {
                        ps.join(", ")
                    };
                    let extra = if t(g(&w, "stale")) {
                        format!(
                            "; {} old runtime error(s), not counted",
                            s(g(&w, "stale"))
                        )
                    } else {
                        String::new()
                    };
                    o.o(format!(
                        "widget {}: {} ({where_}){extra}",
                        s(g(&w, "widget")),
                        s(g(&w, "status"))
                    ));
                }
                if let Some(r) = rt {
                    if t(g(r, "placements")) {
                        let ps: Vec<String> =
                            items(g(r, "placements")).iter().map(placed).collect();
                        o.o(format!("placed: {}", ps.join(", ")));
                    }
                }
                if is(g(dv, "status"), "pending") {
                    let note = match rt {
                        Some(r) => s(g(r, "note")),
                        None => "it hasn't run yet".to_string(),
                    };
                    o.o(format!("pending: {} parses; {note}", files.join(", ")));
                    if env.strict {
                        return o.done(Exit::Usage);
                    }
                    o.o("(exit 0: pending is not an error; --strict exits 2 until it has run)");
                    return o.done(Exit::Ok);
                }
                o.o(format!(
                    "ok: {}",
                    if files.is_empty() {
                        "no files".to_string()
                    } else {
                        files.join(", ")
                    }
                ));
                if let Some(r) = rt {
                    if t(g(r, "note")) {
                        o.o(format!("runtime: {}", s(g(r, "note"))));
                    }
                }
                if stale_runtime(dv) != 0 {
                    if env.strict {
                        o.e(concat!(
                            "--strict: old runtime errors are logged; exit 2 until they are cleared (",
                            k2_client::cli_name!(),
                            " zen widget errors <name> --clear)"
                        ));
                        return o.done(Exit::Usage);
                    }
                    o.o("(exit 0: old runtime errors are warnings; --strict exits 2 while they are logged)");
                }
            }
        }
        "reset" => {
            o.o(format!(
                "reset {} to {}",
                s(g(dv, "file")),
                s(g(dv, "restored"))
            ));
            if t(g(dv, "snapshot")) {
                let snap = s(g(dv, "snapshot"));
                o.o(format!(
                    concat!(
                        "previous file kept as snapshot {} (",
                        k2_client::cli_name!(),
                        " zen reset --to {} to undo)"
                    ),
                    snap, snap
                ));
            }
        }
        "history" => {
            for f in items(g(dv, "files")) {
                let snaps = items(g(&f, "snapshots"));
                let note = if t(g(&f, "deleted")) {
                    ", Garden deleted"
                } else if !g(&f, "exists").map_or(true, py::truthy) {
                    ", file deleted"
                } else {
                    ""
                };
                o.o(format!(
                    "{} ({} snapshot{}{note})",
                    s(g(&f, "file")),
                    snaps.len(),
                    if snaps.len() == 1 { "" } else { "s" }
                ));
                for sn in snaps {
                    o.o(format!(
                        "  {}  {}{}",
                        s(g(&sn, "name")),
                        s(g(&sn, "at")),
                        if t(g(&sn, "clean")) {
                            ""
                        } else {
                            "  (has errors)"
                        }
                    ));
                }
            }
        }
        "doctor" => {
            for c in items(g(dv, "checks")) {
                o.o(format!(
                    "{} {}: {}",
                    if t(g(&c, "ok")) { "ok  " } else { "FAIL" },
                    s(g(&c, "name")),
                    s(g(&c, "detail"))
                ));
            }
            if !t(g(dv, "ok")) {
                return o.done(Exit::Error);
            }
        }
        "path" => o.o(s(g(dv, "path"))),
        "garden-list" | "garden-reorder" => {
            let sync = sync_rows(env.sync_raw);
            for gd in items(g(dv, "gardens")) {
                o.o(garden_line(&gd, &sync));
            }
        }
        "garden-new" => {
            let gd = obj_or(g(dv, "garden"), &empty);
            o.o(s(g(gd, "id")));
            o.o(s(g(dv, "path")));
            if t(g(dv, "made")) && t(g(dv, "message")) {
                o.e(s(g(dv, "message")));
            }
        }
        "garden-rename" => {
            let gd = obj_or(g(dv, "garden"), &empty);
            o.o(format!(
                "Garden {} is now “{}”{}",
                s(g(gd, "id")),
                s(g(gd, "name")),
                if t(g(dv, "changed")) {
                    ""
                } else {
                    " (unchanged)"
                }
            ));
        }
        "garden-template" => {
            let gd = obj_or(g(dv, "garden"), &empty);
            if t(g(dv, "changed")) {
                o.o(format!(
                    "Garden {} (“{}”) is now {}",
                    s(g(gd, "id")),
                    s(g(gd, "name")),
                    s(g(dv, "template"))
                ));
                if t(g(dv, "snapshot")) {
                    o.o(format!(
                        concat!(
                            "its previous file is kept as snapshot {} (",
                            k2_client::cli_name!(),
                            " zen reset --garden {} --to {} to undo)"
                        ),
                        s(g(dv, "snapshot")),
                        s(g(gd, "id")),
                        s(g(dv, "snapshot"))
                    ));
                }
            } else {
                o.o(format!(
                    "Garden {} (“{}”) is already {} (unchanged)",
                    s(g(gd, "id")),
                    s(g(gd, "name")),
                    s(g(dv, "template"))
                ));
            }
        }
        "garden-lock" => {
            let row = obj_or(g(dv, "garden"), &empty);
            let state = if t(g(dv, "locked")) {
                "locked: agents can't change it"
            } else {
                "unlocked: agents can change it again"
            };
            o.o(format!(
                "{} ({}) is {state}{}",
                s(g(row, "name")),
                s(g(row, "id")),
                if t(g(dv, "changed")) {
                    ""
                } else {
                    " (unchanged)"
                }
            ));
        }
        "garden-delete" => {
            let snap = g(dv, "snapshot");
            let kept = if t(snap) {
                format!("; its page is kept in .history as {}", s(snap))
            } else {
                String::new()
            };
            o.o(format!(
                "deleted Garden {} (“{}”){kept}",
                s(g(dv, "deleted")),
                s(g(dv, "name"))
            ));
        }
        "theme-list" => {
            for th in items(g(dv, "themes")) {
                let mut tags = Vec::new();
                if t(g(&th, "builtin")) {
                    tags.push("built in");
                }
                if t(g(&th, "user")) {
                    tags.push(if t(g(&th, "builtin")) {
                        "your override"
                    } else {
                        "yours"
                    });
                }
                let label = match g(&th, "label") {
                    Some(l) if py::truthy(l) => py::str_of(l),
                    _ => s(g(&th, "name")),
                };
                let mut line = format!(
                    "{} {}  {}  ({})",
                    if t(g(&th, "active")) { "*" } else { " " },
                    s(g(&th, "name")),
                    label,
                    tags.join(", ")
                );
                if t(g(&th, "summary")) {
                    line.push_str("  ");
                    line.push_str(&s(g(&th, "summary")));
                }
                o.o(line);
            }
            if t(g(dv, "garden")) {
                o.o(format!(
                    "Garden {}: {}",
                    s(g(dv, "garden")),
                    if t(g(dv, "gardenTheme")) {
                        "its own pick"
                    } else {
                        "follows this computer"
                    }
                ));
            }
            if t(g(dv, "missing")) {
                o.e(format!(
                    "theme '{}' doesn't exist any more; showing {}",
                    s(g(dv, "missing")),
                    s(g(dv, "active"))
                ));
            }
        }
        "theme-switch" => {
            if t(g(dv, "garden")) && is(g(dv, "scope"), "garden") {
                o.o(format!(
                    "theme {} for Garden {}",
                    s(g(dv, "theme")),
                    s(g(dv, "garden"))
                ));
            } else if t(g(dv, "garden")) {
                o.o(format!(
                    "Garden {} follows this computer: theme {}",
                    s(g(dv, "garden")),
                    s(g(dv, "theme"))
                ));
            } else {
                o.o(format!("theme {} for this computer", s(g(dv, "theme"))));
            }
        }
        "sync-list" => {
            for row in items(g(dv, "gardens")) {
                let own = g(&row, "ownChanges");
                let mut extra: Vec<String> = Vec::new();
                if t(own) {
                    extra.push(format!("own changes: {}", join(own, ", ")));
                } else if own.is_none() || matches!(own, Some(Value::Null)) {
                    extra.push("its file doesn't parse".to_string());
                }
                if t(g(&row, "undo")) {
                    extra.push(format!(
                        "undo: {}",
                        join(gp(&row, &["undo", "parts"]), ", ")
                    ));
                }
                let kp = obj_or(g(&row, "keepPrevious"), &empty);
                if t(g(kp, "page")) || t(g(kp, "theme")) {
                    extra.push("keep-previous offered".to_string());
                }
                let tail = if extra.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", extra.join("; "))
                };
                o.o(format!(
                    "{}  {}  {}  {}; {}{tail}",
                    d(g(&row, "index")),
                    s(g(&row, "id")),
                    s(g(&row, "name")),
                    part_line("page", g(&row, "page")),
                    part_line("theme", g(&row, "theme"))
                ));
            }
            let prev = if t(g(dv, "previousDefaults")) {
                format!("; previous {}", s(g(dv, "previousDefaults")))
            } else {
                String::new()
            };
            o.o(format!(
                concat!(
                    k2_client::product_name!(),
                    " defaults: {} (",
                    k2_client::product_name!(),
                    " {}){}"
                ),
                s(g(dv, "liveDefaults")),
                s(g(dv, "k2Version")),
                prev
            ));
        }
        "sync-change" => {
            let row = obj_or(g(dv, "garden"), &empty);
            let parts = format!(
                "{}; {}",
                part_line("page", g(row, "page")),
                part_line("theme", g(row, "theme"))
            );
            if t(g(dv, "changed")) {
                o.o(format!(
                    "{} ({}): {parts}",
                    s(g(row, "name")),
                    s(g(row, "id"))
                ));
                if t(g(row, "undo")) {
                    o.o(format!(
                        concat!("undo with: ", k2_client::cli_name!(), " zen sync {} undo"),
                        s(g(row, "id"))
                    ));
                }
            } else {
                o.o(format!(
                    "{} ({}): unchanged ({parts})",
                    s(g(row, "name")),
                    s(g(row, "id"))
                ));
            }
        }
        "sync-preview" => {
            let gd = obj_or(g(dv, "garden"), &empty);
            let page = obj_or(g(dv, "page"), &empty);
            let theme = obj_or(g(dv, "theme"), &empty);
            let sy = obj_or(g(dv, "sync"), &empty);
            let pv = g(sy, "preview");
            let parts = if is(pv, "page") {
                "page".to_string()
            } else if is(pv, "theme") {
                "theme".to_string()
            } else if is(pv, "both") {
                "page and theme".to_string()
            } else {
                s(pv)
            };
            o.o(format!(
                concat!(
                    "preview of {} ({}) with ",
                    k2_client::product_name!(),
                    "'s default for its {} (nothing written)"
                ),
                s(g(gd, "name")),
                s(g(gd, "id")),
                parts
            ));
            let layout = obj_or(g(page, "layout"), &empty);
            o.o(format!(
                "  template {}, layout {}, {} widget(s)",
                s(g(page, "template")),
                s(g(layout, "split")),
                items(g(page, "widgets")).len()
            ));
            let colors = obj_or(g(obj_or(g(theme, "tokens"), &empty), "colors"), &empty);
            let accent = |mode: &str| -> String {
                match g(colors, mode) {
                    Some(m) => s(g(m, "accent")),
                    None => "None".to_string(),
                }
            };
            o.o(format!(
                "  theme {}, accent {} / {}",
                s(g(theme, "name")),
                accent("light"),
                accent("dark")
            ));
            let mode = |k: &str| s(g(obj_or(g(sy, k), &empty), "mode"));
            o.o(format!(
                "  page now: {}; theme now: {}",
                mode("page"),
                mode("theme")
            ));
        }
        "news" => {
            let list = items(g(dv, "items"));
            if list.is_empty() {
                o.o("no unseen Garden news");
            }
            for it in list {
                if is(g(&it, "kind"), "notice") {
                    o.o(format!(
                        "{}  {}. {}",
                        s(g(&it, "id")),
                        s(g(&it, "title")),
                        s(g(&it, "body"))
                    ));
                    for l in items(g(&it, "detail")) {
                        o.o(format!("    {}", py::str_of(&l)));
                    }
                } else if is(g(&it, "kind"), "catalog") {
                    o.o(format!(
                        "{}  New in the Garden catalog: {}. {}",
                        s(g(&it, "id")),
                        s(g(&it, "label")),
                        s(g(&it, "description"))
                    ));
                } else if is(g(&it, "kind"), "update") {
                    o.o(format!(
                        "{}  {} has a fresh look ({}){}",
                        s(g(&it, "id")),
                        s(g(&it, "gardenName")),
                        s(g(&it, "part")),
                        if t(g(&it, "keepPrevious")) {
                            "; keep-previous offered"
                        } else {
                            ""
                        }
                    ));
                }
            }
            let n = g(dv, "copiesWithNewerDefault");
            if t(n) {
                let one = matches!(n, Some(Value::Int(v)) if v == "1")
                    || matches!(n, Some(Value::Bool(true)))
                    || matches!(n, Some(Value::Float(f)) if *f == 1.0);
                o.o(format!(
                    concat!(
                        "{} own cop{} didn't change; ",
                        k2_client::product_name!(),
                        "'s default is newer (",
                        k2_client::cli_name!(),
                        " zen sync)"
                    ),
                    d(n),
                    if one { "y" } else { "ies" }
                ));
            }
        }
        "news-seen" => {
            let seen = items(g(dv, "seen"));
            let names: Vec<String> = seen.iter().map(py::str_of).collect();
            o.o(format!(
                "marked {} item(s) seen{}",
                seen.len(),
                if names.is_empty() {
                    String::new()
                } else {
                    format!(": {}", names.join(", "))
                }
            ));
        }
        "theme-new" => {
            o.o(format!(
                "made {} from {}",
                s(g(dv, "path")),
                s(g(dv, "from"))
            ));
            if t(g(dv, "copiedImage")) {
                o.o(format!(
                    "copied its background {}",
                    s(g(dv, "copiedImage"))
                ));
            }
            o.o(format!(
                concat!(
                    "edit it, check with ",
                    k2_client::cli_name!(),
                    " zen validate, use it with ",
                    k2_client::cli_name!(),
                    " zen theme set {}"
                ),
                s(g(dv, "name"))
            ));
        }
        "widget-new" => {
            let name = s(g(dv, "name"));
            o.o(s(g(dv, "path")));
            let files = items(g(dv, "files"));
            if !files.is_empty() {
                let names: Vec<String> = files.iter().map(py::str_of).collect();
                o.o(format!("files: {}", names.join(", ")));
            }
            o.o(format!(
                "next: edit it, place it in a Garden with kind = \"custom\" and widget = \"{name}\","
            ));
            o.o(format!(
                concat!(
                    "check with ",
                    k2_client::cli_name!(),
                    " zen validate --widget {}; it talks to your agents as soon as it's placed"
                ),
                name
            ));
        }
        "widget-list" => {
            let ws = items(g(dv, "widgets"));
            if ws.is_empty() {
                o.o(concat!(
                    "no custom widgets yet (",
                    k2_client::cli_name!(),
                    " zen widget new <name> --from k2:diary)"
                ));
            }
            for w in ws {
                let caps = {
                    let c = join(g(&w, "caps"), ", ");
                    if c.is_empty() {
                        "no caps".to_string()
                    } else {
                        c
                    }
                };
                let title = match g(&w, "title") {
                    Some(x) if py::truthy(x) => py::str_of(x),
                    _ => s(g(&w, "name")),
                };
                o.o(format!(
                    "{}  {}  ({})  caps: {}",
                    s(g(&w, "name")),
                    s(g(&w, "state")),
                    title,
                    caps
                ));
                for e in items(g(&w, "errors")).iter().take(3) {
                    o.o(format!("  {}", diag(e, false)));
                }
                let ps = items(g(&w, "placements"));
                if ps.is_empty() {
                    o.o("  not placed in any Garden");
                }
                for p in ps {
                    let state = if t(g(&p, "paused")) {
                        "posting paused (too many posts; the person clicks Resume on it)"
                    } else {
                        "running"
                    };
                    o.o(format!(
                        "  {}/{}  {state}",
                        s(g(&p, "garden")),
                        s(g(&p, "placement"))
                    ));
                }
            }
        }
        _ => {}
    }
    o.done(Exit::Ok)
}

/// A JSON string value.
pub fn jstr(v: &str) -> Value {
    Value::Str(Text::from(v))
}

#[cfg(test)]
mod tests;
