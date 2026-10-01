//! Daemon-first cron infrastructure installer.
//!
//! The pre-P5 flow required a user to visit Settings → Wake Scheduler
//! → Apply before any heartbeat would actually fire from cron. New
//! users who created a heartbeat through Settings → Heartbeats → Add
//! got a DB row and a WAKEUP.md but no `heartbeat.sh` and no launchd
//! plist — silent failure with no obvious recovery path.
//!
//! This module is the daemon's self-bootstrap. [`ensure_cron_installed`]
//! is idempotent and called from [`crate::heartbeats::k2so_heartbeat_add`]
//! after a successful row insert. Headless installs (CLI / daemon
//! without Tauri ever launched) get cron working from the first
//! `k2so heartbeat add` onward.
//!
//! Generates `~/.k2/heartbeat.sh` (the bridge that asks the daemon
//! `/cli/heartbeat/active-projects` and ticks each one) and installs
//! the launchd agent (macOS) or crontab entry (Linux). All file
//! writes are atomic; launchctl operations are best-effort
//! (failures are logged, not fatal).

use std::fs;
use std::path::{Path, PathBuf};

/// Default tick cadence in seconds. Matches the P5.7 default in
/// `src-tauri::commands::settings::default_wake_interval`. Empty
/// ticks return in microseconds (no-op when no heartbeats are due);
/// full ticks are bounded by the P5.4 spawn pool so the increase
/// over the legacy 300s is safe.
pub const DEFAULT_INTERVAL_SECS: u32 = 60;

/// Opt-out flag. When `=1`, nothing in this module writes, loads, boots
/// out or removes the OS scheduler job (the headless e2e harness and any
/// scratch-HOME daemon set it).
pub const NO_SELF_HEAL_ENV: &str = "K2_HEARTBEAT_NO_SELF_HEAL";

// ── HB6 — one transport guard ──────────────────────────────────────────
//
// 2026-09-30: a `cargo test -p k2-core` run swapped `$HOME` to a temp
// folder on one thread while another thread's `heartbeat add` reached
// `install_macos_if_missing`. It booted out this Mac's real
// `dev.k2.heartbeat` job and bootstrapped a plist from the temp folder,
// which the test then deleted. No scheduled heartbeat fired for ~8h.
//
// Every function that writes, loads, boots out or removes the OS
// scheduler job (launchd plist, crontab line, `heartbeat.sh`) calls
// [`transport_writes_allowed`] FIRST. A refusal is logged and returned
// as `Err` — never `Ok`.

/// May this process touch the OS scheduler job right now?
///
/// Refuses (in this order) when:
/// 1. `K2_HEARTBEAT_NO_SELF_HEAL=1` is set;
/// 2. `$HOME` is not the password-database home for this uid (a test,
///    a scratch-HOME daemon, a sandboxed child);
/// 3. this is a `cfg(test)` build of k2-core.
pub fn transport_writes_allowed() -> Result<(), String> {
    let flag = std::env::var(NO_SELF_HEAL_ENV).ok();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let pw_home = password_home();
    let verdict = writes_allowed_for(
        flag.as_deref(),
        home.as_deref(),
        pw_home.as_deref(),
        cfg!(test),
    );
    if let Err(e) = &verdict {
        crate::log_debug!("[heartbeat-transport] {e}");
    }
    verdict
}

/// Pure decision behind [`transport_writes_allowed`]. `pw_home = None`
/// (no password entry, or a platform without one) skips the HOME check;
/// the flag and the test check still apply.
pub(crate) fn writes_allowed_for(
    flag: Option<&str>,
    home: Option<&Path>,
    pw_home: Option<&Path>,
    under_test: bool,
) -> Result<(), String> {
    if flag.map(str::trim) == Some("1") {
        return Err(format!(
            "heartbeat transport write refused: {NO_SELF_HEAL_ENV}=1"
        ));
    }
    if let (Some(home), Some(pw_home)) = (home, pw_home) {
        if !same_dir(home, pw_home) {
            return Err(format!(
                "heartbeat transport write refused: HOME is not the user home \
                 (HOME={}, user home={})",
                home.display(),
                pw_home.display()
            ));
        }
    }
    if under_test {
        return Err(
            "heartbeat transport write refused: cfg(test) builds never touch the OS scheduler"
                .to_string(),
        );
    }
    Ok(())
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => {
            let ta = a.to_string_lossy();
            let tb = b.to_string_lossy();
            ta.trim_end_matches('/') == tb.trim_end_matches('/')
        }
    }
}

/// The home directory the password database lists for this uid.
#[cfg(unix)]
fn password_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    let uid = unsafe { libc::getuid() };
    let mut size = 4096usize;
    loop {
        let mut buf = vec![0 as libc::c_char; size];
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result)
        };
        if rc == libc::ERANGE && size < 1 << 20 {
            size *= 2;
            continue;
        }
        if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        let dir = unsafe { CStr::from_ptr(pwd.pw_dir) }
            .to_string_lossy()
            .into_owned();
        return if dir.is_empty() { None } else { Some(PathBuf::from(dir)) };
    }
}

#[cfg(not(unix))]
fn password_home() -> Option<PathBuf> {
    None
}

// ── HB7 — one command runner ───────────────────────────────────────────
//
// Every `launchctl` / `crontab` call goes through a [`CommandRunner`].
// Production uses [`SystemRunner`]. Under `cfg(test)` the system runner
// records the call and runs NOTHING, so even a test that slips past the
// guard cannot reach the real user session. Tests that need launchctl
// output hand a fake runner to the `*_with` functions.

/// Captured output of one scheduler command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `launchctl` / `crontab`. `Err` means the program could not be
/// run at all (not installed, spawn failure, or a `cfg(test)` refusal).
pub trait CommandRunner {
    fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> Result<CmdOutput, String>;
}

/// The real runner.
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    #[cfg(not(test))]
    fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> Result<CmdOutput, String> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(program);
        cmd.args(args);
        let output = if let Some(input) = stdin {
            let mut child = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("spawn {program}: {e}"))?;
            child
                .stdin
                .take()
                .ok_or_else(|| format!("{program}: no stdin"))?
                .write_all(input.as_bytes())
                .map_err(|e| format!("write {program} stdin: {e}"))?;
            child
                .wait_with_output()
                .map_err(|e| format!("wait {program}: {e}"))?
        } else {
            cmd.output().map_err(|e| format!("run {program}: {e}"))?
        };
        Ok(CmdOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    #[cfg(test)]
    fn run(&self, program: &str, args: &[&str], _stdin: Option<&str>) -> Result<CmdOutput, String> {
        test_recorder::record(program, args);
        Err(format!(
            "cfg(test): SystemRunner never runs `{program}` — use a fake runner"
        ))
    }
}

/// Calls that reached [`SystemRunner`] under `cfg(test)`, per thread.
#[cfg(test)]
pub(crate) mod test_recorder {
    use std::cell::RefCell;

    thread_local! {
        static CALLS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn record(program: &str, args: &[&str]) {
        CALLS.with(|c| c.borrow_mut().push(format!("{program} {}", args.join(" "))));
    }

    /// Drain this thread's recorded calls.
    pub(crate) fn take() -> Vec<String> {
        CALLS.with(|c| std::mem::take(&mut *c.borrow_mut()))
    }
}

/// `crontab -l`, or empty when the user has no crontab / no `crontab`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn crontab_read(runner: &dyn CommandRunner) -> String {
    runner
        .run("crontab", &["-l"], None)
        .ok()
        .and_then(|o| if o.success { Some(o.stdout) } else { None })
        .unwrap_or_default()
}

/// Replace the user's crontab with `content` (`crontab -`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn crontab_write(runner: &dyn CommandRunner, content: &str) -> Result<(), String> {
    let out = runner
        .run("crontab", &["-"], Some(content))
        .map_err(|e| format!("crontab: {e}"))?;
    if !out.success {
        return Err(format!("crontab - failed: {}", out.stderr.trim()));
    }
    Ok(())
}

fn launchd_target() -> String {
    #[cfg(unix)]
    let uid = unsafe { libc::getuid() };
    #[cfg(not(unix))]
    let uid = 0u32;
    format!("gui/{uid}/dev.k2.heartbeat")
}

/// Ensure the cron infrastructure is installed. Safe to call on
/// every heartbeat add — the underlying writes are idempotent and
/// launchctl operations are no-ops when the agent is already in the
/// requested state.
///
/// Returns `Ok(true)` if anything was installed/changed, `Ok(false)`
/// if everything was already up-to-date. Errors are surfaced for
/// logging but the caller is free to ignore them — a partially
/// installed cron is better than blocking the heartbeat add.
pub fn ensure_cron_installed() -> Result<bool, String> {
    transport_writes_allowed()?;
    let k2so_home = home_dir().join(".k2");
    fs::create_dir_all(&k2so_home).map_err(|e| format!("create ~/.k2: {e}"))?;

    let script_path = k2so_home.join("heartbeat.sh");
    let mut changed = false;

    // Write/refresh heartbeat.sh if it doesn't match the current
    // template. This catches users upgrading from a pre-P5.6
    // install whose on-disk script still references
    // ~/.k2/heartbeat-projects.txt.
    let want_script = generate_heartbeat_script();
    let current_script = fs::read_to_string(&script_path).ok();
    if current_script.as_deref() != Some(&want_script) {
        fs::write(&script_path, &want_script)
            .map_err(|e| format!("write heartbeat.sh: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("chmod heartbeat.sh: {e}"))?;
        }
        changed = true;
    }

    // Install platform scheduler if not already loaded.
    #[cfg(target_os = "macos")]
    {
        if install_macos_if_missing(&script_path, DEFAULT_INTERVAL_SECS, false)? {
            changed = true;
        }
    }
    #[cfg(target_os = "linux")]
    {
        if install_linux_if_missing(&script_path)? {
            changed = true;
        }
    }

    Ok(changed)
}

/// Is the tick transport actually installed AND armed?
///
/// macOS: the `dev.k2.heartbeat` plist exists on disk AND launchd
/// reports the agent loaded (`launchctl print gui/<uid>/…`). The
/// misfire study found this box's agent silently missing for ~3 weeks
/// while every enabled heartbeat sat dark with zero signal — the
/// plist-on-disk check alone is not enough.
///
/// Linux: the `k2so-agent-heartbeat` crontab entry exists.
///
/// Other platforms report `true` (no supported transport to verify —
/// don't raise false alarms).
pub fn transport_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        let plist_path = home_dir().join("Library/LaunchAgents/dev.k2.heartbeat.plist");
        if !plist_path.exists() {
            return false;
        }
        let uid_target = launchd_target();
        SystemRunner
            .run("launchctl", &["print", &uid_target], None)
            .map(|o| o.success)
            .unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        SystemRunner
            .run("crontab", &["-l"], None)
            .ok()
            .and_then(|o| if o.success { Some(o.stdout) } else { None })
            .map(|c| c.contains("k2so-agent-heartbeat"))
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        true
    }
}

/// Bash script written to `~/.k2/heartbeat.sh`. Asks the daemon
/// for active projects on every tick and forwards each to
/// `/cli/scheduler-tick`. P5.6 retired the `heartbeat-projects.txt`
/// dependency — the DB is the only source of truth for which
/// workspaces have heartbeats.
pub fn generate_heartbeat_script() -> String {
    let home = home_dir().to_string_lossy().to_string();

    format!(r##"#!/bin/bash
# K2SO Agent Heartbeat — DO NOT EDIT (managed by K2SO daemon)
# Asks the daemon which projects have active heartbeats, then ticks each.

PORT_FILE="{home}/.k2/heartbeat.port"
LOG_FILE="{home}/.k2/heartbeat.log"
TOKEN_FILE="{home}/.k2/heartbeat.token"

ts() {{ date '+%Y-%m-%d %H:%M:%S'; }}

urlencode() {{
    local string="$1" length="${{#1}}" i c
    local encoded=""
    for (( i = 0; i < length; i++ )); do
        c="${{string:i:1}}"
        case "$c" in
            [a-zA-Z0-9._~-]) encoded+="$c" ;;
            *) encoded+=$(printf '%%%02X' "'$c") ;;
        esac
    done
    printf '%s' "$encoded"
}}

if [ ! -f "$PORT_FILE" ]; then
    exit 0
fi
PORT=$(cat "$PORT_FILE" 2>/dev/null)
if [ -z "$PORT" ] || ! [[ "$PORT" =~ ^[0-9]+$ ]]; then
    exit 0
fi

HEALTH=$(curl -s --connect-timeout 2 "http://127.0.0.1:$PORT/health" 2>/dev/null)
if ! echo "$HEALTH" | grep -q '"ok"'; then
    exit 0
fi

TOKEN=""
if [ -f "$TOKEN_FILE" ]; then
    TOKEN=$(cat "$TOKEN_FILE" 2>/dev/null)
fi

if [ -z "$TOKEN" ]; then
    echo "$(ts) ERROR: No auth token available — skipping heartbeat" >> "$LOG_FILE"
    exit 0
fi

# Ask the daemon for the current list of projects with active heartbeats.
PROJECTS=$(curl -s --connect-timeout 2 --max-time 5 \
    "http://127.0.0.1:$PORT/cli/heartbeat/active-projects?token=$TOKEN" 2>>"$LOG_FILE")
if [ -z "$PROJECTS" ]; then
    if [ -f "$LOG_FILE" ]; then
        tail -200 "$LOG_FILE" > "$LOG_FILE.tmp" 2>/dev/null && mv -f "$LOG_FILE.tmp" "$LOG_FILE" 2>/dev/null
    fi
    exit 0
fi

while IFS= read -r project_path; do
    [ -z "$project_path" ] && continue
    ENCODED_PATH=$(urlencode "$project_path")
    RESULT=$(curl -sG "http://127.0.0.1:$PORT/cli/scheduler-tick?token=$TOKEN&project=$ENCODED_PATH" --connect-timeout 5 --max-time 30 2>>"$LOG_FILE")
    CURL_EXIT=$?
    if [ "$CURL_EXIT" -ne 0 ]; then
        echo "$(ts) ERROR curl exit=$CURL_EXIT project=$project_path" >> "$LOG_FILE"
        continue
    fi
    COUNT=$(echo "$RESULT" | grep -o '"count":[0-9]*' | grep -o '[0-9]*' | head -1 || echo 0)
    SKIPPED=$(echo "$RESULT" | grep -o '"skipped":"[^"]*"' | sed 's/"skipped":"\([^"]*\)"/\1/')
    if [ -n "$SKIPPED" ]; then
        echo "$(ts) tick project=$project_path skipped=$SKIPPED" >> "$LOG_FILE"
    elif [ -n "$COUNT" ] && [ "$COUNT" -gt 0 ] 2>/dev/null; then
        echo "$(ts) tick project=$project_path launched=$COUNT" >> "$LOG_FILE"
    else
        echo "$(ts) tick project=$project_path launched=0" >> "$LOG_FILE"
    fi
done <<< "$PROJECTS"

if [ -f "$LOG_FILE" ]; then
    tail -200 "$LOG_FILE" > "$LOG_FILE.tmp" 2>/dev/null && mv -f "$LOG_FILE.tmp" "$LOG_FILE" 2>/dev/null
fi
"##, home = home)
}

#[cfg(target_os = "macos")]
fn install_macos_if_missing(
    script_path: &Path,
    interval_seconds: u32,
    wake_system: bool,
) -> Result<bool, String> {
    let home = home_dir();
    let plist_path = home.join("Library/LaunchAgents/dev.k2.heartbeat.plist");

    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create LaunchAgents dir: {e}"))?;
    }

    // Compose the desired plist.
    let wake_key = if wake_system {
        "\n    <key>WakeSystem</key>\n    <true/>"
    } else {
        ""
    };
    let want_plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>dev.k2.heartbeat</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>{script}</string>
    </array>
    <key>StartInterval</key>
    <integer>{interval}</integer>{wake_key}
    <key>RunAtLoad</key>
    <false/>
    <key>StandardErrorPath</key>
    <string>{home}/.k2/heartbeat-stderr.log</string>
</dict>
</plist>"#,
        script = script_path.to_string_lossy(),
        interval = interval_seconds,
        wake_key = wake_key,
        home = home.to_string_lossy(),
    );

    // Compare with current plist on disk; only rewrite + reload if
    // changed. Idempotent calls are a no-op.
    let current_plist = fs::read_to_string(&plist_path).ok();
    let plist_changed = current_plist.as_deref() != Some(&want_plist);

    // Check if launchd already has the agent loaded — `launchctl
    // print` returns 0 if loaded, non-zero otherwise.
    let uid_target = launchd_target();
    let already_loaded = SystemRunner
        .run("launchctl", &["print", &uid_target], None)
        .map(|o| o.success)
        .unwrap_or(false);

    if !plist_changed && already_loaded {
        return Ok(false);
    }

    // Bootout the existing agent before rewriting (safe even if not loaded).
    if already_loaded {
        let _ = SystemRunner.run("launchctl", &["bootout", &uid_target], None);
    }

    if plist_changed {
        fs::write(&plist_path, &want_plist)
            .map_err(|e| format!("write plist: {e}"))?;
    }

    // Bootstrap into the user's GUI domain.
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let output = SystemRunner
        .run("launchctl", &["bootstrap", &domain, &plist_path.to_string_lossy()], None)
        .map_err(|e| format!("launchctl bootstrap: {e}"))?;
    if !output.success {
        return Err(format!("launchctl bootstrap failed: {}", output.stderr));
    }
    Ok(true)
}

#[cfg(target_os = "linux")]
fn install_linux_if_missing(script_path: &Path) -> Result<bool, String> {
    let marker = "# k2so-agent-heartbeat";
    let entry = format!("* * * * * {} {}", script_path.to_string_lossy(), marker);

    let existing = crontab_read(&SystemRunner);

    // Skip if our entry already present unchanged.
    if existing.lines().any(|l| l == entry) {
        return Ok(false);
    }

    let mut lines: Vec<&str> = existing
        .lines()
        .filter(|l| !l.contains("k2so-agent-heartbeat"))
        .collect();
    lines.push(&entry);
    let new_crontab = lines.join("\n") + "\n";

    crontab_write(&SystemRunner, &new_crontab)?;
    Ok(true)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn ensure_platform_installed(_script_path: &Path) -> Result<bool, String> {
    // No supported scheduler on this platform — caller already wrote
    // heartbeat.sh; user must invoke it manually.
    Ok(false)
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

// ── Phase 2 Unit 7c — explicit install/uninstall/apply (daemon-owned) ──
//
// Pre-Unit-7c the heartbeat-launchd installer lived in
// `src-tauri/src/commands/k2so_agents.rs`, called via Tauri commands
// triggered from the Settings > Heartbeats UI. With Unit 7c the daemon
// owns its own scheduler plist — K2SO Connect (remote daemon) must
// install + remove its own launchd agent without depending on a Tauri
// process on the same host. The functions below back the daemon's
// `/cli/heartbeat/{install-launchd, uninstall-launchd, apply-wake-scheduler}`
// routes.

/// Write `heartbeat.sh` to `~/.k2/` (chmod 0755). Idempotent —
/// callers can invoke before every install_launchd / install_cron pass
/// without worrying about double-write.
///
/// Returns the script path so the caller can hand it to the platform
/// installer.
pub fn write_heartbeat_script() -> Result<PathBuf, String> {
    transport_writes_allowed()?;
    let k2so_home = home_dir().join(".k2");
    fs::create_dir_all(&k2so_home).map_err(|e| format!("create ~/.k2: {e}"))?;

    // Clean up the retired heartbeat-projects.txt artifact (pre-P5.6).
    let _ = fs::remove_file(k2so_home.join("heartbeat-projects.txt"));

    let script_path = k2so_home.join("heartbeat.sh");
    let script = generate_heartbeat_script();
    fs::write(&script_path, &script)
        .map_err(|e| format!("write heartbeat.sh: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod heartbeat.sh: {e}"))?;
    }
    Ok(script_path)
}

/// Install (or reinstall) the heartbeat launchd plist with a
/// user-configurable interval + optional wake-from-sleep behavior.
///
/// - `interval_seconds` maps to `StartInterval` (60 = every minute,
///   300 = every 5 minutes, etc.).
/// - `wake_system` sets `WakeSystem: true` so launchd wakes a sleeping
///   machine — the mechanism that makes lid-closed overnight agent
///   work possible.
///
/// Idempotent: unloads any existing plist with the same label before
/// writing, then loads the new one. Safe to call repeatedly with
/// different settings.
///
/// Returns the plist path on success so callers can surface it for
/// audit / display.
#[cfg(target_os = "macos")]
pub fn install_heartbeat_launchd(
    script_path: &Path,
    interval_seconds: u32,
    wake_system: bool,
) -> Result<PathBuf, String> {
    transport_writes_allowed()?;
    let home = home_dir();
    let plist_path = home.join("Library/LaunchAgents/dev.k2.heartbeat.plist");

    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create LaunchAgents dir: {e}"))?;
    }

    if plist_path.exists() {
        let _ = SystemRunner.run("launchctl", &["unload", &plist_path.to_string_lossy()], None);
    }

    let wake_key = if wake_system {
        "\n    <key>WakeSystem</key>\n    <true/>"
    } else {
        ""
    };

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>dev.k2.heartbeat</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>{script}</string>
    </array>
    <key>StartInterval</key>
    <integer>{interval}</integer>{wake_key}
    <key>RunAtLoad</key>
    <false/>
    <key>StandardErrorPath</key>
    <string>{home}/.k2/heartbeat-stderr.log</string>
</dict>
</plist>"#,
        script = script_path.to_string_lossy(),
        interval = interval_seconds,
        wake_key = wake_key,
        home = home.to_string_lossy(),
    );

    fs::write(&plist_path, &plist).map_err(|e| format!("write plist: {e}"))?;

    let output = SystemRunner
        .run("launchctl", &["load", &plist_path.to_string_lossy()], None)
        .map_err(|e| format!("launchctl: {e}"))?;
    if !output.success {
        return Err(format!("launchctl load failed: {}", output.stderr));
    }

    Ok(plist_path)
}

#[cfg(not(target_os = "macos"))]
pub fn install_heartbeat_launchd(
    _script_path: &Path,
    _interval_seconds: u32,
    _wake_system: bool,
) -> Result<PathBuf, String> {
    Err("install_heartbeat_launchd is macOS-only".to_string())
}

/// Uninstall the heartbeat launchd plist. Idempotent — missing plist
/// is treated as success.
#[cfg(target_os = "macos")]
pub fn uninstall_heartbeat_launchd() -> Result<(), String> {
    transport_writes_allowed()?;
    let home = home_dir();
    let plist_path = home.join("Library/LaunchAgents/dev.k2.heartbeat.plist");
    if plist_path.exists() {
        let _ = SystemRunner.run("launchctl", &["unload", &plist_path.to_string_lossy()], None);
        fs::remove_file(&plist_path)
            .map_err(|e| format!("remove plist: {e}"))?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn uninstall_heartbeat_launchd() -> Result<(), String> {
    Ok(())
}

/// Install the heartbeat crontab entry (Linux). Replaces any pre-
/// existing `k2so-agent-heartbeat` entry. Idempotent.
#[cfg(target_os = "linux")]
pub fn install_heartbeat_cron(script_path: &Path) -> Result<(), String> {
    transport_writes_allowed()?;
    let marker = "# k2so-agent-heartbeat";
    let entry = format!("* * * * * {} {}", script_path.to_string_lossy(), marker);

    let existing = crontab_read(&SystemRunner);

    let mut lines: Vec<&str> = existing
        .lines()
        .filter(|l| !l.contains("k2so-agent-heartbeat"))
        .collect();
    lines.push(&entry);
    let new_crontab = lines.join("\n") + "\n";

    crontab_write(&SystemRunner, &new_crontab)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn install_heartbeat_cron(_script_path: &Path) -> Result<(), String> {
    Err("install_heartbeat_cron is Linux-only".to_string())
}

/// Uninstall the heartbeat crontab entry. Idempotent.
#[cfg(target_os = "linux")]
pub fn uninstall_heartbeat_cron() -> Result<(), String> {
    transport_writes_allowed()?;
    let existing = crontab_read(&SystemRunner);

    let new_crontab: String = existing
        .lines()
        .filter(|l| !l.contains("k2so-agent-heartbeat"))
        .collect::<Vec<&str>>()
        .join("\n")
        + "\n";

    crontab_write(&SystemRunner, &new_crontab)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn uninstall_heartbeat_cron() -> Result<(), String> {
    Ok(())
}

/// Install heartbeat scheduler with the given interval / wake-system
/// settings (macOS launchd or Linux cron). Refreshes `heartbeat.sh`
/// before installing. Returns a brief human-readable summary.
pub fn install_heartbeat_scheduler(
    interval_seconds: u32,
    wake_system: bool,
) -> Result<String, String> {
    transport_writes_allowed()?;
    let script_path = write_heartbeat_script()?;
    #[cfg(target_os = "macos")]
    {
        install_heartbeat_launchd(&script_path, interval_seconds, wake_system)?;
        let mins = interval_seconds.max(60) / 60;
        return Ok(format!(
            "heartbeat scheduler installed (every {} min{}).",
            mins,
            if wake_system { " — wakes system from sleep" } else { "" }
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let _ = interval_seconds;
        let _ = wake_system;
        install_heartbeat_cron(&script_path)?;
        return Ok("heartbeat crontab entry installed.".to_string());
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (script_path, interval_seconds, wake_system);
        Err("unsupported platform".to_string())
    }
}

/// Uninstall whichever scheduler is appropriate for this OS, and
/// remove the on-disk heartbeat script. Idempotent.
pub fn uninstall_heartbeat_scheduler() -> Result<(), String> {
    transport_writes_allowed()?;
    #[cfg(target_os = "macos")]
    uninstall_heartbeat_launchd()?;
    #[cfg(target_os = "linux")]
    uninstall_heartbeat_cron()?;
    let k2so_home = home_dir().join(".k2");
    let _ = fs::remove_file(k2so_home.join("heartbeat.sh"));
    let _ = fs::remove_file(k2so_home.join("heartbeat-projects.txt"));
    Ok(())
}

/// Apply the user's wake-scheduler settings:
///
/// - `mode == "off"` or `"on_demand"` → uninstall any active plist /
///   crontab entry; daemon still fires when started by Tauri/CLI but
///   the system stays asleep.
/// - `mode == "heartbeat"` → write heartbeat.sh + install the plist /
///   crontab entry with the user's `interval_minutes` + `wake_system`.
///
/// Idempotent — safe to call on every Apply click even if nothing
/// changed.
pub fn apply_wake_scheduler(
    mode: &str,
    interval_minutes: u32,
    wake_system: bool,
) -> Result<String, String> {
    transport_writes_allowed()?;
    match mode {
        "off" | "on_demand" => {
            uninstall_heartbeat_scheduler()?;
            Ok(format!(
                "wake scheduler set to '{}' — heartbeat plist removed.",
                mode
            ))
        }
        "heartbeat" => {
            let interval_secs = interval_minutes.max(1) * 60;
            install_heartbeat_scheduler(interval_secs, wake_system)
        }
        other => Err(format!(
            "unknown wake scheduler mode '{}'. Expected 'off', 'on_demand', or 'heartbeat'.",
            other
        )),
    }
}

/// Test scaffolding: run `f` with `$HOME` pointed at a fresh temp folder,
/// holding the crate-wide `themes::HOME_LOCK` so no other HOME-mutating
/// test races it. Restores HOME and removes the folder afterwards.
#[cfg(test)]
pub(crate) fn with_temp_home<R>(label: &str, f: impl FnOnce(&Path) -> R) -> R {
    let _lock = crate::themes::HOME_LOCK.lock();
    let home = std::env::temp_dir().join(format!(
        "k2-hb-transport-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&home).expect("create temp HOME");
    let prev = std::env::var_os("HOME");
    std::env::set_var("HOME", &home);
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&home)));
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = fs::remove_dir_all(&home);
    match out {
        Ok(v) => v,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[cfg(test)]
mod transport_guard_tests {
    use super::*;

    fn assert_no_scheduler_files(home: &Path) {
        let plist = home.join("Library/LaunchAgents/dev.k2.heartbeat.plist");
        let script = home.join(".k2/heartbeat.sh");
        assert!(!plist.exists(), "guard let a plist be written at {}", plist.display());
        assert!(!script.exists(), "guard let heartbeat.sh be written at {}", script.display());
    }

    #[test]
    fn pure_guard_refuses_flag_foreign_home_and_test_builds() {
        let real = Path::new("/Users/someone");
        let tmp = Path::new("/private/var/folders/xx/T/k2so-tunnel-test-1-2");

        let e = writes_allowed_for(Some("1"), Some(real), Some(real), false).unwrap_err();
        assert!(e.contains("K2_HEARTBEAT_NO_SELF_HEAL=1"), "{e}");

        let e = writes_allowed_for(None, Some(tmp), Some(real), false).unwrap_err();
        assert!(e.contains("HOME is not the user home"), "{e}");

        let e = writes_allowed_for(None, Some(real), Some(real), true).unwrap_err();
        assert!(e.contains("cfg(test)"), "{e}");

        // The only allowed shape: no flag, HOME == user home, not a test build.
        writes_allowed_for(None, Some(real), Some(real), false).expect("real session allowed");
        writes_allowed_for(Some("0"), Some(real), Some(real), false).expect("flag=0 allowed");
        writes_allowed_for(None, Some(Path::new("/Users/someone/")), Some(real), false)
            .expect("trailing slash is the same home");
    }

    #[cfg(unix)]
    #[test]
    fn password_home_resolves_on_unix() {
        let pw = password_home().expect("getpwuid_r must resolve this uid's home");
        assert!(pw.is_absolute(), "password home not absolute: {}", pw.display());
    }

    /// T1 — the 2026-09-30 hijack path. Under a temp HOME, a real
    /// `k2so_heartbeat_add` runs no launchctl/crontab and writes nothing
    /// under that HOME; the installer itself returns the HOME refusal.
    #[test]
    fn heartbeat_add_under_temp_home_never_reaches_the_scheduler() {
        crate::db::init_for_tests();
        let project = std::env::temp_dir().join(format!(
            "k2-hb-guard-add-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&project).unwrap();
        let path = project.to_string_lossy().to_string();
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, 'hb-guard', ?2)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), path],
            )
            .unwrap();
        }
        with_temp_home("add", |home| {
            test_recorder::take();
            crate::heartbeats::k2so_heartbeat_add(
                path.clone(),
                "guarded".into(),
                "daily".into(),
                "{}".into(),
            )
            .expect("add succeeds without a scheduler");
            assert_eq!(test_recorder::take(), Vec::<String>::new());
            assert_no_scheduler_files(home);

            let e = ensure_cron_installed().unwrap_err();
            #[cfg(unix)]
            assert!(e.contains("HOME is not the user home"), "{e}");
            #[cfg(not(unix))]
            assert!(e.contains("cfg(test)"), "{e}");
            assert_eq!(test_recorder::take(), Vec::<String>::new());
            assert_no_scheduler_files(home);
        });
        let _ = fs::remove_dir_all(&project);
    }

    /// Every public writer refuses under a temp HOME — none returns Ok.
    #[test]
    fn every_public_writer_refuses_under_temp_home() {
        with_temp_home("writers", |home| {
            test_recorder::take();
            let results: Vec<(&str, Result<(), String>)> = vec![
                ("ensure_cron_installed", ensure_cron_installed().map(|_| ())),
                ("write_heartbeat_script", write_heartbeat_script().map(|_| ())),
                ("install_heartbeat_scheduler", install_heartbeat_scheduler(60, true).map(|_| ())),
                ("uninstall_heartbeat_scheduler", uninstall_heartbeat_scheduler()),
                ("apply_wake_scheduler(off)", apply_wake_scheduler("off", 5, false).map(|_| ())),
                ("apply_wake_scheduler(on_demand)", apply_wake_scheduler("on_demand", 5, false).map(|_| ())),
                ("apply_wake_scheduler(heartbeat)", apply_wake_scheduler("heartbeat", 5, true).map(|_| ())),
            ];
            for (name, r) in results {
                let e = r.expect_err(&format!("{name} must refuse under a temp HOME"));
                assert!(e.contains("refused"), "{name}: {e}");
            }
            assert_eq!(test_recorder::take(), Vec::<String>::new());
            assert_no_scheduler_files(home);
        });
    }

    /// The opt-out flag refuses even with the real HOME.
    #[test]
    fn opt_out_flag_refuses_apply() {
        let _lock = crate::themes::HOME_LOCK.lock();
        let prev = std::env::var_os(NO_SELF_HEAL_ENV);
        std::env::set_var(NO_SELF_HEAL_ENV, "1");
        test_recorder::take();
        let r = apply_wake_scheduler("heartbeat", 5, true);
        match prev {
            Some(v) => std::env::set_var(NO_SELF_HEAL_ENV, v),
            None => std::env::remove_var(NO_SELF_HEAL_ENV),
        }
        let e = r.expect_err("flag must refuse");
        assert!(e.contains("K2_HEARTBEAT_NO_SELF_HEAL=1"), "{e}");
        assert_eq!(test_recorder::take(), Vec::<String>::new());
    }

    /// With the real HOME and no flag, a cfg(test) build still refuses.
    #[test]
    fn real_home_still_refused_in_test_builds() {
        let _lock = crate::themes::HOME_LOCK.lock();
        test_recorder::take();
        let e = ensure_cron_installed().expect_err("cfg(test) must refuse");
        assert!(e.contains("refused"), "{e}");
        assert_eq!(test_recorder::take(), Vec::<String>::new());
    }

    /// The seam itself: under cfg(test) the system runner records and
    /// runs nothing.
    #[test]
    fn system_runner_records_and_runs_nothing_under_test() {
        test_recorder::take();
        let e = SystemRunner
            .run("launchctl", &["print", "gui/0/dev.k2.heartbeat"], None)
            .expect_err("cfg(test) runner must not run");
        assert!(e.contains("never runs"), "{e}");
        assert_eq!(
            test_recorder::take(),
            vec!["launchctl print gui/0/dev.k2.heartbeat".to_string()]
        );
    }
}
