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
use tauri::{AppHandle, Manager};

// ── CLI Install (HOST: writes /usr/local/bin/k2so symlink) ──────────────

/// Find a bundled cli/<name> script (production or development).
/// 0.40.0: the CLI is `k2`; `k2so` remains as a deprecation shim that
/// delegates to `k2` — both ship in cli/ and both get symlinked.
fn find_cli_script_named(name: &str) -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let macos_dir = exe_path.parent()?;

    // Production: K2.app/Contents/MacOS/<bin> → Contents/Resources/_up_/cli/<name>
    // Tauri puts "../cli/*" resources under Resources/_up_/cli/
    let resources_cli = macos_dir.parent()
        .map(|contents| contents.join("Resources").join("_up_").join("cli").join(name));
    if let Some(ref p) = resources_cli {
        if p.exists() { return resources_cli; }
    }

    // Development: target/debug/<bin> → ../../cli/<name> from repo root
    let dev_cli = macos_dir.parent()       // target/
        .and_then(|p| p.parent())          // src-tauri/ (or repo root for workspace target)
        .and_then(|p| p.parent())          // repo root
        .map(|repo| repo.join("cli").join(name));
    if let Some(ref p) = dev_cli {
        if p.exists() { return dev_cli; }
    }

    None
}

fn find_cli_script() -> Option<PathBuf> {
    find_cli_script_named("k2")
}

const CLI_SYMLINK_PATH: &str = "/usr/local/bin/k2";
/// Legacy alias — points at the cli/k2so deprecation shim.
const CLI_LEGACY_SYMLINK_PATH: &str = "/usr/local/bin/k2so";

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
    let symlink_path = Path::new(CLI_SYMLINK_PATH);
    let installed = symlink_path.exists() || symlink_path.is_symlink();
    let target = if installed {
        fs::read_link(symlink_path).ok().map(|p| p.to_string_lossy().to_string())
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
        let actual_path = fs::read_link(symlink_path).unwrap_or_else(|_| symlink_path.to_path_buf());
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

#[tauri::command]
pub fn cli_install() -> Result<String, String> {
    let cli_script = find_cli_script()
        .ok_or_else(|| "CLI script not found in app bundle".to_string())?;
    // The k2so deprecation shim ships alongside; best-effort (an old
    // bundle without it just skips the legacy alias).
    let legacy_shim = find_cli_script_named("k2so");

    // Ensure the scripts are executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&cli_script, fs::Permissions::from_mode(0o755));
        if let Some(ref shim) = legacy_shim {
            let _ = fs::set_permissions(shim, fs::Permissions::from_mode(0o755));
        }
    }

    let symlink_path = Path::new(CLI_SYMLINK_PATH);

    // Check if /usr/local/bin exists and is writable
    let bin_dir = symlink_path.parent().unwrap();
    if !bin_dir.exists() {
        // Try to create /usr/local/bin via osascript (prompts for password)
        let output = std::process::Command::new("osascript")
            .args(["-e", &format!(
                "do shell script \"mkdir -p {}\" with administrator privileges",
                bin_dir.display()
            )])
            .output()
            .map_err(|e| format!("Failed to create {}: {}", bin_dir.display(), e))?;
        if !output.status.success() {
            return Err(format!("Failed to create {}: {}", bin_dir.display(),
                String::from_utf8_lossy(&output.stderr)));
        }
    }

    // Try direct symlinks first (works if user owns /usr/local/bin).
    // 0.40.0: install BOTH `k2` (the CLI) and `k2so` (deprecation shim).
    let _ = fs::remove_file(symlink_path);
    let legacy_path = Path::new(CLI_LEGACY_SYMLINK_PATH);
    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(&cli_script, symlink_path).is_ok() {
            if let Some(ref shim) = legacy_shim {
                let _ = fs::remove_file(legacy_path);
                let _ = std::os::unix::fs::symlink(shim, legacy_path);
            }
            return Ok(CLI_SYMLINK_PATH.to_string());
        }
    }

    // Fall back to osascript with admin privileges — both links in ONE
    // prompt.
    let legacy_ln = legacy_shim
        .as_ref()
        .map(|shim| format!(" && ln -sf '{}' '{}'", shim.display(), CLI_LEGACY_SYMLINK_PATH))
        .unwrap_or_default();
    let script = format!(
        "do shell script \"ln -sf '{}' '{}'{}\" with administrator privileges",
        cli_script.display(),
        CLI_SYMLINK_PATH,
        legacy_ln
    );
    let output = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("Failed to create symlink: {}", e))?;

    if !output.status.success() {
        return Err(format!("Failed to install CLI: {}",
            String::from_utf8_lossy(&output.stderr)));
    }

    Ok(CLI_SYMLINK_PATH.to_string())
}

#[tauri::command]
pub fn cli_uninstall() -> Result<(), String> {
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
        "do shell script \"rm -f '{}'\" with administrator privileges",
        CLI_SYMLINK_PATH
    );
    let output = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("Failed to remove symlink: {}", e))?;

    if !output.status.success() {
        return Err(format!("Failed to uninstall CLI: {}",
            String::from_utf8_lossy(&output.stderr)));
    }

    Ok(())
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
                     sleep 0.5\n\
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
    // Now exit hard — _exit skips Metal destructor crash, helper script handles relaunch
    unsafe { libc::_exit(0); }
}

/// Set the macOS window close button dot (document edited indicator).
#[tauri::command]
#[allow(unexpected_cfgs)]
pub fn set_document_edited(app: AppHandle, edited: bool) -> Result<(), String> {
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
