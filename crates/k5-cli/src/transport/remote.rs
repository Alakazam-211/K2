//! A workspace on another K2 server (K2-as-a-server): the shell's
//! `_apply_remote_host` and `_remote_token_for_host`
//! (`cli/src/remote-msg.sh`), shared by every verb that takes
//! `<workspace>@<host>` (`talk`, `read`, the tray forms of `msg`).
//!
//! The shell repoints `BASE_URL` at `https://<host>` (or the URL as given
//! when it starts with `http://` or `https://`, less one trailing `/`),
//! swaps `TOKEN` for that host's owner token, and never uses the cell
//! socket after that. [`apply`] does the same to a [`Client`]: the
//! credential is that token alone, sent as the owner's but never as one
//! read from this machine's K2 home.
//!
//! This module is the one place the person's proxy settings are read
//! ([`routed`], [`super::proxy`]): only for a base that is `https://` to a
//! host that is not this computer. The daemon's paths never come here.
//! It also holds K2's shared transport rule for a token-bearing request
//! to another machine: https only ([`plain_http_refusal`]).

use super::client::{Auth, Client};
use super::proxy::{self, Route};
use crate::out::json::{self, Value};
use crate::out::py;
use k2_client::env_names;
use k2_client::hosts::ZONE_SUFFIX;
use std::ffi::OsString;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the shell sends once a remote host is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    /// `https://`.
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// curl's `Host` header for the URL: the host, and `:<port>` when the
    /// URL names a port that is not the scheme's.
    pub host_header: String,
    /// The person's proxy for it ([`routed`]), or the setting curl
    /// refuses (the request fails at its connect); none from [`base`].
    pub via: Option<proxy::Via>,
}

/// `BASE_URL` for `host` as `_apply_remote_host` sets it, read as curl
/// reads it. `None`: a URL the binary doesn't send itself (a path after
/// the host, a user part, an IPv6 address, a port curl wouldn't take, or
/// a byte that is not a plain host name's); the shell answers those.
pub fn base(host: &str) -> Option<Base> {
    let (tls, rest) = if let Some(r) = host.strip_prefix("https://") {
        (true, r.strip_suffix('/').unwrap_or(r))
    } else if let Some(r) = host.strip_prefix("http://") {
        (false, r.strip_suffix('/').unwrap_or(r))
    } else {
        (true, host)
    };
    let default = if tls { 443 } else { 80 };
    let (name, port) = match rest.rsplit_once(':') {
        Some((name, digits)) => {
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let port: u16 = digits.parse().ok().filter(|p| *p != 0)?;
            (name, Some(port))
        }
        None => (rest, None),
    };
    let plain = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_');
    if name.is_empty() || !name.bytes().all(plain) {
        return None;
    }
    let port_n = port.unwrap_or(default);
    let host_header = match port {
        Some(p) if p != default => format!("{name}:{p}"),
        _ => name.to_string(),
    };
    Some(Base {
        tls,
        host: name.to_string(),
        port: port_n,
        host_header,
        via: None,
    })
}

/// [`base`], with the person's proxy for it, read from the process
/// environment as curl reads it ([`proxy::decide`]). `None`: the shell's
/// (a URL [`base`] doesn't read). A proxy setting curl refuses is
/// carried in `via` and fails the request at its connect, as curl does.
pub fn routed(host: &str) -> Option<Base> {
    routed_with(host, process_env)
}

/// The process environment, as curl reads it: the one place this module
/// reads it so ([`routed`]).
fn process_env(name: &str) -> Option<OsString> {
    proxy::from_process_env(name)
}

/// [`routed`] with the variables from `env` (the tests' tables).
pub fn routed_with(host: &str, env: impl Fn(&str) -> Option<OsString>) -> Option<Base> {
    let mut b = base(host)?;
    b.via = proxy::decide(&b.host, b.tls, env);
    Some(b)
}

/// The person's proxy for a request a Python block's urllib sends to a
/// host it names (clone's engine), read as the shell's Python reads it
/// (one proxy rule for every urllib-style family, which
/// supersedes the curl rules for clone): plain http never uses a proxy;
/// then `k2_direct` on `host` (the one urllib connects to, brackets off,
/// in any case) and urllib's reading of the variables and `no_proxy` for
/// `netloc` (urllib's `req.host`: the URL's host and port as written,
/// decoded), as [`python_daemon_route`]. [`Route::Shell`]: a setting the
/// binary doesn't read as Python does (a proxy URL of another scheme, a
/// variable that is not UTF-8).
pub fn python_route(host: &str, netloc: &str, https: bool) -> Route {
    if !https {
        return Route::Direct;
    }
    python_daemon_route(host, netloc, &python_proxy_env())
}

/// The variables the shell's embedded Python reads its proxy from, as
/// urllib reads them ([`proxy::python::env`]), for
/// [`python_daemon_route`]: read once per opener
/// ([`super::urllib::Opener::from_client`]).
pub fn python_proxy_env() -> proxy::python::Env {
    proxy::python_environ()
}

/// The route of a request the shell's Python blocks send to the daemon at
/// `K2_HOST=https://<host>` (bash's Python tunnels through
/// the person's proxy, and the binary does too, reading the settings as
/// that Python reads them): `k2_direct` first (this computer never uses
/// a proxy), then urllib's `getproxies_environment` and
/// `proxy_bypass_environment` ([`proxy::python`]) for `netloc`, the
/// URL's `host:port` as urllib's `req.host` has it.
pub fn python_daemon_route(host: &str, netloc: &str, env: &proxy::python::Env) -> Route {
    if proxy::python::direct(host) {
        return Route::Direct;
    }
    proxy::python::decide(netloc, env)
}

/// K2's shared transport rule: a request with a token to a host that is
/// not this computer goes over https only, proxied or not (the rule `k2
/// remote logout` has). The line, exit 2, for a plain `http://` `url`
/// whose host is not a loopback literal ([`literal_loopback`]); checked
/// before any request.
pub fn plain_http_refusal(b: &Base, url: &str) -> Option<String> {
    (!b.tls && !literal_loopback(&b.host)).then(|| plain_http_line(url))
}

/// The rule: this computer is judged by the URL
/// AS WRITTEN, never by resolving a name (DNS rebinding): an IPv4 literal
/// in 127.0.0.0/8, the IPv6 literal `::1` (bracketed or not), or exactly
/// the name `localhost`. `x.localhost`, `LOCALHOST`, `localhost.` and
/// every other name are not.
pub fn literal_loopback(host: &str) -> bool {
    let h = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    h == "localhost" || h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// `_k2_refuse_plain_http URL`, for a URL as the person typed it: `None`
/// for `https` (any case) or a host that is this computer as written
/// ([`crate::prelude::plain_http::url_is_literal_loopback`]); else the
/// refusal's line, exit 2, before the token is looked up. No scheme is
/// plain http (curl guesses it).
pub fn refuse_plain_http(url: &str) -> Option<String> {
    let scheme = url.split_once("://").map_or("", |(s, _)| s);
    if scheme.eq_ignore_ascii_case("https")
        || crate::prelude::plain_http::url_is_literal_loopback(url)
    {
        return None;
    }
    Some(plain_http_line(url))
}

/// The plain-http refusal's line ([`plain_http_refusal`]).
pub fn plain_http_line(url: &str) -> String {
    format!("Error: plain http:// only to this computer; use https:// (got: {url})")
}

/// `BASE_URL` as the shell's `_k2_daemon_base` writes it for `host` and
/// the port's text: `https://<name>:<port>` for an `https://` (or
/// `HTTPS://`) host, one trailing `/` of the name dropped, else
/// `http://<host>:<port>`.
pub fn daemon_base(host: &str, port: &str) -> String {
    match host
        .strip_prefix("https://")
        .or_else(|| host.strip_prefix("HTTPS://"))
    {
        Some(rest) => format!("https://{}:{port}", rest.strip_suffix('/').unwrap_or(rest)),
        None => format!("http://{host}:{port}"),
    }
}

/// `K2_HOST=https://<name>` (or `HTTPS://`): a daemon behind K2's TLS or
/// K2 Connect. The shell's `_k2_daemon_base` makes `BASE_URL`
/// `https://<name>:<PORT>` (one trailing `/` of the name dropped), and
/// `_k2_curl` sends it over TLS, through the person's proxy when the name
/// isn't this computer (`_k2_direct_url`). [`routed`] reads it as it reads
/// a remote host's URL; the credential and the cell socket stay the
/// daemon's, and a refused connect follows no port file
/// (`_k2_follow_port_move` is for a loopback `HOST` only). A URL
/// [`routed`] doesn't read: nothing goes over TCP
/// ([`Client::unsendable`]), never a plain or direct request in its
/// place. Any other host: unchanged.
pub fn daemon_https(client: &mut Client) {
    let Some(rest) = client
        .host
        .strip_prefix("https://")
        .or_else(|| client.host.strip_prefix("HTTPS://"))
    else {
        return;
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let url = format!("https://{rest}:{}", client.port);
    client.tls = true;
    client.port_file = None;
    match routed(&url) {
        Some(b) => {
            client.host = b.host;
            client.port = b.port;
            client.via = b.via;
            client.host_header = Some(b.host_header);
        }
        None => client.unsendable = Some(url),
    }
}

/// `BASE_URL` as `_apply_remote_host` sets it for `host`: the URL as
/// given less one trailing `/`, or `https://<host>`.
pub fn base_url(host: &str) -> String {
    if host.starts_with("http://") || host.starts_with("https://") {
        host.strip_suffix('/').unwrap_or(host).to_string()
    } else {
        format!("https://{host}")
    }
}

/// Whether the binary sends a token to `b` (from [`routed`]) itself, as
/// the shell's curl would: HTTPS, direct or through the person's proxy
/// (`b.via`); plain http only to a literal loopback address
/// (`127.x.x.x`), never a name that would have to be looked up. Anything
/// else is the shell's (it sends there by its own rules).
pub fn sends_here(b: &Base) -> bool {
    b.tls
        || b.host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The client sends to `b` (and through its proxy), keeping its own
/// credential: `daemon restart --host`'s repoint. The K2 home's port
/// file is this machine's daemon, not the host's.
pub fn point(client: &mut Client, b: &Base) {
    client.host = b.host.clone();
    client.port = b.port;
    client.tls = b.tls;
    client.via = b.via.clone();
    client.host_header = Some(b.host_header.clone());
    client.port_file = None;
}

/// `_apply_remote_host`'s swap, once the token is known.
pub fn apply(client: &mut Client, base: &Base, token: String) {
    client.host = base.host.clone();
    client.port = base.port;
    client.tls = base.tls;
    client.via = base.via.clone();
    client.host_header = Some(base.host_header.clone());
    client.auth = Auth::Owner(token);
    client.owner_from_disk = false;
    client.no_token = false;
    client.force_tcp = true;
    // The K2 home's port file is this machine's daemon, not the host's.
    client.port_file = None;
}

/// `_apply_remote_host`'s two lines when no token is found, exit 1.
pub fn no_token_lines(host: &str, home: &Path) -> [String; 2] {
    [
        format!("Error: no owner token for remote host '{host}'."),
        format!(
            "Pass --token <tok>, set $K2_REMOTE_TOKEN, or add it to {}/connect-tokens.json ({{\"{host}\": \"<token>\"}}).",
            home.display()
        ),
    ]
}

/// `_remote_token_for_host <host> <explicit>`: the explicit token, else
/// the first `*_REMOTE_TOKEN` that is set and not empty, else the K2
/// home's `connect-tokens.json`, else the keychain entry for the host's
/// id in `connect-hosts.json`. `None`: no token (the shell's empty
/// answer). The shell looks in the files only when `python3` is there;
/// the caller hands the command to the shell when it can't tell.
pub fn token_for_host(host: &str, explicit: &str, home: &Path) -> Option<String> {
    if !explicit.is_empty() {
        return Some(explicit.to_string());
    }
    if let Some((_, v)) =
        env_names::first_nonempty_os_in(env_names::REMOTE_TOKEN, |n| std::env::var_os(n))
    {
        return Some(v.to_string_lossy().into_owned());
    }
    let homes = homes(home, std::env::var_os("HOME").map(PathBuf::from));
    match lookup(host, &homes)? {
        Found::Token(t) => Some(t),
        Found::HostId(id) => keychain(&id),
    }
}

/// What the shell's Python printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Token(String),
    HostId(String),
}

/// The old name's home folder under the user's home.
pub fn old_home(user_home: &Path) -> PathBuf {
    user_home.join(format!(".{}", k2_client::cli_install::LEGACY_NAME))
}

/// The K2 home, then the old name's home ([`old_home`]) when it is a
/// folder and not the K2 home.
pub fn homes(home: &Path, user_home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut homes = vec![home.to_path_buf()];
    if let Some(u) = user_home {
        let alt = old_home(&u);
        if alt != home && alt.is_dir() {
            homes.push(alt);
        }
    }
    homes
}

/// The Python's `bare`: lower case, less the zone (twice doubled, once).
pub fn bare(h: &str) -> String {
    let mut h = py_strip(h).to_lowercase();
    let doubled = format!("{ZONE_SUFFIX}{ZONE_SUFFIX}");
    while h.ends_with(&doubled) {
        h.truncate(h.len() - ZONE_SUFFIX.len());
    }
    if h.ends_with(ZONE_SUFFIX) {
        h.truncate(h.len() - ZONE_SUFFIX.len());
    }
    h
}

/// The Python's `host_keys`.
pub fn host_keys(h: &str) -> Vec<String> {
    let h = py_strip(h);
    if h.is_empty() {
        return Vec::new();
    }
    let b = bare(h);
    let mut keys = vec![
        h.to_string(),
        h.to_lowercase(),
        b.clone(),
        format!("{b}{ZONE_SUFFIX}"),
        format!("{b}{}", ZONE_SUFFIX.to_uppercase()),
    ];
    keys.retain(|k| !k.is_empty());
    keys
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(crate::families::py_space)
}

/// A JSON object as Python's dict holds it: one entry per key, the last
/// value, at the first key's place.
fn dict(pairs: &[(json::Text, Value)]) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();
    for (k, v) in pairs {
        let k = k
            .as_str()
            .map_or_else(|| py::str_of(&Value::Str(k.clone())), str::to_string);
        match out.iter_mut().find(|(have, _)| *have == k) {
            Some(slot) => slot.1 = v.clone(),
            None => out.push((k, v.clone())),
        }
    }
    out
}

fn load(path: &Path) -> Option<Value> {
    if !path.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    json::parse(&text).ok()
}

/// The shell's Python over `connect-tokens.json`, then
/// `connect-hosts.json`, in each home. `None` also where the Python stops
/// on an exception (a name or label that is not a string), as its
/// `2>/dev/null` and empty answer leave it.
pub fn lookup(want: &str, homes: &[PathBuf]) -> Option<Found> {
    let want_keys = host_keys(want);
    let want_bare = bare(want);
    for base in homes {
        let Some(Value::Object(pairs)) = load(&base.join("connect-tokens.json")) else {
            continue;
        };
        for (k, v) in dict(&pairs) {
            let Value::Str(t) = &v else { continue };
            let token = py::str_of(&Value::Str(t.clone()));
            if token.is_empty() {
                continue;
            }
            if want_keys.contains(&k) || bare(&k) == want_bare {
                return Some(Found::Token(read_field(&token)));
            }
        }
    }
    for base in homes {
        let Some(Value::Array(entries)) = load(&base.join("connect-hosts.json")) else {
            continue;
        };
        for entry in entries {
            let Value::Object(pairs) = entry else {
                continue;
            };
            let d = dict(&pairs);
            let get = |key: &str| d.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
            // `entry.get(x) or ""`: a falsy value is "".
            let text = |key: &str| -> Result<String, ()> {
                match get(key) {
                    Some(v) if py::truthy(&v) => match v {
                        Value::Str(t) => Ok(py::str_of(&Value::Str(t))),
                        _ => Err(()),
                    },
                    _ => Ok(String::new()),
                }
            };
            let hid = match get("id") {
                Some(v) if py::truthy(&v) => py::str_of(&v),
                _ => continue,
            };
            // A name or label that is not a string: `bare()` raises and the
            // Python prints nothing.
            let hn = text("hostname").ok()?;
            let label = text("label").ok()?;
            if want_keys.contains(&hn) || bare(&hn) == want_bare || bare(&label) == want_bare {
                return Some(Found::HostId(read_field(&hid)));
            }
        }
    }
    None
}

/// `IFS=$'\t' read -r kind val` on the Python's line: the value up to the
/// first newline, with tabs at either end dropped.
fn read_field(v: &str) -> String {
    let line = v.split('\n').next().unwrap_or("");
    line.trim_matches('\t').to_string()
}

/// The keychain services, current name first.
pub const SERVICES: [&str; 2] = ["dev.k2.connect.host-token", "com.k2so.connect.host-token"];

/// `security find-generic-password -s <svc> -a <id> -w`, then
/// `secret-tool lookup service <svc> account <id>`, each for both
/// services: the first non-empty answer, less its trailing newlines
/// (`$(…)`). The token comes back on the child's stdout, never on argv.
pub fn keychain(host_id: &str) -> Option<String> {
    for svc in SERVICES {
        if let Some(t) = run_quiet(
            "security",
            &["find-generic-password", "-s", svc, "-a", host_id, "-w"],
        ) {
            return Some(t);
        }
    }
    for svc in SERVICES {
        if let Some(t) = run_quiet(
            "secret-tool",
            &["lookup", "service", svc, "account", host_id],
        ) {
            return Some(t);
        }
    }
    None
}

fn run_quiet(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    while text.ends_with('\n') {
        text.pop();
    }
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    #[test]
    fn daemon_base_is_the_shell_s() {
        assert_eq!(daemon_base("127.0.0.1", "1234"), "http://127.0.0.1:1234");
        assert_eq!(daemon_base("box.lan", "1"), "http://box.lan:1");
        assert_eq!(
            daemon_base("https://acme.example/", "443"),
            "https://acme.example:443"
        );
        assert_eq!(
            daemon_base("HTTPS://acme.example", "443"),
            "https://acme.example:443"
        );
        assert_eq!(daemon_base("127.0.0.1", ""), "http://127.0.0.1:");
    }

    use super::*;

    #[test]
    fn where_the_binary_sends_a_token_itself() {
        let none = |_: &str| -> Option<OsString> { None };
        let proxied = |n: &str| -> Option<OsString> {
            (n == "HTTPS_PROXY").then(|| "socks5h://proxy:1080".into())
        };
        let here = |h: &str, env: &dyn Fn(&str) -> Option<OsString>| {
            let b = routed_with(h, env).expect("a host the binary reads");
            (sends_here(&b), b.via.is_some())
        };
        assert_eq!(here("box.example", &none), (true, false));
        assert_eq!(here("https://box.example:8443/", &none), (true, false));
        assert_eq!(here("box.example", &proxied), (true, true));
        assert_eq!(here("https://localhost:9", &proxied), (true, false));
        assert_eq!(here("https://127.0.0.1:9", &proxied), (true, false));
        assert_eq!(here("http://127.0.0.1:9", &proxied), (true, false));
        assert_eq!(here("http://127.3.4.5", &none), (true, false));
        assert_eq!(here("http://localhost:9", &none), (false, false));
        assert_eq!(here("http://box.example", &proxied), (false, false));
        assert_eq!(here("http://10.0.0.1:80", &none), (false, false));
        assert_eq!(base_url("box"), "https://box");
        assert_eq!(base_url("http://x:1/"), "http://x:1");
    }

    /// Every form of the person's proxy setting is the binary's now: a
    /// proxy curl reads rides in `via`, one curl refuses rides there too
    /// and fails at the connect, and none ever goes direct.
    #[test]
    fn every_proxy_setting_is_routed() {
        let env = |v: &'static str| move |n: &str| (n == "https_proxy").then(|| OsString::from(v));
        for (setting, want) in [
            ("http://p.example:3128", Some("http://p.example:3128")),
            ("https://p.example", Some("https://p.example:443")),
            ("socks4://p.example", Some("socks4://p.example:1080")),
            ("socks4a://p.example:9", Some("socks4a://p.example:9")),
            ("socks5://u:pw@p.example", Some("socks5://p.example:1080")),
            ("socks5h://[::1]:9", Some("socks5h://[::1]:9")),
            ("http://[fd00::1]:3128", Some("http://[fd00::1]:3128")),
            ("u@p.example:3", Some("http://p.example:3")),
            ("http://p.example:3?q", Some("http://p.example:3")),
            ("ftp://p.example", None),
            ("ftp.example:3128", None),
            ("http://p.example:99999", None),
        ] {
            let b = routed_with("box.example", env(setting)).expect("routed");
            let via = b.via.expect("a proxy is set: never direct");
            assert_eq!(
                via.as_ref().ok().map(ToString::to_string).as_deref(),
                want,
                "{setting}"
            );
        }
        let b = routed_with("Box.example.", env("http://p.example:1")).unwrap();
        assert!(matches!(b.via, Some(Ok(_))), "{b:?}");
    }

    #[test]
    fn base_reads_the_url_as_curl_does() {
        let b = base("box.example").unwrap();
        assert_eq!(
            (b.tls, b.host.as_str(), b.port, b.host_header.as_str()),
            (true, "box.example", 443, "box.example")
        );
        let b = base("http://127.0.0.1:4100/").unwrap();
        assert_eq!(
            (b.tls, b.host.as_str(), b.port, b.host_header.as_str()),
            (false, "127.0.0.1", 4100, "127.0.0.1:4100")
        );
        let b = base("https://box.example:443").unwrap();
        assert_eq!((b.port, b.host_header.as_str()), (443, "box.example"));
        let b = base("https://box.example:8443").unwrap();
        assert_eq!(b.host_header, "box.example:8443");
        for shell in [
            "https://box.example/prefix",
            "[::1]:4100",
            "box.example:",
            "box.example:99999",
            "box example",
            "",
            "https://",
        ] {
            assert_eq!(base(shell), None, "{shell}");
        }
    }

    /// The plain-http rule judges the URL as written, never a lookup.
    #[test]
    fn plain_http_is_for_a_loopback_literal_alone() {
        for (host, want) in [
            ("localhost", true),
            ("127.0.0.1", true),
            ("127.2.3.4", true),
            ("::1", true),
            ("[::1]", true),
            ("0:0:0:0:0:0:0:1", true),
            ("x.localhost", false),
            ("LOCALHOST", false),
            ("localhost.", false),
            ("127.example.invalid", false),
            ("127.1", false),
            ("10.0.0.5", false),
            ("box.example", false),
            ("::ffff:127.0.0.1", false),
            ("[fe80::1]", false),
            ("", false),
        ] {
            assert_eq!(literal_loopback(host), want, "{host}");
        }
        let mut b = base("http://x.localhost:9").unwrap();
        assert_eq!(
            plain_http_refusal(&b, "http://x.localhost:9").as_deref(),
            Some("Error: plain http:// only to this computer; use https:// (got: http://x.localhost:9)")
        );
        b.tls = true;
        assert_eq!(plain_http_refusal(&b, "x"), None);
        for ok in ["http://127.0.0.1:9", "http://localhost:9"] {
            assert_eq!(plain_http_refusal(&base(ok).unwrap(), ok), None, "{ok}");
        }
    }

    /// `K2_HOST=https://<name>`: TLS to `<name>` on the daemon's port, as
    /// `_k2_daemon_base` writes the URL; a URL `routed` can't read sends
    /// nothing over TCP; any other host is left as it is.
    #[test]
    fn an_https_daemon_host() {
        let client = |host: &str, port: u16| {
            let mut c = Client::new(host, port, "/p", Auth::Owner("t".into()));
            c.port_file = Some(PathBuf::from("/nonexistent/daemon.port"));
            daemon_https(&mut c);
            c
        };
        for (host, port, name, header) in [
            (
                "https://box.example",
                4711,
                "box.example",
                "box.example:4711",
            ),
            (
                "https://box.example/",
                4711,
                "box.example",
                "box.example:4711",
            ),
            ("HTTPS://localhost", 443, "localhost", "localhost"),
        ] {
            let c = client(host, port);
            assert!(c.tls, "{host}");
            assert_eq!(
                (c.host.as_str(), c.port, c.host_header.as_deref()),
                (name, port, Some(header)),
                "{host}"
            );
            assert_eq!(c.port_file, None, "{host}");
            assert_eq!(c.unsendable, None, "{host}");
            assert!(!c.force_tcp, "the cell socket stays the daemon's: {host}");
        }
        for host in [
            "https://[fd00::1]",
            "https://box.example:8443",
            "https://box/x",
        ] {
            let c = client(host, 4711);
            assert!(c.tls, "{host}");
            assert_eq!(
                c.unsendable.as_deref(),
                Some(format!("{host}:4711").as_str()),
                "{host}"
            );
        }
        for host in ["127.0.0.1", "10.0.0.5", "Https://box.example", "http://box"] {
            let c = client(host, 4711);
            assert!(!c.tls, "{host}");
            assert_eq!((c.host.as_str(), c.unsendable.as_deref()), (host, None));
            assert!(c.port_file.is_some(), "{host}");
        }
    }

    #[test]
    fn bare_drops_the_zone_as_the_python_does() {
        let z = ZONE_SUFFIX;
        assert_eq!(bare(&format!(" Box{z}{z}{z} ")), "box");
        assert_eq!(bare("box"), "box");
        assert_eq!(host_keys(""), Vec::<String>::new());
        assert_eq!(
            host_keys("Box"),
            vec![
                "Box".to_string(),
                "box".into(),
                "box".into(),
                format!("box{z}"),
                format!("box{}", z.to_uppercase())
            ]
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k5-remote-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lookup_takes_connect_tokens_first_then_a_host_id() {
        let d = scratch("lookup");
        let z = ZONE_SUFFIX;
        std::fs::write(
            d.join("connect-tokens.json"),
            format!(r#"{{"other": "x", "box{z}": "", "BOX": 7, "Box{z}": "tok\tmore"}}"#),
        )
        .unwrap();
        assert_eq!(
            lookup("box", std::slice::from_ref(&d)),
            Some(Found::Token("tok\tmore".into()))
        );
        std::fs::write(d.join("connect-tokens.json"), "{not json").unwrap();
        std::fs::write(
            d.join("connect-hosts.json"),
            r#"[1, {"hostname": "a.example", "id": ""}, {"hostname": "", "label": "Box", "id": 42}]"#,
        )
        .unwrap();
        assert_eq!(
            lookup("box", std::slice::from_ref(&d)),
            Some(Found::HostId("42".into()))
        );
        // A hostname that is not a string stops the Python: no answer.
        std::fs::write(
            d.join("connect-hosts.json"),
            r#"[{"hostname": 5, "id": "h1"}, {"hostname": "box", "id": "h2"}]"#,
        )
        .unwrap();
        assert_eq!(lookup("box", std::slice::from_ref(&d)), None);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_old_home_is_searched_only_when_it_is_a_folder() {
        let d = scratch("homes");
        let home = d.join("home");
        assert_eq!(homes(&home, Some(d.clone())), vec![home.clone()]);
        let old = old_home(&d);
        std::fs::create_dir_all(&old).unwrap();
        assert_eq!(
            homes(&home, Some(d.clone())),
            vec![home.clone(), old.clone()]
        );
        assert_eq!(homes(&old, Some(d.clone())), vec![old.clone()]);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
