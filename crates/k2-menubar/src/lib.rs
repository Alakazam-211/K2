//! Status and quit logic for the macOS menu-bar helper.
//!
//! Running is a non-zero pid from `launchctl print gui/<uid>/dev.k2.daemon`
//! AND an HTTP 2xx from loopback `/boot-status`. No token. Quit unloads
//! `~/Library/LaunchAgents/dev.k2.daemon.plist` and does not exit this process.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// How often the helper re-checks launchd and `/boot-status`.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);

pub const DAEMON_LABEL: &str = "dev.k2.daemon";
pub const STATUS_RUNNING: &str = "Daemon: Running";
pub const STATUS_NOT_RUNNING: &str = "Daemon: Not running";
pub const QUIT_LABEL: &str = "Quit daemon";

/// Quit never ends the helper. The icon stays and the row flips.
pub const QUIT_EXITS_HELPER: bool = false;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonState {
    Running,
    NotRunning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootStatus {
    Http(u16),
    Unreachable,
}

impl BootStatus {
    pub fn is_2xx(self) -> bool {
        matches!(self, BootStatus::Http(code) if (200..300).contains(&code))
    }
}

pub fn status_label(state: DaemonState) -> &'static str {
    match state {
        DaemonState::Running => STATUS_RUNNING,
        DaemonState::NotRunning => STATUS_NOT_RUNNING,
    }
}

/// `launchctl print` target. Not `launchctl list` — list exits 0 when the
/// job is loaded with no pid.
pub fn launchctl_print_target(uid: u32) -> String {
    format!("gui/{uid}/{DAEMON_LABEL}")
}

pub fn launchctl_print_argv(uid: u32) -> Vec<String> {
    vec!["print".to_string(), launchctl_print_target(uid)]
}

/// First `pid = N` line. `None` when launchd did not print a pid.
/// A zero pid is returned as `Some(0)` and is not Running.
pub fn pid_from_launchctl_print(stdout: &str) -> Option<u32> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("pid") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let token = rest.trim().split_whitespace().next().unwrap_or("");
        if let Ok(pid) = token.parse::<u32>() {
            return Some(pid);
        }
    }
    None
}

pub fn is_running(print_stdout: &str, boot: BootStatus) -> bool {
    match pid_from_launchctl_print(print_stdout) {
        Some(pid) if pid != 0 => boot.is_2xx(),
        _ => false,
    }
}

pub fn classify(print_stdout: &str, boot: BootStatus) -> DaemonState {
    if is_running(print_stdout, boot) {
        DaemonState::Running
    } else {
        DaemonState::NotRunning
    }
}

pub fn read_daemon_port(contents: &str) -> Option<u16> {
    let port: u16 = contents.trim().parse().ok()?;
    if port == 0 {
        None
    } else {
        Some(port)
    }
}

pub fn boot_status_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/boot-status")
}

/// Request line for the probe. Loopback `/boot-status` only, no token header.
pub fn boot_status_request(port: u16) -> String {
    format!(
        "GET /boot-status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nAccept: */*\r\n\r\n"
    )
}

pub fn http_status_code(prefix: &[u8]) -> Option<u16> {
    let text = std::str::from_utf8(prefix).ok()?;
    let line = text.split(['\r', '\n']).next()?;
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}

pub fn fetch_boot_status(port: u16, timeout: Duration) -> BootStatus {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = match TcpStream::connect_timeout(&addr, timeout) {
        Ok(stream) => stream,
        Err(_) => return BootStatus::Unreachable,
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    if stream
        .write_all(boot_status_request(port).as_bytes())
        .is_err()
    {
        return BootStatus::Unreachable;
    }
    let mut buf = [0u8; 128];
    let mut filled = 0usize;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => {
                filled += n;
                if http_status_code(&buf[..filled]).is_some()
                    && buf[..filled].windows(2).any(|w| w == b"\r\n")
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    match http_status_code(&buf[..filled]) {
        Some(code) => BootStatus::Http(code),
        None => BootStatus::Unreachable,
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

pub fn daemon_plist_path() -> Option<PathBuf> {
    home_dir().map(|home| daemon_plist_path_under(&home))
}

pub fn daemon_plist_path_under(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{DAEMON_LABEL}.plist"))
}

pub fn unload_launchctl_args(plist: &Path) -> Vec<String> {
    vec![
        "unload".to_string(),
        "-w".to_string(),
        plist.display().to_string(),
    ]
}

/// Already stopped (`Could not find` / `No such`, or the plist is gone)
/// is success. The helper still does not exit.
pub fn unload_succeeded(exit_ok: bool, stderr: &str) -> bool {
    exit_ok || stderr.contains("Could not find") || stderr.contains("No such")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuitEffect {
    pub ok: bool,
    pub exit_helper: bool,
}

pub fn interpret_quit(
    plist_existed: bool,
    spawned: bool,
    exit_ok: bool,
    stderr: &str,
) -> QuitEffect {
    if !plist_existed {
        return QuitEffect {
            ok: true,
            exit_helper: QUIT_EXITS_HELPER,
        };
    }
    if !spawned {
        return QuitEffect {
            ok: false,
            exit_helper: QUIT_EXITS_HELPER,
        };
    }
    QuitEffect {
        ok: unload_succeeded(exit_ok, stderr),
        exit_helper: QUIT_EXITS_HELPER,
    }
}

/// Unload `dev.k2.daemon`. Missing plist and already-stopped are `Ok`.
/// Does not exit the process and does not unload `dev.k2.menubar`.
#[cfg(unix)]
pub fn quit_daemon() -> Result<(), String> {
    let Some(path) = daemon_plist_path() else {
        return Err("home directory unavailable".to_string());
    };
    quit_daemon_plist(&path)
}

#[cfg(unix)]
pub fn quit_daemon_plist(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let args = unload_launchctl_args(path);
    let output = Command::new("launchctl")
        .args(&args)
        .output()
        .map_err(|e| format!("launchctl: {e}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if unload_succeeded(output.status.success(), &stderr) {
        return Ok(());
    }
    Err(format!(
        "launchctl unload failed (exit {}): {}",
        output.status.code().unwrap_or(-1),
        stderr.trim()
    ))
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(unix)]
pub fn launchctl_print_stdout(uid: u32) -> String {
    let argv = launchctl_print_argv(uid);
    Command::new("launchctl")
        .args(&argv)
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default()
}

/// This Mac only: pid from `launchctl print` and loopback `/boot-status`.
#[cfg(unix)]
pub fn probe_live() -> DaemonState {
    let print_stdout = launchctl_print_stdout(current_uid());
    let port = home_dir()
        .and_then(|home| std::fs::read_to_string(home.join(".k2").join("daemon.port")).ok())
        .as_deref()
        .and_then(read_daemon_port);
    let boot = match port {
        Some(port) => fetch_boot_status(port, Duration::from_secs(2)),
        None => BootStatus::Unreachable,
    };
    classify(&print_stdout, boot)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRINT_RUNNING: &str = "\
gui/501/dev.k2.daemon = {\n\
\tstate = running\n\
\tpid = 86342\n\
\tlast exit code = 0\n\
}\n";

    #[test]
    fn running_requires_nonzero_pid_and_boot_status_2xx() {
        assert_eq!(
            classify(PRINT_RUNNING, BootStatus::Http(200)),
            DaemonState::Running
        );
        assert_eq!(
            classify(PRINT_RUNNING, BootStatus::Http(204)),
            DaemonState::Running
        );
        assert_eq!(
            classify(PRINT_RUNNING, BootStatus::Http(299)),
            DaemonState::Running
        );
        assert_eq!(status_label(DaemonState::Running), "Daemon: Running");
    }

    #[test]
    fn each_failure_alone_is_not_running() {
        assert_eq!(
            classify(PRINT_RUNNING, BootStatus::Http(503)),
            DaemonState::NotRunning
        );
        assert_eq!(
            classify(PRINT_RUNNING, BootStatus::Http(403)),
            DaemonState::NotRunning
        );
        assert_eq!(
            classify(PRINT_RUNNING, BootStatus::Unreachable),
            DaemonState::NotRunning
        );
        assert_eq!(
            classify("\tpid = 0\n", BootStatus::Http(200)),
            DaemonState::NotRunning
        );
        assert_eq!(
            classify("state = running\n", BootStatus::Http(200)),
            DaemonState::NotRunning
        );
        let list_only = "PID\tStatus\tLabel\n4321\t0\tdev.k2.daemon\n";
        assert_eq!(
            classify(list_only, BootStatus::Http(200)),
            DaemonState::NotRunning,
            "launchctl list success is not Running"
        );
        assert_eq!(status_label(DaemonState::NotRunning), "Daemon: Not running");
    }

    #[test]
    fn print_argv_is_not_list() {
        let argv = launchctl_print_argv(501);
        assert_eq!(
            argv,
            vec!["print".to_string(), "gui/501/dev.k2.daemon".to_string()]
        );
        assert!(!argv.iter().any(|a| a == "list"));
    }

    #[test]
    fn boot_status_request_has_no_token_and_is_not_status() {
        let req = boot_status_request(8787);
        assert!(req.starts_with("GET /boot-status HTTP/1.1\r\n"), "{req}");
        assert_eq!(boot_status_url(8787), "http://127.0.0.1:8787/boot-status");
        assert!(!req.to_ascii_lowercase().contains("token"));
        assert!(!req.contains('?'));
        assert!(!req.to_ascii_lowercase().contains("authorization"));
    }

    #[test]
    fn http_status_parses_any_2xx_line() {
        assert_eq!(http_status_code(b"HTTP/1.1 200 OK\r\n"), Some(200));
        assert_eq!(http_status_code(b"HTTP/1.0 204 \r\n"), Some(204));
        assert_eq!(http_status_code(b"HTTP/1.1 403 Forbidden\r\n"), Some(403));
        assert_eq!(http_status_code(b"not http"), None);
        assert!(BootStatus::Http(200).is_2xx());
        assert!(!BootStatus::Http(403).is_2xx());
        assert!(!BootStatus::Http(199).is_2xx());
        assert!(!BootStatus::Http(300).is_2xx());
    }

    #[test]
    fn port_file_ignores_zero_and_blank() {
        assert_eq!(read_daemon_port("8787\n"), Some(8787));
        assert_eq!(read_daemon_port("  9 "), Some(9));
        assert_eq!(read_daemon_port("0"), None);
        assert_eq!(read_daemon_port(""), None);
        assert_eq!(read_daemon_port("nope"), None);
    }

    #[test]
    fn quit_unloads_daemon_plist_and_does_not_exit() {
        let home = Path::new("/Users/example");
        let plist = daemon_plist_path_under(home);
        assert_eq!(
            plist,
            PathBuf::from("/Users/example/Library/LaunchAgents/dev.k2.daemon.plist")
        );
        assert!(!plist.ends_with("dev.k2.menubar.plist"));
        assert_eq!(
            unload_launchctl_args(&plist),
            vec![
                "unload".to_string(),
                "-w".to_string(),
                plist.display().to_string(),
            ]
        );
        assert!(!QUIT_EXITS_HELPER);
        assert_eq!(QUIT_LABEL, "Quit daemon");
    }

    #[test]
    fn already_stopped_is_success_and_helper_stays() {
        let missing = interpret_quit(false, false, false, "");
        assert!(missing.ok);
        assert!(!missing.exit_helper);

        let could_not = interpret_quit(
            true,
            true,
            false,
            "Unload failed: Could not find specified service\n",
        );
        assert!(could_not.ok);
        assert!(!could_not.exit_helper);

        let no_such = interpret_quit(true, true, false, "No such file or directory");
        assert!(no_such.ok);
        assert!(!no_such.exit_helper);

        let clean = interpret_quit(true, true, true, "");
        assert!(clean.ok);
        assert!(!clean.exit_helper);

        let real_err = interpret_quit(true, true, false, "Load failed: 5: Input/output error");
        assert!(!real_err.ok);
        assert!(!real_err.exit_helper);

        let spawn_fail = interpret_quit(true, false, false, "launchctl: no such file");
        assert!(!spawn_fail.ok);
        assert!(!spawn_fail.exit_helper);
    }

    #[test]
    fn poll_interval_does_not_hammer() {
        assert!(POLL_INTERVAL >= Duration::from_secs(5));
    }

    #[test]
    fn missing_plist_quit_does_not_spawn_launchctl() {
        let missing =
            std::env::temp_dir().join(format!("k2-menubar-missing-{}.plist", std::process::id()));
        let _ = std::fs::remove_file(&missing);
        assert!(!missing.exists());
        #[cfg(unix)]
        {
            quit_daemon_plist(&missing).expect("missing plist is success");
        }
        assert!(!missing.exists());
    }
}
