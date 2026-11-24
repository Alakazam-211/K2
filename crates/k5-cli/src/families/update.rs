//! `k5 update`: what the shell CLI's `k2 update` prints
//! (`cmd_update_v2`, `cli/src/update-flags.sh`; `cmd_update`,
//! `cmd_app_update` and `_app_update_get`, `cli/src/update.sh`). Nothing
//! here downloads, installs or replaces anything, and neither does the
//! shell: `k2 update` only reports versions and says how K2 updates.
//!
//! The words after `update` are read as `_k2_update_form`
//! (`cli/src/cli-tools-policy.sh`) reads them ([`form`]): help when `-h`
//! or `--help` is anywhere; else the list when `--list` is anywhere; else
//! the last of `--app` and `--cli` (`--app` when neither). `--check` and
//! `-c` are the CLI form's; any other word is skipped. Help is the
//! shell's: the front sends a command line with `-h` or `--help` in it to
//! the shell CLI ([`crate::front`]), which prints `_update_usage`.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]) and the gates ([`crate::prelude`]):
//!    the virtual desktop's guard for every form; the connection gate
//!    for `--cli` only. `update`, `--app` and `--list` skip it, as the
//!    shell does (they need no daemon). No form is a tool of the shell's
//!    tool policy.
//! 2. The form:
//!    - [`Form::Cli`] (`update --cli [--check|-c]`): one
//!      `GET /boot-status` to `127.0.0.1` on the port, whatever the host
//!      setting says, with no credential, one try, connect 2 s, at most
//!      4 s ([`crate::transport::Client::get_update_version`]). Any
//!      answer is read, whatever its status; no answer is "didn't
//!      answer". The body is read as the shell's Python reads it
//!      ([`daemon_version`]). Then the lines ([`cli_lines`]) and exit 0.
//!    - [`Form::App`] (`update`, `update --app`): the air-gap refusal
//!      ([`crate::airgap`], exit 1); then `Checking for app updates...`;
//!      then the newest release from GitHub ([`Github::get`]), read as
//!      the shell's Python reads it ([`latest_tag`]); then the lines
//!      ([`app_lines`]) and exit 0. An answer the shell's Python can't
//!      read (not JSON, not an object) ends it with 1, as `set -e` does.
//!    - [`Form::List`] (`update --list`): the air-gap refusal; then the
//!      releases from GitHub; then `Available K2 releases:` and the
//!      shell's Python's loop's lines ([`list_lines`]), and exit 1 when
//!      the Python raised, else 0.
//!
//! [`Github::get`] is `_app_update_get`: `mktemp -d` makes the folder
//! for curl's stderr and the body (`${TMPDIR:-/tmp}/k2-update.XXXXXX`;
//! when it fails, its own words and [`MKTEMP_FAILED`], exit 1); then
//! `_k2_github_curl`: `curl --no-location --proto =https -fsS
//! --connect-timeout 10 --max-time 30 -H "Accept: …" -o <folder>/body -w
//! %{redirect_url} <url>`, its stdout read as where a redirect points.
//! Each redirect is followed here, one curl per hop, only to GitHub's own
//! hosts ([`GITHUB_HOSTS`]), https only, at most [`MAX_REDIRS`]; any
//! other is refused before it is contacted ([`refusal`]): `Error:` and
//! the reason, exit 1. The err file's first line, less a leading
//! `curl: (N) `, is curl's message; the folder is removed. A curl that
//! fails prints [`unreachable_line`] and the command exits 1. No call
//! carries a token: no config, no Authorization header.
//!
//! mktemp and curl are the shell's: the ones on `PATH`, run with the
//! shell's arguments (`argv[0]` their name), this process's environment
//! and folder, stdin as it is; mktemp's stderr is this one's. When either
//! is missing, or bash would run something else for them (an exported
//! function, a startup file), the command goes to the shell, before
//! anything is printed. So does every form when the binary can't tell
//! what the shell's `python3` is ([`crate::python`]).
//!
//! Without `python3` ([`crate::python::NATIVE_WITHOUT_PYTHON3`]) each form
//! prints what it prints with one; the shell there says the daemon
//! didn't answer (`--cli`), or exits 127 once GitHub has answered
//! (`--app`, `--list`).
//!
//! Where this can differ from the shell: a NUL in what curl or the daemon
//! sends is dropped here as bash drops it, but bash 4.4 and later also
//! print a warning of their own; an answer that is not UTF-8 is read as
//! the shell's Python reads it in a UTF-8 locale (a version the daemon
//! didn't say, an exception), where in the C locale Python passes the
//! bytes through. GitHub and the daemon send UTF-8 JSON with no NUL. When
//! the file mktemp named can't be opened, or curl can't be started, the
//! shell also prints its own words, which the binary doesn't.

use super::curl_port;
use crate::airgap;
use crate::daemon::{self, Settings};
use crate::embedded;
use crate::exit::Exit;
use crate::offline::{self, Env};
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Gate};
use crate::python;
use crate::transport::skew;
use k2_client::brand::{CLI_NAME, RELEASES_LATEST_API, RELEASES_URL_PREFIX};
use k2_client::{product_name, release_repo};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["update"];
/// The binary answers it: each form behind its own switch, and [`run`] hands a form whose
/// switch is off to the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

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
/// GitHub's own hosts (`_K2_GITHUB_HOSTS`, `cli/src/update.sh`): the only
/// ones a GitHub fetch follows a redirect to. api.github.com answers a
/// moved repository's old path with a 301 to its new one, github.com does
/// the same for its pages and downloads, and github.com's release
/// downloads redirect to release-assets.githubusercontent.com
/// (objects.githubusercontent.com for older links).
pub const GITHUB_HOSTS: [&str; 4] = [
    "api.github.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
/// The most redirects one fetch follows (`_K2_GITHUB_MAX_REDIRS`).
pub const MAX_REDIRS: usize = 5;
/// curl's first arguments on every hop (`_k2_github_curl`): no `-L`, even
/// from a `~/.curlrc`, and https only.
pub const CURL_GUARD: [&str; 3] = ["--no-location", "--proto", "=https"];
/// curl's last arguments before the URL (`_k2_github_curl`): where a
/// redirect points, on stdout.
pub const CURL_WRITE_OUT: [&str; 2] = ["-w", "%{redirect_url}"];
/// curl's arguments after [`CURL_GUARD`] (`_app_update_get`), before
/// `-o <folder>/body`.
pub const CURL_ARGS: [&str; 7] = [
    "-fsS",
    "--connect-timeout",
    "10",
    "--max-time",
    "30",
    "-H",
    ACCEPT,
];
/// The program that makes the folder for curl's stderr and the body.
pub const MKTEMP: &str = "mktemp";
/// mktemp's flag for a folder.
pub const MKTEMP_DIR: &str = "-d";
/// That folder's name, in `${TMPDIR:-/tmp}`.
pub const TEMP_NAME: &str = "k2-update.XXXXXX";
/// The line when mktemp fails.
pub const MKTEMP_FAILED: &str = "Error: couldn't make a temp file to ask GitHub";
/// The line when curl fails, up to its code.
pub const UNREACHABLE_HEAD: &str = "Error: couldn't reach GitHub (curl exit ";

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
    /// `-h` or `--help`: the usage (the shell's).
    Help,
}

/// The words after `update`, as `_k2_update_form` reads them, with
/// `cmd_update_v2`'s `--check` and `-c`.
pub fn form(words: &[&str]) -> Form {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return Form::Help;
    }
    if words.contains(&"--list") {
        return Form::List;
    }
    let cli = words
        .iter()
        .rev()
        .find(|w| matches!(**w, "--app" | "--cli"))
        .is_some_and(|w| *w == "--cli");
    if cli {
        let check = words.iter().any(|w| matches!(*w, "--check" | "-c"));
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
        Form::Help => false,
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
         The CLI updates together with {product}; {CLI_NAME} never downloads itself.\n  \
         Desktop: {CLI_NAME} update --app, or the app's own updater.\n  \
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
         Or use: {CLI_NAME} update --app --list  to see all versions\n"
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

/// What a program gave: its stdout and its exit code (128 plus the
/// signal that ended it, as bash reports one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub stdout: Vec<u8>,
    pub code: i32,
}

/// Runs `program` (`name` on the shell's `PATH`) with `args` as the shell
/// does: `argv[0]` `name`, this environment and folder, stdin as it is,
/// stdout read, stderr to `stderr`. `None`: it could not be started.
pub fn run_program(program: &Path, name: &str, args: &[&OsStr], stderr: Stdio) -> Option<Ran> {
    let mut cmd = Command::new(program);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.arg0(name);
    }
    #[cfg(not(unix))]
    let _ = name;
    let out = cmd
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(stderr)
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

/// The program `name` bash runs in `env`: the first executable `name` on
/// `PATH`. `None` when there is none, or when bash could run something
/// else for it (a startup file or setting, an exported function): the
/// shell answers then.
pub fn shell_program(env: &Env, name: &str) -> Option<PathBuf> {
    if offline::STARTUP.iter().any(|n| env.get(n).is_some()) || env.any_named("BASH_FUNC_") {
        return None;
    }
    let path = env.get("PATH")?;
    daemon::find_on_path(name, path.to_os_string())
}

/// Whether the shell would find `name`, for a line the binary answers
/// without running it (a gate that sends the line to the shell when the
/// shell's program is missing). Off Unix no shell runs, so nothing of its
/// can be missing: always.
pub fn shell_has(env: &Env, name: &str) -> bool {
    !cfg!(unix) || shell_program(env, name).is_some()
}

/// `"${TMPDIR:-/tmp}/k2-update.XXXXXX"`, mktemp's argument.
pub fn temp_template(tmpdir: Option<&OsStr>) -> OsString {
    let mut s = match tmpdir {
        Some(t) if !t.is_empty() => t.to_os_string(),
        _ => OsString::from(crate::offline::default_tmpdir()),
    };
    s.push("/");
    s.push(TEMP_NAME);
    s
}

/// `msg=$(head -n 1 "$errf" | sed 's/^curl: ([0-9]*) //')`: the first
/// line of what curl wrote on stderr, less a leading `curl: (N) `.
pub fn curl_message(stderr: &[u8]) -> Vec<u8> {
    let line = match stderr.iter().position(|b| *b == b'\n') {
        Some(i) => &stderr[..=i],
        None => stderr,
    };
    let line = line
        .strip_prefix(b"curl: (")
        .and_then(|rest| {
            let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
            rest[digits..].strip_prefix(b") ")
        })
        .unwrap_or(line);
    substituted(line)
}

/// The line when curl ended with `rc`, with its message `msg`.
pub fn unreachable_line(rc: i32, msg: &[u8]) -> Vec<u8> {
    let mut line = format!("{UNREACHABLE_HEAD}{rc}").into_bytes();
    if !msg.is_empty() {
        line.extend_from_slice(b": ");
        line.extend_from_slice(msg);
    }
    line.push(b')');
    line
}

/// `_k2_github_url_ok`: `url` is https on one of [`GITHUB_HOSTS`], with
/// no name before an `@` and no port.
pub fn github_url_ok(url: &[u8]) -> bool {
    let Some(rest) = url.strip_prefix(b"https://") else {
        return false;
    };
    let end = rest
        .iter()
        .position(|b| matches!(*b, b'/' | b'?' | b'#'))
        .unwrap_or(rest.len());
    let host = &rest[..end];
    GITHUB_HOSTS.iter().any(|h| h.as_bytes() == host)
}

/// `_k2_github_where`: `url`'s scheme and host, for an error line, with
/// only the characters `tr -cd '[:graph:]'` keeps in the C locale.
pub fn github_where(url: &[u8]) -> String {
    let cut = |b: &[u8]| -> Vec<u8> {
        let end = b
            .iter()
            .position(|c| matches!(*c, b'/' | b'?' | b'#'))
            .unwrap_or(b.len());
        b[..end].to_vec()
    };
    let shown = match url.windows(3).position(|w| w == b"://") {
        Some(i) => {
            let mut v = url[..i].to_vec();
            v.extend_from_slice(b"://");
            v.extend(cut(&url[i + 3..]));
            v
        }
        None => cut(url),
    };
    shown
        .into_iter()
        .filter(|b| (0x21..=0x7e).contains(b))
        .map(char::from)
        .collect()
}

/// `_k2_github_curl`'s check of the `hops`th redirect, to `next`: the
/// reason it is refused, or `None` when it is followed.
pub fn refusal(hops: usize, next: &[u8]) -> Option<String> {
    let at = github_where(next);
    if hops > MAX_REDIRS {
        return Some(format!(
            "GitHub redirected more than {MAX_REDIRS} times (the last to {at}); stopped there"
        ));
    }
    if !next.starts_with(b"https://") {
        return Some(format!(
            "GitHub redirected to {at}, which is not https; nothing was sent there"
        ));
    }
    if !github_url_ok(next) {
        return Some(format!(
            "GitHub redirected to {at}, which is not GitHub; nothing was sent there"
        ));
    }
    None
}

/// How `_k2_github_curl` ended when it did not end with an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stopped {
    /// curl's exit code.
    Code(i32),
    /// A refused redirect's reason.
    Refused(String),
}

/// `"$tmp<tail>"`: the folder mktemp named, then `tail`.
fn under(tmp: &Path, tail: &str) -> PathBuf {
    let mut s = tmp.as_os_str().to_os_string();
    s.push(tail);
    PathBuf::from(s)
}

/// A path from what `$(mktemp …)` kept.
fn path_of(bytes: Vec<u8>) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Writes `line` and a newline to stderr, flushed.
fn stderr_bytes_line(line: &[u8]) {
    let mut bytes = line.to_vec();
    bytes.push(b'\n');
    let _ = out::write_flushed(&mut std::io::stderr().lock(), &bytes);
}

/// What the GitHub forms need, found before anything is printed.
#[derive(Debug, Clone)]
pub struct Github {
    curl: PathBuf,
    mktemp: PathBuf,
    template: OsString,
    airgap: airgap::State,
    settings_file: PathBuf,
}

impl Github {
    /// The shell's curl and mktemp, the temp file's place and the
    /// air-gap setting in `env`. `None`: the shell answers.
    pub fn find(env: &Env) -> Option<Github> {
        let curl = shell_program(env, CURL)?;
        let mktemp = shell_program(env, MKTEMP)?;
        let (airgap, settings_file) = airgap::state(env)?;
        Some(Github {
            curl,
            mktemp,
            template: temp_template(env.get("TMPDIR")),
            airgap,
            settings_file,
        })
    }

    /// `_k2_refuse_if_airgap`: its line and exit 1 when air-gap is on.
    fn refuse(&self) -> Option<Exit> {
        let line = airgap::refusal(self.airgap, &self.settings_file)?;
        stderr_bytes_line(&line);
        Some(Exit::Error)
    }

    /// `$(mktemp -d …)`: the folder it made, or `None` when it failed.
    fn temp_dir(&self) -> Option<PathBuf> {
        let ran = run_program(
            &self.mktemp,
            MKTEMP,
            &[OsStr::new(MKTEMP_DIR), self.template.as_os_str()],
            Stdio::inherit(),
        )?;
        (ran.code == 0).then(|| path_of(substituted(&ran.stdout)))
    }

    /// `_k2_github_curl "$1" <CURL_ARGS> -o "$tmp/body" 2>"$tmp/err"`:
    /// one curl per hop, every hop's stderr into `err` in turn.
    fn follow(&self, url: &str, body: &Path, err: &File) -> Result<(), Stopped> {
        let mut url = OsString::from(url);
        let mut hops = 0;
        loop {
            let mut args: Vec<&OsStr> = CURL_GUARD
                .iter()
                .chain(CURL_ARGS.iter())
                .map(|a| OsStr::new(*a))
                .collect();
            args.push(OsStr::new("-o"));
            args.push(body.as_os_str());
            args.extend(CURL_WRITE_OUT.iter().map(|a| OsStr::new(*a)));
            args.push(url.as_os_str());
            let ran = match err.try_clone() {
                Ok(f) => run_program(&self.curl, CURL, &args, Stdio::from(f)).unwrap_or(Ran {
                    stdout: Vec::new(),
                    code: 126,
                }),
                Err(_) => Ran {
                    stdout: Vec::new(),
                    code: 1,
                },
            };
            if ran.code != 0 {
                return Err(Stopped::Code(ran.code));
            }
            let next = substituted(&ran.stdout);
            if next.is_empty() {
                return Ok(());
            }
            hops += 1;
            if let Some(why) = refusal(hops, &next) {
                return Err(Stopped::Refused(why));
            }
            url = path_of(next).into_os_string();
        }
    }

    /// `_app_update_get <url>`: the body as `$(…)` keeps it, or the exit
    /// once its line is printed.
    pub fn get(&self, url: &str) -> Result<Vec<u8>, Exit> {
        let Some(tmp) = self.temp_dir() else {
            out::stderr_line(MKTEMP_FAILED);
            return Err(Exit::Error);
        };
        let (body, errf) = (under(&tmp, "/body"), under(&tmp, "/err"));
        // `2>"$tmp/err"`; a file that can't be opened runs no curl, and
        // the code is 1.
        let followed = match File::create(&errf) {
            Ok(f) => self.follow(url, &body, &f),
            Err(_) => Err(Stopped::Code(1)),
        };
        if let Err(Stopped::Refused(why)) = &followed {
            let _ = std::fs::remove_dir_all(&tmp);
            out::stderr_line(&format!("Error: {why}"));
            return Err(Exit::Error);
        }
        let msg = std::fs::read(&errf)
            .map(|b| curl_message(&b))
            .unwrap_or_default();
        if let Err(Stopped::Code(rc)) = followed {
            let _ = std::fs::remove_dir_all(&tmp);
            stderr_bytes_line(&unreachable_line(rc, &msg));
            return Err(Exit::Error);
        }
        // curl makes no file for an empty body.
        let answer = std::fs::read(&body).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(substituted(&answer))
    }
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
    let github = match form {
        Form::Cli { .. } | Form::Help => None,
        Form::App | Form::List => Some(Github::find(&Env::current())?),
    };
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if matches!(form, Form::Cli { .. }) && prelude::gate(&settings).is_none() {
        // A port curl won't take is the shell's: its URL is the port's
        // text as it is.
        settings.port_text.as_deref().and_then(curl_port)?;
    }
    Some(run_with(args, form, github.as_ref(), &settings))
}

fn run_with(args: &[OsString], form: Form, github: Option<&Github>, settings: &Settings) -> Exit {
    // The virtual desktop's guard stops every form; the connection gate
    // only `--cli`.
    match (prelude::gate(settings), form) {
        (Some(gate @ Gate::VirtualDesktop), _) | (Some(gate), Form::Cli { .. }) => {
            return gate.stop();
        }
        _ => {}
    }
    let ours = embedded::version();
    match (form, github) {
        (Form::Cli { check }, _) => {
            let theirs = ask_daemon(args, settings);
            print(&cli_lines(ours, theirs.as_deref(), check), Exit::Ok)
        }
        (Form::App, Some(github)) => app(ours, github),
        (Form::List, Some(github)) => list(github),
        // `run` finds the tools for both GitHub forms, and help is the
        // shell's.
        (Form::App | Form::List, None) | (Form::Help, _) => Exit::Error,
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
fn app(ours: &str, github: &Github) -> Exit {
    if let Some(exit) = github.refuse() {
        return exit;
    }
    if let Err(e) = out::stdout_line(CHECKING) {
        return out::after_stdout_error(&e);
    }
    let latest = match github.get(LATEST_URL) {
        Ok(body) => body,
        Err(exit) => return exit,
    };
    match latest_tag(&latest) {
        Some(tag) => print(&app_lines(ours, &tag), Exit::Ok),
        None => Exit::Error,
    }
}

/// The list form after the gates.
fn list(github: &Github) -> Exit {
    if let Some(exit) = github.refuse() {
        return exit;
    }
    let releases = match github.get(LIST_URL) {
        Ok(body) => body,
        Err(exit) => return exit,
    };
    if let Err(e) = out::stdout_line(LIST_HEAD) {
        return out::after_stdout_error(&e);
    }
    let (lines, ok) = list_lines(&releases);
    let exit = if ok { Exit::Ok } else { Exit::Error };
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
        for f in ["update.sh", "update-flags.sh", "cli-tools-policy.sh"] {
            let p = dir.join(f);
            s.push_str(
                &std::fs::read_to_string(&p)
                    .unwrap_or_else(|e| panic!("read {}: {e}", p.display())),
            );
        }
        // The shell builds its release URLs from prelude.sh's repo.
        let p = dir.join("prelude.sh");
        let prelude =
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
        let repo = k2_client::brand::RELEASE_REPO;
        assert!(
            prelude.contains(&format!("\n_K2_RELEASE_REPO=\"{repo}\"\n")),
            "prelude.sh's release repo is not {repo}"
        );
        assert!(
            s.contains("${_K2_RELEASE_REPO}"),
            "update.sh no longer builds its URLs from _K2_RELEASE_REPO"
        );
        s.replace("${_K2_RELEASE_REPO}", repo)
    }

    #[test]
    fn the_forms_as_k2_update_form_reads_them() {
        for (words, want) in [
            (&[][..], Form::App),
            (&["--app"], Form::App),
            (&["--app", "--check"], Form::App),
            (&["--cli"], Form::Cli { check: false }),
            (&["--cli", "--check"], Form::Cli { check: true }),
            (&["-c", "--cli"], Form::Cli { check: true }),
            (&["--cli", "--app"], Form::App),
            (&["--app", "--cli"], Form::Cli { check: false }),
            (
                &["--cli", "--app", "-c", "--cli"],
                Form::Cli { check: true },
            ),
            (&["--cli", "--list"], Form::List),
            (&["--list", "--cli"], Form::List),
            (&["--app", "--list", "--cli"], Form::List),
            (&["x", "--list"], Form::List),
            (&["--cli", "x", "--bogus"], Form::Cli { check: false }),
            (&["--CLI"], Form::App),
            (&["--LIST"], Form::App),
            (&["--check"], Form::App),
            (&["-c"], Form::App),
            (&["-l"], Form::App),
            (&["--help"], Form::Help),
            (&["--list", "-h"], Form::Help),
            (&["--cli", "--help", "--check"], Form::Help),
            (&["help"], Form::App),
            (&["-H"], Form::App),
        ] {
            assert_eq!(form(words), want, "{words:?}");
        }
    }

    /// The form's words, the dispatch and the gate's skip are the
    /// shell's.
    #[test]
    fn the_flag_loop_is_the_shell_s() {
        let s = source();
        for line in [
            // _k2_update_form.
            "_k2_update_form() {\n    local a form=app\n",
            "        case \"$a\" in -h|--help) echo help; return 0 ;; esac\n",
            "            --list) echo list; return 0 ;;\n            --app) form=app ;;\n            --cli) form=cli ;;\n",
            "    echo \"$form\"\n}\n",
            // The connection gate skips every form but --cli.
            "if [ \"${1:-}\" = \"update\" ]; then\n    case \"$(_k2_update_form \"${@:2}\")\" in help|app|list) _skip_conn_gate=1 ;; esac\nfi\n",
            // cmd_update_v2.
            "        case \"$a\" in --check|-c) cli_args+=(\"$a\") ;; esac\n",
            "    case \"$(_k2_update_form \"$@\")\" in\n        help) _update_usage ;;\n        list) cmd_app_update --list ;;\n        cli)  cmd_update ${cli_args[@]+\"${cli_args[@]}\"} ;;\n        *)    cmd_app_update ;;\n    esac\n",
            // cmd_update.
            "            --check|-c) check_only=1; shift ;;\n",
            // cmd_app_update: air-gap first, then the form.
            "cmd_app_update() {\n    # Both forms ask GitHub: refused under air-gap, like k2 daemon install.\n    _k2_refuse_if_airgap\n    if [ \"${1:-}\" = \"--list\" ]; then\n",
        ] {
            assert!(s.contains(line), "no `{line}` in cli/src");
        }
    }

    /// The URLs, the programs' arguments and every line are the shell's.
    #[test]
    fn the_texts_are_the_shell_s() {
        let s = source();
        let (head, tail) = CURL_ARGS.split_at(5);
        for want in [
            format!(
                "    _K2_GITHUB_REFUSED=\"\"\n    _k2_github_curl \"$1\" {} \\\n        {} \"{}\" -o \"$tmp/body\" 2>\"$tmp/err\" || rc=$?\n",
                head.join(" "),
                tail[0],
                tail[1]
            ),
            format!(
                "        next=$({CURL} {} \"$@\" {} '{}' \"$url\") || rc=$?\n",
                CURL_GUARD.join(" "),
                CURL_WRITE_OUT[0],
                CURL_WRITE_OUT[1]
            ),
            format!("_K2_GITHUB_HOSTS=\"{}\"\n", GITHUB_HOSTS.join(" ")),
            format!("_K2_GITHUB_MAX_REDIRS={MAX_REDIRS}\n"),
            "        [ \"$rc\" -eq 0 ] || return \"$rc\"\n        [ -n \"$next\" ] || return 0\n        hops=$((hops + 1))\n        if [ \"$hops\" -gt \"$_K2_GITHUB_MAX_REDIRS\" ]; then\n".to_string(),
            "            _K2_GITHUB_REFUSED=\"GitHub redirected more than $_K2_GITHUB_MAX_REDIRS times (the last to $(_k2_github_where \"$next\")); stopped there\"\n".to_string(),
            "        case \"$next\" in\n            https://*) ;;\n            *) _K2_GITHUB_REFUSED=\"GitHub redirected to $(_k2_github_where \"$next\"), which is not https; nothing was sent there\"\n".to_string(),
            "        if ! _k2_github_url_ok \"$next\"; then\n            _K2_GITHUB_REFUSED=\"GitHub redirected to $(_k2_github_where \"$next\"), which is not GitHub; nothing was sent there\"\n".to_string(),
            "    rest=\"${1#https://}\"\n    host=\"${rest%%[/?#]*}\"\n    for h in $_K2_GITHUB_HOSTS; do\n        [ \"$host\" = \"$h\" ] && return 0\n    done\n".to_string(),
            "        *://*) scheme=\"${1%%://*}\"; rest=\"${1#*://}\"; rest=\"${scheme}://${rest%%[/?#]*}\" ;;\n        *) rest=\"${1%%[/?#]*}\" ;;\n    esac\n    printf '%s' \"$rest\" | LC_ALL=C tr -cd '[:graph:]'\n".to_string(),
            "    if [ -n \"$_K2_GITHUB_REFUSED\" ]; then\n        rm -rf \"$tmp\"\n        echo \"Error: $_K2_GITHUB_REFUSED\" >&2\n        return 1\n    fi\n    msg=$(head -n 1 \"$tmp/err\" | sed 's/^curl: ([0-9]*) //')\n".to_string(),
            format!("    tmp=$({MKTEMP} {MKTEMP_DIR} \"${{TMPDIR:-/tmp}}/{TEMP_NAME}\") || {{\n        echo \"{MKTEMP_FAILED}\" >&2\n        return 1\n    }}\n"),
            "    if [ \"$rc\" -ne 0 ]; then\n        rm -rf \"$tmp\"\n".to_string(),
            format!("        echo \"{UNREACHABLE_HEAD}$rc${{msg:+: $msg}})\" >&2\n        return 1\n    fi\n    # curl makes no file for an empty body.\n    if [ -f \"$tmp/body\" ]; then cat \"$tmp/body\"; fi\n    rm -rf \"$tmp\"\n"),
            format!("        releases=$(_app_update_get \"{LIST_URL}\") || exit 1\n        echo \"{LIST_HEAD}\"\n        printf '%s' \"$releases\" | python3 -c \""),
            format!("    echo \"{CHECKING}\"\n    local latest\n    latest=$(_app_update_get \"{LATEST_URL}\") || exit 1\n"),
            "    tag=$(echo \"$latest\" | python3 -c \"import json,sys; print(json.loads(sys.stdin.read()).get('tag_name','unknown'))\" 2>/dev/null)\n".to_string(),
            r#""http://127.0.0.1:${PORT}/boot-status" 2>/dev/null) || return 0"#.to_string(),
            "_K2_CURL_TRIES=1 _k2_curl \"\" -s --connect-timeout 2 --max-time 4 \\".to_string(),
        ] {
            assert!(s.contains(&want), "no `{want}` in cli/src");
        }
        assert_eq!(ACCEPT, "Accept: application/vnd.github.v3+json");
        assert_eq!(
            unreachable_line(7, b"x y"),
            b"Error: couldn't reach GitHub (curl exit 7: x y)".to_vec()
        );
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
        assert!(s.contains("data = json.loads(sys.stdin.read())\nfor r in data:\n"));
        assert!(s.contains(r#"v = b.get("version") if isinstance(b, dict) else None"#));
        assert!(s.contains(r#"    b = json.loads(sys.stdin.read() or "null")"#));
    }

    #[test]
    fn curl_s_message_as_head_and_sed_keep_it() {
        for (stderr, want) in [
            ("", ""),
            (
                "curl: (22) The requested URL returned error: 404\n",
                "The requested URL returned error: 404",
            ),
            (
                "curl: (6) Could not resolve host: x\nmore\n",
                "Could not resolve host: x",
            ),
            ("curl: (7) a", "a"),
            ("curl: () b\n", "b"),
            ("curl: (7)b\n", "curl: (7)b"),
            ("curl: (7x) c\n", "curl: (7x) c"),
            ("curl: noise on stderr\n", "curl: noise on stderr"),
            (" curl: (7) d\n", " curl: (7) d"),
            ("curl: (7) curl: (8) e\n", "curl: (8) e"),
            ("\nsecond\n", ""),
            ("a\0b\n", "ab"),
            ("tab\tand\r\n", "tab\tand\r"),
        ] {
            assert_eq!(
                curl_message(stderr.as_bytes()),
                want.as_bytes().to_vec(),
                "{stderr:?}"
            );
        }
        assert_eq!(curl_message(b"\xff\xfe\n"), b"\xff\xfe".to_vec());
    }

    #[test]
    fn the_lines_when_github_is_out_of_reach() {
        assert_eq!(
            unreachable_line(22, b"The requested URL returned error: 404"),
            b"Error: couldn't reach GitHub (curl exit 22: The requested URL returned error: 404)"
                .to_vec()
        );
        assert_eq!(
            unreachable_line(22, b""),
            b"Error: couldn't reach GitHub (curl exit 22)".to_vec()
        );
        assert_eq!(
            unreachable_line(141, b"\xff"),
            b"Error: couldn't reach GitHub (curl exit 141: \xff)".to_vec()
        );
    }

    #[test]
    fn the_temp_file_s_place() {
        // With no TMPDIR, `/tmp` (off Unix, the system's temp folder).
        let unset = format!("{}/k2-update.XXXXXX", crate::offline::default_tmpdir());
        if cfg!(unix) {
            assert_eq!(unset, "/tmp/k2-update.XXXXXX");
        }
        for (tmpdir, want) in [
            (None, unset.as_str()),
            (Some(""), unset.as_str()),
            (Some("/var/t"), "/var/t/k2-update.XXXXXX"),
            (Some("/var/t/"), "/var/t//k2-update.XXXXXX"),
            (Some("rel"), "rel/k2-update.XXXXXX"),
        ] {
            assert_eq!(
                temp_template(tmpdir.map(OsStr::new)),
                OsString::from(want),
                "{tmpdir:?}"
            );
        }
    }

    /// A refused redirect names GitHub and where it pointed, never the
    /// brand's command or product; the list's head names the product.
    #[test]
    fn the_redirect_lines_say_github_never_the_brand_s_name() {
        let cli = k2_client::brand::CLI_NAME;
        let product = k2_client::brand::PRODUCT_NAME;
        for (hops, next) in [
            (MAX_REDIRS + 1, "https://github.com/x"),
            (1, "http://api.github.com/x"),
            (1, "https://evil.example/x"),
        ] {
            let line = refusal(hops, next.as_bytes()).expect("refused");
            assert!(line.starts_with("GitHub redirected "), "{line}");
            for name in [cli, product] {
                assert!(!line.contains(name), "{line} names {name}");
            }
        }
        assert_eq!(LIST_HEAD, format!("Available {product} releases:"));
    }

    /// `_k2_github_url_ok`: https on one of GitHub's hosts, exactly.
    #[test]
    fn only_github_s_hosts_over_https_are_followed() {
        for (url, want) in [
            (
                "https://api.github.com/repositories/42/releases/latest",
                true,
            ),
            ("https://github.com/o/r/releases/download/v1/x", true),
            ("https://objects.githubusercontent.com/x?sig=1", true),
            ("https://release-assets.githubusercontent.com/x?sig=1", true),
            ("https://github.com", true),
            ("https://github.com?x", true),
            ("https://github.com#x", true),
            ("http://github.com/x", false),
            ("HTTPS://github.com/x", false),
            ("https://GitHub.com/x", false),
            ("https://evil.example/x", false),
            ("https://github.com.evil.example/x", false),
            ("https://evil.example/github.com", false),
            ("https://api.github.com@evil.example/x", false),
            ("https://name@github.com/x", false),
            ("https://api.github.com:443/x", false),
            ("https://api.github.com:8443/x", false),
            ("https://github.com objects.githubusercontent.com/x", false),
            ("https://.github.com/x", false),
            ("https:///x", false),
            ("https://", false),
            ("ftp://github.com/x", false),
            ("", false),
        ] {
            assert_eq!(github_url_ok(url.as_bytes()), want, "{url}");
        }
    }

    /// `_k2_github_where`: the scheme and host, printable bytes only.
    #[test]
    fn the_error_line_names_the_scheme_and_host() {
        for (url, want) in [
            ("https://evil.example/a/b?c#d", "https://evil.example"),
            ("http://api.github.com/x", "http://api.github.com"),
            (
                "https://api.github.com@evil.example/x",
                "https://api.github.com@evil.example",
            ),
            (
                "https://api.github.com:8443/x",
                "https://api.github.com:8443",
            ),
            ("https://h?q=/x", "https://h"),
            ("a://b://c/d", "a://b:"),
            ("evil.example/x", "evil.example"),
            (
                "https://ev il\u{1b}[31m.example/x",
                "https://evil[31m.example",
            ),
            ("https://\u{e9}.example/", "https://.example"),
            ("", ""),
        ] {
            assert_eq!(github_where(url.as_bytes()), want, "{url:?}");
        }
    }

    /// `_k2_github_curl`'s checks, in its order: the cap, https, the host.
    #[test]
    fn a_redirect_is_followed_or_refused_as_the_shell_does() {
        assert_eq!(refusal(1, b"https://api.github.com/repositories/1"), None);
        assert_eq!(refusal(MAX_REDIRS, b"https://github.com/x"), None);
        assert_eq!(
            refusal(MAX_REDIRS + 1, b"https://github.com/x"),
            Some("GitHub redirected more than 5 times (the last to https://github.com); stopped there".into())
        );
        assert_eq!(
            refusal(MAX_REDIRS + 1, b"http://evil.example/x"),
            Some("GitHub redirected more than 5 times (the last to http://evil.example); stopped there".into())
        );
        assert_eq!(
            refusal(1, b"http://api.github.com/x"),
            Some("GitHub redirected to http://api.github.com, which is not https; nothing was sent there".into())
        );
        assert_eq!(
            refusal(2, b"https://evil.example/x"),
            Some("GitHub redirected to https://evil.example, which is not GitHub; nothing was sent there".into())
        );
        assert_eq!(MAX_REDIRS, 5);
    }

    /// `_app_update_get` against a stub curl and the real mktemp: the
    /// arguments of each hop, the body, the code, and no file left.
    #[cfg(unix)]
    #[test]
    fn the_get_runs_mktemp_and_curl_as_the_shell_does() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("k5-update-get-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let tmp = d.join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let log = d.join("argv");
        let curl = d.join("curl");
        let path = std::env::var_os("PATH").expect("a PATH");
        let mktemp = daemon::find_on_path(MKTEMP, path).expect("a mktemp on PATH");
        let github = |template: &Path| Github {
            curl: curl.clone(),
            mktemp: mktemp.clone(),
            template: temp_template(Some(template.as_os_str())),
            airgap: airgap::State::Off,
            settings_file: d.join("settings.json"),
        };
        let left = || std::fs::read_dir(&tmp).unwrap().count();
        // The stub curl logs its arguments, one hop a line, with -o's
        // file as <body> (and that file's name in `opath`); writes `err`
        // on stderr; then, when `hops` has a line `<url> <next>`, prints
        // <next> as -w does and exits 0; else writes `body` where -o says
        // and exits `rc`.
        std::fs::write(
            &curl,
            format!(
                r#"#!/bin/sh
d='{d}'
prev=""; url=""; out=""
for a in "$@"; do
    if [ "$prev" = "-o" ]; then out="$a"; printf '[<body>]'; else printf '[%s]' "$a"; fi
    prev="$a"; url="$a"
done >> "$d/argv"
echo >> "$d/argv"
echo "$out" > "$d/opath"
cat "$d/err" >&2
next=$(while read -r from to; do [ "$from" = "$url" ] && printf '%s' "$to"; done < "$d/hops")
if [ -n "$next" ]; then printf '%s' "$next"; exit 0; fi
cat "$d/body" > "$out"
exit "$(cat "$d/rc")"
"#,
                d = d.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let call = |g: &Github, hops: &[(&str, &str)], body: &str, err: &str, rc: i32| {
            let _ = std::fs::remove_file(&log);
            let mut h = String::new();
            for (from, to) in hops {
                h.push_str(&format!("{from} {to}\n"));
            }
            std::fs::write(d.join("hops"), h).unwrap();
            std::fs::write(d.join("body"), body).unwrap();
            std::fs::write(d.join("err"), err).unwrap();
            std::fs::write(d.join("rc"), rc.to_string()).unwrap();
            g.get(LATEST_URL)
        };
        let hop = |url: &str| {
            let mut line = String::new();
            for a in CURL_GUARD.iter().chain(CURL_ARGS.iter()) {
                line.push_str(&format!("[{a}]"));
            }
            line.push_str("[-o][<body>]");
            for a in CURL_WRITE_OUT.iter().chain([&url]) {
                line.push_str(&format!("[{a}]"));
            }
            line.push('\n');
            line
        };
        let logged = || std::fs::read_to_string(&log).unwrap();
        let g = github(&tmp);
        assert_eq!(
            call(&g, &[], "{\"tag_name\":\"v1\"}\n\n", "curl: noise\n", 0),
            Ok(b"{\"tag_name\":\"v1\"}".to_vec())
        );
        assert_eq!(logged(), hop(LATEST_URL));
        let opath = std::fs::read_to_string(d.join("opath")).unwrap();
        assert!(
            opath.starts_with(&format!("{}/k2-update.", tmp.display()))
                && opath.ends_with("/body\n"),
            "{opath}"
        );
        assert_eq!(left(), 0, "a temp file left");
        assert_eq!(
            call(
                &g,
                &[],
                "partial",
                "curl: (22) The requested URL returned error: 404\n",
                22
            ),
            Err(Exit::Error)
        );
        assert_eq!(left(), 0, "a temp file left");
        // A moved repository: the 301 to the new path is followed.
        let moved = "https://api.github.com/repositories/42/releases/latest";
        assert_eq!(
            call(&g, &[(LATEST_URL, moved)], "{\"tag_name\":\"v2\"}", "", 0),
            Ok(b"{\"tag_name\":\"v2\"}".to_vec())
        );
        assert_eq!(logged(), format!("{}{}", hop(LATEST_URL), hop(moved)));
        assert_eq!(left(), 0, "a temp file left");
        // Five hops through GitHub's hosts: followed; a sixth: refused.
        let chain: Vec<String> = (0..7)
            .map(|i| format!("https://{}/c/{i}", GITHUB_HOSTS[i % 4]))
            .collect();
        let mut hops: Vec<(&str, &str)> = vec![(LATEST_URL, chain[0].as_str())];
        for i in 0..4 {
            hops.push((chain[i].as_str(), chain[i + 1].as_str()));
        }
        assert_eq!(call(&g, &hops, "five", "", 0), Ok(b"five".to_vec()));
        assert_eq!(logged().lines().count(), 6);
        hops.push((chain[4].as_str(), chain[5].as_str()));
        assert_eq!(call(&g, &hops, "six", "", 0), Err(Exit::Error));
        assert_eq!(
            logged().lines().count(),
            6,
            "the sixth redirect's target was asked"
        );
        assert!(!logged().contains(&chain[5]));
        assert_eq!(left(), 0, "a temp file left");
        // Off GitHub, or plain http: refused, never asked.
        for to in ["https://evil.example/x", "http://api.github.com/x"] {
            assert_eq!(
                call(&g, &[(LATEST_URL, to)], "x", "", 0),
                Err(Exit::Error),
                "{to}"
            );
            assert_eq!(logged(), hop(LATEST_URL), "{to}");
            assert_eq!(left(), 0, "a temp file left");
        }
        // mktemp fails: curl never runs.
        assert_eq!(
            call(&github(&d.join("no-such-folder")), &[], "x", "", 0),
            Err(Exit::Error)
        );
        assert!(!log.exists(), "curl ran with no temp folder");
        let _ = std::fs::remove_dir_all(&d);
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
        assert!(!native(Form::Help));
    }
}
