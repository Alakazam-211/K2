//! `k5 heartbeat wake` (and `heartbeat signal wake`), as the shell CLI's
//! `cmd_heartbeat_wake` runs it (`cli/src/talk-and-workspace.sh`): one
//! Python block that asks the daemon for inbox work and, when there is
//! some, writes the triage line to the running manager's terminal or
//! launches the manager and writes it once its terminal is up. Words
//! after `wake` are ignored, as the shell ignores them.
//!
//! The block reads its own port and token (the session's `*_PORT` and
//! `*_HOOK_TOKEN`, else `heartbeat.port` and, unless the no-owner switch
//! is on, `heartbeat.token`), sends every request through urllib
//! ([`crate::transport::urllib`]) with a 5 s timeout, heals a refused
//! passport once, and ends on its first failure with one line. Each
//! Python failure is copied: the urllib exceptions by class, a value the
//! Python compares or walks that is the wrong type by its `TypeError` or
//! `AttributeError`, and `type(data).__name__` in the not-a-list lines.
//!
//! Handed to the shell ([`Setup::read`] returns `None`), before anything
//! is sent: a port that is not digits urllib reads as a port, a token
//! urllib would refuse as a header value, and a `heartbeat.port` or
//! `heartbeat.token` that is not UTF-8.

use crate::exit::Exit;
use crate::out::json::{self, Options, Value};
use crate::out::{self, py};
use crate::transport::urllib::{Class, Kind, Method, Opener, Outcome, Raised, Refused, Request};
use crate::transport::{Auth, Client};
use k2_client::{cli_name, env_names, product_name};
use std::path::Path;
use std::time::Duration;

pub const AGENTS: &str = "/cli/agents/list";
pub const INBOX: &str = "/cli/inbox/list";
pub const RUNNING: &str = "/cli/agents/running";
pub const WRITE: &str = "/cli/terminal/write";
pub const LAUNCH: &str = "/cli/agents/launch";
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// The triage line written to the manager's terminal.
pub fn triage() -> String {
    format!(
        "New work detected. Run {} inbox to see your inbox and triage work.",
        cli_name!()
    )
}
/// The polls after a launch, [`POLL_PAUSE`] apart.
pub const POLLS: usize = 12;
pub const POLL_PAUSE: Duration = Duration::from_secs(5);
const MANAGER_DIRS: [&str; 4] = [
    ".k2/agents/manager",
    ".k2so/agents/manager",
    ".k2/agents/coordinator",
    ".k2so/agents/coordinator",
];

/// What the block reads before its first request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    /// The port as the Python holds it (text: a leading zero stays in the
    /// `Host` header); empty: none.
    pub port: String,
    pub token: String,
    /// The token is the session's (`*_HOOK_TOKEN`): the shell's start
    /// has switched it to a passport minted again ([`Wake::new`]).
    pub from_env: bool,
}

/// `.strip()` of a file's text; `Some(None)`: no file to read (OSError);
/// `None`: not UTF-8 (the Python stops there).
fn read_stripped(path: &Path) -> Option<Option<String>> {
    let Ok(b) = std::fs::read(path) else {
        return Some(None);
    };
    let t = String::from_utf8(b).ok()?;
    Some(Some(t.trim_matches(super::super::py_space).to_string()))
}

impl Setup {
    /// The port and token, or `None` for the shell.
    pub fn read(home: &Path, no_owner_fallback: bool) -> Option<Setup> {
        let env = |names: &'static [&'static str]| -> Option<String> {
            match env_names::first_nonempty_os(names) {
                Some((_, v)) => Some(v.into_string().ok()?),
                None => Some(String::new()),
            }
        };
        let mut port = env(env_names::PORT)?;
        let mut token = env(env_names::HOOK_TOKEN)?;
        let from_env = !token.is_empty();
        if port.is_empty() {
            port = read_stripped(&home.join("heartbeat.port"))?.unwrap_or_default();
        }
        if token.is_empty() && !no_owner_fallback {
            token = read_stripped(&home.join("heartbeat.token"))?.unwrap_or_default();
        }
        if !port.is_empty()
            && !(port.bytes().all(|b| b.is_ascii_digit())
                && port.parse::<u32>().is_ok_and(|p| p <= 65535))
        {
            return None;
        }
        if !token
            .bytes()
            .all(|b| b == b'\t' || (0x20..0x7f).contains(&b))
        {
            return None;
        }
        Some(Setup {
            port,
            token,
            from_env,
        })
    }
}

/// How the block ends early: its stderr and exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct End {
    pub stderr: String,
    pub exit: Exit,
}

fn fail(msg: &str) -> End {
    End {
        stderr: format!("Error: {} heartbeat wake: {msg}\n", cli_name!()),
        exit: Exit::Error,
    }
}

/// The outer `except Exception as e`.
fn stopped(name: &str, text: &str) -> End {
    fail(&format!("the wake stopped: {name}: {text}"))
}

fn raised_stop(r: &Raised) -> End {
    let name = r.class.name();
    let short = name.rsplit('.').next().unwrap_or(name);
    stopped(short, &r.text())
}

/// Why a request got no answer, or the block's end.
enum Miss {
    Busy(String),
    End(End),
}

/// `type(x).__name__`.
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `d.get(key, default)`.
fn get_or(v: &Value, key: &str, default: Value) -> Value {
    match v {
        Value::Object(pairs) => super::super::get(pairs, key).cloned().unwrap_or(default),
        _ => default,
    }
}

/// `x > 0`, or the TypeError's text.
pub fn above_zero(x: &Value) -> Result<bool, String> {
    match x {
        Value::Int(d) => Ok(!d.starts_with('-') && d != "0"),
        Value::Float(f) => Ok(*f > 0.0),
        Value::Bool(b) => Ok(*b),
        other => Err(format!(
            "'>' not supported between instances of '{}' and 'int'",
            type_name(other)
        )),
    }
}

/// `needle in hay`, or the TypeError's text.
pub fn contains(hay: &Value, needle: &str) -> Result<bool, String> {
    match hay {
        Value::Str(t) => Ok(py::str_of(&Value::Str(t.clone())).contains(needle)),
        Value::Array(items) => Ok(items
            .iter()
            .any(|i| matches!(i, Value::Str(t) if t.as_str() == Some(needle)))),
        Value::Object(pairs) => Ok(pairs.iter().any(|(k, _)| k.as_str() == Some(needle))),
        other => Err(format!(
            "argument of type '{}' is not iterable",
            type_name(other)
        )),
    }
}

fn in_manager_dir(cwd: &Value) -> Result<bool, String> {
    for d in MANAGER_DIRS {
        if contains(cwd, d)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn as_str(tid: &Value) -> Result<String, String> {
    match tid {
        Value::Str(_) => Ok(py::str_of(tid)),
        other => Err(format!(
            "'{}' object has no attribute 'startswith'",
            type_name(other)
        )),
    }
}

/// `agent_from_tid`.
pub fn agent_from_tid(tid: &str) -> Option<String> {
    if let Some(rest) = tid.strip_prefix("agent-chat:") {
        if rest.starts_with("wt:") {
            return None;
        }
        let parts: Vec<&str> = rest.split(':').collect();
        return (parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty())
            .then(|| parts[1].to_string());
    }
    if let Some(name) = tid.strip_prefix("agent-chat-") {
        if !tid.starts_with("agent-chat-wt-") && !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// `running_manager`: the manager's terminal id, or `''`.
pub fn running_manager(found: &[Value]) -> Result<Value, String> {
    for t in found {
        let cwd = get_or(t, "cwd", super::super::s(""));
        let tid = get_or(t, "terminalId", super::super::s(""));
        if in_manager_dir(&cwd)? {
            return Ok(tid);
        }
        let agent = agent_from_tid(&as_str(&tid)?);
        if matches!(agent.as_deref(), Some("manager" | "coordinator")) {
            return Ok(tid);
        }
    }
    Ok(super::super::s(""))
}

/// `launched_manager`.
pub fn launched_manager(found: &[Value]) -> Result<Value, String> {
    for t in found {
        let cwd = get_or(t, "cwd", super::super::s(""));
        let tid = get_or(t, "terminalId", super::super::s(""));
        if in_manager_dir(&cwd)? {
            return Ok(tid);
        }
        let s = as_str(&tid)?;
        if s.starts_with("agent-chat-manager") || s.starts_with("agent-chat-coordinator") {
            return Ok(tid);
        }
    }
    Ok(super::super::s(""))
}

/// `urllib.parse.quote(s, safe=safe)`.
pub fn quote(s: &str, safe: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) || safe.as_bytes().contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `say(**fields)`: compact JSON, not ASCII-escaped.
fn say(fields: Vec<(&str, Value)>) -> String {
    let opts = Options {
        compact: true,
        ensure_ascii: false,
        ..Options::default()
    };
    let v = Value::Object(
        fields
            .into_iter()
            .map(|(k, v)| (json::Text::from(k), v))
            .collect(),
    );
    format!("{}\n", json::to_string(&v, &opts).unwrap_or_default())
}

/// The block's run.
pub struct Wake {
    opener: Opener,
    project: String,
    base: String,
    token: String,
    healed: bool,
    /// `time.sleep(5)` between polls; a test may shorten it.
    pub pause: Duration,
}

impl Wake {
    /// The block for `setup` with `client`'s hand-over headers and
    /// workspace. `client` comes back as it was.
    pub fn new(client: &mut Client, setup: &Setup) -> Wake {
        // The prelude's `_cli_use_healed_passport` exports the passport it
        // switched to as `*_HOOK_TOKEN`, which the block then reads.
        let mut setup = setup.clone();
        if setup.from_env {
            client.use_healed_passport();
            if let Auth::Passport { passport, .. } = &client.auth {
                setup.token = passport.as_str().to_string();
            }
        }
        let setup = &setup;
        let saved = std::mem::replace(&mut client.auth, Auth::Owner(setup.token.clone()));
        let opener = Opener::from_client(client);
        client.auth = saved;
        let mut opener = opener;
        opener.host = "127.0.0.1".into();
        opener.port = setup.port.parse().unwrap_or(0);
        opener.netloc = format!("127.0.0.1:{}", setup.port);
        Wake {
            opener,
            project: client.project.clone(),
            base: format!("http://127.0.0.1:{}", setup.port),
            token: setup.token.clone(),
            healed: false,
            pause: POLL_PAUSE,
        }
    }

    fn request(
        &mut self,
        route: &str,
        post: bool,
        soft: bool,
        params: &[(&str, &Value)],
    ) -> Result<(u16, Vec<u8>), Miss> {
        let p = product_name!();
        let mut query = format!("project={}", quote(&self.project, "/"));
        for (k, v) in params {
            let Value::Str(_) = v else {
                return Err(Miss::End(stopped(
                    "TypeError",
                    "quote_from_bytes() expected bytes",
                )));
            };
            query.push_str(&format!("&{k}={}", quote(&py::str_of(v), "")));
        }
        let mut req = Request::new(
            if post { Method::Post } else { Method::Get },
            format!("{route}?{query}"),
            TIMEOUT,
        );
        if !self.token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.token));
        }
        if post {
            req = req.data(Vec::new());
        }
        let why = |r: &Raised| -> Result<String, End> {
            if r.kind == Kind::Timeout && matches!(r.class, Class::UrlError | Class::TimeoutError) {
                return Ok(format!("{p}'s server didn't answer {route} within 5 s"));
            }
            match r.class {
                Class::UrlError => Ok(format!(
                    "can't reach {p}'s server at {}: {}",
                    self.base, r.reason
                )),
                Class::TimeoutError => Ok(format!("{p}'s server didn't answer {route} within 5 s")),
                Class::RemoteDisconnected
                | Class::ConnectionResetError
                | Class::BrokenPipeError
                | Class::OsError(_) => Ok(format!(
                    "{p}'s server closed the connection on {route}: {}",
                    r.text()
                )),
                _ => Err(raised_stop(r)),
            }
        };
        // `_K2NoRedirect`: the rule's line, then `SystemExit(3)` (the
        // block's `except SystemExit: raise` passes it on).
        let out = match self.opener.open_ruled(&req) {
            Ok(out) => out,
            Err(r) => {
                return Err(Miss::End(End {
                    stderr: format!("{}\n", r.line),
                    exit: Refused::EXIT,
                }))
            }
        };
        let got = match out {
            Outcome::Answer { status, body } => match body {
                Ok(b) => Ok((status, b)),
                Err(r) => Err(why(&r)),
            },
            Outcome::HttpError { code, body } => match body {
                Ok(b) => Ok((code, b)),
                Err(r) => Err(Err(raised_stop(&r))),
            },
            Outcome::Raised(r) => Err(why(&r)),
        };
        let (code, raw) = match got {
            Ok(x) => x,
            Err(Err(end)) => return Err(Miss::End(end)),
            Err(Ok(why)) => {
                if soft {
                    return Err(Miss::Busy(why));
                }
                return Err(Miss::End(fail(&format!(
                    "{why}; run {} daemon status",
                    cli_name!()
                ))));
            }
        };
        if (code == 401 || code == 403)
            && !self.healed
            && raw
                .windows(b"Invalid or missing auth token".len())
                .any(|w| w == b"Invalid or missing auth token")
        {
            if let Some(new) = self.healed_passport() {
                self.healed = true;
                self.token = new;
                return self.request(route, post, soft, params);
            }
        }
        Ok((code, raw))
    }

    /// `healed_passport()`.
    fn healed_passport(&self) -> Option<String> {
        let cur = &self.token;
        let (_, sd) = env_names::first_nonempty_os(env_names::SECRETS_DIR)?;
        let f = Path::new(&sd).join(".k2-passport");
        if !cur.contains('.') || !f.is_file() || f.is_symlink() {
            return None;
        }
        let text = std::fs::read_to_string(&f).ok()?;
        let line = text.split_inclusive('\n').next().unwrap_or("");
        let t = line.strip_suffix('\n').unwrap_or(line);
        let (head, tail) = t.split_once('.')?;
        let ok = !head.is_empty()
            && head.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !tail.is_empty()
            && tail.bytes().all(|b| b.is_ascii_alphanumeric());
        if !ok || t == cur || Some(head) != cur.split('.').next() {
            return None;
        }
        Some(t.to_string())
    }

    fn checked(
        &mut self,
        route: &str,
        post: bool,
        soft: bool,
        params: &[(&str, &Value)],
    ) -> Result<Option<Value>, Miss> {
        let p = product_name!();
        let (code, raw) = self.request(route, post, soft, params)?;
        let data = std::str::from_utf8(&raw)
            .ok()
            .and_then(|t| json::parse(t).ok());
        if data.is_none() && code == 200 {
            return Err(Miss::End(fail(&format!(
                "{p}'s server answered {route} with something that is not JSON"
            ))));
        }
        if let (true, 503, Some(d @ Value::Object(_))) = (soft, code, &data) {
            if matches!(get_or(d, "state", Value::Null), Value::Str(t) if t.as_str() == Some("migrating"))
            {
                let e = get_or(d, "error", Value::Null);
                let e = if py::truthy(&e) {
                    py::str_of(&e)
                } else {
                    "migrating".into()
                };
                return Err(Miss::Busy(format!(
                    "{p}'s server answered {route} with HTTP 503: {e}"
                )));
            }
        }
        let errs = post
            && matches!(&data, Some(d @ Value::Object(_)) if py::truthy(&get_or(d, "error", Value::Null)));
        if code != 200 || errs {
            return Err(Miss::End(refused(route, code, data.as_ref(), &raw)));
        }
        Ok(data)
    }

    fn list_of_dicts(
        &mut self,
        route: &str,
        soft: bool,
        params: &[(&str, &Value)],
        what: &str,
    ) -> Result<Vec<Value>, Miss> {
        let p = product_name!();
        let data = self
            .checked(route, false, soft, params)?
            .unwrap_or(Value::Null);
        match data {
            Value::Array(items) if items.iter().all(|i| matches!(i, Value::Object(_))) => Ok(items),
            other => Err(Miss::End(fail(&format!(
                "{p}'s server answered {route} with a {}, not a list of {what}",
                type_name(&other)
            )))),
        }
    }

    /// The block, start to end: stdout and how it ended.
    pub fn run(&mut self, setup: &Setup) -> (String, Result<(), End>) {
        let p = product_name!();
        if setup.port.is_empty() {
            return (
                String::new(),
                Err(fail(&format!(
                    "no port for {p}'s server (no K2_PORT and no heartbeat.port); run {} daemon status",
                    cli_name!()
                ))),
            );
        }
        match self.steps() {
            Ok(line) => (line, Ok(())),
            Err(Miss::End(e)) => (String::new(), Err(e)),
            Err(Miss::Busy(why)) => (String::new(), Err(fail(&why))),
        }
    }

    fn steps(&mut self) -> Result<String, Miss> {
        let p = product_name!();
        let te = |e: String| Miss::End(stopped("TypeError", &e));
        let agents = self.list_of_dicts(AGENTS, false, &[], "agents")?;
        let mut agent_inbox = false;
        for a in &agents {
            if above_zero(&get_or(a, "inboxCount", Value::Int("0".into()))).map_err(te)? {
                agent_inbox = true;
                break;
            }
        }
        let folder = super::super::s("");
        let items = self.list_of_dicts(INBOX, false, &[("folder", &folder)], "inbox items")?;
        if !(agent_inbox || !items.is_empty()) {
            return Ok(say(vec![
                ("status", super::super::s("noop")),
                ("reason", super::super::s("no inbox work found")),
            ]));
        }
        let found = self.list_of_dicts(RUNNING, false, &[], "sessions")?;
        let manager = running_manager(&found).map_err(|e| attr_or_type(&e))?;
        let triage = super::super::s(&triage());
        if py::truthy(&manager) {
            self.checked(
                WRITE,
                true,
                false,
                &[("id", &manager), ("message", &triage)],
            )?;
            return Ok(say(vec![
                ("status", super::super::s("notified")),
                ("terminalId", manager),
            ]));
        }
        let who = super::super::s("manager");
        self.checked(LAUNCH, true, false, &[("agent", &who)])?;
        let mut busy = String::new();
        for _ in 0..POLLS {
            std::thread::sleep(self.pause);
            let found = match self.list_of_dicts(RUNNING, true, &[], "sessions") {
                Ok(f) => f,
                Err(Miss::Busy(why)) => {
                    busy = why;
                    continue;
                }
                Err(e) => return Err(e),
            };
            busy.clear();
            let new_id = launched_manager(&found).map_err(|e| attr_or_type(&e))?;
            if py::truthy(&new_id) {
                self.checked(WRITE, true, false, &[("id", &new_id), ("message", &triage)])?;
                return Ok(say(vec![
                    ("status", super::super::s("launched_and_notified")),
                    ("terminalId", new_id),
                ]));
            }
        }
        if !busy.is_empty() {
            return Err(Miss::End(fail(&format!(
                "the manager was launched, but {p} did not answer the last of 12 polls for its terminal (60 s): {busy}; run {} daemon status",
                cli_name!()
            ))));
        }
        Ok(say(vec![
            ("status", super::super::s("launched")),
            ("agent", super::super::s("manager")),
            (
                "note",
                super::super::s("triage message not sent - terminal not ready"),
            ),
        ]))
    }
}

/// A walk's raise: `AttributeError` for `startswith`, else `TypeError`.
fn attr_or_type(e: &str) -> Miss {
    let name = if e.ends_with("'startswith'") {
        "AttributeError"
    } else {
        "TypeError"
    };
    Miss::End(stopped(name, e))
}

/// `refused()`: the line, the permission card's, and the exit.
pub fn refused(route: &str, code: u16, data: Option<&Value>, raw: &[u8]) -> End {
    let p = product_name!();
    let c = cli_name!();
    let dict = data.filter(|d| matches!(d, Value::Object(_)));
    let mut err = dict
        .map(|d| get_or(d, "error", Value::Null))
        .unwrap_or(Value::Null);
    if let Value::Object(_) = err {
        let hint = get_or(&err, "hint", Value::Null);
        err = if py::truthy(&hint) {
            hint
        } else {
            get_or(&err, "code", Value::Null)
        };
    }
    let why = if py::truthy(&err) {
        py::str_of(&err)
    } else {
        let text = String::from_utf8_lossy(raw);
        let t: String = text
            .trim_matches(super::super::py_space)
            .chars()
            .take(300)
            .collect();
        if t.is_empty() {
            "no body".into()
        } else {
            t
        }
    };
    let verb = if code == 401 || code == 403 {
        "refused"
    } else {
        "answered"
    };
    let mut stderr =
        format!("Error: {c} heartbeat wake: {p}'s server {verb} {route} with HTTP {code}: {why}\n");
    if let Some(Value::Object(_)) = dict.map(|d| get_or(d, "permission", Value::Null)).as_ref() {
        let p = dict
            .map(|d| get_or(d, "permission", Value::Null))
            .unwrap_or(Value::Null);
        let p = crate::transport::permission::named(&p).into_owned();
        if let Value::Str(cmd) = get_or(&p, "command", Value::Null) {
            let cmd = py::str_of(&Value::Str(cmd));
            if !cmd.is_empty() {
                stderr.push_str(&format!("Ask with: {cmd}\n"));
                return End {
                    stderr,
                    exit: Exit::Refused,
                };
            }
        }
        if get_or(&p, "requestable", Value::Null) == Value::Bool(false) {
            if let Value::Str(h) = get_or(&p, "hint", Value::Null) {
                let h = py::str_of(&Value::Str(h));
                if !h.is_empty() {
                    stderr.push_str(&format!("{h}\n"));
                }
            }
            stderr.push_str("It can't be requested.\n");
            if get_or(&p, "feedback", Value::Null) == Value::Bool(true) {
                stderr.push_str(crate::transport::permission::NO_CARD_FEEDBACK);
                stderr.push('\n');
            }
            return End {
                stderr,
                exit: Exit::Refused,
            };
        }
    }
    End {
        stderr,
        exit: Exit::Error,
    }
}

/// Prints what the block printed; its exit.
pub fn show(stdout: &str, end: Result<(), End>) -> Exit {
    if !stdout.is_empty() {
        if let Err(e) = out::stdout(stdout) {
            return out::after_stdout_error(&e);
        }
    }
    match end {
        Ok(()) => Exit::Ok,
        Err(e) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), e.stderr.as_bytes());
            e.exit
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_python_s_walks() {
        assert_eq!(
            agent_from_tid("agent-chat:p1:manager:hb:x"),
            Some("manager".into())
        );
        assert_eq!(agent_from_tid("agent-chat:wt:w1"), None);
        assert_eq!(
            agent_from_tid("agent-chat-coordinator"),
            Some("coordinator".into())
        );
        assert_eq!(agent_from_tid("agent-chat-wt-1"), None);
        assert_eq!(above_zero(&Value::Int("3".into())), Ok(true));
        assert_eq!(above_zero(&Value::Int("-3".into())), Ok(false));
        assert_eq!(
            above_zero(&Value::Null),
            Err("'>' not supported between instances of 'NoneType' and 'int'".into())
        );
        assert_eq!(
            contains(&Value::Int("1".into()), "x"),
            Err("argument of type 'int' is not iterable".into())
        );
        assert_eq!(quote("a b/ü", "/"), "a%20b/%C3%BC");
        assert_eq!(quote("a/b", ""), "a%2Fb");
    }

    #[test]
    fn a_refusal_with_a_card() {
        let raw = br#"{"error":{"code":"owner_only","hint":"Only the owner."},"permission":{"command":"x permission request"}}"#;
        let d = json::parse(std::str::from_utf8(raw).unwrap()).unwrap();
        let e = refused(AGENTS, 403, Some(&d), raw);
        assert_eq!(e.exit, Exit::Refused);
        assert!(e
            .stderr
            .ends_with("with HTTP 403: Only the owner.\nAsk with: x permission request\n"));
        let e = refused(AGENTS, 500, None, b"  \n");
        assert!(e
            .stderr
            .ends_with("answered /cli/agents/list with HTTP 500: no body\n"));
        assert_eq!(e.exit, Exit::Error);
    }
}
