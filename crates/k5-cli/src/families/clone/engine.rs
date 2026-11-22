//! The clone engine (`_clone_py` in `cli/src/clone.sh`): `preview`,
//! `push` (`clone to`), `pull` (`clone from`), `here` (and a bare
//! `<ws> --dest`), `show` and `approve`, as its Python runs them: the same
//! requests in the same order, the same lines on stderr and stdout, the
//! same exits, and the same tracebacks where its Python raises.
//!
//! Secrets: this server's token and the remote token are the engine's
//! own (never argv, never the environment, never a child: the binary
//! starts none). A typed password is read from the terminal only
//! ([`super::tty::password`]), spliced into its request body as the last
//! key with no JSON value holding it, and wiped once sent.

use super::net::{self, Sent};
use super::pyurl::{self, Miss};
use super::rule;
use super::tty;
use crate::families::heartbeat::py::{self as p, dget_v, index, iter};
use crate::families::{ljust, s};
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::prompt::Secret;
use crate::transport::urllib::{self, Class, Method, Opener, Outcome, Request};
use k2_client::{cli_name, product_name};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What ends an engine run early.
#[derive(Debug)]
pub enum Stop {
    /// `Fail(code, hint, rc, plain)`: its line on stderr, exit `rc`.
    Fail {
        code: Value,
        hint: Value,
        rc: i32,
        plain: bool,
    },
    /// `KeyboardInterrupt` (Ctrl-C, or the end of the input at a prompt).
    Cancelled,
    /// An exception nothing catches, as `Class: message`: the traceback,
    /// exit 1.
    Raised(String),
    /// `SystemExit(n)`: nothing printed (SIGTERM or SIGHUP in a download).
    Exit(i32),
}

impl Stop {
    pub fn fail(code: &str, hint: impl AsRef<str>, rc: i32) -> Stop {
        Stop::Fail {
            code: s(code),
            hint: s(hint.as_ref()),
            rc,
            plain: false,
        }
    }

    pub fn needs(hint: &str) -> Stop {
        Stop::fail("confirm_needs_a_person", hint, 3)
    }
}

impl From<String> for Stop {
    fn from(e: String) -> Stop {
        Stop::Raised(e)
    }
}

pub type E<T> = Result<T, Stop>;

/// `K2C_OPTS`, as the engine reads and `approve` updates it.
#[derive(Debug, Clone)]
pub struct EOpts {
    pub no_threads: bool,
    pub own: bool,
    pub live_only: bool,
    pub env_files: bool,
    pub yes: bool,
    pub json: bool,
    pub copy: Vec<Value>,
    pub blank: Vec<Value>,
    pub skip: Vec<Value>,
    /// `threadsSince`: an int from the command line, any value from a
    /// proposal; `Null` when absent.
    pub threads_since: Value,
    pub dest: Value,
    pub proposal: Value,
    pub proposal_summary: Value,
}

/// Which server a call goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Local,
    Remote,
}

pub struct Engine {
    /// `BASE_URL`, as the shell's `_k2_daemon_base` writes it.
    pub local_url: String,
    pub opener: Opener,
    pub remote_url: String,
    pub remote_token: Secret,
    pub vsock: PathBuf,
    pub k2_home: PathBuf,
    pub tty: bool,
    pub opts: EOpts,
    pub clone_id: String,
    pub approved: Vec<Value>,
    pub version: String,
}

const CHUNK: usize = 8 * 1024 * 1024;

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `d[k] = v`: replaces the value in place, or adds the key last.
fn set(v: &mut Value, k: &str, x: Value) {
    if let Value::Object(pairs) = v {
        if let Some(slot) = pairs.iter_mut().find(|(key, _)| key.as_str() == Some(k)) {
            slot.1 = x;
        } else {
            pairs.push((Text::from(k), x));
        }
    }
}

fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).unwrap_or_default()
}

fn st(v: &Value) -> String {
    py::str_of(v)
}

/// `v.get(k)` truthy.
fn truthy_get(v: &Value, k: &str) -> E<bool> {
    Ok(py::truthy(&dget_v(v, k)?))
}

/// `x or default`.
fn or(x: Value, d: Value) -> Value {
    p::or(x, d)
}

/// `"%-<w>s" % v`.
fn pad(v: &Value, w: usize) -> String {
    ljust(&st(v), w)
}

/// A value that must be a string (`str` methods on it): its text, or the
/// `AttributeError` Python raises for `method`.
fn need_str(v: &Value, method: &str) -> E<String> {
    match v {
        Value::Str(_) => Ok(st(v)),
        other => Err(Stop::Raised(format!(
            "AttributeError: '{}' object has no attribute '{method}'",
            py::type_name(other)
        ))),
    }
}

/// `"<a>" + v` where `v` must be a string.
fn plus(a: &str, v: &Value) -> E<String> {
    Ok(p::str_plus(a, v)?)
}

/// `x in v` (a list, a string, a dict's keys).
fn py_in(x: &str, v: &Value) -> E<bool> {
    Ok(p::has(v, x)?)
}

/// `json.dumps(v)` with the password spliced in as the last key: the
/// text of `body` (an object) with `, "password": "<pw>"` before its
/// closing brace, `pw` escaped as `ensure_ascii` writes it. No JSON value
/// or string ever holds the password.
fn with_password(body: &Value, pw: &Secret) -> Vec<u8> {
    let head = dumps(body);
    let mut out: Vec<u8> = Vec::with_capacity(head.len() + pw.expose().len() * 6 + 16);
    out.extend_from_slice(&head.as_bytes()[..head.len() - 1]);
    if head.len() > 2 {
        out.extend_from_slice(b", ");
    }
    out.extend_from_slice(b"\"password\": \"");
    let text = String::from_utf8_lossy(pw.expose());
    for c in text.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            ' '..='~' => out.push(c as u8),
            c => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.extend_from_slice(format!("\\u{u:04x}").as_bytes());
                }
            }
        }
    }
    if let std::borrow::Cow::Owned(mut t) = text {
        // SAFETY: zeroes are valid UTF-8; the string is dropped right after.
        for b in unsafe { t.as_bytes_mut() } {
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
    out.extend_from_slice(b"\"}");
    out
}

/// `refusal(status, v)`.
fn refusal(status: u16, v: &Value) -> Stop {
    let is_obj = matches!(v, Value::Object(_));
    let e = if is_obj {
        dget_v(v, "error").unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let (code, hint) = if matches!(e, Value::Object(_)) {
        (
            p::dget_or(&e, "code", s("error")).unwrap_or(Value::Null),
            p::dget_or(&e, "hint", s("")).unwrap_or(Value::Null),
        )
    } else {
        let get = |k: &str| {
            if is_obj {
                dget_v(v, k).unwrap_or(Value::Null)
            } else {
                Value::Null
            }
        };
        let code = or(get("code"), s("error"));
        let estr = if matches!(e, Value::Str(_)) {
            e.clone()
        } else {
            s("")
        };
        let hint = or(or(get("hint"), estr), s(&format!("refused ({status})")));
        (code, hint)
    };
    Stop::Fail {
        code,
        hint,
        rc: if matches!(status, 400 | 413) { 2 } else { 3 },
        plain: false,
    }
}

/// Is this exception an `OSError` (caught by the engine's `except
/// (URLError, OSError)`)?
fn os_class(c: Class) -> bool {
    !matches!(
        c,
        Class::IncompleteRead | Class::BadStatusLine | Class::LineTooLong | Class::HttpException
    )
}

/// `json.loads(raw or b"{}")`, a body that isn't JSON read as call()
/// reads it: `{"error": raw[:200].decode(errors="replace")}`.
fn loads_or_error(raw: &[u8]) -> Value {
    if raw.is_empty() {
        return Value::Object(Vec::new());
    }
    let text = std::str::from_utf8(raw)
        .ok()
        .map(|t| t.strip_prefix('\u{feff}').unwrap_or(t));
    match text.and_then(|t| json::parse(t).ok()) {
        Some(v) => v,
        None => obj(vec![(
            "error",
            s(&String::from_utf8_lossy(&raw[..raw.len().min(200)])),
        )]),
    }
}

/// The refusal for a URL the binary doesn't send to (nothing sent).
pub fn unsendable(base: &str) -> Stop {
    Stop::fail(
        "usage",
        format!("{base} isn't an address {} sends to itself", cli_name!()),
        2,
    )
}

/// The person's proxy for a request to `t`, read as the shell's Python
/// reads it (one proxy rule for every urllib-style family),
/// or the refusal for a proxy setting the binary doesn't read (nothing
/// sent).
pub fn route_of(t: &pyurl::Target, base: &str) -> E<Option<crate::transport::proxy::Proxy>> {
    use crate::transport::proxy::Route;
    match crate::transport::remote::python_route(&t.host, &t.host_header, t.tls) {
        Route::Direct => Ok(None),
        Route::Via(p) => Ok(Some(p)),
        // A proxy URL of another scheme is named (the named difference
        // python-proxy-other-scheme).
        Route::Shell => Err(Stop::fail(
            "usage",
            crate::transport::proxy::python::unsupported_reason(
                &crate::transport::remote::python_proxy_env(),
            )
            .unwrap_or_else(|| {
                format!(
                    "{} won't send to {base}: a proxy setting it doesn't read (an http:// proxy only)",
                    cli_name!()
                )
            }),
            2,
        )),
    }
}

impl Engine {
    fn json_out(&self) -> bool {
        self.opts.json
    }

    /// `say(msg)`: a line on stderr, unless `--json`.
    pub fn say(&self, msg: &str) {
        if !self.json_out() {
            crate::out::stderr_line(msg);
        }
    }

    fn side_url(&self, w: Which) -> &str {
        match w {
            Which::Local => &self.local_url,
            Which::Remote => &self.remote_url,
        }
    }

    /// `https_gate(side, password)`.
    pub fn https_gate(&self, w: Which, password: bool) -> E<()> {
        match rule::url_refusal(self.side_url(w), password) {
            Some(line) => Err(Stop::Fail {
                code: s("https_required"),
                hint: s(&line),
                rc: 3,
                plain: true,
            }),
            None => Ok(()),
        }
    }

    /// `call(side, method, route, body, params, timeout)`; `data` is the
    /// body's JSON text for a POST.
    pub fn call(
        &self,
        w: Which,
        method: Method,
        route: &str,
        data: Option<Vec<u8>>,
        params: &[(&str, Value)],
        timeout: u64,
    ) -> E<Value> {
        let name = match w {
            Which::Local => "local",
            Which::Remote => "remote",
        };
        let base = self.side_url(w).to_string();
        if base.is_empty() {
            return Err(Stop::fail("usage", format!("no {name} server"), 2));
        }
        if w == Which::Remote {
            self.https_gate(w, false)?;
        }
        let q: Vec<(String, String)> = params
            .iter()
            .filter(|(_, v)| !matches!(v, Value::Null))
            .map(|(k, v)| (k.to_string(), st(v)))
            .collect();
        let unreachable = |reason: &str| {
            Stop::fail(
                "daemon_unreachable",
                format!("{name} server {base}: {reason}"),
                3,
            )
        };
        // `base + route + ("?" + urlencode(q) if q else "")`, read as
        // urllib reads it.
        let mut full = format!("{base}{route}");
        if !q.is_empty() {
            full.push('?');
            full.push_str(&urllib::urlencode(&q));
        }
        let t = match pyurl::target(&full) {
            Ok(t) => t,
            Err(Miss::NoHost) => return Err(unreachable("no host given")),
            Err(Miss::Raised(e)) => return Err(Stop::Raised(e)),
            Err(Miss::Unsendable) => return Err(unsendable(&base)),
        };
        let via = route_of(&t, &base)?;
        let timeout_d = Duration::from_secs(timeout);
        let token = match w {
            Which::Local => self.opener.token().map(str::to_string).unwrap_or_default(),
            Which::Remote => String::from_utf8_lossy(self.remote_token.expose()).into_owned(),
        };
        let mut req = Request::new(method, t.selector.clone(), timeout_d);
        if method == Method::Post {
            req = req.data(data.unwrap_or_else(|| b"{}".to_vec()));
        }
        if w == Which::Remote {
            req = req.header("User-Agent", self.opener.user_agent.clone());
        }
        req = req.header("Content-Type", "application/json");
        if !token.is_empty() {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        let mut bytes = match w {
            Which::Local => {
                let mut op = self.opener.clone();
                op.netloc = t.host_header.clone();
                op.encode(&req)
            }
            Which::Remote => urllib::encode_plain(&t.host_header, &req),
        };
        let sent = net::over_tcp(&t, via.as_ref(), &bytes, timeout_d);
        net::wipe(&mut req);
        net::wipe_bytes(&mut bytes);
        let a = match sent {
            Sent::Stopped => return Err(tty::take().unwrap_or(Stop::Cancelled)),
            Sent::Done(a) => a,
        };
        if w == Which::Local {
            crate::transport::skew::note(&self.version, a.server_version.as_deref());
        }
        let (status, raw) = match a.outcome {
            Outcome::Answer { status, body } => match body {
                Ok(b) => (status, b),
                Err(r) if os_class(r.class) => return Err(unreachable(r.reason_text())),
                Err(r) => return Err(Stop::Raised(r.traceback_line())),
            },
            Outcome::HttpError { code, body } => {
                let url = format!("{base}{route}");
                if let Some(r) = urllib::redirect_refusal(&url, code, a.location.as_deref()) {
                    return Err(Stop::fail("redirect_refused", r.line, 3));
                }
                match body {
                    Ok(b) => (code, b),
                    Err(r) => return Err(Stop::Raised(r.traceback_line())),
                }
            }
            Outcome::Raised(r) if os_class(r.class) => return Err(unreachable(r.reason_text())),
            Outcome::Raised(r) => return Err(Stop::Raised(r.traceback_line())),
        };
        let v = loads_or_error(&raw);
        if status != 200 {
            return Err(refusal(status, &v));
        }
        Ok(v)
    }

    /// `call` with a JSON body.
    fn post(&self, w: Which, route: &str, body: &Value, timeout: u64) -> E<Value> {
        self.call(
            w,
            Method::Post,
            route,
            Some(dumps(body).into_bytes()),
            &[],
            timeout,
        )
    }

    fn get(&self, w: Which, route: &str, params: &[(&str, Value)], timeout: u64) -> E<Value> {
        self.call(w, Method::Get, route, None, params, timeout)
    }

    /// `vault_sock(route, body)`.
    fn vault_sock(&self, route: &str, data: Vec<u8>) -> E<Value> {
        let vs = self.vsock.display().to_string();
        let mut req = Request::new(
            Method::Post,
            format!("/cli/vault/{route}"),
            Duration::from_secs(180),
        )
        .data(data)
        .header("Content-Type", "application/json")
        .header("X-K2-Tty", if self.tty { "1" } else { "0" });
        for (n, v) in &self.opener.handover {
            req = req.header(n.clone(), v.clone());
        }
        let bytes = urllib::encode_http_client("localhost", &req);
        let r = net::over_unix(&self.vsock, &bytes, Duration::from_secs(180));
        net::wipe(&mut req);
        let unreachable =
            |e: &str| Stop::fail("vault_unreachable", format!("vault socket {vs}: {e}"), 3);
        let a = match r {
            Err(e) => return Err(unreachable(&e.text())),
            Ok(Sent::Stopped) => return Err(tty::take().unwrap_or(Stop::Cancelled)),
            Ok(Sent::Done(a)) => a,
        };
        let (status, raw) = match a.outcome {
            Outcome::Answer { status, body } | Outcome::HttpError { code: status, body } => {
                match body {
                    Ok(b) => (status, b),
                    Err(r) if os_class(r.class) => return Err(unreachable(&r.text())),
                    Err(r) => return Err(Stop::Raised(r.traceback_line())),
                }
            }
            Outcome::Raised(r) if os_class(r.class) => return Err(unreachable(&r.text())),
            Outcome::Raised(r) => return Err(Stop::Raised(r.traceback_line())),
        };
        let v = if raw.is_empty() {
            Value::Object(Vec::new())
        } else {
            p::loads(&String::from_utf8_lossy(&raw))?
        };
        if status != 200 {
            return Err(refusal(status, &v));
        }
        Ok(v)
    }

    /// `need_vault_sock()`.
    fn need_vault_sock(&self) -> E<()> {
        use std::os::unix::fs::FileTypeExt;
        let ok = !self.vsock.as_os_str().is_empty()
            && std::fs::metadata(&self.vsock).is_ok_and(|m| m.file_type().is_socket());
        if ok {
            return Ok(());
        }
        Err(Stop::fail(
            "vault_unreachable",
            format!(
                "the vault socket {} isn't there (is the {} daemon running on this computer?); a confirm here goes only through it",
                self.vsock.display(),
                product_name!()
            ),
            3,
        ))
    }

    fn password(&self, prompt: &str) -> E<Secret> {
        tty::password(self.tty, prompt)
    }

    fn no_method_local() -> Stop {
        Stop::fail(
            "no_confirm_method",
            format!(
                "set a vault password first: {} vault confirm-password set (from a terminal outside {})",
                cli_name!(),
                product_name!()
            ),
            3,
        )
    }

    /// `confirm(LOCAL, action, secret_id, label)`: a fresh vault confirm
    /// token on this computer's daemon (the only side the engine asks).
    fn confirm_local(&self, action: &str, secret_id: &Value, label: &str) -> E<Value> {
        self.need_vault_sock()?;
        let start = obj(vec![("action", s(action))]);
        let methods = or(
            dget_v(
                &self.vault_sock("confirm/start", dumps(&start).into_bytes())?,
                "methods",
            )?,
            Value::Array(Vec::new()),
        );
        let mut body = obj(vec![("action", s(action)), ("secretId", secret_id.clone())]);
        let data = if py_in("local-auth", &methods)? {
            self.say(&format!(
                "Approve on this Mac (Touch ID or your login password): {label}"
            ));
            set(&mut body, "method", s("local-auth"));
            set(&mut body, "label", s(label));
            dumps(&body).into_bytes()
        } else if py_in("connect-password", &methods)? {
            set(&mut body, "method", s("connect-password"));
            let pw = self.password(&format!("Your password on {} ({label}): ", self.local_url))?;
            with_password(&body, &pw)
        } else if py_in("vault-password", &methods)? {
            set(&mut body, "method", s("vault-password"));
            let pw = self.password(&format!("Vault password ({label}): "))?;
            with_password(&body, &pw)
        } else {
            return Err(Self::no_method_local());
        };
        let r = self.vault_sock("confirm/finish", data)?;
        Ok(index(&r, "token")?)
    }

    /// `vault_target(side, label)`.
    fn vault_target(&self, w: Which, label: &str) -> E<Value> {
        let r = match self.get(w, "/cli/vault/recipient", &[], 120) {
            Ok(r) => r,
            Err(Stop::Fail { code, .. }) => {
                self.say(&format!(
                    "(secrets stay behind: {label}'s vault answered {})",
                    st(&code)
                ));
                return Ok(Value::Null);
            }
            Err(e) => return Err(e),
        };
        if !truthy_get(&r, "recipient")? || !truthy_get(&r, "vaultId")? {
            return Ok(Value::Null);
        }
        Ok(obj(vec![
            ("recipient", index(&r, "recipient")?),
            ("vault_id", index(&r, "vaultId")?),
            ("label", s(label)),
        ]))
    }

    /// `carry_ask(path, target, choices, env_files)`.
    fn carry_ask(&self, path: &Value, target: &Value, choices: &Value, env: bool) -> Value {
        let mut a = obj(vec![
            ("project_path", path.clone()),
            ("clone_id", s(&self.clone_id)),
            ("secrets", choices.clone()),
            ("own_box_move", Value::Bool(self.opts.own)),
            ("send_env_files", Value::Bool(env)),
        ]);
        if py::truthy(target) {
            set(&mut a, "vault_target", target.clone());
        }
        a
    }

    /// `carry_confirm_call(side, ask, pv, proposal)`.
    fn carry_confirm_call(&self, w: Which, ask: &Value, pv: &Value, proposal: &Value) -> E<Value> {
        let local = w == Which::Local;
        if local {
            self.need_vault_sock()?;
        }
        let conf = or(
            dget_v(&or(dget_v(pv, "secrets")?, obj(vec![])), "confirm")?,
            obj(vec![]),
        );
        let mut reason = or(dget_v(&conf, "reason")?, s("copy secrets in a clone"));
        if py::truthy(proposal) {
            reason = s(&plus("approve the clone proposal and ", &reason)?);
        }
        let reason = st(&reason);
        if local {
            let start = obj(vec![("action", s("clone-carry"))]);
            let methods = or(
                dget_v(
                    &self.vault_sock("confirm/start", dumps(&start).into_bytes())?,
                    "methods",
                )?,
                Value::Array(Vec::new()),
            );
            let mut body = obj(vec![
                ("action", s("clone-carry")),
                ("clonePlan", ask.clone()),
            ]);
            if py::truthy(proposal) {
                set(&mut body, "proposal", proposal.clone());
            }
            let data = if py_in("local-auth", &methods)? {
                self.say(&format!(
                    "Approve on this Mac (Touch ID or your login password): {reason}"
                ));
                set(&mut body, "method", s("local-auth"));
                dumps(&body).into_bytes()
            } else if py_in("vault-password", &methods)? {
                set(&mut body, "method", s("vault-password"));
                let pw = self.password(&format!("Vault password ({reason}): "))?;
                with_password(&body, &pw)
            } else {
                return Err(Self::no_method_local());
            };
            return self.vault_sock("confirm/finish", data);
        }
        self.https_gate(w, true)?;
        let methods = or(dget_v(&conf, "methods")?, Value::Array(Vec::new()));
        let mut body = ask.clone();
        if py::truthy(proposal) {
            set(&mut body, "proposal", proposal.clone());
        }
        let url = self.side_url(w).to_string();
        let data = if py_in("connect-password", &methods)? {
            set(&mut body, "method", s("connect-password"));
            let pw = self.password(&format!("Your password on {url} ({reason}): "))?;
            with_password(&body, &pw)
        } else if py_in("vault-password", &methods)? {
            set(&mut body, "method", s("vault-password"));
            let pw = self.password(&format!("Vault password on {url} ({reason}): "))?;
            with_password(&body, &pw)
        } else {
            return Err(Stop::fail(
                "no_confirm_method",
                format!("{url} offers no password confirm for you (an Owner or Admin login, or a vault password, is needed)"),
                3,
            ));
        };
        self.call(
            w,
            Method::Post,
            "/cli/clone/vault/confirm",
            Some(data),
            &[],
            120,
        )
    }

    /// `approve_proposal(approve_token)`.
    fn approve_proposal(&mut self, approve_token: Value) -> E<()> {
        let pid = self.opts.proposal.clone();
        self.post(
            Which::Local,
            "/cli/clone-proposal/approve",
            &obj(vec![
                ("proposal", pid.clone()),
                ("confirmToken", approve_token),
            ]),
            120,
        )?;
        if !self.approved.contains(&pid) {
            self.approved.push(pid);
        }
        Ok(())
    }

    /// `threads_value()`.
    fn threads_value(&self) -> E<Value> {
        if self.opts.no_threads {
            return Ok(s("none"));
        }
        if py::truthy(&self.opts.threads_since) {
            let n = p::int(&self.opts.threads_since)?;
            return Ok(obj(vec![("sinceDays", Value::Int(n.to_string()))]));
        }
        Ok(s("all"))
    }

    /// `ask_choice(row, default)`.
    fn ask_choice(&self, row: &Value, copyable: bool, default: &Value) -> E<Value> {
        let allowed: &[&str] = if copyable {
            &["copy", "blank", "skip"]
        } else {
            &["blank", "skip"]
        };
        let note = if truthy_get(row, "note")? {
            plus(" \u{2014} ", &index(row, "note")?)?
        } else {
            String::new()
        };
        let name = index(row, "name")?;
        let kind = p::dget_or(row, "kind", s(""))?;
        let t = tty::Choice::open()?;
        loop {
            t.write(&format!(
                "  {} {} {} [{}]{note}: ",
                pad(&name, 24),
                pad(&kind, 10),
                allowed.join("/"),
                st(default)
            ))?;
            let a = t.readline()?.trim().to_lowercase();
            if a.is_empty() {
                return Ok(default.clone());
            }
            if let Some(c) = allowed.iter().find(|c| c.starts_with(&a)) {
                return Ok(s(c));
            }
            t.write(&format!("    choose {}\n", allowed.join(", ")))?;
        }
    }

    /// `choose_secrets(preview)`.
    fn choose_secrets(&self, pv: &Value) -> E<Value> {
        let sec = or(dget_v(pv, "secrets")?, obj(vec![]));
        let rows = if truthy_get(&sec, "available")? {
            or(dget_v(&sec, "rows")?, Value::Array(Vec::new()))
        } else {
            Value::Array(Vec::new())
        };
        let rows = iter(&rows)?;
        let can_copy = truthy_get(pv, "callerCanCopySecrets")?;
        let mut flags: Vec<(Value, &str)> = Vec::new();
        for (key, list) in [
            ("copy", &self.opts.copy),
            ("blank", &self.opts.blank),
            ("skip", &self.opts.skip),
        ] {
            for n in list {
                if let Some(f) = flags.iter_mut().find(|(x, _)| x == n) {
                    if f.1 != key {
                        return Err(Stop::fail(
                            "usage",
                            format!("'{}' is named twice with different choices", st(n)),
                            2,
                        ));
                    }
                    f.1 = key;
                } else {
                    flags.push((n.clone(), key));
                }
            }
        }
        if !flags.is_empty() && rows.is_empty() {
            return Err(Stop::fail(
                "clone_secrets_unavailable",
                "no secrets to choose here (none, or the destination's vault can't take a carry): drop --copy/--blank/--skip",
                2,
            ));
        }
        let mut names = Vec::new();
        for r in &rows {
            names.push(index(r, "name")?);
        }
        for (n, _) in &flags {
            if !names.contains(n) {
                return Err(Stop::fail(
                    "usage",
                    format!(
                        "no secret '{}' in this agent's plan ({} clone preview)",
                        st(n),
                        cli_name!()
                    ),
                    2,
                ));
            }
        }
        let mut choices: Vec<(Value, Value)> = Vec::new();
        let put = |choices: &mut Vec<(Value, Value)>, k: Value, v: Value| {
            if let Some(slot) = choices.iter_mut().find(|(x, _)| *x == k) {
                slot.1 = v;
            } else {
                choices.push((k, v));
            }
        };
        for r in &rows {
            let mut d = or(or(dget_v(r, "choice")?, dget_v(r, "default")?), s("skip"));
            if d == s("copy")
                && !(can_copy && py::truthy(&p::dget_or(r, "copyable", Value::Bool(false))?))
            {
                d = s("blank");
            }
            let name = index(r, "name")?;
            let pick = flags
                .iter()
                .find(|(x, _)| *x == name)
                .map(|(_, k)| s(k))
                .unwrap_or(d);
            put(&mut choices, name, pick);
        }
        let get = |choices: &Vec<(Value, Value)>, k: &Value| {
            choices
                .iter()
                .find(|(x, _)| x == k)
                .map(|(_, v)| v.clone())
                .unwrap_or(Value::Null)
        };
        if !rows.is_empty() && flags.is_empty() && self.tty {
            self.say("Secrets (names only). Enter keeps the default.");
            for r in &rows {
                let name = index(r, "name")?;
                let copyable =
                    py::truthy(&p::dget_or(r, "copyable", Value::Bool(false))?) && can_copy;
                let now = get(&choices, &name);
                let c = self.ask_choice(r, copyable, &now)?;
                put(&mut choices, name, c);
            }
        }
        for r in &rows {
            let name = index(r, "name")?;
            if get(&choices, &name) == s("copy")
                && !py::truthy(&p::dget_or(r, "copyable", Value::Bool(false))?)
            {
                let note = or(
                    dget_v(r, "note")?,
                    s("copying secrets needs an Owner or Admin"),
                );
                return Err(Stop::fail(
                    if can_copy {
                        "usage"
                    } else {
                        "clone_secrets_owner_or_admin"
                    },
                    format!("'{}' can't be copied by you ({})", st(&name), st(&note)),
                    if can_copy { 2 } else { 3 },
                ));
            }
        }
        Ok(Value::Object(
            choices
                .into_iter()
                .map(|(k, v)| (Text::from(st(&k).as_str()), v))
                .collect(),
        ))
    }

    /// `preview(side, ws, target)`.
    pub fn preview(&self, w: Which, ws: &str, target: &Value) -> E<Value> {
        let mut body = if ws.starts_with('/') {
            obj(vec![("project_path", s(ws))])
        } else {
            obj(vec![("workspace", s(ws))])
        };
        if py::truthy(&self.opts.threads_since) {
            let n = p::int(&self.opts.threads_since)?;
            set(&mut body, "sinceDays", Value::Int(n.to_string()));
        }
        set(&mut body, "clone_id", s(&self.clone_id));
        set(&mut body, "own_box_move", Value::Bool(self.opts.own));
        if py::truthy(target) {
            set(&mut body, "vault_target", target.clone());
        }
        self.post(w, "/cli/clone/preview", &body, 600)
    }

    /// `env_files_wanted(pv)`.
    fn env_files_wanted(&self, pv: &Value) -> E<bool> {
        if !self.opts.env_files {
            return Ok(false);
        }
        if !truthy_get(pv, "envFiles")? {
            self.say("(no .env files to send)");
            return Ok(false);
        }
        Ok(true)
    }

    fn copies(choices: &Value) -> bool {
        matches!(choices, Value::Object(pairs) if pairs.iter().any(|(_, c)| *c == s("copy")))
    }

    /// `bundle_body(side, pv, choices, target, with_vault)`.
    fn bundle_body(
        &mut self,
        w: Which,
        pv: &Value,
        choices: &Value,
        target: &Value,
        with_vault: bool,
    ) -> E<Value> {
        let path = index(&index(pv, "workspace")?, "path")?;
        let env = self.env_files_wanted(pv)?;
        let mut body = obj(vec![
            ("project_path", path.clone()),
            ("live_only", Value::Bool(self.opts.live_only)),
            ("threads", self.threads_value()?),
            ("clone_id", s(&self.clone_id)),
            ("carry_secrets", Value::Bool(env)),
        ]);
        if py::truthy(&self.opts.proposal) {
            set(&mut body, "proposal_id", self.opts.proposal.clone());
        }
        let empty = obj(vec![]);
        let ask = self.carry_ask(
            &path,
            if with_vault { target } else { &Value::Null },
            if with_vault { choices } else { &empty },
            env,
        );
        if with_vault && py::truthy(target) {
            set(&mut body, "vault_target", target.clone());
            set(&mut body, "secrets", choices.clone());
            set(&mut body, "own_box_move", Value::Bool(self.opts.own));
        }
        let copies = with_vault && Self::copies(choices);
        if copies && !py::truthy(target) {
            return Err(Stop::fail(
                "clone_carry_unavailable",
                format!(
                    "the destination can't take a vault carry (older {} or its vault is closed); choose blank or skip",
                    product_name!()
                ),
                3,
            ));
        }
        let pid = self.opts.proposal.clone();
        if py::truthy(&pid) && !self.approved.contains(&pid) {
            if copies || env {
                let r = self.carry_confirm_call(w, &ask, pv, &pid)?;
                self.approve_proposal(index(&r, "approveToken")?)?;
                set(&mut body, "confirm_token", index(&r, "token")?);
            } else {
                let summary = or(self.opts.proposal_summary.clone(), pid.clone());
                let summary = match &summary {
                    Value::Str(_) => st(&summary).chars().take(60).collect::<String>(),
                    other => {
                        return Err(Stop::Raised(format!(
                            "TypeError: can only concatenate str (not \"{}\") to str",
                            py::type_name(other)
                        )))
                    }
                };
                let tok =
                    self.confirm_local("clone-approve", &pid, &format!("approve {summary}"))?;
                self.approve_proposal(tok)?;
            }
            return Ok(body);
        }
        if copies || env {
            let r = self.carry_confirm_call(w, &ask, pv, &Value::Null)?;
            set(&mut body, "confirm_token", index(&r, "token")?);
        }
        Ok(body)
    }

    /// `upload(side, local_path, dest_dir)`.
    fn upload(&self, local_path: &Value, dest_dir: &str) -> E<Value> {
        let lp = match local_path {
            Value::Str(_) => st(local_path),
            other => {
                return Err(Stop::Raised(format!(
                    "TypeError: stat: path should be string, bytes, os.PathLike or integer, not {}",
                    py::type_name(other)
                )))
            }
        };
        let missing = |e: &std::io::Error| {
            let r = urllib::os_error(e);
            let class = match e.kind() {
                std::io::ErrorKind::NotFound => "FileNotFoundError",
                std::io::ErrorKind::PermissionDenied => "PermissionError",
                _ => "OSError",
            };
            Stop::Raised(format!("{class}: {}: {}", r.reason, py::repr(&s(&lp))))
        };
        let size = std::fs::metadata(&lp).map_err(|e| missing(&e))?.len();
        let name = lp.rsplit('/').next().unwrap_or("").to_string();
        let uid = format!("clone-cli-{}", &hex_id()[..12]);
        let mut f = std::fs::File::open(&lp).map_err(|e| missing(&e))?;
        let mut off: u64 = 0;
        loop {
            let data = read_chunk(&mut f).map_err(|e| missing(&e))?;
            let last = off + data.len() as u64 >= size;
            let mut body = obj(vec![
                ("upload_id", s(&uid)),
                ("dir", s(dest_dir)),
                ("filename", s(&name)),
                ("offset", Value::Int(off.to_string())),
                ("base64", s(&crate::out::pylib::base64::b64encode(&data))),
                ("is_last", Value::Bool(last)),
            ]);
            if off == 0 {
                set(&mut body, "total_bytes", Value::Int(size.to_string()));
            }
            let r = self.post(Which::Remote, "/cli/fs/upload-chunk", &body, 600)?;
            off += data.len() as u64;
            if last {
                let fin = dget_v(&r, "path")?;
                if !py::truthy(&fin) {
                    return Err(Stop::fail(
                        "upload_failed",
                        "the upload finished without a path",
                        3,
                    ));
                }
                return Ok(fin);
            }
        }
    }

    /// `clone_tmp()`: K2_HOME/clone-tmp as a folder of ours at 0700.
    fn clone_tmp(&self) -> E<(PathBuf, std::fs::File)> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
        let tmp = self.k2_home.join("clone-tmp");
        std::fs::create_dir_all(&self.k2_home).map_err(|e| {
            Stop::Raised(format!(
                "OSError: {}: {}",
                urllib::os_error(&e).reason,
                py::repr(&s(&self.k2_home.display().to_string()))
            ))
        })?;
        let _ = std::fs::DirBuilder::new().mode(0o700).create(&tmp);
        let unsafe_line = format!(
            "{} isn't a folder of yours (a link, or another account's); move it away and try again",
            tmp.display()
        );
        let d = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&tmp)
            .map_err(|_| Stop::fail("clone_tmp_unsafe", &unsafe_line, 3))?;
        let m = d
            .metadata()
            .map_err(|_| Stop::fail("clone_tmp_unsafe", &unsafe_line, 3))?;
        // SAFETY: getuid only reads.
        if !m.is_dir() || m.uid() != unsafe { libc::getuid() } {
            return Err(Stop::fail("clone_tmp_unsafe", &unsafe_line, 3));
        }
        let _ = d.set_permissions(std::fs::Permissions::from_mode(0o700));
        Ok((tmp, d))
    }

    /// `download(side, remote_path, size)`.
    fn download(&self, remote_path: &Value, size: &Value) -> E<String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let rp = need_str(remote_path, "rfind")?;
        let name = rp.rsplit('/').next().unwrap_or("").to_string();
        if matches!(name.as_str(), "" | "." | "..") {
            return Err(Stop::fail(
                "download_failed",
                "the remote server named no bundle file",
                3,
            ));
        }
        let (tmp, _dir) = self.clone_tmp()?;
        let part = tmp.join(format!("{name}.part"));
        let whole = tmp.join(&name);
        let _term = tty::Caught::start(libc::SIGTERM);
        let _hup = tty::Caught::start(libc::SIGHUP);
        let open = || {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&part)
        };
        let r = (|| -> E<()> {
            let mut f = match open() {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Left by a run that was killed outright (SIGKILL).
                    let _ = std::fs::remove_file(&part);
                    open().map_err(|e| {
                        Stop::Raised(format!("OSError: {}", urllib::os_error(&e).reason))
                    })?
                }
                Err(e) => {
                    return Err(Stop::Raised(format!(
                        "OSError: {}",
                        urllib::os_error(&e).reason
                    )))
                }
            };
            let mut off: u64 = 0;
            loop {
                let r = self.get(
                    Which::Remote,
                    "/cli/fs/read-range",
                    &[
                        ("path", remote_path.clone()),
                        ("offset", Value::Int(off.to_string())),
                        ("len", Value::Int(CHUNK.to_string())),
                    ],
                    600,
                )?;
                let data =
                    crate::families::mail::reads::b64decode(&or(dget_v(&r, "base64")?, s("")))?;
                f.write_all(&data).map_err(|e| {
                    Stop::Raised(format!("OSError: {}", urllib::os_error(&e).reason))
                })?;
                off += data.len() as u64;
                tty::check()?;
                if truthy_get(&r, "eof")? || data.is_empty() {
                    break;
                }
            }
            if py::truthy(size) {
                let want = p::int(size)?;
                if i128::from(off) != want {
                    return Err(Stop::fail(
                        "download_failed",
                        format!("got {off} of {want} bytes"),
                        3,
                    ));
                }
            }
            drop(f);
            std::fs::rename(&part, &whole)
                .map_err(|e| Stop::Raised(format!("OSError: {}", urllib::os_error(&e).reason)))?;
            Ok(())
        })();
        if r.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        r?;
        Ok(whole.display().to_string())
    }

    /// `remote_home(side)`.
    fn remote_home(&self) -> E<(String, String)> {
        let info = self.get(Which::Remote, "/cli/fs/info", &[], 120)?;
        let sep = or(dget_v(&info, "separator")?, s("/"));
        let home = or(dget_v(&info, "home")?, s(""));
        let home_s = need_str(&home, "rstrip")?;
        let sep_s = match &sep {
            Value::Str(_) => st(&sep),
            other => {
                return Err(Stop::Raised(format!(
                    "TypeError: rstrip arg must be None or str, not {}",
                    py::type_name(other)
                )))
            }
        };
        let home_s = home_s
            .trim_end_matches(|c: char| sep_s.contains(c))
            .to_string();
        if home_s.is_empty() {
            return Err(Stop::fail(
                "remote_home_unknown",
                "the remote server didn't say its home folder; pass --dest",
                3,
            ));
        }
        Ok((home_s, sep_s))
    }

    /// `result_json(mode, pv, choices, unpacked, extra)`.
    fn result_json(
        &self,
        mode: &str,
        pv: &Value,
        choices: &Value,
        u: &Value,
        extra: Vec<(&str, Value)>,
    ) -> E<Value> {
        let project = or(dget_v(u, "project")?, obj(vec![]));
        let mut proj = Vec::new();
        for k in ["id", "name", "path"] {
            proj.push((k, dget_v(&project, k)?));
        }
        let mut out = obj(vec![
            ("ok", Value::Bool(true)),
            ("mode", s(mode)),
            ("cloneId", s(&self.clone_id)),
            ("source", index(&index(pv, "workspace")?, "handle")?),
            ("destPath", dget_v(u, "dest_path")?),
            ("project", obj(proj)),
            ("threads", self.threads_value()?),
            ("secrets", choices.clone()),
        ]);
        for k in [
            "threads_imported",
            "threads_skipped",
            "vault_imported",
            "vault_skipped",
            "vault_grants",
            "vault_cards_posted",
            "vault_cards_deferred",
            "vault_cards_overflow",
            "vault_error",
            "threads_error",
        ] {
            let v = dget_v(u, k)?;
            let none = matches!(&v, Value::Null)
                || matches!(&v, Value::Array(a) if a.is_empty())
                || v == s("");
            if !none {
                set(&mut out, k, v);
            }
        }
        for (k, v) in extra {
            set(&mut out, k, v);
        }
        Ok(out)
    }

    /// `show_plan(pv, where)`.
    fn show_plan(&self, pv: &Value, where_: &str) -> E<()> {
        if self.json_out() {
            return Ok(());
        }
        let w = index(pv, "workspace")?;
        let h = or(dget_v(pv, "history")?, obj(vec![]));
        self.say(&format!(
            "Clone {} ({}) {where_}",
            st(&index(&w, "handle")?),
            st(&index(&w, "path")?)
        ));
        self.say(&format!(
            "  files: {} \u{b7} chat transcripts: {}",
            st(&dget_v(&h, "files")?),
            st(&dget_v(&h, "sessions")?)
        ));
        let t = or(dget_v(pv, "threads")?, obj(vec![]));
        if self.opts.no_threads {
            self.say("  Thread: left behind (--no-threads)");
        } else if truthy_get(&t, "available")? {
            let from = if truthy_get(&t, "participants")? {
                format!(" from {}", p::join(&index(&t, "participants")?, ", ")?)
            } else {
                String::new()
            };
            self.say(&format!(
                "  Thread: {} conversation(s), {} message(s){from}. Chatter never travels.",
                st(&dget_v(&t, "conversations")?),
                st(&dget_v(&t, "docs")?)
            ));
        } else {
            self.say("  Thread: brought along (counts need the Thread update on this server). Chatter never travels.");
        }
        if truthy_get(pv, "envFiles")? {
            let names = p::join(&index(pv, "envFiles")?, ", ")?;
            let tail = if self.opts.env_files {
                String::new()
            } else {
                format!(
                    " (move them into the vault: {} vault import .env)",
                    cli_name!()
                )
            };
            self.say(&format!("  .env files held back: {names}{tail}"));
        }
        Ok(())
    }

    /// `push(ws)`: `clone to`.
    pub fn push(&mut self, ws: &str) -> E<Value> {
        let label = self
            .remote_url
            .rsplit("://")
            .next()
            .unwrap_or("")
            .to_string();
        let target = self.vault_target(Which::Remote, &label)?;
        let pv = self.preview(Which::Local, ws, &target)?;
        self.show_plan(&pv, &format!("to {}", self.remote_url))?;
        let choices = self.choose_secrets(&pv)?;
        let body = self.bundle_body(Which::Local, &pv, &choices, &target, true)?;
        self.say("Building the bundle\u{2026}");
        let b = self.post(Which::Local, "/cli/clone/bundle", &body, 3600)?;
        let (home, sep) = self.remote_home()?;
        let dest = or(self.opts.dest.clone(), s(&home));
        let bp = index(&b, "bundle_path")?;
        let bps = need_str(&bp, "rfind")?;
        self.say(&format!(
            "Uploading {}\u{2026}",
            bps.rsplit('/').next().unwrap_or("")
        ));
        let remote_bundle = self.upload(&bp, &format!("{home}{sep}.k2{sep}clone-tmp"))?;
        let _ = std::fs::remove_file(&bps);
        let mut ub = obj(vec![("bundle_path", remote_bundle), ("dest_parent", dest)]);
        if truthy_get(&b, "vault_carry_b64")? {
            set(&mut ub, "vault_carry_b64", index(&b, "vault_carry_b64")?);
        }
        self.say(&format!("Unpacking on {}\u{2026}", self.remote_url));
        let u = self.post(Which::Remote, "/cli/clone/unpack", &ub, 3600)?;
        let host = s(&self.remote_url);
        self.result_json("to", &pv, &choices, &u, vec![("host", host)])
    }

    /// `pull(ws)`: `clone from`.
    pub fn pull(&mut self, ws: &str) -> E<Value> {
        let target = self.vault_target(Which::Local, "this computer")?;
        let pv = self.preview(Which::Remote, ws, &target)?;
        self.show_plan(&pv, &format!("from {} to this computer", self.remote_url))?;
        let choices = self.choose_secrets(&pv)?;
        let body = self.bundle_body(Which::Remote, &pv, &choices, &target, true)?;
        self.say(&format!("Packing on {}\u{2026}", self.remote_url));
        let job = index(
            &self.post(Which::Remote, "/cli/clone/pack", &body, 120)?,
            "job_id",
        )?;
        let deadline = std::time::Instant::now() + Duration::from_secs(3600);
        let stat = loop {
            let stat = self.get(
                Which::Remote,
                "/cli/clone/pack-status",
                &[("job_id", job.clone())],
                120,
            )?;
            let phase = dget_v(&stat, "phase")?;
            if phase == s("done") {
                break stat;
            }
            if phase == s("failed") {
                let why = or(dget_v(&stat, "error")?, s("the pack failed"));
                return Err(Stop::Fail {
                    code: s("pack_failed"),
                    hint: why,
                    rc: 3,
                    plain: false,
                });
            }
            if std::time::Instant::now() > deadline {
                return Err(Stop::fail("pack_timeout", "the pack took over an hour", 3));
            }
            sleep_watched(Duration::from_millis(500))?;
        };
        self.say(&format!(
            "Downloading {} bytes\u{2026}",
            st(&dget_v(&stat, "size_bytes")?)
        ));
        let got = (|| -> E<String> {
            let bp = index(&stat, "bundle_path")?;
            let size = or(dget_v(&stat, "size_bytes")?, Value::Int("0".into()));
            self.download(&bp, &size)
        })();
        // finally: the pack is cleaned up whatever happened.
        match self.post(
            Which::Remote,
            "/cli/clone/pack-cleanup",
            &obj(vec![("job_id", job)]),
            120,
        ) {
            Ok(_) | Err(Stop::Fail { .. }) => {}
            Err(e) => return Err(e),
        }
        let local_bundle = got?;
        let dest = or(self.opts.dest.clone(), s(&expand_home()));
        let mut ub = obj(vec![
            ("bundle_path", s(&local_bundle)),
            ("dest_parent", dest),
        ]);
        if truthy_get(&stat, "vault_carry_b64")? {
            set(&mut ub, "vault_carry_b64", index(&stat, "vault_carry_b64")?);
        }
        self.say("Unpacking here\u{2026}");
        let u = self.post(Which::Local, "/cli/clone/unpack", &ub, 3600)?;
        let host = s(&self.remote_url);
        self.result_json("from", &pv, &choices, &u, vec![("host", host)])
    }

    /// `here(ws)`: a second copy on this server.
    pub fn here(&mut self, ws: &str) -> E<Value> {
        let target = self.vault_target(Which::Local, "a second copy on this server")?;
        let pv = self.preview(Which::Local, ws, &target)?;
        self.show_plan(&pv, "to a second copy on this server")?;
        let choices = self.choose_secrets(&pv)?;
        let body = self.bundle_body(Which::Local, &pv, &choices, &target, false)?;
        let b = self.post(Which::Local, "/cli/clone/bundle", &body, 3600)?;
        let path = index(&index(&pv, "workspace")?, "path")?;
        let dest = if py::truthy(&self.opts.dest) {
            self.opts.dest.clone()
        } else {
            let ps = need_str(&path, "rstrip")?;
            s(&dirname(ps.trim_end_matches('/')))
        };
        let u = self.post(
            Which::Local,
            "/cli/clone/unpack",
            &obj(vec![
                ("bundle_path", index(&b, "bundle_path")?),
                ("dest_parent", dest),
            ]),
            3600,
        )?;
        let mut extra = Vec::new();
        let any_kept =
            matches!(&choices, Value::Object(pairs) if pairs.iter().any(|(_, c)| *c != s("skip")));
        if py::truthy(&target) && any_kept {
            let ask = self.carry_ask(&path, &target, &choices, false);
            let mut cbody = ask.clone();
            set(&mut cbody, "new_project_path", dget_v(&u, "dest_path")?);
            if Self::copies(&choices) {
                let r = self.carry_confirm_call(Which::Local, &ask, &pv, &Value::Null)?;
                set(&mut cbody, "confirm_token", index(&r, "token")?);
            }
            extra.push((
                "vault",
                self.post(Which::Local, "/cli/clone/vault/copy", &cbody, 600)?,
            ));
        }
        self.result_json("here", &pv, &choices, &u, extra)
    }

    /// `find_proposal(token)`.
    pub fn find_proposal(&self, token: &str) -> E<Value> {
        let list = self.get(
            Which::Local,
            "/cli/clone-proposal/list",
            &[("status", s("all"))],
            120,
        )?;
        let rows = iter(&or(dget_v(&list, "proposals")?, Value::Array(Vec::new())))?;
        let mut hit = Vec::new();
        for r in &rows {
            if index(r, "id")? == s(token) {
                hit.push(r.clone());
            }
        }
        if hit.is_empty() {
            for r in &rows {
                let id = index(r, "id")?;
                if need_str(&id, "startswith")?.starts_with(token) {
                    hit.push(r.clone());
                }
            }
        }
        if hit.len() != 1 {
            return Err(Stop::fail(
                "proposal_not_found",
                format!(
                    "no single clone proposal matches '{token}' ({} clone proposals)",
                    cli_name!()
                ),
                3,
            ));
        }
        Ok(hit.remove(0))
    }

    /// `confirm_host(target)`.
    fn confirm_host(&self, target: &Value) -> E<()> {
        let t = shown(&need_iter_str(target)?);
        let p = product_name!();
        let needs = format!(
            "this proposal sends your token to {t}: approve it from a terminal, where {p} asks first"
        );
        let Some(fd) = tty::open_tty() else {
            return Err(Stop::needs(&needs));
        };
        let url = if s(&self.remote_url) == *target {
            String::new()
        } else {
            format!(" ({})", shown(&self.remote_url))
        };
        let typed = tty::read_line(
            fd,
            &format!("Send your token to {t}?{url} The agent proposed this host. [y/N] "),
            &needs,
        );
        tty::close(fd);
        let typed = typed?;
        let a = typed.expose().trim_ascii().to_ascii_lowercase();
        if a != b"y" && a != b"yes" {
            return Err(Stop::fail(
                "not_approved",
                format!("nothing was sent to {t}; the proposal is still open"),
                3,
            ));
        }
        Ok(())
    }

    /// `approve(token)`.
    pub fn approve(&mut self, token: &str) -> E<Value> {
        let pr = self.find_proposal(token)?;
        let status = index(&pr, "status")?;
        if status != s("open") {
            return Err(Stop::fail(
                "proposal_not_open",
                format!("{} is {}", st(&index(&pr, "id")?), st(&status)),
                3,
            ));
        }
        self.say(&format!(
            "{}: {}",
            st(&index(&pr, "id")?),
            st(&index(&pr, "summary")?)
        ));
        let why = if truthy_get(&pr, "why")? {
            plus(": ", &index(&pr, "why")?)?
        } else {
            String::new()
        };
        self.say(&format!("asked by {}{why}", st(&index(&pr, "proposedBy")?)));
        let target = index(&pr, "target")?;
        if target != s("here") {
            self.confirm_host(&target)?;
        }
        let ws = dget_v(&pr, "workspacePath")?;
        if !py::truthy(&ws) {
            return Err(Stop::fail(
                "workspace_not_found",
                "the proposal's workspace isn't registered here any more",
                3,
            ));
        }
        let o = or(dget_v(&pr, "options")?, obj(vec![]));
        let secrets = or(dget_v(&o, "secrets")?, obj(vec![]));
        let names = |want: &str| -> E<Vec<Value>> {
            match &secrets {
                Value::Object(pairs) => Ok(pairs
                    .iter()
                    .filter(|(_, c)| *c == s(want))
                    .map(|(k, _)| s(k.as_str().unwrap_or_default()))
                    .collect()),
                other => Err(Stop::Raised(format!(
                    "AttributeError: '{}' object has no attribute 'items'",
                    py::type_name(other)
                ))),
            }
        };
        let id = index(&pr, "id")?;
        self.opts.proposal = id.clone();
        self.opts.proposal_summary = dget_v(&pr, "summary")?;
        self.opts.no_threads = !py::truthy(&p::dget_or(&o, "threads", Value::Bool(true))?);
        self.opts.threads_since = dget_v(&o, "sinceDays")?;
        self.opts.live_only = !py::truthy(&p::dget_or(&o, "allHistory", Value::Bool(true))?);
        if !py::truthy(&self.opts.dest) {
            self.opts.dest = dget_v(&o, "dest")?;
        }
        self.opts.copy = names("copy")?;
        self.opts.blank = names("blank")?;
        self.opts.skip = names("skip")?;
        let ws_s = need_str(&ws, "startswith")?;
        let res = if target == s("here") {
            self.here(&ws_s)
        } else {
            self.push(&ws_s)
        };
        let mut res = match res {
            Ok(r) => r,
            Err(Stop::Fail {
                code,
                hint,
                rc,
                plain,
            }) => {
                if self.approved.contains(&id) {
                    self.post(
                        Which::Local,
                        "/cli/clone-proposal/finish",
                        &obj(vec![
                            ("proposal", id.clone()),
                            ("ok", Value::Bool(false)),
                            ("detail", code.clone()),
                        ]),
                        120,
                    )?;
                }
                return Err(Stop::Fail {
                    code,
                    hint,
                    rc,
                    plain,
                });
            }
            Err(e) => return Err(e),
        };
        self.post(
            Which::Local,
            "/cli/clone-proposal/finish",
            &obj(vec![("proposal", id.clone()), ("ok", Value::Bool(true))]),
            120,
        )?;
        set(&mut res, "proposal", id);
        Ok(res)
    }
}

/// `time.sleep(t)`, stopped by Ctrl-C.
fn sleep_watched(t: Duration) -> E<()> {
    let end = std::time::Instant::now() + t;
    while std::time::Instant::now() < end {
        tty::check()?;
        std::thread::sleep(Duration::from_millis(50).min(end - std::time::Instant::now()));
    }
    tty::check()
}

/// `f.read(CHUNK)`: up to [`CHUNK`] bytes, fewer only at the end.
fn read_chunk(f: &mut std::fs::File) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut data = Vec::with_capacity(CHUNK.min(1 << 20));
    f.by_ref().take(CHUNK as u64).read_to_end(&mut data)?;
    Ok(data)
}

/// `os.path.dirname(p)`.
fn dirname(path: &str) -> String {
    let Some(i) = path.rfind('/') else {
        return String::new();
    };
    let head = &path[..=i];
    if head.bytes().all(|b| b == b'/') {
        head.to_string()
    } else {
        head.trim_end_matches('/').to_string()
    }
}

/// `os.path.expanduser("~")`.
fn expand_home() -> String {
    match std::env::var_os("HOME") {
        Some(h) => {
            let h = h.to_string_lossy().into_owned();
            let t = h.trim_end_matches('/').to_string();
            if t.is_empty() {
                "/".to_string()
            } else {
                t
            }
        }
        #[allow(deprecated)]
        None => std::env::home_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~".into()),
    }
}

/// A string to iterate (`shown(text)` walks its characters).
fn need_iter_str(v: &Value) -> E<String> {
    match v {
        Value::Str(_) => Ok(st(v)),
        Value::Array(_) | Value::Object(_) => Err(Stop::Raised(
            "TypeError: sequence item 0: expected str instance".into(),
        )),
        other => Err(Stop::Raised(format!(
            "TypeError: '{}' object is not iterable",
            py::type_name(other)
        ))),
    }
}

/// `shown(text)`: what a terminal wouldn't print as itself, spelled out.
pub fn shown(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if py::isprintable(c) {
            out.push(c);
        } else if u32::from(c) < 256 {
            out.push_str(&format!("\\x{:02x}", u32::from(c)));
        } else {
            out.push_str(&format!("\\u{:04x}", u32::from(c)));
        }
    }
    out
}

/// 16 random bytes from the system.
fn random16() -> [u8; 16] {
    use std::io::Read;
    let mut b = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    }
    b
}

/// `uuid.uuid4().hex`.
pub fn hex_id() -> String {
    let mut b = random16();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `str(uuid.uuid4())`.
pub fn uuid4() -> String {
    let h = hex_id();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// `main()`'s end for a result: `--json`'s indented object, or the lines.
pub fn print_result(res: &Value, json_out: bool) -> E<()> {
    if json_out {
        let _ = crate::out::stdout_line(&p::dumps_indent(res)?);
        return Ok(());
    }
    let host = if truthy_get(res, "host")? {
        plus(" on ", &index(res, "host")?)?
    } else {
        String::new()
    };
    let _ = crate::out::stdout_line(&format!(
        "Cloned {} \u{2192} {}{host}",
        st(&index(res, "source")?),
        st(&dget_v(res, "destPath")?)
    ));
    let secrets = dget_v(res, "secrets")?;
    if py::truthy(&secrets) {
        let mut counts: Vec<(String, usize)> = Vec::new();
        if let Value::Object(pairs) = &secrets {
            for (_, c) in pairs {
                let k = st(c);
                match counts.iter_mut().find(|(x, _)| *x == k) {
                    Some(slot) => slot.1 += 1,
                    None => counts.push((k, 1)),
                }
            }
        }
        counts.sort();
        let parts: Vec<String> = counts.iter().map(|(k, n)| format!("{k} {n}")).collect();
        let _ = crate::out::stdout_line(&format!("  secrets: {}", parts.join(" \u{b7} ")));
    }
    for k in ["threads_imported", "vault_imported", "cards_posted"] {
        if let Value::Object(pairs) = res {
            if let Some((_, v)) = pairs.iter().find(|(x, _)| x.as_str() == Some(k)) {
                let _ = crate::out::stdout_line(&format!("  {}: {}", k.replace('_', " "), st(v)));
            }
        }
    }
    Ok(())
}

/// `print(json.dumps(out, indent=2))` (preview `--json`).
pub fn print_indented(v: &Value) -> E<()> {
    let _ = crate::out::stdout_line(&p::dumps_indent(v)?);
    Ok(())
}

/// `preview`'s lines (not `--json`).
pub fn print_preview(e: &Engine, out: &Value) -> E<()> {
    e.show_plan(out, "(preview)")?;
    let sec = or(dget_v(out, "secrets")?, obj(vec![]));
    for r in iter(&or(dget_v(&sec, "rows")?, Value::Array(Vec::new())))? {
        let note = if truthy_get(&r, "note")? {
            plus(" \u{2014} ", &index(&r, "note")?)?
        } else {
            String::new()
        };
        crate::out::stderr_line(&format!(
            "  {} {} {} default {}{note}",
            pad(&index(&r, "name")?, 24),
            pad(&p::dget_or(&r, "kind", s(""))?, 10),
            pad(&p::dget_or(&r, "group", s(""))?, 9),
            st(&dget_v(&r, "default")?)
        ));
    }
    if truthy_get(&sec, "notCarried")? {
        crate::out::stderr_line(&format!(
            "  not carried: {}",
            p::join(&index(&sec, "notCarried")?, ", ")?
        ));
    }
    if !truthy_get(out, "callerCanCopySecrets")? {
        crate::out::stderr_line("  copying secrets needs an Owner or Admin");
    }
    Ok(())
}

/// The vault socket's path: `<K2 home>/vault.sock` (`_vault_sock`).
pub fn vault_sock_path(k2_home: &Path) -> PathBuf {
    k2_home.join("vault.sock")
}
