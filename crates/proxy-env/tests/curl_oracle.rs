//! The black-box table: each row's settings run through the real curl
//! command (a separate process) and through this crate, against the same
//! stub proxies. Both must pick the same proxy (or none), send it the
//! same bytes, and fail with the same exit code.
//!
//! It needs curl 8.5.0 ([`proxy_env::CURL_VERSION`]): `curl` on
//! `PATH`, or `CURL_PROXY_ENV_CURL=<path>`. Another version fails the
//! test, naming it. It is ignored by default (it runs a program from the
//! machine); run it with
//! `cargo test -p proxy-env --test curl_oracle -- --ignored`.
//!
//! The differences allowed, each named in the rows it touches:
//!
//! - a curl built with GSS-API also offers SOCKS5 method 1 (GSS-API) in
//!   its greeting. The crate never does; the stubs never pick it. The
//!   method is taken out of curl's greeting before the bytes are
//!   compared, and the row says so;
//! - a proxy URL whose user or password holds a control byte once
//!   decoded: curl 8.5 drops the proxy and connects direct; the crate
//!   refuses (exit 5) and connects nowhere. Those rows ([`Row::fail_closed`])
//!   check that curl still goes direct and the crate still refuses.

#![cfg(all(unix, feature = "rustls"))]

mod common;

use common::{closed_port, shown, Mode, Pki, Stub};
use proxy_env::{connect, proxy_for, Options};
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::process::Command;
use std::time::Duration;

const UA: &str = "oracle/1";

/// One row: a name, the variables (with `{X}` for stub X's port and `\xNN`
/// for a byte), the URL, and whether the https proxy's CA is trusted.
struct Row {
    name: &'static str,
    env: &'static [(&'static str, &'static str)],
    url: &'static str,
    trust: bool,
    /// curl goes direct here and the crate refuses (exit 5).
    fail_closed: bool,
}

const fn row(
    name: &'static str,
    env: &'static [(&'static str, &'static str)],
    url: &'static str,
) -> Row {
    Row {
        name,
        env,
        url,
        trust: true,
        fail_closed: false,
    }
}

const fn fail_closed(
    name: &'static str,
    env: &'static [(&'static str, &'static str)],
    url: &'static str,
) -> Row {
    Row {
        name,
        env,
        url,
        trust: true,
        fail_closed: true,
    }
}

const T: &str = "https://h.example.invalid:8443/";
const T_IP: &str = "https://127.0.0.5:8443/";
const T_V6: &str = "https://[::1]:8443/";
const T_HTTP: &str = "http://h.example.invalid:8080/";

#[rustfmt::skip]
const ROWS: &[Row] = &[
    // Which variable names the proxy.
    row("https_proxy", &[("https_proxy", "http://127.0.0.1:{A}")], T),
    row("HTTPS_PROXY", &[("HTTPS_PROXY", "http://127.0.0.1:{A}")], T),
    row("lower wins", &[("https_proxy", "http://127.0.0.1:{A}"), ("HTTPS_PROXY", "http://127.0.0.1:{B}")], T),
    row("empty lower", &[("https_proxy", ""), ("HTTPS_PROXY", "http://127.0.0.1:{B}")], T),
    row("all_proxy", &[("all_proxy", "http://127.0.0.1:{C}")], T),
    row("ALL_PROXY", &[("ALL_PROXY", "http://127.0.0.1:{D}")], T),
    row("all lower wins", &[("all_proxy", "http://127.0.0.1:{C}"), ("ALL_PROXY", "http://127.0.0.1:{D}")], T),
    row("scheme's own first", &[("https_proxy", "http://127.0.0.1:{A}"), ("all_proxy", "http://127.0.0.1:{C}")], T),
    row("http_proxy not for https", &[("http_proxy", "http://127.0.0.1:{A}")], T),
    row("HTTP_PROXY not for https", &[("HTTP_PROXY", "http://127.0.0.1:{A}")], T),
    row("http_proxy for http", &[("http_proxy", "http://127.0.0.1:{A}")], T_HTTP),
    row("HTTP_PROXY never", &[("HTTP_PROXY", "http://127.0.0.1:{A}")], T_HTTP),
    row("HTTP_PROXY, ALL_PROXY", &[("HTTP_PROXY", "http://127.0.0.1:{A}"), ("ALL_PROXY", "http://127.0.0.1:{D}")], T_HTTP),
    row("https_proxy not for http", &[("https_proxy", "http://127.0.0.1:{A}")], T_HTTP),
    // no_proxy.
    row("no_proxy the name", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "h.example.invalid")], T),
    row("no_proxy a domain", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "example.invalid")], T),
    row("no_proxy .domain", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", ".example.invalid")], T),
    row("no_proxy not at a dot", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "ample.invalid")], T),
    row("no_proxy a longer name", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "x.h.example.invalid")], T),
    row("no_proxy *", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "*")], T),
    row("NO_PROXY *", &[("https_proxy", "http://127.0.0.1:{A}"), ("NO_PROXY", "*")], T),
    row("no_proxy lower wins", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "other.invalid"), ("NO_PROXY", "*")], T),
    row("no_proxy empty", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", ""), ("NO_PROXY", "h.example.invalid")], T),
    row("no_proxy list", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "other.invalid,h.example.invalid")], T),
    row("no_proxy blank", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "other.invalid h.example.invalid")], T),
    row("no_proxy comma blank", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "other.invalid, h.example.invalid")], T),
    row("no_proxy tab", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "h.example.invalid\\x09other.invalid")], T),
    row("no_proxy *.domain", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "*.example.invalid")], T),
    row("no_proxy *,x", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "*,x")], T),
    row("no_proxy case", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "H.EXAMPLE.INVALID")], T),
    row("no_proxy trailing dot", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "h.example.invalid.")], T),
    row("no_proxy with a port", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "h.example.invalid:8443")], T),
    row("no_proxy not text", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "\\xff,h.example.invalid")], T),
    row("no_proxy, target dot", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "h.example.invalid")], "https://h.example.invalid.:8443/"),
    row("no_proxy an address", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "127.0.0.5")], T_IP),
    row("no_proxy /8", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "127.0.0.0/8")], T_IP),
    row("no_proxy /31 in", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "127.0.0.4/31")], T_IP),
    row("no_proxy /31 out", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "127.0.0.6/31")], T_IP),
    row("no_proxy /33", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "127.0.0.0/33")], T_IP),
    row("no_proxy /0 same", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "127.0.0.5/0")], T_IP),
    row("no_proxy /0 other", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "127.0.0.0/0")], T_IP),
    row("no_proxy /1", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "0.0.0.0/1")], T_IP),
    row("no_proxy /8x", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "127.0.0.0/8x")], T_IP),
    row("no_proxy a name, an address", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "localhost")], T_IP),
    row("no_proxy ::1", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "::1")], T_V6),
    row("no_proxy [::1]", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "[::1]")], T_V6),
    row("no_proxy ::/127", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "::/127")], T_V6),
    row("no_proxy ::1/128", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "::1/128")], T_V6),
    row("no_proxy ::/64", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "::/64")], T_V6),
    row("no_proxy ::1/0", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "::1/0")], T_V6),
    row("no_proxy other v6", &[("https_proxy", "http://127.0.0.1:{A}"), ("no_proxy", "::2")], T_V6),
    // The target, through the proxy.
    row("target v6", &[("https_proxy", "http://127.0.0.1:{A}")], T_V6),
    row("target capital", &[("https_proxy", "http://127.0.0.1:{A}")], "https://H.Example.invalid:8443/"),
    row("target trailing dot", &[("https_proxy", "http://127.0.0.1:{A}")], "https://h.example.invalid.:8443/"),
    row("target port 443", &[("https_proxy", "http://127.0.0.1:{A}")], "https://h.example.invalid/"),
    row("target address", &[("https_proxy", "http://127.0.0.1:{A}")], T_IP),
    // The proxy URL.
    row("no scheme", &[("https_proxy", "127.0.0.1:{A}")], T),
    row("scheme case, slash", &[("https_proxy", "HTTP://127.0.0.1:{A}/")], T),
    row("path query fragment", &[("https_proxy", "http://127.0.0.1:{A}/p?q=1#f")], T),
    row("query", &[("https_proxy", "http://127.0.0.1:{A}?q=1")], T),
    row("path not text", &[("https_proxy", "http://127.0.0.1:{A}/\\xff")], T),
    row("user and password", &[("https_proxy", "http://u:p@127.0.0.1:{A}")], T),
    row("escapes", &[("https_proxy", "http://ac%40me:p%3As%2Fs%20w@127.0.0.1:{A}")], T),
    row("user only", &[("https_proxy", "http://only@127.0.0.1:{A}")], T),
    row("empty password", &[("https_proxy", "http://u:@127.0.0.1:{A}")], T),
    row("two @", &[("https_proxy", "http://a@b@127.0.0.1:{A}")], T),
    row("bad escape", &[("https_proxy", "http://u:p%zz@127.0.0.1:{A}")], T),
    fail_closed("escaped newline", &[("https_proxy", "http://u:p%0a@127.0.0.1:{A}")], T),
    fail_closed("escaped NUL", &[("https_proxy", "http://u%00:p@127.0.0.1:{A}")], T),
    row("escaped DEL", &[("https_proxy", "http://u%7f:p@127.0.0.1:{A}")], T),
    row("raw DEL, user", &[("https_proxy", "http://u\\x7f:p@127.0.0.1:{A}")], T),
    row("raw DEL, path", &[("https_proxy", "http://127.0.0.1:{A}/\\x7f")], T),
    row("raw DEL, host", &[("https_proxy", "http://127.0.0.1\\x7f:{A}")], T),
    row("escaped tab in the host", &[("https_proxy", "http://127.0%09.0.1:{A}")], T),
    row("no scheme, user", &[("https_proxy", "user@127.0.0.1:{A}")], T),
    row("a proxy name", &[("https_proxy", "http://localhost:{A}")], T),
    row("tab", &[("https_proxy", "http://127.0.0.1:{A}\\x09")], T),
    row("blank", &[("https_proxy", "http://127.0.0.1 :{A}")], T),
    row("port 99999", &[("https_proxy", "http://127.0.0.1:99999")], T),
    row("port 0", &[("https_proxy", "http://127.0.0.1:0")], T),
    row("port not a number", &[("https_proxy", "http://127.0.0.1:1x")], T),
    row("empty port", &[("https_proxy", "http://127.0.0.1:")], T),
    row("default port", &[("https_proxy", "http://127.0.0.1")], T),
    row("default port, no scheme", &[("https_proxy", "127.0.0.1")], T),
    row("socks default port", &[("https_proxy", "socks5h://127.0.0.1")], T),
    row("ftp://", &[("https_proxy", "ftp://127.0.0.1:{A}")], T),
    row("gopher://", &[("https_proxy", "gopher://127.0.0.1:{A}")], T),
    row("socks://", &[("https_proxy", "socks://127.0.0.1:{S4}")], T),
    row("guessed ftp.", &[("https_proxy", "ftp.example.invalid:{A}")], T),
    row("proxy v6", &[("https_proxy", "http://[::1]:{V6}")], T),
    row("proxy v6, no scheme", &[("https_proxy", "[::1]:{V6}")], T),
    row("proxy v6 broken", &[("https_proxy", "http://[::1:{V6}")], T),
    row("escaped host", &[("https_proxy", "http://127%2e0.0.1:{A}")], T),
    row("no host", &[("https_proxy", "http://")], T),
    row("proxy name doesn't resolve", &[("https_proxy", "http://p.example.invalid:{A}")], T),
    row("proxy refuses", &[("https_proxy", "http://127.0.0.1:{CLOSED}")], T),
    // SOCKS.
    row("socks5 a name", &[("https_proxy", "socks5://127.0.0.1:{S5}")], T),
    row("socks5h a name", &[("https_proxy", "socks5h://127.0.0.1:{S5}")], T),
    row("socks5 an address", &[("https_proxy", "socks5://127.0.0.1:{S5}")], T_IP),
    row("socks5h an address", &[("https_proxy", "socks5h://127.0.0.1:{S5}")], T_IP),
    row("socks5h v6", &[("https_proxy", "socks5h://127.0.0.1:{S5}")], T_V6),
    row("socks5 v6", &[("https_proxy", "socks5://127.0.0.1:{S5}")], T_V6),
    row("SOCKS5H case", &[("https_proxy", "SOCKS5H://127.0.0.1:{S5}")], T),
    row("socks5h capital", &[("https_proxy", "socks5h://127.0.0.1:{S5}")], "https://H.Example.invalid:8443/"),
    row("socks5h password", &[("https_proxy", "socks5h://acme:pw@127.0.0.1:{S5A}")], T),
    row("socks5h wrong password", &[("https_proxy", "socks5h://acme:no@127.0.0.1:{S5A}")], T),
    row("socks5h no password", &[("https_proxy", "socks5h://127.0.0.1:{S5A}")], T),
    row("socks5h user only", &[("https_proxy", "socks5h://acme@127.0.0.1:{S5A}")], T),
    row("socks5h escapes", &[("https_proxy", "socks5h://ac%40me:p%3A@127.0.0.1:{S5A}")], T),
    row("socks5 password, no auth stub", &[("https_proxy", "socks5h://acme:pw@127.0.0.1:{S5}")], T),
    row("all_proxy socks5h", &[("all_proxy", "socks5h://127.0.0.1:{S5}")], T),
    row("socks4 an address", &[("https_proxy", "socks4://127.0.0.1:{S4}")], T_IP),
    row("socks4 a name", &[("https_proxy", "socks4://127.0.0.1:{S4}")], T),
    row("socks4 v6", &[("https_proxy", "socks4://127.0.0.1:{S4}")], T_V6),
    row("socks4a a name", &[("https_proxy", "socks4a://127.0.0.1:{S4}")], T),
    row("socks4a an address", &[("https_proxy", "socks4a://127.0.0.1:{S4}")], T_IP),
    row("socks4a user", &[("https_proxy", "socks4a://user:pw@127.0.0.1:{S4}")], T),
    // An https:// proxy.
    row("https proxy", &[("https_proxy", "https://127.0.0.1:{TLS}")], T),
    row("https proxy, password", &[("https_proxy", "https://u:p@127.0.0.1:{TLS}")], T),
    row("https proxy by name", &[("https_proxy", "https://localhost:{TLS}")], T),
    Row { name: "https proxy, CA not trusted", env: &[("https_proxy", "https://127.0.0.1:{TLS}")], url: T, trust: false, fail_closed: false },
    row("https proxy, default port", &[("https_proxy", "https://127.0.0.1")], T),
];

/// `\xNN` to its byte.
fn unescape(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"\\x") && i + 4 <= b.len() {
            let hex = std::str::from_utf8(&b[i + 2..i + 4]).unwrap();
            out.push(u8::from_str_radix(hex, 16).unwrap());
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// What one side did: its exit code (0: the tunnel opened), and each
/// stub's connections' bytes.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    code: i32,
    contacted: Vec<(String, Vec<String>)>,
}

struct Stubs {
    named: Vec<(&'static str, Stub)>,
    closed: u16,
    pki: Pki,
    ca_file: std::path::PathBuf,
}

impl Stubs {
    fn start(dir: &std::path::Path) -> Stubs {
        let pki = Pki::new();
        let ca_file = dir.join("ca.pem");
        std::fs::write(&ca_file, &pki.ca_pem).unwrap();
        let refuse = "403 Forbidden";
        let named = vec![
            ("A", Stub::start(Mode::Http(refuse))),
            ("B", Stub::start(Mode::Http(refuse))),
            ("C", Stub::start(Mode::Http(refuse))),
            ("D", Stub::start(Mode::Http(refuse))),
            ("TLS", Stub::start(Mode::https(refuse, &pki))),
            ("S5", Stub::start(Mode::Socks5(None, 2))),
            (
                "S5A",
                Stub::start(Mode::Socks5(Some((b"acme".to_vec(), b"pw".to_vec())), 2)),
            ),
            ("S4", Stub::start(Mode::Socks4(91))),
            ("V6", Stub::bind("[::1]:0", Mode::Http(refuse))),
            // The default ports: curl's 1080 (443 needs root: nothing
            // listens there, and both sides must fail to connect).
            ("1080", Stub::bind("127.0.0.1:1080", Mode::Raw)),
        ];
        Stubs {
            named,
            closed: closed_port(),
            pki,
            ca_file,
        }
    }

    fn fill(&self, v: &str) -> Vec<u8> {
        let mut s = v.replace("{CLOSED}", &self.closed.to_string());
        for (n, stub) in &self.named {
            s = s.replace(&format!("{{{n}}}"), &stub.port().to_string());
        }
        unescape(&s)
    }

    /// Each stub's connections since the last take, the GSS-API method
    /// taken out of a SOCKS5 greeting (and whether one was).
    fn take(&self) -> (Vec<(String, Vec<String>)>, bool) {
        std::thread::sleep(Duration::from_millis(150));
        let mut gss = false;
        let mut out = Vec::new();
        for (n, stub) in &self.named {
            let conns = stub.take();
            if conns.is_empty() {
                continue;
            }
            let conns = conns
                .into_iter()
                .map(|mut c| {
                    if (n.starts_with("S5") || *n == "1080") && c.len() >= 2 && c[0] == 5 {
                        let k = usize::from(c[1]);
                        if c.len() >= 2 + k && c[2..2 + k].contains(&1) {
                            gss = true;
                            let methods: Vec<u8> =
                                c[2..2 + k].iter().copied().filter(|m| *m != 1).collect();
                            let mut fixed = vec![5, methods.len() as u8];
                            fixed.extend_from_slice(&methods);
                            fixed.extend_from_slice(&c[2 + k..]);
                            c = fixed;
                        }
                    }
                    shown(&c)
                })
                .collect();
            out.push((n.to_string(), conns));
        }
        (out, gss)
    }
}

fn curl_bin() -> OsString {
    std::env::var_os("CURL_PROXY_ENV_CURL").unwrap_or_else(|| "curl".into())
}

fn run_curl(stubs: &Stubs, r: &Row, env: &HashMap<String, Vec<u8>>) -> i32 {
    let mut c = Command::new(curl_bin());
    c.env_clear()
        .env("PATH", std::env::var_os("PATH").expect("a PATH"))
        .args([
            "-q",
            "-s",
            "-S",
            "-o",
            "/dev/null",
            "--connect-timeout",
            "5",
            "-A",
            UA,
        ]);
    if r.trust {
        c.arg("--proxy-cacert").arg(&stubs.ca_file);
    }
    for (k, v) in env {
        c.env(k, OsString::from_vec(v.clone()));
    }
    c.arg(r.url);
    let o = c.output().expect("run curl");
    o.status.code().expect("curl exited")
}

/// The URL's scheme, host (as written, brackets kept) and port.
fn split(url: &str) -> (&str, &str, u16) {
    let (scheme, rest) = url.split_once("://").unwrap();
    let auth = rest.split('/').next().unwrap();
    let default = if scheme == "https" { 443 } else { 80 };
    if let Some(end) = auth.find(']') {
        let port = auth[end + 1..]
            .strip_prefix(':')
            .map_or(default, |p| p.parse().unwrap());
        return (scheme, &auth[..=end], port);
    }
    match auth.rsplit_once(':') {
        Some((h, p)) => (scheme, h, p.parse().unwrap()),
        None => (scheme, auth, default),
    }
}

fn run_crate(stubs: &Stubs, r: &Row, env: &HashMap<String, Vec<u8>>) -> i32 {
    let (scheme, host, port) = split(r.url);
    let lookup = |n: &str| env.get(n).map(|v| OsString::from_vec(v.clone()));
    let proxy = match proxy_for(scheme, host, lookup) {
        Ok(p) => p,
        Err(e) => return e.curl_exit_code(),
    };
    let opts = Options::new()
        .user_agent(UA)
        .timeout(Duration::from_secs(5))
        .proxy_tls(stubs.pki.client(r.trust));
    let tunnel_host = host;
    match connect(proxy.as_ref(), tunnel_host, port, &opts) {
        Ok(_) => 0,
        Err(e) => e.curl_exit_code(),
    }
}

#[test]
#[ignore = "runs the curl command (8.5.0): cargo test --test curl_oracle -- --ignored"]
fn the_crate_does_what_curl_does() {
    let v = Command::new(curl_bin())
        .arg("--version")
        .output()
        .expect("run curl --version (CURL_PROXY_ENV_CURL names another curl)");
    let first = String::from_utf8_lossy(&v.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    let want = format!("curl {} ", proxy_env::CURL_VERSION);
    assert!(
        first.starts_with(&want),
        "this table is checked against curl {}; this curl is {first:?}",
        proxy_env::CURL_VERSION
    );
    println!("curl: {first}");
    let dir = std::env::temp_dir().join(format!("proxy-env-oracle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let stubs = Stubs::start(&dir);
    let mut wrong = Vec::new();
    for r in ROWS {
        let env: HashMap<String, Vec<u8>> = r
            .env
            .iter()
            .map(|(k, v)| (k.to_string(), stubs.fill(v)))
            .collect();
        stubs.take();
        let code = run_curl(&stubs, r, &env);
        let (contacted, gss) = stubs.take();
        let curl = Seen { code, contacted };
        let code = run_crate(&stubs, r, &env);
        let (contacted, _) = stubs.take();
        let ours = Seen { code, contacted };
        // An http:// target goes as a proxy request, not a tunnel: only
        // which proxy is compared.
        let same = if r.fail_closed {
            curl == Seen {
                code: 6,
                contacted: vec![],
            } && ours
                == Seen {
                    code: 5,
                    contacted: vec![],
                }
        } else if r.url.starts_with("http://") {
            let names = |s: &Seen| {
                s.contacted
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect::<Vec<_>>()
            };
            names(&curl) == names(&ours)
        } else {
            curl == ours
        };
        println!(
            "{} {:?}{}\n  curl:  {:?}\n  crate: {:?}",
            if same { "same" } else { "DIFFERENT" },
            r.name,
            if gss {
                " (curl's GSS-API method taken out)"
            } else {
                ""
            },
            curl,
            ours
        );
        if !same {
            wrong.push(r.name);
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        wrong.is_empty(),
        "{} rows differ from curl: {wrong:?}",
        wrong.len()
    );
}
