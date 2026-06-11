//! Phase 2 Unit 5 — Claude Code authentication scheduler ownership
//! moved from Tauri to the daemon.
//!
//! Tauri's pre-Unit-5 `src-tauri/src/commands/claude_auth.rs` did all
//! of:
//!   1. Read OAuth credentials from the macOS Keychain (preferred) or
//!      `~/.claude/.credentials.json` (fallback).
//!   2. Compute their state (valid / expiring / expired / missing).
//!   3. Perform the OAuth refresh-token POST against
//!      `platform.claude.com` and write the new tokens back to the
//!      same two stores.
//!   4. Install / uninstall the launchd plist
//!      (`~/Library/LaunchAgents/dev.k2.claude-auth.plist`)
//!      that runs `~/.k2so/claude-auth-refresh.sh` every 20 minutes.
//!
//! Post-Unit-5, all four responsibilities live here so the refresh
//! still happens when Tauri is closed. Routes:
//!
//! - `GET  /cli/claude-auth/status` — credential state + scheduler-installed flag.
//! - `POST /cli/claude-auth/refresh-now` — immediate OAuth refresh.
//! - `POST /cli/claude-auth/install-scheduler` — write plist + load.
//! - `POST /cli/claude-auth/uninstall-scheduler` — unload + delete.
//!
//! ## Keychain access from a daemon process
//!
//! Verified by the Unit 5 spike: a foreground daemon binary launched
//! from a user shell can read the same `Claude Code-credentials`
//! Keychain entry that Tauri's command did. LaunchAgents run in the
//! same user GUI session as Tauri (unlike LaunchDaemons in the system
//! session), so the same access works post-install.
//!
//! ## TODO Phase 2.1
//!
//! The shell script the plist invokes (`claude-auth-refresh.sh`)
//! still does its own OAuth dance via `curl + python3` — the same
//! pre-Unit-5 implementation. Phase 2.1's CLI verb cleanup is the
//! right time to point the scheduler at `k2so claude-auth refresh`
//! (which would HTTP-POST to the daemon's `/cli/claude-auth/refresh-now`
//! route) instead of the standalone shell script. Until then we
//! keep the bash script as-is to avoid coupling the scheduler's
//! refresh path to the daemon being alive — a daemon crash mid-
//! refresh is acceptable failure mode for a 20-minute polling job
//! but a scheduler that depends on the daemon for every fire is
//! not. Left as a `TODO Phase 2.1` marker.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::cli_response::CliResponse;

// ── Constants ───────────────────────────────────────────────────────────

const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const KEYCHAIN_ACCOUNT: &str = "Claude Code";
const TOKEN_ENDPOINT: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "https://claude.ai/oauth/claude-code-client-metadata";
const EXPIRY_BUFFER_SECS: i64 = 300; // 5 minutes

#[cfg(target_os = "macos")]
const PLIST_LABEL: &str = "dev.k2.claude-auth";

// ── Credential helpers ──────────────────────────────────────────────────

struct RawCredentials {
    json: serde_json::Value,
    refresh_token: String,
    expires_at: i64,
}

fn credentials_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude/.credentials.json")
}

fn read_credentials() -> Option<RawCredentials> {
    #[cfg(target_os = "macos")]
    {
        if let Some(creds) = read_keychain_credentials() {
            return Some(creds);
        }
    }
    read_file_credentials()
}

#[cfg(target_os = "macos")]
fn read_keychain_credentials() -> Option<RawCredentials> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    parse_credential_json(raw.trim())
}

fn read_file_credentials() -> Option<RawCredentials> {
    let path = credentials_file();
    let raw = fs::read_to_string(&path).ok()?;
    parse_credential_json(&raw)
}

fn parse_credential_json(raw: &str) -> Option<RawCredentials> {
    let json: serde_json::Value = serde_json::from_str(raw).ok()?;
    let oauth = json.get("claudeAiOauth")?;
    let access_token = oauth.get("accessToken")?.as_str()?.to_string();
    let refresh_token = oauth.get("refreshToken")?.as_str()?.to_string();
    let expires_at = oauth.get("expiresAt")?.as_i64()?;
    if access_token.is_empty() || refresh_token.is_empty() {
        return None;
    }
    Some(RawCredentials {
        json,
        refresh_token,
        expires_at,
    })
}

fn compute_auth_state(expires_at: i64) -> (String, i64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let remaining = expires_at - now;
    let state = if remaining > EXPIRY_BUFFER_SECS {
        "valid"
    } else if remaining > 0 {
        "expiring"
    } else {
        "expired"
    };
    (state.to_string(), remaining)
}

fn write_credentials(updated_json: &serde_json::Value) -> Result<(), String> {
    let json_str = serde_json::to_string(updated_json).map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    {
        // -U: update if exists. Pass via -w arg so the value never
        // hits a shell — keeps newlines/quotes/escapes intact.
        let status = Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                KEYCHAIN_ACCOUNT,
                "-w",
                &json_str,
            ])
            .output()
            .map_err(|e| format!("Failed to run security command: {}", e))?;
        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            k2_core::log_debug!("[claude-auth] Keychain write failed: {}", stderr);
        }
    }

    let path = credentials_file();
    if path.exists() {
        fs::write(&path, &json_str)
            .map_err(|e| format!("Failed to write credentials file: {}", e))?;
    }
    Ok(())
}

// ── Scheduler helpers ───────────────────────────────────────────────────

fn k2so_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
}

fn refresh_script_path() -> PathBuf {
    k2so_dir().join("claude-auth-refresh.sh")
}

#[cfg(target_os = "macos")]
fn plist_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents/dev.k2.claude-auth.plist")
}

/// Generates the bash refresh script. Identical content to the
/// pre-Unit-5 Tauri implementation. See module docs for why this
/// stays as a self-contained shell script rather than dispatching
/// through the daemon's HTTP surface.
fn generate_refresh_script() -> String {
    let home = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .to_string_lossy()
        .to_string();

    format!(
        r##"#!/bin/bash
# K2SO Claude Auth Refresh — DO NOT EDIT (managed by K2SO)
# Runs via launchd/cron every 20 minutes.

CRED_SERVICE="Claude Code-credentials"
CRED_ACCOUNT="Claude Code"
TOKEN_URL="{token_url}"
CLIENT_ID="{client_id}"
LOG_FILE="{home}/.k2so/claude-auth-refresh.log"
BUFFER_SECONDS=300

ts() {{ date '+%Y-%m-%d %H:%M:%S'; }}

# Read credentials from keychain (macOS) or file
CREDS=""
if command -v security &>/dev/null; then
    CREDS=$(security find-generic-password -s "$CRED_SERVICE" -w 2>/dev/null)
fi
if [ -z "$CREDS" ] && [ -f "{home}/.claude/.credentials.json" ]; then
    CREDS=$(cat "{home}/.claude/.credentials.json" 2>/dev/null)
fi
if [ -z "$CREDS" ]; then
    echo "$(ts) No credentials found — skipping" >> "$LOG_FILE"
    exit 0
fi

# Extract fields using python3
if ! command -v python3 &>/dev/null; then
    echo "$(ts) python3 not found — cannot parse credentials" >> "$LOG_FILE"
    exit 1
fi

EXPIRES_AT=$(echo "$CREDS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('claudeAiOauth',{{}}).get('expiresAt',''))" 2>/dev/null)
REFRESH_TOKEN=$(echo "$CREDS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('claudeAiOauth',{{}}).get('refreshToken',''))" 2>/dev/null)

if [ -z "$REFRESH_TOKEN" ]; then
    echo "$(ts) No refresh token found — skipping" >> "$LOG_FILE"
    exit 0
fi

# Check if token needs refresh
NOW=$(date +%s)
if echo "$EXPIRES_AT" | grep -qE '^[0-9]+$'; then
    EXPIRY=$EXPIRES_AT
else
    EXPIRY=0
fi

REMAINING=$((EXPIRY - NOW))
if [ "$REMAINING" -gt "$BUFFER_SECONDS" ]; then
    exit 0
fi

echo "$(ts) Token expiring in ${{REMAINING}}s — refreshing..." >> "$LOG_FILE"

# Perform OAuth refresh
RESPONSE=$(curl -s -X POST "$TOKEN_URL" \
    -H "Content-Type: application/x-www-form-urlencoded" \
    --connect-timeout 10 --max-time 30 \
    -d "grant_type=refresh_token&client_id=$CLIENT_ID&refresh_token=$REFRESH_TOKEN")

if echo "$RESPONSE" | python3 -c "import sys,json; d=json.load(sys.stdin); sys.exit(0 if 'access_token' in d else 1)" 2>/dev/null; then
    : # success
else
    echo "$(ts) Refresh failed: $RESPONSE" >> "$LOG_FILE"
    exit 1
fi

# Update credentials
NEW_CREDS=$(python3 -c "
import json, time, sys
creds = json.loads(sys.argv[1])
resp = json.loads(sys.argv[2])
oauth = creds.get('claudeAiOauth', {{}})
oauth['accessToken'] = resp['access_token']
if 'refresh_token' in resp:
    oauth['refreshToken'] = resp['refresh_token']
expires_in = resp.get('expires_in', 3600)
oauth['expiresAt'] = int(time.time()) + expires_in
creds['claudeAiOauth'] = oauth
print(json.dumps(creds))
" "$CREDS" "$RESPONSE" 2>/dev/null)

if [ -z "$NEW_CREDS" ]; then
    echo "$(ts) Failed to construct updated credentials" >> "$LOG_FILE"
    exit 1
fi

# Write back
if command -v security &>/dev/null; then
    security add-generic-password -U -s "$CRED_SERVICE" -a "$CRED_ACCOUNT" -w "$NEW_CREDS" 2>/dev/null
fi
if [ -f "{home}/.claude/.credentials.json" ]; then
    echo "$NEW_CREDS" > "{home}/.claude/.credentials.json"
fi

echo "$(ts) Token refreshed successfully" >> "$LOG_FILE"

# Trim log
tail -100 "$LOG_FILE" > "$LOG_FILE.tmp" && mv "$LOG_FILE.tmp" "$LOG_FILE"
"##,
        token_url = TOKEN_ENDPOINT,
        client_id = CLIENT_ID,
        home = home,
    )
}

#[cfg(target_os = "macos")]
fn generate_plist() -> String {
    let script = refresh_script_path();
    let home = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .to_string_lossy()
        .to_string();

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>{script}</string>
    </array>
    <key>StartInterval</key>
    <integer>1200</integer>
    <key>RunAtLoad</key>
    <false/>
    <key>StandardErrorPath</key>
    <string>{home}/.k2so/claude-auth-refresh-stderr.log</string>
</dict>
</plist>"#,
        label = PLIST_LABEL,
        script = script.to_string_lossy(),
        home = home,
    )
}

fn is_scheduler_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        let plist = plist_path();
        if !plist.exists() {
            return false;
        }
        return Command::new("launchctl")
            .args(["list", PLIST_LABEL])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    }

    #[cfg(target_os = "linux")]
    {
        return Command::new("crontab")
            .args(["-l"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.contains("k2so-claude-auth-refresh"))
            .unwrap_or(false);
    }

    #[allow(unreachable_code)]
    false
}

#[cfg(target_os = "macos")]
fn install_launchd() -> Result<(), String> {
    let plist = plist_path();
    if let Some(parent) = plist.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create LaunchAgents dir: {}", e))?;
    }
    // Best-effort unload of any prior plist so launchctl load -w
    // doesn't fail with "already loaded" on a re-install.
    if plist.exists() {
        let _ = Command::new("launchctl")
            .args(["unload", &plist.to_string_lossy()])
            .output();
    }
    let plist_content = generate_plist();
    fs::write(&plist, &plist_content)
        .map_err(|e| format!("Failed to write plist: {}", e))?;
    let output = Command::new("launchctl")
        .args(["load", &plist.to_string_lossy()])
        .output()
        .map_err(|e| format!("Failed to run launchctl load: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("launchctl load failed: {}", stderr));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_crontab() -> Result<(), String> {
    let script = refresh_script_path();
    let marker = "# k2so-claude-auth-refresh";
    let entry = format!("*/20 * * * * {} {}", script.to_string_lossy(), marker);
    let existing = Command::new("crontab")
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
    let mut lines: Vec<&str> = existing
        .lines()
        .filter(|l| !l.contains("k2so-claude-auth-refresh"))
        .collect();
    lines.push(&entry);
    let new_crontab = lines.join("\n") + "\n";
    let mut child = Command::new("crontab")
        .args(["-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn crontab: {}", e))?;
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .ok_or("Failed to open crontab stdin")?
        .write_all(new_crontab.as_bytes())
        .map_err(|e| format!("Failed to write crontab: {}", e))?;
    let status = child.wait().map_err(|e| format!("crontab wait failed: {}", e))?;
    if !status.success() {
        return Err("crontab install failed".to_string());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_launchd() -> Result<(), String> {
    let plist = plist_path();
    if plist.exists() {
        let _ = Command::new("launchctl")
            .args(["unload", &plist.to_string_lossy()])
            .output();
        fs::remove_file(&plist)
            .map_err(|e| format!("Failed to remove plist: {}", e))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_crontab() -> Result<(), String> {
    let existing = Command::new("crontab")
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
    let new_crontab: String = existing
        .lines()
        .filter(|l| !l.contains("k2so-claude-auth-refresh"))
        .collect::<Vec<&str>>()
        .join("\n")
        + "\n";
    let mut child = Command::new("crontab")
        .args(["-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn crontab: {}", e))?;
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .ok_or("Failed to open crontab stdin")?
        .write_all(new_crontab.as_bytes())
        .map_err(|e| format!("Failed to write crontab: {}", e))?;
    child.wait().map_err(|e| format!("crontab wait failed: {}", e))?;
    Ok(())
}

// ── Public route handlers ───────────────────────────────────────────────

/// Handler for `GET /cli/claude-auth/status`.
///
/// Response shape (camelCase, matches the pre-Unit-5 Tauri command so
/// the renderer needs no shape change):
/// ```json
/// {
///   "state": "valid" | "expiring" | "expired" | "missing",
///   "expiresAt": <unix-secs> | null,
///   "secondsRemaining": <i64> | null,
///   "schedulerInstalled": bool
/// }
/// ```
pub fn handle_status() -> CliResponse {
    let scheduler_installed = is_scheduler_installed();
    let body = match read_credentials() {
        Some(creds) => {
            let (state, remaining) = compute_auth_state(creds.expires_at);
            serde_json::json!({
                "state": state,
                "expiresAt": creds.expires_at,
                "secondsRemaining": remaining,
                "schedulerInstalled": scheduler_installed,
            })
        }
        None => serde_json::json!({
            "state": "missing",
            "expiresAt": null,
            "secondsRemaining": null,
            "schedulerInstalled": scheduler_installed,
        }),
    };
    CliResponse::ok_json(body.to_string())
}

/// Handler for `POST /cli/claude-auth/refresh-now`.
///
/// Runs the same OAuth refresh-token POST that the pre-Unit-5 Tauri
/// `claude_auth_refresh` command did, but server-side: no Tauri
/// event channel required, no IPC ping. Returns the updated status
/// payload (same shape as `handle_status`) on success.
///
/// Body is ignored (kept on POST instead of GET because the side
/// effect — token write — should not be cacheable).
pub fn handle_refresh_now() -> CliResponse {
    let creds = match read_credentials() {
        Some(c) => c,
        None => return CliResponse::bad_request("No Claude credentials found"),
    };

    let body = format!(
        "grant_type=refresh_token&client_id={}&refresh_token={}",
        CLIENT_ID, creds.refresh_token
    );

    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(e) => return CliResponse::bad_request(format!("HTTP client init failed: {e}")),
    };

    let response = match client
        .post(TOKEN_ENDPOINT)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
    {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("Refresh request failed: {e}")),
    };

    if !response.status().is_success() {
        let status = response.status();
        let resp_body = response.text().unwrap_or_default();
        return CliResponse::bad_request(format!("Refresh failed (HTTP {status}): {resp_body}"));
    }

    let resp_text = match response.text() {
        Ok(t) => t,
        Err(e) => {
            return CliResponse::bad_request(format!("Failed to read refresh response: {e}"))
        }
    };
    let resp_json: serde_json::Value = match serde_json::from_str(&resp_text) {
        Ok(j) => j,
        Err(e) => {
            return CliResponse::bad_request(format!("Failed to parse refresh response: {e}"))
        }
    };

    let new_access = match resp_json.get("access_token").and_then(serde_json::Value::as_str) {
        Some(s) => s,
        None => return CliResponse::bad_request("No access_token in refresh response"),
    };
    let new_refresh = resp_json
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&creds.refresh_token);
    let expires_in = resp_json
        .get("expires_in")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(3600);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let new_expires_at = now + expires_in;

    let mut updated = creds.json.clone();
    if let Some(oauth) = updated.get_mut("claudeAiOauth") {
        oauth["accessToken"] = serde_json::Value::String(new_access.to_string());
        oauth["refreshToken"] = serde_json::Value::String(new_refresh.to_string());
        oauth["expiresAt"] =
            serde_json::Value::Number(serde_json::Number::from(new_expires_at));
    }
    if let Err(e) = write_credentials(&updated) {
        return CliResponse::bad_request(e);
    }

    let (state, remaining) = compute_auth_state(new_expires_at);
    let body = serde_json::json!({
        "state": state,
        "expiresAt": new_expires_at,
        "secondsRemaining": remaining,
        "schedulerInstalled": is_scheduler_installed(),
    });
    CliResponse::ok_json(body.to_string())
}

/// Handler for `POST /cli/claude-auth/install-scheduler`.
///
/// 1. Creates `~/.k2so/` if missing.
/// 2. Writes `claude-auth-refresh.sh` with executable mode.
/// 3. On macOS, writes + loads the launchd plist. On Linux, edits
///    the user's crontab.
///
/// Idempotent — calling twice in a row reinstalls cleanly because
/// `install_launchd` best-effort-unloads any prior plist first.
pub fn handle_install_scheduler() -> CliResponse {
    let dir = k2so_dir();
    if let Err(e) = fs::create_dir_all(&dir) {
        return CliResponse::bad_request(format!("Failed to create ~/.k2so: {e}"));
    }

    let script_path = refresh_script_path();
    let script_content = generate_refresh_script();
    if let Err(e) = fs::write(&script_path, &script_content) {
        return CliResponse::bad_request(format!("Failed to write refresh script: {e}"));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        if let Err(e) = fs::set_permissions(&script_path, perms) {
            return CliResponse::bad_request(format!("Failed to set script permissions: {e}"));
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Err(e) = install_launchd() {
            return CliResponse::bad_request(e);
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Err(e) = install_crontab() {
            return CliResponse::bad_request(e);
        }
    }

    CliResponse::ok_json(r#"{"success":true}"#.to_string())
}

/// Handler for `POST /cli/claude-auth/uninstall-scheduler`.
///
/// macOS: `launchctl unload` + delete plist + delete script.
/// Linux: remove the marker line from crontab.
///
/// Idempotent — succeeds on a system that never had the scheduler
/// installed.
pub fn handle_uninstall_scheduler() -> CliResponse {
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = uninstall_launchd() {
            return CliResponse::bad_request(e);
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Err(e) = uninstall_crontab() {
            return CliResponse::bad_request(e);
        }
    }

    let script = refresh_script_path();
    if script.exists() {
        let _ = fs::remove_file(&script);
    }

    CliResponse::ok_json(r#"{"success":true}"#.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_auth_state_classifies_correctly() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let (state, _) = compute_auth_state(now + 3600);
        assert_eq!(state, "valid");
        let (state, _) = compute_auth_state(now + 60);
        assert_eq!(state, "expiring");
        let (state, _) = compute_auth_state(now - 10);
        assert_eq!(state, "expired");
    }

    #[test]
    fn parse_credential_json_extracts_oauth_fields() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":1234}}"#;
        let parsed = parse_credential_json(raw).expect("should parse");
        assert_eq!(parsed.refresh_token, "r");
        assert_eq!(parsed.expires_at, 1234);
    }

    #[test]
    fn parse_credential_json_rejects_empty_tokens() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"","refreshToken":"","expiresAt":0}}"#;
        assert!(parse_credential_json(raw).is_none());
    }

    #[test]
    fn parse_credential_json_returns_none_on_missing_oauth_block() {
        let raw = r#"{"otherKey":1}"#;
        assert!(parse_credential_json(raw).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn generated_plist_contains_label_and_script_path() {
        let plist = generate_plist();
        assert!(plist.contains("dev.k2.claude-auth"));
        assert!(plist.contains("claude-auth-refresh.sh"));
        assert!(plist.contains("<integer>1200</integer>"));
    }

    #[test]
    fn generated_refresh_script_has_required_pieces() {
        let s = generate_refresh_script();
        assert!(s.contains("Claude Code-credentials"));
        assert!(s.contains("platform.claude.com"));
        assert!(s.contains("refresh_token"));
    }
}
