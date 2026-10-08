//! Keep each K2-launched Codex session on its own app-server.
//!
//! Codex 0.155+ runs ONE shared, detached app-server per `CODEX_HOME`
//! (`codex app-server --listen unix://`, parent pid 1) and every
//! interactive Codex TUI attaches to it. Tool commands run under that
//! shared server, so they inherit the environment of whichever session
//! started it first — that session's `K2_HOOK_SOCK` / `K2_HOOK_TOKEN`.
//! From any other K2 tab, `k2 thread` / `k2 msg` then carry a dead socket
//! and a stale token (401), or worse, a still-valid token that belongs to
//! a different session (and maybe a different workspace).
//!
//! Codex ships the switch: `--no-daemon` ("Run without the shared
//! background server, even if it is already running"), a shared
//! interactive flag accepted at the root and on `resume` / `fork`; K2 puts
//! it at the root like the other interactive flags it passes. With it the TUI keeps its app-server
//! in-process, so tool commands are children of THIS PTY and inherit
//! THIS session's env.
//!
//! Older Codex (≤ 0.154) rejects the unknown flag and would not start, so
//! the flag is only added when the resolved binary's `--help` lists it.
//! The probe result is cached per binary (path + size + mtime + ctime,
//! one-hour TTL), so a spawn pays for it once per Codex install.
//!
//! Called from [`super::daemon_pty::DaemonPtySession::spawn`], the one
//! choke point every PTY launch crosses (pinned chat, extra tabs,
//! sidecars, heartbeats, host sessions, remote sessions). Only the exec
//! argv changes; durable identity args stored on the session stay clean.
//!
//! Escape hatch: `K2_CODEX_NO_DAEMON=0` in the daemon's env turns this off;
//! `K2_CODEX_NO_DAEMON=1` adds the flag without probing.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// The Codex flag that skips the shared background app-server.
pub const NO_DAEMON_FLAG: &str = "--no-daemon";

/// Daemon env override: `0`/`off` disables, `1`/`on` forces (no probe).
pub const OVERRIDE_ENV: &str = "K2_CODEX_NO_DAEMON";

/// How long a probe answer is trusted before re-probing the same binary.
const PROBE_TTL: Duration = Duration::from_secs(60 * 60);

/// Hard cap on one `codex --help` probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// True when `program` is the Codex CLI (by basename; `.exe`/`.cmd`
/// suffixes allowed for Windows npm layouts).
pub fn is_codex_program(program: &str) -> bool {
    let Some(name) = Path::new(program.trim()).file_name() else {
        return false;
    };
    let name = name.to_string_lossy().to_ascii_lowercase();
    matches!(
        name.as_str(),
        "codex" | "codex.exe" | "codex.cmd" | "codex.bat"
    )
}

/// Root flags that take a separate value (`-m gpt-5`), so the value is
/// never mistaken for a subcommand. `--flag=value` forms need no entry.
const VALUE_FLAGS: &[&str] = &[
    "-c",
    "--config",
    "--enable",
    "--disable",
    "--remote",
    "--remote-auth-token-env",
    "-i",
    "--image",
    "-m",
    "--model",
    "--local-provider",
    "-p",
    "--profile",
    "-s",
    "--sandbox",
    "-a",
    "--ask-for-approval",
    "-C",
    "--cd",
    "--add-dir",
    "--code-mode-host",
];

/// Interactive subcommands that take `--no-daemon` themselves.
const TUI_SUBCOMMANDS: &[&str] = &["resume", "fork"];

/// Every other Codex subcommand. These are not the interactive TUI
/// (`exec`, `login`, `app-server`, …) and get no flag. `agents` is the
/// shared-daemon browser by definition.
const OTHER_SUBCOMMANDS: &[&str] = &[
    "agents",
    "exec",
    "e",
    "review",
    "login",
    "logout",
    "mcp",
    "plugin",
    "app-server",
    "remote-control",
    "app",
    "completion",
    "update",
    "doctor",
    "sandbox",
    "debug",
    "apply",
    "a",
    "queue",
    "archive",
    "delete",
    "migrate-rollouts",
    "unarchive",
    "cloud",
    "exec-server",
    "features",
    "help",
    "mcp-server",
    "responses-api-proxy",
    "stdio-to-uds",
];

/// Where `--no-daemon` goes in a Codex argv, or `None` when the argv is
/// not an interactive TUI launch or already carries the flag.
///
/// Always index 0 (a root flag) for the TUI: bare `codex [flags] [prompt]`
/// and `codex [flags] resume|fork …`. `--no-daemon` is one of the shared
/// interactive flags (it is listed in both `codex --help` and `codex resume
/// --help`, like `--dangerously-bypass-approvals-and-sandbox`), and K2's
/// resume launches already rely on root-level interactive flags being
/// honored (`codex --yolo resume <id>`). Root placement also keeps
/// `resume <id>` adjacent. Any other subcommand → `None`.
pub fn no_daemon_insert_at(args: &[String]) -> Option<usize> {
    if args.iter().any(|a| a == NO_DAEMON_FLAG) {
        return None;
    }
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            // Everything after is a prompt — no subcommand.
            return Some(0);
        }
        if a.starts_with('-') {
            if !a.contains('=') && VALUE_FLAGS.contains(&a) {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        // First positional: a subcommand or the prompt.
        if TUI_SUBCOMMANDS.contains(&a) {
            return Some(0);
        }
        if OTHER_SUBCOMMANDS.contains(&a) {
            return None;
        }
        return Some(0);
    }
    Some(0)
}

/// True when a `codex --help` text lists the `--no-daemon` flag.
pub fn help_lists_no_daemon(help: &str) -> bool {
    help.lines().any(|l| {
        let t = l.trim_start();
        t == NO_DAEMON_FLAG || t.starts_with("--no-daemon ") || t.starts_with("--no-daemon\t")
    })
}

/// What the daemon env asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Probe the binary; add the flag when its help lists it (default).
    Probe,
    /// Always add the flag (`K2_CODEX_NO_DAEMON=1`).
    Force,
    /// Never add the flag (`K2_CODEX_NO_DAEMON=0`).
    Off,
}

impl Policy {
    pub fn from_value(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()) {
            Some(v) if matches!(v.as_str(), "0" | "off" | "false" | "no") => Policy::Off,
            Some(v) if matches!(v.as_str(), "1" | "on" | "true" | "yes" | "force") => Policy::Force,
            _ => Policy::Probe,
        }
    }

    pub fn from_process() -> Self {
        Self::from_value(std::env::var(OVERRIDE_ENV).ok().as_deref())
    }
}

/// What [`apply`] did (for logs and tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Not a Codex program.
    NotCodex,
    /// Codex, but not an interactive TUI launch (or the flag is present).
    NotTui,
    /// `K2_CODEX_NO_DAEMON=0`.
    Disabled,
    /// The binary does not list `--no-daemon` (older Codex) or could not
    /// be probed; argv unchanged.
    Unsupported(String),
    /// `--no-daemon` inserted at this index.
    Added(usize),
}

/// Add `--no-daemon` to a Codex TUI exec argv when the binary supports it.
///
/// `program` is the program the PTY will exec (bare name or path);
/// `search_path` is the child's PATH (also handed to the probe so an npm
/// `#!/usr/bin/env node` launcher finds node).
pub fn apply(program: &str, args: &mut Vec<String>, search_path: &str, policy: Policy) -> Outcome {
    if !is_codex_program(program) {
        return Outcome::NotCodex;
    }
    let Some(at) = no_daemon_insert_at(args) else {
        return Outcome::NotTui;
    };
    match policy {
        Policy::Off => return Outcome::Disabled,
        Policy::Force => {}
        Policy::Probe => {
            if let Err(why) = binary_supports_no_daemon(program, search_path) {
                return Outcome::Unsupported(why);
            }
        }
    }
    args.insert(at, NO_DAEMON_FLAG.to_string());
    Outcome::Added(at)
}

/// Identity of an installed binary for the probe cache. A reinstall in
/// place (npm normalizes mtimes, so mtime alone repeats across versions)
/// still changes the inode change time.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    mtime: Option<SystemTime>,
    ctime: i64,
    ctime_nsec: i64,
}

fn fingerprint(p: &Path) -> Option<Fingerprint> {
    let md = std::fs::metadata(p).ok()?;
    #[cfg(unix)]
    let (ctime, ctime_nsec) = {
        use std::os::unix::fs::MetadataExt;
        (md.ctime(), md.ctime_nsec())
    };
    #[cfg(not(unix))]
    let (ctime, ctime_nsec) = (0i64, 0i64);
    Some(Fingerprint {
        len: md.len(),
        mtime: md.modified().ok(),
        ctime,
        ctime_nsec,
    })
}

struct CacheEntry {
    fp: Fingerprint,
    supported: bool,
    at: Instant,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, CacheEntry>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<PathBuf, CacheEntry>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `Ok(())` when the Codex binary `program` resolves to on `search_path`
/// lists `--no-daemon` in its `--help`. Cached per binary.
pub fn binary_supports_no_daemon(program: &str, search_path: &str) -> Result<(), String> {
    let Some(found) = crate::terminal::agent_spawn_guard::locate_on_path(program, search_path)
    else {
        return Err(format!("{program} not found on the spawn PATH"));
    };
    let key = found.canonicalize().unwrap_or_else(|_| found.clone());
    let fp = fingerprint(&key).ok_or_else(|| format!("cannot stat {}", key.display()))?;
    {
        let g = cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = g.get(&key) {
            if e.fp == fp && e.at.elapsed() < PROBE_TTL {
                return if e.supported {
                    Ok(())
                } else {
                    Err(format!("{} --help does not list {NO_DAEMON_FLAG}", key.display()))
                };
            }
        }
    }
    let help = probe_help(&found, search_path)?;
    let supported = help_lists_no_daemon(&help);
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert(
        key.clone(),
        CacheEntry {
            fp,
            supported,
            at: Instant::now(),
        },
    );
    if supported {
        Ok(())
    } else {
        Err(format!("{} --help does not list {NO_DAEMON_FLAG}", key.display()))
    }
}

/// Run `<binary> --help` (stdin closed, `PROBE_TIMEOUT` cap) and return
/// stdout. Never a TUI: `--help` prints and exits.
fn probe_help(binary: &Path, search_path: &str) -> Result<String, String> {
    let program = binary.to_string_lossy().into_owned();
    let (prog, args) =
        crate::terminal::win_cmd::resolve_spawn(&program, &["--help".to_string()], search_path);
    let mut cmd = Command::new(&prog);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if !search_path.is_empty() {
        cmd.env("PATH", search_path);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("probe {} --help: {e}", binary.display()))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() >= PROBE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("probe {} --help timed out", binary.display()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(format!("probe {} --help: {e}", binary.display())),
        }
    }
    reader
        .join()
        .map_err(|_| format!("probe {} --help: reader panicked", binary.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn scratch(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "k2-codex-nd-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    fn write_exec(path: &Path, body: &str) {
        crate::test_env::write_executable(path, body);
    }

    /// Fake Codex whose `--help` lists the flag (like 0.155+); any other
    /// call appends its argv to `calls.log` next to it.
    #[cfg(unix)]
    fn new_codex(dir: &Path) {
        write_exec(
            &dir.join("codex"),
            "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then\n  printf 'Usage: codex\\n      --no-alt-screen\\n          Disable alternate screen mode\\n      --no-daemon\\n          Run without the shared background server, even if it is already running\\n'\n  exit 0\nfi\necho \"$@\" >> \"$(dirname \"$0\")/calls.log\"\n",
        );
    }

    /// Fake Codex 0.154: no `--no-daemon` in help.
    #[cfg(unix)]
    fn old_codex(dir: &Path) {
        write_exec(
            &dir.join("codex"),
            "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then\n  printf 'Usage: codex\\n      --no-alt-screen\\n          Disable alternate screen mode\\n'\n  exit 0\nfi\nexit 2\n",
        );
    }

    #[test]
    fn codex_program_by_basename() {
        assert!(is_codex_program("codex"));
        assert!(is_codex_program("/opt/homebrew/bin/codex"));
        assert!(is_codex_program("codex.exe"));
        assert!(!is_codex_program("claude"));
        assert!(!is_codex_program("codex-helper"));
        assert!(!is_codex_program(""));
    }

    #[test]
    fn insert_position_for_every_launch_shape() {
        // Fresh pinned chat / extra tab: bare TUI.
        assert_eq!(no_daemon_insert_at(&v(&[])), Some(0));
        assert_eq!(no_daemon_insert_at(&v(&["--yolo"])), Some(0));
        // Fresh with a positional prompt (sidecar brief / heartbeat).
        assert_eq!(no_daemon_insert_at(&v(&["--yolo", "--", "read BRIEF.md"])), Some(0));
        assert_eq!(no_daemon_insert_at(&v(&["do the thing"])), Some(0));
        // Resume / fork (pinned chat revival, heartbeat resume fire,
        // sidecar): still a root flag, so `resume <id>` stays adjacent.
        assert_eq!(no_daemon_insert_at(&v(&["resume", "abc"])), Some(0));
        assert_eq!(
            no_daemon_insert_at(&v(&["--yolo", "resume", "abc", "continue"])),
            Some(0)
        );
        assert_eq!(
            no_daemon_insert_at(&v(&["-c", "model=\"o3\"", "fork", "abc"])),
            Some(0)
        );
        // A value flag whose value looks like a subcommand is skipped.
        assert_eq!(no_daemon_insert_at(&v(&["-m", "exec", "resume", "abc"])), Some(0));
        assert_eq!(no_daemon_insert_at(&v(&["-m", "resume", "exec", "hi"])), None);
        assert_eq!(no_daemon_insert_at(&v(&["--model=gpt-5", "resume", "x"])), Some(0));
        // Not the TUI.
        assert_eq!(no_daemon_insert_at(&v(&["exec", "hi"])), None);
        assert_eq!(no_daemon_insert_at(&v(&["--yolo", "exec", "hi"])), None);
        assert_eq!(no_daemon_insert_at(&v(&["login", "--device-auth"])), None);
        assert_eq!(no_daemon_insert_at(&v(&["app-server", "--listen", "unix://"])), None);
        // Already present → untouched.
        assert_eq!(no_daemon_insert_at(&v(&["--no-daemon", "resume", "x"])), None);
        assert_eq!(no_daemon_insert_at(&v(&["resume", "--no-daemon", "x"])), None);
    }

    #[test]
    fn help_detection_matches_real_help_shapes() {
        let real_0161 = "      --no-alt-screen\n          Disable alternate screen mode\n\n      --no-daemon\n          Run without the shared background server, even if it is already running\n\n  -h, --help\n";
        assert!(help_lists_no_daemon(real_0161));
        let real_0154 = "      --no-alt-screen\n          Disable alternate screen mode\n\n  -h, --help\n";
        assert!(!help_lists_no_daemon(real_0154));
        // Short help (`-h`) puts the description on the same line.
        assert!(help_lists_no_daemon("      --no-daemon  Run without the shared background server\n"));
        // A mention in prose is not the flag.
        assert!(!help_lists_no_daemon("see also: use --no-daemon-ish things\n"));
    }

    #[test]
    fn policy_from_env_value() {
        assert_eq!(Policy::from_value(None), Policy::Probe);
        assert_eq!(Policy::from_value(Some("")), Policy::Probe);
        assert_eq!(Policy::from_value(Some("0")), Policy::Off);
        assert_eq!(Policy::from_value(Some("off")), Policy::Off);
        assert_eq!(Policy::from_value(Some("1")), Policy::Force);
        assert_eq!(Policy::from_value(Some("force")), Policy::Force);
    }

    #[test]
    fn non_codex_and_non_tui_argv_untouched() {
        let mut a = v(&["--resume", "x"]);
        assert_eq!(apply("claude", &mut a, "", Policy::Force), Outcome::NotCodex);
        assert_eq!(a, v(&["--resume", "x"]));
        let mut a = v(&["login"]);
        assert_eq!(apply("codex", &mut a, "", Policy::Force), Outcome::NotTui);
        assert_eq!(a, v(&["login"]));
        let mut a = v(&["--yolo"]);
        assert_eq!(apply("codex", &mut a, "", Policy::Off), Outcome::Disabled);
        assert_eq!(a, v(&["--yolo"]));
    }

    #[cfg(unix)]
    #[test]
    fn probe_adds_flag_for_new_codex_and_caches() {
        let dir = scratch("new");
        new_codex(&dir);
        let path = dir.to_string_lossy().into_owned();
        let mut a = v(&["--yolo", "resume", "sid-1"]);
        assert_eq!(apply("codex", &mut a, &path, Policy::Probe), Outcome::Added(0));
        assert_eq!(a, v(&["--no-daemon", "--yolo", "resume", "sid-1"]));
        let mut fresh = v(&["--yolo"]);
        assert_eq!(apply("codex", &mut fresh, &path, Policy::Probe), Outcome::Added(0));
        assert_eq!(fresh, v(&["--no-daemon", "--yolo"]));
        // The probe only ever ran `--help`: nothing else reached the binary.
        assert!(
            !dir.join("calls.log").exists(),
            "probe must run only `codex --help`"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn probe_leaves_old_codex_argv_unchanged() {
        let dir = scratch("old");
        old_codex(&dir);
        let path = dir.to_string_lossy().into_owned();
        let mut a = v(&["--yolo", "resume", "sid-1"]);
        let out = apply("codex", &mut a, &path, Policy::Probe);
        assert!(matches!(out, Outcome::Unsupported(_)), "got {out:?}");
        assert_eq!(a, v(&["--yolo", "resume", "sid-1"]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn reinstall_in_place_is_reprobed() {
        let dir = scratch("upgrade");
        old_codex(&dir);
        let path = dir.to_string_lossy().into_owned();
        assert!(binary_supports_no_daemon("codex", &path).is_err());
        // Upgrade in place: a new file at the same path (new inode change
        // time even when an installer normalizes mtime).
        std::thread::sleep(Duration::from_millis(20));
        std::fs::remove_file(dir.join("codex")).unwrap();
        new_codex(&dir);
        assert!(binary_supports_no_daemon("codex", &path).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_binary_is_unsupported_not_added() {
        let dir = scratch("missing");
        let path = dir.to_string_lossy().into_owned();
        let mut a = v(&[]);
        let out = apply("codex", &mut a, &path, Policy::Probe);
        assert!(matches!(out, Outcome::Unsupported(_)), "got {out:?}");
        assert!(a.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
