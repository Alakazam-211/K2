//! `_mail_py` (`cli/src/mail.sh`): the Python every `k5 mail` verb but
//! `watch` and `reader` runs, as the binary runs it.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]).
//! 2. `mail` is an `id` tool: with `HOOK_SOCK` set, the stale-passport or
//!    require-passport lines, exit 3 ([`crate::prelude::id_tool`]).
//! 3. The verb's words, as its `cmd_mail_*` reads them (each group's
//!    `parse`): a mistake is `_mail_err "usage" …`, one line of JSON on
//!    stderr, exit 2, and nothing is sent.
//! 4. The Python, with the verb's `M_*` settings ([`Env`]): its calls go
//!    through `request` and `call` ([`Py::request`], [`Py::call`]) over
//!    the urllib adapter ([`crate::transport::urllib`]), with the shell's
//!    tries (a refused connect and the 503 "migrating" answer, four tries
//!    a second apart; a dropped GET once more). `K5_MAIL_HTTP_TIMEOUT`
//!    (and its older names) sets every call's timeout.
//! 5. `die` ([`Py::die`]): the error's one line of JSON on stderr, the
//!    permission card's closing line, and exit 3 for the codes that need
//!    a person ([`NEEDS_HUMAN`]), 2 for usage, else 1. No answer at all is
//!    `daemon_unreachable`, exit 1; no answer in time is `timeout`.
//!
//! Where the shell's Python raises (an answer that is not an object, a
//! field of a type it doesn't expect), this prints a traceback in Python
//! 3.12's form for `_mail_py`'s script (read from stdin, run by the boot
//! script's `exec`): the frames this module pins and the exception.
//!
//! Where this differs from the shell:
//!
//! - `iso_ago` reads the ISO dates Python 3.12's `fromisoformat` reads in
//!   their usual forms (`YYYY-MM-DD`, `YYYYMMDD`, a time to the second or
//!   a fraction of one, `Z` or an offset). A rarer form Python takes (an
//!   ISO week date, a time with no date part before it) is printed as it
//!   came, where Python prints its age.
//! - A string with half of a surrogate pair prints as `\udXXX` where
//!   Python's `print` fails. The daemon writes its JSON from Rust strings,
//!   which hold none.

use super::super::curl_port;
use super::py::{self as p, attr_get, iter, join, text};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude;
use crate::python;
use crate::transport::urllib::{self, Kind, Method, Opener, Outcome, Request};
use k2_client::{cli_name, product_name};
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

/// `die`'s codes that need a person: exit 3.
pub const NEEDS_HUMAN: &[&str] = &[
    "gated",
    "forbidden",
    "owner_only",
    "agent_send_path_only",
    "capability_missing",
];

/// `die`'s codes for a mistake in the request: exit 2.
pub const USAGE_CODES: &[&str] = &["usage", "attachment_secret", "attachment_too_large"];

/// The line of the boot script (`_K2_PY_STDIN_BOOT`) that runs `_mail_py`.
pub const BOOT_LINE: u32 = 13;

/// The lines of `_mail_py` (line 1 is `import base64, …`) where it raises.
/// `tests` pins each to its statement.
pub mod line {
    pub const DIE: u32 = 22;
    pub const DIE_CODE: u32 = 26;
    pub const DIE_HINT: u32 = 27;
    pub const DIE_IN: u32 = 28;
    pub const DIE_CONCAT: u32 = 31;
    pub const DIE_CAP: u32 = 36;
    pub const REQUEST_FLOAT: u32 = 88;
    pub const REQUEST_READ: u32 = 107;
    pub const CALL_REQUEST: u32 = 146;
    pub const CALL_TIMEOUT: u32 = 149;
    pub const CALL_DIE_TIMEOUT: u32 = 150;
    pub const CALL_ERROR: u32 = 151;
    pub const CALL_DIE_ERROR: u32 = 152;
    pub const CALL_OK: u32 = 153;
    pub const FMT_ADDR: u32 = 218;
    pub const FMT_ADDR_EMAIL: u32 = 219;
    pub const FMT_ADDRS: u32 = 223;
    pub const CAP_USED: u32 = 226;
    pub const CAP_LIMIT: u32 = 227;
    pub const FULL_ID: u32 = 237;
    pub const FULL_ADDRESS: u32 = 238;
    pub const FULL_FROM: u32 = 239;
    pub const FULL_TO: u32 = 240;
    pub const FULL_CC_IF: u32 = 241;
    pub const FULL_CC: u32 = 242;
    pub const FULL_SUBJECT: u32 = 243;
    pub const FULL_DATE: u32 = 244;
    pub const FULL_AUTH: u32 = 245;
    pub const FULL_ANY: u32 = 246;
    pub const FULL_AUTH_LINE: u32 = 247;
    pub const FULL_ATTS: u32 = 249;
    pub const FULL_FOR: u32 = 252;
    pub const FULL_ATT: u32 = 253;
    pub const FULL_FETCH: u32 = 255;
    pub const FULL_TEXT: u32 = 257;
    pub const FULL_HTML_IF: u32 = 258;
    pub const FULL_HTML: u32 = 260;
    pub const ROW_FIRST: u32 = 300;
    pub const ROW_BCC_IF: u32 = 303;
    pub const ROW_BCC: u32 = 304;
    pub const ROW_POLICY_IF: u32 = 305;
    pub const ROW_POLICY: u32 = 306;
    pub const ROW_REPLY_IF: u32 = 307;
    pub const ROW_REPLY: u32 = 308;
    pub const ROW_FWD_IF: u32 = 309;
    pub const ROW_FWD: u32 = 310;
    pub const ROW_ATT: u32 = 311;
    pub const ROW_ATT_IF: u32 = 312;
    pub const ROW_ATT_LINE: u32 = 315;
    pub const ROW_NOTE_IF: u32 = 316;
    pub const ROW_NOTE: u32 = 318;
    pub const FWD_LINE: u32 = 295;
    pub const FWD_MAILBOX: u32 = 296;
    pub const FWD_EML: u32 = 297;
    pub const DETAIL_ID: u32 = 321;
    pub const DETAIL_STATUS: u32 = 322;
    pub const DETAIL_FROM: u32 = 323;
    pub const DETAIL_TO: u32 = 324;
    pub const DETAIL_CC_IF: u32 = 325;
    pub const DETAIL_CC: u32 = 326;
    pub const DETAIL_BCC_IF: u32 = 327;
    pub const DETAIL_BCC: u32 = 328;
    pub const DETAIL_POLICY_IF: u32 = 329;
    pub const DETAIL_POLICY: u32 = 330;
    pub const DETAIL_REPLY_IF: u32 = 331;
    pub const DETAIL_REPLY: u32 = 332;
    pub const DETAIL_FWD_IF: u32 = 333;
    pub const DETAIL_FWD: u32 = 334;
    pub const DETAIL_SUBJECT: u32 = 335;
    pub const DETAIL_ATT: u32 = 336;
    pub const DETAIL_ATT_IF: u32 = 337;
    pub const DETAIL_ATT_LINE: u32 = 338;
    pub const DETAIL_QUEUED: u32 = 339;
    pub const DETAIL_DECIDED_IF: u32 = 340;
    pub const DETAIL_BY: u32 = 341;
    pub const DETAIL_DECIDED: u32 = 342;
    pub const DETAIL_SENT_IF: u32 = 343;
    pub const DETAIL_SENT: u32 = 344;
    pub const DETAIL_NOTE_IF: u32 = 345;
    pub const DETAIL_NOTE: u32 = 346;
    pub const DOMAIN_FIRST: u32 = 355;
    pub const DOMAIN_FOR: u32 = 361;
    pub const DOMAIN_NOTE: u32 = 371;
    pub const INBOXES_W: u32 = 423;
    pub const INBOXES_ST: u32 = 434;
    pub const INBOXES_ROW: u32 = 437;
    pub const INBOXES_PRI: u32 = 458;
    pub const INBOXES_LVL: u32 = 462;
    pub const INBOXES_GSUM: u32 = 468;
    pub const INBOXES_OWNER_ROW: u32 = 470;
    pub const INBOXES_SORTED: u32 = 479;
}

/// The verb's `M_*` settings: what `env(k)` reads. A setting not given
/// reads as empty, as the shell's `${M_X:-}` does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env(Vec<(&'static str, String)>);

impl Env {
    pub fn new() -> Env {
        Env::default()
    }

    /// Sets `key` (the last one set wins).
    pub fn set(&mut self, key: &'static str, value: impl Into<String>) {
        let value = value.into();
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }

    /// With `key` set.
    pub fn with(mut self, key: &'static str, value: impl Into<String>) -> Env {
        self.set(key, value);
        self
    }

    /// `env(k)`.
    pub fn get(&self, key: &str) -> &str {
        self.0
            .iter()
            .find(|(k, _)| *k == key)
            .map_or("", |(_, v)| v.as_str())
    }

    /// `env(k) == "true"`.
    pub fn on(&self, key: &str) -> bool {
        self.get(key) == "true"
    }

    /// `env_list(k)`: newline-joined, each piece also split at commas,
    /// trimmed, empty ones dropped.
    pub fn list(&self, key: &str) -> Vec<String> {
        self.get(key)
            .split('\n')
            .flat_map(|chunk| chunk.split(','))
            .map(|piece| piece.trim_matches(super::super::py_space).to_string())
            .filter(|piece| !piece.is_empty())
            .collect()
    }

    /// `env_lines(k)`: newline-joined only, lines of nothing but white
    /// space dropped.
    pub fn lines(&self, key: &str) -> Vec<String> {
        self.get(key)
            .split('\n')
            .filter(|l| !l.trim_matches(super::super::py_space).is_empty())
            .map(str::to_string)
            .collect()
    }
}

/// What a verb's words came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// `_mail_py` with these settings: `M_VERB`, `M_JSON` and the rest.
    Run(Env),
    /// A help page (`cli/text/help/<page>.txt`), exit 0; nothing sent.
    Help(&'static str),
    /// `_mail_err "usage" <hint>`, exit 2; nothing sent.
    Usage(String),
    /// The shell's.
    Shell,
}

/// `_mail_err "usage" <hint>`.
pub fn usage(hint: impl Into<String>) -> Plan {
    Plan::Usage(hint.into())
}

/// `_mail_py` with `M_VERB` set to `verb` and `M_JSON` to `json`.
pub fn run_env(verb: &str, json: bool) -> Env {
    Env::new()
        .with("M_VERB", verb)
        .with("M_JSON", if json { "true" } else { "false" })
}

/// `_mail_is_uint`.
pub fn is_uint(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `[ "$n" -lt … ]` takes it: digits that fit bash's 64-bit integers.
pub fn fits_bash(s: &str) -> bool {
    let t = s.trim_start_matches('0');
    t.len() < 19 || (t.len() == 19 && t <= "9223372036854775807")
}

/// The value of `n` as bash's `[` reads it (digits that fit).
pub fn bash_num(s: &str) -> u64 {
    s.trim_start_matches('0').parse().unwrap_or(0)
}

/// `_mail_err "<code>" <hint>`'s line.
pub fn err_line(code: &str, hint: &str) -> String {
    dumps(&obj(vec![(
        "error",
        obj(vec![("code", p::sv(code)), ("hint", p::sv(hint))]),
    )]))
}

/// `json.dumps(v, ensure_ascii=False, separators=(",", ":"))`; a lone
/// surrogate is written as Python's `\udXXX` escape would show it.
pub fn dumps(v: &Value) -> String {
    let opts = Options {
        compact: true,
        ensure_ascii: false,
        indent: None,
        sort_keys: false,
    };
    json::to_string(v, &opts).unwrap_or_else(|_| {
        json::to_string(
            v,
            &Options {
                ensure_ascii: true,
                ..opts
            },
        )
        .unwrap_or_default()
    })
}

/// A dict of these pairs.
pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// How the Python ended early: `sys.exit` after `die` wrote its line, or
/// an exception.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    Exit(Exit),
    Raise(Tb),
}

/// A traceback: the frames in `_mail_py` (line, function), outermost
/// first, and the exception's line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tb {
    pub frames: Vec<(u32, &'static str)>,
    pub exception: String,
}

impl Tb {
    /// As Python 3.12 prints it: the boot script's `exec`, then the
    /// script's frames (no source lines for either).
    pub fn text(&self) -> String {
        let mut s = String::from("Traceback (most recent call last):\n");
        s.push_str(&format!(
            "  File \"<string>\", line {BOOT_LINE}, in <module>\n"
        ));
        for (line, name) in &self.frames {
            s.push_str(&format!("  File \"<stdin>\", line {line}, in {name}\n"));
        }
        s.push_str(&self.exception);
        s.push('\n');
        s
    }
}

pub type R<T> = Result<T, End>;

/// The module's name in a frame.
pub const MODULE: &str = "<module>";

/// An exception's text raised at `line` of `name`.
pub trait At<T> {
    fn at(self, line: u32, name: &'static str) -> R<T>;
}

impl<T> At<T> for Result<T, String> {
    fn at(self, line: u32, name: &'static str) -> R<T> {
        self.map_err(|exception| {
            End::Raise(Tb {
                frames: vec![(line, name)],
                exception,
            })
        })
    }
}

/// The caller's frame on an exception from a function it called.
pub trait Within {
    fn within(self, line: u32, name: &'static str) -> Self;
}

impl<T> Within for R<T> {
    fn within(self, line: u32, name: &'static str) -> Self {
        self.map_err(|e| match e {
            End::Raise(mut t) => {
                t.frames.insert(0, (line, name));
                End::Raise(t)
            }
            other => other,
        })
    }
}

/// An exception raised at `line` of the module.
pub fn raise_at(line: u32, exception: impl Into<String>) -> End {
    End::Raise(Tb {
        frames: vec![(line, MODULE)],
        exception: exception.into(),
    })
}

/// The `K5_MAIL_HTTP_TIMEOUT` setting (its K5_, K2_ and K2SO_ names, the
/// first that is set and not empty), as Python's `os.environ` holds it.
fn timeout_setting() -> Option<Result<String, ()>> {
    k2_client::env_names::first_nonempty_os_in(k2_client::env_names::MAIL_HTTP_TIMEOUT, |n| {
        std::env::var_os(n)
    })
    .map(|(_, v)| v.into_string().map_err(|_| ()))
}

/// Python's `float(text)`: `None` where it raises `ValueError`.
pub fn py_float(text: &str) -> Option<f64> {
    let t = text.trim_matches(super::super::py_space);
    let lower = t.to_ascii_lowercase();
    let unsigned = lower.trim_start_matches(['+', '-']);
    if lower.len() - unsigned.len() > 1 {
        return None;
    }
    let neg = lower.starts_with('-');
    let special = match unsigned {
        "inf" | "infinity" => Some(f64::INFINITY),
        "nan" => Some(f64::NAN),
        _ => None,
    };
    if let Some(v) = special {
        return Some(if neg { -v } else { v });
    }
    // Digits with single `_` between them, an optional point and
    // exponent: what Rust's parser takes once the `_` are gone.
    let b = unsigned.as_bytes();
    if b.is_empty()
        || !b
            .iter()
            .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'+' | b'-' | b'_'))
    {
        return None;
    }
    for (i, c) in b.iter().enumerate() {
        if *c == b'_' {
            let before = i.checked_sub(1).map(|j| b[j]);
            let after = b.get(i + 1);
            if !before.is_some_and(|x| x.is_ascii_digit())
                || !after.is_some_and(|x| x.is_ascii_digit())
            {
                return None;
            }
        }
    }
    t.replace('_', "").parse::<f64>().ok()
}

/// A finite number of seconds the socket takes; `None` where urlopen's
/// `settimeout` raises (negative, NaN, too big), which `request` catches
/// as no answer.
fn socket_timeout(secs: f64) -> Option<Duration> {
    // _PyTime_t holds about 292 years of nanoseconds.
    (secs > 0.0 && secs < 9.2e9).then(|| Duration::from_secs_f64(secs))
}

/// The Python run: its settings and what it printed so far.
pub struct Py {
    pub opener: Opener,
    /// `ws_path` (`$PROJECT`).
    pub ws: String,
    pub env: Env,
    /// `as_json`.
    pub json: bool,
    pub stdout: Vec<u8>,
}

/// A timeout as `call` takes it: whole seconds, or the setting's float.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Secs {
    Int(u64),
    Float(f64),
}

impl Secs {
    fn as_f64(self) -> f64 {
        match self {
            Secs::Int(n) => n as f64,
            Secs::Float(f) => f,
        }
    }

    /// `int(timeout_secs)` as `%s` prints it.
    fn whole(self) -> String {
        match self {
            Secs::Int(n) => n.to_string(),
            Secs::Float(f) => py::percent_d(&Value::Float(f)).unwrap_or_else(|_| "0".into()),
        }
    }
}

impl Py {
    pub fn new(opener: Opener, ws: String, env: Env) -> Py {
        let json = env.on("M_JSON");
        Py {
            opener,
            ws,
            env,
            json,
            stdout: Vec::new(),
        }
    }

    /// `env(k)`.
    pub fn env(&self, key: &str) -> &str {
        self.env.get(key)
    }

    /// `print(text)`.
    pub fn print(&mut self, text: &str) {
        self.stdout.extend_from_slice(text.as_bytes());
        self.stdout.push(b'\n');
    }

    /// `print(v)`.
    pub fn print_v(&mut self, v: &Value) {
        let t = py::str_of(v);
        self.print(&t);
    }

    /// `print(dumps(d))`.
    pub fn print_json(&mut self, d: &Value) {
        let t = dumps(d);
        self.print(&t);
    }

    /// `die(err, exit_code, permission)`: the error's line on stderr (and
    /// the permission card's closing line), and its exit. `err` is what
    /// the Python handed it; where `die` itself raises, the traceback.
    pub fn die(&self, err: &Value, exit: Option<Exit>, permission: Option<&Value>) -> End {
        let fr = |e: String, line: u32| -> End {
            End::Raise(Tb {
                frames: vec![(line, "die")],
                exception: e,
            })
        };
        let err = match err {
            Value::Str(_) => obj(vec![("code", p::sv("error")), ("hint", err.clone())]),
            other => other.clone(),
        };
        let code = match p::get_or(&err, "code", p::sv("error")) {
            Ok(v) => v,
            Err(e) => return fr(e, line::DIE_CODE),
        };
        let mut hint = match p::get_or(&err, "hint", p::sv("")) {
            Ok(v) => v,
            Err(e) => return fr(e, line::DIE_HINT),
        };
        if p::eq_str(&code, "forbidden") || p::eq_str(&code, "owner_only") {
            match p::contains(&hint, "ask your human") {
                Err(e) => return fr(e, line::DIE_IN),
                Ok(true) => {}
                Ok(false) => {
                    let head = if py::truthy(&hint) {
                        match p::concat_str(&hint, " — ") {
                            Ok(h) => h,
                            Err(e) => return fr(e, line::DIE_CONCAT),
                        }
                    } else {
                        String::new()
                    };
                    hint = p::sv(&format!("{head}requires owner/admin — ask your human"));
                }
            }
        }
        let mut out = vec![("code", code.clone()), ("hint", hint)];
        for k in ["cap", "reason", "fix"] {
            // `err` is a dict here: `get` worked above.
            if let Ok(Some(v)) = attr_get(&err, k) {
                if py::truthy(&v) {
                    out.push((k, v));
                }
            }
        }
        let mut lines = dumps(&obj(vec![("error", obj(out))]));
        lines.push('\n');
        if let Some(Value::Object(perm)) = permission {
            let perm = Value::Object(perm.clone());
            let cmd = attr_get(&perm, "command").ok().flatten();
            match cmd {
                Some(Value::Str(t)) if !t.code_points().is_empty() => {
                    lines.push_str(&format!("Ask with: {}\n", text(&t)));
                }
                _ => {
                    if matches!(
                        attr_get(&perm, "requestable").ok().flatten(),
                        Some(Value::Bool(false))
                    ) {
                        if let Ok(Some(Value::Str(h))) = attr_get(&perm, "hint") {
                            if !h.code_points().is_empty() {
                                lines.push_str(&format!("{}\n", text(&h)));
                            }
                        }
                        lines.push_str("It can't be requested.\n");
                    }
                }
            }
        }
        let _ = out::write_flushed(&mut std::io::stderr().lock(), lines.as_bytes());
        End::Exit(exit.unwrap_or_else(|| {
            let named = |list: &[&str]| list.iter().any(|c| p::eq_str(&code, c));
            if named(NEEDS_HUMAN) {
                Exit::Refused
            } else if named(USAGE_CODES) {
                Exit::Usage
            } else {
                Exit::Error
            }
        }))
    }

    /// `die({"code": code, "hint": hint}, exit_code)`.
    pub fn die_with(&self, code: &str, hint: &str, exit: Option<Exit>) -> End {
        self.die(
            &obj(vec![("code", p::sv(code)), ("hint", p::sv(hint))]),
            exit,
            None,
        )
    }

    /// `die({"code": "daemon_unreachable", …})`.
    pub fn unreachable(&self) -> End {
        self.die_with(
            "daemon_unreachable",
            concat!(
                "no response from the ",
                product_name!(),
                " daemon (is it running?)"
            ),
            None,
        )
    }

    /// `request(method, route, params, body, timeout)`: the answer as
    /// `json.loads` read it (any value, `None` as `Ok(None)`), the HTTP
    /// error's stand-in, the timeout's, or no answer (`Ok(None)`).
    pub fn request(
        &self,
        method: Method,
        route: &str,
        params: &[(String, String)],
        body: Option<&Value>,
        timeout: Secs,
    ) -> R<Option<Value>> {
        let timeout = match timeout_setting() {
            None => timeout,
            Some(Ok(t)) => match py_float(&t) {
                Some(f) => Secs::Float(f),
                None => {
                    return Err(End::Raise(Tb {
                        frames: vec![(line::REQUEST_FLOAT, "request")],
                        exception: format!(
                            "ValueError: could not convert string to float: {}",
                            py::repr(&p::sv(&t))
                        ),
                    }))
                }
            },
            Some(Err(())) => {
                return Err(End::Raise(Tb {
                    frames: vec![(line::REQUEST_FLOAT, "request")],
                    exception: "ValueError: could not convert string to float".into(),
                }))
            }
        };
        let Some(dur) = socket_timeout(timeout.as_f64()) else {
            return Ok(None);
        };
        let mut req = Request::new(method, route, dur).query(params.to_vec());
        if let Some(b) = body {
            let data = dumps(b).into_bytes();
            req = req.data(data.clone());
            if !data.is_empty() {
                req = req.header("Content-Type", "application/json");
            }
        }
        if let Some(t) = self.opener.token() {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        match self.opener.open_retrying(&req) {
            Outcome::Answer { body: Ok(b), .. } => match json::parse(&urllib::decode(&b)) {
                Ok(Value::Null) | Err(_) => Ok(None),
                Ok(d) => Ok(Some(d)),
            },
            Outcome::HttpError { code, body } => {
                // `e.read()` failing raises out of the handler: the shell
                // prints a chained traceback through the standard
                // library; here, the same exception's last line.
                let b = body.map_err(|r| {
                    End::Raise(Tb {
                        frames: vec![(line::REQUEST_READ, "request")],
                        exception: r.traceback_line(),
                    })
                })?;
                Ok(match json::parse(&urllib::decode(&b)) {
                    Ok(Value::Null) => None,
                    Ok(d) => Some(d),
                    Err(_) => Some(obj(vec![(
                        "error",
                        obj(vec![
                            ("code", p::sv("error")),
                            ("hint", p::sv(&format!("HTTP {code} on {route}"))),
                        ]),
                    )])),
                })
            }
            Outcome::Answer { body: Err(r), .. } | Outcome::Raised(r) => {
                if r.kind == Kind::Timeout {
                    return Ok(Some(obj(vec![(
                        "error",
                        obj(vec![
                            ("code", p::sv("timeout")),
                            (
                                "hint",
                                p::sv(&format!(
                                    "daemon did not finish within {}s (still running — the mail fetch is slow, not down)",
                                    timeout.whole()
                                )),
                            ),
                        ]),
                    )])));
                }
                Ok(None)
            }
        }
    }

    /// `call(method, route, params, body, timeout, timeout_hint)`, from
    /// module line `from`.
    #[allow(clippy::too_many_arguments)]
    pub fn call_full(
        &self,
        method: Method,
        route: &str,
        params: &[(String, String)],
        body: Option<&Value>,
        timeout: Secs,
        timeout_hint: Option<&dyn Fn(&Value) -> String>,
        from: u32,
    ) -> R<Value> {
        let within = |r: End| match r {
            End::Raise(mut t) => {
                t.frames.insert(0, (from, MODULE));
                End::Raise(t)
            }
            other => other,
        };
        let inside = |line: u32, e: String| {
            within(End::Raise(Tb {
                frames: vec![(line, "call")],
                exception: e,
            }))
        };
        let d = self
            .request(method, route, params, body, timeout)
            .within(line::CALL_REQUEST, "call")
            .map_err(within)?;
        let Some(d) = d else {
            return Err(self.unreachable());
        };
        if let Some(hint_of) = timeout_hint {
            let e = attr_get(&d, "error").map_err(|e| inside(line::CALL_TIMEOUT, e))?;
            let e = p::or(e, Value::Object(Vec::new()));
            let code = attr_get(&e, "code").map_err(|e| inside(line::CALL_TIMEOUT, e))?;
            if code.as_ref().is_some_and(|c| p::eq_str(c, "timeout")) {
                // `d["error"].get("hint", "")`: `d["error"]` is the dict
                // whose code was read.
                let h = p::get_or(&e, "hint", p::sv("")).unwrap_or_else(|_| p::sv(""));
                return Err(self.die_with("timeout", &hint_of(&h), None));
            }
        }
        let err = attr_get(&d, "error").map_err(|e| inside(line::CALL_ERROR, e))?;
        if let Some(e) = err.filter(py::truthy) {
            let perm = attr_get(&d, "permission").ok().flatten();
            return Err(within(match self.die(&e, None, perm.as_ref()) {
                End::Raise(mut t) => {
                    t.frames.insert(0, (line::CALL_DIE_ERROR, "call"));
                    End::Raise(t)
                }
                other => other,
            }));
        }
        let ok = attr_get(&d, "ok").map_err(|e| inside(line::CALL_OK, e))?;
        if !p::truthy(ok.as_ref()) {
            let t: String = dumps(&d).chars().take(200).collect();
            return Err(self.die_with("error", &format!("unexpected daemon response: {t}"), None));
        }
        Ok(d)
    }

    /// `call(method, route, params=params)` (timeout 30), from line `from`.
    pub fn get(&self, route: &str, params: &[(String, String)], from: u32) -> R<Value> {
        self.call_full(Method::Get, route, params, None, Secs::Int(30), None, from)
    }

    /// `call("POST", route, body=body)` (timeout 30), from line `from`.
    pub fn post(&self, route: &str, body: &Value, from: u32) -> R<Value> {
        self.call_full(
            Method::Post,
            route,
            &[],
            Some(body),
            Secs::Int(30),
            None,
            from,
        )
    }

    /// `{"project": ws_path}` as the query.
    pub fn project(&self) -> Vec<(String, String)> {
        vec![("project".to_string(), self.ws.clone())]
    }
}

/// The printers `_mail_py`'s verbs share. Each returns the exception's
/// text with its own frame; the caller adds its line.
pub mod print {
    use super::*;

    fn at<T>(r: Result<T, String>, line: u32, name: &'static str) -> R<T> {
        r.at(line, name)
    }

    /// `fmt_addr(a)`.
    pub fn fmt_addr(a: Option<&Value>) -> R<String> {
        let Some(a) = a.filter(|a| py::truthy(a)) else {
            return Ok("?".into());
        };
        let name = at(attr_get(a, "name"), line::FMT_ADDR, "fmt_addr")?;
        let email = at(attr_get(a, "address"), line::FMT_ADDR_EMAIL, "fmt_addr")?;
        let email = p::or(email, p::sv("?"));
        Ok(match name.filter(py::truthy) {
            Some(n) => format!("{} <{}>", py::str_of(&n), py::str_of(&email)),
            None => py::str_of(&email),
        })
    }

    /// `fmt_addrs(lst)`.
    pub fn fmt_addrs(lst: Option<&Value>) -> R<String> {
        let lst = p::or(lst.cloned(), Value::Array(Vec::new()));
        let mut parts = Vec::new();
        for a in at(iter(&lst), line::FMT_ADDRS, "fmt_addrs")? {
            parts.push(fmt_addr(Some(&a)).within(line::FMT_ADDRS, "<genexpr>")?);
        }
        Ok(parts.join(", "))
    }

    /// `cap_str(cap)`.
    pub fn cap_str(cap: Option<&Value>) -> R<String> {
        let c = p::or(cap.cloned(), Value::Object(Vec::new()));
        let used = at(p::get_or(&c, "used", p::sv("?")), line::CAP_USED, "cap_str")?;
        let limit = at(
            p::get_or(&c, "cap", Value::Int("0".into())),
            line::CAP_LIMIT,
            "cap_str",
        )?;
        Ok(if !py::truthy(&limit) {
            format!("cap {} used (unlimited)", py::str_of(&used))
        } else {
            format!("cap {}/{} used", py::str_of(&used), py::str_of(&limit))
        })
    }

    /// `print_full(m)`: the `read` format.
    pub fn full(py_: &mut Py, m: &Value) -> R<()> {
        const F: &str = "print_full";
        let g = |k: &str, l: u32| at(attr_get(m, k), l, F);
        let id = at(p::get_or(m, "id", p::sv("")), line::FULL_ID, F)?;
        py_.print(&format!("message  : {}", py::str_of(&id)));
        let a = at(p::get_or(m, "address", p::sv("?")), line::FULL_ADDRESS, F)?;
        py_.print(&format!("mailbox  : {}", py::str_of(&a)));
        let from = fmt_addr(g("from", line::FULL_FROM)?.as_ref()).within(line::FULL_FROM, F)?;
        py_.print(&format!("from     : {from}"));
        let to = fmt_addrs(g("to", line::FULL_TO)?.as_ref()).within(line::FULL_TO, F)?;
        py_.print(&format!(
            "to       : {}",
            if to.is_empty() { "?" } else { to.as_str() }
        ));
        if p::truthy(g("cc", line::FULL_CC_IF)?.as_ref()) {
            let cc = fmt_addrs(g("cc", line::FULL_CC)?.as_ref()).within(line::FULL_CC, F)?;
            py_.print(&format!("cc       : {cc}"));
        }
        let subject = p::or(g("subject", line::FULL_SUBJECT)?, p::sv("(no subject)"));
        py_.print(&format!("subject  : {}", py::str_of(&subject)));
        let date = g("date", line::FULL_DATE)?;
        let shown = p::or(date.clone(), p::sv("?"));
        py_.print(&format!(
            "date     : {}  ({})",
            py::str_of(&shown),
            iso_ago(date.as_ref())
        ));
        let ar = p::or(
            g("authResults", line::FULL_AUTH)?,
            Value::Object(Vec::new()),
        );
        let keys = ["spf", "dkim", "dmarc"];
        let mut any = false;
        for k in keys {
            if p::truthy(
                at(attr_get(&ar, k), line::FULL_ANY, "<genexpr>")
                    .within(line::FULL_ANY, F)?
                    .as_ref(),
            ) {
                any = true;
                break;
            }
        }
        if any {
            let mut parts = Vec::new();
            for k in keys {
                // `ar` is a dict here: `any` read it.
                let v = p::or(attr_get(&ar, k).unwrap_or(None), p::sv("?"));
                parts.push(format!("{k}={}", py::str_of(&v)));
            }
            py_.print(&format!("auth     : {}", parts.join("  ")));
        }
        let atts = p::or(g("attachments", line::FULL_ATTS)?, Value::Array(Vec::new()));
        if py::truthy(&atts) {
            py_.print("attachments:");
            for a in at(iter(&atts), line::FULL_FOR, F)? {
                py_.print(&at(att_line(&a), line::FULL_ATT, F)?);
            }
            let id = at(p::get_or(m, "id", p::sv("")), line::FULL_FETCH, F)?;
            py_.print(&format!(
                "  fetch one: {} mail attachments {} --get <n> --out <path>",
                cli_name!(),
                py::str_of(&id)
            ));
        }
        py_.print("");
        let t = p::or(g("text", line::FULL_TEXT)?, p::sv(""));
        py_.print_v(&t);
        if p::truthy(g("html", line::FULL_HTML_IF)?.as_ref()) {
            py_.print("");
            let h = g("html", line::FULL_HTML)?.unwrap_or(Value::Null);
            py_.print_v(&h);
        }
        Ok(())
    }

    /// `"  %s. %s (%s, %s bytes)" % (a.get("index"), …)`: one attachment.
    pub fn att_line(a: &Value) -> Result<String, String> {
        let index = attr_get(a, "index")?.unwrap_or(Value::Null);
        let name = p::or(attr_get(a, "filename")?, p::sv("(unnamed)"));
        let mime = p::or(attr_get(a, "mime")?, p::sv("?"));
        let size = attr_get(a, "size")?.unwrap_or(Value::Null);
        Ok(format!(
            "  {}. {} ({}, {} bytes)",
            py::str_of(&index),
            py::str_of(&name),
            py::str_of(&mime),
            py::str_of(&size)
        ))
    }

    /// `forward_line(f)`.
    pub fn forward_line(f: &Value) -> R<String> {
        const F: &str = "forward_line";
        let mailbox = p::or(
            at(attr_get(f, "mailbox"), line::FWD_MAILBOX, F)?,
            p::sv("?"),
        );
        let sender = p::or(at(attr_get(f, "sender"), line::FWD_MAILBOX, F)?, p::sv("?"));
        let eml = at(attr_get(f, "asEml"), line::FWD_EML, F)?;
        Ok(format!(
            "a message in {} from {} (unverified){}",
            term_line(&mailbox),
            term_line(&sender),
            if p::truthy(eml.as_ref()) {
                ", whole, as an .eml file"
            } else {
                ""
            }
        ))
    }

    /// `", ".join(r[key])`, raised at `line` of `name`.
    fn joined(r: &Value, key: &str, line: u32, name: &'static str) -> R<String> {
        let v = at(p::py_index(r, key), line, name)?;
        at(join(", ", &v), line, name)
    }

    /// `"%d (%s)" % (att["count"], ", ".join(att.get("names") or []))`.
    fn att_count(att: &Value, line: u32, name: &'static str) -> R<String> {
        let count = at(p::py_index(att, "count"), line, name)?;
        let n = at(py::percent_d(&count), line, name)?;
        let names = p::or(
            at(attr_get(att, "names"), line, name)?,
            Value::Array(Vec::new()),
        );
        Ok(format!("{n} ({})", at(join(", ", &names), line, name)?))
    }

    /// `print_outbound_row(r)`.
    pub fn outbound_row(py_: &mut Py, r: &Value) -> R<()> {
        const F: &str = "print_outbound_row";
        let g = |k: &str, l: u32| at(attr_get(r, k), l, F);
        let id = at(p::get_or(r, "id", p::sv("")), line::ROW_FIRST, F)?;
        let status = at(p::get_or(r, "status", p::sv("?")), line::ROW_FIRST, F)?;
        let ago = unix_ago(g("createdAt", line::ROW_FIRST)?.as_ref());
        let to = p::or(g("to", line::ROW_FIRST)?, Value::Array(Vec::new()));
        let to = at(join(", ", &to), line::ROW_FIRST, F)?;
        let subject = p::or(g("subject", line::ROW_FIRST)?, p::sv("(no subject)"));
        py_.print(&format!(
            "{}  {} {} to {} — {}",
            py::str_of(&id),
            p::ljust(&py::str_of(&status), 17),
            p::ljust(&ago, 11),
            if to.is_empty() { "?" } else { to.as_str() },
            py::str_of(&subject)
        ));
        if p::truthy(g("bcc", line::ROW_BCC_IF)?.as_ref()) {
            let j = joined(r, "bcc", line::ROW_BCC, F)?;
            py_.print(&format!("    bcc: {j}"));
        }
        if p::truthy(g("policyBcc", line::ROW_POLICY_IF)?.as_ref()) {
            let j = joined(r, "policyBcc", line::ROW_POLICY, F)?;
            py_.print(&format!("    bcc (owner policy): {j}"));
        }
        if p::truthy(g("replyTo", line::ROW_REPLY_IF)?.as_ref()) {
            let j = joined(r, "replyTo", line::ROW_REPLY, F)?;
            py_.print(&format!("    reply-to: {j}"));
        }
        if p::truthy(g("forwardOf", line::ROW_FWD_IF)?.as_ref()) {
            let f = at(p::py_index(r, "forwardOf"), line::ROW_FWD, F)?;
            let l = forward_line(&f).within(line::ROW_FWD, F)?;
            py_.print(&format!("    forwards: {l}"));
        }
        let att = p::or(g("attachments", line::ROW_ATT)?, Value::Object(Vec::new()));
        if p::truthy(at(attr_get(&att, "count"), line::ROW_ATT_IF, F)?.as_ref()) {
            let l = att_count(&att, line::ROW_ATT_LINE, F)?;
            py_.print(&format!("    attachments: {l}"));
        }
        if p::truthy(g("note", line::ROW_NOTE_IF)?.as_ref()) {
            let n = at(p::py_index(r, "note"), line::ROW_NOTE, F)?;
            py_.print(&format!("    note from your human: {}", py::str_of(&n)));
        }
        Ok(())
    }

    /// `print_outbound_detail(r)`.
    pub fn outbound_detail(py_: &mut Py, r: &Value) -> R<()> {
        const F: &str = "print_outbound_detail";
        let g = |k: &str, l: u32| at(attr_get(r, k), l, F);
        let id = at(p::get_or(r, "id", p::sv("")), line::DETAIL_ID, F)?;
        py_.print(&format!("outbound : {}", py::str_of(&id)));
        let status = at(p::get_or(r, "status", p::sv("?")), line::DETAIL_STATUS, F)?;
        let note = at(
            p::get_or(r, "statusNote", p::sv("")),
            line::DETAIL_STATUS,
            F,
        )?;
        py_.print(&format!(
            "status   : {} — {}",
            py::str_of(&status),
            py::str_of(&note)
        ));
        let from = p::or(g("from", line::DETAIL_FROM)?, p::sv("?"));
        py_.print(&format!("from     : {}", py::str_of(&from)));
        let to = p::or(g("to", line::DETAIL_TO)?, Value::Array(Vec::new()));
        let to = at(join(", ", &to), line::DETAIL_TO, F)?;
        py_.print(&format!(
            "to       : {}",
            if to.is_empty() { "?" } else { to.as_str() }
        ));
        for (key, label, l_if, l) in [
            ("cc", "cc       : ", line::DETAIL_CC_IF, line::DETAIL_CC),
            ("bcc", "bcc      : ", line::DETAIL_BCC_IF, line::DETAIL_BCC),
            (
                "policyBcc",
                "bcc (owner policy): ",
                line::DETAIL_POLICY_IF,
                line::DETAIL_POLICY,
            ),
            (
                "replyTo",
                "reply-to : ",
                line::DETAIL_REPLY_IF,
                line::DETAIL_REPLY,
            ),
        ] {
            if p::truthy(g(key, l_if)?.as_ref()) {
                let j = joined(r, key, l, F)?;
                py_.print(&format!("{label}{j}"));
            }
        }
        if p::truthy(g("forwardOf", line::DETAIL_FWD_IF)?.as_ref()) {
            let f = at(p::py_index(r, "forwardOf"), line::DETAIL_FWD, F)?;
            let l = forward_line(&f).within(line::DETAIL_FWD, F)?;
            py_.print(&format!("forwards : {l}"));
        }
        let subject = p::or(g("subject", line::DETAIL_SUBJECT)?, p::sv("(no subject)"));
        py_.print(&format!("subject  : {}", py::str_of(&subject)));
        let att = p::or(
            g("attachments", line::DETAIL_ATT)?,
            Value::Object(Vec::new()),
        );
        if p::truthy(at(attr_get(&att, "count"), line::DETAIL_ATT_IF, F)?.as_ref()) {
            let l = att_count(&att, line::DETAIL_ATT_LINE, F)?;
            py_.print(&format!("attach   : {l}"));
        }
        let queued = unix_ago(g("createdAt", line::DETAIL_QUEUED)?.as_ref());
        py_.print(&format!("queued   : {queued}"));
        if p::truthy(g("decidedAt", line::DETAIL_DECIDED_IF)?.as_ref()) {
            let by = if p::truthy(g("decidedBy", line::DETAIL_BY)?.as_ref()) {
                let b = at(p::py_index(r, "decidedBy"), line::DETAIL_BY, F)?;
                format!(" by {}", py::str_of(&b))
            } else {
                String::new()
            };
            let ago = unix_ago(g("decidedAt", line::DETAIL_DECIDED)?.as_ref());
            py_.print(&format!("decided  : {ago}{by}"));
        }
        if p::truthy(g("sentAt", line::DETAIL_SENT_IF)?.as_ref()) {
            let ago = unix_ago(g("sentAt", line::DETAIL_SENT)?.as_ref());
            py_.print(&format!("submitted: {ago}"));
        }
        if p::truthy(g("note", line::DETAIL_NOTE_IF)?.as_ref()) {
            let n = at(p::py_index(r, "note"), line::DETAIL_NOTE, F)?;
            py_.print(&format!("note     : {}", py::str_of(&n)));
        }
        Ok(())
    }

    /// `DOMAIN_MARKS`.
    pub const DOMAIN_MARKS: &[(&str, &str)] = &[
        ("valid", "valid"),
        ("missing", "MISSING"),
        ("wrong", "WRONG"),
        ("pending", "pending"),
        ("unverifiable", "manual"),
        ("optional", "optional"),
        ("unknown", "?"),
        ("skip", "skip for now"),
    ];

    /// `print_domain(d)`.
    pub fn domain(py_: &mut Py, d: &Value) -> R<()> {
        const F: &str = "print_domain";
        let name = at(p::get_or(d, "domain", p::sv("?")), line::DOMAIN_FIRST, F)?;
        let status = at(p::get_or(d, "status", p::sv("?")), line::DOMAIN_FIRST, F)?;
        py_.print(&format!(
            "domain  : {}  ({})",
            py::str_of(&name),
            py::str_of(&status)
        ));
        let send = at(
            p::get_or(d, "sendMode", p::sv("?")),
            line::DOMAIN_FIRST + 1,
            F,
        )?;
        py_.print(&format!("send    : {}", py::str_of(&send)));
        if p::truthy(at(attr_get(d, "dmarcNag"), line::DOMAIN_FIRST + 2, F)?.as_ref()) {
            py_.print(
                "dmarc   : strongly recommended, not yet valid (starts at p=none — observe first)",
            );
        }
        py_.print("");
        py_.print("DNS records (set these at your registrar):");
        let records = p::or(
            at(attr_get(d, "records"), line::DOMAIN_FOR, F)?,
            Value::Array(Vec::new()),
        );
        for r in at(iter(&records), line::DOMAIN_FOR, F)? {
            let st = at(p::get_or(&r, "status", p::sv("?")), line::DOMAIN_FOR + 1, F)?;
            let mark = match at(p::lookup(DOMAIN_MARKS, &st), line::DOMAIN_FOR + 2, F)? {
                Some(m) => m,
                None => py::str_of(&st),
            };
            let l = line::DOMAIN_FOR + 2;
            let ty = at(p::get_or(&r, "type", p::sv("")), l, F)?;
            let nm = at(p::get_or(&r, "name", p::sv("")), l, F)?;
            let purpose = at(p::get_or(&r, "purpose", p::sv("")), l, F)?;
            py_.print(&format!(
                "  [{mark}] {} {} — {}",
                py::str_of(&ty),
                py::str_of(&nm),
                py::str_of(&purpose)
            ));
            let exp = at(
                p::get_or(&r, "expected", p::sv("")),
                line::DOMAIN_FOR + 4,
                F,
            )?;
            py_.print(&format!("      value: {}", py::str_of(&exp)));
            if p::truthy(at(attr_get(&r, "live"), line::DOMAIN_FOR + 5, F)?.as_ref()) {
                let l = line::DOMAIN_FOR + 8;
                let live = at(p::py_index(&r, "live"), l, F)?;
                py_.print(&format!("      live : {}", at(join(" | ", &live), l, F)?));
            }
        }
        py_.print("");
        if p::truthy(at(attr_get(d, "note"), line::DOMAIN_NOTE, F)?.as_ref()) {
            let n = at(p::py_index(d, "note"), line::DOMAIN_NOTE + 1, F)?;
            py_.print(&format!("note: {}", py::str_of(&n)));
        }
        let name = at(p::get_or(d, "domain", p::sv("")), line::DOMAIN_NOTE + 2, F)?;
        py_.print(&format!(
            "re-check now: {} hostmail domain check {}",
            cli_name!(),
            py::str_of(&name)
        ));
        Ok(())
    }

    /// `_level_word(lvl)`.
    fn level_word(lvl: Option<&Value>) -> Result<String, String> {
        let v = lvl.cloned().unwrap_or(Value::Null);
        let table = [("read", "read"), ("draft", "draft"), ("send", "send")];
        Ok(match p::lookup(&table, &v)? {
            Some(w) => w,
            None => py::str_of(&p::or(lvl.cloned(), p::sv("?"))),
        })
    }

    /// `_send_word(state)`.
    fn send_word(state: Option<&Value>) -> Result<String, String> {
        let key = p::or(state.cloned(), p::sv(""));
        let table = [
            ("on", "yes"),
            ("approval", "approval"),
            ("blocked_off", "BLOCKED"),
            ("blocked_approval", "BLOCKED"),
            ("no_send_level", "-"),
        ];
        Ok(p::lookup(&table, &key)?.unwrap_or_else(|| "?".into()))
    }

    fn is_blocked(st: Option<&Value>) -> bool {
        st.is_some_and(|v| p::eq_str(v, "blocked_off") || p::eq_str(v, "blocked_approval"))
    }

    /// `print_inboxes(rows, agent_view)`.
    pub fn inboxes(py_: &mut Py, rows: &Value, agent_view: bool) -> R<()> {
        const F: &str = "print_inboxes";
        let cli = cli_name!();
        if !py::truthy(rows) {
            py_.print(&if agent_view {
                format!("No inboxes you can access — mint a hosted address ({cli} hostmail create) or ask your human to link/grant one.")
            } else {
                format!("No inboxes yet — mint one ({cli} hostmail create) or link one ({cli} mail link add).")
            });
            return Ok(());
        }
        let list = at(iter(rows), line::INBOXES_W, F)?;
        let mut w = 0usize;
        for r in &list {
            let a = at(
                p::get_or(r, "address", p::sv("")),
                line::INBOXES_W,
                "<genexpr>",
            )
            .within(line::INBOXES_W, F)?;
            w = w.max(at(p::len(&a), line::INBOXES_W, "<genexpr>").within(line::INBOXES_W, F)?);
        }
        let w = w.max("ADDRESS".len()) + 2;
        let mut blocked: Vec<(Value, Value)> = Vec::new();
        if agent_view {
            py_.print(&format!(
                "{}{}{}{}{}STATUS",
                p::ljust("ADDRESS", w),
                p::ljust("SOURCE", 9),
                p::ljust("LEVEL", 7),
                p::ljust("SEND", 10),
                p::ljust("MAX", 7)
            ));
            for r in &list {
                let st = at(attr_get(r, "yourSendState"), line::INBOXES_ST, F)?;
                if is_blocked(st.as_ref()) {
                    blocked.push((st.clone().unwrap_or(Value::Null), Value::Null));
                }
                let l = line::INBOXES_ROW;
                let a = at(p::get_or(r, "address", p::sv("")), l, F)?;
                let src = at(p::get_or(r, "source", p::sv("?")), l, F)?;
                let lvl = at(attr_get(r, "yourLevel"), l, F)?;
                let lvl = at(level_word(lvl.as_ref()), l, "_level_word")?;
                let send = at(send_word(st.as_ref()), l, "_send_word")?;
                let max = at(p::get_or(r, "maxLevel", p::sv("?")), l, F)?;
                let status = at(p::get_or(r, "status", p::sv("?")), l, F)?;
                py_.print(&format!(
                    "{}{}{}{}{}{}",
                    p::ljust(&py::str_of(&a), w),
                    p::ljust(&py::str_of(&src), 9),
                    p::ljust(&lvl, 7),
                    p::ljust(&send, 10),
                    p::ljust(&py::str_of(&max), 7),
                    py::str_of(&status)
                ));
            }
            py_.print("");
            py_.print(&format!("read: {cli} mail messages [<address>]   send needs level 'send' (hosted, or a linked inbox with SMTP)"));
            py_.print("MAX is the CEILING, not a promise — sending also needs a send transport configured.");
            if !blocked.is_empty() {
                let approval = blocked
                    .iter()
                    .any(|(st, _)| p::eq_str(st, "blocked_approval"));
                py_.print("");
                if !approval {
                    py_.print(&format!("SEND BLOCKED: this workspace's agent sending is off, so level 'send' can't send yet ({cli} mail send/reply answer 'gated')."));
                } else {
                    py_.print("SEND BLOCKED: this workspace's agent sending is 'approval', which only queues hosted mailboxes — a linked inbox needs 'on'.");
                }
                py_.print(&format!("Ask your human to run: {cli} mail sending <this workspace> on   (or Settings → Workspaces / Agents → <workspace> → Permissions → Email sending)"));
            }
            return Ok(());
        }
        py_.print(&format!(
            "{}{}{}GRANTS",
            p::ljust("ADDRESS", w),
            p::ljust("SOURCE", 9),
            p::ljust("PRIMARY", 22)
        ));
        for r in &list {
            let pri = p::or(
                at(attr_get(r, "primary"), line::INBOXES_PRI, F)?,
                Value::Object(Vec::new()),
            );
            let who = match at(attr_get(&pri, "workspace"), line::INBOXES_PRI + 1, F)?
                .filter(py::truthy)
            {
                Some(v) => v,
                None => p::or(
                    at(attr_get(&pri, "projectId"), line::INBOXES_PRI + 1, F)?,
                    p::sv("?"),
                ),
            };
            let grants = p::or(
                at(attr_get(r, "grants"), line::INBOXES_PRI + 2, F)?,
                Value::Array(Vec::new()),
            );
            // `_lvl(e)`: the level, and the grant's workspace when its
            // sending is blocked.
            let lvl = |e: &Value, blocked: &mut Vec<(Value, Value)>| -> R<String> {
                const L: &str = "_lvl";
                let st = at(attr_get(e, "sendState"), line::INBOXES_LVL, L)?;
                if is_blocked(st.as_ref()) {
                    let ws = match at(attr_get(e, "workspace"), line::INBOXES_LVL + 3, L)?
                        .filter(py::truthy)
                    {
                        Some(v) => v,
                        None => at(
                            p::get_or(e, "projectId", p::sv("?")),
                            line::INBOXES_LVL + 3,
                            L,
                        )?,
                    };
                    blocked.push((st.unwrap_or(Value::Null), ws));
                    let level = at(p::get_or(e, "level", p::sv("?")), line::INBOXES_LVL + 4, L)?;
                    return Ok(format!("{}, BLOCKED", py::str_of(&level)));
                }
                let level = at(p::get_or(e, "level", p::sv("?")), line::INBOXES_LVL + 5, L)?;
                Ok(py::str_of(&level))
            };
            let mut parts = Vec::new();
            for g in at(iter(&grants), line::INBOXES_GSUM, F)? {
                let l = line::INBOXES_GSUM;
                let gw = match at(attr_get(&g, "workspace"), l, "<genexpr>")
                    .within(l, F)?
                    .filter(py::truthy)
                {
                    Some(v) => v,
                    None => {
                        at(p::get_or(&g, "projectId", p::sv("?")), l, "<genexpr>").within(l, F)?
                    }
                };
                let lv = lvl(&g, &mut blocked).within(l, "<genexpr>").within(l, F)?;
                parts.push(format!("{}:{lv}", py::str_of(&gw)));
            }
            let gsum = if parts.is_empty() {
                "-".to_string()
            } else {
                parts.join(", ")
            };
            let l = line::INBOXES_OWNER_ROW;
            let a = at(p::get_or(r, "address", p::sv("")), l, F)?;
            let src = at(p::get_or(r, "source", p::sv("?")), l, F)?;
            let plv = lvl(&pri, &mut blocked).within(l, F)?;
            py_.print(&format!(
                "{}{}{}{gsum}",
                p::ljust(&py::str_of(&a), w),
                p::ljust(&py::str_of(&src), 9),
                p::ljust(&format!("{} ({plv})", py::str_of(&who)), 22)
            ));
        }
        py_.print("");
        py_.print(&format!(
            "share: {cli} mail access grant <address> <workspace> --level read|draft|send"
        ));
        if !blocked.is_empty() {
            py_.print("");
            py_.print("BLOCKED = level 'send', but that workspace's agent sending is off (or 'approval' on a linked inbox), so its sends are gated. Turn it on:");
            let names = at(
                sorted_set(blocked.into_iter().map(|(_, ws)| ws).collect()),
                line::INBOXES_SORTED,
                F,
            )?;
            for ws in names {
                py_.print(&format!("  {cli} mail sending {} on", py::str_of(&ws)));
            }
        }
        Ok(())
    }

    /// `sorted(set(values))`, for the strings and numbers a workspace
    /// name can be; the exception's text for what Python can't hash or
    /// order.
    fn sorted_set(values: Vec<Value>) -> Result<Vec<Value>, String> {
        let mut uniq: Vec<Value> = Vec::new();
        for v in values {
            match &v {
                Value::Array(_) => return Err("TypeError: unhashable type: 'list'".into()),
                Value::Object(_) => return Err("TypeError: unhashable type: 'dict'".into()),
                _ => {}
            }
            if !uniq.contains(&v) {
                uniq.push(v);
            }
        }
        if uniq.len() < 2 {
            return Ok(uniq);
        }
        if uniq
            .iter()
            .all(|v| matches!(v, Value::Int(_) | Value::Float(_) | Value::Bool(_)))
        {
            let num = |v: &Value| match v {
                Value::Int(d) => d.parse::<f64>().unwrap_or(0.0),
                Value::Float(f) => *f,
                Value::Bool(b) => f64::from(u8::from(*b)),
                _ => 0.0,
            };
            uniq.sort_by(|a, b| num(a).total_cmp(&num(b)));
            return Ok(uniq);
        }
        if uniq.iter().all(|v| matches!(v, Value::Str(_))) {
            uniq.sort_by_key(|v| match v {
                Value::Str(t) => t.code_points(),
                _ => Vec::new(),
            });
            return Ok(uniq);
        }
        let first = py::type_name(&uniq[0]);
        let other = uniq
            .iter()
            .map(py::type_name)
            .find(|t| *t != first)
            .unwrap_or(first);
        Err(format!(
            "TypeError: '<' not supported between instances of '{other}' and '{first}'"
        ))
    }

    /// `term_safe(s)`: line breaks and tabs kept, any other control
    /// character a `?`.
    pub fn term_safe(v: &Value) -> String {
        py::str_of(v)
            .chars()
            .map(|c| {
                if c == '\n'
                    || c == '\t'
                    || (c >= ' ' && c != '\x7f' && !('\u{80}'..='\u{9f}').contains(&c))
                {
                    c
                } else {
                    '?'
                }
            })
            .collect()
    }

    /// `term_line(s)`.
    pub fn term_line(v: &Value) -> String {
        term_safe(v).replace(['\n', '\t'], " ")
    }
}

/// Seconds since the epoch, as `time.time()` reads them.
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// `int(ts)` as a float, or `None` where Python's `int()` raises (or the
/// number is past a float, where the subtraction raises).
fn int_secs(ts: Option<&Value>) -> Option<f64> {
    let digits = p::py_int(ts?).ok()?;
    let f: f64 = digits.parse().ok()?;
    f.is_finite().then_some(f)
}

/// `unix_ago(ts)`.
pub fn unix_ago(ts: Option<&Value>) -> String {
    let Some(t) = int_secs(ts) else {
        return "?".into();
    };
    ago_from((now() - t).trunc())
}

fn ago_from(s: f64) -> String {
    let s = s.max(0.0) as i128;
    if s < 60 {
        "just now".into()
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 172_800 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86400)
    }
}

/// `until(ts)`.
pub fn until(ts: Option<&Value>) -> String {
    let Some(t) = int_secs(ts) else {
        return "?".into();
    };
    let s = (t - now()).trunc() as i128;
    if s <= 0 {
        "expired".into()
    } else if s < 3600 {
        format!("in {}m", s / 60)
    } else if s < 172_800 {
        format!("in {}h", s / 3600)
    } else {
        format!("in {}d", s / 86400)
    }
}

/// `iso_ago(s)`.
pub fn iso_ago(s: Option<&Value>) -> String {
    let Some(v) = s.filter(|v| py::truthy(v)) else {
        return "?".into();
    };
    let t = py::str_of(v).replace('Z', "+00:00");
    match iso::timestamp(&t) {
        Some(ts) => ago_from((now() - ts.trunc()).trunc()),
        None => py::str_of(v),
    }
}

/// `datetime.fromisoformat(text).timestamp()`, for the forms the module
/// notes name.
pub mod iso {
    fn num(s: &str) -> Option<i64> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    }

    fn days_in(y: i64, m: i64) -> i64 {
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
            _ => 28,
        }
    }

    /// Days from 1970-01-01 to the date.
    fn days(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    fn date(s: &str) -> Option<(i64, i64, i64)> {
        let (y, m, d) = match s.len() {
            10 if s.as_bytes()[4] == b'-' && s.as_bytes()[7] == b'-' => {
                (num(&s[..4])?, num(&s[5..7])?, num(&s[8..])?)
            }
            8 => (num(&s[..4])?, num(&s[4..6])?, num(&s[6..])?),
            _ => return None,
        };
        ((1..=9999).contains(&y) && (1..=12).contains(&m) && (1..=days_in(y, m)).contains(&d))
            .then_some((y, m, d))
    }

    /// `HH[:MM[:SS[.f…]]]` or `HH[MM[SS[.f…]]]`: seconds into the day.
    fn time(s: &str) -> Option<f64> {
        let (whole, frac) = match s.find(['.', ',']) {
            Some(i) => (&s[..i], Some(&s[i + 1..])),
            None => (s, None),
        };
        let parts: Vec<&str> = if whole.contains(':') {
            whole.split(':').collect()
        } else {
            whole
                .as_bytes()
                .chunks(2)
                .map(|c| std::str::from_utf8(c).unwrap_or(""))
                .collect()
        };
        if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.len() != 2) {
            return None;
        }
        if frac.is_some() && parts.len() != 3 {
            return None;
        }
        let h = num(parts[0])?;
        let m = parts.get(1).map_or(Some(0), |p| num(p))?;
        let sec = parts.get(2).map_or(Some(0), |p| num(p))?;
        if h > 23 || m > 59 || sec > 59 {
            return None;
        }
        let mut f = 0.0;
        if let Some(fr) = frac {
            if fr.is_empty() || !fr.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let six: String = fr.chars().take(6).collect();
            f = format!("0.{six}").parse().ok()?;
        }
        Some((h * 3600 + m * 60 + sec) as f64 + f)
    }

    /// `+HH:MM[:SS]`, `+HHMM`, `+HH`: seconds east of UTC.
    fn offset(s: &str) -> Option<i64> {
        let (sign, rest) = match s.as_bytes().first()? {
            b'+' => (1, &s[1..]),
            b'-' => (-1, &s[1..]),
            _ => return None,
        };
        let t = time(rest)?;
        (t.fract() == 0.0).then_some(sign * t as i64)
    }

    /// The timestamp, or `None` where this reads no date.
    pub fn timestamp(text: &str) -> Option<f64> {
        if !text.is_ascii() {
            return None;
        }
        let dlen = if text.len() >= 10 && text.as_bytes()[4] == b'-' {
            10
        } else {
            8.min(text.len())
        };
        let (y, m, d) = date(&text[..dlen])?;
        let rest = &text[dlen..];
        let day = days(y, m, d) as f64 * 86400.0;
        if rest.is_empty() {
            return Some(local(day));
        }
        let rest = &rest[1..];
        let cut = rest.find(['+', '-']).unwrap_or(rest.len());
        let secs = time(&rest[..cut])?;
        if cut == rest.len() {
            return Some(local(day + secs));
        }
        let off = offset(&rest[cut..])?;
        Some(day + secs - off as f64)
    }

    /// A naive time as the local zone reads it (`mktime`).
    #[cfg(unix)]
    fn local(naive: f64) -> f64 {
        let whole = naive.floor();
        let frac = naive - whole;
        let secs = whole as i64;
        let days = secs.div_euclid(86400);
        let tod = secs.rem_euclid(86400);
        // Civil date from days.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        // SAFETY: a zeroed `tm` is a valid value; mktime reads and
        // normalises it.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        tm.tm_year = (y - 1900) as i32;
        tm.tm_mon = (m - 1) as i32;
        tm.tm_mday = d as i32;
        tm.tm_hour = (tod / 3600) as i32;
        tm.tm_min = (tod % 3600 / 60) as i32;
        tm.tm_sec = (tod % 60) as i32;
        tm.tm_isdst = -1;
        // SAFETY: `tm` is a valid, owned value.
        let t = unsafe { libc::mktime(&mut tm) };
        if t == -1 {
            naive
        } else {
            t as f64 + frac
        }
    }

    #[cfg(not(unix))]
    fn local(naive: f64) -> f64 {
        naive
    }
}

/// Runs the verb's Python on `py_` and prints what it printed: stdout,
/// then the traceback if it raised (`die` wrote its own lines). The exit.
pub fn finish(py_: &mut Py, body: impl FnOnce(&mut Py) -> R<()>) -> Exit {
    let end = body(py_);
    if !py_.stdout.is_empty() {
        if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &py_.stdout) {
            return out::after_stdout_error(&e);
        }
    }
    match end {
        Ok(()) => Exit::Ok,
        Err(End::Exit(e)) => e,
        Err(End::Raise(t)) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), t.text().as_bytes());
            Exit::Error
        }
    }
}

/// `k5 mail <verb> …` for a verb that runs `_mail_py`: `args` is the whole
/// command line after `argv[0]`, `parse` reads the words after the verb
/// as its `cmd_mail_*` does, and `exec` is the Python's verb chain.
/// `None`: the shell CLI answers it.
pub fn run(
    args: &[OsString],
    parse: fn(&str, &[&str]) -> Plan,
    exec: fn(&mut Py) -> R<()>,
) -> Option<Exit> {
    let hook_sock = super::super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let verb = words.first().copied()?;
    let plan = parse(verb, &words[1..]);
    if plan == Plan::Shell || !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(args, plan, hook_sock.as_deref(), settings, exec))
}

fn run_with(
    args: &[OsString],
    plan: Plan,
    hook_sock: Option<&Path>,
    settings: Settings,
    exec: fn(&mut Py) -> R<()>,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, super::TOOL) {
        return stop;
    }
    let env = match plan {
        Plan::Help(page) => {
            let text = crate::text::help(page).unwrap_or_default();
            return match out::stdout(text) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            };
        }
        Plan::Usage(hint) => {
            out::stderr_line(&err_line("usage", &hint));
            return Exit::Usage;
        }
        Plan::Shell => unreachable!("handed to the shell in run"),
        Plan::Run(env) => env,
    };
    let port_text = settings.port_text.clone().unwrap_or_default();
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut opener = Opener::from_client(&client);
    opener.netloc = format!("{}:{port_text}", settings.host);
    let mut py_ = Py::new(opener, settings.project.clone(), env);
    finish(&mut py_, exec)
}
