//! `k5 daemon start|stop|restart|survive|log|logs|uninstall`, and the
//! daemon row's usage for any other word, as the shell CLI answers them:
//! `cmd_daemon_*` (`cli/src/daemon.sh`) and the router's `daemon` arm
//! (`cli/src/router.sh`). `install` stays the shell's.
//!
//! The service is driven as the shell drives it: by the programs on
//! `PATH`, by name (`systemctl`, `launchctl`, `id`, `uname`, `tail`), with
//! the shell's arguments. Nothing here names a program by an absolute
//! path. The OS is the shell's `_daemon_os`: what `uname -s` prints
//! ([`os`]), so the binary takes the branch the shell takes on the same
//! machine.
//!
//! Calls to the daemon are `_k2_curl`'s, made by the shared client:
//!
//! - `_daemon_local_post`: a POST with no data, the owner token from
//!   `daemon.token` as Bearer, connect 5 s, at most 30 s; only whether
//!   the answer holds the word the shell's `grep -q` looks for counts
//!   ([`local_post`]). With no token there, nothing is sent.
//! - `_daemon_wait_ready` (`--wait`): two seconds, then up to 30 GETs of
//!   `/boot-status` two seconds apart, connect 3 s, at most 5 s, with the
//!   token as Bearer when there is one ([`wait_ready`]).
//! - `survive on|off`: `cli_post_json /cli/settings/update`; `survive
//!   status`: `cli_request /cli/settings/get`, then `/boot-status` with
//!   `$TOKEN` (connect 3 s, at most 5 s). Then what the shell's Python
//!   prints of them.
//! - `restart --host <url>`: the POST to that server, with the token read
//!   from stdin (`--token-stdin`), given (`--token`) or in `daemon.token`.
//!   The token is only ever a header: never in argv, a URL, a log line or
//!   a child's environment.
//!
//! The binary hands these to the shell ([`parse`] says `None`) before it
//! prints, reads or sends anything: `install`; a value flag as the last
//! word (`restart --host`, `restart --token`, `log --tail`, `log -n`: the
//! shell's own `shift 2` or `$2` failure); `restart --host` with a host
//! that is not an `http://` or `https://` URL the shared remote reader
//! takes ([`remote::base`]: the shell's curl would read a bare name as
//! `http://`, a path or a user part as they are); and `restart --host
//! https://…` to another machine with a proxy set ([`proxied`]: the
//! shell's curl goes through it, and the shared client has no proxy).

use super::{capture, linux_unit, systemctl};
use crate::daemon::Settings;
use crate::exit::Exit;
use crate::families::connections::held;
use crate::families::{raised, Raise, Shown};
use crate::out::json::Value;
use crate::out::pylib::json::loads;
use crate::out::{self, py};
use crate::prompt::{self, Echo};
use crate::transport::{remote, Call, Channel, Client, Error, OwnCurl};
use k2_client::brand::{CLI_NAME as CLI, PRODUCT_NAME as PRODUCT};
use k2_client::hosts::ZONE_SUFFIX;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Writes a line on stdout; a broken stdout ends the command as the
/// shell's `echo` does.
macro_rules! say {
    ($($t:tt)*) => {
        if let Err(e) = out::stdout_line(&format!($($t)*)) {
            return out::after_stdout_error(&e);
        }
    };
}

/// A verb's words, read as the shell reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Life {
    /// The router's usage (`*)`): exit 1.
    Usage,
    Start,
    Stop {
        keep: bool,
    },
    /// `stop` with a word other than `--keep-agents`: exit 2.
    StopUsage,
    Restart(Restart),
    /// `restart` with a word it doesn't take: exit 2.
    RestartUnknown(String),
    Survive(Survive),
    Log {
        tail: String,
        follow: bool,
    },
    Uninstall {
        yes: bool,
    },
}

/// `restart`'s flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Restart {
    /// `--host`; empty: the local daemon.
    pub host: String,
    /// `--token`; empty: none given.
    pub token: String,
    pub token_stdin: bool,
    pub wait: bool,
    pub sessions: bool,
}

/// `survive`'s word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Survive {
    Switch(bool),
    Status,
    Help,
    Usage,
}

/// The words after `daemon <verb>` (`rest`) for `verb`; `None`: the
/// shell's.
pub fn parse(verb: &str, rest: &[&str]) -> Option<Life> {
    Some(match verb {
        "start" => Life::Start,
        "stop" => {
            let mut keep = false;
            for w in rest {
                if *w != "--keep-agents" {
                    return Some(Life::StopUsage);
                }
                keep = true;
            }
            Life::Stop { keep }
        }
        "restart" => return restart_words(rest),
        "survive" => Life::Survive(match rest.first().copied().unwrap_or("status") {
            "on" => Survive::Switch(true),
            "off" => Survive::Switch(false),
            "status" => Survive::Status,
            "-h" | "--help" | "help" => Survive::Help,
            _ => Survive::Usage,
        }),
        "log" | "logs" => {
            let (mut tail, mut follow) = ("200".to_string(), false);
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "--tail" | "-n" => {
                        tail = rest.get(i + 1)?.to_string();
                        i += 2;
                    }
                    "--follow" | "-f" => {
                        follow = true;
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            Life::Log { tail, follow }
        }
        "install" => return None,
        "uninstall" => Life::Uninstall {
            yes: matches!(rest.first().copied(), Some("--yes" | "-y")),
        },
        _ => Life::Usage,
    })
}

/// `cmd_daemon_restart`'s loop.
fn restart_words(rest: &[&str]) -> Option<Life> {
    let mut r = Restart::default();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--sessions" => r.sessions = true,
            "--host" | "--token" => {
                let v = rest.get(i + 1)?.to_string();
                if w == "--host" {
                    r.host = v;
                } else {
                    r.token = v;
                }
                i += 1;
            }
            "--token-stdin" => r.token_stdin = true,
            "--wait" => r.wait = true,
            _ => {
                if let Some(h) = w.strip_prefix("--host=") {
                    r.host = h.to_string();
                } else if let Some(t) = w.strip_prefix("--token=") {
                    r.token = t.to_string();
                } else {
                    return Some(Life::RestartUnknown(w.to_string()));
                }
            }
        }
        i += 1;
    }
    if !r.host.is_empty() && (remote_base(&r.host).is_none() || proxied(&r.host)) {
        return None;
    }
    Some(Life::Restart(r))
}

/// `_k2_curl` would send to `host` through the person's proxy
/// ([`crate::families::remote::logout::proxied`]): the shell's.
fn proxied(host: &str) -> bool {
    crate::families::remote::logout::proxied(host)
}

/// `--host`'s server, when it is a URL the binary sends to itself.
fn remote_base(host: &str) -> Option<remote::Base> {
    if !(host.starts_with("http://") || host.starts_with("https://")) {
        return None;
    }
    remote::base(host)
}

/// `_daemon_os`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Linux,
    Windows,
    Other,
}

/// `_daemon_os`: `uname -s`, as the shell reads it.
pub fn os() -> Os {
    let (name, _) = capture("uname", &["-s"]);
    match name.as_str() {
        "Darwin" => Os::MacOs,
        "Linux" => Os::Linux,
        n if n.starts_with("MINGW")
            || n.starts_with("MSYS")
            || n.starts_with("CYGWIN")
            || n == "Windows_NT" =>
        {
            Os::Windows
        }
        _ => Os::Other,
    }
}

/// `$HOME` as the shell writes it into a path.
fn user_home() -> String {
    std::env::var_os("HOME")
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The launch agent's label and plist (`_daemon_label`, `_daemon_plist`):
/// `dev.k2.daemon`, or the old label when only its plist is there.
fn launchd() -> (&'static str, String) {
    let agents = format!("{}/Library/LaunchAgents", user_home());
    let plist = format!("{agents}/dev.k2.daemon.plist");
    let old = format!("{agents}/com.k2so.k2so-daemon.plist");
    if !Path::new(&plist).is_file() && Path::new(&old).is_file() {
        ("com.k2so.k2so-daemon", old)
    } else {
        ("dev.k2.daemon", plist)
    }
}

/// `$(prog args… 2>&1)`: what it wrote on stdout and stderr, in order,
/// its trailing newlines and NULs gone. Nothing when it can't start.
fn capture_both(prog: &str, args: &[&str]) -> String {
    let Ok((mut reader, writer)) = std::io::pipe() else {
        return String::new();
    };
    let Ok(writer2) = writer.try_clone() else {
        return String::new();
    };
    let mut cmd = Command::new(prog);
    cmd.args(args).stdout(writer).stderr(writer2);
    let child = cmd.spawn();
    drop(cmd);
    let Ok(mut child) = child else {
        return String::new();
    };
    let mut bytes = Vec::new();
    let _ = reader.read_to_end(&mut bytes);
    let _ = child.wait();
    let text: Vec<u8> = bytes.into_iter().filter(|b| *b != 0).collect();
    let mut text = String::from_utf8_lossy(&text).into_owned();
    while text.ends_with('\n') {
        text.pop();
    }
    text
}

/// `prog args…` with this process's stdin, stdout and stderr: whether it
/// exited 0.
fn run_inherited(prog: &str, args: &[&str]) -> bool {
    Command::new(prog)
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

/// `_daemon_systemctl <scope> <args…>`, its output the command's.
fn systemctl_shown(scope: &str, args: &[&str]) -> bool {
    let mut all = Vec::with_capacity(args.len() + 1);
    if scope == "user" {
        all.push("--user");
    }
    all.extend_from_slice(args);
    run_inherited("systemctl", &all)
}

/// `_daemon_write_stop_intent cli <name>`: `<K2 home>/run/<name>.json`,
/// 0600 in a 0700 folder, through a temporary file renamed over it.
fn write_intent(home: &Path, name: &str) -> bool {
    let dir = home.join("run");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let pid = std::process::id();
    let tmp = dir.join(format!("{name}.json.tmp.{pid}"));
    let text = format!("{{\"by\":\"cli\",\"at\":{now},\"nonce\":\"{pid}-{now}\"}}\n");
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600);
    }
    let written = open.open(&tmp).and_then(|mut f| {
        use std::io::Write;
        f.write_all(text.as_bytes())
    });
    if written.is_err() {
        return false;
    }
    std::fs::rename(&tmp, dir.join(format!("{name}.json"))).is_ok()
}

/// `rm -f <K2 home>/run/<name>.json`.
fn remove_intent(home: &Path, name: &str) {
    let _ = std::fs::remove_file(home.join("run").join(format!("{name}.json")));
}

/// `_daemon_slice_pending <scope> <unit>`: `(running slice, unit's slice)`
/// when they differ.
fn slice_pending(scope: &str, unit: &str) -> Option<(String, String)> {
    let (want, ok) = systemctl(scope, &["show", "-p", "Slice", "--value", unit]);
    if !ok {
        return None;
    }
    let (cg, ok) = systemctl(scope, &["show", "-p", "ControlGroup", "--value", unit]);
    if !ok || want.is_empty() || cg.is_empty() {
        return None;
    }
    // `${cg%/*}`, then `${run##*/}`.
    let run = cg.rsplit_once('/').map_or(cg.as_str(), |(head, _)| head);
    let run = run.rsplit_once('/').map_or(run, |(_, tail)| tail);
    let run = if run.is_empty() { "-.slice" } else { run };
    (run != want).then(|| (run.to_string(), want))
}

/// `tr -d '[:space:]' < <file>`, when it is a file.
fn token_in(file: &Path) -> String {
    super::file_text(file)
}

/// What the verbs say the daemon is at: `$BASE_URL`.
fn base_url(s: &Settings) -> String {
    format!(
        "http://{}:{}",
        s.host,
        s.port_text.as_deref().unwrap_or_default()
    )
}

/// `_daemon_local_post <path> 2>/dev/null | grep -q <want>`.
fn local_post(s: &Settings, client: &mut Client, path: &str, want: &str) -> bool {
    let token = token_in(&s.home.join("daemon.token"));
    if token.is_empty() {
        return false;
    }
    let how = OwnCurl {
        post: true,
        credential: Some(&token),
        max_time: Duration::from_secs(30),
        note: false,
    };
    match client.own_curl_bare(path, how, Duration::from_secs(5)) {
        Ok(r) => contains(&r.body, want.as_bytes()),
        Err(_) => false,
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// `grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1`, then
/// the `sed` that keeps what is inside the last quotes.
pub fn ready_version(text: &str) -> Option<String> {
    let space = |c: char| matches!(c, ' ' | '\t' | '\r' | '\u{b}' | '\u{c}');
    for line in text.split('\n') {
        let mut from = 0;
        while let Some(at) = line[from..].find("\"version\"") {
            let start = from + at;
            from = start + 1;
            let rest = line[start + "\"version\"".len()..].trim_start_matches(space);
            let Some(rest) = rest.strip_prefix(':') else {
                continue;
            };
            let Some(rest) = rest.trim_start_matches(space).strip_prefix('"') else {
                continue;
            };
            if let Some(end) = rest.find('"') {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

/// `_daemon_wait_ready <base> [token]`: whether it came back.
fn wait_ready(client: &mut Client, base: &str, token: Option<&str>) -> Result<bool, Exit> {
    std::thread::sleep(Duration::from_secs(2));
    for _ in 0..30 {
        let how = OwnCurl {
            post: false,
            credential: token,
            max_time: Duration::from_secs(5),
            note: false,
        };
        let body = client
            .own_curl_bare("/boot-status", how, Duration::from_secs(3))
            .map(|r| r.body)
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&held(&body)).into_owned();
        if text.contains("\"phase\":\"ready\"") || text.contains("\"phase\": \"ready\"") {
            let line = match ready_version(&text) {
                Some(v) if !v.is_empty() => format!("Daemon back up on {v}"),
                _ => "Daemon back up (version unknown)".to_string(),
            };
            if let Err(e) = out::stdout_line(&line) {
                return Err(out::after_stdout_error(&e));
            }
            return Ok(true);
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    out::stderr_line(&format!(
        "daemon restart --wait: timed out after ~60s waiting for {base} to report phase=ready"
    ));
    Ok(false)
}

/// `--wait` after a restart the daemon took: exit 0, or 1 when it didn't
/// come back.
fn then_wait(client: &mut Client, wait: bool, base: &str, token: Option<&str>) -> Exit {
    if !wait {
        return Exit::Ok;
    }
    match wait_ready(client, base, token) {
        Ok(true) => Exit::Ok,
        Ok(false) => Exit::Error,
        Err(e) => e,
    }
}

/// The verb, once the gates let it through. `client` talks to the daemon
/// at the settings' port.
pub fn run(s: &Settings, client: &mut Client, v: Life) -> Exit {
    match v {
        Life::Usage => {
            for line in usage() {
                out::stderr_line(&line);
            }
            Exit::Error
        }
        Life::Start => start(),
        Life::Stop { keep: true } => stop_keeping_agents(s),
        Life::Stop { keep: false } => stop(s, client),
        Life::StopUsage => {
            out::stderr_line(&format!(
                "{{\"error\":\"usage\",\"hint\":\"{CLI} daemon stop [--keep-agents]\"}}"
            ));
            Exit::Usage
        }
        Life::Restart(r) => restart(s, client, r),
        Life::RestartUnknown(w) => {
            out::stderr_line(&format!("daemon restart: unknown argument: {w}"));
            Exit::Usage
        }
        Life::Survive(sv) => survive(client, sv),
        Life::Log { tail, follow } => log(s, &tail, follow),
        Life::Uninstall { yes } => uninstall(s, yes),
    }
}

/// The router's usage for a word it has no arm for.
pub fn usage() -> Vec<String> {
    vec![
        format!("Usage: {CLI} daemon <status|start|stop|restart|survive|log|install|uninstall>"),
        String::new(),
        "  survive [on|off|status]".into(),
        format!("    On by default: agents keep running through {PRODUCT} updates, restarts and crashes"),
        "    (the daemon's terminal.ptyd setting; same as Settings \u{2192} General \u{2192} Server).".into(),
        String::new(),
        "  install [--version <x.y.z>] [--bin-dir <dir>] [--manifest-url <url>]".into(),
        "          [--no-service] [--dry-run]".into(),
        "    Install the STANDALONE (headless, no-GUI) daemon binary from a".into(),
        "    signed GitHub release. Minisign-verifies the binary against the".into(),
        "    embedded updater pubkey, installs to ~/.local/bin by default, and".into(),
        "    writes a systemd (Linux) / launchd (macOS) supervisor unit so a".into(),
        "    crash respawns it. --dry-run prints the plan without downloading.".into(),
        String::new(),
        "  stop".into(),
        "    Quit the daemon. This ENDS its agent sessions. macOS: launchctl unload;".into(),
        "    Linux: systemctl [--user] stop <unit>; Windows: POST /cli/daemon/stop.".into(),
        "  stop --keep-agents".into(),
        "    Linux under systemd: a maintenance stop. The daemon stops, its agents keep".into(),
        format!("    running and the next {CLI} daemon start takes them back (as a restart does)."),
        format!("    Until then their {CLI} commands fail. A system unit: run the sudo systemctl"),
        "    stop it prints within 2 minutes.".into(),
        String::new(),
        "  restart [--sessions] [--host <base-url>] [--token-stdin | --token <token>] [--wait]".into(),
        "    Default: restart the LOCAL daemon (macOS launchctl kickstart, Linux".into(),
        "    systemctl restart). Agent sessions hosted by k2-ptyd keep running.".into(),
        "    --sessions: end the agent sessions too (they start fresh).".into(),
        "    --host:  restart a REMOTE daemon with no GUI by POSTing".into(),
        format!("             <base-url>/cli/daemon/restart (e.g. --host https://<sub>{ZONE_SUFFIX})."),
        "             Authorizes with the owner token OR an Owner/Admin connect-user".into(),
        "             session token; the token defaults to ~/.k2/daemon.token.".into(),
        "    --token-stdin: read the token from stdin. A token on the command line".into(),
        "             (--token <token>) is visible to other users on this machine.".into(),
        "    --wait:  after the restart is accepted, poll <base>/boot-status every".into(),
        "             ~2s up to ~60s until phase=ready, then print the version it".into(),
        "             came back on. Timeout \u{2192} message + non-zero exit.".into(),
    ]
}

/// `cmd_daemon_start`.
fn start() -> Exit {
    let (_, plist) = launchd();
    if !Path::new(&plist).is_file() {
        out::stderr_line(&format!("Daemon plist not installed at {plist}"));
        out::stderr_line(&format!(
            "Open {PRODUCT} \u{2192} Settings \u{2192} General \u{2192} Server \u{2192} {PRODUCT} server \u{2192} Install."
        ));
        return Exit::Error;
    }
    let err = capture_both("launchctl", &["load", "-w", &plist]);
    let helper = format!("{}/Library/LaunchAgents/dev.k2.menubar.plist", user_home());
    if Path::new(&helper).is_file() {
        let _ = Command::new("launchctl")
            .args(["load", "-w", helper.as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    if err.contains("service already loaded") || err.contains("Load failed: 37") {
        say!("Daemon already running.");
        return Exit::Ok;
    }
    if !err.is_empty() {
        out::stderr_line(&format!("launchctl load: {err}"));
    }
    say!("Daemon started.");
    Exit::Ok
}

/// `grep -qi "Could not find\|No such"` (and `"Could not find"` alone).
fn says_missing(err: &str, no_such_too: bool) -> bool {
    let low = err.to_ascii_lowercase();
    low.contains("could not find") || (no_such_too && low.contains("no such"))
}

/// `_daemon_stop_keeping_agents`.
fn stop_keeping_agents(s: &Settings) -> Exit {
    if os() != Os::Linux {
        out::stderr_line(&format!(
            "{{\"error\":\"usage\",\"hint\":\"--keep-agents is for Linux under systemd; on this machine {CLI} daemon restart keeps agents\"}}"
        ));
        return Exit::Usage;
    }
    let Some((scope, unit)) = linux_unit(&s.home) else {
        out::stderr_line(&format!(
            "{{\"error\":\"usage\",\"hint\":\"--keep-agents needs the daemon's systemd unit (none found); {CLI} daemon restart keeps agents\"}}"
        ));
        return Exit::Usage;
    };
    if let Some((run, want)) = slice_pending(&scope, &unit) {
        out::stderr_line(&format!(
            "{{\"error\":\"slice_pending\",\"hint\":\"the daemon runs in {run} but its unit says {want}: a stop that keeps agents would leave its next start failing. Restart instead ({CLI} daemon restart keeps agents), and reboot the box to apply the new slice\"}}"
        ));
        return Exit::Usage;
    }
    remove_intent(&s.home, "stop-intent");
    if !write_intent(&s.home, "keep-intent") {
        out::stderr_line(&format!(
            "daemon stop: could not write {}/run/keep-intent.json",
            s.home.display()
        ));
        return Exit::Error;
    }
    if systemctl_shown(&scope, &["stop", &unit]) {
        say!(
            "Daemon stopped ({unit}). Its agents keep running; they can't reach {PRODUCT} until it starts again: {CLI} daemon start."
        );
        return Exit::Ok;
    }
    if scope == "system" {
        out::stderr_line(&format!(
            "Stopping a system unit needs root. Within 2 minutes: sudo systemctl stop {unit} (agents are kept)."
        ));
    } else {
        remove_intent(&s.home, "keep-intent");
    }
    Exit::Error
}

/// `cmd_daemon_stop` with no `--keep-agents`.
fn stop(s: &Settings, client: &mut Client) -> Exit {
    match os() {
        Os::Linux => {
            let su = linux_unit(&s.home);
            if let Some((scope, unit)) = &su {
                if let Some((run, want)) = slice_pending(scope, unit) {
                    out::stderr_line(&format!(
                        "Note: the daemon runs in {run} but its unit says {want}. While processes from this run live (frpc, published services), {CLI} daemon start fails; reboot the box to start it in the new slice."
                    ));
                }
            }
            let _ = write_intent(&s.home, "stop-intent");
            let Some((scope, unit)) = su else {
                if local_post(s, client, "/cli/daemon/stop", "\"stopping\":true") {
                    say!("Daemon stopped (its agents were ended).");
                    return Exit::Ok;
                }
                remove_intent(&s.home, "stop-intent");
                say!(
                    "Daemon was not running (no systemd unit, and nothing answered at {}).",
                    base_url(s)
                );
                return Exit::Ok;
            };
            if systemctl_shown(&scope, &["stop", &unit]) {
                say!("Daemon stopped ({unit}; its agents were ended).");
                return Exit::Ok;
            }
            remove_intent(&s.home, "stop-intent");
            if scope == "system" {
                out::stderr_line(&format!(
                    "Stopping a system unit needs root: sudo systemctl stop {unit}"
                ));
            }
            return Exit::Error;
        }
        Os::Windows => {
            if local_post(s, client, "/cli/daemon/stop", "\"stopping\":true") {
                say!("Daemon stopped (its agents were ended).");
                return Exit::Ok;
            }
            out::stderr_line(&format!(
                "daemon stop: the daemon did not answer at {}",
                base_url(s)
            ));
            return Exit::Error;
        }
        Os::MacOs | Os::Other => {}
    }
    let (_, plist) = launchd();
    if !Path::new(&plist).is_file() {
        say!("Daemon plist not installed; nothing to stop.");
        return Exit::Ok;
    }
    let _ = write_intent(&s.home, "stop-intent");
    let err = capture_both("launchctl", &["unload", "-w", &plist]);
    if says_missing(&err, true) {
        say!("Daemon was not running.");
        return Exit::Ok;
    }
    if !err.is_empty() {
        out::stderr_line(&format!("launchctl unload: {err}"));
    }
    say!("Daemon stopped.");
    Exit::Ok
}

/// `cmd_daemon_restart`, once its words are read.
fn restart(s: &Settings, client: &mut Client, r: Restart) -> Exit {
    let mut token = r.token.clone();
    if r.token_stdin {
        if !token.is_empty() {
            out::stderr_line("daemon restart: --token-stdin or --token, not both");
            return Exit::Usage;
        }
        let secret = prompt::read_secret_stdin("Token: ");
        token = String::from_utf8_lossy(secret.expose()).into_owned();
        if token.is_empty() {
            out::stderr_line("daemon restart: --token-stdin: no token on stdin");
            return Exit::Usage;
        }
    }
    if !r.host.is_empty() {
        return restart_remote(s, client, &r, token);
    }
    if r.sessions {
        if local_post(
            s,
            client,
            "/cli/daemon/restart?end_agents=1",
            "\"restarting\":true",
        ) {
            say!("Daemon restarting; its agent sessions were ended.");
            return then_wait(client, r.wait, &base_url(s), None);
        }
        out::stderr_line(&format!(
            "daemon restart --sessions: the daemon did not answer at {}",
            base_url(s)
        ));
        return Exit::Error;
    }
    let os = os();
    if matches!(os, Os::Linux | Os::Windows) {
        // `_daemon_os` again: the unit is looked for on Linux only.
        let su = if self::os() == Os::Linux {
            linux_unit(&s.home)
        } else {
            None
        };
        if let Some((scope, unit)) = su {
            if !systemctl_shown(&scope, &["restart", &unit]) {
                if scope == "system" {
                    out::stderr_line(&format!(
                        "Restarting a system unit needs root: sudo systemctl restart {unit}"
                    ));
                }
                return Exit::Error;
            }
            say!("Daemon restarted ({unit}).");
        } else if local_post(s, client, "/cli/daemon/restart", "\"restarting\":true") {
            say!("Daemon restarting.");
        } else {
            out::stderr_line(&format!(
                "daemon restart: no systemd unit found and the daemon did not answer at {}",
                base_url(s)
            ));
            return Exit::Error;
        }
        return then_wait(client, r.wait, &base_url(s), None);
    }
    let (label, _) = launchd();
    let (uid, _) = capture("id", &["-u"]);
    let err = capture_both(
        "launchctl",
        &["kickstart", "-k", &format!("gui/{uid}/{label}")],
    );
    if says_missing(&err, false) {
        return start();
    }
    if !err.is_empty() {
        out::stderr_line(&format!("launchctl kickstart: {err}"));
    }
    say!("Daemon restarted.");
    then_wait(client, r.wait, &base_url(s), None)
}

/// `cmd_daemon_restart --host <url>`.
fn restart_remote(s: &Settings, client: &mut Client, r: &Restart, mut token: String) -> Exit {
    let token_file = s.home.join("daemon.token");
    if token.is_empty() && super::token_present(&s.home) {
        token = token_in(&token_file);
    }
    if token.is_empty() {
        out::stderr_line(&format!(
            "daemon restart --host: no owner token (pass --token-stdin or populate {})",
            token_file.display()
        ));
        return Exit::Error;
    }
    let base = r.host.strip_suffix('/').unwrap_or(&r.host).to_string();
    let Some(server) = remote_base(&r.host) else {
        // `parse` handed these to the shell.
        return Exit::Error;
    };
    client.host = server.host.clone();
    client.port = server.port;
    client.tls = server.tls;
    client.host_header = Some(server.host_header.clone());
    client.port_file = None;
    let path = if r.sessions {
        "/cli/daemon/restart?end_agents=1"
    } else {
        "/cli/daemon/restart"
    };
    let how = OwnCurl {
        post: true,
        credential: Some(&token),
        max_time: Duration::from_secs(30),
        note: false,
    };
    let body = client
        .own_curl_bare(path, how, Duration::from_secs(5))
        .map(|a| a.body)
        .unwrap_or_default();
    let resp = String::from_utf8_lossy(&held(&body)).into_owned();
    if resp.is_empty() {
        out::stderr_line(&format!(
            "daemon restart --host: no response from {base} (unreachable, or the daemon exited before flushing)"
        ));
        return Exit::Error;
    }
    if resp.contains("\"restarting\":true") {
        say!("Remote daemon at {base} is restarting.");
        say!("{resp}");
        return then_wait(client, r.wait, &base, Some(token.as_str()));
    }
    if resp.contains("\"error\"") || resp.contains("403") || resp.contains("405") {
        out::stderr_line(&format!(
            "daemon restart --host: rejected by {base}: {resp}"
        ));
        return Exit::Error;
    }
    out::stderr_line(&format!(
        "daemon restart --host: unexpected response from {base}: {resp}"
    ));
    Exit::Error
}

/// The Python's line numbers in `cmd_daemon_survive` (`cli/src/daemon.sh`).
pub mod line {
    /// `on|off`'s `python3 -c '…'`.
    pub mod switch {
        pub const TERMINAL: u32 = 9;
    }
    /// `status`'s `python3 -c '…'`.
    pub mod status {
        pub const SETTING: u32 = 8;
        pub const STRIP: u32 = 9;
        pub const IN: u32 = 15;
        pub const INDEX: u32 = 16;
    }
}

/// `cmd_daemon_survive`.
fn survive(client: &mut Client, sv: Survive) -> Exit {
    match sv {
        Survive::Help => {
            say!("Usage: {CLI} daemon survive [on|off|status]");
            say!("  On by default: agents keep running through {PRODUCT} updates, restarts and");
            say!("  crashes (macOS and Linux). Quitting the daemon still ends them.");
            say!("  'off' makes new sessions stop with the daemon again.");
            Exit::Ok
        }
        Survive::Usage => {
            out::stderr_line(&format!("Usage: {CLI} daemon survive [on|off|status]"));
            Exit::Usage
        }
        Survive::Switch(on) => {
            let want = if on { "on" } else { "off" };
            let body = format!("{{\"terminal\":{{\"ptyd\":\"{want}\"}}}}");
            let mut closing = crate::prelude::Closing::default();
            let passport = client.passport().is_some();
            let resp = match client.call(
                Channel::Tcp,
                "/cli/settings/update",
                Call::PostJson(body.as_bytes()),
            ) {
                Ok(r) => {
                    closing.answer(&r, passport);
                    held(&r.body)
                }
                Err(Error::NoAnswer(n)) => {
                    out::stderr_line(&n.to_string());
                    closing.flush(client, Exit::Error);
                    return Exit::Error;
                }
                Err(_) => Vec::new(),
            };
            let exit = switch_reader(&String::from_utf8_lossy(&resp), want).print();
            closing.flush(client, exit);
            exit
        }
        Survive::Status => {
            let mut closing = crate::prelude::Closing::default();
            let passport = client.passport().is_some();
            let resp = match client.call(Channel::Tcp, "/cli/settings/get", Call::Request(&[])) {
                Ok(r) => {
                    closing.answer(&r, passport);
                    held(&r.body)
                }
                Err(Error::NoAnswer(n)) => {
                    out::stderr_line(&n.to_string());
                    closing.flush(client, Exit::Error);
                    return Exit::Error;
                }
                Err(_) => Vec::new(),
            };
            let token = client.tcp_credential();
            let how = OwnCurl {
                post: false,
                credential: token.as_deref(),
                max_time: Duration::from_secs(5),
                note: false,
            };
            let boot = client
                .own_curl_bare("/boot-status", how, Duration::from_secs(3))
                .map(|r| held(&r.body))
                .unwrap_or_default();
            let mut input = String::from_utf8_lossy(&resp).into_owned();
            input.push('\n');
            input.push_str(&String::from_utf8_lossy(&boot));
            let exit = status_reader(&input).print();
            closing.flush(client, exit);
            exit
        }
    }
}

/// `get(v, key)` on a value Python must find a dict.
fn py_get<'a>(v: &'a Value, key: &str, line: u32) -> Result<Option<&'a Value>, Raise> {
    crate::families::py_get(v, key, line)
}

/// `(d.get("terminal") or {}).get("ptyd")` at `line`.
fn terminal_ptyd(d: &Value, line: u32) -> Result<Option<Value>, Raise> {
    let t = py_get(d, "terminal", line)?.filter(|v| py::truthy(v));
    match t {
        None => Ok(None),
        Some(t) => Ok(py_get(t, "ptyd", line)?.cloned()),
    }
}

/// `on|off`'s Python on the answer (`SV_WANT` is `want`).
pub fn switch_reader(raw: &str, want: &str) -> Shown {
    let text = if raw.is_empty() { "null" } else { raw };
    let Ok(d) = loads(text) else {
        return Shown::err(
            format!("Error: the {PRODUCT} daemon gave no usable answer\n"),
            Exit::Error,
        );
    };
    let err = match &d {
        Value::Object(_) => py_get(&d, "error", 7).ok().flatten().cloned(),
        _ => None,
    };
    if !matches!(d, Value::Object(_)) {
        return Shown::err(format!("Error: {}\n", py::str_of(&d)), Exit::Refused);
    }
    if let Some(e) = err.filter(py::truthy) {
        return Shown::err(format!("Error: {}\n", py::str_of(&e)), Exit::Refused);
    }
    let got = match terminal_ptyd(&d, line::switch::TERMINAL) {
        Ok(v) => v.filter(py::truthy),
        Err(r) => return raised(String::new(), String::new(), &r),
    };
    let got_text = got.as_ref().map_or("off".to_string(), py::str_of);
    let same =
        matches!(&got, Some(Value::Str(_)) if got_text == want) || (got.is_none() && want == "off");
    if !same {
        return Shown::err(
            format!("Error: the daemon kept terminal.ptyd={got_text}\n"),
            Exit::Error,
        );
    }
    if want == "on" {
        Shown::out(
            "Agents keep running through updates and restarts: on.\nNew agent sessions run under k2-ptyd; sessions already running stop at the next restart.\n",
        )
    } else {
        Shown::out(
            "Agents keep running through updates and restarts: off. New sessions stop with the daemon.\n",
        )
    }
}

/// `status`'s Python on `"$resp\n$boot"`.
pub fn status_reader(input: &str) -> Shown {
    let (first, second) = match input.split_once('\n') {
        Some((a, b)) => (a, Some(b)),
        None => (input, None),
    };
    let Ok(s) = loads(first) else {
        return Shown::err(
            format!("Error: the {PRODUCT} daemon gave no usable answer\n"),
            Exit::Error,
        );
    };
    let mut stdout = String::new();
    let setting = match terminal_ptyd(&s, line::status::SETTING) {
        Ok(v) => v.filter(py::truthy),
        Err(r) => return raised(stdout, String::new(), &r),
    };
    let setting = match setting {
        None => String::new(),
        Some(v @ Value::Str(_)) => py::str_of(&v),
        Some(other) => {
            return raised(
                stdout,
                String::new(),
                &Raise::no_attribute(line::status::STRIP, &other, "strip"),
            )
        }
    };
    let effective = if setting
        .trim_matches(crate::families::py_space)
        .to_lowercase()
        == "off"
    {
        "off"
    } else {
        "on"
    };
    stdout.push_str(&format!(
        "Setting:  terminal.ptyd = {} ({effective})\n",
        if setting.is_empty() {
            "unset"
        } else {
            &setting
        }
    ));
    let b = match second {
        Some(t) if !t.trim_matches(crate::families::py_space).is_empty() => {
            loads(t).unwrap_or(Value::Object(Vec::new()))
        }
        _ => Value::Object(Vec::new()),
    };
    let has = match crate::families::skin_token::py::py_in("ptyd", &b, line::status::IN) {
        Ok(h) => h,
        Err(r) => return raised(stdout, String::new(), &r),
    };
    if has {
        let ptyd = match &b {
            Value::Object(_) => py_get(&b, "ptyd", line::status::INDEX)
                .ok()
                .flatten()
                .cloned(),
            Value::Array(_) => {
                return raised(
                    stdout,
                    String::new(),
                    &Raise::at(
                        line::status::INDEX,
                        "TypeError: list indices must be integers or slices, not str",
                    ),
                )
            }
            _ => {
                return raised(
                    stdout,
                    String::new(),
                    &Raise::at(
                        line::status::INDEX,
                        "TypeError: string indices must be integers, not 'str'",
                    ),
                )
            }
        };
        let ptyd = ptyd.unwrap_or(Value::Null);
        stdout.push_str(&format!("Backend:  {}\n", py::str_of(&ptyd)));
        let adopted = py_get(&b, "adopted", line::status::INDEX + 1)
            .ok()
            .flatten()
            .cloned()
            .unwrap_or(Value::Int("0".into()));
        stdout.push_str(&format!(
            "Adopted at the last boot: {}\n",
            py::str_of(&adopted)
        ));
    }
    Shown::out(stdout)
}

/// `cmd_daemon_log`.
fn log(s: &Settings, tail: &str, follow: bool) -> Exit {
    let path = s.home.join("daemon.stdout.log");
    let file = path.to_string_lossy().into_owned();
    if !path.is_file() {
        say!("No daemon log yet at {file}");
        return Exit::Ok;
    }
    let mut args = vec!["-n", tail];
    if follow {
        args.push("-f");
    }
    args.push(&file);
    match Command::new("tail").args(&args).status() {
        Ok(st) => match st.code() {
            Some(0) => Exit::Ok,
            Some(c) => Exit::from_curl(c),
            None => Exit::Error,
        },
        Err(_) => Exit::from_curl(127),
    }
}

/// `cmd_daemon_uninstall`.
fn uninstall(s: &Settings, yes: bool) -> Exit {
    if !yes {
        say!("This will remove the {PRODUCT} daemon launch agent and stop the daemon.");
        say!("Your workspace data (~/.k2 database + agent files) is preserved.");
        if let Err(e) = out::stdout("Proceed? [y/N] ") {
            return out::after_stdout_error(&e);
        }
        // `read -r answer` under `set -e`: an input that ends before a
        // newline ends the command, exit 1.
        let Some(line) = prompt::read_line_or_end(Echo::Shown) else {
            return Exit::Error;
        };
        let answer = String::from_utf8_lossy(line.expose()).into_owned();
        let answer = answer.trim_matches(|c| c == ' ' || c == '\t');
        if !matches!(answer, "y" | "Y" | "yes" | "YES") {
            say!("Cancelled.");
            return Exit::Error;
        }
    }
    let (_, plist) = launchd();
    if Path::new(&plist).is_file() {
        let _ = Command::new("launchctl")
            .args(["unload", "-w", plist.as_str()])
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_file(&plist);
    }
    let _ = std::fs::remove_file(s.home.join("daemon.port"));
    let _ = std::fs::remove_file(s.home.join("daemon.token"));
    say!("Daemon uninstalled.");
    Exit::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_read_as_the_shell_reads_them() {
        assert_eq!(parse("install", &[]), None);
        assert_eq!(parse("--json", &[]), Some(Life::Usage));
        assert_eq!(
            parse("stop", &["--keep-agents"]),
            Some(Life::Stop { keep: true })
        );
        assert_eq!(parse("stop", &["x"]), Some(Life::StopUsage));
        assert_eq!(parse("log", &["--tail"]), None);
        assert_eq!(
            parse("logs", &["-n", "5", "-f"]),
            Some(Life::Log {
                tail: "5".into(),
                follow: true
            })
        );
        assert_eq!(
            parse("uninstall", &["x", "--yes"]),
            Some(Life::Uninstall { yes: false })
        );
        assert_eq!(parse("survive", &[]), Some(Life::Survive(Survive::Status)));
        assert_eq!(parse("restart", &["--host"]), None);
        assert_eq!(parse("restart", &["--host", "k2.example"]), None);
        assert_eq!(
            parse("restart", &["--wait", "nope", "--host"]),
            Some(Life::RestartUnknown("nope".into()))
        );
        let Some(Life::Restart(r)) = parse(
            "restart",
            &["--host=http://127.0.0.1:9/", "--token-stdin", "--sessions"],
        ) else {
            panic!("a restart");
        };
        assert_eq!(r.host, "http://127.0.0.1:9/");
        assert!(r.token_stdin && r.sessions && !r.wait);
    }

    #[test]
    fn the_version_is_the_first_one_grep_finds() {
        assert_eq!(
            ready_version("{\"phase\":\"ready\",\"version\" :  \"0.46.17\"}"),
            Some("0.46.17".into())
        );
        assert_eq!(
            ready_version("\"version\":1,\n\"version\":\"a\",\"version\":\"b\""),
            Some("a".into())
        );
        assert_eq!(ready_version("\"version\":\"open"), None);
    }

    #[test]
    fn the_survive_readers_print_what_the_python_prints() {
        let on = switch_reader(r#"{"terminal":{"ptyd":"on"}}"#, "on");
        assert_eq!(on.exit, Exit::Ok);
        assert!(on.stdout.starts_with("Agents keep running"));
        let kept = switch_reader(r#"{"terminal":{"ptyd":1}}"#, "on");
        assert_eq!(kept.stderr, "Error: the daemon kept terminal.ptyd=1\n");
        assert_eq!(switch_reader("{}", "off").exit, Exit::Ok);
        assert_eq!(switch_reader("[1]", "on").exit, Exit::Refused);
        assert_eq!(switch_reader("nope", "on").exit, Exit::Error);
        let s =
            status_reader("{\"terminal\":{\"ptyd\":\" OFF \"}}\n{\"ptyd\":\"on\",\"adopted\":2}");
        assert_eq!(
            s.stdout,
            "Setting:  terminal.ptyd =  OFF  (off)\nBackend:  on\nAdopted at the last boot: 2\n"
        );
        let unset = status_reader("{}\n");
        assert_eq!(unset.stdout, "Setting:  terminal.ptyd = unset (on)\n");
    }
}
