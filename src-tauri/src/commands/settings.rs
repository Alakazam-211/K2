// The `unexpected_cfgs` allowance silences `cfg(cargo-clippy)` gates
// that the `objc::msg_send!` macro expands to under recent Rust (the
// objc crate hasn't updated its macros for the stricter cfg check).
// `deprecated` silences the cocoa→objc2 migration warnings — that
// migration is its own follow-up.
#![allow(deprecated, unexpected_cfgs)]

//! Tauri-side host shims for global app settings.
//!
//! Phase 2 Unit 7c — the residual `read_settings`/`write_settings`
//! compat wrappers (last hold-outs from Unit 7a) are gone. Every
//! Tauri-side reader now calls `k2_core::app_settings::load()`
//! directly; writers go through the daemon's `/cli/settings/{update,
//! reset}` route so the daemon's process-wide settings lock is the
//! sole serializer.
//!
//! What's left in this file:
//!
//! - CLI-install / window-edited / relaunch helpers — genuine HOST
//!   concerns (sudo-bound symlink writes, native window AppKit
//!   calls, `.app` relaunch). They stay because the daemon has no
//!   business writing `/usr/local/bin/k2so` or talking to AppKit.
//!
//! Plan B cleanup: the `settings_{get,update,reset}` daemon proxies
//! (and their `connect()` helper + the now-unused `AppSettings`
//! re-export) were deleted — the renderer reaches settings data
//! host-aware via `/cli/settings/*` on the active daemon. Any Rust
//! caller that still needs the type imports it from
//! `k2_core::app_settings::AppSettings` directly.

use std::fs;
use std::path::{Path, PathBuf};
use tauri::AppHandle;
#[cfg(target_os = "macos")] // only the mac set_document_edited body needs it
use tauri::Manager;

// ── CLI Install ────────────────────────────────────────────────────────
// macOS: symlink into /usr/local/bin (osascript for admin).
// Windows: copy into %LOCALAPPDATA%\K2\cli + k2.cmd launcher on user PATH
// (no admin). Requires Git for Windows bash to run the shell CLI script.

/// Find a bundled cli/<name> script (production or development).
/// 0.40.0: the CLI is `k2`; `k2so` remains as a deprecation shim that
/// delegates to `k2` — both ship in cli/ and both get installed.
fn find_cli_script_named(name: &str) -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let exe_dir = exe_path.parent()?;

    // Production macOS: K2.app/Contents/MacOS/<bin> → Contents/Resources/_up_/cli/<name>
    // Tauri puts "../cli/*" resources under Resources/_up_/cli/
    let resources_cli = exe_dir.parent().map(|contents| {
        contents
            .join("Resources")
            .join("_up_")
            .join("cli")
            .join(name)
    });
    if let Some(ref p) = resources_cli {
        if p.exists() {
            return resources_cli;
        }
    }

    // Production Windows (NSIS/MSI): resources land next to the exe or under
    // resources/ / cli/ (layout varies by Tauri version / bundler).
    for p in [
        exe_dir.join("cli").join(name),
        exe_dir.join(name),
        exe_dir.join("resources").join("cli").join(name),
        exe_dir.join("resources").join(name),
        exe_dir.join("_up_").join("cli").join(name),
    ] {
        if p.exists() {
            return Some(p);
        }
    }

    // Development: target/debug/<bin> → ../../cli/<name> from repo root
    let dev_cli = exe_dir
        .parent() // target/
        .and_then(|p| p.parent()) // src-tauri/ (or repo root for workspace target)
        .and_then(|p| p.parent()) // repo root
        .map(|repo| repo.join("cli").join(name));
    if let Some(ref p) = dev_cli {
        if p.exists() {
            return dev_cli;
        }
    }

    None
}

fn find_cli_script() -> Option<PathBuf> {
    find_cli_script_named("k2")
}

/// macOS install path for the `k2` symlink.
#[cfg(not(windows))]
const CLI_SYMLINK_PATH: &str = "/usr/local/bin/k2";
/// Legacy alias — points at the cli/k2so deprecation shim.
#[cfg(not(windows))]
const CLI_LEGACY_SYMLINK_PATH: &str = "/usr/local/bin/k2so";

/// Windows: user-scoped install root (`%LOCALAPPDATA%\K2`).
#[cfg(windows)]
fn windows_k2_home() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| "LOCALAPPDATA is not set".to_string())?;
    Ok(PathBuf::from(local).join("K2"))
}

#[cfg(windows)]
fn windows_cli_cmd_path() -> Result<PathBuf, String> {
    Ok(windows_k2_home()?.join("bin").join("k2.cmd"))
}

/// Write a `k2.cmd` launcher that runs the bundled bash CLI via Git Bash.
#[cfg(windows)]
fn write_windows_cli_cmd(cmd_path: &Path, script_path: &Path) -> Result<(), String> {
    let script = script_path.display().to_string().replace('/', "\\");
    let body = format!(
        "@echo off\r\n\
         setlocal\r\n\
         set \"K2_SCRIPT={script}\"\r\n\
         if exist \"%ProgramFiles%\\Git\\bin\\bash.exe\" (\r\n\
           \"%ProgramFiles%\\Git\\bin\\bash.exe\" \"%K2_SCRIPT%\" %*\r\n\
           exit /b %ERRORLEVEL%\r\n\
         )\r\n\
         if exist \"%ProgramFiles(x86)%\\Git\\bin\\bash.exe\" (\r\n\
           \"%ProgramFiles(x86)%\\Git\\bin\\bash.exe\" \"%K2_SCRIPT%\" %*\r\n\
           exit /b %ERRORLEVEL%\r\n\
         )\r\n\
         where bash >nul 2>&1 && (\r\n\
           bash \"%K2_SCRIPT%\" %*\r\n\
           exit /b %ERRORLEVEL%\r\n\
         )\r\n\
         echo K2 CLI needs Git for Windows (bash) on PATH. Install Git, then retry Settings → Install CLI. 1>&2\r\n\
         exit /b 1\r\n"
    );
    if let Some(parent) = cmd_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::write(cmd_path, body).map_err(|e| format!("write {}: {e}", cmd_path.display()))
}

/// Ensure `%LOCALAPPDATA%\K2\bin` is on the *user* PATH (no admin).
#[cfg(windows)]
fn ensure_windows_user_path_has(bin_dir: &Path) -> Result<(), String> {
    let bin = bin_dir
        .canonicalize()
        .unwrap_or_else(|_| bin_dir.to_path_buf());
    let bin_s = bin.display().to_string();
    // PowerShell: append if missing. Broadcasts WM_SETTINGCHANGE so new
    // terminals pick it up; already-open shells still need a restart.
    let ps = format!(
        "$bin = '{}'; \
         $user = [Environment]::GetEnvironmentVariable('Path','User'); \
         if (-not $user) {{ $user = '' }}; \
         $parts = $user -split ';' | Where-Object {{ $_ -ne '' }}; \
         if ($parts -contains $bin) {{ exit 0 }}; \
         $new = if ($user.TrimEnd(';') -eq '') {{ $bin }} else {{ $user.TrimEnd(';') + ';' + $bin }}; \
         [Environment]::SetEnvironmentVariable('Path', $new, 'User');",
        bin_s.replace('\'', "''")
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .output()
        .map_err(|e| format!("PATH update failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "PATH update failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_cli_install(cli_script: &Path, legacy_shim: &Option<PathBuf>) -> Result<String, String> {
    let home = windows_k2_home()?;
    let cli_dir = home.join("cli");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&cli_dir).map_err(|e| format!("mkdir {}: {e}", cli_dir.display()))?;
    fs::create_dir_all(&bin_dir).map_err(|e| format!("mkdir {}: {e}", bin_dir.display()))?;

    let dest_script = cli_dir.join("k2");
    fs::copy(cli_script, &dest_script)
        .map_err(|e| format!("copy CLI script to {}: {e}", dest_script.display()))?;
    if let Some(shim) = legacy_shim {
        let _ = fs::copy(shim, cli_dir.join("k2so"));
    }

    let cmd_path = bin_dir.join("k2.cmd");
    write_windows_cli_cmd(&cmd_path, &dest_script)?;
    if legacy_shim.is_some() {
        let shim_script = cli_dir.join("k2so");
        if shim_script.exists() {
            let _ = write_windows_cli_cmd(&bin_dir.join("k2so.cmd"), &shim_script);
        }
    }

    ensure_windows_user_path_has(&bin_dir)?;
    Ok(cmd_path.display().to_string())
}

#[cfg(windows)]
fn windows_cli_uninstall() -> Result<(), String> {
    let home = windows_k2_home()?;
    let bin_dir = home.join("bin");
    let _ = fs::remove_file(bin_dir.join("k2.cmd"));
    let _ = fs::remove_file(bin_dir.join("k2so.cmd"));
    // Leave PATH entry (harmless empty dir); removing PATH entries is racy.
    let _ = fs::remove_dir_all(home.join("cli"));
    Ok(())
}

/// True when the on-PATH CLI install should be (re)created at boot.
///
/// **macOS:** `/usr/local/bin/k2` missing, broken, or pointing at a different
/// bundle (K2SO→K2 rename left a dangling target → CLI Version reads `v?`).
///
/// **Windows:** `%LOCALAPPDATA%\K2\bin\k2.cmd` missing or its copied script
/// is gone. Silent heal only (no admin); Install CLI in Settings remains
/// the explicit path when PATH wasn't updated yet.
///
/// Returns false when there's no bundled CLI, or the running app is in a
/// transient location (DMG mount / Gatekeeper translocation).
pub(crate) fn cli_symlink_needs_heal() -> bool {
    let Some(bundled) = find_cli_script() else {
        return false;
    };
    if k2_core::daemon_lifecycle::is_transient_exe_location(&bundled) {
        return false;
    }
    #[cfg(windows)]
    {
        let Ok(cmd) = windows_cli_cmd_path() else {
            return false;
        };
        if !cmd.exists() {
            return true;
        }
        let script = windows_k2_home()
            .map(|h| h.join("cli").join("k2"))
            .unwrap_or_default();
        return !script.exists();
    }
    #[cfg(not(windows))]
    {
        let new_cli = Path::new(CLI_SYMLINK_PATH);
        match fs::read_link(new_cli) {
            // It's a symlink. Heal only if its target is gone, or it points at a
            // GENUINELY different bundle. Compare CANONICALIZED paths: macOS
            // firmlinks (`/Applications` ↔ `/System/Volumes/Data/Applications`)
            // and realpath resolution make two equivalent paths compare unequal,
            // which used to re-trigger a (false) heal — and an admin prompt —
            // on every single launch. Canonicalizing both sides makes the check
            // idempotent: a correctly-installed symlink reports no heal needed.
            Ok(target) => symlink_target_needs_heal(&target, &bundled),
            // Not a symlink: heal if absent, or a legacy k2so symlink exists
            // (pre-0.40 install that still needs its `k2` sibling).
            Err(_) => !new_cli.exists() || Path::new(CLI_LEGACY_SYMLINK_PATH).is_symlink(),
        }
    }
}

/// Decide whether an existing `/usr/local/bin/k2` symlink (pointing at
/// `target`) needs healing given the currently-running app's `bundled` CLI
/// path. Pure + testable. Canonicalizes BOTH sides so macOS firmlink /
/// realpath-equivalent paths (`/Applications` vs
/// `/System/Volumes/Data/Applications`) don't compare unequal and loop a false
/// heal every launch (#56). A target that fails to canonicalize is broken
/// (its file is gone) → heal.
fn symlink_target_needs_heal(target: &Path, bundled: &Path) -> bool {
    match (fs::canonicalize(target), fs::canonicalize(bundled)) {
        (Ok(t), Ok(b)) => t != b,
        _ => true,
    }
}

/// Extract the CLI version from a k2/k2so CLI script. Accepts both the
/// 0.40.0 `K2_CLI_VERSION` and the legacy `K2SO_CLI_VERSION` prefixes so
/// the installed-version probe works across the rename boundary.
fn read_cli_version(script_path: &Path) -> Option<String> {
    let content = fs::read_to_string(script_path).ok()?;
    for line in content.lines().take(20) {
        for prefix in ["K2_CLI_VERSION=", "K2SO_CLI_VERSION="] {
            if let Some(rest) = line.strip_prefix(prefix) {
                return Some(rest.trim_matches('"').to_string());
            }
        }
    }
    None
}

#[tauri::command]
pub fn cli_install_status() -> Result<serde_json::Value, String> {
    #[cfg(windows)]
    {
        let cmd = windows_cli_cmd_path().ok();
        let installed = cmd.as_ref().map(|p| p.exists()).unwrap_or(false);
        let script = windows_k2_home().ok().map(|h| h.join("cli").join("k2"));
        let bundled = find_cli_script();
        let bundled_path = bundled.as_ref().map(|p| p.to_string_lossy().to_string());
        let bundled_version = bundled.as_ref().and_then(|p| read_cli_version(p));
        let installed_version = if installed {
            script.as_ref().and_then(|p| read_cli_version(p))
        } else {
            None
        };
        let update_available = match (&bundled_version, &installed_version) {
            (Some(bundled_v), Some(installed_v)) => {
                let bv: Vec<u32> = bundled_v.split('.').filter_map(|s| s.parse().ok()).collect();
                let iv: Vec<u32> = installed_v.split('.').filter_map(|s| s.parse().ok()).collect();
                bv > iv
            }
            _ => false,
        };
        return Ok(serde_json::json!({
            "installed": installed,
            "symlinkPath": cmd.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            "target": script.as_ref().map(|p| p.display().to_string()),
            "bundledPath": bundled_path,
            "bundledVersion": bundled_version,
            "installedVersion": installed_version,
            "updateAvailable": update_available,
        }));
    }
    #[cfg(not(windows))]
    {
        let symlink_path = Path::new(CLI_SYMLINK_PATH);
        let installed = symlink_path.exists() || symlink_path.is_symlink();
        let target = if installed {
            fs::read_link(symlink_path)
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };
        let bundled = find_cli_script();
        let bundled_path = bundled.as_ref().map(|p| p.to_string_lossy().to_string());

        // Read version from bundled CLI (current app version)
        let bundled_version = bundled.as_ref().and_then(|p| read_cli_version(p));

        // Read version from installed CLI (what's on PATH)
        let installed_version = if installed {
            // Read from the actual target, not the symlink
            let actual_path =
                fs::read_link(symlink_path).unwrap_or_else(|_| symlink_path.to_path_buf());
            read_cli_version(&actual_path)
        } else {
            None
        };

        // Determine if an update is available (bundled must be strictly newer)
        let update_available = match (&bundled_version, &installed_version) {
            (Some(bundled_v), Some(installed_v)) => {
                let bv: Vec<u32> = bundled_v.split('.').filter_map(|s| s.parse().ok()).collect();
                let iv: Vec<u32> = installed_v.split('.').filter_map(|s| s.parse().ok()).collect();
                bv > iv
            }
            _ => false,
        };

        Ok(serde_json::json!({
            "installed": installed,
            "symlinkPath": CLI_SYMLINK_PATH,
            "target": target,
            "bundledPath": bundled_path,
            "bundledVersion": bundled_version,
            "installedVersion": installed_version,
            "updateAvailable": update_available,
        }))
    }
}

/// Make the bundled CLI scripts executable (best-effort).
fn ensure_cli_executable(cli_script: &Path, legacy_shim: &Option<PathBuf>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(cli_script, fs::Permissions::from_mode(0o755));
        if let Some(shim) = legacy_shim {
            let _ = fs::set_permissions(shim, fs::Permissions::from_mode(0o755));
        }
    }
}

/// Try to (re)create the `k2` (+ `k2so` alias) symlinks in /usr/local/bin
/// WITHOUT any admin prompt. Works only when /usr/local/bin already exists and
/// is user-writable (Intel Homebrew, or a chowned prefix). Returns true iff the
/// `k2` symlink was created. NEVER spawns osascript — callers that run at boot
/// (the auto-heal) must not throw a password dialog.
///
/// On Windows, performs the user-local `%LOCALAPPDATA%\K2` install (no admin).
fn try_direct_cli_symlink(cli_script: &Path, legacy_shim: &Option<PathBuf>) -> bool {
    #[cfg(windows)]
    {
        return windows_cli_install(cli_script, legacy_shim).is_ok();
    }
    #[cfg(not(windows))]
    {
        let symlink_path = Path::new(CLI_SYMLINK_PATH);
        // Can't create the parent dir without admin — defer silently.
        match symlink_path.parent() {
            Some(bin_dir) if bin_dir.exists() => {}
            _ => return false,
        }
        let _ = fs::remove_file(symlink_path);
        #[cfg(unix)]
        {
            if std::os::unix::fs::symlink(cli_script, symlink_path).is_ok() {
                if let Some(shim) = legacy_shim {
                    let legacy_path = Path::new(CLI_LEGACY_SYMLINK_PATH);
                    let _ = fs::remove_file(legacy_path);
                    let _ = std::os::unix::fs::symlink(shim, legacy_path);
                }
                return true;
            }
        }
        false
    }
}

/// Boot-time CLI heal — SILENT, never prompts. Tries the direct (non-admin)
/// install; if macOS /usr/local/bin needs admin to write, returns `Ok(false)`
/// and leaves the user to install from Settings → Install CLI (the ONE place
/// an admin prompt is allowed). This is why a routine app update no longer
/// fires a surprise "enter your password" dialog on every launch (#56 /
/// 0.40.10): the only password prompt is now a deliberate user click in
/// Settings. Windows heal never needs admin.
pub(crate) fn cli_heal_silent() -> Result<bool, String> {
    let cli_script =
        find_cli_script().ok_or_else(|| "CLI script not found in app bundle".to_string())?;
    let legacy_shim = find_cli_script_named("k2so");
    ensure_cli_executable(&cli_script, &legacy_shim);
    Ok(try_direct_cli_symlink(&cli_script, &legacy_shim))
}

#[tauri::command]
pub fn cli_install() -> Result<String, String> {
    let cli_script =
        find_cli_script().ok_or_else(|| "CLI script not found in app bundle".to_string())?;
    // The k2so deprecation shim ships alongside; best-effort (an old
    // bundle without it just skips the legacy alias).
    let legacy_shim = find_cli_script_named("k2so");
    ensure_cli_executable(&cli_script, &legacy_shim);

    #[cfg(windows)]
    {
        return windows_cli_install(&cli_script, &legacy_shim);
    }

    #[cfg(not(windows))]
    {
        let symlink_path = Path::new(CLI_SYMLINK_PATH);

        // Check if /usr/local/bin exists and is writable
        let bin_dir = symlink_path.parent().unwrap();
        if !bin_dir.exists() {
            // Try to create /usr/local/bin via osascript (prompts for password)
            let output = std::process::Command::new("osascript")
                .args([
                    "-e",
                    &format!(
                        "do shell script \"mkdir -p {}\" with prompt \"K2 needs to create /usr/local/bin to install its command-line tool.\" with administrator privileges",
                        bin_dir.display()
                    ),
                ])
                .output()
                .map_err(|e| format!("Failed to create {}: {}", bin_dir.display(), e))?;
            if !output.status.success() {
                return Err(format!(
                    "Failed to create {}: {}",
                    bin_dir.display(),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        // Try direct symlinks first (works if user owns /usr/local/bin) — no prompt.
        // 0.40.0: install BOTH `k2` (the CLI) and `k2so` (deprecation shim).
        if try_direct_cli_symlink(&cli_script, &legacy_shim) {
            return Ok(CLI_SYMLINK_PATH.to_string());
        }

        // Fall back to osascript with admin privileges — both links in ONE
        // prompt. This is the ONLY admin prompt path; it only runs from an
        // explicit Settings → Install CLI click, never the boot auto-heal.
        let legacy_ln = legacy_shim
            .as_ref()
            .map(|shim| format!(" && ln -sf '{}' '{}'", shim.display(), CLI_LEGACY_SYMLINK_PATH))
            .unwrap_or_default();
        let script = format!(
            "do shell script \"ln -sf '{}' '{}'{}\" with prompt \"K2 needs to install the k2 command-line tool (and the k2so compatibility alias) in /usr/local/bin.\" with administrator privileges",
            cli_script.display(),
            CLI_SYMLINK_PATH,
            legacy_ln
        );
        let output = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
            .map_err(|e| format!("Failed to create symlink: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to install CLI: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(CLI_SYMLINK_PATH.to_string())
    }
}

#[tauri::command]
pub fn cli_uninstall() -> Result<(), String> {
    #[cfg(windows)]
    {
        return windows_cli_uninstall();
    }
    #[cfg(not(windows))]
    {
        let symlink_path = Path::new(CLI_SYMLINK_PATH);
        if !symlink_path.exists() && !symlink_path.is_symlink() {
            return Ok(());
        }

        // Try direct remove first
        if fs::remove_file(symlink_path).is_ok() {
            return Ok(());
        }

        // Fall back to osascript with admin privileges
        let script = format!(
            "do shell script \"rm -f '{}'\" with prompt \"K2 needs to remove its command-line tool from /usr/local/bin.\" with administrator privileges",
            CLI_SYMLINK_PATH
        );
        let output = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
            .map_err(|e| format!("Failed to remove symlink: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to uninstall CLI: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }
}

/// Signal that the app is about to relaunch (skip _exit in close handler).
#[tauri::command]
pub fn set_relaunch_mode() {
    crate::RELAUNCH_MODE.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Relaunch the app via a helper script that waits for this process to die,
/// then opens the .app bundle cleanly. This avoids:
/// 1. Two dock icons (old process still alive when new one launches)
/// 2. Metal SIGABRT from std::process::exit() running __cxa_finalize_ranges
/// 3. Tauri's built-in relaunch spawning a bare binary (not a .app bundle)
#[tauri::command]
pub fn relaunch_via_open(_app: AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let pid = std::process::id();
        // Get the .app bundle path: binary is at K2SO.app/Contents/MacOS/k2so
        if let Ok(exe) = std::env::current_exe() {
            if let Some(app_bundle) = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
                let bundle_path = app_bundle.display().to_string();
                let script = format!(
                    "#!/bin/bash\n\
                     # K2SO relaunch helper — waits for old process to exit, then reopens\n\
                     while kill -0 {pid} 2>/dev/null; do sleep 0.2; done\n\
                     # 1.5s (was 0.5s): the old instance's WebKit helper\n\
                     # processes can outlive the main PID briefly; launching\n\
                     # into that window risks a webview that never commits\n\
                     # its first navigation (black screen after self-update).\n\
                     sleep 1.5\n\
                     open -a \"{bundle_path}\"\n\
                     rm -f \"$0\"\n"
                );

                let script_path = format!("/tmp/k2so-relaunch-{pid}.sh");
                if std::fs::write(&script_path, &script).is_ok() {
                    let _ = std::fs::set_permissions(
                        &script_path,
                        std::os::unix::fs::PermissionsExt::from_mode(0o755),
                    );
                    log_debug!("[relaunch] Helper script: {script_path}, waiting for PID {pid}");
                    // Spawn detached — inherits no stdin/stdout, won't be killed with us
                    let _ = std::process::Command::new("/bin/bash")
                        .arg(&script_path)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
            }
        }
    }
    // Now exit hard — _exit skips Metal destructor crash, helper script handles relaunch.
    // libc::_exit is unix-shaped; Windows uses process::exit (no Metal teardown race).
    #[cfg(unix)]
    unsafe {
        libc::_exit(0);
    }
    #[cfg(not(unix))]
    std::process::exit(0);
}

/// Set the macOS window close button dot (document edited indicator).
#[tauri::command]
#[allow(unexpected_cfgs)]
pub fn set_document_edited(app: AppHandle, edited: bool) -> Result<(), String> {
    // Non-mac: no NSWindow document-edited dot; params intentionally idle.
    #[cfg(not(target_os = "macos"))]
    let _ = (&app, edited);
    #[cfg(target_os = "macos")]
    {
        let app_clone = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(window) = app_clone.get_webview_window("main") {
                let _ = window.with_webview(move |webview| {
                    unsafe {
                        let wk: *mut std::ffi::c_void = webview.inner() as _;
                        let ns_window: *mut std::ffi::c_void = msg_send![wk as *mut objc::runtime::Object, window];
                        if !ns_window.is_null() {
                            let _: () = msg_send![ns_window as *mut objc::runtime::Object, setDocumentEdited: edited];
                        }
                    }
                });
            }
        });
    }
    Ok(())
}

#[cfg(test)]
mod cli_heal_tests {
    use super::symlink_target_needs_heal;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    // Unique scratch dir per test (real FS — we exercise fs::canonicalize).
    fn scratch() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("k2-cliheal-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn no_heal_when_target_equals_bundled() {
        let d = scratch();
        let cli = d.join("k2");
        fs::write(&cli, "#!/bin/sh\n").unwrap();
        // Symlink points at exactly the bundled path → no heal.
        assert!(!symlink_target_needs_heal(&cli, &cli));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn no_heal_when_paths_are_symlink_equivalent() {
        // The macOS firmlink case: target reaches the SAME real file by a
        // different (symlinked) path. canonicalize() must collapse them so we
        // don't loop a false heal (+ admin prompt) every launch.
        let d = scratch();
        let real = d.join("real");
        fs::create_dir_all(&real).unwrap();
        let cli = real.join("k2");
        fs::write(&cli, "#!/bin/sh\n").unwrap();
        let link = d.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        // target via the symlinked dir; bundled via the real dir.
        let target = link.join("k2");
        assert!(!symlink_target_needs_heal(&target, &cli));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn heal_when_target_is_broken() {
        let d = scratch();
        let bundled = d.join("k2");
        fs::write(&bundled, "#!/bin/sh\n").unwrap();
        let gone = d.join("deleted-app-k2"); // never created
        assert!(symlink_target_needs_heal(&gone, &bundled));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn heal_when_target_points_at_different_file() {
        let d = scratch();
        let bundled = d.join("new-k2");
        fs::write(&bundled, "new\n").unwrap();
        let other = d.join("old-k2");
        fs::write(&other, "old\n").unwrap();
        assert!(symlink_target_needs_heal(&other, &bundled));
        let _ = fs::remove_dir_all(&d);
    }
}
