//! PATH enrichment for daemon-spawned child processes (issue #15).
//!
//! The daemon runs under macOS launchd with a bare PATH
//! (`/usr/bin:/bin:/usr/sbin:/sbin`). When it spawns agent CLIs
//! (`claude`, `cursor`, `gemini`) by bare name through the PTY layer,
//! those binaries live in user-shell-only directories the launchd
//! environment never sees — `~/.local/bin` (the Claude native
//! installer default), `/opt/homebrew/bin`, `/usr/local/bin`, nvm
//! shims, etc. The bare PATH makes them resolve to ENOENT:
//! "Failed to spawn command 'claude': No such file or directory".
//!
//! This module computes an enriched PATH by unioning three sources,
//! first-occurrence-wins so the user's login-shell ordering is
//! preserved:
//!
//!   1. The user's interactive login-shell PATH (captured ONCE by
//!      running their `$SHELL -l -i -c 'printf %s "$PATH"'`). This
//!      picks up nvm shims, asdf, pyenv, and anything else their
//!      rc files inject.
//!   2. A static set of well-known install dirs (homebrew, cargo,
//!      bun, the Claude installer's `~/.local/bin`) — a backstop for
//!      the case where the login shell didn't export them (or
//!      capturing it failed).
//!   3. The daemon's own inherited PATH (the bare launchd value),
//!      kept last so nothing the daemon already had is dropped.
//!
//! The login-shell capture is memoized in a `OnceLock` because
//! spawning an interactive login shell is relatively expensive and
//! the answer is process-stable.

use std::path::PathBuf;
use std::sync::OnceLock;

/// De-duplicated, order-preserving union of three PATH sources, in
/// priority order: login-shell entries, then known fallback dirs,
/// then the daemon's inherited entries.
///
/// PURE — no I/O, no globals. Splits each source on `:`, skips empty
/// segments (so we never inject `""`, which a shell interprets as the
/// current working directory — a security/correctness footgun), and
/// keeps the FIRST occurrence of each distinct entry. The result is
/// re-joined with `:`.
///
/// - `login_path`: the captured login-shell PATH, if available.
/// - `known_dirs`: well-known install dirs to guarantee are present.
/// - `inherited`: the process's current PATH (bare launchd value).
pub fn merge_path(
    login_path: Option<&str>,
    known_dirs: &[PathBuf],
    inherited: &str,
) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();

    let mut push = |entry: &str| {
        if entry.is_empty() {
            return;
        }
        if seen.insert(entry.to_string()) {
            out.push(entry.to_string());
        }
    };

    // 1. Login-shell entries first — preserves the user's ordering.
    if let Some(lp) = login_path {
        for entry in lp.split(':') {
            push(entry);
        }
    }

    // 2. Known fallback dirs.
    for dir in known_dirs {
        push(&dir.to_string_lossy());
    }

    // 3. Inherited (bare launchd) entries last.
    for entry in inherited.split(':') {
        push(entry);
    }

    out.join(":")
}

/// Run the user's interactive login shell ONCE and capture its
/// exported PATH. Memoized in a `OnceLock`; subsequent calls return
/// the cached value without re-spawning a shell.
///
/// Returns `None` when the capture fails, the shell exits non-zero,
/// or the captured PATH is empty. On non-unix this is always `None`.
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

#[cfg(not(unix))]
fn capture_login_shell_path() -> Option<String> {
    None
}

/// Well-known install directories that agent CLIs land in, filtered
/// to those that actually exist on this machine. These are the
/// backstop for when the login-shell capture misses something (or
/// fails entirely):
///
///   - `/opt/homebrew/bin` — Homebrew on Apple Silicon
///   - `/usr/local/bin` — Homebrew on Intel + many manual installs
///   - `~/.local/bin` — the Claude native installer default
///   - `~/.cargo/bin` — Rust toolchain (cargo-installed CLIs)
///   - `~/.bun/bin` — Bun-installed global CLIs
///
/// Empty on non-unix.
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

#[cfg(not(unix))]
pub fn known_fallback_dirs() -> Vec<PathBuf> {
    Vec::new()
}

/// Compute the enriched PATH for a daemon-spawned child: the union of
/// the captured login-shell PATH, the known fallback dirs, and the
/// daemon's `inherited` PATH — first occurrence wins.
///
/// This is the single entry point the spawn layer calls.
pub fn augmented_path(inherited: &str) -> String {
    merge_path(login_shell_path(), &known_fallback_dirs(), inherited)
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
}
