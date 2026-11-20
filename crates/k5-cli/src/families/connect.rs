//! `k5 connect status`: this device's K2 Connect pairing, as the shell
//! CLI's `cmd_connect_status` prints it (`cli/src/connect-account.sh`).
//! It reads two files and asks the local daemon; it never talks to the
//! Connect control plane or the account service.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]) and the virtual desktop's guard
//!    ([`crate::prelude`]). `connect` skips the connection gate: no port
//!    or no token is not an error here.
//! 2. `connect` is the locked `connect-account` tool, so with `HOOK_SOCK`
//!    set: the stale-passport or require-passport lines, exit 3
//!    ([`crate::prelude::id_tool`]).
//! 3. A word after `status`: `Usage: k2 connect status`, exit 2.
//! 4. The account session: `connect-account.json` in the K2 home, when
//!    it is a file; its email is the third NUL-ended field the shell's
//!    Python writes of it ([`account_email`]).
//! 5. The pairing: `tunnel.json`'s `subdomain` and whether its `token` is
//!    set, read as data from the two NUL-ended fields its Python writes
//!    ([`tunnel`]).
//! 6. The daemon, when there is a port and a token (`_connect_require_daemon`:
//!    the port setting, `heartbeat.port`, then `daemon.port`): when
//!    `tunnel.json` holds no token, `cli_send cli_request
//!    /cli/tunnel/config` (TCP: the cell socket doesn't serve it) says
//!    whether the daemon holds one (a desktop keeps it in the vault);
//!    any failure is `no`, and no answer is the transport's line and
//!    death by SIGPIPE (the call is on the left of a pipe inside `$(…)`).
//! 7. Four lines, then, with a daemon, `cli_send cli_request
//!    /cli/tunnel/status` and what the shell's Python prints of it
//!    ([`status_lines`]); no answer is the transport's line and exit 1;
//!    any other failure reads as an empty answer. With no daemon, one
//!    line that says so.
//! 8. The EXIT trap's lines ([`crate::prelude::Closing`]); exit 0.
//!
//! Both calls run inside `$(…)`, so each call has its own client; a
//! passport the first one healed is taken up before the second
//! (`_cli_use_healed_passport`), which starts from it.
//!
//! The binary hands these to the shell before it prints or sends
//! anything ([`run`] returns `None`): a command line with `-h` or `--help`
//! (the front sends those on); a word that is not UTF-8; `HOOK_SOCK` set
//! with a `CODEX_` variable exported; a port that is set but is no port
//! curl takes; a `python3` the binary can't tell; and files whose reading
//! by the shell this does not copy: one that is not UTF-8 or nests deeper
//! than [`crate::out::json::MAX_DEPTH`].
//!
//! An answer from the daemon that is not UTF-8, and half of a surrogate
//! pair in one, are as [`super::inbox`] says.

use super::{curl_port, get, piped_body, py_space, s, without_nuls};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::transport::{Auth, Call, Error};
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["connect"];
/// The shell's but for [`VERBS`].
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// Only `status`; `login`, `logout`, the old publish words and
/// `connect` alone are the shell's.
pub const VERBS: &[crate::route::Verb] = &[crate::route::Verb {
    words: &["status"],
    owner: crate::route::Owner::Native,
}];

/// Where the daemon says whether it holds the tunnel token.
pub const CONFIG: &str = "/cli/tunnel/config";
/// Where the daemon says how the tunnel is.
pub const STATUS: &str = "/cli/tunnel/status";
/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "connect-account";
/// A word after `status`: exit 2.
pub const USAGE: &str = "Usage: k2 connect status";
/// The account session's file in the K2 home.
pub const ACCOUNT_FILE: &str = "connect-account.json";
/// The pairing's file in the K2 home.
pub const TUNNEL_FILE: &str = "tunnel.json";
/// The first line.
pub const HEADING: &str = concat!(k2_client::product_name!(), " Connect account");
/// `token_set` when the daemon can't read its vault.
pub const UNREADABLE: &str = concat!(
    "unknown (the ",
    k2_client::product_name!(),
    " vault could not be read)"
);
/// The last line with no daemon.
pub const NO_DAEMON: &str =
    "  Tunnel status:   (daemon not running \u{2014} start with: k2 daemon start)";
/// The last line when the shell's Python fails.
pub const UNREACHABLE: &str = "  Tunnel status:   (daemon unreachable)";

/// `k5 connect status`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.first() != Some(&"status") {
        return None;
    }
    let extra = words.len() > 1;
    if !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let facts = if extra {
        None
    } else {
        Some(Facts::read(&settings)?)
    };
    Some(run_with(args, facts, hook_sock.as_deref(), settings))
}

/// What the shell reads before it prints: the files, and the daemon's
/// port when there is a token too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// `connect-account.json` is a file.
    pub account: bool,
    /// The email the status line shows (empty: none).
    pub email: String,
    /// `tunnel.json`'s subdomain (empty: none).
    pub sub: String,
    /// `tunnel.json` holds a token.
    pub token_on_disk: bool,
    /// The daemon's port, when there is one and a token.
    pub daemon: Option<u16>,
}

impl Facts {
    /// The facts, or `None` where the shell's reading of a file or the
    /// port is not one this copies (see the module's notes).
    pub fn read(settings: &Settings) -> Option<Facts> {
        let home = &settings.home;
        let account_path = home.join(ACCOUNT_FILE);
        let account = account_path.is_file();
        let email = if account {
            account_email(&load(&account_path)?)
        } else {
            String::new()
        };
        let tunnel_path = home.join(TUNNEL_FILE);
        let (sub, token_on_disk) = if tunnel_path.is_file() {
            tunnel(&load(&tunnel_path)?)
        } else {
            (String::new(), false)
        };
        let daemon = match daemon_port(settings) {
            None => None,
            Some(text) => Some(curl_port(&text)?),
        };
        Some(Facts {
            account,
            email,
            sub,
            token_on_disk,
            daemon,
        })
    }
}

/// `_connect_require_daemon`'s port and token: the port the prelude read
/// (the setting, else `heartbeat.port`), else `daemon.port`'s text; `None`
/// when there is no port or no token.
pub fn daemon_port(settings: &Settings) -> Option<String> {
    if settings.auth == Auth::None {
        return None;
    }
    let text = match settings.port_text.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => {
            let f = settings.home.join("daemon.port");
            if !f.is_file() {
                return None;
            }
            // `$(cat … 2>/dev/null)`: unreadable is empty.
            std::fs::read_to_string(&f)
                .map(|t| t.trim_end_matches('\n').to_string())
                .unwrap_or_default()
        }
    };
    (!text.is_empty()).then_some(text)
}

/// `json.load(open(path))`: the value, or `Some(None)` where Python's
/// `except` catches it (no file to read, not JSON). `None`: a file this
/// does not read as Python would (not UTF-8, nested too deep).
pub fn load(path: &Path) -> Option<Option<Value>> {
    let Ok(bytes) = std::fs::read(path) else {
        return Some(None);
    };
    let text = String::from_utf8(bytes).ok()?;
    match json::parse(&text) {
        Ok(v) => Some(Some(v)),
        Err(json::Error::TooDeep { .. }) => None,
        Err(_) => Some(None),
    }
}

/// `s.strip()` on a string's code points; `None` for a value that is not
/// a string (Python's `AttributeError`).
fn stripped(v: &Value) -> Option<Vec<u32>> {
    let Value::Str(t) = v else {
        return None;
    };
    let cps = t.code_points();
    let space = |cp: &u32| char::from_u32(*cp).is_some_and(py_space);
    let start = cps.iter().position(|c| !space(c)).unwrap_or(cps.len());
    let end = cps.iter().rposition(|c| !space(c)).map_or(start, |i| i + 1);
    Some(cps[start..end.max(start)].to_vec())
}

/// `(d.get(key) or "").strip()`, or `None` where Python raises.
fn field(d: &Value, key: &str) -> Option<Vec<u32>> {
    let Value::Object(pairs) = d else {
        return None;
    };
    let v = get(pairs, key)
        .filter(|x| py::truthy(x))
        .cloned()
        .unwrap_or_else(|| s(""));
    stripped(&v)
}

/// A field as the shell's Python writes it and `read -r -d ''` takes it:
/// its NULs dropped (`.replace("\0", "")`) and the rest as UTF-8 with
/// `errors="replace"`, so half of a surrogate pair is a `?`.
fn as_text(cps: &[u32]) -> String {
    cps.iter()
        .filter(|cp| **cp != 0)
        .map(|cp| char::from_u32(*cp).unwrap_or('?'))
        .collect()
}

/// The account line's email: `_connect_account_read` writes the access
/// token, the refresh token and the email, each `(d.get(k) or "")`
/// stripped and ended by a NUL, and the shell reads the three in turn.
/// A value that is not a string stops Python there, so the email is set
/// only when all three were written; an object that is not a dict, and
/// a file Python can't read, write nothing.
pub fn account_email(loaded: &Option<Value>) -> String {
    let Some(d) = loaded else {
        return String::new();
    };
    let mut fields = Vec::new();
    for key in ["access_token", "refresh_token", "email"] {
        let Some(cps) = field(d, key) else {
            return String::new();
        };
        fields.push(cps);
    }
    as_text(&fields[2])
}

/// `tunnel.json`'s subdomain and whether its token is set, as the shell
/// reads the two NUL-ended fields its Python writes (0.46.17: as data,
/// never run). Python writes both or nothing (a value that is not a
/// string, or an object that is not a dict, raises before it writes), and
/// nothing leaves `sub` empty and `token_set` `no`.
pub fn tunnel(loaded: &Option<Value>) -> (String, bool) {
    let empty = Value::Object(Vec::new());
    let d = loaded.as_ref().unwrap_or(&empty);
    match (field(d, "subdomain"), field(d, "token")) {
        (Some(sub), Some(tok)) => (as_text(&sub), !tok.is_empty()),
        _ => (String::new(), false),
    }
}

/// The answer to `/cli/tunnel/config` as the shell's Python reads it:
/// `yes`, the vault line, or `no`.
pub fn token_flag(body: &[u8]) -> &'static str {
    let text = String::from_utf8_lossy(body);
    let Ok(d @ Value::Object(_)) = json::parse(&text) else {
        return "no";
    };
    let truthy = |key: &str| match &d {
        Value::Object(pairs) => get(pairs, key).is_some_and(py::truthy),
        _ => false,
    };
    if truthy("tokenSet") {
        "yes"
    } else if truthy("tokenUnreadable") {
        UNREADABLE
    } else {
        "no"
    }
}

/// What the shell's Python prints for `raw` (the answer to
/// `/cli/tunnel/status` as `$(…)` kept it).
pub fn status_lines(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let d = if text.trim_matches(py_space).is_empty() {
        Value::Object(Vec::new())
    } else {
        match json::parse(&text) {
            Ok(v) => v,
            Err(_) => return "  Tunnel status:   (unparseable daemon response)\n".into(),
        }
    };
    let Value::Object(pairs) = &d else {
        return "  Tunnel status:   (error: unknown)\n".into();
    };
    let at = |key: &str| get(pairs, key);
    let truthy = |key: &str| at(key).is_some_and(py::truthy);
    if let Some(err) = at("error").filter(|e| py::truthy(e)) {
        return format!("  Tunnel status:   (error: {})\n", py::str_of(err));
    }
    let running = truthy("running");
    let mut out = format!(
        "  Tunnel running:  {}\n",
        if running { "yes" } else { "no" }
    );
    if running {
        let url = at("public_url")
            .filter(|u| py::truthy(u))
            .cloned()
            .unwrap_or_else(|| s("(assigned by server)"));
        out.push_str(&format!("  Public URL:      {}\n", py::str_of(&url)));
    } else if truthy("released") {
        out.push_str("  State:           RELEASED \u{2014} re-pair with: k2 connect login\n");
    } else if at("enabled") == Some(&Value::Bool(false)) {
        out.push_str("  State:           DISABLED \u{2014} re-enable with: k2 tunnel enable\n");
    } else {
        out.push_str("  State:           stopped\n");
    }
    out
}

/// The four lines before the daemon's status.
pub fn account_lines(f: &Facts, token_set: &str) -> String {
    let mut out = format!("{HEADING}\n");
    out.push_str(&format!(
        "  Account session: {}",
        if f.account { "yes" } else { "no" }
    ));
    if !f.email.is_empty() {
        out.push_str(&format!("  ({})", f.email));
    }
    out.push('\n');
    if f.sub.is_empty() {
        out.push_str("  Paired subdomain: (none)\n");
    } else {
        out.push_str(&format!(
            "  Paired subdomain: {sub} \u{2192} https://{sub}.{zone}\n",
            sub = f.sub,
            zone = k2_client::brand::INFRA_ZONE
        ));
    }
    out.push_str(&format!("  Tunnel token:     {token_set}\n"));
    out
}

fn run_with(
    args: &[OsString],
    facts: Option<Facts>,
    hook_sock: Option<&Path>,
    settings: Settings,
) -> Exit {
    // `connect` skips the connection gate; the guard still stands.
    if let Some(Gate::VirtualDesktop) = prelude::gate(&settings) {
        return Gate::VirtualDesktop.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let Some(facts) = facts else {
        out::stderr_line(USAGE);
        return Exit::Usage;
    };
    let mut closing = Closing::default();
    // Each call is the shell's `$(…)`, so each has its own client. A
    // passport the first one healed is taken up after it (0.46.17,
    // `_cli_use_healed_passport`): the second client is made only then,
    // and starts from the passport as it is now.
    let mut token_set = if facts.token_on_disk { "yes" } else { "no" };
    if let (false, Some(port)) = (facts.token_on_disk, facts.daemon) {
        let mut c = settings.client_on(port, would_refuse);
        let passport = c.passport().is_some();
        token_set = match c.send(CONFIG, Call::Request(&[])) {
            Ok(r) => {
                closing.answer(&r, passport);
                token_flag(&piped_body(&r))
            }
            Err(Error::NoAnswer(n)) => {
                out::stderr_line(&n.to_string());
                super::end_in_pipe()
            }
            Err(_) => "no",
        };
    }
    if let Err(e) = out::stdout(&account_lines(&facts, token_set)) {
        return out::after_stdout_error(&e);
    }
    let Some(port) = facts.daemon else {
        if let Err(e) = out::stdout_line(NO_DAEMON) {
            return out::after_stdout_error(&e);
        }
        return Exit::Ok;
    };
    let mut client = settings.client_on(port, would_refuse);
    let passport = client.passport().is_some();
    let raw = match client.send(STATUS, Call::Request(&[])) {
        Ok(r) => {
            closing.answer(&r, passport);
            let mut b = without_nuls(&r.body);
            while b.last() == Some(&b'\n') {
                b.pop();
            }
            b
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            closing.flush(&mut client, Exit::Error);
            return Exit::Error;
        }
        Err(_) => Vec::new(),
    };
    let exit = match out::stdout(&status_lines(&raw)) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    };
    closing.flush(&mut client, exit);
    exit
}

#[cfg(test)]
mod tests;
