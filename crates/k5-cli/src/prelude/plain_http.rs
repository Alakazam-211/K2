//! The prelude's last step (`cli/src/cli-tools-policy.sh`, right after
//! `_k2_daemon_base BASE_URL`): a daemon on another computer over plain
//! http.
//!
//! `K2_HOST` set to a name or address that isn't this computer as written
//! sends the owner token across the network unencrypted. The shell allows
//! it, with one stderr line per run, printed after its two gates and
//! before the command does anything:
//!
//! ```text
//! warning: <name> is sending this server's owner token to <host> over plain http, unencrypted. That's fine on a private network such as a tailnet; elsewhere use https.
//! ```
//!
//! `<name>` is the command's own name (`${0##*/}` in the shell, the
//! brand's CLI name here). No line for a loopback literal, a Tailscale
//! literal (100.64.0.0/10, fd7a:115c:a1e0::/48, a name ending in
//! `.ts.net`, all as written, never looked up), `K2_HOST=https://<name>`,
//! or `K2_PLAIN_HTTP_OK=1`. A password never goes there: a command line
//! with `--password`, `--password-stdin` or `--password=...` is refused
//! instead, exit 2, the switch or not. help, study, --schema, --version
//! and `-h`/`--help` anywhere stay local: nothing at all.
//!
//! The binary runs the step once, just before the first thing it prints
//! ([`crate::out`], [`crate::prompt`]) or sends (every connection,
//! [`crate::transport`]), so a command it hands to the shell, having
//! printed and sent nothing, gets the shell's own line only. A gate that
//! stops the command ([`super::Gate::stop`]) ends the step unrun, as the
//! shell's gates exit before it.

use crate::exit::Exit;
use k2_client::env_names;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// The step ran, or a gate stopped the command first.
static DONE: AtomicBool = AtomicBool::new(false);

/// What the step does for one command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Nothing.
    Quiet,
    /// This warning line on stderr, then the command.
    Warn(String),
    /// This line on stderr, exit 2, nothing sent.
    Refuse(String),
}

/// Runs the step, once per run: the warning line, or the password
/// refusal (which ends the process with exit 2).
pub fn run() {
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let first = |names: &[&'static str]| -> Option<String> {
        env_names::first_nonempty_os(names).map(|(_, v)| v.to_string_lossy().into_owned())
    };
    let host = first(env_names::HOST).unwrap_or_else(|| crate::daemon::DEFAULT_HOST.to_string());
    let plain_ok = first(env_names::PLAIN_HTTP_OK).is_some_and(|v| v == "1");
    let step = step(&args, &host, plain_ok, || {
        first(env_names::PORT)
            .or_else(|| {
                let home = crate::home::k2_home().ok()?;
                let text = std::fs::read(home.join("heartbeat.port")).ok()?;
                let text = String::from_utf8_lossy(&text);
                Some(text.trim_end_matches('\n').to_string())
            })
            .unwrap_or_default()
    });
    let mut err = std::io::stderr().lock();
    match step {
        Step::Quiet => {}
        Step::Warn(line) => {
            let _ = err.write_all(format!("{line}\n").as_bytes());
            let _ = err.flush();
        }
        Step::Refuse(line) => {
            let _ = err.write_all(format!("{line}\n").as_bytes());
            let _ = err.flush();
            std::process::exit(Exit::Usage.code());
        }
    }
}

/// The step won't run this time: a gate stopped the command first.
pub fn skip() {
    DONE.store(true, Ordering::SeqCst);
}

/// The step for `args` (the command line after the command's name), with
/// the daemon at `host` (`HOST` as the shell reads it, `127.0.0.1` when
/// unset) and `plain_ok` (`PLAIN_HTTP_OK` is `1`). `port`: the shell's
/// `PORT`, asked only for the refusal's URL.
pub fn step(args: &[OsString], host: &str, plain_ok: bool, port: impl FnOnce() -> String) -> Step {
    if offline_verb(args) || !daemon_is_plain_remote(host) {
        return Step::Quiet;
    }
    if args.iter().any(|a| password_flag(a)) {
        return Step::Refuse(refusal_line(&format!("http://{host}:{}", port())));
    }
    if plain_ok || host_is_tailnet_literal(host) {
        return Step::Quiet;
    }
    Step::Warn(warning_line(host))
}

/// The warning, for `host` as written.
pub fn warning_line(host: &str) -> String {
    format!(
        "warning: {} is sending this server's owner token to {host} over plain http, unencrypted. That's fine on a private network such as a tailnet; elsewhere use https.",
        k2_client::cli_name!()
    )
}

/// The password refusal, for the daemon's URL (`BASE_URL`).
pub fn refusal_line(base_url: &str) -> String {
    format!("Error: a password never goes over plain http:// to another computer; use K2_HOST=https://<name> (got: {base_url})")
}

/// `_k2_offline_verb`: the first word is none, empty, `study`, `help`,
/// `--help`, `-h`, `--schema`, `version` or `--version`, or a word
/// anywhere is `-h` or `--help` (after a `--` too, as the shell's loop
/// reads every word).
pub fn offline_verb(args: &[OsString]) -> bool {
    let first = args.first().map_or(OsStr::new(""), OsString::as_os_str);
    let local = [
        "",
        "study",
        "help",
        "--help",
        "-h",
        "--schema",
        "version",
        "--version",
    ];
    local.iter().any(|w| first == *w) || args.iter().any(|a| a == "-h" || a == "--help")
}

/// `--password`, `--password-stdin` or `--password=...`.
fn password_flag(a: &OsStr) -> bool {
    a == "--password" || a == "--password-stdin" || a.as_encoded_bytes().starts_with(b"--password=")
}

/// `_k2_daemon_is_plain_remote`: the daemon at `host` is reached over
/// plain http (`host` doesn't start `https://` or `HTTPS://`) and `host`,
/// as written, isn't this computer ([`url_is_literal_loopback`]; a bare
/// IPv6 address is read bracketed, so `::1` counts as `[::1]`).
pub fn daemon_is_plain_remote(host: &str) -> bool {
    if host.starts_with("https://") || host.starts_with("HTTPS://") {
        return false;
    }
    let h = if !host.starts_with('[') && host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    !url_is_literal_loopback(&format!("http://{h}"))
}

/// `_k2_url_is_literal_loopback`: the URL's host, as written, is an IPv4
/// address in 127.0.0.0/8 written as four plain numbers, `[::1]`, or the
/// name `localhost` in any case. The shell's text rule, step by step.
pub fn url_is_literal_loopback(url: &str) -> bool {
    let mut h = url.split_once("://").map_or(url, |(_, rest)| rest);
    for stop in ['/', '?', '#'] {
        h = h.split(stop).next().unwrap_or("");
    }
    h = h.rsplit('@').next().unwrap_or("");
    if h == "[::1]" || h.starts_with("[::1]:") {
        return true;
    }
    let h = h.split(':').next().unwrap_or("").to_ascii_lowercase();
    if h == "localhost" {
        return true;
    }
    let parts: Vec<&str> = h.split('.').collect();
    parts.len() == 4 && parts[0] == "127" && parts[1..].iter().all(|p| octet(p).is_some())
}

/// `_k2_host_is_tailnet_literal`: `host`, lower-cased, one leading `[` and
/// one trailing `]` taken off, starts `fd7a:115c:a1e0:`, or ends `.ts.net`
/// with at least one character before it, or is an IPv4 address in
/// 100.64.0.0/10 written as four plain numbers.
pub fn host_is_tailnet_literal(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    let h = h.strip_prefix('[').unwrap_or(&h);
    let h = h.strip_suffix(']').unwrap_or(h);
    if h.starts_with("fd7a:115c:a1e0:") || (h.len() > ".ts.net".len() && h.ends_with(".ts.net")) {
        return true;
    }
    let parts: Vec<&str> = h.split('.').collect();
    parts.len() == 4
        && parts[0] == "100"
        && octet(parts[1]).is_some_and(|n| (64..=127).contains(&n))
        && parts[2..].iter().all(|p| octet(p).is_some())
}

/// One number of the shell's address patterns
/// (`25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9]`): 0 to 255, digits
/// only, no leading zero.
fn octet(p: &str) -> Option<u16> {
    let digits = (1..=3).contains(&p.len())
        && p.bytes().all(|b| b.is_ascii_digit())
        && (p.len() == 1 || !p.starts_with('0'));
    if !digits {
        return None;
    }
    p.parse::<u16>().ok().filter(|n| *n <= 255)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(w: &[&str]) -> Vec<OsString> {
        w.iter().map(OsString::from).collect()
    }

    fn policy() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/src/cli-tools-policy.sh");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// The two lines are the shell's, the command's name aside.
    #[test]
    fn the_lines_are_the_shell_s() {
        let p = policy();
        let warn = warning_line("${HOST}").replacen(k2_client::cli_name!(), "${0##*/}", 1);
        assert!(p.contains(&format!("echo \"{warn}\" >&2")), "{warn}");
        let refuse = refusal_line("${BASE_URL}");
        assert!(p.contains(&format!("echo \"{refuse}\" >&2")), "{refuse}");
        assert!(p.contains(
            "    _envn_into _k2_plain_ok PLAIN_HTTP_OK\n    if [ \"$_k2_plain_ok\" != 1 ] && ! _k2_host_is_tailnet_literal \"$HOST\"; then\n"
        ));
        assert!(p.contains("            --password|--password-stdin|--password=*)\n"));
        assert!(p.contains(
            "case \"${1:-}\" in \"\"|study|help|--help|-h|--schema|version|--version) _k2_offline_verb=1 ;; esac\nfor _a in \"$@\"; do case \"$_a\" in -h|--help) _k2_offline_verb=1 ;; esac; done\n"
        ));
    }

    #[test]
    fn the_step_for_each_command_line() {
        let port = || "4711".to_string();
        let name = k2_client::cli_name!();
        let warn = |h: &str| {
            Step::Warn(format!("warning: {name} is sending this server's owner token to {h} over plain http, unencrypted. That's fine on a private network such as a tailnet; elsewhere use https."))
        };
        let refuse = |u: &str| {
            Step::Refuse(format!("Error: a password never goes over plain http:// to another computer; use K2_HOST=https://<name> (got: {u})"))
        };
        let w = words(&["whoami"]);
        assert_eq!(step(&w, "10.0.0.5", false, port), warn("10.0.0.5"));
        assert_eq!(step(&w, "box.example", false, port), warn("box.example"));
        assert_eq!(step(&w, "0.0.0.0", false, port), warn("0.0.0.0"));
        assert_eq!(step(&w, "10.0.0.5", true, port), Step::Quiet);
        for quiet in [
            "127.0.0.1",
            "127.9.8.7",
            "localhost",
            "LocalHost",
            "::1",
            "[::1]",
            "https://box.example",
            "HTTPS://box.example",
            "100.64.0.1",
            "box.tail1234.ts.net",
            "fd7a:115c:a1e0::5",
        ] {
            assert_eq!(step(&w, quiet, false, port), Step::Quiet, "{quiet}");
        }
        // As written, never looked up.
        for loud in [
            "x.localhost",
            "127.1",
            "127.0.0.01",
            "0:0:0:0:0:0:0:1",
            "Https://box.example",
            "box.ts.net.evil.example",
            "100.128.0.1",
            "192.0.2.10",
        ] {
            assert_eq!(step(&w, loud, false, port), warn(loud), "{loud}");
        }
        // Help and the pages stay local.
        let locals: [&[&str]; 11] = [
            &[],
            &[""],
            &["study", "whoami"],
            &["help"],
            &["--help"],
            &["-h"],
            &["--schema"],
            &["version"],
            &["--version", "--check"],
            &["users", "add", "bob", "--password-stdin", "-h"],
            &["whoami", "--", "--help"],
        ];
        for local in locals {
            assert_eq!(
                step(&words(local), "10.0.0.5", false, port),
                Step::Quiet,
                "{local:?}"
            );
        }
        // A password, the switch or not; never asked for this computer.
        for pw in ["--password", "--password-stdin", "--password=x"] {
            let w = words(&["users", "add", "bob", pw]);
            assert_eq!(
                step(&w, "10.0.0.5", true, port),
                refuse("http://10.0.0.5:4711")
            );
            assert_eq!(
                step(&w, "100.64.0.1", false, port),
                refuse("http://100.64.0.1:4711")
            );
            assert_eq!(step(&w, "127.0.0.1", false, || unreachable!()), Step::Quiet);
            assert_eq!(
                step(&w, "https://box", false, || unreachable!()),
                Step::Quiet
            );
        }
        let w = words(&["users", "add", "bob", "--passwords"]);
        assert_eq!(step(&w, "10.0.0.5", false, port), warn("10.0.0.5"));
    }

    /// The warning names the command by the brand: "xy" in the
    /// placeholder build, never a typed k2 or k5.
    #[test]
    fn the_lines_say_the_brand_s_name() {
        let cli = k2_client::brand::CLI_NAME;
        let line = warning_line("10.0.0.5");
        assert!(line.starts_with(&format!("warning: {cli} is sending ")), "{line}");
        if cli != "k2" {
            for l in [line, refusal_line("http://10.0.0.5:4711")] {
                assert!(!l.contains("k2 ") && !l.contains("k5 "), "{l}");
            }
        }
    }

    #[test]
    fn tailnet_literals_as_the_shell_writes_them() {
        for (host, want) in [
            ("100.64.0.1", true),
            ("100.127.255.254", true),
            ("100.63.255.255", false),
            ("100.128.0.1", false),
            ("100.064.0.1", false),
            ("100.64.0.256", false),
            ("fd7a:115c:a1e0::1", true),
            ("FD7A:115C:A1E0::1", true),
            ("[fd7a:115c:a1e0:ab12::5]", true),
            ("fd7a:115c:a1e0", false),
            ("fd7a:115c:a1e1::1", false),
            ("box.tail1234.ts.net", true),
            ("BOX.TS.NET", true),
            ("x.ts.net", true),
            (".ts.net", false),
            ("ts.net", false),
            ("box.ts.net.evil.example", false),
            ("10.0.0.5", false),
            ("box.example", false),
        ] {
            assert_eq!(host_is_tailnet_literal(host), want, "{host}");
        }
    }

    #[test]
    fn loopback_as_the_shell_writes_it() {
        for (url, want) in [
            ("http://127.0.0.1", true),
            ("http://127.0.0.1:9/x", true),
            ("http://user@127.255.255.255:9", true),
            ("http://[::1]", true),
            ("http://[::1]:9", true),
            ("http://LOCALHOST:9", true),
            ("http://localhost.", false),
            ("http://127.0.0.256", false),
            ("http://127.00.0.1", false),
            ("http://[0:0:0:0:0:0:0:1]", false),
            ("http://127.example.invalid", false),
            ("127.0.0.1", true),
        ] {
            assert_eq!(url_is_literal_loopback(url), want, "{url}");
        }
    }
}
