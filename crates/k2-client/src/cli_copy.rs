//! The Linux desktop app's copy of the `k2` script: Settings → Install and
//! the app's silent boot heal. The daemon stages its own copy
//! (`k2_core::cli_stage`); this is the app's backstop, and it decides here
//! (not in `src-tauri`) so cargo tests it on Linux.
//!
//! Two rules keep it off what isn't its own:
//!
//! 1. **What counts as installed** ([`first_k2_cli`]): the K2 CLI by its
//!    bytes, the script or the k5 binary ([`crate::cli_install::is_k2_cli_script`],
//!    links followed). With the owner's "New k5 command (Beta)" on, the
//!    daemon makes `~/.local/bin/k2` a link to the binary; the app reads
//!    that as installed and leaves it.
//! 2. **Never through a link** ([`copy_script`]): a file is rewritten with
//!    `O_NOFOLLOW`, and a link is replaced itself (a temp file beside it,
//!    renamed over it), so a link's target is never written. A link that still
//!    runs the K2 CLI (the script or the binary) is ours and kept; a link
//!    whose K2 target is gone ([`crate::cli_install::is_k2_cli_target`]) is
//!    replaced; a link to anything else, a folder, or a file that isn't the
//!    K2 CLI is left alone. Only the daemon's guarded code
//!    (`cli_install`'s guard) ever makes a link to the binary.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::brand::PRODUCT_NAME;
use crate::cli_install::{self as ci, CLI_NAME};

/// Distro packages (Arch pacman) install the CLI here. The app never
/// writes a second copy over it.
pub const DISTRO_CLI_PATH: &str = "/usr/bin/k2";

/// The places a login shell finds `k2` on Linux, in PATH order:
/// `/usr/local/bin` is ahead of `/usr/bin`, which is ahead of
/// `~/.local/bin`. The first that is the K2 CLI is the one `k2` runs.
pub fn search_paths(home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/usr/local/bin").join(CLI_NAME), PathBuf::from(DISTRO_CLI_PATH)];
    if let Some(home) = home.filter(|h| !h.as_os_str().is_empty()) {
        paths.push(ci::user_bin_dir(home).join(CLI_NAME));
    }
    paths
}

/// The first of `paths` that is the K2 CLI: the script or the k5 binary
/// (links followed). The .deb's GUI binary at `/usr/bin/k2` is neither.
pub fn first_k2_cli(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|p| ci::is_k2_cli_script(p)).cloned()
}

/// `path` is where the distro package puts the CLI.
pub fn is_distro_package(path: &Path) -> bool {
    path == Path::new(DISTRO_CLI_PATH)
}

/// What the boot heal does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealPlan {
    /// A K2 CLI is already on the search path here: nothing to do.
    Installed(PathBuf),
    /// Nothing is: copy the script to this place.
    Copy(PathBuf),
}

/// The boot heal's decision: leave the first K2 CLI on `search` alone,
/// else copy to `dest` (`~/.local/bin/k2`).
pub fn heal_plan(search: &[PathBuf], dest: &Path) -> HealPlan {
    match first_k2_cli(search) {
        Some(found) => HealPlan::Installed(found),
        None => HealPlan::Copy(dest.to_path_buf()),
    }
}

/// What [`copy_script`] may do at a destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyPlan {
    /// Nothing there, an empty file, or a file that is the K2 CLI: write.
    Write,
    /// A link to a K2 CLI that is gone: the link itself is replaced.
    ReplaceLink { was: PathBuf },
    /// Left alone, and why: a link that runs the K2 CLI (ours, kept), or
    /// something that isn't K2's.
    Leave { why: String },
}

/// The link's target, relative targets read from the link's folder.
fn link_target(link: &Path) -> io::Result<PathBuf> {
    let raw = fs::read_link(link)?;
    Ok(match link.parent() {
        Some(dir) if raw.is_relative() => dir.join(raw),
        _ => raw,
    })
}

/// [`CopyPlan`] for `dest`. Reads only; opens a link's target only to read
/// what it is.
pub fn copy_plan(dest: &Path) -> CopyPlan {
    let meta = match fs::symlink_metadata(dest) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return CopyPlan::Write,
        Err(e) => return CopyPlan::Leave { why: format!("something {PRODUCT_NAME} can't read ({e})") },
    };
    if meta.file_type().is_symlink() {
        let target = match link_target(dest) {
            Ok(t) => t,
            Err(e) => return CopyPlan::Leave { why: format!("a link {PRODUCT_NAME} can't read ({e})") },
        };
        let live = fs::metadata(dest).is_ok();
        if live && ci::is_k2_cli_script(dest) {
            return CopyPlan::Leave { why: format!("a link to the {PRODUCT_NAME} CLI at {}", target.display()) };
        }
        if !live && ci::is_k2_cli_target(&target) {
            return CopyPlan::ReplaceLink { was: target };
        }
        return CopyPlan::Leave { why: format!("a link to {}, not {PRODUCT_NAME}'s", target.display()) };
    }
    if meta.is_dir() {
        return CopyPlan::Leave { why: "a folder".to_string() };
    }
    if meta.len() == 0 || ci::is_k2_cli_script(dest) {
        return CopyPlan::Write;
    }
    CopyPlan::Leave { why: format!("a file that isn't the {PRODUCT_NAME} CLI") }
}

/// What [`copy_script`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyOutcome {
    Written,
    /// A link to a K2 CLI that was gone, replaced by the copy.
    LinkReplaced { was: PathBuf },
    /// Nothing written ([`CopyPlan::Leave`]).
    Left { why: String },
}

impl CopyOutcome {
    /// The copy is now at the destination.
    pub fn is_written(&self) -> bool {
        matches!(self, Self::Written | Self::LinkReplaced { .. })
    }
}

/// Copy the script `src` to `dest` (mode 0755) as [`copy_plan`] allows.
/// Never writes through a link: a file already there is rewritten in place
/// and opened with `O_NOFOLLOW` (so a link swapped in meanwhile fails the
/// open; a chowned `/usr/local/bin/k2` in a root folder still updates, as
/// before); a new copy, or one over a link, is a temp file beside `dest`
/// renamed over it, which replaces the link itself.
pub fn copy_script(src: &Path, dest: &Path) -> Result<CopyOutcome, String> {
    let done = match copy_plan(dest) {
        CopyPlan::Leave { why } => return Ok(CopyOutcome::Left { why }),
        CopyPlan::Write => CopyOutcome::Written,
        CopyPlan::ReplaceLink { was } => CopyOutcome::LinkReplaced { was },
    };
    let bytes = fs::read(src).map_err(|e| format!("read {}: {e}", src.display()))?;
    if done == CopyOutcome::Written && fs::symlink_metadata(dest).is_ok() {
        return write_in_place(dest, &bytes).map(|()| done);
    }
    let dir = dest.parent().ok_or_else(|| format!("{} has no folder", dest.display()))?;
    fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let tmp = ci::swap_temp_path(dest);
    let _ = fs::remove_file(&tmp);
    if let Err(e) = write_new(&tmp, &bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    ci::finish_swap(&tmp, dest).map_err(|e| format!("replace {}: {e}", dest.display()))?;
    Ok(done)
}

fn open_flags(o: &mut fs::OpenOptions) -> &mut fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    o.custom_flags(libc::O_NOFOLLOW).mode(0o755)
}

/// A new file at `path` (never one that is there, never through a link).
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut f = open_flags(fs::OpenOptions::new().write(true).create_new(true))
        .open(path)
        .map_err(|e| format!("create {}: {e}", path.display()))?;
    f.write_all(bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
    set_exec(&f, path)
}

/// The regular file at `path`, rewritten (never through a link).
fn write_in_place(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut f = open_flags(fs::OpenOptions::new().write(true).truncate(true))
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    f.write_all(bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
    set_exec(&f, path)
}

fn set_exec(f: &fs::File, path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    f.set_permissions(fs::Permissions::from_mode(0o755)).map_err(|e| format!("chmod {}: {e}", path.display()))
}

// ── Uninstall (Settings) ────────────────────────────────────────────────

/// The retired `k2so` shim's mark (the second line of `cli/k2so`).
const SHIM_MARK: &str = "k2so \u{2192} k2 transition shim";

/// `path` (named `name` in `~/.local/bin`) is K2's to remove: a link K2
/// made ([`ci::is_our_link`]: to the script or the k5 binary, even when the
/// target is gone), or, for `k2`, the copy Settings wrote (a file that is
/// the K2 CLI) and, for `k2so`, the shim's copy. A `k5` is K2's only as a
/// link; a folder never is.
fn is_ours_to_remove(path: &Path, name: &str) -> bool {
    if ci::is_our_link(path) {
        return true;
    }
    let Ok(meta) = fs::symlink_metadata(path) else { return false };
    if !meta.file_type().is_file() {
        return false;
    }
    match name {
        CLI_NAME => ci::is_k2_cli_script(path),
        ci::LEGACY_NAME => fs::read(path).is_ok_and(|b| String::from_utf8_lossy(&b[..b.len().min(4096)]).contains(SHIM_MARK)),
        _ => false,
    }
}

/// Settings' uninstall on Linux: `k2`, `k5` and `k2so` in `~/.local/bin`,
/// each only when it is K2's ([`is_ours_to_remove`]), the same rule as the
/// macOS uninstall's. Someone else's `k5` (a file, a folder, a link to
/// another tool) stays, and so does what a link points at (the script, the
/// k5 binary). Never the distro package's `/usr/bin/k2`. Returns what went;
/// stops at the first that can't be removed.
pub fn uninstall_user(home: &Path) -> Result<Vec<PathBuf>, String> {
    let dir = ci::user_bin_dir(home);
    let mut gone = Vec::new();
    for name in ci::UNINSTALL_NAMES {
        let path = dir.join(name);
        if is_ours_to_remove(&path, name) {
            fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))?;
            gone.push(path);
        }
    }
    Ok(gone)
}

/// The tests name the k5 binary's file, which this file doesn't
/// (`tests/packaging/k5_cli_shipped_off_by_default.sh`).
#[cfg(test)]
#[path = "cli_copy_tests.rs"]
mod tests;
