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

use super::client::{Auth, Client};
use crate::out::json::{self, Value};
use crate::out::py;
use k2_client::env_names;
use k2_client::hosts::ZONE_SUFFIX;
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
    })
}

/// `_apply_remote_host`'s swap, once the token is known.
pub fn apply(client: &mut Client, base: &Base, token: String) {
    client.host = base.host.clone();
    client.port = base.port;
    client.tls = base.tls;
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
        let k = k.as_str().map_or_else(|| py::str_of(&Value::Str(k.clone())), str::to_string);
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
            let Value::Object(pairs) = entry else { continue };
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
        if let Some(t) = run_quiet("security", &["find-generic-password", "-s", svc, "-a", host_id, "-w"]) {
            return Some(t);
        }
    }
    for svc in SERVICES {
        if let Some(t) = run_quiet("secret-tool", &["lookup", "service", svc, "account", host_id]) {
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
    use super::*;

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
            lookup("box", &[d.clone()]),
            Some(Found::Token("tok\tmore".into()))
        );
        std::fs::write(d.join("connect-tokens.json"), "{not json").unwrap();
        std::fs::write(
            d.join("connect-hosts.json"),
            r#"[1, {"hostname": "a.example", "id": ""}, {"hostname": "", "label": "Box", "id": 42}]"#,
        )
        .unwrap();
        assert_eq!(lookup("box", &[d.clone()]), Some(Found::HostId("42".into())));
        // A hostname that is not a string stops the Python: no answer.
        std::fs::write(
            d.join("connect-hosts.json"),
            r#"[{"hostname": 5, "id": "h1"}, {"hostname": "box", "id": "h2"}]"#,
        )
        .unwrap();
        assert_eq!(lookup("box", &[d.clone()]), None);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_old_home_is_searched_only_when_it_is_a_folder() {
        let d = scratch("homes");
        let home = d.join("home");
        assert_eq!(homes(&home, Some(d.clone())), vec![home.clone()]);
        let old = old_home(&d);
        std::fs::create_dir_all(&old).unwrap();
        assert_eq!(homes(&home, Some(d.clone())), vec![home.clone(), old.clone()]);
        assert_eq!(homes(&old, Some(d.clone())), vec![old.clone()]);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
