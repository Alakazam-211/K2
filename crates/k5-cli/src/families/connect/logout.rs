//! `k5 connect logout`, as the shell CLI's `cmd_connect` and
//! `cmd_connect_logout` answer it (`cli/src/connect-account.sh`):
//!
//! 1. `_connect_db_resolve` ([`resolve`]): the account server's base URL
//!    (`K2_CONNECT_DB_URL`, then the `connectDbUrl` setting, then the
//!    built-in one), and a line on stderr when it is not the built-in one.
//! 2. A word after `logout`: `Usage: k2 connect logout`, exit 2.
//! 3. The saved access token (`_connect_account_read`'s first field).
//!    When there is one: a best-effort `POST <base>/auth/v1/logout` with
//!    the token as `Authorization: Bearer`, the anon key as `apikey`, an
//!    empty body and 10 s; whatever comes back (or doesn't) is ignored.
//! 4. `connect-account.json` goes, and two lines on stdout; exit 0.
//!
//! The token goes in that one header only: no child runs, so it is in no
//! argv, environment or log line.
//!
//! Handed to the shell ([`run`] returns `None`), before anything is
//! printed or sent: a base URL the binary doesn't send itself (an IPv6
//! address, a port it can't read), HTTPS to another computer with a
//! proxy set in the environment (Python's urllib would use it), and a
//! settings or account file the shell's Python reads otherwise (not
//! UTF-8, nested too deep).
//!
//! Where it differs: the request's head is the binary's own (curl's
//! shape) where the shell's Python sends urllib's; the account server's
//! answer is never read by either.

use super::{field_text, load};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::{self, py};
use crate::transport::wire::{self, Body, Method, Request, Target};
use k2_client::cli_name;
use k2_client::hosts::ZONE_SUFFIX;
use std::path::Path;
use std::time::{Duration, Instant};

/// The built-in account server (`_K2_SUPABASE_URL`).
pub const DEFAULT_URL: &str = "https://ttgcalfrzzgkxnfepkiu.supabase.co";
/// Its public anon key (`_K2_SUPABASE_ANON_KEY`): not a secret, the
/// desktop client ships it too.
pub const ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6InR0Z2NhbGZyenpna3huZmVwa2l1Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3ODA1MDIyMzksImV4cCI6MjA5NjA3ODIzOX0.L28xgtYkPEj5eCNDGO5Zf5xxhdKQLxKD8c1CJRHNqI8";
/// The setting that names the account server.
pub const ENV_URL: &str = "K2_CONNECT_DB_URL";
/// The route.
pub const LOGOUT: &str = "/auth/v1/logout";
/// `urlopen(..., timeout=10)`.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// The Python's `normalize`: the URL less trailing `/`s, or why not.
pub fn normalize(raw: &str) -> Result<String, &'static str> {
    let t = raw.trim_matches(super::super::py_space);
    if t.chars().any(|c| super::super::py_space(c) || (c as u32) < 32) {
        return Err("must not contain spaces");
    }
    let rest = if let Some(r) = t.strip_prefix("https://") {
        r
    } else if let Some(r) = t.strip_prefix("http://") {
        let auth = authority(r);
        let host = if let Some(h) = auth.strip_prefix('[') {
            h.split(']').next().unwrap_or("")
        } else {
            auth.split(':').next().unwrap_or("")
        };
        if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
            return Err("must be an https:// URL (plain http:// only to localhost)");
        }
        r
    } else {
        return Err("must be an https:// URL");
    };
    let auth = authority(rest);
    if auth.is_empty() {
        return Err("has no host");
    }
    if auth.contains('@') {
        return Err("must not carry a user name or password");
    }
    if rest.contains('?') || rest.contains('#') {
        return Err("must not carry a query or fragment");
    }
    Ok(t.trim_end_matches('/').to_string())
}

/// `rest.split("/")[0].split("?")[0].split("#")[0]`.
fn authority(rest: &str) -> &str {
    let a = rest.split('/').next().unwrap_or("");
    let a = a.split('?').next().unwrap_or("");
    a.split('#').next().unwrap_or("")
}

/// `_connect_db_resolve`: the base URL and its lines on stderr. Any
/// failure to read the settings file is no setting (the Python's
/// `except Exception`).
pub fn resolve(env: Option<String>, settings: &Path) -> (String, Vec<String>) {
    let c = cli_name!();
    let mut lines = Vec::new();
    let env = env.unwrap_or_default();
    let env = env.trim_matches(super::super::py_space);
    if !env.is_empty() {
        match normalize(env) {
            Ok(url) => {
                lines.push(format!(
                    "{c} connect: account server {url} (from {ENV_URL})"
                ));
                return (url, lines);
            }
            Err(why) => lines.push(format!("{c} connect: {ENV_URL} ignored: {why}")),
        }
    }
    // `json.load(f).get("connectDbUrl") or ""`, any exception "".
    let setting = match load(settings) {
        Some(Some(Value::Object(pairs))) => match super::super::get(&pairs, "connectDbUrl") {
            Some(v @ Value::Str(_)) if py::truthy(v) => py::str_of(v),
            _ => String::new(),
        },
        _ => String::new(),
    };
    if !setting.trim_matches(super::super::py_space).is_empty() {
        match normalize(&setting) {
            Ok(url) => {
                lines.push(format!(
                    "{c} connect: account server {url} (from the connectDbUrl setting)"
                ));
                return (url, lines);
            }
            Err(why) => lines.push(format!("{c} connect: setting connectDbUrl ignored: {why}")),
        }
    }
    (DEFAULT_URL.to_string(), lines)
}

/// Where the logout goes: the target, the `Host` header and the path.
pub fn target(base: &str) -> Option<(Target, String, String)> {
    let (tls, rest) = if let Some(r) = base.strip_prefix("https://") {
        (true, r)
    } else {
        (false, base.strip_prefix("http://")?)
    };
    let (auth, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let default = if tls { 443 } else { 80 };
    let (host, port) = match auth.rsplit_once(':') {
        Some((h, p)) => {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (h, Some(p.parse::<u16>().ok().filter(|p| *p != 0)?))
        }
        None => (auth, None),
    };
    let plain = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_');
    if host.is_empty() || !host.bytes().all(plain) {
        return None;
    }
    if !path
        .bytes()
        .all(|b| b.is_ascii_graphic() && !matches!(b, b'?' | b'#'))
    {
        return None;
    }
    let port_n = port.unwrap_or(default);
    let host_header = match port {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    };
    let t = if tls {
        Target::Tls {
            host: host.to_string(),
            port: port_n,
        }
    } else {
        Target::Tcp {
            host: host.to_string(),
            port: port_n,
        }
    };
    Some((t, host_header, format!("{path}{LOGOUT}")))
}

/// The saved access token: `_connect_account_read`'s first field (empty
/// where its Python's `except` or a raise leaves nothing).
pub fn access_token(path: &Path) -> String {
    match load(path) {
        Some(Some(d)) => field_text(&d, "access_token").unwrap_or_default(),
        _ => String::new(),
    }
}

/// The two lines on stdout.
pub fn done_lines() -> String {
    let c = cli_name!();
    let zone = ZONE_SUFFIX.trim_start_matches('.');
    format!(
        "Logged out of {zone} account (cleared ~/{}/connect-account.json).\nTunnel pairing is unchanged. To unpair this device: {c} tunnel release --confirm\n",
        k2_client::paths::HOME_DIR_NAME
    )
}

/// `cmd_connect logout ...` after the router's checks; `words` are the
/// words after `logout`. `None`: the shell answers.
pub fn run(words: &[&str], home: &Path) -> Option<Exit> {
    let env = match std::env::var_os(ENV_URL) {
        Some(v) => Some(v.into_string().ok()?),
        None => None,
    };
    let (base, notes) = resolve(env, &home.join("settings.json"));
    let account = home.join(super::ACCOUNT_FILE);
    let token = access_token(&account);
    let to = if words.is_empty() && !token.is_empty() {
        let t = target(&base)?;
        let direct = k2_client::proxy_rule::direct(&format!("{base}{LOGOUT}"));
        if !direct
            && super::super::remote::logout::PROXY_ENV
                .iter()
                .any(|n| std::env::var_os(n).is_some_and(|v| !v.is_empty()))
        {
            return None;
        }
        Some(t)
    } else {
        None
    };
    for n in &notes {
        out::stderr_line(n);
    }
    if !words.is_empty() {
        out::stderr_line(&format!("Usage: {} connect logout", cli_name!()));
        return Some(Exit::Usage);
    }
    if let Some((t, host, path)) = to {
        send(&t, host, path, &token);
    }
    if account.is_file() {
        let _ = std::fs::remove_file(&account);
    }
    Some(match out::stdout(&done_lines()) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    })
}

/// The best-effort POST: nothing it meets is shown.
fn send(t: &Target, host: String, path: String, token: &str) {
    let deadline = Instant::now() + TIMEOUT;
    let Ok(mut conn) = wire::connect(t, TIMEOUT, deadline) else {
        return;
    };
    let req = Request {
        method: Method::Post,
        target: path,
        host,
        user_agent: format!("k2-cli/{}", crate::embedded::version()),
        headers: vec![
            ("apikey".into(), ANON_KEY.into()),
            ("Authorization".into(), format!("Bearer {token}")),
        ],
        body: Some(Body {
            bytes: Vec::new(),
            curl_form_type: true,
        }),
    };
    let _ = wire::exchange(&mut conn, &req, deadline);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_server_url() {
        assert_eq!(normalize(" https://a.example/ "), Ok("https://a.example".into()));
        assert_eq!(normalize("http://127.0.0.1:9/"), Ok("http://127.0.0.1:9".into()));
        assert_eq!(normalize("http://[::1]:9"), Ok("http://[::1]:9".into()));
        assert!(normalize("http://a.example").is_err());
        assert_eq!(normalize("ftp://a"), Err("must be an https:// URL"));
        assert_eq!(normalize("https://"), Err("has no host"));
        assert_eq!(
            normalize("https://u@a"),
            Err("must not carry a user name or password")
        );
        assert_eq!(
            normalize("https://a/?x"),
            Err("must not carry a query or fragment")
        );
        assert_eq!(normalize("https://a b"), Err("must not contain spaces"));
    }

    #[test]
    fn where_the_logout_goes() {
        let (t, host, path) = target("http://127.0.0.1:9/base").unwrap();
        assert!(matches!(t, Target::Tcp { ref host, port: 9 } if host == "127.0.0.1"));
        assert_eq!((host.as_str(), path.as_str()), ("127.0.0.1:9", "/base/auth/v1/logout"));
        let (t, host, path) = target(DEFAULT_URL).unwrap();
        assert!(matches!(t, Target::Tls { port: 443, .. }));
        assert_eq!(host, "ttgcalfrzzgkxnfepkiu.supabase.co");
        assert_eq!(path, LOGOUT);
        assert!(target("http://[::1]:9").is_none());
    }

    #[test]
    fn the_settings_line_and_the_default() {
        let dir = std::env::temp_dir().join(format!("k5-connect-resolve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = dir.join("settings.json");
        std::fs::write(&s, r#"{"connectDbUrl":"https://db.example/"}"#).unwrap();
        let (url, lines) = resolve(Some("nope".into()), &s);
        assert_eq!(url, "https://db.example");
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("K2_CONNECT_DB_URL ignored: must be an https:// URL"));
        std::fs::write(&s, "[1]").unwrap();
        assert_eq!(resolve(None, &s), (DEFAULT_URL.to_string(), vec![]));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
