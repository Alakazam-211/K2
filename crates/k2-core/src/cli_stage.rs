//! 0.40.41 — self-stage the `k2` CLI on headless Linux servers.
//!
//! The `k2` CLI (`cli/k2`, a bash client that talks to the daemon over
//! HTTP via `~/.k2/heartbeat.{port,token}`) reaches a box one of two
//! ways: the DESKTOP APP symlinks `/usr/local/bin/k2` into its bundle
//! (macOS — see `commands/settings.rs::cli_symlink_needs_heal`), or the
//! provision script `install`s it (Linux). A headless Linux SERVER has no
//! app, so if the provision/migration step is skipped the CLI silently
//! never appears — daemon + data present, `k2` command absent. That bit
//! a client-box migration (internal migration-runbook lesson L-cli).
//!
//! Fix: the daemon self-stages the CLI at boot, exactly like the
//! `k2-open` shim ([`crate::open_shim`]) — embed `cli/k2` via
//! `include_str!` and, on Linux, write it to `/usr/local/bin/k2` when it
//! is ABSENT or an out-of-date regular file. Guardrails:
//!   - **Linux only.** macOS keeps the app-managed symlink; we never race
//!     it.
//!   - **Never clobber a SYMLINK** at the target. That respects both the
//!     macOS app's symlink and a developer's repo symlink — we only ever
//!     manage a regular-file install.
//!   - **Non-fatal.** A failed write (not root, read-only `/usr/local/bin`)
//!     is logged by the caller, never crashes boot.
//!
//! New k5 command (Beta): when the owner's switch is on and the k5 binary
//! sits beside this daemon at this release's version
//! ([`k2_client::cli_install::binary_in_dir_if_on`]), `k2` becomes a link to
//! the binary and a `k5` link joins it ([`StageAs::Binary`]). Off, or with no
//! such binary, it is the copy of the script as before, and a link to the
//! binary that K2 made is put back to the copy (a `k5` K2 made gets a copy
//! too; none is made while off). The one symlink the stage replaces is a
//! link to the binary (`is_binary_target`); any other symlink is still
//! never touched. Each change is one rename over the old file or link, so
//! the path never goes missing; `k2` changes before `k5`, so a stage stopped
//! between them leaves both working, one on each.

use std::path::{Path, PathBuf};

/// The CLI script, embedded verbatim from `cli/k2` at the repo root so
/// the daemon binary is self-contained (same reasoning as the SQL
/// migrations' and the k2-open shim's `include_str!`).
// The repo's script file, not the command's name: a brand build of the bash CLI is a later step (C7).
pub const CLI_CONTENT: &str = include_str!("../../../cli/k2");

/// Current version marker inside `cli/k2` (bumped by `release.sh`).
const VERSION_PREFIX: &str = "K2_CLI_VERSION=";
/// Pre-rename marker — lets an on-disk k2so-era CLI read as outdated so
/// it gets refreshed forward.
const LEGACY_VERSION_PREFIX: &str = "K2SO_CLI_VERSION=";

/// Canonical install location (on PATH; matches `provision-k2-server.sh`):
/// `/usr/local/bin/<the command>`, from the brand file.
const INSTALL_PATH: &str = crate::cli_install::SYSTEM_CLI_PATH;
/// The command's name (the file name of [`INSTALL_PATH`]).
const CLI_NAME: &str = k2_client::cli_install::CLI_NAME;

/// Extract the CLI version value, quote-trimmed. `None` when neither
/// marker is present (treated as outdated so a marker-less / hand-edited
/// file is refreshed toward the embedded copy).
fn cli_version(content: &str) -> Option<String> {
    for line in content.lines().take(30) {
        for prefix in [VERSION_PREFIX, LEGACY_VERSION_PREFIX] {
            if let Some(rest) = line.strip_prefix(prefix) {
                return Some(rest.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

/// The embedded CLI's version (= the daemon's in a release build).
pub fn embedded_version() -> Option<String> {
    cli_version(CLI_CONTENT)
}

/// The version of a file that IS the K2 CLI: a `#!/bin/bash` first line
/// plus the version marker. `None` for anything else (another program
/// named `k2` is never treated as a stale K2 CLI).
pub fn k2_cli_version_of(content: &str) -> Option<String> {
    if content.lines().next() != Some("#!/bin/bash") {
        return None;
    }
    cli_version(content)
}

/// Write `content` at `path` with mode 0755 (world-executable script; it
/// carries no secrets — tokens come from `~/.k2` at runtime).
fn write_exec(path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod 0755 {}: {e}", path.display()))?;
    }
    Ok(())
}

/// What the stage puts at a command's path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageAs<'a> {
    /// A copy of the embedded script (today's).
    Script,
    /// A link to the k5 binary at this path.
    Binary(&'a Path),
}

impl<'a> StageAs<'a> {
    /// From the links' [`k2_client::cli_install::Choice`].
    pub fn of(choice: &'a k2_client::cli_install::Choice) -> Self {
        choice.binary().map_or(StageAs::Script, StageAs::Binary)
    }
}

/// True when `path` is a symlink to the k5 binary (by its name).
fn is_binary_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
        && std::fs::read_link(path).is_ok_and(|t| k2_client::cli_install::is_binary_target(&t))
}

/// Put the script copy at `target` in one step (a temp file beside it,
/// renamed over it), for a path that holds a link to the binary.
fn swap_in_script(target: &Path) -> Result<(), String> {
    let tmp = k2_client::cli_install::swap_temp_path(target);
    let _ = std::fs::remove_file(&tmp);
    write_exec(&tmp, CLI_CONTENT)?;
    k2_client::cli_install::finish_swap(&tmp, target).map_err(|e| format!("replace {}: {e}", target.display()))
}

/// Put a link to `binary` at `target` in one step.
#[cfg(unix)]
fn swap_in_binary(binary: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {} (for {}): {e}", parent.display(), target.display()))?;
    }
    let tmp = k2_client::cli_install::begin_link_swap(binary, target)
        .map_err(|e| format!("link {}: {e}", target.display()))?;
    k2_client::cli_install::finish_swap(&tmp, target).map_err(|e| format!("replace {}: {e}", target.display()))
}

#[cfg(not(unix))]
fn swap_in_binary(_binary: &Path, target: &Path) -> Result<(), String> {
    Err(format!("{}: the k5 binary is linked on Unix only", target.display()))
}

/// Testable core — decide + act at an explicit `target`. Returns
/// `Ok(Some(path))` when the CLI is now staged (freshly written or
/// already current), `Ok(None)` when deliberately skipped because the
/// target is a SYMLINK (owned by the app / a dev) or a file that isn't
/// the K2 CLI (another program named `k2`), and `Err` only on a real
/// write/permission failure.
pub fn stage_cli_at(target: &Path) -> Result<Option<PathBuf>, String> {
    stage_at(target, StageAs::Script, true)
}

/// [`stage_cli_at`] for what `as_` says. `create`: make the path when
/// nothing is there (`false` for a `k5` while the links run the script).
/// A link to the k5 binary is K2's: it is replaced (by the script copy, or
/// re-pointed at this binary). Leftover swap temps beside it are removed
/// first ([`k2_client::cli_install::sweep_temp_links`]).
pub fn stage_at(target: &Path, as_: StageAs<'_>, create: bool) -> Result<Option<PathBuf>, String> {
    #[cfg(unix)]
    if let Some(dir) = target.parent() {
        for gone in k2_client::cli_install::sweep_temp_links(dir) {
            crate::log_debug!("[cli-stage] removed {}, left by an interrupted swap", gone.display());
        }
    }
    let present = std::fs::symlink_metadata(target).is_ok();
    if !present && !create {
        return Ok(None);
    }
    if is_binary_link(target) {
        return match as_ {
            StageAs::Binary(binary) => {
                let current = std::fs::read_link(target).ok().map(|t| match target.parent() {
                    Some(dir) if t.is_relative() => dir.join(t),
                    _ => t,
                });
                if current.as_deref() == Some(binary) {
                    return Ok(Some(target.to_path_buf()));
                }
                swap_in_binary(binary, target)?;
                Ok(Some(target.to_path_buf()))
            }
            StageAs::Script => {
                swap_in_script(target)?;
                Ok(Some(target.to_path_buf()))
            }
        };
    }
    // Never clobber any other symlink: the macOS app points it into its
    // bundle, and a developer may link it at their repo checkout. We manage
    // only a regular-file install (and a link to the binary, above).
    if std::fs::symlink_metadata(target)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Ok(None);
    }

    // 0.46.11: never overwrite a `k2` that isn't K2's. A regular file is
    // replaced only when it is the K2 CLI (any version) or empty (a write
    // that died half way); another program named `k2` stays, with one log
    // line. A directory still falls through to the write, which fails, so
    // the caller's fallback runs.
    let on_disk = std::fs::metadata(target)
        .ok()
        .filter(|m| m.is_file())
        .map(|_| std::fs::read(target).unwrap_or_default());
    if let Some(bytes) = &on_disk {
        let text = String::from_utf8_lossy(bytes);
        if !text.trim().is_empty() && k2_cli_version_of(&text).is_none() {
            crate::log_debug!("[cli-stage] left {} alone: it isn't the K2 CLI", target.display());
            return Ok(None);
        }
    }

    if let StageAs::Binary(binary) = as_ {
        // Over the script copy (or nothing): one rename. A folder there
        // fails the rename, so the caller's fallback runs, as for a write.
        swap_in_binary(binary, target)?;
        return Ok(Some(target.to_path_buf()));
    }

    let embedded = cli_version(CLI_CONTENT);
    debug_assert!(embedded.is_some(), "cli/k2 lost its K2_CLI_VERSION line");

    let current = on_disk.as_ref().and_then(|b| cli_version(&String::from_utf8_lossy(b)));
    if embedded.is_some() && current == embedded {
        return Ok(Some(target.to_path_buf())); // already current — leave it
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!("create {} (for {}): {e}", parent.display(), target.display())
        })?;
    }
    write_exec(target, CLI_CONTENT)?;
    Ok(Some(target.to_path_buf()))
}

/// Testable core of the boot entry: try `primary`, and when it fails
/// with a real write error (typically EACCES: a root-owned
/// `/usr/local/bin/k2` under a non-root daemon — the customer-box
/// migration trap, 2026-07-15), fall back to `fallback` so the CLI can
/// never silently freeze at an old version again. `~/.local/bin` is the
/// same user-writable dir the Linux daemon itself installs into, and is
/// on Ubuntu's default login PATH.
///
/// A symlink at the primary still short-circuits to `Ok(None)` (owned
/// installs are respected — no fallback write behind them). The primary
/// error is logged, not swallowed: the operator-visible fix is still to
/// chown the canonical path.
pub fn stage_cli_with_fallback(
    primary: &Path,
    fallback: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    stage_cli_with_fallback_as(primary, fallback, StageAs::Script)
}

/// [`stage_cli_with_fallback`] for what `as_` says, then the `k5` beside
/// the staged `k2` (made for the binary; with the script only refreshed
/// where K2 made one). With the script, a link to the binary at the other
/// place (`k2` or `k5`) goes back to the script too, so turning the switch
/// off reaches every link it made.
pub fn stage_cli_with_fallback_as(
    primary: &Path,
    fallback: Option<&Path>,
    as_: StageAs<'_>,
) -> Result<Option<PathBuf>, String> {
    let staged = stage_k2_with_fallback(primary, fallback, as_)?;
    if let Some(k2) = &staged {
        let k5 = k2.with_file_name(k2_client::cli_install::K5_NAME);
        if let Err(e) = stage_at(&k5, as_, matches!(as_, StageAs::Binary(_))) {
            crate::log_debug!("[cli-stage] WARN: stage {}: {e}", k5.display());
        }
    }
    if as_ == StageAs::Script {
        for other in [Some(primary), fallback].into_iter().flatten() {
            for name in [CLI_NAME, k2_client::cli_install::K5_NAME] {
                let path = other.with_file_name(name);
                if staged.as_deref().and_then(Path::parent) == path.parent() || !is_binary_link(&path) {
                    continue;
                }
                if let Err(e) = stage_at(&path, StageAs::Script, false) {
                    crate::log_debug!("[cli-stage] WARN: put the script back at {}: {e}", path.display());
                }
            }
        }
    }
    Ok(staged)
}

fn stage_k2_with_fallback(
    primary: &Path,
    fallback: Option<&Path>,
    as_: StageAs<'_>,
) -> Result<Option<PathBuf>, String> {
    let primary_err = match stage_at(primary, as_, true) {
        Ok(out) => return Ok(out),
        Err(e) => e,
    };
    let Some(fallback) = fallback else {
        return Err(primary_err);
    };
    crate::log_debug!(
        "[cli-stage] {} not writable ({primary_err}) — falling back to {} (fix: chown the canonical path to the daemon user)",
        primary.display(),
        fallback.display(),
    );
    stage_at(fallback, as_, true)
        .map_err(|e| format!("{primary_err}; fallback {e}"))
}

/// Real boot entry — Linux ONLY (macOS keeps the app-managed symlink).
/// No-op on other platforms. Stages into `/usr/local/bin/k2`, falling
/// back to `~/.local/bin/k2` when the canonical path isn't writable by
/// the daemon user.
pub fn stage_cli() -> Result<Option<PathBuf>, String> {
    if !cfg!(target_os = "linux") {
        return Ok(None);
    }
    let fallback = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| crate::cli_install::user_bin_dir(Path::new(&h)).join(crate::cli_install::CLI_NAME));
    let choice = current_choice();
    stage_cli_with_fallback_as(Path::new(INSTALL_PATH), fallback.as_deref(), StageAs::of(&choice))
}

/// What this daemon's commands run, by the owner's switch: on Linux the
/// k5 binary beside this daemon checked against the embedded script; on
/// macOS the one beside the bundled script ([`k2_client::cli_install::choice_for_script`]);
/// the script anywhere else, or for a build with no bundle.
pub fn current_choice() -> k2_client::cli_install::Choice {
    use k2_client::cli_install::{self as ci, Choice};
    let on = ci::switch_on();
    let exe = std::env::current_exe().ok();
    if cfg!(target_os = "linux") {
        let dir = exe.as_deref().and_then(Path::parent);
        let found = match dir {
            Some(dir) => ci::binary_in_dir_if_on(on, dir, embedded_version().as_deref()),
            None => Err("this daemon's folder is unknown".to_string()),
        };
        return Choice::from_found(on, found);
    }
    if cfg!(target_os = "macos") {
        if let Some(script) = exe.as_deref().and_then(|e| ci::packaged_cli_for_exe(e, ci::CLI_NAME)) {
            return ci::choice_for_script(on, &script);
        }
        return Choice::Script { note: on.then(|| "this build isn't an installed app".to_string()) };
    }
    Choice::Script { note: on.then(|| "this system runs the script only".to_string()) }
}

/// The k5 binary's tests (they name its file, which this file doesn't:
/// `tests/packaging/k5_cli_shipped_off_by_default.sh`).
#[cfg(all(test, unix))]
#[path = "cli_stage_binary_tests.rs"]
mod binary_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("k2-cli-stage-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn embedded_cli_has_version_and_bash_shebang() {
        // The staging cycle keys off K2_CLI_VERSION; the CLI is bash.
        assert_eq!(CLI_CONTENT.lines().next(), Some("#!/bin/bash"));
        let v = cli_version(CLI_CONTENT).expect("K2_CLI_VERSION present");
        assert!(!v.is_empty(), "version value must be non-empty");
    }

    #[test]
    fn version_parse_trims_quotes_and_accepts_legacy() {
        assert_eq!(cli_version("K2_CLI_VERSION=\"1.2.3\"\n").as_deref(), Some("1.2.3"));
        assert_eq!(cli_version("K2SO_CLI_VERSION=\"0.9\"\n").as_deref(), Some("0.9"));
        assert_eq!(cli_version("# no version here\n"), None);
    }

    #[test]
    fn stages_when_absent_0755() {
        let dir = tmp_dir("absent");
        let target = dir.join("k2");
        let out = stage_cli_at(&target).expect("stage ok");
        assert_eq!(out, Some(target.clone()));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), CLI_CONTENT);
        #[cfg(unix)]
        assert_eq!(mode_of(&target), 0o755, "CLI must be executable");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn idempotent_when_current() {
        let dir = tmp_dir("current");
        let target = dir.join("k2");
        stage_cli_at(&target).expect("first stage");
        // Same-version file is left untouched on the second pass.
        let again = stage_cli_at(&target).expect("second stage");
        assert_eq!(again, Some(target.clone()));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), CLI_CONTENT);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refreshes_an_outdated_regular_file() {
        let dir = tmp_dir("stale");
        let target = dir.join("k2");
        std::fs::write(&target, "#!/bin/bash\nK2_CLI_VERSION=\"0.0.1\"\necho old\n").unwrap();
        stage_cli_at(&target).expect("stage ok");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            CLI_CONTENT,
            "an older on-disk version must be refreshed"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn never_clobbers_a_symlink() {
        // Respect the macOS app's symlink / a dev's repo symlink: if the
        // target is a symlink we skip entirely and leave it in place.
        let dir = tmp_dir("symlink");
        let real = dir.join("elsewhere-k2");
        std::fs::write(&real, "#!/bin/bash\nK2_CLI_VERSION=\"9.9.9\"\n").unwrap();
        let target = dir.join("k2");
        std::os::unix::fs::symlink(&real, &target).unwrap();

        let out = stage_cli_at(&target).expect("stage ok");
        assert_eq!(out, None, "a symlink target must be skipped");
        assert!(
            std::fs::symlink_metadata(&target).unwrap().file_type().is_symlink(),
            "symlink must be left intact"
        );
        assert_eq!(
            std::fs::read_to_string(&real).unwrap(),
            "#!/bin/bash\nK2_CLI_VERSION=\"9.9.9\"\n",
            "the symlink's real target must be untouched"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn never_overwrites_a_k2_that_is_not_the_k2_cli() {
        // 0.46.11: another program named `k2` (a binary, or a script with
        // no K2 version line) stays byte for byte, and nothing is staged.
        let dir = tmp_dir("foreign");
        let target = dir.join("k2");
        for other in [&b"\x7fELF\x02\x01\x01\x00"[..], b"#!/bin/sh\necho some other k2\n"] {
            std::fs::write(&target, other).unwrap();
            let out = stage_cli_at(&target).expect("stage ok");
            assert_eq!(out, None, "a k2 that isn't K2's must be skipped");
            assert_eq!(std::fs::read(&target).unwrap(), other, "and left untouched");
        }
        // An empty file (a write that died) is K2's to fill.
        std::fs::write(&target, b"").unwrap();
        assert_eq!(stage_cli_at(&target).expect("stage ok"), Some(target.clone()));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), CLI_CONTENT);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Write failures are injected STRUCTURALLY (a path under a regular
    // file -> ENOTDIR/EEXIST, or a directory where the file goes ->
    // EISDIR). chmod-based injection does nothing when the suite runs as
    // root (the Linux gate does), so it must never be used here.

    #[test]
    #[cfg(unix)]
    fn unwritable_primary_falls_back() {
        // Simulate the customer-box migration trap: the primary location cannot be
        // written by the daemon user. Here its parent "directory" is a
        // regular file, which fails for every user, root included.
        let dir = tmp_dir("fallback");
        let blocker = dir.join("ro-bin");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let primary = blocker.join("k2");

        let fallback = dir.join("home-local-bin").join("k2");
        let out = stage_cli_with_fallback(&primary, Some(&fallback))
            .expect("fallback stage must succeed");
        assert_eq!(out, Some(fallback.clone()));
        assert_eq!(
            std::fs::read_to_string(&fallback).unwrap(),
            CLI_CONTENT,
            "current CLI must land at the fallback"
        );
        assert_eq!(mode_of(&fallback), 0o755, "fallback CLI must be executable");
        assert_eq!(
            std::fs::read(&blocker).unwrap(),
            b"not a directory",
            "the blocking file is left byte-identical"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn unwritable_existing_primary_falls_back() {
        // The stale-install variant: something already sits at the
        // primary path and cannot be overwritten. A DIRECTORY named `k2`
        // makes the write fail with EISDIR, even for root.
        let dir = tmp_dir("fallback-isdir");
        let primary = dir.join("bin").join("k2");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::write(primary.join("keep"), b"untouched").unwrap();

        let fallback = dir.join("home-local-bin").join("k2");
        let out = stage_cli_with_fallback(&primary, Some(&fallback))
            .expect("fallback stage must succeed");
        assert_eq!(out, Some(fallback.clone()));
        assert_eq!(std::fs::read_to_string(&fallback).unwrap(), CLI_CONTENT);
        assert!(primary.is_dir(), "the blocking directory is left in place");
        assert_eq!(std::fs::read(primary.join("keep")).unwrap(), b"untouched");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn writable_primary_never_touches_fallback() {
        let dir = tmp_dir("primary-wins");
        let primary = dir.join("k2");
        let fallback = dir.join("home-local-bin").join("k2");
        let out = stage_cli_with_fallback(&primary, Some(&fallback))
            .expect("primary stage must succeed");
        assert_eq!(out, Some(primary.clone()));
        assert_eq!(std::fs::read_to_string(&primary).unwrap(), CLI_CONTENT);
        assert!(!fallback.exists(), "fallback must be untouched when primary works");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn no_fallback_surfaces_the_primary_error() {
        let dir = tmp_dir("no-fallback");
        let blocker = dir.join("ro-bin");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let primary = blocker.join("k2");

        let err = stage_cli_with_fallback(&primary, None)
            .expect_err("no fallback -> the write error must surface");
        assert!(
            err.contains(&primary.display().to_string()),
            "error names the primary path: {err}"
        );
        assert_eq!(std::fs::read(&blocker).unwrap(), b"not a directory");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
