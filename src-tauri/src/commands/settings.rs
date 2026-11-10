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
    // Linux .deb / .rpm / AppImage: the exe is `<prefix>/bin/k2` and the
    // resources `<prefix>/lib/K2/_up_/cli/`.
    //
    // 0.46.11: only a script counts. On the .deb the exe ITSELF is
    // `/usr/bin/k2` (the GUI), so `exe_dir/k2` used to "find" the GUI
    // binary as the CLI.
    let usr_lib = exe_dir.parent().map(|prefix| prefix.join("lib"));
    let mut candidates = vec![
        exe_dir.join("cli").join(name),
        exe_dir.join(name),
        exe_dir.join("resources").join("cli").join(name),
        exe_dir.join("resources").join(name),
        exe_dir.join("_up_").join("cli").join(name),
    ];
    if let Some(lib) = usr_lib {
        candidates.push(lib.join("K2").join("_up_").join("cli").join(name));
        candidates.push(lib.join("k2").join("_up_").join("cli").join(name));
    }
    for p in candidates {
        if p != exe_path && is_script(&p) {
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

/// A file that starts with `#!` (the bundled `k2` / `k2so` are bash
/// scripts; a binary that shares the name is not one).
fn is_script(p: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 2];
    fs::File::open(p)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|_| &head == b"#!")
}

/// macOS install path for the `k2` symlink.
#[cfg(all(not(windows), not(target_os = "linux")))]
const CLI_SYMLINK_PATH: &str = "/usr/local/bin/k2";
/// Legacy alias — points at the cli/k2so deprecation shim.
#[cfg(all(not(windows), not(target_os = "linux")))]
const CLI_LEGACY_SYMLINK_PATH: &str = "/usr/local/bin/k2so";

/// Distro packages (Arch pacman) install the CLI here. Settings must not
/// write a second copy over it.
#[cfg(any(target_os = "linux", test))]
const DISTRO_CLI_PATH: &str = "/usr/bin/k2";

/// Login PATH order on Linux. `/usr/local/bin` is ahead of `/usr/bin`,
/// which is ahead of `~/.local/bin`. The first file that exists is the
/// one `k2` runs.
#[cfg(any(target_os = "linux", test))]
fn linux_cli_search_paths(home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = vec![
        PathBuf::from("/usr/local/bin/k2"),
        PathBuf::from(DISTRO_CLI_PATH),
    ];
    if let Some(home) = home {
        if !home.as_os_str().is_empty() {
            paths.push(home.join(".local/bin/k2"));
        }
    }
    paths
}

/// The first of `paths` that is the K2 CLI script (a `#!` file with a
/// version line, links followed). 0.46.11: the .deb puts the GUI binary at
/// `/usr/bin/k2`, which used to read as "installed by the package" and
/// left the person with a `k2` that opens the app.
#[cfg(any(target_os = "linux", test))]
fn first_k2_cli(paths: &[PathBuf]) -> Option<PathBuf> {
    paths
        .iter()
        .find(|p| is_script(p) && read_cli_version(p).is_some())
        .cloned()
}

#[cfg(any(target_os = "linux", test))]
fn cli_is_distro_package(path: &Path) -> bool {
    path == Path::new(DISTRO_CLI_PATH)
}

#[cfg(any(target_os = "linux", test))]
fn copy_cli_script(src: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::copy(src, dest).map_err(|e| format!("copy CLI to {}: {e}", dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dest, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod {}: {e}", dest.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

#[cfg(target_os = "linux")]
fn linux_installed_cli() -> Option<PathBuf> {
    let home = linux_home();
    first_k2_cli(&linux_cli_search_paths(home.as_deref()))
}

#[cfg(target_os = "linux")]
fn linux_user_cli_dest() -> Result<PathBuf, String> {
    let home = linux_home().ok_or_else(|| "HOME is not set".to_string())?;
    Ok(home.join(".local/bin/k2"))
}

/// Copy the bundled CLI (and the k2so shim, when present) onto `dest`.
#[cfg(target_os = "linux")]
fn linux_write_cli(
    dest: &Path,
    cli_script: &Path,
    legacy_shim: &Option<PathBuf>,
) -> Result<(), String> {
    copy_cli_script(cli_script, dest)?;
    if let (Some(shim), Some(parent)) = (legacy_shim.as_ref(), dest.parent()) {
        let _ = copy_cli_script(shim, &parent.join("k2so"));
    }
    Ok(())
}

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
    let body = k2_client::win_cli::cmd_body(&script);
    if let Some(parent) = cmd_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::write(cmd_path, body).map_err(|e| format!("write {}: {e}", cmd_path.display()))
}

/// Ensure `%LOCALAPPDATA%\K2\bin` is on the *user* PATH (no admin).
#[cfg(windows)]
fn ensure_windows_user_path_has(bin_dir: &Path) -> Result<(), String> {
    let bin = dunce::canonicalize(&bin_dir)
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
    let out = k2_client::bg_proc::command("powershell")
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
    // `k2 zen widget preview --shot` runs the Playwright runner that sits
    // next to the script.
    if let Some(runner) = cli_script.parent().map(|d| d.join("k2-widget-shot.mjs")).filter(|p| p.is_file()) {
        fs::copy(&runner, cli_dir.join("k2-widget-shot.mjs"))
            .map_err(|e| format!("copy the widget-shot runner to {}: {e}", cli_dir.display()))?;
    }
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

/// True when the on-PATH CLI install should be (re)created at boot
/// (Windows and Linux; macOS runs `k2_client::cli_install::boot_heal`,
/// which also runs in the daemon).
///
/// **Windows:** `%LOCALAPPDATA%\K2\bin\k2.cmd` missing or its copied script
/// is gone. Silent heal only (no admin); Install CLI in Settings remains
/// the explicit path when PATH wasn't updated yet.
///
/// **Linux:** no K2 CLI script at `/usr/local/bin`, `/usr/bin` or
/// `~/.local/bin`.
///
/// Returns false when there's no bundled CLI, or the running app is in a
/// transient location (DMG mount / Gatekeeper translocation).
#[cfg(any(windows, target_os = "linux"))]
pub(crate) fn cli_symlink_needs_heal() -> bool {
    let Some(bundled) = find_cli_script() else {
        return false;
    };
    if k2_client::daemon_lifecycle::is_transient_exe_location(&bundled) {
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
    #[cfg(target_os = "linux")]
    {
        // A distro package or an earlier daemon stage already put `k2` on
        // PATH. Do not add `/usr/local/bin/k2` in front of `/usr/bin/k2`.
        linux_installed_cli().is_none()
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

fn version_is_newer(bundled: Option<&str>, installed: Option<&str>) -> bool {
    match (bundled, installed) {
        (Some(bundled_v), Some(installed_v)) => {
            let bv: Vec<u32> = bundled_v.split('.').filter_map(|s| s.parse().ok()).collect();
            let iv: Vec<u32> = installed_v.split('.').filter_map(|s| s.parse().ok()).collect();
            bv > iv
        }
        _ => false,
    }
}

/// PATH facts for Settings (macOS and Linux): which startup file K2's line
/// goes in, whether it is there, and whether a new Terminal window finds
/// `k2`. Probes the login shell, so callers run off the main thread.
#[cfg(unix)]
fn path_status() -> Option<k2_client::cli_install::PathStatus> {
    use k2_client::cli_install as ci;
    let home = k2_client::paths::home_dir()?;
    let shell = ci::user_shell();
    let login = k2_client::process::login_shell_path(&shell);
    let zdot = ci::zdotdir();
    Some(ci::path_status_at(&home, &shell, zdot.as_deref(), login.as_deref()))
}

/// The explicit (Settings) PATH line on Linux, once the CLI copy is at
/// `~/.local/bin/k2`. Any other place is already on PATH.
#[cfg(target_os = "linux")]
fn linux_path_line(dest: &Path) -> Result<(), String> {
    use k2_client::cli_install as ci;
    let Some(home) = linux_home() else { return Ok(()) };
    if dest != ci::user_bin_dir(&home).join("k2") {
        return Ok(());
    }
    let shell = ci::user_shell();
    let login = k2_client::process::login_shell_path(&shell);
    let zdot = ci::zdotdir();
    let outcome = ci::ensure_path_block(&home, &shell, zdot.as_deref(), login.as_deref(), true)?;
    eprintln!("[cli] PATH line: {outcome:?}");
    Ok(())
}

/// macOS: the no-admin install (`~/.local/bin/k2`, `/usr/local/bin/k2`
/// when writable, the PATH line). `force_path`: Settings' explicit install.
#[cfg(all(not(windows), not(target_os = "linux")))]
fn mac_heal(force_path: bool) -> Result<k2_client::cli_install::HealReport, String> {
    use k2_client::cli_install as ci;
    let cli = find_cli_script().ok_or_else(|| "CLI script not found in app bundle".to_string())?;
    let shim = find_cli_script_named("k2so");
    ensure_cli_executable(&cli, &shim);
    let shell = ci::user_shell();
    let login = k2_client::process::login_shell_path(&shell);
    let zdot = ci::zdotdir();
    ci::heal(&ci::HealInput {
        cli: &cli,
        legacy_shim: shim.as_deref(),
        system_bin: Some(Path::new(ci::SYSTEM_BIN)),
        shell: &shell,
        zdotdir: zdot.as_deref(),
        login_path: login.as_deref(),
        force_path,
    })
}

/// Where `k2` is installed and whether a new Terminal window finds it.
/// Probes the login shell (a second or two), so it runs off the main thread.
#[tauri::command]
pub async fn cli_install_status() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(cli_install_status_now)
        .await
        .map_err(|e| format!("read the k2 command's status: {e}"))?
}

fn cli_install_status_now() -> Result<serde_json::Value, String> {
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
        let update_available =
            version_is_newer(bundled_version.as_deref(), installed_version.as_deref());
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
    #[cfg(target_os = "linux")]
    {
        let bundled = find_cli_script();
        let bundled_path = bundled.as_ref().map(|p| p.to_string_lossy().to_string());
        let bundled_version = bundled.as_ref().and_then(|p| read_cli_version(p));
        let found = linux_installed_cli();
        let package_managed = found.as_ref().is_some_and(|p| cli_is_distro_package(p));
        let installed_version = found.as_ref().and_then(|p| {
            let actual = dunce::canonicalize(&p).unwrap_or_else(|_| p.clone());
            read_cli_version(&actual)
        });
        let update_available = if package_managed {
            false
        } else {
            version_is_newer(bundled_version.as_deref(), installed_version.as_deref())
        };
        return Ok(serde_json::json!({
            "installed": found.is_some(),
            "packageManaged": package_managed,
            "symlinkPath": found.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            "target": found.as_ref().map(|p| p.display().to_string()),
            "bundledPath": bundled_path,
            "bundledVersion": bundled_version,
            "installedVersion": installed_version,
            "updateAvailable": update_available,
            "path": path_status(),
        }));
    }
    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        use k2_client::cli_install as ci;
        let bundled = find_cli_script();
        let bundled_path = bundled.as_ref().map(|p| p.to_string_lossy().to_string());
        let bundled_version = bundled.as_ref().and_then(|p| read_cli_version(p));

        let user_link_path = k2_client::paths::home_dir().map(|h| ci::user_bin_dir(&h).join("k2"));
        let system_link_path = Path::new(CLI_SYMLINK_PATH);
        let state_of = |link: &Path| bundled.as_deref().map(|b| ci::link_state(link, b));
        let user_link = user_link_path.as_deref().and_then(state_of);
        let system_link = state_of(system_link_path);
        let path = path_status();

        // What a new Terminal window runs, else one of K2's own links.
        let installed_at: Option<PathBuf> = path
            .as_ref()
            .and_then(|p| p.new_terminals_run.clone())
            .or_else(|| {
                (user_link == Some(ci::LinkState::Current)).then(|| user_link_path.clone()).flatten()
            })
            .or_else(|| {
                (system_link == Some(ci::LinkState::Current)).then(|| system_link_path.to_path_buf())
            });
        let installed = installed_at.is_some();
        let target = installed_at
            .as_ref()
            .and_then(|p| fs::read_link(p).ok())
            .map(|p| p.to_string_lossy().to_string());
        let installed_version = installed_at.as_ref().and_then(|p| {
            let actual = dunce::canonicalize(p).unwrap_or_else(|_| p.clone());
            read_cli_version(&actual)
        });
        let update_available =
            version_is_newer(bundled_version.as_deref(), installed_version.as_deref());

        Ok(serde_json::json!({
            "installed": installed,
            "symlinkPath": installed_at
                .as_ref()
                .or(user_link_path.as_ref())
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            "target": target,
            "bundledPath": bundled_path,
            "bundledVersion": bundled_version,
            "installedVersion": installed_version,
            "updateAvailable": update_available,
            "userLinkPath": user_link_path.as_ref().map(|p| p.display().to_string()),
            "userLink": user_link,
            "systemLinkPath": CLI_SYMLINK_PATH,
            "systemLink": system_link,
            "path": path,
        }))
    }
}

/// Make the bundled CLI scripts executable (best-effort).
fn ensure_cli_executable(cli_script: &Path, legacy_shim: &Option<PathBuf>) {
    // Windows has no exec bit: a script runs by its interpreter.
    #[cfg(not(unix))]
    let _ = (cli_script, legacy_shim);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(cli_script, fs::Permissions::from_mode(0o755));
        if let Some(shim) = legacy_shim {
            let _ = fs::set_permissions(shim, fs::Permissions::from_mode(0o755));
        }
    }
}

/// Boot-time CLI heal on Windows and Linux — SILENT, never prompts.
/// Windows: the user-local `%LOCALAPPDATA%\K2` install (no admin). Linux:
/// a copy at `~/.local/bin/k2` when no K2 CLI is on the search path, plus
/// the PATH line when the login PATH lacks `~/.local/bin`. macOS runs
/// `k2_client::cli_install::boot_heal` instead (`~/.local/bin/k2` + the
/// PATH line, never an admin prompt; #56 / 0.40.10 / 0.46.11).
#[cfg(any(windows, target_os = "linux"))]
pub(crate) fn cli_heal_silent() -> Result<bool, String> {
    let cli_script =
        find_cli_script().ok_or_else(|| "CLI script not found in app bundle".to_string())?;
    let legacy_shim = find_cli_script_named("k2so");
    ensure_cli_executable(&cli_script, &legacy_shim);
    #[cfg(windows)]
    {
        return Ok(windows_cli_install(&cli_script, &legacy_shim).is_ok());
    }
    #[cfg(target_os = "linux")]
    {
        if linux_installed_cli().is_some() {
            return Ok(false);
        }
        let dest = linux_user_cli_dest()?;
        linux_write_cli(&dest, &cli_script, &legacy_shim)?;
        if let Some(Err(e)) = k2_client::cli_install::boot_path_for_staged(&dest) {
            eprintln!("[boot] CLI PATH line: {e}");
        }
        Ok(true)
    }
}

/// Install `k2` with no admin prompt: macOS links `~/.local/bin/k2` (and
/// `/usr/local/bin/k2` when that folder is writable) and adds the PATH
/// line; Linux copies to `~/.local/bin/k2` (or refreshes the copy it
/// finds) and adds the PATH line; Windows copies to `%LOCALAPPDATA%\K2`.
#[tauri::command]
pub async fn cli_install() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(cli_install_now)
        .await
        .map_err(|e| format!("install the k2 command: {e}"))?
}

fn cli_install_now() -> Result<String, String> {
    #[cfg(windows)]
    {
        let cli_script =
            find_cli_script().ok_or_else(|| "CLI script not found in app bundle".to_string())?;
        // The k2so deprecation shim ships alongside; best-effort (an old
        // bundle without it just skips the legacy alias).
        let legacy_shim = find_cli_script_named("k2so");
        return windows_cli_install(&cli_script, &legacy_shim);
    }

    #[cfg(target_os = "linux")]
    {
        let cli_script =
            find_cli_script().ok_or_else(|| "CLI script not found in app bundle".to_string())?;
        let legacy_shim = find_cli_script_named("k2so");
        ensure_cli_executable(&cli_script, &legacy_shim);
        if let Some(found) = linux_installed_cli() {
            if cli_is_distro_package(&found) {
                return Ok(found.display().to_string());
            }
            let is_link = fs::symlink_metadata(&found)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            if !is_link {
                linux_write_cli(&found, &cli_script, &legacy_shim)?;
            }
            linux_path_line(&found)?;
            return Ok(found.display().to_string());
        }
        let dest = linux_user_cli_dest()?;
        linux_write_cli(&dest, &cli_script, &legacy_shim)?;
        linux_path_line(&dest)?;
        return Ok(dest.display().to_string());
    }

    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        use k2_client::cli_install::LinkOutcome;
        let report = mac_heal(true)?;
        for line in report.log_lines() {
            eprintln!("[cli] {line}");
        }
        if report.user_link.is_ours() {
            return Ok(report.user_link_path.display().to_string());
        }
        if report.system_link.as_ref().is_some_and(|l| l.is_ours()) {
            return Ok(CLI_SYMLINK_PATH.to_string());
        }
        let why = match &report.user_link {
            LinkOutcome::Foreign { what } => format!("it is {what}, so K2 left it alone"),
            LinkOutcome::NotWritable { reason } => reason.clone(),
            other => format!("{other:?}"),
        };
        Err(format!(
            "Couldn't put k2 at {}: {why}. Move that file aside, or use Install in /usr/local/bin.",
            report.user_link_path.display()
        ))
    }
}

/// macOS, optional: also link `/usr/local/bin/k2` (+ the k2so alias), with
/// one admin prompt when the folder needs it. Never over a `k2` that isn't
/// K2's. The ONLY admin prompt path; only from an explicit Settings click.
#[tauri::command]
pub async fn cli_install_system() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(cli_install_system_now)
        .await
        .map_err(|e| format!("install the k2 command in /usr/local/bin: {e}"))?
}

fn cli_install_system_now() -> Result<String, String> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        return Err("Installing in /usr/local/bin is a macOS option.".to_string());
    }
    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        use k2_client::cli_install as ci;
        let cli_script =
            find_cli_script().ok_or_else(|| "CLI script not found in app bundle".to_string())?;
        let legacy_shim = find_cli_script_named("k2so");
        ensure_cli_executable(&cli_script, &legacy_shim);
        let symlink_path = Path::new(CLI_SYMLINK_PATH);
        if let ci::LinkState::Foreign { what } = ci::link_state(symlink_path, &cli_script) {
            return Err(format!("{CLI_SYMLINK_PATH} is {what}, not K2's, so K2 left it alone."));
        }
        // The alias only over nothing or K2's own link.
        let legacy_shim = legacy_shim.filter(|shim| {
            !matches!(
                ci::link_state(Path::new(CLI_LEGACY_SYMLINK_PATH), shim),
                ci::LinkState::Foreign { .. }
            )
        });

        let bin_dir = symlink_path.parent().unwrap();
        if !bin_dir.exists() {
            // Create /usr/local/bin via osascript (prompts for password)
            let output = k2_client::bg_proc::command("osascript")
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

        // No prompt when the folder is writable (Intel Homebrew, a chowned prefix).
        if ci::ensure_link(symlink_path, &cli_script, false).is_ours() {
            if let Some(shim) = &legacy_shim {
                let _ = ci::ensure_link(Path::new(CLI_LEGACY_SYMLINK_PATH), shim, false);
            }
            return Ok(CLI_SYMLINK_PATH.to_string());
        }

        // osascript with admin privileges — both links in ONE prompt.
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
        let output = k2_client::bg_proc::command("osascript")
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

/// Remove K2's PATH line: only the lines between the `# >>> K2 CLI >>>`
/// markers, from every startup file that has them. The boot heal then
/// leaves it out until the person installs from Settings again. Returns the
/// files changed.
#[tauri::command]
pub fn cli_remove_path_block() -> Result<Vec<String>, String> {
    #[cfg(unix)]
    {
        use k2_client::cli_install as ci;
        let home = k2_client::paths::home_dir().ok_or_else(|| "HOME is not set".to_string())?;
        let zdot = ci::zdotdir();
        let changed = ci::remove_path_block_at(&home, zdot.as_deref())?;
        Ok(changed.iter().map(|p| p.display().to_string()).collect())
    }
    #[cfg(not(unix))]
    {
        Ok(Vec::new())
    }
}

#[tauri::command]
pub fn cli_uninstall() -> Result<(), String> {
    #[cfg(windows)]
    {
        return windows_cli_uninstall();
    }
    #[cfg(target_os = "linux")]
    {
        // Never remove the distro package. Drop only the user-local copy
        // Settings wrote when nothing else was on PATH.
        let Ok(dest) = linux_user_cli_dest() else {
            return Ok(());
        };
        if dest.exists() || dest.is_symlink() {
            fs::remove_file(&dest).map_err(|e| format!("remove {}: {e}", dest.display()))?;
        }
        if let Some(parent) = dest.parent() {
            let shim = parent.join("k2so");
            if shim.exists() || shim.is_symlink() {
                let _ = fs::remove_file(shim);
            }
        }
        return Ok(());
    }
    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        use k2_client::cli_install as ci;
        let is_k2_link = |p: &Path| fs::read_link(p).is_ok_and(|t| ci::is_k2_cli_target(&t));
        // ~/.local/bin: only K2's own links.
        if let Some(home) = k2_client::paths::home_dir() {
            for name in [ci::CLI_NAME, ci::LEGACY_NAME] {
                let link = ci::user_bin_dir(&home).join(name);
                if is_k2_link(&link) {
                    fs::remove_file(&link).map_err(|e| format!("remove {}: {e}", link.display()))?;
                }
            }
        }

        let symlink_path = Path::new(CLI_SYMLINK_PATH);
        if !is_k2_link(symlink_path) {
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
        let output = k2_client::bg_proc::command("osascript")
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

/// Stop the local bundled `k2-daemon` so a Windows NSIS/in-app update can
/// overwrite `%LOCALAPPDATA%\K2\k2-daemon.exe`. A live daemon holds the
/// file lock → installer error "already … in the file path" and the app
/// comes back on a *new* `k2.exe` talking to the *old* daemon → Connecting
/// forever (`localPairedPolicy` exact version).
#[tauri::command]
pub fn stop_bundled_daemon_for_update() -> Result<(), String> {
    crate::stop_bundled_daemon_processes();
    Ok(())
}

/// Relaunch the app via a helper script that waits for this process to die,
/// then opens the .app bundle cleanly. This avoids:
/// 1. Two dock icons (old process still alive when new one launches)
/// 2. Metal SIGABRT from std::process::exit() running __cxa_finalize_ranges
/// 3. Tauri's built-in relaunch spawning a bare binary (not a .app bundle)
///
/// Windows: `relaunch_via_open` previously only `process::exit(0)` — no
/// helper — so "Install & Relaunch" quit the app after NSIS without
/// restarting it.
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
                    let _ = k2_client::bg_proc::command("/bin/bash")
                        .arg(&script_path)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
            }
        }
    }
    #[cfg(windows)]
    {
        let pid = std::process::id();
        if let Ok(exe) = std::env::current_exe() {
            let exe_s = exe.display().to_string();
            let script = format!(
                "@echo off\r\n\
                 :wait\r\n\
                 tasklist /FI \"PID eq {pid}\" | find \"{pid}\" >nul\r\n\
                 if not errorlevel 1 (timeout /t 1 /nobreak >nul & goto wait)\r\n\
                 timeout /t 2 /nobreak >nul\r\n\
                 start \"\" \"{exe_s}\"\r\n\
                 del \"%~f0\"\r\n"
            );
            let script_path = std::env::temp_dir().join(format!("k2-relaunch-{pid}.cmd"));
            if std::fs::write(&script_path, &script).is_ok() {
                log_debug!(
                    "[relaunch] Windows helper {} waiting for PID {pid}",
                    script_path.display()
                );
                let _ = k2_client::bg_proc::command("cmd")
                    .args(["/C", "start", "", "/MIN", &script_path.display().to_string()])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
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

/// A board file holds an edit that isn't in the file yet (prd-board-tab-v1; K2 Cores' condition on
/// held edits): Quit asks in the page instead of exiting.
pub static BOARD_FILE_UNSAVED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[tauri::command]
pub fn set_board_file_unsaved(unsaved: bool) {
    BOARD_FILE_UNSAVED.store(unsaved, std::sync::atomic::Ordering::Relaxed);
}

/// The person answered Quit without saving.
#[tauri::command]
pub fn board_file_quit_now(app: AppHandle) {
    BOARD_FILE_UNSAVED.store(false, std::sync::atomic::Ordering::Relaxed);
    app.exit(0);
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
    use super::{
        cli_is_distro_package, copy_cli_script, first_k2_cli, is_script, linux_cli_search_paths,
        read_cli_version, version_is_newer,
    };
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
    fn linux_search_order_matches_login_path() {
        let home = PathBuf::from("/home/z");
        let paths = linux_cli_search_paths(Some(&home));
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/usr/local/bin/k2"),
                PathBuf::from("/usr/bin/k2"),
                home.join(".local/bin/k2"),
            ]
        );
        assert!(linux_cli_search_paths(None).len() == 2);
    }

    #[test]
    fn first_k2_cli_skips_absent_and_distro_is_usr_bin_only() {
        let d = scratch();
        let absent = d.join("missing");
        let present = d.join("k2");
        fs::write(&present, "#!/bin/bash\nK2_CLI_VERSION=\"0.41.2\"\n").unwrap();
        let found = first_k2_cli(&[absent, present.clone()]).unwrap();
        assert_eq!(found, present);
        assert_eq!(read_cli_version(&found).as_deref(), Some("0.41.2"));
        assert!(!cli_is_distro_package(&found));
        assert!(cli_is_distro_package(std::path::Path::new("/usr/bin/k2")));
        assert!(!cli_is_distro_package(std::path::Path::new("/usr/local/bin/k2")));
        assert!(!cli_is_distro_package(std::path::Path::new(
            "/home/z/.local/bin/k2"
        )));
        assert!(version_is_newer(Some("0.41.3"), Some("0.41.2")));
        assert!(!version_is_newer(Some("0.41.2"), Some("0.41.2")));
        assert!(!version_is_newer(Some("0.41.3"), None));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_gui_binary_named_k2_is_not_the_cli() {
        // 0.46.11: the .deb installs the GUI at /usr/bin/k2. It must not
        // read as an installed CLI (Settings said "installed, updates with
        // the package" and `k2` in a terminal opened the app).
        let d = scratch();
        let gui = d.join("usr-bin-k2");
        fs::write(&gui, b"\x7fELF\x02\x01\x01\x00\x00").unwrap();
        let no_version = d.join("other-k2");
        fs::write(&no_version, "#!/bin/sh\necho hi\n").unwrap();
        let cli = d.join("local-bin-k2");
        fs::write(&cli, "#!/bin/bash\nK2_CLI_VERSION=\"0.46.11\"\n").unwrap();
        assert!(!is_script(&gui));
        assert!(is_script(&no_version));
        assert_eq!(first_k2_cli(&[gui.clone(), no_version.clone(), cli.clone()]), Some(cli));
        assert_eq!(first_k2_cli(&[gui, no_version]), None);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn copy_cli_script_writes_an_executable_user_copy() {
        let d = scratch();
        let src = d.join("bundled");
        fs::write(&src, "#!/bin/bash\nK2_CLI_VERSION=\"9.9.9\"\n").unwrap();
        let dest = d.join("bin").join("k2");
        copy_cli_script(&src, &dest).unwrap();
        assert_eq!(read_cli_version(&dest).as_deref(), Some("9.9.9"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dest).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111);
        }
        let _ = fs::remove_dir_all(&d);
    }
}
