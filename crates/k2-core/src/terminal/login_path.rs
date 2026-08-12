//! PATH enrichment for daemon-spawned child processes (issue #15).
//!
//! The daemon runs under macOS launchd with a bare PATH
//! (`/usr/bin:/bin:/usr/sbin:/sbin`), and under Windows services /
//! non-interactive hosts with a similarly incomplete `Path`. When it
//! spawns agent CLIs (`claude`, `cursor`, `gemini`) by bare name
//! through the PTY layer, those binaries live in user-shell-only
//! directories the host environment never sees — `~/.local/bin` (the
//! Claude native installer default), homebrew, cargo, nvm shims, npm
//! global bins, etc. The bare PATH makes them resolve to ENOENT:
//! "Failed to spawn command 'claude': No such file or directory".
//!
//! This module computes an enriched PATH by unioning three sources,
//! first-occurrence-wins so the user's login-shell ordering is
//! preserved:
//!
//!   1. The user's interactive login-shell PATH (Unix) or User+Machine
//!      Path from the registry / PowerShell (Windows), captured ONCE.
//!   2. A static set of well-known install dirs — a backstop when
//!      capture misses something (or fails).
//!   3. The daemon's own inherited PATH, kept last so nothing the
//!      daemon already had is dropped.
//!
//! Capture is memoized in a `OnceLock` because spawning a login shell
//! (or PowerShell) is relatively expensive and the answer is
//! process-stable. Path splitting/joining is delegated to
//! [`super::path_env`] so Unix `:` and Windows `;` stay correct.

use std::path::PathBuf;
use std::sync::OnceLock;

use super::path_env;

/// De-duplicated, order-preserving union of three PATH sources, in
/// priority order: login-shell entries, then known fallback dirs,
/// then the daemon's inherited entries.
///
/// PURE — no I/O, no globals. Uses [`path_env`] so the host separator
/// is correct; empty segments are skipped (never inject `""`, which a
/// shell interprets as CWD). First occurrence of each distinct entry
/// wins.
///
/// - `login_path`: the captured login-shell / User+Machine PATH, if available.
/// - `known_dirs`: well-known install dirs to guarantee are present.
/// - `inherited`: the process's current PATH (bare host value).
pub fn merge_path(
    login_path: Option<&str>,
    known_dirs: &[PathBuf],
    inherited: &str,
) -> String {
    path_env::merge_login_known_inherited(login_path, known_dirs, inherited)
}

/// Run the user's interactive login shell (Unix) or read User+Machine
/// Path (Windows) ONCE and capture the result. Memoized in a
/// `OnceLock`; subsequent calls return the cached value without
/// re-spawning.
///
/// Returns `None` when the capture fails, the shell exits non-zero,
/// or the captured PATH is empty.
pub fn login_shell_path() -> Option<&'static str> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(capture_login_shell_path)
        .as_deref()
}

#[cfg(unix)]
fn capture_login_shell_path() -> Option<String> {
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());

    // Run the capture on a worker thread bounded by a timeout so a
    // pathological / interactive rc file (one that blocks on input or a
    // slow network/prompt) can never hang the daemon's first spawn — on
    // timeout we fall back to the known-dirs list. stdin = /dev/null so
    // an interactive shell can't block reading from a tty.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // `-l -i` so rc files / profile fully run (nvm, asdf, pyenv hooks
        // typically only fire for login + interactive shells). `printf %s`
        // emits the PATH with no trailing-newline noise of its own.
        let out = Command::new(&shell)
            .args(["-l", "-i", "-c", "printf %s \"$PATH\""])
            .stdin(Stdio::null())
            .output();
        let _ = tx.send(out);
    });

    // 5s is generous for shell init; on timeout/spawn-error we fall back.
    let output = match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(o)) => o,
        _ => return None,
    };

    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

/// Windows: capture User + Machine `Path` values and merge them
/// (User first, matching how interactive shells compose PATH). Prefer
/// PowerShell with a 5s timeout (mirrors Unix login-shell capture);
/// on failure return `None` so [`augmented_path`] keeps the inherited
/// PATH and known fallback dirs only.
#[cfg(windows)]
fn capture_login_shell_path() -> Option<String> {
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    // Single PowerShell invocation reads both User and Machine Path
    // environment values from the registry-backed Environment providers
    // and joins them with `;` (User first). `-NoProfile` keeps startup
    // cheap; we only need the stored Path values, not profile hooks.
    let script = r#"
$user = [Environment]::GetEnvironmentVariable('Path','User')
$machine = [Environment]::GetEnvironmentVariable('Path','Machine')
$parts = @()
if ($user) { $parts += $user }
if ($machine) { $parts += $machine }
Write-Output ($parts -join ';')
"#;

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Prefer `pwsh` (PowerShell 7+) then fall back to Windows PowerShell 5.
        let shells: &[&str] = &["pwsh", "powershell"];
        for shell in shells {
            let out = Command::new(shell)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    script,
                ])
                .stdin(Stdio::null())
                .output();
            match out {
                Ok(o) if o.status.success() => {
                    let _ = tx.send(Ok(o));
                    return;
                }
                Ok(o) => {
                    // Try next shell.
                    let _ = o;
                }
                Err(_) => continue,
            }
        }
        let _ = tx.send(Err(()));
    });

    let output = match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(o)) => o,
        _ => return None,
    };

    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

#[cfg(not(any(unix, windows)))]
fn capture_login_shell_path() -> Option<String> {
    None
}

/// Well-known install directories that agent CLIs land in, filtered
/// to those that actually exist on this machine. These are the
/// backstop for when the login-shell capture misses something (or
/// fails entirely).
///
/// Unix:
///   - `/opt/homebrew/bin` — Homebrew on Apple Silicon
///   - `/usr/local/bin` — Homebrew on Intel + many manual installs
///   - `~/.local/bin` — the Claude native installer default
///   - `~/.cargo/bin` — Rust toolchain (cargo-installed CLIs)
///   - `~/.bun/bin` — Bun-installed global CLIs
///
/// Windows:
///   - `%USERPROFILE%\.local\bin`, `.cargo\bin`, `.bun\bin`
///   - `%APPDATA%\npm` — npm global bin (Windows default)
///   - `%LOCALAPPDATA%\Programs` common tool roots when present
#[cfg(unix)]
pub fn known_fallback_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".cargo/bin"));
        dirs.push(home.join(".bun/bin"));
    }
    dirs.into_iter().filter(|d| d.exists()).collect()
}

#[cfg(windows)]
pub fn known_fallback_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    // Prefer USERPROFILE for user-local tool bins (more reliable on
    // Windows than HOME, which may be unset under services).
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(dirs::home_dir);

    if let Some(home) = home {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".cargo").join("bin"));
        dirs.push(home.join(".bun").join("bin"));
        // npm / pnpm / yarn sometimes land under %USERPROFILE%\AppData\...
        // but the canonical Windows npm global bin is %APPDATA%\npm.
    }

    if let Some(appdata) = std::env::var_os("APPDATA") {
        let appdata = PathBuf::from(appdata);
        dirs.push(appdata.join("npm"));
        // pnpm global bin (default)
        dirs.push(appdata.join("npm").join("bin"));
        dirs.push(appdata.join("pnpm"));
    }

    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        // Volta, fnm, and some Node installers put shims here.
        dirs.push(local.join("fnm_multishells"));
        dirs.push(local.join("Programs").join("Microsoft VS Code").join("bin"));
        dirs.push(local.join("Programs").join("cursor").join("resources").join("app").join("bin"));
    }

    dirs.into_iter().filter(|d| d.exists()).collect()
}

#[cfg(not(any(unix, windows)))]
pub fn known_fallback_dirs() -> Vec<PathBuf> {
    Vec::new()
}

/// Compute the enriched PATH for a daemon-spawned child: the union of
/// the captured login-shell / User+Machine PATH, the known fallback
/// dirs, and the daemon's `inherited` PATH — first occurrence wins.
///
/// This is the single entry point the spawn layer calls. On capture
/// failure the login source is `None` and known dirs + inherited still
/// apply (inherited is never dropped).
pub fn augmented_path(inherited: &str) -> String {
    merge_path(login_shell_path(), &known_fallback_dirs(), inherited)
}

/// Look up PATH from a child env map, treating `PATH` and `Path` as
/// equivalent on Windows (the OS uses `Path`; some callers set `PATH`).
/// Returns `(key_used, value)` so callers can overwrite the same key.
pub fn env_path_entry(env: &std::collections::HashMap<String, String>) -> Option<(&str, &str)> {
    if let Some(v) = env.get("PATH") {
        return Some(("PATH", v.as_str()));
    }
    #[cfg(windows)]
    if let Some(v) = env.get("Path") {
        return Some(("Path", v.as_str()));
    }
    // Case-insensitive scan as a last resort on Windows-shaped maps.
    #[cfg(windows)]
    {
        for (k, v) in env {
            if k.eq_ignore_ascii_case("PATH") {
                return Some((k.as_str(), v.as_str()));
            }
        }
    }
    None
}

/// True when the child env already carries an explicit PATH/`Path`
/// (caller-supplied — leave it untouched).
pub fn env_has_path(env: &std::collections::HashMap<String, String>) -> bool {
    env_path_entry(env).is_some()
}

/// Inherited process PATH, accepting either `PATH` or `Path`.
pub fn process_path() -> String {
    std::env::var("PATH")
        .or_else(|_| std::env::var("Path"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_present_dedups_vs_inherited() {
        // /usr/bin appears in both login and inherited — kept once,
        // at its login-shell position.
        let login = "/opt/homebrew/bin:/usr/bin";
        let inherited = "/usr/bin:/bin";
        let merged = merge_path(Some(login), &[], inherited);
        assert_eq!(merged, "/opt/homebrew/bin:/usr/bin:/bin");
    }

    #[test]
    fn login_absent_known_then_inherited() {
        // No login PATH → known dirs lead, inherited follows.
        let known = vec![PathBuf::from("/opt/homebrew/bin")];
        let inherited = "/usr/bin:/bin";
        let merged = merge_path(None, &known, inherited);
        assert_eq!(merged, "/opt/homebrew/bin:/usr/bin:/bin");
    }

    #[test]
    fn ordering_login_then_known_then_inherited() {
        let login = "/login/bin";
        let known = vec![PathBuf::from("/known/bin")];
        let inherited = "/inherited/bin";
        let merged = merge_path(Some(login), &known, inherited);
        assert_eq!(merged, "/login/bin:/known/bin:/inherited/bin");
    }

    #[test]
    fn no_dup_across_all_three() {
        // /shared/bin appears in all three sources — exactly one copy
        // survives, at its earliest (login) position.
        let login = "/a:/shared/bin";
        let known = vec![PathBuf::from("/shared/bin"), PathBuf::from("/b")];
        let inherited = "/shared/bin:/c";
        let merged = merge_path(Some(login), &known, inherited);
        assert_eq!(merged, "/a:/shared/bin:/b:/c");
    }

    #[test]
    fn empty_inherited_tolerated() {
        let login = "/opt/homebrew/bin:/usr/bin";
        let merged = merge_path(Some(login), &[], "");
        assert_eq!(merged, "/opt/homebrew/bin:/usr/bin");
    }

    #[test]
    fn known_dir_already_in_login_not_duplicated() {
        // The known fallback dir is already present in the login PATH;
        // it must NOT be appended a second time.
        let login = "/opt/homebrew/bin:/usr/bin";
        let known = vec![PathBuf::from("/opt/homebrew/bin")];
        let merged = merge_path(Some(login), &known, "/bin");
        assert_eq!(merged, "/opt/homebrew/bin:/usr/bin:/bin");
    }

    #[test]
    fn empty_segments_skipped() {
        // Leading/trailing/consecutive colons produce empty segments
        // which must be dropped (never inject "" = cwd).
        let login = ":/usr/bin::/bin:";
        let inherited = "::/sbin:";
        let merged = merge_path(Some(login), &[], inherited);
        assert_eq!(merged, "/usr/bin:/bin:/sbin");
    }

    #[test]
    fn all_empty_yields_empty() {
        let merged = merge_path(Some(""), &[], "");
        assert_eq!(merged, "");
        // And the None / no-known / empty-inherited shape too.
        let merged2 = merge_path(None, &[], "");
        assert_eq!(merged2, "");
    }

    #[test]
    fn none_login_no_known_returns_inherited() {
        let merged = merge_path(None, &[], "/usr/bin:/bin");
        assert_eq!(merged, "/usr/bin:/bin");
    }

    #[test]
    fn env_has_path_recognizes_path_key() {
        let mut env = std::collections::HashMap::new();
        assert!(!env_has_path(&env));
        env.insert("PATH".to_string(), "/bin".to_string());
        assert!(env_has_path(&env));
    }

    #[cfg(windows)]
    #[test]
    fn env_has_path_recognizes_windows_path_key() {
        let mut env = std::collections::HashMap::new();
        env.insert("Path".to_string(), r"C:\Windows\System32".to_string());
        assert!(env_has_path(&env));
        let (k, v) = env_path_entry(&env).unwrap();
        assert_eq!(k, "Path");
        assert_eq!(v, r"C:\Windows\System32");
    }

    /// Pure merge with semicolon-shaped synthetic strings — documents
    /// Windows PATH composition when the host separator is `;`.
    #[cfg(windows)]
    #[test]
    fn windows_semicolon_merge_drive_letters() {
        let login = r"C:\Users\u\.local\bin;C:\Users\u\.cargo\bin";
        let known = vec![PathBuf::from(r"C:\Users\u\AppData\Roaming\npm")];
        let inherited = r"C:\Windows\System32;C:\Windows";
        let merged = merge_path(Some(login), &known, inherited);
        assert_eq!(
            merged,
            r"C:\Users\u\.local\bin;C:\Users\u\.cargo\bin;C:\Users\u\AppData\Roaming\npm;C:\Windows\System32;C:\Windows"
        );
    }
}
