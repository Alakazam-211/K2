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
//! Generates `~/.k2so/heartbeat.sh` (the bridge that asks the daemon
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
    let k2so_home = home_dir().join(".k2");
    fs::create_dir_all(&k2so_home).map_err(|e| format!("create ~/.k2so: {e}"))?;

    let script_path = k2so_home.join("heartbeat.sh");
    let mut changed = false;

    // Write/refresh heartbeat.sh if it doesn't match the current
    // template. This catches users upgrading from a pre-P5.6
    // install whose on-disk script still references
    // ~/.k2so/heartbeat-projects.txt.
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

/// Bash script written to `~/.k2so/heartbeat.sh`. Asks the daemon
/// for active projects on every tick and forwards each to
/// `/cli/scheduler-tick`. P5.6 retired the `heartbeat-projects.txt`
/// dependency — the DB is the only source of truth for which
/// workspaces have heartbeats.
pub fn generate_heartbeat_script() -> String {
    let home = home_dir().to_string_lossy().to_string();

    format!(r##"#!/bin/bash
# K2SO Agent Heartbeat — DO NOT EDIT (managed by K2SO daemon)
# Asks the daemon which projects have active heartbeats, then ticks each.

PORT_FILE="{home}/.k2so/heartbeat.port"
LOG_FILE="{home}/.k2so/heartbeat.log"
TOKEN_FILE="{home}/.k2so/heartbeat.token"

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
    <string>{home}/.k2so/heartbeat-stderr.log</string>
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
    let uid_target = format!("gui/{}/dev.k2.heartbeat", unsafe {
        libc::getuid()
    });
    let already_loaded = std::process::Command::new("launchctl")
        .args(["print", &uid_target])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !plist_changed && already_loaded {
        return Ok(false);
    }

    // Bootout the existing agent before rewriting (safe even if not loaded).
    if already_loaded {
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &uid_target])
            .output();
    }

    if plist_changed {
        fs::write(&plist_path, &want_plist)
            .map_err(|e| format!("write plist: {e}"))?;
    }

    // Bootstrap into the user's GUI domain.
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let output = std::process::Command::new("launchctl")
        .args(["bootstrap", &domain, &plist_path.to_string_lossy()])
        .output()
        .map_err(|e| format!("launchctl bootstrap: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(true)
}

#[cfg(target_os = "linux")]
fn install_linux_if_missing(script_path: &Path) -> Result<bool, String> {
    let marker = "# k2so-agent-heartbeat";
    let entry = format!("* * * * * {} {}", script_path.to_string_lossy(), marker);

    let existing = std::process::Command::new("crontab")
        .args(["-l"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .unwrap_or_default();

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

    let mut child = std::process::Command::new("crontab")
        .args(["-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn crontab: {e}"))?;

    use std::io::Write;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| "crontab stdin".to_string())?
        .write_all(new_crontab.as_bytes())
        .map_err(|e| format!("write crontab: {e}"))?;
    child.wait().map_err(|e| format!("wait crontab: {e}"))?;
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

/// Write `heartbeat.sh` to `~/.k2so/` (chmod 0755). Idempotent —
/// callers can invoke before every install_launchd / install_cron pass
/// without worrying about double-write.
///
/// Returns the script path so the caller can hand it to the platform
/// installer.
pub fn write_heartbeat_script() -> Result<PathBuf, String> {
    let k2so_home = home_dir().join(".k2");
    fs::create_dir_all(&k2so_home).map_err(|e| format!("create ~/.k2so: {e}"))?;

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
    let home = home_dir();
    let plist_path = home.join("Library/LaunchAgents/dev.k2.heartbeat.plist");

    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create LaunchAgents dir: {e}"))?;
    }

    if plist_path.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &plist_path.to_string_lossy()])
            .output();
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
    <string>{home}/.k2so/heartbeat-stderr.log</string>
</dict>
</plist>"#,
        script = script_path.to_string_lossy(),
        interval = interval_seconds,
        wake_key = wake_key,
        home = home.to_string_lossy(),
    );

    fs::write(&plist_path, &plist).map_err(|e| format!("write plist: {e}"))?;

    let output = std::process::Command::new("launchctl")
        .args(["load", &plist_path.to_string_lossy()])
        .output()
        .map_err(|e| format!("launchctl: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "launchctl load failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
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
    let home = home_dir();
    let plist_path = home.join("Library/LaunchAgents/dev.k2.heartbeat.plist");
    if plist_path.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &plist_path.to_string_lossy()])
            .output();
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
    let marker = "# k2so-agent-heartbeat";
    let entry = format!("* * * * * {} {}", script_path.to_string_lossy(), marker);

    let existing = std::process::Command::new("crontab")
        .args(["-l"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
        .unwrap_or_default();

    let mut lines: Vec<&str> = existing
        .lines()
        .filter(|l| !l.contains("k2so-agent-heartbeat"))
        .collect();
    lines.push(&entry);
    let new_crontab = lines.join("\n") + "\n";

    let mut child = std::process::Command::new("crontab")
        .args(["-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn crontab: {e}"))?;

    use std::io::Write;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| "crontab stdin".to_string())?
        .write_all(new_crontab.as_bytes())
        .map_err(|e| format!("write crontab: {e}"))?;
    child.wait().map_err(|e| format!("wait crontab: {e}"))?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn install_heartbeat_cron(_script_path: &Path) -> Result<(), String> {
    Err("install_heartbeat_cron is Linux-only".to_string())
}

/// Uninstall the heartbeat crontab entry. Idempotent.
#[cfg(target_os = "linux")]
pub fn uninstall_heartbeat_cron() -> Result<(), String> {
    let existing = std::process::Command::new("crontab")
        .args(["-l"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
        .unwrap_or_default();

    let new_crontab: String = existing
        .lines()
        .filter(|l| !l.contains("k2so-agent-heartbeat"))
        .collect::<Vec<&str>>()
        .join("\n")
        + "\n";

    let mut child = std::process::Command::new("crontab")
        .args(["-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn crontab: {e}"))?;

    use std::io::Write;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| "crontab stdin".to_string())?
        .write_all(new_crontab.as_bytes())
        .map_err(|e| format!("write crontab: {e}"))?;
    child.wait().map_err(|e| format!("wait crontab: {e}"))?;
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
