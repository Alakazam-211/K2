//! 0.40.34 — browser-open shim staging (`~/.k2/bin/k2-open`).
//!
//! When a process inside a K2 terminal session runs `xdg-open <url>`
//! (Linux), types the mac habit `open <url>`, or honors `$BROWSER`, the
//! URL must surface in the CONNECTED K2 app as a session event (the
//! renderer opens it in a browser tab) instead of launching a browser
//! on the daemon's machine — which on a headless server does nothing.
//!
//! The daemon embeds `scripts/k2-open` (POSIX sh) in its binary and
//! stages it at boot into `~/.k2/bin/`:
//!   - `k2-open` — always (every platform). The PTY env block sets
//!     `BROWSER=<abs path>` so `$BROWSER`-honoring programs pick it up.
//!   - `xdg-open` — a same-content copy, **Linux only**. Headless boxes
//!     rarely ship a real `xdg-open`, and ours doing something useful
//!     beats a broken no-op. We deliberately do NOT shadow `open` on
//!     macOS: `/usr/bin/open` does far more than URLs (apps, files,
//!     -R reveal, …) and shadowing it would be far too invasive.
//!
//! Staged files are 0755 and refreshed when the embedded
//! `# K2_OPEN_SHIM_VERSION:` line differs from the on-disk one (so a
//! daemon upgrade rolls the shim forward; a byte-identical version is
//! left untouched).
//!
//! The PATH injection lives in `terminal::daemon_pty` — `~/.k2/bin` is
//! PREPENDED to the child PATH (prepend = first in `execvp` lookup
//! order = our `xdg-open` wins over any system one) and `BROWSER` is
//! filled via `entry().or_insert_with` so a caller-supplied `BROWSER`
//! always wins.

use std::path::{Path, PathBuf};

/// The shim script, embedded verbatim from `scripts/k2-open` at the
/// repo root so the daemon binary is self-contained (nothing to ship
/// alongside it — same reasoning as the SQL migrations' `include_str!`).
pub const SHIM_CONTENT: &str = include_str!("../../../scripts/k2-open");

/// Marker prefix of the version line inside the shim (line 3 of the
/// script). Bump the number in `scripts/k2-open` to force a re-stage
/// on the next daemon boot.
const VERSION_PREFIX: &str = "# K2_OPEN_SHIM_VERSION:";

/// `<home>/.k2/bin` — the staging directory.
pub fn bin_dir_at(home: &Path) -> PathBuf {
    home.join(".k2").join("bin")
}

/// `<home>/.k2/bin/k2-open` — the canonical shim path.
pub fn shim_path_at(home: &Path) -> PathBuf {
    bin_dir_at(home).join("k2-open")
}

/// Extract the value of the `# K2_OPEN_SHIM_VERSION:` line, trimmed.
/// `None` when the marker is absent (treated as "outdated" so a
/// marker-less / hand-edited file gets refreshed).
fn shim_version(content: &str) -> Option<&str> {
    content
        .lines()
        .find_map(|l| l.strip_prefix(VERSION_PREFIX))
        .map(str::trim)
}

/// Does the on-disk file at `path` already carry the embedded version?
fn on_disk_is_current(path: &Path) -> bool {
    let embedded = shim_version(SHIM_CONTENT);
    debug_assert!(embedded.is_some(), "scripts/k2-open lost its version line");
    match std::fs::read_to_string(path) {
        Ok(disk) => shim_version(&disk) == embedded && embedded.is_some(),
        Err(_) => false,
    }
}

/// Write `content` at `path` with mode 0755 (world-executable script;
/// it contains no secrets — tokens come from the caller's env/disk).
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

/// Testable core — stage into an explicit home root. `stage_xdg_open`
/// selects the Linux-only `xdg-open` copy (callers pass
/// `cfg!(target_os = "linux")`; tests pass both). Returns the paths
/// that are now staged and current (whether freshly written or already
/// up to date).
pub fn stage_at(home: &Path, stage_xdg_open: bool) -> Result<Vec<PathBuf>, String> {
    let bin = bin_dir_at(home);
    std::fs::create_dir_all(&bin).map_err(|e| format!("create {}: {e}", bin.display()))?;

    let mut staged = Vec::new();
    let mut names: Vec<&str> = vec!["k2-open"];
    if stage_xdg_open {
        names.push("xdg-open");
    }
    for name in names {
        let path = bin.join(name);
        if !on_disk_is_current(&path) {
            write_exec(&path, SHIM_CONTENT)?;
        }
        staged.push(path);
    }
    Ok(staged)
}

/// Stage against the real `$HOME` (daemon boot). `xdg-open` is staged
/// on Linux ONLY — see the module docs for why macOS keeps its `open`.
pub fn stage() -> Result<Vec<PathBuf>, String> {
    let home = dirs::home_dir().ok_or("no home dir")?;
    stage_at(&home, cfg!(target_os = "linux"))
}

/// The staged shim's absolute path, `Some` only when the file actually
/// exists on disk — the PTY env block gates on this so a failed staging
/// never points `BROWSER` at a dangling path.
pub fn staged_shim_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = shim_path_at(&home);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

/// PURE — prepend `bin` to a PATH value so binaries in it WIN
/// `execvp` / SearchPath lookup (first match wins). Uses
/// [`crate::terminal::path_env`] so the host separator (`:` / `;`) is
/// correct — no hardcoded `:`. No-op when `bin` is already a segment
/// (idempotent across respawns); an empty `path` yields just `bin`.
pub fn prepend_bin_dir(path: &str, bin: &Path) -> String {
    crate::terminal::path_env::prepend(path, bin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-open-shim-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn embedded_shim_has_a_version_line_and_posix_shebang() {
        // The staging cycle keys off this line — losing it would make
        // every boot rewrite (harmless) but breaks upgrade detection
        // semantics; the shebang must be POSIX sh (no bashisms contract).
        assert_eq!(SHIM_CONTENT.lines().next(), Some("#!/bin/sh"));
        let v = shim_version(SHIM_CONTENT).expect("version line present");
        assert!(!v.is_empty(), "version value must be non-empty");
    }

    #[test]
    fn stage_writes_0755_and_is_idempotent() {
        let home = tmp_home("write");
        let staged = stage_at(&home, false).expect("stage ok");
        assert_eq!(staged, vec![shim_path_at(&home)]);
        let on_disk = std::fs::read_to_string(&staged[0]).unwrap();
        assert_eq!(on_disk, SHIM_CONTENT);
        #[cfg(unix)]
        assert_eq!(mode_of(&staged[0]), 0o755, "shim must be executable");

        // Second run: same-version file is left in place (still current).
        let again = stage_at(&home, false).expect("re-stage ok");
        assert_eq!(again, staged);
        assert_eq!(std::fs::read_to_string(&staged[0]).unwrap(), SHIM_CONTENT);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn stage_refreshes_an_outdated_version() {
        let home = tmp_home("bump");
        let path = shim_path_at(&home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // A prior daemon staged version 0 — the current embed must win.
        std::fs::write(
            &path,
            "#!/bin/sh\n# K2_OPEN_SHIM_VERSION: 0\necho stale\n",
        )
        .unwrap();
        stage_at(&home, false).expect("stage ok");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            SHIM_CONTENT,
            "outdated version line must trigger a rewrite"
        );
        #[cfg(unix)]
        assert_eq!(mode_of(&path), 0o755);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn stage_refreshes_a_marker_less_file() {
        // Hand-edited / truncated file without the marker → treated as
        // outdated (fail toward the known-good embed).
        let home = tmp_home("nomarker");
        let path = shim_path_at(&home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        stage_at(&home, false).expect("stage ok");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SHIM_CONTENT);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn xdg_open_copy_is_gated_on_the_linux_flag() {
        // Linux-shaped staging gets BOTH names; mac-shaped staging must
        // NOT create an `xdg-open` (and never an `open` — we don't
        // shadow /usr/bin/open at all, by design).
        let home = tmp_home("xdg");
        let staged = stage_at(&home, true).expect("stage ok");
        assert_eq!(
            staged,
            vec![
                bin_dir_at(&home).join("k2-open"),
                bin_dir_at(&home).join("xdg-open"),
            ]
        );
        assert_eq!(
            std::fs::read_to_string(bin_dir_at(&home).join("xdg-open")).unwrap(),
            SHIM_CONTENT
        );
        #[cfg(unix)]
        assert_eq!(mode_of(&bin_dir_at(&home).join("xdg-open")), 0o755);

        let home2 = tmp_home("noxdg");
        stage_at(&home2, false).expect("stage ok");
        assert!(!bin_dir_at(&home2).join("xdg-open").exists());
        assert!(
            !bin_dir_at(&home2).join("open").exists(),
            "must never stage an `open` shadow"
        );
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&home2).unwrap();
    }

    #[test]
    fn prepend_bin_dir_puts_bin_first_and_is_idempotent() {
        let bin = Path::new("/home/u/.k2/bin");
        // Prepend = FIRST segment = wins execvp lookup order.
        assert_eq!(
            prepend_bin_dir("/usr/bin:/bin", bin),
            "/home/u/.k2/bin:/usr/bin:/bin"
        );
        // Already present (anywhere) → unchanged, no duplicate.
        assert_eq!(
            prepend_bin_dir("/usr/bin:/home/u/.k2/bin:/bin", bin),
            "/usr/bin:/home/u/.k2/bin:/bin"
        );
        // Empty PATH → just the bin dir (never a leading `:` which the
        // shell reads as CWD).
        assert_eq!(prepend_bin_dir("", bin), "/home/u/.k2/bin");
    }
}
