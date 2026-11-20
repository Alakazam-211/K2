//! `publish`'s readers (`cli/src/connect.sh`): what each of the shell's
//! Python blocks prints for the answer it reads, and the request bodies
//! its Python writes.
//!
//! - [`handle`]: `_publish_handle_resp` (`run`, `start`, `stop`, `rm`,
//!   `frame`, `ps` and `logs`).
//! - [`site`]: `_publish_site_render` (`visibility`, `open`, `sessions`).
//! - [`claimed`]: the `subdomain claim` and `unclaim` reader.
//! - [`resp_code`], [`asks_pin`], [`needs_yes_line`]: the small readers
//!   `visibility` asks between its requests.
//! - [`probe_timeout`]: `_publish_probe_timeout_check`.
//!
//! The answer reaches each reader as the shell's `$(…)` holds it (NULs and
//! trailing newlines gone), read as UTF-8 with U+FFFD for a bad byte.
//! Where the shell's Python raises, the binary prints a Python 3.12
//! traceback (no source lines) and exits 1.

use crate::exit::Exit;
use crate::families::{get, py_iter, raised, Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::out::pylib::json::loads;
use crate::out::pylib::ops::py_eq;
use k2_client::brand::{CLI_NAME as CLI, PRODUCT_NAME as PRODUCT};

/// What `_publish_handle_resp`'s `P_MODE` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Service,
    List,
    Logs,
}

type Pairs = [(Text, Value)];
type R<T> = Result<T, Raise>;

/// A string value.
pub fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// An object with these keys, in order.
pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `json.dumps(v)`: Python's default layout.
pub fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).expect("a value from JSON always prints")
}

/// `json.dumps(v, separators=(",", ":"))`.
pub fn compact(v: &Value) -> String {
    json::to_string(
        v,
        &Options {
            compact: true,
            ..Options::default()
        },
    )
    .expect("a value from JSON always prints")
}

/// `_publish_err <code> <hint>`: the JSON line under `--json`, else
/// `Error: <hint>`; without its newline.
pub fn err_line(code: &str, hint: &str, as_json: bool) -> String {
    if as_json {
        dumps(&obj(vec![("error", obj(vec![("code", s(code)), ("hint", s(hint))]))]))
    } else {
        format!("Error: {hint}")
    }
}

fn is_str(v: Option<&Value>, text: &str) -> bool {
    matches!(v, Some(Value::Str(t)) if t.as_str() == Some(text))
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

/// `x or y`, both maybe missing (`None`).
fn or<'a>(x: Option<&'a Value>, y: Option<&'a Value>) -> Option<&'a Value> {
    if truthy(x) {
        x
    } else {
        y
    }
}

/// `str(v or "<default>")`.
fn or_s(v: Option<&Value>, default: &str) -> String {
    match v {
        Some(v) if py::truthy(v) => py::str_of(v),
        _ => default.to_string(),
    }
}

/// `str(v)`, `None` for a missing key.
fn str_opt(v: Option<&Value>) -> String {
    v.map_or_else(|| "None".to_string(), py::str_of)
}

fn as_obj(v: Option<&Value>) -> Option<&Pairs> {
    match v {
        Some(Value::Object(p)) => Some(p),
        _ => None,
    }
}

fn chars(t: &str) -> usize {
    t.chars().count()
}

/// `s.ljust(width)` by code points.
fn pad(t: &str, width: usize) -> String {
    let n = chars(t);
    if n < width {
        format!("{t}{}", " ".repeat(width - n))
    } else {
        t.to_string()
    }
}

/// Python's `str.strip()`.
fn strip(t: &str) -> &str {
    t.trim_matches(crate::families::py_space)
}

/// `(v or "").strip()` where `v` must be a string when it is truthy.
fn strip_or_empty(v: Option<&Value>, line: u32) -> R<String> {
    match v {
        Some(Value::Str(t)) => Ok(strip(&py::str_of(&Value::Str(t.clone()))).to_string()),
        Some(other) if py::truthy(other) => Err(Raise::no_attribute(line, other, "strip")),
        _ => Ok(String::new()),
    }
}

/// `int(v)`: the value, or Python's exception.
fn py_int(v: &Value) -> Result<i128, String> {
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Int(d) => Ok(d.parse::<i128>().unwrap_or(if d.starts_with('-') {
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
            let raw = py::str_of(&Value::Str(t.clone()));
            let st = strip(&raw);
            let (neg, digits) = match st.as_bytes().first() {
                Some(b'-') => (true, &st[1..]),
                Some(b'+') => (false, &st[1..]),
                _ => (false, st),
            };
            let ok = !digits.is_empty()
                && !digits.starts_with('_')
                && !digits.ends_with('_')
                && !digits.contains("__")
                && digits.chars().all(|c| c.is_ascii_digit() || c == '_');
            if !ok {
                return Err(format!(
                    "ValueError: invalid literal for int() with base 10: {}",
                    py::repr(&Value::Str(t.clone()))
                ));
            }
            let plain: String = digits.chars().filter(|c| *c != '_').collect();
            let n = plain.parse::<i128>().unwrap_or(i128::MAX);
            Ok(if neg { -n } else { n })
        }
        other => Err(format!(
            "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py::type_name(other)
        )),
    }
}

/// Python's `a // b` for a positive `b`.
fn floor_div(a: i128, b: i128) -> i128 {
    a.div_euclid(b)
}

/// What a reader printed so far.
#[derive(Default)]
struct Out {
    stdout: String,
    stderr: String,
}

impl Out {
    fn line(&mut self, t: &str) {
        self.stdout.push_str(t);
        self.stdout.push('\n');
    }

    fn err(&mut self, t: &str) {
        self.stderr.push_str(t);
    }

    fn end(self, r: R<Exit>) -> Shown {
        match r {
            Ok(exit) => Shown {
                stdout: self.stdout,
                stderr: self.stderr,
                exit,
            },
            Err(e) => raised(self.stdout, self.stderr, &e),
        }
    }
}

/// `_publish_handle_resp`'s `fail(code, hint)`.
fn fail(o: &mut Out, verb: &str, as_json: bool, code: &Value, hint: &str) -> Exit {
    let is = |set: &[&str]| matches!(code, Value::Str(t) if t.as_str().is_some_and(|c| set.contains(&c)));
    let mut exit = Exit::Error;
    if verb == "frame" {
        if is(&["usage", "bad_frame_origin", "frame_too_many", "bad_frame_policy"]) {
            exit = Exit::Usage;
        } else if is(&["owner_only", "role_required"]) {
            exit = Exit::Refused;
        }
    }
    if is(&["disk_low", "skin_retired", "idle_off_person_only"]) {
        exit = Exit::Refused;
    }
    if is(&["dev_needs_cmd", "dev_needs_tunnel", "usage"]) {
        exit = Exit::Usage;
    }
    let text = hint.trim_end_matches('\n');
    if as_json {
        let v = obj(vec![(
            "error",
            obj(vec![("code", s(&py::str_of(code))), ("hint", s(text))]),
        )]);
        o.err(&format!("{}\n", compact(&v)));
    } else if text.starts_with("Error:") {
        o.err(&format!("{text}\n"));
    } else {
        o.err(&format!("Error: {text}\n"));
    }
    exit
}

/// `env_legacy(s)`.
fn env_legacy(s: &Pairs) -> bool {
    match get(s, "envMode") {
        Some(Value::Null) | None => truthy(get(s, "envDeprecation")),
        Some(m) => is_str(Some(m), "legacy"),
    }
}

/// `env_ends(s)`.
fn env_ends(s: &Pairs) -> Option<String> {
    get(s, "envRemovedIn")
        .filter(|v| py::truthy(v))
        .map(py::str_of)
}

/// `env_now(s)`.
fn env_now(s: &Pairs) -> bool {
    truthy(get(s, "cutoverNow"))
}

/// `env_when(s)`.
fn env_when(s: &Pairs) -> Option<String> {
    let e = env_ends(s)?;
    Some(if env_now(s) {
        format!("with this release ({PRODUCT} {e})")
    } else {
        format!("in {PRODUCT} {e}")
    })
}

/// `env_warning(n, s)`.
fn env_warning(n: &str, s: &Pairs) -> String {
    match env_when(s) {
        None => format!(
            "warning: {n} still inherits the daemon environment (it keeps it as a --cmd service). Start it clean with: {CLI} publish env {n} clean (grant secrets with {CLI} publish secret add)."
        ),
        Some(w) => format!(
            "warning: {n} still inherits the daemon environment; this ends {w}. Switch with: {CLI} publish env {n} clean (grant secrets with {CLI} publish secret add)."
        ),
    }
}

/// `url_or_target(s)`.
fn url_or_target(s: &Pairs) -> R<String> {
    let u = strip_or_empty(get(s, "url"), 254)?;
    if !u.is_empty() {
        return Ok(u);
    }
    let t = strip_or_empty(get(s, "target"), 257)?;
    if !t.is_empty() {
        return Ok(t);
    }
    match get(s, "port") {
        Some(p) if *p != Value::Null && py::str_of(p) != "" => {
            Ok(format!("localhost:{}", py::str_of(p)))
        }
        _ => Ok("-".to_string()),
    }
}

/// `fmt_left(ms)`.
fn fmt_left(ms: &Value) -> String {
    let Ok(n) = py_int(ms) else {
        return "?".to_string();
    };
    let secs = floor_div(n, 1000).max(0);
    let (m, sec) = (secs / 60, secs % 60);
    if m != 0 && sec != 0 {
        format!("{m}m{sec:02}s")
    } else if m != 0 {
        format!("{m}m")
    } else {
        format!("{sec}s")
    }
}

/// `status_text(s)`.
fn status_text(s: &Pairs) -> String {
    let st = or_s(or(get(s, "status"), get(s, "desired")), "-");
    if st == "starting" && !matches!(get(s, "startingLeftMs"), None | Some(Value::Null)) {
        return format!(
            "starting (waiting for port {}, {} left)",
            str_opt(get(s, "port")),
            fmt_left(get(s, "startingLeftMs").expect("checked above"))
        );
    }
    st
}

/// `vis_label(s)`.
fn vis_label(s: &Pairs) -> String {
    let Some(v) = as_obj(get(s, "visibility")) else {
        return if is_str(get(s, "cmd"), "(app)") {
            "own logins".to_string()
        } else {
            "-".to_string()
        };
    };
    let mode = or_s(get(v, "mode"), "?");
    if matches!(get(v, "enforced"), Some(Value::Bool(false))) {
        return format!("{mode} (not enforced: E2E off)");
    }
    if truthy(get(v, "dev")) {
        format!("{mode}, dev")
    } else {
        mode
    }
}

/// `frame_label(s)`.
fn frame_label(s: &Pairs) -> String {
    if matches!(get(s, "framePolicy"), None | Some(Value::Null)) {
        return "own server".to_string();
    }
    let raw = or_s(get(s, "frameAncestors"), "");
    match raw.as_str() {
        "" => "default".to_string(),
        "none" => "none".to_string(),
        _ => format!("default + {}", raw.replace(' ', ",")),
    }
}

/// `"<text>" + v`: a string, or the `TypeError` at `line`.
fn concat(text: &str, v: &Value, line: u32) -> R<String> {
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

/// `_publish_handle_resp` on `raw` (`RESP`), with `AS_JSON`, `P_MODE`,
/// `P_VERB` and `P_LONG`.
pub fn handle(raw: &str, as_json: bool, mode: Mode, verb: &str, long: bool) -> Shown {
    let mut o = Out::default();
    let r = handle_into(&mut o, raw, as_json, mode, verb, long);
    o.end(r)
}

fn handle_into(o: &mut Out, raw: &str, as_json: bool, mode: Mode, verb: &str, long: bool) -> R<Exit> {
    let stripped = strip(raw);
    if stripped.is_empty() {
        return Ok(fail(o, verb, as_json, &s("unreachable"), "no response from the daemon"));
    }
    let d = loads(raw).ok();
    if mode == Mode::Logs && !matches!(d, Some(Value::Object(_))) {
        o.stdout.push_str(raw);
        if !raw.ends_with('\n') {
            o.stdout.push('\n');
        }
        return Ok(Exit::Ok);
    }
    let Some(d) = d else {
        let hint: String = stripped.chars().take(2000).collect();
        return Ok(fail(o, verb, as_json, &s("error"), &hint));
    };
    let d = match d {
        Value::Object(d) => d,
        other if as_json => {
            o.line(&compact(&other));
            return Ok(Exit::Ok);
        }
        _ => {
            return Ok(fail(
                o,
                verb,
                as_json,
                &s("bad_response"),
                "unexpected response from /cli/publish",
            ))
        }
    };
    let d = &d[..];

    if let Some(err) = get(d, "error") {
        let skip = matches!(err, Value::Null | Value::Bool(false))
            || matches!(err, Value::Str(t) if t.code_points().is_empty());
        if !skip {
            let (code, hint) = if let Value::Object(e) = err {
                let code = or(get(e, "code"), None).cloned().unwrap_or_else(|| s("error"));
                let hint = or(or(or(get(e, "hint"), get(e, "message")), get(e, "detail")), None)
                    .map_or_else(|| py::str_of(&code), py::str_of);
                (code, hint)
            } else if truthy(get(d, "code")) {
                let code = s(&py::str_of(get(d, "code").expect("truthy")));
                let mut hint = py::str_of(err);
                if let Some(h) = get(d, "hint").filter(|h| py::truthy(h)) {
                    hint = format!("{hint}\n  {}", py::str_of(h));
                }
                (code, hint)
            } else {
                let e = py::str_of(err);
                let bare = e.replace('_', "");
                let word = !bare.is_empty()
                    && bare.chars().all(char::is_alphanumeric)
                    && chars(&e) < 48;
                let code = s(if word { &e } else { "error" });
                let mut hint = e;
                if truthy(get(d, "hint")) {
                    hint = py::str_of(get(d, "hint").expect("truthy"));
                } else if truthy(get(d, "detail")) {
                    hint = py::str_of(get(d, "detail").expect("truthy"));
                } else if truthy(get(d, "message")) && (hint == "error" || hint == "pro_required") {
                    hint = py::str_of(get(d, "message").expect("truthy"));
                }
                (code, hint)
            };
            return Ok(fail(o, verb, as_json, &code, &hint));
        }
    }

    if matches!(get(d, "success"), Some(Value::Bool(false))) {
        let hint = or_s(or(get(d, "hint"), get(d, "message")), "request failed");
        return Ok(fail(o, verb, as_json, &s("error"), &hint));
    }

    if mode == Mode::Logs {
        let mut text = None;
        for k in ["log", "text", "body"] {
            if let Some(v) = get(d, k).filter(|v| **v != Value::Null) {
                text = Some(py::str_of(v));
                break;
            }
        }
        if text.is_none() {
            if let Some(Value::Array(lines)) = get(d, "lines") {
                text = Some(lines.iter().map(py::str_of).collect::<Vec<_>>().join("\n"));
            }
        }
        let text = text.unwrap_or_else(|| raw.to_string());
        o.stdout.push_str(&text);
        if !text.ends_with('\n') {
            o.stdout.push('\n');
        }
        return Ok(Exit::Ok);
    }

    if mode == Mode::Service && verb == "run" && is_str(get(d, "status"), "held") {
        return held(o, d, as_json);
    }
    if mode == Mode::Service && verb == "run" {
        for (k, label) in [("diskWarning", "disk"), ("diskOverride", "disk override")] {
            let Some(x) = as_obj(get(d, k)) else { continue };
            if truthy(get(x, "hint")) {
                o.err(&format!(
                    "{label} warning: {}\n",
                    py::str_of(get(x, "hint").expect("truthy"))
                ));
            } else if truthy(get(x, "applied")) {
                o.err(&format!(
                    "disk override ({}): {}\n",
                    get(x, "via").map_or_else(String::new, py::str_of),
                    get(x, "reason").map_or_else(String::new, py::str_of)
                ));
            }
        }
    }
    if mode == Mode::Service && (verb == "run" || verb == "start") {
        let sv = as_obj(get(d, "service")).unwrap_or(d);
        if let Some(dv) = as_obj(get(sv, "dev")) {
            if truthy(get(d, "devWarning")) {
                o.err(&format!(
                    "warning: {}\n",
                    py::str_of(get(d, "devWarning").expect("truthy"))
                ));
            }
            o.err(&format!(
                "dev: live folder, not walled \u{b7} caps: {}\n",
                or_s(get(dv, "caps"), "?")
            ));
            if truthy(get(dv, "warning")) {
                o.err(&format!(
                    "warning: {}\n",
                    py::str_of(get(dv, "warning").expect("truthy"))
                ));
            }
        }
        if env_legacy(sv) {
            let n = or_s(get(sv, "name"), "?");
            o.err(&format!("{}\n", env_warning(&n, sv)));
        }
        if let Some(c) = as_obj(get(d, "crossover")) {
            if truthy(get(c, "wouldHold")) {
                let msg = match get(c, "message").filter(|m| py::truthy(m)) {
                    Some(m) => py::str_of(m),
                    None => format!(
                        "it ran (this release only warns); {}",
                        get(c, "hint").map_or_else(String::new, py::str_of)
                    ),
                };
                o.err(&format!("warning: {msg}\n"));
            }
        }
        if let Some(w) = as_obj(get(d, "wall")) {
            if is_str(get(w, "path"), "open") {
                o.err(&format!(
                    "{}: {}, deployment {} ({})\n",
                    or_s(get(w, "app"), "?"),
                    or_s(get(w, "label"), "open path"),
                    or_s(get(w, "deployment"), "?"),
                    if truthy(get(w, "created")) {
                        "new App"
                    } else {
                        "redeployed"
                    }
                ));
            } else if is_str(get(w, "path"), "legacy") {
                o.err(&format!(
                    "note: it runs unwalled: {}\n",
                    or_s(get(w, "why"), "the open path isn't ready on this box")
                ));
            }
        }
    }

    if as_json {
        o.line(&compact(&Value::Object(d.to_vec())));
        return Ok(Exit::Ok);
    }

    if mode == Mode::List {
        return list(o, d, long);
    }
    service(o, d, verb)
}

/// The held `run`: the request, why, where the owner sees it; exit 2.
fn held(o: &mut Out, d: &Pairs, as_json: bool) -> R<Exit> {
    if as_json {
        o.line(&compact(&Value::Object(d.to_vec())));
        return Ok(Exit::Usage);
    }
    let empty: Vec<(Text, Value)> = Vec::new();
    let r = as_obj(get(d, "request")).unwrap_or(&empty[..]);
    let id = or(or(get(r, "id"), get(d, "requestId")), None);
    o.line(&format!(
        "held for the owner: {}  {}",
        id.map_or_else(|| "?".to_string(), py::str_of),
        or_s(get(r, "summary"), "")
    ));
    if let Some(why) = get(d, "reasons").filter(|w| py::truthy(w)) {
        for w in py_iter(why, 461)? {
            o.line(&format!("  why: {}", py::str_of(&w)));
        }
    }
    if truthy(get(d, "reused")) {
        o.line(&format!(
            "  this is your earlier request {} (asked {} ago), still pending; nothing new was filed",
            or_s(get(r, "id"), "?"),
            or_s(get(d, "askedAgo"), "a while")
        ));
    }
    if let Some(c) = as_obj(get(d, "card")) {
        if truthy(get(c, "where")) {
            o.line(&format!(
                "  owner: {}",
                py::str_of(get(c, "where").expect("truthy"))
            ));
        }
    }
    if truthy(get(d, "hint")) {
        o.line(&py::str_of(get(d, "hint").expect("truthy")));
    }
    Ok(Exit::Usage)
}

/// `ps`: the table and the lines under it.
fn list(o: &mut Out, d: &Pairs, long: bool) -> R<Exit> {
    let services = match get(d, "services") {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let mut rows: Vec<[String; 6]> = Vec::new();
    let mut warnings: Vec<(String, String)> = Vec::new();
    let mut details: Vec<(String, String)> = Vec::new();
    let mut reasons: Vec<(String, String, String)> = Vec::new();
    let mut legacy: Vec<String> = Vec::new();
    let mut unwalled: Vec<String> = Vec::new();
    let mut hint = format!("{CLI} study app-move");
    let mut ends: Option<String> = None;
    for item in &services {
        let Value::Object(sv) = item else { continue };
        let name = or_s(get(sv, "name"), "?");
        let port_s = match get(sv, "port") {
            None | Some(Value::Null) => "-".to_string(),
            Some(p) if py::str_of(p).is_empty() => "-".to_string(),
            Some(p) => py::str_of(p),
        };
        let mut u = url_or_target(sv)?;
        if env_legacy(sv) {
            legacy.push(name.clone());
            ends = env_when(sv);
            match env_ends(sv) {
                Some(e) => {
                    let when = if env_now(sv) { format!("now, {e}") } else { e };
                    u.push_str(&format!("  [legacy env (ends {when})]"));
                }
                None => u.push_str("  [legacy env]"),
            }
        }
        if truthy(get(sv, "legacyLabel")) {
            unwalled.push(name.clone());
            hint = match get(sv, "legacyHint").filter(|h| py::truthy(h)) {
                Some(h) => py::str_of(h),
                None => format!("{CLI} study app-move"),
            };
            let moved = match get(sv, "labelMovedTo").filter(|m| py::truthy(m)) {
                Some(m) => concat(": label moved to App ", m, 609)?,
                None => String::new(),
            };
            u.push_str(&format!("  [legacy (--cmd){moved}]"));
        }
        rows.push([
            name.clone(),
            status_text(sv),
            port_s,
            vis_label(sv),
            frame_label(sv),
            u,
        ]);
        if truthy(get(sv, "bindWarning")) {
            warnings.push((name.clone(), py::str_of(get(sv, "bindWarning").expect("truthy"))));
        }
        if long {
            let empty: Vec<(Text, Value)> = Vec::new();
            let v = as_obj(get(sv, "visibility")).unwrap_or(&empty[..]);
            if is_str(get(v, "mode"), "public") && truthy(get(v, "reason")) {
                details.push((
                    name.clone(),
                    format!(
                        "public because: {}",
                        py::str_of(get(v, "reason").expect("truthy"))
                    ),
                ));
            }
            if let Some(dv) = as_obj(get(sv, "dev")) {
                let idle = match get(dv, "idleSecs") {
                    None | Some(Value::Null) => "off".to_string(),
                    Some(i) => {
                        let n = py_int(i).map_err(|e| Raise::at(632, e))?;
                        format!("{}m", floor_div(n, 60))
                    }
                };
                details.push((
                    name.clone(),
                    format!(
                        "dev: {} \u{b7} caps: {} \u{b7} idle stop: {idle}",
                        or_s(get(dv, "lane"), "?"),
                        or_s(get(dv, "caps"), "?")
                    ),
                ));
                if truthy(get(dv, "warning")) {
                    warnings.push((name.clone(), py::str_of(get(dv, "warning").expect("truthy"))));
                }
            }
        }
        if truthy(get(sv, "frameWarning")) {
            warnings.push((name.clone(), py::str_of(get(sv, "frameWarning").expect("truthy"))));
        }
        if truthy(get(sv, "reason")) {
            reasons.push((
                name.clone(),
                or_s(get(sv, "status"), "down"),
                py::str_of(get(sv, "reason").expect("truthy")),
            ));
        }
        if truthy(get(sv, "reservedLabel")) {
            warnings.push((
                name.clone(),
                format!(
                    "this name is now reserved for a {PRODUCT} box door; it keeps working, but pick another name for anything new"
                ),
            ));
        }
    }
    if rows.is_empty() {
        o.line("No published services.");
        return Ok(Exit::Ok);
    }
    let heads = ["NAME", "STATUS", "PORT", "VISIBILITY", "FRAME"];
    let widths: Vec<usize> = (0..5)
        .map(|i| {
            rows.iter()
                .map(|r| chars(&r[i]))
                .chain([heads[i].len()])
                .max()
                .expect("one at least")
        })
        .collect();
    let line = |cells: [&str; 6]| -> String {
        let mut t = String::new();
        for (cell, width) in cells.iter().zip(&widths) {
            t.push_str(&pad(cell, *width));
            t.push_str("  ");
        }
        t.push_str(cells[5]);
        t
    };
    o.line(&line([
        "NAME",
        "STATUS",
        "PORT",
        "VISIBILITY",
        "FRAME",
        "URL/TARGET",
    ]));
    for r in &rows {
        o.line(&line([&r[0], &r[1], &r[2], &r[3], &r[4], &r[5]]));
    }
    for (n, w) in &details {
        o.line(&format!("{n}: {w}"));
    }
    for (n, w) in &warnings {
        o.line(&format!("warning: {n}: {w}"));
    }
    for (n, st, why) in &reasons {
        o.line(&format!(
            "{n} is not serving ({st}): {why}. More: {CLI} publish logs {n}"
        ));
    }
    if !legacy.is_empty() {
        let one = legacy.len() == 1;
        o.line(&format!(
            "warning: {} service{} still inherit{} the daemon environment ({}){}. Switch each with: {CLI} publish env <service> clean (grant secrets with {CLI} publish secret add <service> NAME).",
            legacy.len(),
            if one { "" } else { "s" },
            if one { "s" } else { "" },
            legacy.join(", "),
            ends.map_or_else(String::new, |e| format!("; this ends {e}"))
        ));
    }
    if !unwalled.is_empty() {
        let one = unwalled.len() == 1;
        o.line(&format!(
            "legacy (--cmd): {} run{} unwalled and keep{} running; move {} to an App when ready ({hint}).",
            unwalled.join(", "),
            if one { "s" } else { "" },
            if one { "s" } else { "" },
            if one { "it" } else { "them" }
        ));
    }
    Ok(Exit::Ok)
}

/// One service's answer (`run`, `start`, `stop`, `rm`, `frame`).
fn service(o: &mut Out, d: &Pairs, verb: &str) -> R<Exit> {
    let sv = as_obj(get(d, "service")).unwrap_or(d);
    let name = or_s(get(sv, "name"), "?");
    let status = status_text(sv);
    let port = get(sv, "port");
    let url = url_or_target(sv)?;
    if verb == "frame" {
        o.line(&format!("Frame policy {name} \u{b7} {}", frame_label(sv)));
        o.line(&format!(
            "  Content-Security-Policy: frame-ancestors {}",
            or_s(get(sv, "framePolicy"), "?")
        ));
        if truthy(get(d, "restarted")) {
            o.line("  App restarted with the new policy.");
        } else {
            o.line("  App is not running; the policy applies at the next start.");
        }
        return Ok(Exit::Ok);
    }
    let past = match verb {
        "run" | "start" => "Started",
        "stop" => "Stopped",
        "rm" => "Removed",
        _ => "ok",
    };
    let mut line = format!("{past} {name}");
    if !status.is_empty() {
        line.push_str(&format!(" \u{b7} {status}"));
    }
    let port_text = match port {
        None | Some(Value::Null) => None,
        Some(p) => Some(py::str_of(p)).filter(|t| !t.is_empty()),
    };
    if let Some(p) = &port_text {
        line.push_str(&format!(" \u{b7} port {p}"));
    }
    o.line(&line);
    if !url.is_empty() && url != "-" {
        o.line(&format!("  {url}"));
    }
    if truthy(get(sv, "hostnamePending")) {
        o.line(&format!(
            "  public URL is published once port {} answers ({CLI} publish status {name})",
            str_opt(port)
        ));
    }
    if verb == "run" {
        if let Some(v) = as_obj(get(sv, "visibility")) {
            let vm = str_opt(get(v, "mode"));
            if matches!(get(v, "enforced"), Some(Value::Bool(false))) {
                o.line(&format!(
                    "  {vm}, but {PRODUCT}'s check can't see this site's traffic with E2E off: anyone with the address can open it"
                ));
            } else if is_str(get(v, "mode"), "private") {
                o.line(&format!(
                    "  private: open it in a {PRODUCT} tab, or {CLI} publish open {name}. Agents: test on http://127.0.0.1:{} ({CLI} study publish-visibility)",
                    str_opt(port)
                ));
            } else if is_str(get(v, "mode"), "pin") {
                o.line(&format!("  behind a PIN ({CLI} publish visibility {name})"));
            }
        }
    }
    if truthy(get(d, "visibilityNote")) {
        o.line(&format!(
            "  {}",
            py::str_of(get(d, "visibilityNote").expect("truthy"))
        ));
    }
    if truthy(get(sv, "error")) {
        o.line(&format!(
            "  error: {}",
            py::str_of(get(sv, "error").expect("truthy"))
        ));
    }
    if env_legacy(sv) && verb != "run" && verb != "start" {
        match env_when(sv) {
            Some(w) => o.line(&format!(
                "  deprecated: inherits the daemon environment; ends {w} ({CLI} publish env {name} clean)"
            )),
            None => o.line(&format!(
                "  inherits the daemon environment (start it clean: {CLI} publish env {name} clean)"
            )),
        }
    }
    Ok(Exit::Ok)
}

/// The codes `_publish_site_render` exits 2 for.
const SITE_USAGE: &[&str] = &[
    "usage",
    "pin_required",
    "pin_length",
    "pin_characters",
    "pin_days",
    "pin_in_argv",
    "reason_required",
    "needs_yes",
    "dev_public_needs_flag",
    "bad_label",
    "bad_path",
];

/// `_publish_site_render` on `raw`, with `AS_JSON` and `P_VERB`.
pub fn site(raw: &str, as_json: bool, verb: &str) -> Shown {
    let mut o = Out::default();
    let r = site_into(&mut o, raw, as_json, verb);
    o.end(r)
}

fn bail(o: &mut Out, as_json: bool, code: &Value, text: &Value, exit: Exit) -> Exit {
    if as_json {
        let v = obj(vec![("error", obj(vec![("code", code.clone()), ("hint", text.clone())]))]);
        o.err(&format!("{}\n", compact(&v)));
    } else {
        o.err(&format!("Error: {}\n", py::str_of(text)));
    }
    exit
}

/// `when(ts)`.
fn when(ts: Option<&Value>) -> String {
    let Some(Ok(n)) = ts.map(py_int) else {
        return "-".to_string();
    };
    // `datetime` holds the years 1 to 9999 only (a day's margin for the
    // local offset at each end).
    let Ok(secs) = i64::try_from(n) else {
        return "-".to_string();
    };
    if !(-62_135_510_400..=253_402_214_400).contains(&secs) {
        return "-".to_string();
    }
    crate::out::pylib::time::strftime_local(secs, "%Y-%m-%d %H:%M").unwrap_or_else(|_| "-".to_string())
}

fn site_into(o: &mut Out, raw: &str, as_json: bool, verb: &str) -> R<Exit> {
    let d = match loads(raw) {
        Ok(Value::Object(d)) => d,
        _ => {
            let head: String = strip(raw).chars().take(200).collect();
            let head = if head.is_empty() { "(empty)".to_string() } else { head };
            return Ok(bail(
                o,
                as_json,
                &s("unreachable"),
                &s(&format!("no answer from the daemon: {head}")),
                Exit::Error,
            ));
        }
    };
    let d = &d[..];
    if is_str(get(d, "status"), "held") {
        if as_json {
            o.line(&compact(&Value::Object(d.to_vec())));
        } else {
            o.line(&match get(d, "message").filter(|m| py::truthy(m)) {
                Some(m) => py::str_of(m),
                None => format!("This needs a person: {PRODUCT} put a card on your Thread."),
            });
            if truthy(get(d, "request")) {
                o.line(&format!(
                    "  request: {}",
                    py::str_of(get(d, "request").expect("truthy"))
                ));
            }
        }
        return Ok(Exit::Usage);
    }
    if let Some(err) = get(d, "error").filter(|e| py::truthy(e)) {
        let (code, mut text) = if let Value::Object(e) = err {
            (
                or(get(e, "code"), None).cloned().unwrap_or_else(|| s("error")),
                or(or(get(e, "hint"), get(e, "message")), None)
                    .cloned()
                    .unwrap_or_else(|| s("refused")),
            )
        } else {
            (
                or(get(d, "code"), None).cloned().unwrap_or_else(|| s("error")),
                s(&py::str_of(err)),
            )
        };
        if let Some(h) = get(d, "hint").filter(|h| py::truthy(h)) {
            text = s(&format!("{}\n  {}", py::str_of(&text), py::str_of(h)));
        }
        let usage = SITE_USAGE.iter().any(|c| py_eq(&code, &s(c)));
        let exit = if usage { Exit::Usage } else { Exit::Refused };
        return Ok(bail(o, as_json, &code, &text, exit));
    }
    if as_json {
        o.line(&compact(&Value::Object(d.to_vec())));
        return Ok(Exit::Ok);
    }
    match verb {
        "visibility" => {
            let mode = or(get(d, "mode"), None).cloned().unwrap_or_else(|| s("?"));
            let label = or_s(get(d, "label"), "?");
            o.line(&format!(
                "{label}: {}{}",
                py::str_of(&mode),
                if truthy(get(d, "dev")) { ", dev" } else { "" }
            ));
            if truthy(get(d, "url")) {
                o.line(&format!("  {}", py::str_of(get(d, "url").expect("truthy"))));
            }
            if matches!(mode, Value::Array(_) | Value::Object(_)) {
                return Err(Raise::at(
                    1347,
                    format!("TypeError: unhashable type: '{}'", py::type_name(&mode)),
                ));
            }
            let words = match &mode {
                Value::Str(t) => match t.as_str() {
                    Some("private") => Some(format!(
                        "Only the owner and Admins can open it ({CLI} publish open {}).",
                        or_s(get(d, "label"), "<name>")
                    )),
                    Some("pin") => Some(format!(
                        "{PRODUCT} asks for the PIN first. Send the address and the PIN separately."
                    )),
                    Some("public") => Some("Anyone with the address can open it.".to_string()),
                    _ => None,
                },
                _ => None,
            };
            if let Some(w) = words {
                o.line(&format!("  {w}"));
            }
            if is_str(Some(&mode), "public") && truthy(get(d, "reason")) {
                o.line(&format!(
                    "  Why: {}",
                    py::str_of(get(d, "reason").expect("truthy"))
                ));
            }
            if let Some(ws) = get(d, "warnings").filter(|w| py::truthy(w)) {
                for w in py_iter(ws, 1353)? {
                    o.line(&format!("warning: {}", py::str_of(&w)));
                }
            }
        }
        "open" => {
            if truthy(get(d, "opened")) {
                o.line(&format!("Opened {} in your browser.", or_s(get(d, "label"), "the site")));
            } else {
                o.line(&or_s(get(d, "url"), ""));
                if truthy(get(d, "oneTime")) {
                    o.err(&format!(
                        "  This link works once, within {} seconds.\n",
                        or_s(get(d, "expiresInSecs"), "60")
                    ));
                }
            }
        }
        "sessions" => {
            match get(d, "revoked") {
                Some(n) if *n != Value::Null => {
                    let one = py_eq(n, &Value::Int("1".to_string()));
                    o.line(&format!(
                        "Ended {} session{} on {}.",
                        py::str_of(n),
                        if one { "" } else { "s" },
                        or_s(get(d, "label"), "?")
                    ));
                }
                _ => {
                    let rows = or(get(d, "sessions"), None).cloned();
                    match rows {
                        None => o.line(&format!(
                            "Nobody is signed in to {}.",
                            or_s(get(d, "label"), "?")
                        )),
                        Some(rows) => {
                            o.line(&format!(
                                "{}  {}  {}  ENDS",
                                pad("KIND", 8),
                                pad("WHO", 24),
                                pad("STARTED", 16)
                            ));
                            let items = py_iter(&rows, 1371)?;
                            for r in &items {
                                let Value::Object(p) = r else {
                                    return Err(Raise::no_attribute(1373, r, "get"));
                                };
                                o.line(&format!(
                                    "{}  {}  {}  {}",
                                    pad(&or_s(get(p, "kind"), "?"), 8),
                                    pad(&or_s(get(p, "user"), "-"), 24),
                                    pad(&when(get(p, "createdAt")), 16),
                                    when(get(p, "expiresAt"))
                                ));
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(Exit::Ok)
}

/// The `subdomain claim` / `unclaim` reader on the daemon's answer.
pub fn claimed(raw: &str, action: &str, label: &str) -> Shown {
    let text = if raw.is_empty() { "{}" } else { raw };
    let d = loads(text).unwrap_or_else(|_| Value::Object(Vec::new()));
    let pairs = match &d {
        Value::Object(p) => Some(&p[..]),
        _ => None,
    };
    let ok = pairs.is_some_and(|p| truthy(get(p, "success")));
    if !ok {
        let err = pairs
            .and_then(|p| get(p, "error"))
            .filter(|e| py::truthy(e))
            .map_or_else(|| "no response from the daemon".to_string(), py::str_of);
        let mut lines = format!("Error: {err}\n");
        if let Some(h) = pairs.and_then(|p| get(p, "hint")).filter(|h| py::truthy(h)) {
            lines.push_str(&format!("  {}\n", py::str_of(h)));
        }
        return Shown::err(lines, Exit::Error);
    }
    let p = pairs.expect("a dict when it succeeded");
    let project = or_s(get(p, "projectId"), "?");
    let line = if action == "claim" && truthy(get(p, "transferred")) {
        format!(
            "Transferred {label} to this workspace (project {project}). Recorded in the access audit log."
        )
    } else if action == "claim" {
        format!("Claimed {label} for this workspace (project {project}).")
    } else if truthy(get(p, "removed")) {
        format!("Unclaimed {label} \u{2014} no workspace attribution remains.")
    } else {
        format!("{label} had no workspace attribution (nothing to remove).")
    };
    Shown::out(format!("{line}\n"))
}

/// `_publish_resp_code`: the refusal's `code`, as `$(…)` holds it.
pub fn resp_code(raw: &str) -> String {
    let code = match loads(raw) {
        Ok(Value::Object(p)) => or_s(get(&p, "code"), ""),
        _ => String::new(),
    };
    code.trim_end_matches('\n').to_string()
}

/// `json.loads(RESP).get("check")` is truthy (any error: no).
pub fn asks_pin(raw: &str) -> bool {
    matches!(loads(raw), Ok(Value::Object(p)) if truthy(get(&p, "check")))
}

/// `print(json.loads(RESP).get("error", ""))`, for an answer that is a
/// dict (its code was read from it).
pub fn needs_yes_line(raw: &str) -> String {
    match loads(raw) {
        Ok(Value::Object(p)) => format!("{}\n", get(&p, "error").map_or_else(String::new, py::str_of)),
        _ => "\n".to_string(),
    }
}

/// `--probe-timeout`'s check, as the daemon's: `Err` with the line its
/// Python prints.
pub fn probe_timeout(value: &str) -> Result<(), String> {
    let shown = strip(value);
    let raw = shown.to_lowercase();
    let bad = format!(
        "--probe-timeout must be a duration like 90s, 10m or 1500ms (at most 15m), got '{shown}'"
    );
    if raw == "default" {
        return Ok(());
    }
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let num = |t: &str| -> u128 {
        t.trim_start_matches('0')
            .parse::<u128>()
            .unwrap_or(if t.trim_start_matches('0').is_empty() { 0 } else { u128::MAX })
    };
    let ms: u128 = if digits(&raw) {
        num(&raw).saturating_mul(1000)
    } else {
        let mut total: u128 = 0;
        let mut rest = raw.as_str();
        if rest.is_empty() {
            return Err(bad);
        }
        while !rest.is_empty() {
            let n = rest.bytes().take_while(u8::is_ascii_digit).count();
            if n == 0 {
                return Err(bad);
            }
            let (d, after) = rest.split_at(n);
            let (unit, after) = if let Some(a) = after.strip_prefix("ms") {
                (1u128, a)
            } else if let Some(a) = after.strip_prefix('s') {
                (1000, a)
            } else if let Some(a) = after.strip_prefix('m') {
                (60_000, a)
            } else if let Some(a) = after.strip_prefix('h') {
                (3_600_000, a)
            } else {
                return Err(bad);
            };
            total = total.saturating_add(num(d).saturating_mul(unit));
            rest = after;
        }
        total
    };
    if ms == 0 {
        return Err(bad);
    }
    if ms > 15 * 60 * 1000 {
        return Err(format!(
            "--probe-timeout is at most 15m (got {shown}); a service that needs longer should open its port first and build behind it"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_timeout_rule() {
        for ok in ["90s", "10m", "1m30s", "1500ms", "default", " DEFAULT ", "900", "15m"] {
            assert_eq!(probe_timeout(ok), Ok(()), "{ok}");
        }
        for bad in ["", "0", "0s", "1d", "m", "5x", "1msm", "1.5s"] {
            assert!(
                probe_timeout(bad).unwrap_err().starts_with("--probe-timeout must be"),
                "{bad}"
            );
        }
        assert!(probe_timeout("16m").unwrap_err().starts_with("--probe-timeout is at most 15m (got 16m)"));
        assert!(probe_timeout("99999999999999999999999999999999999999999h")
            .unwrap_err()
            .starts_with("--probe-timeout is at most"));
    }

    #[test]
    fn fmt_left_as_python_does() {
        assert_eq!(fmt_left(&Value::Int("252000".into())), "4m12s");
        assert_eq!(fmt_left(&Value::Int("240000".into())), "4m");
        assert_eq!(fmt_left(&Value::Int("5999".into())), "5s");
        assert_eq!(fmt_left(&Value::Int("-5".into())), "0s");
        assert_eq!(fmt_left(&s("x")), "?");
        assert_eq!(fmt_left(&Value::Null), "?");
    }
}
