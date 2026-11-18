//! Whether permission cards are on for this caller, as the shell CLI asks
//! before it prints a page that depends on them (`_permission_cli_enabled`
//! and `_permission_probe`, `cli/src/permission.sh`): the study list and
//! usage, the apps page (and its `skins` alias) and the cards-only
//! `permissions` topic.
//!
//! The shell's rule, for a page:
//!
//! - No passport (`HOOK_TOKEN`, after the prelude took a re-minted one
//!   from `SECRETS_DIR`): off, and nothing is asked.
//! - A passport and `HOOK_SOCK` a socket (`[ -S ]`): one `GET
//!   /cli/permission/enabled` on the cell socket, the passport as Bearer.
//! - Else, a port (`PORT`, else the K2 home's `heartbeat.port`; the
//!   prelude's [`Prelude::port`]): the same GET over TCP with `token=` in
//!   the query.
//! - Else off, nothing asked.
//!
//! Connect 1 s, at most 1 s, and only `_k2_curl`'s tries: a refused
//! connect again up to four times a second apart (over TCP, to a port
//! the daemon moved to). On only for a 200 whose body the shell's Python
//! reads as a JSON object with `"enabled"` exactly `true`, on the first
//! try: curl's `-w '\n%{http_code}'` writes `\n000` for each refused try
//! into the same `$(…)`, so an answer after one reads as `\n000<body>`,
//! which is not JSON (a shell bug, kept here for parity); anything else
//! is off: no answer in time (curl 28), a close with nothing (52), a
//! reset (56), an empty or non-JSON body, a status other than 200, a
//! refused connect after the tries (7). The shell prints nothing for any
//! of them, and neither does this. One request per command: the shell
//! keeps the answer for the rest of the run.
//!
//! [`ask`] decides before anything is sent. It gives `None`, so the page
//! goes to the shell, where the binary can't be sure to send the very
//! same request or read the answer as the shell's Python would: a
//! `PYTHON*` setting, a locale that is not UTF-8 nor C/POSIX, a port that
//! is not a plain port number, a host that is not an IPv4 address, or a
//! passport, host or port that is not plain ASCII.
//!
//! Where this differs from the shell, on purpose or as a known gap: the
//! binary's own request headers ([`crate::transport`]: its User-Agent,
//! `X-K5-Cli-Version` and `X-K5-Cli-Via: native`), the one line when the
//! daemon's version is not the binary's ([`crate::transport::skew`]), and
//! a body with a NUL in it: the shell drops the NUL (bash 4.4 and later
//! also print a warning on stderr), and so does this, without the warning.

use crate::daemon::{self, Settings};
use crate::offline::{self, Env, Prelude};
use crate::out::json::{self, Value};
use crate::transport::{Auth, Client};
use std::borrow::Cow;
use std::net::Ipv4Addr;
use std::path::Path;

/// How the shell's Python reads its stdin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decode {
    /// A UTF-8 locale: bytes that are not UTF-8 are an error.
    Strict,
    /// C, POSIX or no locale (Python's UTF-8 mode), or one of the names
    /// Python's locale coercion sets (`C.UTF-8`, `C.utf8`, `UTF-8`): such
    /// bytes are kept as stand-ins (`surrogateescape`), which JSON reads
    /// like any other character.
    Lossy,
}

/// What the probe will do, decided before anything is sent.
pub enum Ask {
    /// Off, and nothing is asked: no passport, or neither a socket nor a
    /// port.
    Off,
    /// One request on this client (its channel already chosen by
    /// [`Client::has_cell_socket`]), read as `decode` says; `python`:
    /// the shell's `python3` is on `PATH` (without it, the answer is off).
    Request {
        client: Box<Client>,
        decode: Decode,
        python: bool,
    },
}

/// How `python3` reads stdin in `env`, or `None` when the binary can't
/// tell: a `PYTHON*` setting, or a locale that is neither UTF-8 nor C or
/// POSIX. (A locale this machine doesn't have reads as C to Python; that
/// changes only a body that is not UTF-8.)
pub fn decode(env: &Env) -> Option<Decode> {
    if env.any_named("PYTHON") {
        return None;
    }
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|n| env.get(n).filter(|v| !v.is_empty()))
        .map(|v| v.to_string_lossy().into_owned());
    let Some(name) = locale else {
        return Some(Decode::Lossy);
    };
    let lower = name.to_ascii_lowercase();
    let coerced = ["C.UTF-8", "C.utf8", "UTF-8"].contains(&name.as_str());
    if lower == "c" || lower == "posix" || coerced {
        Some(Decode::Lossy)
    } else if lower.contains("utf-8") || lower.contains("utf8") {
        Some(Decode::Strict)
    } else {
        None
    }
}

/// A port as the shell's URL gives it to curl, when it is plainly one:
/// digits, no leading zero, 1 to 65535.
pub fn plain_port(text: &str) -> Option<u16> {
    if text.is_empty() || text.starts_with('0') || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|p| *p != 0)
}

/// Printable ASCII with no space: what goes into a header or a query
/// the same way from curl's config and from here.
pub fn plain(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// The shell's settings in `env`: its K2 home (`$HOME/.k2`, else the old
/// name), never the binary's home override.
pub fn shell_settings(env: &Env, cwd: &Path) -> Option<Settings> {
    let home = offline::shell_k2_home(env)?;
    Some(daemon::settings(
        |n| env.get(n).map(|v| v.to_os_string()),
        &home,
        cwd,
    ))
}

/// What the shell's probe does in `env`, or `None`: the page goes to the
/// shell. `p` says whether the shell has a port at all; `would_refuse`:
/// the passport rule's count, on the request.
pub fn ask(env: &Env, p: &Prelude, would_refuse: impl FnOnce() -> bool) -> Option<Ask> {
    let settings = shell_settings(env, Path::new("/"))?;
    let Auth::Passport { passport, sock } = &settings.auth else {
        return Some(Ask::Off);
    };
    if !plain(passport.as_str()) {
        return None;
    }
    let socket = sock
        .as_deref()
        .is_some_and(crate::transport::client::is_socket);
    let port = if socket {
        0
    } else if !p.port {
        return Some(Ask::Off);
    } else {
        // The shell's port, as its URL gives it to curl.
        let port = plain_port(settings.port_text.as_deref()?)?;
        if settings.host.parse::<Ipv4Addr>().is_err() {
            return None;
        }
        port
    };
    let decode = decode(env)?;
    let client = settings.client_on(port, would_refuse());
    // The re-minted passport the client took must be as plain.
    if !client.passport().is_some_and(|t| plain(t.as_str())) {
        return None;
    }
    // The socket the client checks is the one checked here.
    if client.has_cell_socket() != socket {
        return None;
    }
    Some(Ask::Request {
        client: Box::new(client),
        decode,
        python: offline::on_path(env, "python3"),
    })
}

/// Sends the request, if there is one, and reads the answer.
pub fn on(ask: Ask) -> bool {
    match ask {
        Ask::Off => false,
        Ask::Request {
            mut client,
            decode,
            python,
        } => match client.permission_enabled() {
            // A refused try then an answer: `-w` printed `\n000` for the
            // failed try into `$(…)` first, so the shell's Python reads
            // `\n000<body>`, which is not JSON: off.
            Ok(r) => {
                r.status == 200
                    && client.refused_before_answer() == 0
                    && python
                    && enabled(&r.body, decode)
            }
            Err(_) => false,
        },
    }
}

/// The shell's Python on the body `$(…)` held (NULs dropped):
/// `json.load(sys.stdin)`, then on only for a `dict` whose `"enabled"`
/// is `True` (the last value of a key that comes twice).
pub fn enabled(body: &[u8], decode: Decode) -> bool {
    let bytes: Vec<u8> = body.iter().copied().filter(|b| *b != 0).collect();
    let text: Cow<'_, str> = match std::str::from_utf8(&bytes) {
        Ok(t) => t.into(),
        Err(_) if decode == Decode::Lossy => String::from_utf8_lossy(&bytes),
        Err(_) => return false,
    };
    match json::parse(&text) {
        Ok(Value::Object(pairs)) => pairs
            .iter()
            .any(|(k, v)| k.as_str() == Some("enabled") && *v == Value::Bool(true)),
        _ => false,
    }
}

/// `_perm_lines`: a line ending ` #if-cards` only when `on` (without the
/// tag), a line ending ` #if-no-cards` only when off (likewise), every
/// other line as it is; each line printed with a newline after it.
pub fn perm_lines(text: &str, on: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    for line in lines {
        let kept = if let Some(l) = line.strip_suffix(" #if-cards") {
            on.then_some(l)
        } else if let Some(l) = line.strip_suffix(" #if-no-cards") {
            (!on).then_some(l)
        } else {
            Some(line)
        };
        if let Some(l) = kept {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_locale_says_how_python_reads() {
        assert_eq!(decode(&Env::of(&[])), Some(Decode::Lossy));
        assert_eq!(decode(&Env::of(&[("LC_ALL", "C")])), Some(Decode::Lossy));
        assert_eq!(decode(&Env::of(&[("LANG", "POSIX")])), Some(Decode::Lossy));
        assert_eq!(
            decode(&Env::of(&[("LC_ALL", ""), ("LANG", "en_US.UTF-8")])),
            Some(Decode::Strict)
        );
        for coerced in ["C.UTF-8", "C.utf8", "UTF-8"] {
            assert_eq!(
                decode(&Env::of(&[("LC_ALL", coerced)])),
                Some(Decode::Lossy),
                "{coerced}"
            );
        }
        assert_eq!(
            decode(&Env::of(&[("LC_ALL", "c.utf-8")])),
            Some(Decode::Strict)
        );
        assert_eq!(decode(&Env::of(&[("LANG", "de_DE.ISO8859-1")])), None);
        assert_eq!(decode(&Env::of(&[("PYTHONUTF8", "1")])), None);
    }

    #[test]
    fn a_plain_port() {
        assert_eq!(plain_port("4100"), Some(4100));
        assert_eq!(plain_port("65535"), Some(65535));
        for no in ["", "0", "04100", "65536", " 4100", "41x", "-1"] {
            assert_eq!(plain_port(no), None, "{no:?}");
        }
    }

    #[test]
    fn on_only_for_enabled_true() {
        let on = |b: &str| enabled(b.as_bytes(), Decode::Strict);
        assert!(on(r#"{"enabled":true}"#));
        assert!(on(" {\"enabled\": true, \"x\": [1, NaN]}\n"));
        assert!(on(r#"{"enabled":false,"enabled":true}"#));
        for off in [
            "",
            "{}",
            r#"{"enabled":false}"#,
            r#"{"enabled":"true"}"#,
            r#"{"enabled":1}"#,
            r#"{"enabled":true,"enabled":null}"#,
            r#"[{"enabled":true}]"#,
            r#"{"Enabled":true}"#,
            r#"{"enabled":true}x"#,
            "\u{feff}{\"enabled\":true}",
        ] {
            assert!(!on(off), "{off:?}");
        }
        assert!(enabled(b"{\"enabled\":\0true}", Decode::Strict));
        let bad = b"{\"enabled\":true,\"x\":\"\xff\"}";
        assert!(!enabled(bad, Decode::Strict));
        assert!(enabled(bad, Decode::Lossy));
        assert!(!enabled(b"{\"enabled\":true}\xff", Decode::Lossy));
    }

    #[test]
    fn perm_lines_both_ways() {
        let t = "a\nb #if-cards\nc #if-no-cards\nd";
        assert_eq!(perm_lines(t, false), "a\nc\nd\n");
        assert_eq!(perm_lines(t, true), "a\nb\nd\n");
        assert_eq!(perm_lines("", true), "");
        assert_eq!(perm_lines("\n", false), "\n");
        assert_eq!(
            perm_lines("x #if-no-cards #if-cards\n", true),
            "x #if-no-cards\n"
        );
        assert_eq!(perm_lines("x #if-no-cards #if-cards\n", false), "");
    }
}
