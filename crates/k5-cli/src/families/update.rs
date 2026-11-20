//! `k5 update`: what the shell CLI's `k2 update` prints
//! (`cmd_update_v2`, `cli/src/update-flags.sh`; `cmd_update` and
//! `cmd_app_update`, `cli/src/update.sh`). Nothing here downloads,
//! installs or replaces anything, and neither does the shell: `k2 update`
//! only reports versions and says how K2 updates.
//!
//! The words after `update` are read as `cmd_update_v2` reads them
//! ([`form`]): `--list` anywhere is the list at once; else the last of
//! `--app` and `--cli` picks the form (`--app` when neither);
//! `--check` and `-c` are the CLI form's; any other word is skipped. `-h`
//! and `--help` never get here: the front sends a command line holding
//! either to the shell CLI ([`crate::front`]).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]): `update` is not a verb the gates skip,
//!    and no tool of the shell's tool policy.
//! 2. The form:
//!    - [`Form::Cli`] (`update --cli [--check|-c]`): one
//!      `GET /boot-status` to `127.0.0.1` on the port, whatever the host
//!      setting says, with no credential, one try, connect 2 s, at most
//!      4 s ([`crate::transport::Client::get_update_version`]). Any
//!      answer is read, whatever its status; no answer is "didn't
//!      answer". The body is read as the shell's Python reads it
//!      ([`daemon_version`]). Then the lines ([`cli_lines`]) and exit 0.
//!    - [`Form::App`] (`update`, `update --app`): `Checking for app
//!      updates...`, then `curl -sH "Accept: …" <latest>` with its stderr
//!      dropped and its stdout read ([`latest_tag`]); then the lines
//!      ([`app_lines`]) and exit 0. A curl that fails ends the command
//!      with curl's code; an answer the shell's Python can't read (not
//!      JSON, not an object) ends it with 1, as `set -e` does.
//!    - [`Form::List`] (`update --list`): `Available K2 releases:`, then
//!      `curl -sH "Accept: …" <releases?per_page=10>` read by the shell's
//!      Python's loop ([`list_lines`]). Its lines, then the exit
//!      `pipefail` gives: 1 when the Python raised, else curl's code.
//!
//! curl is the shell's: the `curl` on `PATH`, run with the shell's
//! arguments (its `argv[0]` is `curl`), this process's environment and
//! folder, stdin as it is, stderr to nowhere. When there is no such
//! curl, or bash would run something else for it (an exported function,
//! a startup file), the command goes to the shell, before anything is
//! printed. So does every form when the binary can't tell what the
//! shell's `python3` is ([`crate::python`]).
//!
//! Without `python3` ([`crate::python::NATIVE_WITHOUT_PYTHON3`]) each form
//! prints what it prints with one; the shell there says the daemon
//! didn't answer (`--cli`), or exits 127 after its first line (`--app`,
//! `--list`).
//!
//! Where this can differ from the shell: a NUL in what curl or the daemon
//! sends is dropped here as bash drops it, but bash 4.4 and later also
//! print a warning of their own; an answer that is not UTF-8 is read as
//! the shell's Python reads it in a UTF-8 locale (a version the daemon
//! didn't say, an exception), where in the C locale Python passes the
//! bytes through. GitHub and the daemon send UTF-8 JSON with no NUL.

use super::curl_port;
use crate::daemon::{self, Settings};
use crate::embedded;
use crate::exit::Exit;
use crate::offline::{self, Env};
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude;
use crate::python;
use crate::transport::skew;
use k2_client::brand::{RELEASES_LATEST_API, RELEASES_URL_PREFIX};
use k2_client::{product_name, release_repo};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `k5 update --cli [--check]` is the binary's. Off, the shell answers
/// it. Each form has its own switch, so one form can go back to the
/// shell alone.
pub const CLI_NATIVE: bool = true;

/// `k5 update` and `k5 update --app` are the binary's. Off, the shell
/// answers them (and runs curl itself).
pub const APP_NATIVE: bool = true;

/// `k5 update --list` is the binary's. Off, the shell answers it (and
/// runs curl itself).
pub const LIST_NATIVE: bool = true;

/// What the daemon's version is read from.
pub const BOOT_STATUS: &str = "/boot-status";
/// The header both GitHub calls send.
pub const ACCEPT: &str = "Accept: application/vnd.github.v3+json";
/// GitHub's newest release.
pub const LATEST_URL: &str = RELEASES_LATEST_API;
/// GitHub's ten newest releases.
pub const LIST_URL: &str = concat!(
    "https://api.github.com/repos/",
    release_repo!(),
    "/releases?per_page=10"
);
/// The program both GitHub calls run.
pub const CURL: &str = "curl";

/// The form a command line asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// `--cli`: the CLI's and the daemon's versions. `check`: `--check`
    /// or `-c` (no closing line).
    Cli { check: bool },
    /// The newest release on GitHub.
    App,
    /// The ten newest releases on GitHub.
    List,
}

/// The words after `update`, as `cmd_update_v2` reads them.
pub fn form(words: &[&str]) -> Form {
    let (mut cli, mut check) = (false, false);
    for w in words {
        match *w {
            "--app" => cli = false,
            "--cli" => cli = true,
            "--list" => return Form::List,
            "--check" | "-c" => check = true,
            _ => {}
        }
    }
    if cli {
        Form::Cli { check }
    } else {
        Form::App
    }
}

/// Whether the binary answers `form` (its switch).
pub fn native(form: Form) -> bool {
    match form {
        Form::Cli { .. } => CLI_NATIVE,
        Form::App => APP_NATIVE,
        Form::List => LIST_NATIVE,
    }
}

/// Python's `str.isspace()` for one code point: Unicode's White_Space,
/// and the four separators U+001C to U+001F.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// What `body=$(…)` keeps of a body: NULs and trailing newlines dropped.
fn substituted(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = bytes.iter().copied().filter(|b| *b != 0).collect();
    while out.last() == Some(&b'\n') {
        out.pop();
    }
    out
}

/// What `$(…)` keeps of a Python line: NULs and trailing newlines dropped.
fn printed(text: &str) -> String {
    let mut out: String = text.chars().filter(|c| *c != '\0').collect();
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// `_update_daemon_version`'s reading of a `/boot-status` body, then
/// `${daemon_version#v}`: the version when the body is a JSON object
/// whose `version` is a string with something in it besides Python's
/// whitespace; `None` (the daemon "didn't answer") otherwise.
pub fn daemon_version(body: &[u8]) -> Option<String> {
    let body = substituted(body);
    let text = std::str::from_utf8(&body).ok()?;
    let text = if text.is_empty() { "null" } else { text };
    let Ok(Value::Object(pairs)) = json::parse(text) else {
        return None;
    };
    let Some(Value::Str(v)) = super::get(&pairs, "version") else {
        return None;
    };
    let stripped = v.as_str()?.trim_matches(py_space);
    if stripped.is_empty() {
        return None;
    }
    let shown = printed(stripped);
    let version = shown.strip_prefix('v').unwrap_or(&shown);
    (!version.is_empty()).then(|| version.to_string())
}

/// `cmd_update`'s lines for the CLI's version `ours` and the daemon's
/// `theirs` (`None`: it didn't answer); `check`: no closing line.
pub fn cli_lines(ours: &str, theirs: Option<&str>, check: bool) -> String {
    let product = product_name!();
    let daemon = match theirs {
        None => format!("{product} daemon: didn't answer, so its version is unknown."),
        Some(v) if v == ours => format!("{product} daemon: {v} (same as the CLI)"),
        Some(v) => format!("{product} daemon: {v} (differs from the CLI's {ours})"),
    };
    let mut s = format!(
        "k2 CLI {ours}\n{daemon}\n\n\
         The CLI updates together with {product}; k2 never downloads itself.\n  \
         Desktop: k2 update --app, or the app's own updater.\n  \
         Linux:   your {product} package, or the install script.\n  \
         The {product} daemon and root helper keep this CLI matched to {product}.\n"
    );
    if !check {
        s.push_str("\nNothing was changed.\n");
    }
    s
}

/// The App form's first line.
pub const CHECKING: &str = "Checking for app updates...";
/// The list's first line.
pub const LIST_HEAD: &str = concat!("Available ", product_name!(), " releases:");

/// The shell's Python on the newest release's answer (`latest`, curl's
/// stdout): the tag it prints, as `tag=$(…)` keeps it; `None` when the
/// Python raises (the command then ends with 1).
pub fn latest_tag(latest: &[u8]) -> Option<String> {
    let latest = substituted(latest);
    let text = std::str::from_utf8(&latest).ok()?;
    let Ok(Value::Object(pairs)) = json::parse(text) else {
        return None;
    };
    let tag = super::get(&pairs, "tag_name").map_or_else(|| "unknown".to_string(), py::str_of);
    Some(printed(&tag))
}

/// `cmd_app_update`'s lines after the first, for the CLI's version `ours`
/// and the tag.
pub fn app_lines(ours: &str, tag: &str) -> String {
    format!(
        "  Current: v{ours}\n  Latest:  {tag}\n\n\
         To update the app, download from:\n  {RELEASES_URL_PREFIX}tag/{tag}\n\n\
         Or use: k2 update --app --list  to see all versions\n"
    )
}

/// `s[:n]` of a Python string, written as [`py::str_of`] writes one.
fn first_code_points(t: &json::Text, n: usize) -> String {
    match t.as_str() {
        Some(s) => s.chars().take(n).collect(),
        None => t
            .code_points()
            .into_iter()
            .take(n)
            .map(|cp| char::from_u32(cp).map_or_else(|| format!("\\u{cp:04x}"), String::from))
            .collect(),
    }
}

/// The shell's Python loop on the releases' answer (`body`, curl's
/// stdout as it came): the lines it prints, and whether it ended without
/// raising.
pub fn list_lines(body: &[u8]) -> (String, bool) {
    let Some(data) = std::str::from_utf8(body)
        .ok()
        .and_then(|t| json::parse(t).ok())
    else {
        return (String::new(), false);
    };
    let items = match data {
        Value::Array(items) => items,
        // Iterating a dict gives its keys and a str its characters, and
        // `.get` on either raises; an empty one runs the loop no times.
        Value::Object(pairs) => return (String::new(), pairs.is_empty()),
        Value::Str(t) => return (String::new(), t.code_points().is_empty()),
        // Not iterable.
        _ => return (String::new(), false),
    };
    let mut out = String::new();
    for r in &items {
        let Value::Object(r) = r else {
            return (out, false);
        };
        let tag = super::get(r, "tag_name").map_or_else(|| "?".to_string(), py::str_of);
        let date = match super::get(r, "published_at") {
            None => String::new(),
            Some(Value::Str(t)) => first_code_points(t, 10),
            Some(Value::Array(v)) => {
                py::str_of(&Value::Array(v.iter().take(10).cloned().collect()))
            }
            // `None[:10]`, `5[:10]`, `{}[:10]` raise.
            Some(_) => return (out, false),
        };
        let pre = if super::get(r, "prerelease").is_some_and(py::truthy) {
            " (pre-release)"
        } else {
            ""
        };
        out.push_str(&format!("  {tag}  {date}{pre}\n"));
    }
    (out, true)
}

/// What curl gave: its stdout and its exit code (128 plus the signal
/// that ended it, as bash reports one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub stdout: Vec<u8>,
    pub code: i32,
}

/// Runs `curl` (`program`, the one on the shell's `PATH`) with `args` as
/// the shell does: `argv[0]` `curl`, this environment and folder, stdin
/// as it is, stdout read, stderr to nowhere. `None`: it could not be
/// started.
pub fn run_curl(program: &Path, args: &[&str]) -> Option<Ran> {
    let mut cmd = Command::new(program);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.arg0(CURL);
    }
    let out = cmd
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        out.status.signal()
    };
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    let code = out
        .status
        .code()
        .or_else(|| signal.map(|s| 128 + s))
        .unwrap_or(1);
    Some(Ran {
        stdout: out.stdout,
        code,
    })
}

/// The `curl` bash runs for the GitHub forms in `env`: the first
/// executable `curl` on `PATH`. `None` when there is none, or when bash
/// could run something else for it (a startup file or setting, an
/// exported function): the shell answers then.
pub fn shell_curl(env: &Env) -> Option<PathBuf> {
    if offline::STARTUP.iter().any(|n| env.get(n).is_some()) || env.any_named("BASH_FUNC_") {
        return None;
    }
    let path = env.get("PATH")?;
    daemon::find_on_path(CURL, path.to_os_string())
}

/// `k5 update ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let form = form(&words);
    if !native(form) || !python::family_native(python::here()) {
        return None;
    }
    let curl = match form {
        Form::Cli { .. } => None,
        Form::App | Form::List => Some(shell_curl(&Env::current())?),
    };
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if prelude::gate(&settings).is_none() {
        // A port curl won't take is the shell's: its URL is the port's
        // text as it is.
        settings.port_text.as_deref().and_then(curl_port)?;
    }
    Some(run_with(args, form, curl.as_deref(), &settings))
}

fn run_with(args: &[OsString], form: Form, curl: Option<&Path>, settings: &Settings) -> Exit {
    if let Some(gate) = prelude::gate(settings) {
        return gate.stop();
    }
    let ours = embedded::version();
    match (form, curl) {
        (Form::Cli { check }, _) => {
            let theirs = ask_daemon(args, settings);
            print(&cli_lines(ours, theirs.as_deref(), check), Exit::Ok)
        }
        (Form::App, Some(curl)) => app(ours, curl),
        (Form::List, Some(curl)) => list(curl),
        // `run` finds curl for both GitHub forms.
        (Form::App | Form::List, None) => Exit::Error,
    }
}

/// Writes `text` to stdout; `then`, or the exit a broken stdout gives.
fn print(text: &str, then: Exit) -> Exit {
    match out::stdout(text) {
        Ok(()) => then,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// The daemon's version, read from `/boot-status` on `127.0.0.1` at the
/// settings' port as `_update_daemon_version` reads it.
fn ask_daemon(args: &[OsString], settings: &Settings) -> Option<String> {
    // `run` hands a port curl won't take to the shell, and the gate
    // passed, so there is one.
    let port = settings.port_text.as_deref().and_then(curl_port)?;
    let local = Settings {
        host: "127.0.0.1".to_string(),
        port: Some(port),
        ..settings.clone()
    };
    let mut client = local.client(passport::current(args).would_refuse()).ok()?;
    // This answer's version is the line: no skew line of the transport's.
    skew::mark_shown();
    let r = client.get_update_version(BOOT_STATUS).ok()?;
    daemon_version(&r.body)
}

/// The App form after the gates.
fn app(ours: &str, curl: &Path) -> Exit {
    if let Err(e) = out::stdout_line(CHECKING) {
        return out::after_stdout_error(&e);
    }
    let Some(ran) = run_curl(curl, &["-sH", ACCEPT, LATEST_URL]) else {
        return Exit::from_curl(126);
    };
    if ran.code != 0 {
        return Exit::from_curl(ran.code);
    }
    match latest_tag(&ran.stdout) {
        Some(tag) => print(&app_lines(ours, &tag), Exit::Ok),
        None => Exit::Error,
    }
}

/// The list form after the gates.
fn list(curl: &Path) -> Exit {
    if let Err(e) = out::stdout_line(LIST_HEAD) {
        return out::after_stdout_error(&e);
    }
    let Some(ran) = run_curl(curl, &["-sH", ACCEPT, LIST_URL]) else {
        return Exit::from_curl(126);
    };
    let (lines, ok) = list_lines(&ran.stdout);
    let exit = if !ok {
        Exit::Error
    } else {
        Exit::from_curl(ran.code)
    };
    if lines.is_empty() {
        return exit;
    }
    print(&lines, exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> String {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src");
        let mut s = String::new();
        for f in ["update.sh", "update-flags.sh"] {
            let p = dir.join(f);
            s.push_str(
                &std::fs::read_to_string(&p)
                    .unwrap_or_else(|e| panic!("read {}: {e}", p.display())),
            );
        }
        s
    }

    #[test]
    fn the_forms_as_cmd_update_v2_reads_them() {
        for (words, want) in [
            (&[][..], Form::App),
            (&["--app"], Form::App),
            (&["--app", "--check"], Form::App),
            (&["--cli"], Form::Cli { check: false }),
            (&["--cli", "--check"], Form::Cli { check: true }),
            (&["-c", "--cli"], Form::Cli { check: true }),
            (&["--cli", "--app"], Form::App),
            (&["--app", "--cli"], Form::Cli { check: false }),
            (&["--cli", "--list"], Form::List),
            (&["--list", "--cli"], Form::List),
            (&["x", "--list"], Form::List),
            (&["--cli", "x", "--bogus"], Form::Cli { check: false }),
            (&["--CLI"], Form::App),
            (&["--check"], Form::App),
        ] {
            assert_eq!(form(words), want, "{words:?}");
        }
    }

    /// The flag loop and the words the binary reads are the shell's.
    #[test]
    fn the_flag_loop_is_the_shell_s() {
        let s = source();
        for line in [
            r#"            --app)  target="app";  shift ;;"#,
            r#"            --cli)  target="cli";  shift ;;"#,
            r#"            --list) cmd_app_update --list; return ;;"#,
            r#"            --check|-c) cli_args+=("$1"); shift ;;"#,
            r#"            *) shift ;;"#,
            r#"            --check|-c) check_only=1; shift ;;"#,
        ] {
            assert!(s.contains(line), "no `{line}` in update.sh/update-flags.sh");
        }
    }

    /// The URLs, the header and every line are the shell's.
    #[test]
    fn the_texts_are_the_shell_s() {
        let s = source();
        for want in [
            format!(r#"curl -sH "{ACCEPT}" \"#),
            format!(r#""{LIST_URL}" 2>/dev/null | \"#),
            format!(r#""{LATEST_URL}" 2>/dev/null)"#),
            r#""http://127.0.0.1:${PORT}/boot-status" 2>/dev/null) || return 0"#.to_string(),
            "_K2_CURL_TRIES=1 _k2_curl \"\" -s --connect-timeout 2 --max-time 4 \\".to_string(),
            format!(r#"echo "{CHECKING}""#),
            format!(r#"echo "{LIST_HEAD}""#),
        ] {
            assert!(s.contains(&want), "no `{want}` in update.sh");
        }
        let echoed = |text: &str| {
            for line in text.lines() {
                let want = if line.is_empty() {
                    r#"echo """#.to_string()
                } else {
                    format!(r#"echo "{line}""#)
                };
                assert!(s.contains(&want), "no `{want}` in update.sh");
            }
        };
        let (v, d) = ("${K2_CLI_VERSION}", "${daemon_version}");
        echoed(&cli_lines(v, None, false));
        assert!(
            cli_lines(d, Some(d), true).contains("K2 daemon: ${daemon_version} (same as the CLI)")
        );
        assert!(s.contains(r#"echo "K2 daemon: ${daemon_version} (same as the CLI)""#));
        assert!(cli_lines(v, Some(d), true)
            .contains("K2 daemon: ${daemon_version} (differs from the CLI's ${K2_CLI_VERSION})"));
        assert!(s.contains(
            r#"echo "K2 daemon: ${daemon_version} (differs from the CLI's ${K2_CLI_VERSION})""#
        ));
        echoed(&app_lines("$K2_CLI_VERSION", "$tag"));
        // The list's line, in the Python.
        assert!(s.contains("    print(f'  {tag}  {date}{pre}')"));
        assert!(s.contains("    date = r.get('published_at', '')[:10]"));
        assert!(s.contains("    tag = r.get('tag_name', '?')"));
        assert!(s.contains("    pre = ' (pre-release)' if r.get('prerelease') else ''"));
        assert!(s.contains("get('tag_name','unknown')"));
        assert!(s.contains(r#"v = b.get("version") if isinstance(b, dict) else None"#));
        assert!(s.contains(r#"    b = json.loads(sys.stdin.read() or "null")"#));
    }

    #[test]
    fn the_daemon_s_version_as_the_shell_reads_it() {
        for (body, want) in [
            (r#"{"phase":"ready","version":"0.46.14"}"#, Some("0.46.14")),
            (r#"{"version":"v0.46.14"}"#, Some("0.46.14")),
            (r#"{"version":"vv1"}"#, Some("v1")),
            (
                "{\"version\":\" \\u00a00.1\\u2003\\u001c\\n\"}\n\n",
                Some("0.1"),
            ),
            (r#"{"version":"0.1\n0.2"}"#, Some("0.1\n0.2")),
            (r#"{"version":"1","version":"2"}"#, Some("2")),
            (r#"{"version":"a\u0000b"}"#, Some("ab")),
            (r#"{"version":"v"}"#, None),
            (r#"{"version":"\u0000"}"#, None),
            (r#"{"version":"  "}"#, None),
            (r#"{"version":46}"#, None),
            (r#"{"version":null}"#, None),
            (r#"{"phase":"ready"}"#, None),
            (r#"["version"]"#, None),
            ("", None),
            ("\n\n", None),
            ("not json", None),
            ("\u{feff}{\"version\":\"1\"}", None),
        ] {
            assert_eq!(daemon_version(body.as_bytes()).as_deref(), want, "{body:?}");
        }
        assert_eq!(daemon_version(b"{\"version\":\"1\xff\"}"), None);
    }

    #[test]
    fn the_newest_tag_as_the_shell_s_python_prints_it() {
        for (body, want) in [
            (r#"{"tag_name":"v0.46.20"}"#, Some("v0.46.20")),
            (r#"{"message":"API rate limit exceeded"}"#, Some("unknown")),
            (r#"{"tag_name":null}"#, Some("None")),
            (r#"{"tag_name":true}"#, Some("True")),
            (r#"{"tag_name":46}"#, Some("46")),
            (r#"{"tag_name":1.5e3}"#, Some("1500.0")),
            (r#"{"tag_name":["a",1]}"#, Some("['a', 1]")),
            ("{\"tag_name\":\"v1\\n\\n\"}\n", Some("v1")),
            (r#"{"tag_name":"a\u0000b"}"#, Some("ab")),
            ("[1]", None),
            ("\"x\"", None),
            ("", None),
            ("<html>", None),
        ] {
            assert_eq!(latest_tag(body.as_bytes()).as_deref(), want, "{body:?}");
        }
    }

    #[test]
    fn the_list_as_the_shell_s_python_prints_it() {
        let (out, ok) = list_lines(
            br#"[{"tag_name":"v0.46.20","published_at":"2026-11-18T01:02:03Z","prerelease":false},
                {"tag_name":"v0.47.0-rc1","published_at":"2026-11-19T00:00:00Z","prerelease":true},
                {"published_at":["a",1,2,3,4,5,6,7,8,9,10,11]},
                {"tag_name":null,"prerelease":"yes"}]"#,
        );
        assert!(ok);
        assert_eq!(
            out,
            "  v0.46.20  2026-11-18\n  v0.47.0-rc1  2026-11-19 (pre-release)\n  ?  ['a', 1, 2, 3, 4, 5, 6, 7, 8, 9]\n  None   (pre-release)\n"
        );
        for (body, want, ok) in [
            ("{}", "", true),
            ("\"\"", "", true),
            ("[]", "", true),
            (r#"{"a":1}"#, "", false),
            ("\"x\"", "", false),
            ("5", "", false),
            ("null", "", false),
            ("not json", "", false),
            ("", "", false),
            (r#"[{"tag_name":"a"},5]"#, "  a  \n", false),
            (
                r#"[{"tag_name":"a"},{"published_at":null}]"#,
                "  a  \n",
                false,
            ),
            (r#"[{"published_at":{}}]"#, "", false),
        ] {
            assert_eq!(
                list_lines(body.as_bytes()),
                (want.to_string(), ok),
                "{body:?}"
            );
        }
    }

    #[test]
    fn python_s_whitespace() {
        for c in [
            '\t', '\n', '\u{b}', '\u{c}', '\r', ' ', '\u{1c}', '\u{1f}', '\u{85}', '\u{a0}',
            '\u{3000}',
        ] {
            assert!(py_space(c), "{c:?}");
        }
        for c in ['\0', '\u{1b}', 'a', '\u{200b}', '\u{feff}'] {
            assert!(!py_space(c), "{c:?}");
        }
    }

    #[test]
    fn the_switches_pick_the_forms() {
        assert_eq!(native(Form::Cli { check: true }), CLI_NATIVE);
        assert_eq!(native(Form::App), APP_NATIVE);
        assert_eq!(native(Form::List), LIST_NATIVE);
    }
}
