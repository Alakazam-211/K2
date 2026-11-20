//! `k5 git-tools`: git on a first run (Apple's command line tools, or a
//! Linux distribution's git), as the shell CLI's `cmd_git_tools` answers
//! it (`cli/src/git-tools.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]). `git-tools` is an open tool.
//! 2. The verb, `${1:-status}` (`--json` first is `status --json`);
//!    `help` is the help page on stdout, exit 0. A word after it other than
//!    `--json` is its line, exit 2; then a verb other than `status`,
//!    `install` or `recheck`, likewise.
//! 3. One `_k2_curl` of its own ([`Client::own_curl`]): `status` a GET of
//!    `/cli/git-tools`, `install` and `recheck` a POST of `{}`; with
//!    `?format=text` unless `--json`; `TOKEN` as Bearer (the owner token,
//!    or in a session the passport, the named difference
//!    `owner-token-with-passport`); at most 120 s.
//! 4. What came back ([`render`]): no status (nothing listening, no
//!    answer) is "daemon unreachable", exit 1; `--json` prints the body and
//!    exits 1 unless it was a 200; a 200 prints the body as it came; else
//!    the daemon's own `k2 git-tools:` text, or the error (and an
//!    `owner_only` hint, exit 3), exit 1.
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! Still the shell's: a word that is not UTF-8, a command line with `-h`
//! or `--help` (the front sends those on), a port that is set but is no
//! port curl takes, and a `python3` the binary can't tell.

use super::{without_nuls, Shown};
use crate::daemon;
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::{Auth, OwnCurl};
use k2_client::cli_name;
use std::ffi::OsString;
use std::time::Duration;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["git-tools"];
/// The binary's, the whole row.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// None: the row is native whole.
pub const VERBS: &[crate::route::Verb] = &[];

pub const STATUS: &str = "/cli/git-tools";
pub const INSTALL: &str = "/cli/git-tools/install";
pub const RECHECK: &str = "/cli/git-tools/recheck";
/// `--max-time`.
pub const MAX_TIME: Duration = Duration::from_secs(120);

pub const UNREACHABLE: &str = concat!(cli_name!(), " git-tools: daemon unreachable");
/// The daemon's text answers start with this (`git_tools_routes.rs`
/// writes it; in the default brand it is the command's name).
pub const DAEMON_TEXT: &str = concat!(cli_name!(), " git-tools: ");

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    Help,
    /// The route, a POST, `--json`.
    Call(&'static str, bool, bool),
    Usage(String),
}

/// The words after `git-tools`.
pub fn parse(words: &[&str]) -> Form {
    let mut sub = words.first().copied().unwrap_or("status");
    let mut rest: Vec<&str> = words.get(1..).unwrap_or_default().to_vec();
    match sub {
        "help" | "--help" | "-h" => return Form::Help,
        "--json" => {
            sub = "status";
            rest.insert(0, "--json");
        }
        _ => {}
    }
    let mut json = false;
    for w in rest {
        match w {
            "--json" => json = true,
            "-h" | "--help" => return Form::Help,
            other => {
                return Form::Usage(format!(
                    "usage: unknown argument '{other}' ({} git-tools --help)",
                    cli_name!()
                ))
            }
        }
    }
    match sub {
        "status" => Form::Call(STATUS, false, json),
        "install" => Form::Call(INSTALL, true, json),
        "recheck" => Form::Call(RECHECK, true, json),
        other => Form::Usage(format!(
            "usage: unknown git-tools verb '{other}' (status|install|recheck)"
        )),
    }
}

/// `k5 git-tools ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
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
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    let port = settings.port_text.as_deref().and_then(super::curl_port)?;
    let (route, post, json) = match parse(&words) {
        Form::Help => {
            let page = crate::text::help("git-tools").unwrap_or_default();
            return Some(match out::stdout(page) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            });
        }
        Form::Usage(line) => {
            out::stderr_line(&line);
            return Some(Exit::Usage);
        }
        Form::Call(r, p, j) => (r, p, j),
    };
    let would_refuse = passport::current(args).would_refuse();
    let mut client = settings.client_on(port, would_refuse);
    let token = match &client.auth {
        Auth::Owner(t) => t.clone(),
        Auth::Passport { passport, .. } => passport.as_str().to_string(),
        Auth::None => String::new(),
    };
    let endpoint = if json {
        route.to_string()
    } else {
        format!("{route}?format=text")
    };
    let how = OwnCurl {
        post,
        credential: Some(&token),
        max_time: MAX_TIME,
        note: true,
    };
    let body: &[u8] = if post { b"{}" } else { b"" };
    let mut closing = Closing::default();
    let answer = match client.own_curl(&endpoint, how, body) {
        Ok(r) => {
            closing.answer(&r, client.passport().is_some());
            Some((r.status, without_nuls(&r.body)))
        }
        Err(_) => None,
    };
    let exit = render(json, answer).print();
    closing.flush(&mut client, exit);
    Some(exit)
}

/// `_git_tools_render`: `answer` is the status and the body, `None` when
/// curl wrote no status (`000`).
pub fn render(json: bool, answer: Option<(u16, Vec<u8>)>) -> Shown {
    let Some((status, body)) = answer.filter(|(s, _)| *s != 0) else {
        return Shown::err(format!("{UNREACHABLE}\n"), Exit::Error);
    };
    let body = String::from_utf8_lossy(&body).into_owned();
    if json {
        let mut s = Shown::out(format!("{body}\n"));
        if status != 200 {
            s.exit = Exit::Error;
        }
        return s;
    }
    if status == 200 {
        return Shown::out(body);
    }
    if body.starts_with(DAEMON_TEXT) {
        return Shown::err(body, Exit::Error);
    }
    let err = quoted_after(&body, "error").unwrap_or_else(|| format!("HTTP {status}"));
    let mut lines = format!("{} git-tools: error: {err}\n", cli_name!());
    if status == 403 && err == "owner_only" {
        if let Some(hint) = quoted_after(&body, "hint") {
            lines.push_str(&format!("  {hint}\n"));
        }
        return Shown::err(lines, Exit::Refused);
    }
    Shown::err(lines, Exit::Error)
}

/// Bash's `[[ $body =~ \"<key>\"[[:space:]]*:[[:space:]]*\"([^\"]*)\" ]]`:
/// the leftmost match's group.
pub fn quoted_after(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r');
    let mut from = 0;
    while let Some(at) = body[from..].find(&needle) {
        let start = from + at;
        let rest = body[start + needle.len()..].trim_start_matches(space);
        if let Some(rest) = rest.strip_prefix(':') {
            let rest = rest.trim_start_matches(space);
            if let Some(rest) = rest.strip_prefix('"') {
                if let Some(end) = rest.find('"') {
                    return Some(rest[..end].to_string());
                }
            }
        }
        from = start + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words() {
        assert_eq!(parse(&[]), Form::Call(STATUS, false, false));
        assert_eq!(parse(&["--json"]), Form::Call(STATUS, false, true));
        assert_eq!(parse(&["install", "--json"]), Form::Call(INSTALL, true, true));
        assert_eq!(parse(&["help"]), Form::Help);
        assert_eq!(
            parse(&["nope", "x"]),
            Form::Usage("usage: unknown argument 'x' (k2 git-tools --help)".into())
        );
        assert_eq!(
            parse(&["nope"]),
            Form::Usage("usage: unknown git-tools verb 'nope' (status|install|recheck)".into())
        );
    }

    #[test]
    fn what_came_back() {
        let r = |j, s, b: &str| {
            let x = render(j, Some((s, b.as_bytes().to_vec())));
            (x.stdout, x.stderr, x.exit)
        };
        assert_eq!(render(false, None).stderr, "k2 git-tools: daemon unreachable\n");
        assert_eq!(r(false, 200, "git: ok\n"), ("git: ok\n".into(), String::new(), Exit::Ok));
        assert_eq!(r(true, 500, "{}"), ("{}\n".into(), String::new(), Exit::Error));
        assert_eq!(
            r(false, 403, r#"{"error" : "owner_only", "hint":"Ask the owner"}"#),
            (
                String::new(),
                "k2 git-tools: error: owner_only\n  Ask the owner\n".into(),
                Exit::Refused
            )
        );
        assert_eq!(r(false, 500, "x").1, "k2 git-tools: error: HTTP 500\n");
        assert_eq!(r(false, 409, "k2 git-tools: busy\n").1, "k2 git-tools: busy\n");
        assert_eq!(quoted_after(r#""error":1, "error": "b""#, "error"), Some("b".into()));
    }

    #[test]
    fn the_lines_are_the_shell_s() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/git-tools.sh");
        let text = std::fs::read_to_string(p).unwrap();
        assert!(text.contains(UNREACHABLE));
        assert!(text.contains(&format!("\"{DAEMON_TEXT}\"*)")));
    }
}
