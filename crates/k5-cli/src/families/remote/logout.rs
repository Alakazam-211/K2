//! `k5 remote logout --host HOST [--token-stdin]`, as the shell CLI's
//! `cmd_remote_logout` answers it (`cli/src/remote-logout.sh`): one POST
//! of `{}` to `<HOST>/cli/auth/logout` with the token as `Authorization:
//! Bearer`, sent once (`_K2_CURL_TRIES=1`), then the saved copies of that
//! token for HOST go (`_remote_forget_token`): the `connect-tokens.json`
//! entries in the K2 home and in the old name's home, and the keychain
//! items holding exactly it.
//!
//! The token is never on argv, in a URL, in a log line or in a child's
//! environment: it is the body of nothing but the request's header, and
//! the keychain tools get only the service and the account.
//!
//! HTTPS to another computer goes through the person's proxy as `_k2_curl`
//! sends it ([`remote::routed`], curl's rule).
//!
//! Handed to the shell ([`run`] returns `None`), all before stdin is
//! read and before anything is sent: a HOST URL the binary doesn't send
//! itself ([`remote::base`]), and a `connect-tokens.json` or
//! `connect-hosts.json` the shell's Python would stop on (not JSON, not
//! UTF-8, a tokens file that is not an object, a host name or label that
//! is not a string).

use super::super::setup::utf8_locale;
use crate::exit::Exit;
use crate::out::json::{self, Options, Value};
use crate::out::{self, py};
use crate::prompt;
use crate::transport::client::{Auth, Client, OwnCurl};
use crate::transport::remote;
use k2_client::cli_install::LEGACY_NAME;
use k2_client::cli_name;
use k2_client::hosts::ZONE_SUFFIX;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// The route.
pub const LOGOUT: &str = "/cli/auth/logout";
/// `--max-time 20`.
pub const MAX_TIME: Duration = Duration::from_secs(20);
/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    /// The host (less one trailing `/`), the URL the shell sends to, and
    /// `--token-stdin`.
    Send {
        host: String,
        base: String,
        from_stdin: bool,
    },
    /// A line on stderr, exit 2.
    Usage(String),
}

/// The usage line.
pub fn usage_line() -> String {
    format!(
        "usage: {} remote logout --host HOST [--token-stdin]",
        cli_name!()
    )
}

/// The words after `remote logout`. `-h` and `--help` are the front's.
pub fn parse(words: &[&str]) -> Form {
    let c = cli_name!();
    let usage = || Form::Usage(usage_line());
    let mut host = String::new();
    let mut from_stdin = false;
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--host" => match words.get(i + 1) {
                Some(v) if !v.is_empty() => {
                    host = v.to_string();
                    i += 2;
                }
                _ => return usage(),
            },
            w if w.starts_with("--host=") => {
                host = w["--host=".len()..].to_string();
                i += 1;
            }
            "--token-stdin" => {
                from_stdin = true;
                i += 1;
            }
            w if w == "--token" || w.starts_with("--token=") => {
                return Form::Usage(format!(
                    "{c} remote logout: no --token (a command line is visible to every account on this computer); use --token-stdin"
                ))
            }
            _ => return usage(),
        }
    }
    if host.is_empty() {
        return usage();
    }
    if let Some(h) = host.strip_suffix('/') {
        host = h.to_string();
    }
    let base = if host.starts_with("https://") || host.starts_with("HTTPS://") {
        host.clone()
    } else if host.starts_with("http://") || host.starts_with("HTTP://") {
        if !loopback(&host) {
            return Form::Usage(format!(
                "{c} remote logout: plain http:// only to this computer; use https://"
            ));
        }
        host.clone()
    } else {
        format!("https://{host}")
    };
    Form::Send {
        host,
        base,
        from_stdin,
    }
}

/// `_k2_url_is_literal_loopback`: this computer as written, never
/// resolved (`x.localhost` is not), the transport's shared rule.
pub fn loopback(url: &str) -> bool {
    crate::prelude::plain_http::url_is_literal_loopback(url)
}

/// The homes and labels `_remote_forget_token`'s Python writes: the K2
/// home, then the old name's home when it is a folder and another one.
pub fn token_homes(home: &Path, user_home: Option<&Path>) -> Vec<(PathBuf, String)> {
    let mut homes = vec![(home.to_path_buf(), "connect-tokens.json".to_string())];
    if let Some(u) = user_home {
        let alt = remote::old_home(u);
        let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if alt.is_dir() && real(&alt) != real(home) {
            homes.push((alt, format!("the old .{LEGACY_NAME} connect-tokens.json")));
        }
    }
    homes
}

/// The homes `_remote_host_id_for_host`'s Python reads: the K2 home, then
/// the old name's home when it is a folder and not the same path.
pub fn host_id_homes(home: &Path, user_home: Option<&Path>) -> Vec<PathBuf> {
    let mut homes = vec![home.to_path_buf()];
    if let Some(u) = user_home {
        let alt = remote::old_home(u);
        if alt != home && alt.is_dir() {
            homes.push(alt);
        }
    }
    homes
}

/// A file the Python reads: `None` when it is not there (not a file),
/// `Some(Err)` when `json.load` would stop on it.
fn load(path: &Path) -> Option<Result<Value, ()>> {
    if !path.is_file() {
        return None;
    }
    let read = std::fs::read(path)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| json::parse(&t).ok());
    Some(read.ok_or(()))
}

/// Whether the shell's two Pythons would run through without stopping on
/// these homes' files ([`run`] hands the command to the shell when not).
pub fn files_readable(tokens: &[(PathBuf, String)], hosts: &[PathBuf]) -> bool {
    for (base, _) in tokens {
        match load(&base.join("connect-tokens.json")) {
            None | Some(Ok(Value::Object(_))) => {}
            Some(_) => return false,
        }
    }
    for base in hosts {
        match load(&base.join("connect-hosts.json")) {
            None => {}
            Some(Err(())) => return false,
            Some(Ok(Value::Array(entries))) => {
                for e in entries {
                    let Value::Object(pairs) = e else { continue };
                    for key in ["hostname", "label"] {
                        let v = last(&pairs, key);
                        if v.is_some_and(|v| py::truthy(v) && !matches!(v, Value::Str(_))) {
                            return false;
                        }
                    }
                }
            }
            Some(Ok(_)) => {}
        }
    }
    true
}

/// A Python dict's value for `key`: the last one given.
fn last<'a>(pairs: &'a [(json::Text, Value)], key: &str) -> Option<&'a Value> {
    pairs
        .iter()
        .rev()
        .find(|(k, _)| k.as_str() == Some(key))
        .map(|(_, v)| v)
}

/// `entry.get(key) or ""` for a value that is a string or falsy.
fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(v @ Value::Str(_)) if py::truthy(v) => py::str_of(v),
        _ => String::new(),
    }
}

/// `_remote_host_id_for_host HOST`: the id `connect-hosts.json` gives
/// HOST (by its name, its bare form, or the label), or `None`.
pub fn host_id(want: &str, homes: &[PathBuf]) -> Option<String> {
    let b = remote::bare(want);
    let want_keys = [
        want.to_string(),
        want.to_lowercase(),
        b.clone(),
        format!("{b}{ZONE_SUFFIX}"),
        format!("{b}{}", ZONE_SUFFIX.to_uppercase()),
    ];
    for base in homes {
        let Some(Ok(Value::Array(entries))) = load(&base.join("connect-hosts.json")) else {
            continue;
        };
        for entry in entries {
            let Value::Object(pairs) = &entry else {
                continue;
            };
            let id = match last(pairs, "id") {
                Some(v) if py::truthy(v) => py::str_of(v),
                _ => continue,
            };
            let hn = text_of(last(pairs, "hostname"));
            let label = text_of(last(pairs, "label"));
            if want_keys.contains(&hn) || remote::bare(&hn) == b || remote::bare(&label) == b {
                return Some(id);
            }
        }
    }
    None
}

/// The tokens Python's keys for HOST: HOST stripped, its form less the
/// zone, and that form with the zone; each once, none empty.
pub fn forget_keys(host: &str) -> Vec<String> {
    let host = host.trim_matches(super::super::py_space);
    let upper = ZONE_SUFFIX.to_uppercase();
    let mut b = host;
    while let Some(s) = b.strip_suffix(ZONE_SUFFIX) {
        b = s;
    }
    while let Some(s) = b.strip_suffix(upper.as_str()) {
        b = s;
    }
    let mut keys: Vec<String> = Vec::new();
    for k in [host.to_string(), b.to_string(), format!("{b}{ZONE_SUFFIX}")] {
        if !k.is_empty() && !keys.contains(&k) {
            keys.push(k);
        }
    }
    keys
}

/// The tokens Python on one file's object: the object less the keys whose
/// value is the token, or `None` when no key holds it.
pub fn without_token(
    pairs: &[(json::Text, Value)],
    keys: &[String],
    token: &str,
) -> Option<Vec<(json::Text, Value)>> {
    let holds =
        |k: &str| matches!(last(pairs, k), Some(Value::Str(t)) if t.as_str() == Some(token));
    let gone: Vec<&String> = keys.iter().filter(|k| holds(k)).collect();
    if gone.is_empty() {
        return None;
    }
    Some(
        pairs
            .iter()
            .filter(|(k, _)| !gone.iter().any(|g| k.as_str() == Some(g.as_str())))
            .cloned()
            .collect(),
    )
}

/// `json.dump(data, fh, indent=2)` to `<path>.tmp`, mode 0600, then over
/// `path`.
fn write_tokens(path: &Path, pairs: Vec<(json::Text, Value)>) -> std::io::Result<()> {
    let opts = Options {
        indent: Some(2),
        ..Options::default()
    };
    let text = json::to_string(&Value::Object(pairs), &opts)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        use std::io::Write;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(text.as_bytes())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}

/// A keychain tool, with no remote token in its environment.
fn tool(program: &str) -> Command {
    let mut cmd = Command::new(program);
    for name in k2_client::env_names::REMOTE_TOKEN {
        cmd.env_remove(name);
    }
    cmd
}

/// `$(<program> <args> 2>/dev/null || true)`: its stdout less trailing
/// newlines; `None` when it can't be started (`command -v` finds none).
fn capture(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let out = tool(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let mut b = out.stdout;
    b.retain(|c| *c != 0);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    Some(b)
}

/// `<program> <args> >/dev/null 2>&1`: it ran and exited 0.
fn quiet(program: &str, args: &[&str]) -> bool {
    tool(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `_remote_forget_token HOST TOKEN`: what went, or the error line(s).
fn forget(host: &str, token: &str, home: &Path) -> Result<String, Vec<String>> {
    let c = cli_name!();
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    let keys = forget_keys(host);
    let mut gone: Vec<String> = Vec::new();
    for (base, label) in token_homes(home, user_home.as_deref()) {
        let path = base.join("connect-tokens.json");
        let Some(Ok(Value::Object(pairs))) = load(&path) else {
            continue;
        };
        let Some(kept) = without_token(&pairs, &keys, token) else {
            continue;
        };
        if let Err(e) = write_tokens(&path, kept) {
            return Err(vec![
                e.to_string(),
                format!("{c} remote logout: could not update connect-tokens.json"),
            ]);
        }
        gone.push(label);
    }
    let mut removed = gone.join(", ");
    let homes = host_id_homes(home, user_home.as_deref());
    let Some(id) = host_id(host, &homes) else {
        return Ok(removed);
    };
    let mut kc_gone = false;
    for svc in remote::SERVICES {
        if let Some(kc) = capture(
            "security",
            &["find-generic-password", "-s", svc, "-a", &id, "-w"],
        ) {
            if !kc.is_empty() && kc == token.as_bytes() {
                if !quiet(
                    "security",
                    &["delete-generic-password", "-s", svc, "-a", &id],
                ) {
                    return Err(vec![format!(
                        "{c} remote logout: could not remove the keychain item {svc}"
                    )]);
                }
                kc_gone = true;
            }
        }
        if let Some(kc) = capture("secret-tool", &["lookup", "service", svc, "account", &id]) {
            if !kc.is_empty() && kc == token.as_bytes() {
                if !quiet("secret-tool", &["clear", "service", svc, "account", &id]) {
                    return Err(vec![format!(
                        "{c} remote logout: could not remove the keyring item {svc}"
                    )]);
                }
                kc_gone = true;
            }
        }
    }
    if kc_gone {
        if !removed.is_empty() {
            removed.push_str(", ");
        }
        removed.push_str("keychain");
    }
    Ok(removed)
}

/// `${body:0:300}` after `tr '\n' ' '`: 300 characters in a UTF-8
/// locale (a byte that starts no whole character counts as one), else
/// 300 bytes.
pub fn shown_body(body: &[u8], utf8: impl FnOnce() -> bool) -> Vec<u8> {
    const N: usize = 300;
    let body: Vec<u8> = body
        .iter()
        .filter(|b| **b != 0)
        .map(|b| if *b == b'\n' { b' ' } else { *b })
        .collect();
    if body.len() <= N || body[..N].is_ascii() || !utf8() {
        return body[..body.len().min(N)].to_vec();
    }
    let mut i = 0;
    for _ in 0..N {
        if i >= body.len() {
            break;
        }
        i += char_len(&body[i..]);
    }
    body[..i].to_vec()
}

fn char_len(b: &[u8]) -> usize {
    let want = match b[0] {
        0x00..=0x7f => return 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return 1,
    };
    match b.get(..want) {
        Some(c) if std::str::from_utf8(c).is_ok() => want,
        _ => 1,
    }
}

/// Lines on stderr, as bytes.
fn err_bytes(lines: &[Vec<u8>]) {
    let mut all = Vec::new();
    for l in lines {
        all.extend_from_slice(l);
        all.push(b'\n');
    }
    let _ = out::write_flushed(&mut std::io::stderr().lock(), &all);
}

/// `cmd_remote_logout` after the router's checks. `None`: the shell
/// answers (nothing read, nothing sent).
/// `hand`: whether the shell's own token came from disk, and whether
/// the passport rule would refuse (its hand-over headers).
pub fn run(words: &[&str], home: &Path, hand: (bool, bool)) -> Option<Exit> {
    let (host, base, from_stdin) = match parse(words) {
        Form::Usage(line) => {
            out::stderr_line(&line);
            return Some(Exit::Usage);
        }
        Form::Send {
            host,
            base,
            from_stdin,
        } => (host, base, from_stdin),
    };
    let target = remote::routed(&base)?;
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    if !files_readable(
        &token_homes(home, user_home.as_deref()),
        &host_id_homes(home, user_home.as_deref()),
    ) {
        return None;
    }
    Some(send(&host, &target, from_stdin, home, hand))
}

fn send(
    host: &str,
    target: &remote::Base,
    from_stdin: bool,
    home: &Path,
    hand: (bool, bool),
) -> Exit {
    let c = cli_name!();
    let token = if from_stdin {
        let secret = prompt::read_secret_stdin(&format!("Token for {host}: "));
        let mut t = String::from_utf8_lossy(secret.expose()).into_owned();
        if t.ends_with('\r') {
            t.pop();
        }
        if t.is_empty() {
            out::stderr_line(&format!(
                "{c} remote logout: --token-stdin: no token on stdin"
            ));
            return Exit::Usage;
        }
        t
    } else {
        match remote::token_for_host(host, "", home) {
            Some(t) if !t.is_empty() => t,
            _ => {
                out::stderr_line(&format!(
                    "{c} remote logout: no saved token for '{host}' ($K2_REMOTE_TOKEN, {}/connect-tokens.json or the keychain); pass it with --token-stdin",
                    home.display()
                ));
                return Exit::Usage;
            }
        }
    };
    let mut client = Client::new(target.host.clone(), target.port, "", Auth::None);
    remote::apply(&mut client, target, token.clone());
    // The hand-over headers are the shell's: `X-K5-Owner-Source: disk`
    // when its own token came from disk, whatever token this request
    // carries.
    (client.owner_from_disk, client.would_refuse) = hand;
    let how = OwnCurl {
        post: true,
        credential: Some(&token),
        max_time: MAX_TIME,
        note: false,
    };
    let answer = client.own_curl_once(LOGOUT, how, b"{}");
    let (code, body) = match answer {
        Ok(r) => (r.status, r.body),
        Err(_) => {
            out::stderr_line(&format!(
                "{c} remote logout: no answer from {host}; nothing removed here (the token may still work)"
            ));
            return Exit::Error;
        }
    };
    if !(200..300).contains(&code) {
        let shown = shown_body(&body, utf8_locale);
        let (said, exit) = if code == 401 || code == 403 {
            (format!("refused (HTTP {code:03})"), Exit::Refused)
        } else {
            (format!("answered HTTP {code:03}"), Exit::Error)
        };
        let mut first = format!("{c} remote logout: {host} {said}: ").into_bytes();
        first.extend_from_slice(&shown);
        err_bytes(&[first, b"Nothing removed here.".to_vec()]);
        return exit;
    }
    match forget(host, &token, home) {
        Ok(removed) if removed.is_empty() => out_line(&format!(
            "signed out of {host} (it ends this session if it had one); no saved copy of this token here"
        )),
        Ok(removed) => out_line(&format!(
            "signed out of {host} (it ends this session if it had one); removed the saved token: {removed}"
        )),
        Err(lines) => {
            for l in lines {
                out::stderr_line(&l);
            }
            out::stderr_line(&format!(
                "{c} remote logout: {host} ended the session, but a saved copy of the token is still here"
            ));
            Exit::Error
        }
    }
}

fn out_line(line: &str) -> Exit {
    match out::stdout(&format!("{line}\n")) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words() {
        let c = cli_name!();
        assert_eq!(parse(&[]), Form::Usage(usage_line()));
        assert_eq!(parse(&["--host"]), Form::Usage(usage_line()));
        assert_eq!(parse(&["--host", ""]), Form::Usage(usage_line()));
        assert_eq!(parse(&["--host", "h", "x"]), Form::Usage(usage_line()));
        assert!(matches!(
            parse(&["--host", "h", "--token", "t"]),
            Form::Usage(l) if l.starts_with(&format!("{c} remote logout: no --token"))
        ));
        assert_eq!(
            parse(&["--host=box.example/", "--token-stdin"]),
            Form::Send {
                host: "box.example".into(),
                base: "https://box.example".into(),
                from_stdin: true
            }
        );
        assert!(matches!(
            parse(&["--host", "http://example.invalid"]),
            Form::Usage(l) if l.ends_with("plain http:// only to this computer; use https://")
        ));
        assert!(matches!(
            parse(&["--host", "http://127.0.0.1:9/"]),
            Form::Send { base, .. } if base == "http://127.0.0.1:9"
        ));
    }

    #[test]
    fn the_forget_keys() {
        let z = ZONE_SUFFIX;
        assert_eq!(
            forget_keys(&format!(" box{z}{z} ")),
            vec![format!("box{z}{z}"), "box".to_string(), format!("box{z}")]
        );
        assert_eq!(
            forget_keys("http://127.0.0.1:9"),
            vec![
                "http://127.0.0.1:9".to_string(),
                format!("http://127.0.0.1:9{z}")
            ]
        );
    }

    #[test]
    fn only_the_keys_holding_the_token_go() {
        let Value::Object(pairs) = json::parse(r#"{"a":"t","b":"u","c":"t"}"#).unwrap() else {
            panic!()
        };
        let keys = vec!["a".to_string(), "b".to_string()];
        let kept = without_token(&pairs, &keys, "t").unwrap();
        assert_eq!(
            json::to_string(&Value::Object(kept), &Options::default()).unwrap(),
            r#"{"b": "u", "c": "t"}"#
        );
        assert!(without_token(&pairs, &keys, "v").is_none());
    }

    #[test]
    fn the_body_cut() {
        let long = "é".repeat(400);
        assert_eq!(shown_body(long.as_bytes(), || true).len(), 600);
        assert_eq!(shown_body(long.as_bytes(), || false).len(), 300);
        assert_eq!(shown_body(b"a\nb\0", || true), b"a b".to_vec());
    }
}
