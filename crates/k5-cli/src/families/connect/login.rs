//! `k5 connect login`, as the shell CLI's `cmd_connect` and
//! `cmd_connect_login` answer it (`cli/src/connect-account.sh`): sign in
//! to the account server, pick a subdomain, bind its tunnel token to the
//! local daemon and start the tunnel.
//!
//! The steps are the shell's, in its order:
//!
//! 1. `_connect_db_resolve` ([`super::logout::resolve`]): the account
//!    server's base URL, and its line on stderr when it is not the
//!    built-in one.
//! 2. The words ([`parse`]). (Air-gap was refused before the router:
//!    [`airgap_refusal`].)
//! 3. The access token: `--token-stdin` reads one line
//!    ([`crate::prompt::read_secret_stdin`]), `--token <jwt>` prints its
//!    note. With neither: the email (`--email`, else a prompt on stderr
//!    and one line of stdin) and the password, by Python's getpass on the
//!    terminal ([`crate::prompt::getpass_tty`]), or from stdin when there
//!    is no terminal to open ([`getpass_stdin`]), then the password grant
//!    ([`sign_in`]).
//! 4. `connect-account.json`, 0600, through a temporary file
//!    ([`save_account`]); `Signed in as <email>.`.
//! 5. The account's subdomains ([`list`]), and the shell's picker
//!    ([`pick`]): `--subdomain`, the only one, or a numbered list and one
//!    line of stdin. The chosen row's tunnel token ([`tunnel_token`]).
//! 6. `_connect_require_daemon` ([`require_daemon`]), the device's id
//!    (`tunnel.json`'s, else a new UUID 4) and label (the host name), the
//!    best-effort claim ([`claim`]).
//! 7. `cli_send cli_post_json /cli/tunnel/config` with the token in the
//!    body, `cli_send cli_post /cli/tunnel/start`, each answer read as the
//!    shell's Python reads it, then `✓ live at <url>` only when the start
//!    answered a JSON object with no error (no answer is an error line,
//!    exit 1); the EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The account server is a Python block's `urlopen` in the shell
//! ([`crate::transport::urllib::open_url`]): http.client's request, no
//! hand-over headers, TLS for `https://` by the shared client. The tokens
//! and the password are only ever in a request's body or its
//! `Authorization` header, and in the account file: never on argv, in a
//! URL, a log line or a child's environment (no child runs).
//!
//! Where it differs: a failed TLS handshake's words are the binary's,
//! where the shell's Python prints OpenSSL's (the named difference
//! python-tls-error-text); the exit code and the nothing-sent are the
//! same. A password read with no terminal: Python's getpass warns with
//! its own file and line, the binary with one line of its own that says
//! the same (getpass-no-tty-warning). A subdomain list above what one
//! environment variable may hold: the shell's picker can't start
//! ("Argument list too long"), the binary picks (picker-env-size).
//!
//! Handed to the shell ([`run`] returns `None`) before anything is
//! printed, read or sent: a base URL the binary doesn't send itself, an
//! `https://` one the person's proxy would carry, a password Python's
//! getpass would read some other way than on the terminal or from a
//! stdin that is no terminal ([`getpass_stdin_ok`]), and a daemon port
//! that is set but is no port curl takes.

use super::logout::{self, ANON_KEY, ENV_URL};
use super::{account_email, load, ACCOUNT_FILE, CONFIG, TUNNEL_FILE};
use crate::daemon::Settings;
use crate::exit::Exit;
use crate::families::{get, py_get, py_space, Raise};
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::json::decode_utf8;
use crate::out::{self, py};
use crate::prelude::{Closing, VIRTUAL_DESKTOP};
use crate::prompt::{self, AtEof, Echo, Getpass};
use crate::transport::urllib::{self, Method, Outcome, Place, Request};
use crate::transport::wire::Target;
use crate::transport::{Auth, Call, Channel, Error};
use k2_client::brand::{CLI_NAME as CLI, INFRA_ZONE, PRODUCT_NAME as PRODUCT, PUBLIC_SITE};
use k2_client::hosts::ZONE_SUFFIX;
use std::path::Path;
use std::time::Duration;

/// Where the tunnel starts.
pub const START: &str = "/cli/tunnel/start";
/// `urlopen(..., timeout=30)`.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// The rows the subdomain list asks for.
pub const SUBDOMAINS: &str =
    "/rest/v1/subdomains?select=label,status,tunnel_token,claimed_by,claimed_at,claimed_label";

/// Writes lines on stderr, then returns `exit`.
macro_rules! fail {
    ($exit:expr; $($line:expr),+ $(,)?) => {{
        $(out::stderr_line(&$line);)+
        return Err($exit);
    }};
}

/// `k2.dev`, as the account lines name it.
fn zone() -> &'static str {
    ZONE_SUFFIX.trim_start_matches('.')
}

/// `login`'s words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Words {
    pub email: String,
    pub token: String,
    /// `--token` or `--token=` was given (an empty value too).
    pub token_flag: bool,
    pub token_stdin: bool,
    pub subdomain: String,
}

/// How the words end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Words(Words),
    /// Lines on stderr, the usage after them when `usage`, then exit 2.
    Stop {
        lines: Vec<String>,
        usage: bool,
    },
}

/// `cmd_connect_login`'s loop (`-h` and `--help` never get here: the
/// front hands those lines to the shell).
pub fn parse(words: &[&str]) -> Parsed {
    let mut w = Words::default();
    let stop = |line: String, usage: bool| Parsed::Stop {
        lines: vec![line],
        usage,
    };
    let mut i = 0;
    while i < words.len() {
        let a = words[i];
        let value = |flag: &str| -> Result<String, Parsed> {
            words
                .get(i + 1)
                .map(|v| v.to_string())
                .ok_or_else(|| stop(format!("Error: {flag} needs a value"), false))
        };
        match a {
            "--email" | "--token" | "--subdomain" => {
                if a == "--token" {
                    w.token_flag = true;
                }
                let v = match value(a) {
                    Ok(v) => v,
                    Err(p) => return p,
                };
                match a {
                    "--email" => w.email = v,
                    "--token" => w.token = v,
                    _ => w.subdomain = v,
                }
                i += 2;
                continue;
            }
            "--token-stdin" => w.token_stdin = true,
            "--password" | "--pass" => {
                return Parsed::Stop {
                    lines: vec![
                        "Error: passwords must not be passed on the command line.".into(),
                        "Use the interactive prompt, or --token-stdin for scripting.".into(),
                    ],
                    usage: false,
                }
            }
            _ => {
                if let Some(v) = a.strip_prefix("--email=") {
                    w.email = v.to_string();
                } else if let Some(v) = a.strip_prefix("--token=") {
                    w.token = v.to_string();
                    w.token_flag = true;
                } else if let Some(v) = a.strip_prefix("--subdomain=") {
                    w.subdomain = v.to_string();
                } else if a.starts_with('-') {
                    return stop(format!("Error: unknown flag '{a}'"), true);
                } else {
                    return stop(format!("Error: unexpected argument '{a}'"), true);
                }
            }
        }
        i += 1;
    }
    Parsed::Words(w)
}

/// Where the account server is: the place, and the base URL's path
/// before each route. `None`: a URL the binary doesn't send itself.
pub fn place(base: &str) -> Option<(Place, String)> {
    let (t, netloc, path) = logout::target(base)?;
    let prefix = path.strip_suffix(logout::LOGOUT)?.to_string();
    let p = match t {
        Target::Tcp { host, port } => Place {
            host,
            port,
            netloc,
            tls: false,
            via: None,
        },
        Target::Tls { host, port, .. } => Place {
            host,
            port,
            netloc,
            tls: true,
            via: None,
        },
        #[cfg(unix)]
        Target::Unix(_) => return None,
    };
    Some((p, prefix))
}

/// `_connect_require_daemon`'s port text: the prelude's (the setting, else
/// `heartbeat.port`), else `daemon.port`'s (`$(cat …)`), else none.
fn daemon_port_text(s: &Settings) -> Option<String> {
    if let Some(t) = s.port_text.as_deref().filter(|t| !t.is_empty()) {
        return Some(t.to_string());
    }
    let f = s.home.join("daemon.port");
    if !f.is_file() {
        return None;
    }
    let mut t = std::fs::read(&f)
        .map(|b| b.into_iter().filter(|c| *c != 0).collect::<Vec<u8>>())
        .unwrap_or_default();
    while t.last() == Some(&b'\n') {
        t.pop();
    }
    Some(String::from_utf8_lossy(&t).into_owned()).filter(|t| !t.is_empty())
}

/// A text as Python's `str` holds it.
fn text(t: &str) -> Value {
    Value::Str(Text::from(t))
}

/// `json.dumps(v)`: Python's defaults (`", "`, `": "`, ASCII only).
fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).unwrap_or_default()
}

/// `s.strip()`.
fn strip(s: &str) -> &str {
    s.trim_matches(py_space)
}

/// `(r.get(key) or "")`, then `.strip()` when `stripped`: the text, or
/// the `AttributeError` at `line` for a value that is not a string.
fn get_text(
    pairs: &[(Text, Value)],
    key: &str,
    stripped: bool,
    line: u32,
) -> Result<String, Raise> {
    match get(pairs, key).filter(|v| py::truthy(v)) {
        None => Ok(String::new()),
        Some(v @ Value::Str(_)) => {
            let t = py::str_of(v);
            Ok(if stripped { strip(&t).to_string() } else { t })
        }
        Some(other) if stripped => Err(Raise::no_attribute(line, other, "strip")),
        Some(other) => Ok(py::str_of(other)),
    }
}

/// `json.loads(raw) if raw.strip() else <empty>`, any exception the
/// empty value too.
fn loads_or(raw: &str, empty: Value) -> Value {
    if strip(raw).is_empty() {
        return empty;
    }
    json::parse(raw).unwrap_or(empty)
}

/// `raw[:200]`: its first 200 code points.
fn first_200(raw: &str) -> String {
    raw.chars().take(200).collect()
}

/// What one `urlopen` in the shell's account blocks came to.
enum Got {
    /// It answered (2xx, or an `HTTPError`): the status and the body
    /// `.decode()`d.
    Answer { status: u16, raw: String },
    /// `except Exception as e`: `"%s" % e`.
    Failed(String),
    /// Raised in the `except HTTPError` arm, where nothing catches it.
    Raised(Raise),
}

/// `try: with urlopen(...) as resp: raw = resp.read().decode() ...
/// except urllib.error.HTTPError as e: raw = e.read().decode() ...
/// except Exception as e: ...`; `line` is the HTTPError arm's.
fn got(o: Outcome, line: u32) -> Got {
    match o {
        Outcome::Answer { status, body } => match body {
            Ok(b) => match decode_utf8(&b) {
                Ok(raw) => Got::Answer { status, raw },
                Err(f) => Got::Failed(f.message),
            },
            Err(r) => Got::Failed(r.text()),
        },
        Outcome::HttpError { code, body } => match body {
            Ok(b) => match decode_utf8(&b) {
                Ok(raw) => Got::Answer { status: code, raw },
                Err(f) => Got::Raised(Raise::at(line, f.last_line())),
            },
            Err(r) => Got::Raised(Raise::at(line, r.traceback_line())),
        },
        Outcome::Raised(r) => Got::Failed(r.text()),
    }
}

/// A traceback on stderr: exit 1.
fn raise(r: &Raise) -> Exit {
    let _ = out::write_flushed(&mut std::io::stderr().lock(), r.text().as_bytes());
    Exit::Error
}

/// The account server and its headers.
struct Account {
    place: Place,
    prefix: String,
    ua: String,
}

impl Account {
    fn request(&self, method: Method, route: &str) -> Request {
        Request::new(method, format!("{}{route}", self.prefix), TIMEOUT)
            .header("Apikey", ANON_KEY)
            .header("User-Agent", self.ua.as_str())
    }
}

/// `_connect_supabase_sign_in`: the access token, the refresh token and
/// the email, as the shell's `sed -n '1p'`, `'2p'` and `'3p'` take them
/// from what the Python printed.
fn sign_in(acc: &Account, email: &str, password: &str) -> Result<[String; 3], Exit> {
    let body = Value::Object(vec![
        (Text::from("email"), text(email)),
        (Text::from("password"), text(password)),
    ]);
    let req = acc
        .request(Method::Post, "/auth/v1/token?grant_type=password")
        .header("Content-Type", "application/json")
        .data(dumps(&body).into_bytes());
    let (status, raw) = match got(urllib::open_url(&acc.place, &req), 16) {
        Got::Answer { status, raw } => (status, raw),
        Got::Failed(e) => {
            fail!(Exit::Error; format!("Error: could not reach {} account service: {e}", zone()))
        }
        Got::Raised(r) => return Err(raise(&r)),
    };
    let d = loads_or(&raw, Value::Object(Vec::new()));
    let ok_status = (200..300).contains(&status);
    let access = if ok_status {
        py_get(&d, "access_token", 25).map_err(|r| raise(&r))?
    } else {
        None
    };
    let Some(access) = access.filter(|v| py::truthy(v)).cloned() else {
        let mut msg = None;
        for key in ["error_description", "msg", "message"] {
            let v = py_get(&d, key, 27).map_err(|r| raise(&r))?;
            if let Some(v) = v.filter(|v| py::truthy(v)) {
                msg = Some(py::str_of(v));
                break;
            }
        }
        if msg.is_none() {
            if let Some(v @ Value::Str(_)) = py_get(&d, "error", 28).map_err(|r| raise(&r))? {
                if py::truthy(v) {
                    msg = Some(py::str_of(v));
                }
            }
        }
        let msg = msg.unwrap_or_else(|| format!("Request failed ({status})"));
        fail!(Exit::Error; format!("Error: sign-in failed: {msg}"));
    };
    let Value::Object(pairs) = &d else {
        unreachable!("access_token came from a dict");
    };
    let user = get(pairs, "user").filter(|v| py::truthy(v));
    let from_user = match user {
        Some(Value::Object(u)) => get(u, "email").filter(|v| py::truthy(v)).map(py::str_of),
        _ => None,
    };
    let account_email = from_user.unwrap_or_else(|| email.to_string());
    let refresh = get(pairs, "refresh_token")
        .filter(|v| py::truthy(v))
        .map(py::str_of)
        .unwrap_or_default();
    // print() of each, then `$(…)`, then each line by its number.
    let mut printed = format!("{}\n{refresh}\n{account_email}\n", py::str_of(&access));
    while printed.ends_with('\n') {
        printed.pop();
    }
    let mut lines = printed.split('\n').map(str::to_string);
    let mut next = || lines.next().unwrap_or_default();
    Ok([next(), next(), next()])
}

/// `_connect_account_save`: the session, 0600, through `<file>.tmp`.
fn save_account(home: &Path, access: &str, refresh: &str, email: &str) -> Result<(), Exit> {
    // printf '%s\n%s' "$access" "$refresh" | ... .partition("\n")
    let joined = format!("{access}\n{refresh}");
    let (a, r) = joined.split_once('\n').unwrap_or((&joined, ""));
    let data = Value::Object(vec![
        (Text::from("access_token"), text(a)),
        (Text::from("refresh_token"), text(r)),
        (Text::from("email"), text(email)),
    ]);
    let compact = Options {
        compact: true,
        ..Options::default()
    };
    let body = json::to_string(&data, &compact).unwrap_or_default();
    let path = home.join(ACCOUNT_FILE);
    let tmp = format!("{}.tmp", path.display());
    if let Err(e) = std::fs::write(&tmp, body) {
        let f = crate::out::pylib::json::os_error(&e, &tmp);
        return Err(raise(&Raise::at(11, f.last_line())));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)) {
            let f = crate::out::pylib::json::os_error(&e, &tmp);
            return Err(raise(&Raise::at(13, f.last_line())));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let f = crate::out::pylib::json::os_error(&e, &tmp);
        return Err(raise(&Raise::at(14, f.last_line())));
    }
    Ok(())
}

/// `_connect_list_subdomains`: the rows (a list, else none).
fn list(acc: &Account, access: &str) -> Result<Vec<Value>, Exit> {
    let req = acc
        .request(Method::Get, SUBDOMAINS)
        .header("Authorization", format!("Bearer {access}"));
    let (status, raw) = match got(urllib::open_url(&acc.place, &req), 19) {
        Got::Answer { status, raw } => (status, raw),
        Got::Failed(e) => fail!(Exit::Error; format!("Error: could not list subdomains: {e}")),
        Got::Raised(r) => return Err(raise(&r)),
    };
    if !(200..300).contains(&status) {
        let d = loads_or(&raw, Value::Object(Vec::new()));
        let mut msg = None;
        for key in ["message", "msg", "error_description"] {
            if let Some(v) = py_get(&d, key, 29)
                .map_err(|r| raise(&r))?
                .filter(|v| py::truthy(v))
            {
                msg = Some(py::str_of(v));
                break;
            }
        }
        let msg = msg
            .or_else(|| Some(first_200(&raw)).filter(|t| !t.is_empty()))
            .unwrap_or_else(|| format!("HTTP {status}"));
        fail!(Exit::Error; format!("Error: list subdomains failed: {msg}"));
    }
    Ok(match loads_or(&raw, Value::Array(Vec::new())) {
        Value::Array(rows) => rows,
        _ => Vec::new(),
    })
}

/// The picker's rows: each dict row without its `tunnel_token`.
fn public(rows: &[Value]) -> Vec<Value> {
    rows.iter()
        .map(|r| match r {
            Value::Object(pairs) => Value::Object(
                pairs
                    .iter()
                    .filter(|(k, _)| *k != Text::from("tunnel_token"))
                    .cloned()
                    .collect(),
            ),
            other => other.clone(),
        })
        .collect()
}

/// Python's `int(text.strip())` for a selection.
fn py_int(t: &str) -> Option<i64> {
    let t = strip(t);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    {
        return None;
    }
    let n: i64 = digits.replace('_', "").parse().ok()?;
    Some(if neg { -n } else { n })
}

/// The picker: the chosen label (stripped). Its lines on stderr; the
/// numbered list reads one line of stdin.
fn pick(rows: &[Value], want: &str) -> Result<String, Exit> {
    let cli = CLI;
    let none = || -> Exit {
        out::stderr_line("Error: this account has no purchased subdomains.");
        out::stderr_line(&format!(
            "Buy one at {PUBLIC_SITE}/dashboard then re-run: {cli} connect login"
        ));
        Exit::Error
    };
    if rows.is_empty() {
        return Err(none());
    }
    let want = strip(want).to_lowercase();
    let mut norm: Vec<&[(Text, Value)]> = Vec::new();
    for r in rows {
        let Value::Object(pairs) = r else {
            continue;
        };
        if !get_text(pairs, "label", true, 16)
            .map_err(|r| raise(&r))?
            .is_empty()
        {
            norm.push(pairs);
        }
    }
    if norm.is_empty() {
        return Err(none());
    }
    let label_or = |p: &[(Text, Value)]| -> String {
        get(p, "label")
            .filter(|v| py::truthy(v))
            .map_or_else(|| "?".to_string(), py::str_of)
    };
    let chosen: &[(Text, Value)] = if !want.is_empty() {
        let mut found = None;
        for p in &norm {
            if get_text(p, "label", true, 29)
                .map_err(|r| raise(&r))?
                .to_lowercase()
                == want
            {
                found = Some(*p);
                break;
            }
        }
        match found {
            Some(p) => p,
            None => {
                let available: Vec<String> = norm.iter().map(|p| label_or(p)).collect();
                fail!(Exit::Error;
                    format!("Error: subdomain {} is not in your account.", py::repr(&text(&want))),
                    format!("Available: {}", available.join(", ")),
                );
            }
        }
    } else if norm.len() == 1 {
        out::stderr_line(&format!(
            "Using only subdomain: {}.{INFRA_ZONE}",
            label_or(norm[0])
        ));
        norm[0]
    } else {
        let mut listed = String::from("Your subdomains:\n");
        for (i, p) in norm.iter().enumerate() {
            let st = get_text(p, "status", true, 42).map_err(|r| raise(&r))?;
            let extra = if !st.is_empty() && st != "active" {
                format!(" ({st})")
            } else {
                String::new()
            };
            listed.push_str(&format!(
                "  {}) {}.{INFRA_ZONE}{extra}\n",
                i + 1,
                label_or(p)
            ));
        }
        listed.push_str(&format!("Pick a number [1-{}]: ", norm.len()));
        let _ = out::write_flushed(&mut std::io::stderr().lock(), listed.as_bytes());
        let line = prompt::read_line(Echo::Shown, AtEof::Keep);
        let line = String::from_utf8_lossy(line.expose()).into_owned();
        let Some(n) = py_int(&line) else {
            fail!(Exit::Error; "Error: invalid selection.");
        };
        if n < 1 || n > norm.len() as i64 {
            fail!(Exit::Error; "Error: selection out of range.");
        }
        norm[(n - 1) as usize]
    };
    let label = get_text(chosen, "label", true, 58).map_err(|r| raise(&r))?;
    if label.is_empty() {
        fail!(Exit::Error; "Error: selected subdomain is missing label or tunnel_token.");
    }
    Ok(label)
}

/// The tunnel token of the first row whose label is `label`.
fn tunnel_token(rows: &[Value], label: &str) -> Result<String, Exit> {
    for r in rows {
        let Value::Object(pairs) = r else {
            continue;
        };
        let this = get_text(pairs, "label", true, 6)
            .map_err(|e| raise(&Raise::inside(5, 6, "<genexpr>", e.exception)))?;
        if this == label {
            let tok = get_text(pairs, "tunnel_token", true, 5)
                .map_err(|e| raise(&Raise::inside(5, 5, "<genexpr>", e.exception)))?;
            if tok.is_empty() {
                break;
            }
            return Ok(tok);
        }
    }
    fail!(Exit::Error; "Error: selected subdomain is missing label or tunnel_token.");
}

/// `_connect_require_daemon`: the settings with the token it found, and
/// the port. Its lines and exit when there is none.
fn require_daemon(s: &Settings, port: Option<u16>) -> Result<(Settings, u16), Exit> {
    let mut s2 = s.clone();
    let text = daemon_port_text(s);
    if s.auth == Auth::None {
        if s.no_owner_fallback {
            for l in VIRTUAL_DESKTOP {
                out::stderr_line(l);
            }
            return Err(Exit::Refused);
        }
        for f in ["heartbeat.token", "daemon.token"] {
            let p = s.home.join(f);
            if let Ok(b) = std::fs::read(&p) {
                let mut b: Vec<u8> = b.into_iter().filter(|c| *c != 0).collect();
                while b.last() == Some(&b'\n') {
                    b.pop();
                }
                let t = String::from_utf8_lossy(&b).into_owned();
                if !t.is_empty() {
                    s2.auth = Auth::Owner(t);
                    s2.owner_from_disk = true;
                }
                break;
            }
        }
    }
    if let (Some(_), Some(p)) = (&text, port) {
        if s2.auth != Auth::None {
            return Ok((s2, p));
        }
    }
    let k2 = k2_client::paths::HOME_DIR_NAME;
    fail!(Exit::Error;
        format!("Error: local {PRODUCT} daemon is not reachable (need ~/{k2}/heartbeat.port + token)."),
        format!("Start it first, then re-run: {CLI} connect login"),
        format!("  {CLI} daemon start     # or: {CLI} daemon install  (fresh headless box)"),
    );
}

/// `_connect_device_id`: `tunnel.json`'s `device_id` (or `deviceId`),
/// stripped, else a new UUID 4.
fn device_id(home: &Path) -> String {
    if let Some(Some(Value::Object(pairs))) = load(&home.join(TUNNEL_FILE)) {
        let pick = |k: &str| get(&pairs, k).filter(|v| py::truthy(v)).cloned();
        if let Some(Value::Str(t)) = pick("device_id").or_else(|| pick("deviceId")) {
            let t = py::str_of(&Value::Str(t));
            let t = strip(&t);
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    uuid4()
}

/// `str(uuid.uuid4())`: 16 random bytes, version 4, the RFC 4122
/// variant.
fn uuid4() -> String {
    let mut b = [0u8; 16];
    random(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// The system's random bytes (`os.urandom`).
fn random(buf: &mut [u8]) {
    if crate::sysrand::fill(buf) {
        return;
    }
    // No system random bytes: the clock and the process, never all zeroes.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(1)
        ^ u128::from(std::process::id());
    for (i, x) in buf.iter_mut().enumerate() {
        *x = (seed >> ((i % 16) * 8)) as u8;
    }
}

/// `hostname 2>/dev/null || echo "k2-cli"`.
fn device_label() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: gethostname writes at most the buffer's length.
        if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0 {
            let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            let name = String::from_utf8_lossy(&buf[..end]).into_owned();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "k2-cli".to_string()
}

/// `_connect_claim_subdomain`: `Err` only when another device holds it.
fn claim(acc: &Account, access: &str, label: &str, did: &str, dlbl: &str) -> Result<(), Exit> {
    let body = Value::Object(vec![
        (Text::from("p_label"), text(label)),
        (Text::from("p_device_id"), text(did)),
        (
            Text::from("p_device_label"),
            if dlbl.is_empty() {
                Value::Null
            } else {
                text(dlbl)
            },
        ),
    ]);
    let req = acc
        .request(Method::Post, "/rest/v1/rpc/claim_subdomain")
        .header("Authorization", format!("Bearer {access}"))
        .header("Content-Type", "application/json")
        .data(dumps(&body).into_bytes());
    let (status, raw) = match got(urllib::open_url(&acc.place, &req), 22) {
        Got::Answer { status, raw } => (status, raw),
        Got::Failed(e) => {
            out::stderr_line(&format!(
                "Warning: claim_subdomain failed ({e}) \u{2014} continuing bind."
            ));
            return Ok(());
        }
        Got::Raised(r) => return Err(raise(&r)),
    };
    if !(200..300).contains(&status) {
        out::stderr_line(&format!(
            "Warning: claim_subdomain HTTP {status} \u{2014} continuing bind."
        ));
        return Ok(());
    }
    let rows = loads_or(&raw, Value::Array(Vec::new()));
    let row = match &rows {
        Value::Array(items) if !items.is_empty() => items[0].clone(),
        Value::Object(_) => rows.clone(),
        _ => Value::Object(Vec::new()),
    };
    let Value::Object(pairs) = &row else {
        return Ok(());
    };
    if get(pairs, "claimed").is_some_and(py::truthy) {
        return Ok(());
    }
    let Some(holder) = get(pairs, "holder").filter(|v| py::truthy(v)) else {
        return Ok(());
    };
    if *holder == text(did) {
        return Ok(());
    }
    let hl = get(pairs, "holder_label")
        .filter(|v| py::truthy(v))
        .unwrap_or(holder);
    fail!(Exit::Error; format!(
        "Error: {label}.{INFRA_ZONE} is in use on another device ({}) \u{2014} stop it there first.",
        py::str_of(hl)
    ));
}

/// A daemon answer as the shell's `$(…)` holds it: NULs dropped, the
/// trailing newlines taken off, read as text.
fn held(body: &[u8]) -> String {
    let mut b = crate::families::without_nuls(body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    String::from_utf8_lossy(&b).into_owned()
}

/// The binary's line where Python's getpass warns, with its own file and
/// line, that it reads the password with no terminal (`GetPassWarning:
/// Can not control echo on the terminal.`): the named difference
/// getpass-no-tty-warning.
pub const NO_TTY_WARNING: &str = "Warning: Can not control echo on the terminal.";

/// Python's getpass falls back to stdin (`fallback_getpass`): `/dev/tty`
/// can't be opened and stdin is open but no terminal. Any other way it
/// reads (a `/dev/tty` whose settings can't be read, a terminal on stdin
/// with no `/dev/tty`, a closed stdin) is the shell's.
#[cfg(unix)]
fn getpass_stdin_ok() -> bool {
    use std::os::unix::fs::OpenOptionsExt;
    let tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open("/dev/tty");
    // SAFETY: fcntl(F_GETFD) and isatty only read the descriptor's state.
    let open = unsafe { libc::fcntl(0, libc::F_GETFD) } != -1;
    tty.is_err() && open && unsafe { libc::isatty(0) } == 0
}

#[cfg(not(unix))]
fn getpass_stdin_ok() -> bool {
    false
}

/// `getpass.getpass(prompt)` with no terminal (`fallback_getpass`): the
/// warning ([`NO_TTY_WARNING`], then Python's own second line) and the
/// prompt on stderr, one line from stdin, then a newline on stderr.
/// Python's `sys.stdin.readline()` reads stdin in chunks of up to 8 KiB
/// until one holds a `\n`, and what else came in them is gone with its
/// process: so is it here. Only `\n` ends the line (Python's stdin on
/// Unix translates no newlines: a `\r` stays in the password). Nothing at
/// all before the end of the input is its `EOFError`.
fn getpass_stdin(prompt_text: &str) -> Getpass {
    let lines = format!("{NO_TTY_WARNING}\nWarning: Password input may be echoed.\n{prompt_text}");
    let _ = out::write_flushed(&mut std::io::stderr().lock(), lines.as_bytes());
    let mut got: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let end = loop {
        if let Some(i) = got.iter().position(|b| *b == b'\n') {
            break Some(i);
        }
        match read_fd0(&mut chunk) {
            0 => break None,
            n => got.extend_from_slice(&chunk[..n]),
        }
    };
    if got.is_empty() {
        return Getpass::Cancelled;
    }
    if let Some(i) = end {
        got.truncate(i);
    }
    prompt::prompt("\n");
    Getpass::Line(prompt::Secret::new(got))
}

/// One `read(2)` on stdin, retried when a signal stops it: its count, 0
/// at the end of the input or on an error (Python's read raises then;
/// the binary takes it as the end).
#[cfg(unix)]
fn read_fd0(buf: &mut [u8]) -> usize {
    loop {
        // SAFETY: the buffer is valid for its length.
        let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
        if n >= 0 {
            return n as usize;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return 0;
        }
    }
}

#[cfg(not(unix))]
fn read_fd0(buf: &mut [u8]) -> usize {
    use std::io::Read;
    std::io::stdin().lock().read(buf).unwrap_or(0)
}

/// The `/cli/tunnel/config` reader: `Err` with its line.
fn config_reader(raw: &str) -> Result<(), String> {
    let d = if strip(raw).is_empty() {
        Value::Object(Vec::new())
    } else {
        match json::parse(raw) {
            Ok(v) => v,
            Err(_) => {
                return Err(format!(
                    "Error: tunnel/config returned unparseable response: {}",
                    repr_text(&first_200(raw))
                ))
            }
        }
    };
    let Value::Object(pairs) = &d else {
        let shown = Some(first_200(raw)).filter(|t| !t.is_empty());
        return Err(format!(
            "Error: tunnel/config failed: {}",
            shown.unwrap_or_else(|| "empty".into())
        ));
    };
    if let Some(err) = get(pairs, "error").filter(|v| **v != Value::Null) {
        return Err(format!("Error binding tunnel config: {}", hint(err)));
    }
    if get(pairs, "tokenSet") == Some(&Value::Bool(false))
        && !get(pairs, "subdomain").is_some_and(py::truthy)
    {
        return Err("Error: tunnel/config did not accept the token.".into());
    }
    Ok(())
}

/// `err.get("hint") if isinstance(err, dict) else err`, as `%s` prints it.
fn hint(err: &Value) -> String {
    match err {
        Value::Object(e) => get(e, "hint").map_or_else(|| "None".to_string(), py::str_of),
        other => py::str_of(other),
    }
}

/// `%r` of a text, as Python writes `repr(str)`.
fn repr_text(t: &str) -> String {
    py::repr(&text(t))
}

/// The `/cli/tunnel/start` reader: its line on stdout, or `Err` with its
/// line on stderr. The live line only for a JSON object with no error: no
/// answer (an empty one), an answer that isn't JSON and one that isn't an
/// object each end with their line.
fn start_reader(raw: &str, label: &str) -> Result<String, String> {
    let label = if label.is_empty() { "?" } else { label };
    if strip(raw).is_empty() {
        return Err(format!(
            "Error starting tunnel: no answer from the {PRODUCT} daemon on {START}; check with: {CLI} connect status"
        ));
    }
    let d = match json::parse(raw) {
        Ok(v) => v,
        Err(_) => {
            return Err(format!(
                "Error: tunnel/start returned unparseable response: {}",
                repr_text(&first_200(raw))
            ))
        }
    };
    let Value::Object(pairs) = &d else {
        return Err(format!("Error: tunnel/start failed: {}", first_200(raw)));
    };
    if let Some(err) = get(pairs, "error").filter(|v| **v != Value::Null) {
        return Err(format!("Error starting tunnel: {}", hint(err)));
    }
    let url = get(pairs, "public_url")
        .filter(|v| py::truthy(v))
        .map_or_else(|| format!("https://{label}.{INFRA_ZONE}"), py::str_of);
    Ok(format!("\u{2713} live at {url}"))
}

/// The policy's air-gap refusal for `connect login`, before the guard
/// and the session's checks (`cli/src/cli-tools-policy.sh`): its line,
/// then exit 1. `Some(None)` when air-gap is off; `None`: the shell's (no
/// `HOME`).
pub fn airgap_refusal() -> Option<Option<Exit>> {
    let (state, file) = crate::airgap::state(&crate::offline::Env::current())?;
    Some(crate::airgap::refusal(state, &file).map(|mut line| {
        line.push(b'\n');
        let _ = out::write_flushed(&mut std::io::stderr().lock(), &line);
        Exit::Error
    }))
}

/// `cmd_connect login ...` after the router's checks; `words` are the
/// words after `login`. `None`: the shell answers (see the module's
/// notes).
pub fn run(words: &[&str], s: &Settings, would_refuse: bool) -> Option<Exit> {
    let env = match std::env::var_os(ENV_URL) {
        Some(v) => Some(v.into_string().ok()?),
        None => None,
    };
    let (base, notes) = logout::resolve(env, &s.home.join("settings.json"));
    let (mut place, prefix) = place(&base)?;
    let direct = k2_client::proxy_rule::direct(&format!("{base}{SUBDOMAINS}"));
    if !direct {
        // The account calls are the shell's Python urllib: its proxy rule.
        match logout::python_proxy(&place.host, &place.netloc, place.tls) {
            Ok(via) => place.via = via,
            Err(line) => {
                out::stderr_line(&line);
                return Some(Exit::Error);
            }
        }
    }
    let parsed = parse(words);
    if let Parsed::Words(w) = &parsed {
        let password = !w.token_stdin && w.token.is_empty();
        if password && !prompt::getpass_tty_ok() && !getpass_stdin_ok() {
            return None;
        }
    }
    let port = match daemon_port_text(s) {
        Some(t) => Some(crate::families::curl_port(&t)?),
        None => None,
    };
    for n in &notes {
        out::stderr_line(n);
    }
    Some(match login(parsed, s, would_refuse, place, prefix, port) {
        Ok(e) | Err(e) => e,
    })
}

/// The steps after the hand-back checks.
fn login(
    parsed: Parsed,
    s: &Settings,
    would_refuse: bool,
    place: Place,
    prefix: String,
    port: Option<u16>,
) -> Result<Exit, Exit> {
    let w = match parsed {
        Parsed::Words(w) => w,
        Parsed::Stop { lines, usage } => {
            let mut text = lines.join("\n");
            text.push('\n');
            if usage {
                text.push_str(crate::text::help("connect").unwrap_or_default());
            }
            let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
            return Err(Exit::Usage);
        }
    };
    // `_k2_refuse_if_airgap` here finds it off: the policy refused
    // before the router ([`airgap_refusal`]).
    let mut token = w.token.clone();
    if w.token_stdin {
        if w.token_flag {
            fail!(Exit::Usage; "Error: --token-stdin or --token, not both");
        }
        let secret = prompt::read_secret_stdin("Access JWT: ");
        let mut t = secret.expose().to_vec();
        if t.last() == Some(&b'\r') {
            t.pop();
        }
        token = String::from_utf8_lossy(&t).into_owned();
        if token.is_empty() {
            fail!(Exit::Usage; "Error: --token-stdin: no token on stdin");
        }
    } else if w.token_flag {
        out::stderr_line("note: --token is visible to every account on this computer while the command runs; use --token-stdin");
    }
    let acc = Account {
        place,
        prefix,
        ua: format!("k2-cli/{}", crate::embedded::version()),
    };
    let (access, refresh, account_email_shown) = if !token.is_empty() {
        let mut e = w.email.clone();
        if e.is_empty() {
            e = account_email(&load(&s.home.join(ACCOUNT_FILE)).unwrap_or(None));
        }
        if e.is_empty() {
            e = "(token)".to_string();
        }
        (token, String::new(), e)
    } else {
        let mut email = w.email.clone();
        if email.is_empty() {
            prompt::prompt(&format!("{} email: ", zone()));
            let line = prompt::read_line(Echo::Shown, AtEof::Keep);
            email = String::from_utf8_lossy(line.expose()).into_owned();
        }
        if email.is_empty() {
            fail!(Exit::Usage; "Error: email is required (or pass --token-stdin with an access JWT).");
        }
        let asked = format!("{} password: ", zone());
        let got = match prompt::getpass_tty(&asked) {
            Some(g) => g,
            None => getpass_stdin(&asked),
        };
        let pw = match got {
            Getpass::Line(s) => s,
            Getpass::Cancelled => fail!(Exit::Error; "\nError: password prompt cancelled."),
        };
        let mut pw: Vec<u8> = pw.expose().iter().copied().filter(|b| *b != 0).collect();
        while pw.last() == Some(&b'\n') {
            pw.pop();
        }
        if pw.is_empty() {
            fail!(Exit::Error; "Error: password must not be empty.");
        }
        let password = String::from_utf8_lossy(&pw).into_owned();
        pw.iter_mut().for_each(|b| *b = 0);
        let [a, r, mut e] = sign_in(&acc, &email, &password)?;
        if e.is_empty() {
            e = email;
        }
        (a, r, e)
    };

    save_account(&s.home, &access, &refresh, &account_email_shown)?;
    out::stderr_line(&format!("Signed in as {account_email_shown}."));

    let rows = list(&acc, &access)?;
    let label = pick(&public(&rows), &w.subdomain)?;
    let tunnel = tunnel_token(&rows, &label)?;

    let (s2, port) = require_daemon(s, port)?;
    let did = device_id(&s.home);
    let dlbl = device_label();
    claim(&acc, &access, &label, &did, &dlbl)?;

    let cfg = Value::Object(vec![
        (Text::from("subdomain"), text(&label)),
        (Text::from("token"), text(&tunnel)),
        (Text::from("deviceId"), text(&did)),
        (Text::from("deviceLabel"), text(&dlbl)),
        (Text::from("autoStart"), Value::Bool(true)),
    ]);
    let cfg = dumps(&cfg);
    let mut closing = Closing::default();
    let mut c1 = s2.client_on(port, would_refuse);
    let passport = c1.passport().is_some();
    let cfg_raw = match c1.call(Channel::Tcp, CONFIG, Call::PostJson(cfg.as_bytes())) {
        Ok(r) => {
            closing.answer(&r, passport);
            held(&r.body)
        }
        Err(Error::NoAnswer(n)) => {
            // The no-answer line ends the command there (exit 1).
            out::stderr_line(&n.to_string());
            closing.flush(&mut c1, Exit::Error);
            return Err(Exit::Error);
        }
        Err(_) => String::new(),
    };
    if let Err(line) = config_reader(&cfg_raw) {
        out::stderr_line(&line);
        closing.flush(&mut c1, Exit::Error);
        return Err(Exit::Error);
    }
    let mut c2 = s2.client_on(port, would_refuse);
    let passport = c2.passport().is_some();
    let pair = format!("subdomain={}", crate::transport::encode::quote(&label));
    let start_raw = match c2.post_query(START, &[pair.as_str()]) {
        Ok(r) => {
            closing.answer(&r, passport);
            held(&r.body)
        }
        Err(_) => String::new(),
    };
    let exit = match start_reader(&start_raw, &label) {
        Ok(line) => match out::stdout_line(&line) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        },
        Err(line) => {
            out::stderr_line(&line);
            Exit::Error
        }
    };
    closing.flush(&mut c2, exit);
    Ok(exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_read_as_the_shell_reads_them() {
        let w = |e: &str, t: &str, f: bool, si: bool, sub: &str| {
            Parsed::Words(Words {
                email: e.into(),
                token: t.into(),
                token_flag: f,
                token_stdin: si,
                subdomain: sub.into(),
            })
        };
        assert_eq!(parse(&[]), w("", "", false, false, ""));
        assert_eq!(
            parse(&["--email", "a@b", "--subdomain=acme", "--token-stdin"]),
            w("a@b", "", false, true, "acme")
        );
        assert_eq!(parse(&["--token="]), w("", "", true, false, ""));
        assert_eq!(parse(&["--token", "jwt"]), w("", "jwt", true, false, ""));
        let stop = |l: &str, usage: bool| Parsed::Stop {
            lines: vec![l.into()],
            usage,
        };
        assert_eq!(
            parse(&["--email"]),
            stop("Error: --email needs a value", false)
        );
        assert_eq!(
            parse(&["--token"]),
            stop("Error: --token needs a value", false)
        );
        assert_eq!(parse(&["-x"]), stop("Error: unknown flag '-x'", true));
        assert_eq!(parse(&["x"]), stop("Error: unexpected argument 'x'", true));
        assert!(matches!(
            parse(&["--pass", "x"]),
            Parsed::Stop { usage: false, .. }
        ));
    }

    #[test]
    fn the_account_server_s_place() {
        let (p, prefix) = place("http://127.0.0.1:9/base").unwrap();
        assert_eq!(
            (p.netloc.as_str(), p.tls, prefix.as_str()),
            ("127.0.0.1:9", false, "/base")
        );
        let (p, prefix) = place(logout::DEFAULT_URL).unwrap();
        assert_eq!((p.port, p.tls, prefix.as_str()), (443, true, ""));
        assert!(place("http://[::1]:9").is_none());
    }

    #[test]
    fn a_selection_is_python_s_int() {
        assert_eq!(py_int(" 2\n"), Some(2));
        assert_eq!(py_int("+1"), Some(1));
        assert_eq!(py_int("1_0"), Some(10));
        assert_eq!(py_int(""), None);
        assert_eq!(py_int("1x"), None);
        assert_eq!(py_int("_1"), None);
    }

    #[test]
    fn the_readers_print_what_the_python_prints() {
        assert_eq!(config_reader(""), Ok(()));
        assert_eq!(
            config_reader(r#"{"subdomain":"acme","tokenSet":true}"#),
            Ok(())
        );
        assert_eq!(
            config_reader(r#"{"tokenSet":false}"#),
            Err("Error: tunnel/config did not accept the token.".into())
        );
        assert_eq!(
            config_reader(r#"{"error":{"hint":"no"}}"#),
            Err("Error binding tunnel config: no".into())
        );
        assert_eq!(
            config_reader(r#"{"error":{"code":"x"}}"#),
            Err("Error binding tunnel config: None".into())
        );
        assert_eq!(
            config_reader("[1]"),
            Err("Error: tunnel/config failed: [1]".into())
        );
        assert_eq!(
            config_reader("nope"),
            Err("Error: tunnel/config returned unparseable response: 'nope'".into())
        );
        let no_answer = format!(
            "Error starting tunnel: no answer from the {PRODUCT} daemon on {START}; check with: {CLI} connect status"
        );
        assert_eq!(start_reader("", "acme"), Err(no_answer.clone()));
        assert_eq!(start_reader(" \n\t", "acme"), Err(no_answer));
        assert_eq!(
            start_reader("{}", "acme"),
            Ok(format!("\u{2713} live at https://acme.{INFRA_ZONE}"))
        );
        assert_eq!(
            start_reader("[]", "acme"),
            Err("Error: tunnel/start failed: []".into())
        );
        assert_eq!(
            start_reader("null", "acme"),
            Err("Error: tunnel/start failed: null".into())
        );
        assert_eq!(
            start_reader(r#"{"public_url":"https://x"}"#, "acme"),
            Ok("\u{2713} live at https://x".into())
        );
        assert_eq!(
            start_reader(r#"{"error":"down"}"#, "acme"),
            Err("Error starting tunnel: down".into())
        );
        assert_eq!(
            start_reader("<html>", "acme"),
            Err("Error: tunnel/start returned unparseable response: '<html>'".into())
        );
    }

    /// A loopback server that answers one request with `answer` and
    /// hands back what it read.
    fn one_answer(answer: &'static str) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                if let Some(at) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&got[..at]).to_string();
                    let len: usize = head
                        .lines()
                        .find_map(|x| x.strip_prefix("Content-Length: "))
                        .map_or(0, |v| v.parse().unwrap());
                    if got.len() >= at + 4 + len {
                        break;
                    }
                }
                match c.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            c.write_all(answer.as_bytes()).unwrap();
            got
        });
        (port, h)
    }

    fn account(port: u16) -> Account {
        Account {
            place: Place {
                host: "127.0.0.1".into(),
                port,
                netloc: format!("127.0.0.1:{port}"),
                tls: false,
                via: None,
            },
            prefix: "/base".into(),
            ua: "k2-cli/9.9.9".into(),
        }
    }

    #[test]
    fn the_password_grant_is_python_s_request_and_reading() {
        let (port, h) = one_answer(
            "HTTP/1.1 200 OK\r\nContent-Length: 80\r\n\r\n{\"access_token\":\"acc\",\"refresh_token\":null,\"user\":{\"email\":\"them@acme.example\"}}",
        );
        let got = sign_in(&account(port), "me@acme.example", "pw\u{e9}").unwrap();
        assert_eq!(
            got,
            ["acc".to_string(), String::new(), "them@acme.example".into()]
        );
        let body = r#"{"email": "me@acme.example", "password": "pw\u00e9"}"#;
        assert_eq!(
            String::from_utf8(h.join().unwrap()).unwrap(),
            format!(
                "POST /base/auth/v1/token?grant_type=password HTTP/1.1\r\nAccept-Encoding: identity\r\nContent-Length: {}\r\nHost: 127.0.0.1:{port}\r\nApikey: {ANON_KEY}\r\nUser-Agent: k2-cli/9.9.9\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        );
        let (port, h) = one_answer(
            "HTTP/1.1 400 Bad Request\r\nContent-Length: 49\r\n\r\n{\"error_description\":\"Invalid login credentials\"}",
        );
        assert_eq!(
            sign_in(&account(port), "me@acme.example", "x"),
            Err(Exit::Error)
        );
        h.join().unwrap();
    }

    #[test]
    fn a_uuid_is_version_4() {
        let u = uuid4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!(matches!(&u[19..20], "8" | "9" | "a" | "b"), "{u}");
        assert_ne!(u, uuid4());
    }
}
