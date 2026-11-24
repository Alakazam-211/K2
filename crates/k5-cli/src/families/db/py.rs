//! `_db_py` (`cli/src/db-store.sh`): the shell's Python for `k2 db` and
//! `k2 store`, read from stdin (`python3 - <<'PYEOF'`). One call through
//! the urllib adapter ([`crate::transport::urllib`], no tries: a plain
//! `urlopen`), two for `clusters`; then the refusal, the `--json` answer
//! or the verb's printer.
//!
//! What it does, in its order:
//!
//! 1. The request: `GET` with the query urlencoded, or `POST` with the
//!    body as compact JSON (`ensure_ascii=False`) and `Content-Type:
//!    application/json` after the `Authorization` header (`if token:`).
//!    Timeout 60 s. An HTTP error is an answer (`e.code`, `e.read()`);
//!    anything else raised is `die` with `daemon unreachable: <e>`, exit 1.
//! 2. `load`: the body as JSON, `{}` when empty, `{"raw": <body>}` when it
//!    is not JSON.
//! 3. A dict with `ok` exactly `false` and an `error` dict, or a status of
//!    400 or more: the refusal's `permission` object, if any, becomes the
//!    closing line the EXIT trap prints after the error (`note_permission`
//!    writes the note file), then `die`. `die` prints one line of JSON on
//!    stderr: exit 3 for a refusal code, 2 for `usage`, else 1;
//!    `forbidden` and `owner_only` gain "requires owner/admin — ask your
//!    human".
//! 4. `--json`: the answer as compact JSON (a string answer as it is).
//! 5. The verb's lines.
//!
//! Where the Python raises (an answer of a type it doesn't expect), this
//! prints a Python 3.12 traceback for a script read from stdin, after
//! what it printed so far, and exits 1.

use super::super::{get, py_iter, py_left, py_len, s, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::out::pylib::ops::{contains, join, py_eq, round_int};
use crate::transport::urllib::{self, Method, Opener, Outcome, Raised, Request};
use k2_client::{cli_name, product_name};
use std::time::Duration;

/// `urlopen(..., timeout=60)`.
const TIMEOUT: Duration = Duration::from_secs(60);

/// `die`'s codes that are refusals: exit 3.
pub const REFUSED: &[&str] = &[
    "gated",
    "forbidden",
    "owner_only",
    "unknown_major",
    "unsupported_major",
    "preview_major",
    "major_eol",
    "not_upward",
    "cluster_refused",
    "helper_needed",
    "db_upgrade_off",
    "major_mismatch",
    "import_unresolved",
    "import_target_not_empty",
    "import_migrations_folder",
    "roles_conflict",
];

/// The lines of `_db_py`'s script (line 1 is the one after `<<'PYEOF'`)
/// where it calls or raises. `tests` pins them.
pub mod line {
    pub const DIE_IN: u32 = 20;
    pub const DIE_ADD: u32 = 21;
    pub const REQUEST_DIE: u32 = 48;
    pub const REQUEST_HTTP_READ: u32 = 45;
    pub const GRANT_RELATIONS: u32 = 214;
    pub const PUT_JSON: u32 = 251;
    pub const NOTE: u32 = 299;
    pub const DIE_OK_FALSE: u32 = 300;
    pub const NOTE_STATUS: u32 = 302;
    pub const DIE_STATUS_ERROR: u32 = 304;
    pub const DIE_STATUS: u32 = 305;
    pub const STATUS: u32 = 312;
    pub const STATUS_SRC: u32 = 327;
    pub const STATUS_CLUSTERS: u32 = 328;
    pub const STATUS_CLUSTER: u32 = 329;
    pub const STATUS_UPGRADES: u32 = 330;
    pub const STATUS_UPGRADE: u32 = 331;
    pub const STATUS_UPGRADE_CHECK: u32 = 332;
    pub const STATUS_UPGRADE_LATER: u32 = 334;
    pub const STATUS_FIRST: u32 = 336;
    pub const STATUS_STALE: u32 = 338;
    pub const STATUS_STALE_LINE: u32 = 340;
    pub const CLUSTERS: u32 = 342;
    pub const CLUSTERS_HEAD: u32 = 343;
    pub const CLUSTERS_FOR: u32 = 344;
    pub const CLUSTERS_LABEL: u32 = 345;
    pub const CLUSTERS_NAMED_N: u32 = 348;
    pub const CLUSTERS_MAIN_N: u32 = 350;
    pub const CLUSTERS_ROLE: u32 = 352;
    pub const CLUSTERS_ROW: u32 = 356;
    pub const CLUSTERS_HEALTH: u32 = 357;
    pub const CLUSTERS_SIZES: u32 = 364;
    pub const CLUSTERS_ARCHIVER: u32 = 365;
    pub const CLUSTERS_CONNECTIONS: u32 = 366;
    pub const CLUSTERS_WAL: u32 = 368;
    pub const CLUSTERS_LSN: u32 = 369;
    pub const CLUSTERS_LSN_PRINT: u32 = 370;
    pub const CLUSTERS_UPGRADES: u32 = 371;
    pub const CLUSTERS_UPGRADE: u32 = 372;
    pub const DEFAULT_MAJOR: u32 = 374;
    pub const STATE: u32 = 376;
    pub const CREATE: u32 = 378;
    pub const CREATE_KEPT: u32 = 390;
    pub const CREATE_CAP: u32 = 396;
    pub const CREATE_APPS: u32 = 397;
    pub const CREATE_APP: u32 = 398;
    pub const DSN: u32 = 402;
    pub const LIST: u32 = 413;
    pub const LIST_LEN: u32 = 414;
    pub const LIST_FOR: u32 = 415;
    pub const LIST_ROW: u32 = 416;
    pub const MIGRATE: u32 = 418;
    pub const MIGRATE_APPLIED: u32 = 421;
    pub const MIGRATE_NOOP: u32 = 422;
    pub const MIGRATE_NONE: u32 = 425;
    pub const DUMP: u32 = 427;
    pub const RESTORE: u32 = 429;
    pub const DROP: u32 = 433;
    pub const DROP_APPS: u32 = 434;
    pub const DROP_APP: u32 = 435;
    pub const GRANT: u32 = 437;
    pub const GRANT_APPS: u32 = 440;
    pub const GRANT_APP: u32 = 441;
    pub const REVOKE: u32 = 443;
    pub const REVOKE_APPS: u32 = 444;
    pub const REVOKE_APP: u32 = 445;
    pub const BIND: u32 = 447;
    pub const DOCTOR: u32 = 449;
    pub const DOCTOR_CLUSTERS: u32 = 455;
    pub const DOCTOR_CLUSTER: u32 = 457;
    pub const DOCTOR_RAM: u32 = 459;
    pub const DOCTOR_GUCS: u32 = 469;
    pub const DOCTOR_GUC: u32 = 470;
    pub const DOCTOR_HINT: u32 = 471;
    pub const OTHER: u32 = 474;
}

/// The verbs `_db_py` takes (`D_VERB`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Status,
    Enable,
    Disable,
    Uninstall,
    Doctor,
    Create,
    Clusters,
    DefaultMajor,
    List,
    Dsn,
    Migrate,
    Dump,
    Restore,
    Drop,
    Grant,
    Revoke,
    Bind,
    StoreCreate,
    StoreList,
    StorePut,
    StoreGet,
    StoreQuery,
    StoreRm,
    StoreDrop,
}

impl Verb {
    /// The line `request(...)` is called from, for its frame.
    fn call_line(self) -> u32 {
        match self {
            Verb::Status => 99,
            Verb::Enable => 102,
            Verb::Disable => 105,
            Verb::Uninstall => 108,
            Verb::Doctor => 111,
            Verb::Create => 125,
            Verb::Clusters => 128,
            Verb::DefaultMajor => 144,
            Verb::List => 147,
            Verb::Dsn => 163,
            Verb::Migrate => 171,
            Verb::Dump => 179,
            Verb::Restore => 185,
            Verb::Drop => 208,
            Verb::Grant => 224,
            Verb::Revoke => 231,
            Verb::Bind => 237,
            Verb::StoreCreate => 240,
            Verb::StoreList => 243,
            Verb::StorePut => 253,
            Verb::StoreGet => 256,
            Verb::StoreQuery => 262,
            Verb::StoreRm => 265,
            Verb::StoreDrop => 268,
        }
    }
}

/// `clusters`' second call (`GET /cli/db/clusters`).
const CLUSTERS_CALL: u32 = 137;

/// The `D_*` settings the shell hands `_db_py`: the strings as the shell
/// set them (`""` for one it didn't), the `true`/`false` ones as flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub verb: Verb,
    pub json: bool,
    pub id: String,
    pub name: String,
    pub out: String,
    pub file: String,
    pub dir: String,
    pub yes: bool,
    pub health: bool,
    pub doc_id: String,
    /// `D_DOC_JSON`: `store put --json <doc>`.
    pub doc_json: String,
    pub limit: String,
    pub grantee: String,
    pub db: String,
    pub level: String,
    pub manage: bool,
    pub role: String,
    pub access: String,
    /// `D_RELATIONS`, each `--relation` as given.
    pub relations: Vec<String>,
    pub major: String,
    pub reveal: bool,
    pub as_name: String,
    pub test: bool,
    pub actor: String,
    pub reference: bool,
}

impl Ask {
    pub fn new(verb: Verb) -> Ask {
        Ask {
            verb,
            json: false,
            id: String::new(),
            name: String::new(),
            out: String::new(),
            file: String::new(),
            dir: String::new(),
            yes: false,
            health: false,
            doc_id: String::new(),
            doc_json: String::new(),
            limit: String::new(),
            grantee: String::new(),
            db: String::new(),
            level: String::new(),
            manage: false,
            role: String::new(),
            access: String::new(),
            relations: Vec::new(),
            major: String::new(),
            reveal: false,
            as_name: String::new(),
            test: false,
            actor: String::new(),
            reference: false,
        }
    }
}

/// How the script ended early: `sys.exit` (after `die` wrote its line),
/// or an exception.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    Exit(Exit),
    Raise(Raise),
}

type R<T> = Result<T, Stop>;

impl From<Raise> for Stop {
    fn from(r: Raise) -> Stop {
        Stop::Raise(r)
    }
}

/// A traceback as Python 3.12 prints it for a script read from stdin.
pub fn stdin_traceback(r: &Raise) -> String {
    let mut t = String::from("Traceback (most recent call last):\n");
    for (line, name) in &r.frames {
        t.push_str(&format!("  File \"<stdin>\", line {line}, in {name}\n"));
    }
    t.push_str(&r.exception);
    t.push('\n');
    t
}

fn options(compact: bool, ensure_ascii: bool) -> Options {
    Options {
        compact,
        ensure_ascii,
        indent: None,
        sort_keys: false,
    }
}

/// `json.dumps(v, ensure_ascii=False, separators=(",", ":"))`; a lone
/// surrogate as Python's escape would show it.
pub fn dumps(v: &Value) -> String {
    json::to_string(v, &options(true, false))
        .or_else(|_| json::to_string(v, &options(true, true)))
        .unwrap_or_default()
}

/// `json.dumps(v)`: `, ` and `: `, everything past ASCII escaped.
pub fn dumps_ascii(v: &Value) -> String {
    json::to_string(v, &options(false, true)).unwrap_or_default()
}

/// `json.dumps(v, ensure_ascii=False)`: `, ` and `: `.
pub fn dumps_spaced(v: &Value) -> String {
    json::to_string(v, &options(false, false))
        .or_else(|_| json::to_string(v, &options(false, true)))
        .unwrap_or_default()
}

pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `_db_err <code> <hint>`'s line.
pub fn err_line(code: &str, hint: &str) -> String {
    dumps(&obj(vec![(
        "error",
        obj(vec![("code", s(code)), ("hint", s(hint))]),
    )]))
}

/// `v.get(key)` on a value known to be a dict.
pub fn at<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(pairs) => get(pairs, key),
        _ => None,
    }
}

/// `v.get(key)`: the `AttributeError` at `line` for anything but a dict.
pub fn g<'a>(v: &'a Value, key: &str, line: u32) -> Result<Option<&'a Value>, Raise> {
    super::super::py_get(v, key, line)
}

/// `bool(x)` of a `.get`.
pub fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

/// `"%s" % v`.
pub fn st(v: Option<&Value>) -> String {
    v.map_or_else(|| "None".to_string(), py::str_of)
}

/// `x or default`.
pub fn or(v: Option<&Value>, default: Value) -> Value {
    v.filter(|x| py::truthy(x)).cloned().unwrap_or(default)
}

fn empty_list() -> Value {
    Value::Array(Vec::new())
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

pub fn is_false(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Bool(false)))
}

fn is_true(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Bool(true)))
}

/// `v == text`.
pub fn is_str(v: Option<&Value>, text: &str) -> bool {
    matches!(v, Some(Value::Str(t)) if t.as_str() == Some(text))
}

/// `a + b` where `b` is a string: the `TypeError` for anything but a
/// string `a`.
fn concat(a: &Value, b: &str, line: u32) -> Result<String, Raise> {
    match a {
        Value::Str(_) => Ok(format!("{}{b}", py::str_of(a))),
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

/// `"text" + v` where `v` must be a string.
fn concat_after(text: &str, v: &Value, line: u32) -> Result<String, Raise> {
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

/// `"/".join(str(x) for x in v or [])`.
fn join_str(v: Option<&Value>, sep: &str, line: u32) -> Result<String, Raise> {
    let items = py_iter(&or(v, empty_list()), line)?;
    Ok(items.iter().map(py::str_of).collect::<Vec<_>>().join(sep))
}

/// `round(v / 1048576)` printed with `%s`.
fn megabytes(v: &Value, line: u32) -> Result<String, Raise> {
    let n = match v {
        Value::Bool(_) | Value::Int(_) | Value::Float(_) => number(v).unwrap_or(0.0),
        other => {
            return Err(Raise::at(
                line,
                format!(
                    "TypeError: unsupported operand type(s) for /: '{}' and 'int'",
                    py::type_name(other)
                ),
            ))
        }
    };
    round_int(n / 1_048_576.0, line)
}

/// A number as Python compares it (`True` is 1).
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Int(d) => d.parse::<f64>().ok(),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// `a > 0`.
fn above_zero(v: &Value, line: u32) -> Result<bool, Raise> {
    match number(v) {
        Some(n) => Ok(n > 0.0),
        None => Err(Raise::at(
            line,
            format!(
                "TypeError: '>' not supported between instances of '{}' and 'int'",
                py::type_name(v)
            ),
        )),
    }
}

/// `{...}.get(key, "")` for a dict of string keys: the `TypeError` for a
/// key Python can't hash.
fn src_of(key: Option<&Value>, line: u32) -> Result<&'static str, Raise> {
    match key {
        Some(v @ (Value::Array(_) | Value::Object(_))) => Err(Raise::at(
            line,
            format!("TypeError: unhashable type: '{}'", py::type_name(v)),
        )),
        Some(Value::Str(t)) => Ok(match t.as_str() {
            Some("set") => SRC_SET,
            Some("primary") => " (this box's 5432 cluster)",
            Some("new-install") => SRC_NEW,
            _ => "",
        }),
        _ => Ok(""),
    }
}

const SRC_SET: &str = concat!(" (set: ", cli_name!(), " db default-major)");
const SRC_NEW: &str = concat!(" (", product_name!(), "'s new-install default)");

/// The Python run: its settings and what it printed.
pub struct Py<'a> {
    pub opener: &'a Opener,
    /// `D_WS_PATH`: the shell's `PROJECT`.
    pub ws: &'a str,
    pub ask: &'a Ask,
    pub stdout: String,
    pub stderr: String,
    /// What `note_permission` wrote to the note file.
    pub pending_note: Option<String>,
}

impl<'a> Py<'a> {
    pub fn new(opener: &'a Opener, ws: &'a str, ask: &'a Ask) -> Py<'a> {
        Py {
            opener,
            ws,
            ask,
            stdout: String::new(),
            stderr: String::new(),
            pending_note: None,
        }
    }

    /// The whole run, as the shell would show it, and the note file's
    /// text (the EXIT trap prints it after a failure).
    pub fn shown(mut self) -> (Shown, Option<String>) {
        let exit = match self.main() {
            Ok(()) => Exit::Ok,
            Err(Stop::Exit(e)) => e,
            Err(Stop::Raise(r)) => {
                self.stderr.push_str(&stdin_traceback(&r));
                Exit::Error
            }
        };
        (
            Shown {
                stdout: self.stdout,
                stderr: self.stderr,
                exit,
            },
            self.pending_note,
        )
    }

    fn print(&mut self, line: impl AsRef<str>) {
        self.stdout.push_str(line.as_ref());
        self.stdout.push('\n');
    }

    fn eprint(&mut self, line: impl AsRef<str>) {
        self.stderr.push_str(line.as_ref());
        self.stderr.push('\n');
    }

    /// `die(err)`, called from `from` (the frames above it).
    fn die(&mut self, err: &Value, frames: &[(u32, &'static str)]) -> Stop {
        let raise = |line: u32, e: Raise| {
            let mut f = frames.to_vec();
            f.push((line, "die"));
            Stop::Raise(Raise {
                frames: f,
                exception: e.exception,
            })
        };
        // `err.get("code", "error")` and `err.get("hint", "")`: `err` is
        // always a dict here.
        let code = at(err, "code").cloned().unwrap_or_else(|| s("error"));
        let mut hint = at(err, "hint").cloned().unwrap_or_else(|| s(""));
        if is_str(Some(&code), "forbidden") || is_str(Some(&code), "owner_only") {
            let has = match contains(&hint, "ask your human", line::DIE_IN) {
                Ok(b) => b,
                Err(e) => return raise(line::DIE_IN, e),
            };
            if !has {
                let head = if py::truthy(&hint) {
                    match concat(&hint, " — ", line::DIE_ADD) {
                        Ok(t) => t,
                        Err(e) => return raise(line::DIE_ADD, e),
                    }
                } else {
                    String::new()
                };
                hint = s(&format!("{head}requires owner/admin — ask your human"));
            }
        }
        let text = dumps(&obj(vec![(
            "error",
            obj(vec![("code", code.clone()), ("hint", hint)]),
        )]));
        self.eprint(text);
        let exit = if REFUSED.iter().any(|r| is_str(Some(&code), r)) {
            Exit::Refused
        } else if is_str(Some(&code), "usage") {
            Exit::Usage
        } else {
            Exit::Error
        };
        Stop::Exit(exit)
    }

    /// `request(method, route, params, body)` called from `from`: the
    /// status and the body as text.
    fn request(
        &mut self,
        method: Method,
        route: &str,
        params: Vec<(&str, String)>,
        body: Option<Value>,
        from: u32,
    ) -> R<(u16, String)> {
        let mut req = Request::new(method, route, TIMEOUT).query(params);
        if let Some(t) = self.opener.token() {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        if let Some(b) = &body {
            req = req
                .data(dumps(b).into_bytes())
                .header("Content-Type", "application/json");
        }
        let unreachable = |me: &mut Self, r: &Raised| {
            let hint = format!("daemon unreachable: {}", r.text());
            me.die(
                &obj(vec![("code", s("error")), ("hint", s(&hint))]),
                &[(from, "<module>"), (line::REQUEST_DIE, "request")],
            )
        };
        // `_K2NoRedirect`: the rule's line, then `SystemExit(3)`.
        let out = match self.opener.open_ruled(&req) {
            Ok(out) => out,
            Err(r) => {
                self.eprint(r.line);
                return Err(Stop::Exit(urllib::Refused::EXIT));
            }
        };
        match out {
            Outcome::Answer {
                status,
                body: Ok(b),
            } => Ok((status, urllib::decode(&b))),
            Outcome::HttpError { code, body: Ok(b) } => Ok((code, urllib::decode(&b))),
            Outcome::HttpError { body: Err(r), .. } => Err(Stop::Raise(Raise {
                frames: vec![(from, "<module>"), (line::REQUEST_HTTP_READ, "request")],
                exception: r.traceback_line(),
            })),
            Outcome::Answer { body: Err(r), .. } | Outcome::Raised(r) => Err(unreachable(self, &r)),
        }
    }

    fn main(&mut self) -> R<()> {
        let ask = self.ask;
        let ws = s(self.ws);
        let verb = ask.verb;
        let from = verb.call_line();
        let (status, raw, d) = match verb {
            Verb::Status => {
                let mut params = Vec::new();
                if ask.health {
                    params.push(("health", "1".to_string()));
                }
                self.load(Method::Get, "/cli/db/status", params, None, from)?
            }
            Verb::Enable => self.load(
                Method::Post,
                "/cli/db/server/enable",
                vec![],
                Some(empty_dict()),
                from,
            )?,
            Verb::Disable => self.load(
                Method::Post,
                "/cli/db/server/disable",
                vec![],
                Some(empty_dict()),
                from,
            )?,
            Verb::Uninstall => self.load(
                Method::Post,
                "/cli/db/server/uninstall",
                vec![],
                Some(empty_dict()),
                from,
            )?,
            Verb::Doctor => self.load(
                Method::Post,
                "/cli/db/doctor",
                vec![],
                Some(empty_dict()),
                from,
            )?,
            Verb::Create => {
                let mut body = vec![("project", ws.clone())];
                if !ask.id.is_empty() {
                    body.push(("clientId", s(&ask.id)));
                }
                if !ask.name.is_empty() {
                    body.push(("name", s(&ask.name)));
                }
                if !ask.access.is_empty() {
                    body.push(("access", s(&ask.access)));
                }
                if ask.test {
                    body.push(("test", Value::Bool(true)));
                }
                if !ask.major.is_empty() {
                    body.push(("major", int_of(&ask.major)));
                }
                self.load(
                    Method::Post,
                    "/cli/db/create",
                    vec![],
                    Some(obj(body)),
                    from,
                )?
            }
            Verb::Clusters => self.clusters()?,
            Verb::DefaultMajor => self.load(
                Method::Post,
                "/cli/db/server/default-major",
                vec![],
                Some(obj(vec![("major", int_of(&ask.major))])),
                from,
            )?,
            Verb::List => self.load(
                Method::Get,
                "/cli/db/list",
                vec![("project", self.ws.to_string())],
                None,
                from,
            )?,
            Verb::Dsn => {
                let mut params = vec![("project", self.ws.to_string())];
                if ask.test {
                    params.push(("test", "1".to_string()));
                }
                if !ask.actor.is_empty() {
                    params.push(("actor", ask.actor.clone()));
                }
                if ask.reference {
                    params.push(("ref", "1".to_string()));
                }
                if !ask.db.is_empty() {
                    params.push(("db", ask.db.clone()));
                }
                if ask.reveal {
                    params.push(("reveal", "1".to_string()));
                }
                if !ask.as_name.is_empty() {
                    params.push(("as", ask.as_name.clone()));
                }
                self.load(Method::Get, "/cli/db/dsn", params, None, from)?
            }
            Verb::Migrate => {
                let mut body = vec![("project", ws.clone())];
                if !ask.dir.is_empty() {
                    body.push(("dir", s(&ask.dir)));
                }
                if !ask.db.is_empty() {
                    body.push(("db", s(&ask.db)));
                }
                self.load(
                    Method::Post,
                    "/cli/db/migrate",
                    vec![],
                    Some(obj(body)),
                    from,
                )?
            }
            Verb::Dump => {
                let mut body = vec![("project", ws.clone())];
                if !ask.out.is_empty() {
                    body.push(("out", s(&ask.out)));
                }
                if !ask.db.is_empty() {
                    body.push(("db", s(&ask.db)));
                }
                self.load(Method::Post, "/cli/db/dump", vec![], Some(obj(body)), from)?
            }
            Verb::Restore => {
                let mut body = vec![("project", ws.clone()), ("file", s(&ask.file))];
                if !ask.db.is_empty() {
                    body.push(("db", s(&ask.db)));
                }
                self.load(
                    Method::Post,
                    "/cli/db/restore",
                    vec![],
                    Some(obj(body)),
                    from,
                )?
            }
            Verb::Drop => {
                let mut body = vec![("project", ws.clone()), ("yes", Value::Bool(ask.yes))];
                if ask.test {
                    body.push(("test", Value::Bool(true)));
                }
                if !ask.db.is_empty() {
                    body.push(("db", s(&ask.db)));
                }
                self.load(Method::Post, "/cli/db/drop", vec![], Some(obj(body)), from)?
            }
            Verb::Grant => {
                let relations = Value::Array(ask.relations.iter().map(|r| s(r)).collect());
                let body = obj(vec![
                    ("project", s(&ask.grantee)),
                    ("db", s(&ask.db)),
                    ("level", s(&ask.level)),
                    ("manage", Value::Bool(ask.manage)),
                    ("relations", relations),
                ]);
                self.load(Method::Post, "/cli/db/grant", vec![], Some(body), from)?
            }
            Verb::Revoke => {
                let body = obj(vec![("project", s(&ask.grantee)), ("db", s(&ask.db))]);
                self.load(Method::Post, "/cli/db/revoke", vec![], Some(body), from)?
            }
            Verb::Bind => {
                let mut body = vec![("project", ws.clone()), ("role", s(&ask.role))];
                if !ask.db.is_empty() {
                    body.push(("db", s(&ask.db)));
                }
                self.load(Method::Post, "/cli/db/bind", vec![], Some(obj(body)), from)?
            }
            Verb::StoreCreate => {
                let body = obj(vec![("project", ws.clone()), ("name", s(&ask.name))]);
                self.load(Method::Post, "/cli/store/create", vec![], Some(body), from)?
            }
            Verb::StoreList => self.load(
                Method::Get,
                "/cli/store/list",
                vec![("project", self.ws.to_string())],
                None,
                from,
            )?,
            Verb::StorePut => {
                let raw = if ask.doc_json.is_empty() {
                    "{}"
                } else {
                    ask.doc_json.as_str()
                };
                let Ok(doc) = json::parse(raw) else {
                    return Err(self.die(
                        &obj(vec![
                            ("code", s("usage")),
                            ("hint", s("--json body must be JSON")),
                        ]),
                        &[(line::PUT_JSON, "<module>")],
                    ));
                };
                let body = obj(vec![
                    ("project", ws.clone()),
                    ("name", s(&ask.name)),
                    ("id", s(&ask.doc_id)),
                    ("json", doc),
                ]);
                self.load(Method::Post, "/cli/store/put", vec![], Some(body), from)?
            }
            Verb::StoreGet => self.load(
                Method::Get,
                "/cli/store/get",
                vec![
                    ("project", self.ws.to_string()),
                    ("name", ask.name.clone()),
                    ("id", ask.doc_id.clone()),
                ],
                None,
                from,
            )?,
            Verb::StoreQuery => {
                let mut params = vec![("project", self.ws.to_string()), ("name", ask.name.clone())];
                if !ask.limit.is_empty() {
                    params.push(("limit", ask.limit.clone()));
                }
                self.load(Method::Get, "/cli/store/query", params, None, from)?
            }
            Verb::StoreRm => {
                let body = obj(vec![
                    ("project", ws.clone()),
                    ("name", s(&ask.name)),
                    ("id", s(&ask.doc_id)),
                ]);
                self.load(Method::Post, "/cli/store/rm", vec![], Some(body), from)?
            }
            Verb::StoreDrop => {
                let body = obj(vec![("project", ws.clone()), ("name", s(&ask.name))]);
                self.load(Method::Post, "/cli/store/drop", vec![], Some(body), from)?
            }
        };
        if matches!(d, Value::Object(_))
            && is_false(at(&d, "ok"))
            && matches!(at(&d, "error"), Some(Value::Object(_)))
        {
            self.note_permission(&d);
            let e = at(&d, "error").cloned().unwrap_or(Value::Null);
            return Err(self.die(&e, &[(line::DIE_OK_FALSE, "<module>")]));
        }
        if status >= 400 {
            self.note_permission(&d);
            if let Some(e @ Value::Object(_)) = at(&d, "error") {
                let e = e.clone();
                return Err(self.die(&e, &[(line::DIE_STATUS_ERROR, "<module>")]));
            }
            let head: String = raw.chars().take(400).collect();
            let hint = if head.is_empty() {
                format!("HTTP {status}")
            } else {
                head
            };
            return Err(self.die(
                &obj(vec![("code", s("error")), ("hint", s(&hint))]),
                &[(line::DIE_STATUS, "<module>")],
            ));
        }
        if self.ask.json {
            let text = match &d {
                Value::Str(_) => py::str_of(&d),
                other => dumps(other),
            };
            self.print(text);
            return Ok(());
        }
        self.printer(&d)?;
        Ok(())
    }

    /// A call and `load` of its answer: the status, the body's text and
    /// the value.
    fn load(
        &mut self,
        method: Method,
        route: &str,
        params: Vec<(&str, String)>,
        body: Option<Value>,
        from: u32,
    ) -> R<(u16, String, Value)> {
        let (status, raw) = self.request(method, route, params, body, from)?;
        let d = load(&raw);
        Ok((status, raw, d))
    }

    /// `clusters`: the status call, and the helper's view from the
    /// clusters call when the first was no refusal.
    fn clusters(&mut self) -> R<(u16, String, Value)> {
        let (status, raw, d) = self.load(
            Method::Get,
            "/cli/db/status",
            vec![],
            None,
            Verb::Clusters.call_line(),
        )?;
        if !matches!(d, Value::Object(_)) || is_false(at(&d, "ok")) {
            return Ok((status, raw, d));
        }
        let pick = |k: &str| at(&d, k).cloned().unwrap_or(Value::Null);
        let mut out = vec![
            ("ok", Value::Bool(true)),
            ("clusters", or(at(&d, "clusters"), empty_list())),
            ("databases", or(at(&d, "databases"), empty_list())),
            ("defaultMajor", pick("defaultMajor")),
            ("defaultMajorPinned", pick("defaultMajorPinned")),
            ("defaultMajorSource", pick("defaultMajorSource")),
            (
                "upgradeAvailable",
                or(at(&d, "upgradeAvailable"), empty_list()),
            ),
        ];
        let params = if self.ask.health {
            vec![("health", "1".to_string())]
        } else {
            vec![]
        };
        let (cstatus, craw) =
            self.request(Method::Get, "/cli/db/clusters", params, None, CLUSTERS_CALL)?;
        let cd = load(&craw);
        if cstatus == 200 {
            if let Some(list @ Value::Array(_)) = at(&cd, "clusters") {
                out[1].1 = list.clone();
                if truthy(at(&cd, "helperView")) {
                    out.push((
                        "helperView",
                        at(&cd, "helperView").cloned().unwrap_or(Value::Null),
                    ));
                }
            }
        }
        Ok((status, raw, obj(out)))
    }

    /// `note_permission(v)`: the closing line, printed after the error
    /// line (the EXIT trap prints the note file once the command failed).
    fn note_permission(&mut self, v: &Value) {
        let Some(p @ Value::Object(_)) = at(v, "permission") else {
            return;
        };
        let p = &*crate::transport::permission::named(p);
        let mut lines = Vec::new();
        match at(p, "command") {
            Some(Value::Str(c)) if py::truthy(&Value::Str(c.clone())) => {
                lines.push(format!("Ask with: {}", py::str_of(&Value::Str(c.clone()))));
            }
            _ if is_false(at(p, "requestable")) => {
                if let Some(h @ Value::Str(_)) = at(p, "hint") {
                    if py::truthy(h) {
                        lines.push(py::str_of(h));
                    }
                }
                lines.push("It can't be requested.".to_string());
                if at(p, "feedback") == Some(&Value::Bool(true)) {
                    lines.push(crate::transport::permission::NO_CARD_FEEDBACK.to_string());
                }
            }
            _ => {}
        }
        if !lines.is_empty() {
            self.pending_note = Some(lines.join("\n") + "\n");
        }
    }
}

/// `load(raw)`: `{}` for an empty body, `{"raw": raw}` for one that is
/// not JSON.
pub fn load(raw: &str) -> Value {
    if raw.is_empty() {
        return empty_dict();
    }
    json::parse(raw).unwrap_or_else(|_| obj(vec![("raw", s(raw))]))
}

/// `int(<digits>)`, for a major the shell checked is `[1-9][0-9]`.
fn int_of(digits: &str) -> Value {
    Value::Int(digits.to_string())
}

/// `x.get(k)` for `x` that Python must find a dict, at `line`.
macro_rules! gs {
    ($v:expr, $k:expr, $line:expr) => {
        st(g($v, $k, $line)?)
    };
}

impl Py<'_> {
    /// The verb's lines (`d` is not a refusal and `--json` is off).
    fn printer(&mut self, d: &Value) -> R<()> {
        match self.ask.verb {
            Verb::Status => self.status(d),
            Verb::Clusters => self.cluster_lines(d),
            Verb::DefaultMajor => {
                let t = format!(
                    "new databases go on Postgres {}",
                    gs!(d, "defaultMajor", line::DEFAULT_MAJOR)
                );
                self.print(t);
                Ok(())
            }
            Verb::Enable | Verb::Disable | Verb::Uninstall => {
                let t = format!("state: {}", gs!(d, "state", line::STATE));
                self.print(t);
                Ok(())
            }
            Verb::Create => self.create(d),
            Verb::Dsn => self.dsn(d),
            Verb::List => {
                let dbs = or(g(d, "databases", line::LIST)?, empty_list());
                let n = py_len(&dbs, line::LIST_LEN)?;
                self.print(format!("{n} database(s)"));
                for row in py_iter(&dbs, line::LIST_FOR)? {
                    let t = format!(
                        "  {}  {}",
                        gs!(&row, "name", line::LIST_ROW),
                        gs!(&row, "status", line::LIST_ROW)
                    );
                    self.print(t);
                }
                Ok(())
            }
            Verb::Migrate => {
                let applied = or(g(d, "applied", line::MIGRATE)?, empty_list());
                let discovered = or(g(d, "discovered", line::MIGRATE)?, Value::Int("0".into()));
                if py::truthy(&applied) {
                    let t = format!("applied: {}", join(&applied, ", ", line::MIGRATE_APPLIED)?);
                    self.print(t);
                } else if truthy(at(d, "noop")) && above_zero(&discovered, line::MIGRATE_NOOP)? {
                    self.print("migrations: already applied");
                } else {
                    let t = format!("applied: {}", join(&applied, ", ", line::MIGRATE_NONE)?);
                    self.print(t);
                }
                Ok(())
            }
            Verb::Dump => {
                let t = format!("dump: {}", gs!(d, "path", line::DUMP));
                self.print(t);
                Ok(())
            }
            Verb::Restore => {
                let t = format!("restored {}", gs!(d, "restored", line::RESTORE));
                self.print(t);
                Ok(())
            }
            Verb::Drop => {
                let t = format!("dropped {}", gs!(d, "dropped", line::DROP));
                self.print(t);
                self.apps(d, line::DROP_APPS, line::DROP_APP)
            }
            Verb::Grant => {
                let t = format!(
                    "granted {} on {} to {}",
                    gs!(d, "level", line::GRANT),
                    st(at(d, "name")),
                    st(at(d, "projectId"))
                );
                self.print(t);
                if truthy(at(d, "canManage")) {
                    self.print("can_manage: true");
                }
                self.apps(d, line::GRANT_APPS, line::GRANT_APP)
            }
            Verb::Revoke => {
                let t = format!(
                    "revoked {} from {}",
                    gs!(d, "name", line::REVOKE),
                    st(at(d, "projectId"))
                );
                self.print(t);
                self.apps(d, line::REVOKE_APPS, line::REVOKE_APP)
            }
            Verb::Bind => {
                let t = format!("bind role: {}", gs!(d, "bindRole", line::BIND));
                self.print(t);
                Ok(())
            }
            Verb::Doctor => self.doctor(d),
            Verb::StoreCreate
            | Verb::StoreList
            | Verb::StorePut
            | Verb::StoreGet
            | Verb::StoreQuery
            | Verb::StoreRm
            | Verb::StoreDrop => {
                self.print(dumps(d));
                Ok(())
            }
        }
    }

    /// `for a in d.get("apps") or []: print("app:      %s" % a.get("note"))`.
    fn apps(&mut self, d: &Value, for_line: u32, row_line: u32) -> R<()> {
        for a in py_iter(&or(at(d, "apps"), empty_list()), for_line)? {
            let t = format!("app:      {}", gs!(&a, "note", row_line));
            self.print(t);
        }
        Ok(())
    }

    fn status(&mut self, d: &Value) -> R<()> {
        let t = format!("supported: {}", gs!(d, "supported", line::STATUS));
        self.print(t);
        self.print(format!("state:     {}", st(at(d, "state"))));
        if at(d, "installedMajor").is_some_and(|v| *v != Value::Null) {
            self.print(format!("pg major:  {}", st(at(d, "installedMajor"))));
        }
        if truthy(at(d, "listen")) {
            self.print(format!("listen:    {}", st(at(d, "listen"))));
        }
        if let Some(port) = at(d, "port").filter(|v| **v != Value::Null) {
            self.print(format!("port:      {}", py::str_of(port)));
        }
        if truthy(at(d, "publishHint")) {
            self.print(format!("off-box:   {}", st(at(d, "publishHint"))));
        }
        if truthy(at(d, "lastError")) {
            self.print(format!("last error: {}", st(at(d, "lastError"))));
        }
        if at(d, "defaultMajor").is_some_and(|v| *v != Value::Null) {
            let src = src_of(at(d, "defaultMajorSource"), line::STATUS_SRC)?;
            self.print(format!(
                "new dbs:   Postgres {}{src}",
                st(at(d, "defaultMajor"))
            ));
        }
        for c in py_iter(&or(at(d, "clusters"), empty_list()), line::STATUS_CLUSTERS)? {
            let l = line::STATUS_CLUSTER;
            let t = format!(
                "cluster:   Postgres {} on {} ({}, {})",
                gs!(&c, "major", l),
                gs!(&c, "port", l),
                gs!(&c, "role", l),
                gs!(&c, "state", l)
            );
            self.print(t);
        }
        let upgrades = or(at(d, "upgradeAvailable"), empty_list());
        for u in py_iter(&upgrades, line::STATUS_UPGRADES)? {
            if truthy(g(&u, "check", line::STATUS_UPGRADE)?) {
                let l = line::STATUS_UPGRADE_CHECK;
                let t = format!(
                    "upgrade:   {} on {} → {} available: {}",
                    st(at(&u, "name")),
                    st(at(&u, "from")),
                    join_str(at(&u, "to"), "/", l)?,
                    st(at(&u, "check"))
                );
                self.print(t);
            } else {
                let l = line::STATUS_UPGRADE_LATER;
                let t = format!(
                    "upgrade:   {} on {} → {} available, once Postgres runs",
                    st(at(&u, "name")),
                    st(at(&u, "from")),
                    join_str(at(&u, "to"), "/", l)?
                );
                self.print(t);
            }
        }
        if let Some(blocked @ Value::Object(_)) = at(d, "upgradeBlocked") {
            if py::truthy(&upgrades) {
                self.print(format!("first:     {}", st(at(blocked, "hint"))));
            }
        }
        for x in py_iter(&or(at(d, "pgStale"), empty_list()), line::STATUS_STALE)? {
            let l = line::STATUS_STALE_LINE;
            let name = gs!(&x, "name", l);
            let package = st(at(&x, "package"));
            let version = if truthy(at(&x, "installedVersion")) {
                concat_after(" ", at(&x, "installedVersion").unwrap_or(&Value::Null), l)?
            } else {
                String::new()
            };
            let t = format!(
                "outdated:  {name} still runs the Postgres it started with; {package}{version} was installed since. {}",
                st(at(&x, "fix"))
            );
            self.print(t);
        }
        Ok(())
    }

    fn cluster_lines(&mut self, d: &Value) -> R<()> {
        let cl = or(g(d, "clusters", line::CLUSTERS)?, empty_list());
        let n = py_len(&cl, line::CLUSTERS_HEAD)?;
        let pinned = if truthy(at(d, "defaultMajorPinned")) {
            " (pinned)"
        } else {
            ""
        };
        self.print(format!(
            "{n} cluster(s) · new databases go on Postgres {}{pinned}",
            st(at(d, "defaultMajor"))
        ));
        for c in py_iter(&cl, line::CLUSTERS_FOR)? {
            let l = line::CLUSTERS_LABEL;
            let label = match g(&c, "label", l)? {
                Some(v) if py::truthy(v) => v.clone(),
                _ => s(&format!(
                    "{}/{}",
                    st(at(&c, "major")),
                    py::str_of(&or(at(&c, "name"), s("main")))
                )),
            };
            let named = !is_str(Some(&or(at(&c, "name"), s("main"))), "main");
            let dbs = or(at(d, "databases"), empty_list());
            let mut n = 0;
            if named {
                for x in py_iter(&dbs, line::CLUSTERS_NAMED_N)? {
                    if py_eq(
                        g(&x, "cluster", line::CLUSTERS_NAMED_N)?.unwrap_or(&Value::Null),
                        &label,
                    ) {
                        n += 1;
                    }
                }
            } else {
                for x in py_iter(&dbs, line::CLUSTERS_MAIN_N)? {
                    let same = py_eq(
                        g(&x, "major", line::CLUSTERS_MAIN_N)?.unwrap_or(&Value::Null),
                        at(&c, "major").unwrap_or(&Value::Null),
                    );
                    if same && !truthy(at(&x, "cluster")) {
                        n += 1;
                    }
                }
            }
            let extra = if is_str(at(&c, "role"), "foreign") {
                concat!("  made outside ", product_name!(), ": never touched").to_string()
            } else if named {
                format!(
                    "  named{}",
                    if truthy(at(&c, "walArchive")) {
                        ", WAL archive"
                    } else {
                        ""
                    }
                )
            } else {
                String::new()
            };
            self.print(format!(
                "  {} port {}  {}  {}/{}  {n} database(s){extra}",
                py_left(&label, 14),
                st(at(&c, "port")),
                st(at(&c, "unit")),
                st(at(&c, "role")),
                st(at(&c, "state"))
            ));
            if let Some(h @ Value::Object(_)) = at(&c, "health") {
                if truthy(at(h, "error")) {
                    self.print(format!("      health: {}", st(at(h, "error"))));
                } else if is_false(at(h, "online")) {
                    self.print("      health: not running");
                } else {
                    let l = line::CLUSTERS_SIZES;
                    let mut sizes = Vec::new();
                    for x in py_iter(&or(at(h, "databases"), empty_list()), l)? {
                        let name = gs!(&x, "name", l);
                        let bytes = or(at(&x, "bytes"), Value::Int("0".into()));
                        sizes.push(format!("{name} {} MB", megabytes(&bytes, l)?));
                    }
                    let sizes = sizes.join(", ");
                    let a = or(at(h, "archiver"), empty_dict());
                    self.print(format!(
                        "      {}/{} connections · {}",
                        st(at(h, "connections")),
                        st(at(h, "maxConnections")),
                        if sizes.is_empty() {
                            "no databases"
                        } else {
                            sizes.as_str()
                        }
                    ));
                    if is_str(at(h, "archiveMode"), "on") {
                        let l = line::CLUSTERS_WAL;
                        let t = format!(
                            "      WAL archive: {} archived (last {}), {} failed (last {})",
                            gs!(&a, "archivedCount", l),
                            py::str_of(&or(at(&a, "lastArchivedAt"), s("-"))),
                            st(at(&a, "failedCount")),
                            py::str_of(&or(at(&a, "lastFailedAt"), s("-")))
                        );
                        self.print(t);
                    }
                    if truthy(g(&a, "currentLsn", line::CLUSTERS_LSN)?) {
                        let lsn = st(at(&a, "currentLsn"));
                        self.print(format!("      current WAL position: {lsn}"));
                    }
                }
            }
        }
        let upgrades = or(at(d, "upgradeAvailable"), empty_list());
        for u in py_iter(&upgrades, line::CLUSTERS_UPGRADES)? {
            let l = line::CLUSTERS_UPGRADE;
            let t = format!(
                "  upgrade available: {} ({} → {})",
                gs!(&u, "name", l),
                st(at(&u, "from")),
                join_str(at(&u, "to"), "/", l)?
            );
            self.print(t);
        }
        Ok(())
    }

    fn create(&mut self, d: &Value) -> R<()> {
        let name = gs!(d, "name", line::CREATE);
        let on = if truthy(at(d, "major")) {
            format!(" on Postgres {}", st(at(d, "major")))
        } else {
            String::new()
        };
        self.print(format!("created {name}{on}"));
        if truthy(at(d, "majorNote")) {
            self.print(format!("note: {}", st(at(d, "majorNote"))));
        }
        if truthy(at(d, "existing")) {
            self.print("(already there: same --id or name)");
        }
        if truthy(at(d, "dsn")) {
            self.print(format!("dsn: {}", st(at(d, "dsn"))));
        }
        if truthy(at(d, "migratorDsn")) {
            self.print(format!("migratorDsn: {}", st(at(d, "migratorDsn"))));
        }
        if truthy(at(d, "dsnHint")) {
            self.print(format!("no password shown; {}", st(at(d, "dsnHint"))));
        }
        let kept = or(at(d, "kept"), empty_dict());
        if truthy(g(&kept, "ref", line::CREATE_KEPT)?) {
            self.print(format!(
                "kept:     {} as {} (this workspace's default database)",
                st(at(&kept, "ref")),
                st(at(&kept, "name"))
            ));
        } else if truthy(at(&kept, "error")) {
            self.print(format!(
                "kept:     no DATABASE_URL reference ({})",
                st(at(&kept, "error"))
            ));
        }
        let cap = or(at(d, "cap"), empty_dict());
        if py::truthy(&cap) {
            let used = gs!(&cap, "used", line::CREATE_CAP);
            let of = if truthy(at(&cap, "cap")) {
                st(at(&cap, "cap"))
            } else {
                "unlimited".to_string()
            };
            self.print(format!("cap {used}/{of} used"));
        }
        self.apps(d, line::CREATE_APPS, line::CREATE_APP)?;
        if truthy(at(d, "next")) {
            self.print(format!(
                "next:     {} (an App's own login and DATABASE_URL, checked live)",
                st(at(d, "next"))
            ));
        }
        Ok(())
    }

    fn dsn(&mut self, d: &Value) -> R<()> {
        if truthy(g(d, "ref", line::DSN)?) {
            self.print(st(at(d, "ref")));
            if truthy(at(d, "name")) && !is_str(at(d, "name"), "DATABASE_URL") {
                self.eprint(format!("kept as {}", st(at(d, "name"))));
            }
        } else {
            self.print(py::str_of(&or(at(d, "dsn"), s(""))));
            if is_false(at(d, "dsnHasPassword")) && truthy(at(d, "dsnHint")) {
                self.eprint(st(at(d, "dsnHint")));
            } else if is_true(at(d, "dsnHasPassword")) {
                self.eprint(concat!(
                    "this printed a password: keep it out of chats, logs, notes and git (",
                    cli_name!(),
                    " db run -- <cmd> needs no copy)"
                ));
            }
        }
        Ok(())
    }

    fn doctor(&mut self, d: &Value) -> R<()> {
        let t = format!("ok:       {}", gs!(d, "ok", line::DOCTOR));
        self.print(t);
        self.print(format!("baked:    {}", st(at(d, "baked"))));
        self.print(format!("unit:     {}", st(at(d, "unit"))));
        self.print(format!("select1:  {}", st(at(d, "select1"))));
        self.print(format!(
            "major:    recorded {} · live {}{}",
            st(at(d, "installedMajor")),
            st(at(d, "liveMajor")),
            if truthy(at(d, "majorMismatch")) {
                "  MISMATCH"
            } else {
                ""
            }
        ));
        for c in py_iter(&or(at(d, "clusters"), empty_list()), line::DOCTOR_CLUSTERS)? {
            let l = line::DOCTOR_CLUSTER;
            let t = format!(
                "cluster:  Postgres {} port {} {} ({}) · {} database(s)",
                gs!(&c, "major", l),
                gs!(&c, "port", l),
                gs!(&c, "unit", l),
                gs!(&c, "unitState", l),
                gs!(&c, "databases", l)
            );
            self.print(t);
        }
        let ram = or(at(d, "ram"), empty_dict());
        let cur = gs!(&ram, "currentBytes", line::DOCTOR_RAM);
        self.print(format!(
            "rss:      {cur} / {}  (MemoryHigh={} MemoryMax={} withinCap={})",
            st(at(&ram, "maxBytes")),
            st(at(&ram, "high")),
            st(at(&ram, "max")),
            st(at(&ram, "withinCap"))
        ));
        self.print(format!(
            "ram unit: {}  oom={}  dropinOk={}",
            st(at(&ram, "unit")),
            st(at(&ram, "oomTarget")),
            st(at(&ram, "dropinOk"))
        ));
        let gucs = or(at(d, "gucs"), empty_dict());
        self.print(format!("gucsOk:   {}", st(at(d, "gucsOk"))));
        for k in [
            "shared_buffers",
            "work_mem",
            "max_connections",
            "max_parallel_workers",
            "statement_timeout",
            "idle_in_transaction_session_timeout",
            "temp_file_limit",
        ] {
            if contains(&gucs, k, line::DOCTOR_GUCS)? {
                let t = format!("  {k} = {}", gs!(&gucs, k, line::DOCTOR_GUC));
                self.print(t);
            }
        }
        if truthy(at(d, "hint")) {
            self.print(format!("hint:     {}", st(at(d, "hint"))));
        }
        Ok(())
    }
}
