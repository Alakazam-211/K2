//! Safe deletion — sends paths to the OS recycle bin instead of
//! permanent unlink. **Never permanently deletes user-authored
//! content.**
//!
//! ## Why
//!
//! 0.37.5 shipped a user report: a workspace's user code got deleted
//! after a "move + pin" interaction. Forensics pointed at
//! `git.rs::remove_worktree`'s fallback path which calls
//! `git worktree remove` (permanent, no recycle bin) when the
//! rename-to-trash fast path fails. Per user directive: **every
//! deletion path that can touch user content goes to the recycle
//! bin. Permanent unlink is reserved for daemon scratch state we
//! own and can recreate (atomic-write temps, drained inbox files,
//! lock files, launchd plists, test cleanup).**
//!
//! ## What's a "user content" path?
//!
//! Anything where:
//! - The user manually created or edited content under it (their
//!   code, AGENT.md they tweaked, work-item markdown they wrote,
//!   skill layer files they authored).
//! - K2SO created it but losing it would surprise the user
//!   (CLAUDE.md, .aider.conf.yml, persona files).
//! - It's the workspace root, a worktree, or any subtree thereof.
//!
//! Helpers in this module take such paths and route them through
//! the OS recycle bin (`trash` crate — handles macOS, Windows,
//! freedesktop Linux). On platforms without a recycle bin (some
//! minimal Linux configs), `trash::delete` returns an error and
//! the helpers surface it; callers can choose to fall back to
//! permanent unlink with explicit acknowledgment, but the default
//! is "if we can't trash, we don't delete."
//!
//! ## What stays as `fs::remove_*`?
//!
//! - `fs_atomic.rs`: atomic-write temp files (the temp IS a
//!   half-finished write that needs cleanup; nothing to recover).
//! - `pending_live::drain*`: drained signal JSONs (already injected
//!   into a session; the file is consumed state, not user content).
//! - `awareness::inbox::drain`: same as above.
//! - `agents::session::*_unlock`: `.lock` file removal.
//! - Test cleanup paths: temp workspaces under `std::env::temp_dir()`.
//! - launchd plist install/uninstall: K2SO-owned, recreatable.
//!
//! Each remaining `fs::remove_*` call site has an inline comment
//! justifying why it's safe.

use std::path::Path;

/// Test builds only: the folder that stands in for the OS Trash
/// (`K2_TEST_TRASH_DIR`; `TempHome` sets it to `<home>/.Trash`).
pub const TEST_TRASH_DIR_ENV: &str = "K2_TEST_TRASH_DIR";

/// The ONE call into the OS recycle bin (the `trash` crate). Every helper
/// here and `fs_commands::delete` go through it.
///
/// 0.46.3: in test builds (`cfg(test)` or k2-core's `test-util`, which only
/// dev-dependencies enable) it never touches the real Trash: on macOS that
/// drives Finder over AppleEvents, which pops Touch ID on a desk Mac and
/// times out (-1712) on the headless compute minis. It moves the path into
/// [`TEST_TRASH_DIR_ENV`] instead, or `<temp>/k2-test-trash` when unset —
/// still a move, never an unlink, so "it went to the Trash" assertions keep
/// meaning something. A release build compiles none of this: production is
/// `trash::delete`, unchanged.
///
/// 0.46.4: inside a [`BootTrashScope`] (the daemon's boot migrations) it
/// takes [`boot_trash`] instead, which never starts Finder and never waits
/// longer than [`boot_trash_timeout`].
pub(crate) fn os_trash(path: &Path) -> Result<(), String> {
    if BOOT_SCOPE.with(|b| b.borrow().is_some()) {
        return boot_trash(path);
    }
    #[cfg(any(test, feature = "test-util"))]
    {
        test_trash(path)
    }
    #[cfg(not(any(test, feature = "test-util")))]
    {
        trash::delete(path).map_err(|e| e.to_string())
    }
}

// ── Boot-safe trash (0.46.4) ────────────────────────────────────────────
//
// The `trash` crate's default macOS method runs `osascript` → Finder. On a
// Mac with no GUI session (headless, before login) or waiting on the
// Finder-automation consent prompt, that child never returns. The daemon's
// first-boot migrations (work → inbox, skills consolidation, unification)
// trash leftovers while `/boot-status` says `migrating` and every real
// route answers 503, so one hung Finder call kept the whole daemon down.
//
// Inside a `BootTrashScope`:
//  * the trash goes through `NSFileManager trashItemAtURL` on macOS (no
//    Finder child, no automation consent; still the user's Trash) and the
//    freedesktop / Windows Trash elsewhere;
//  * each call waits at most `boot_trash_timeout()` on a helper thread;
//  * after one timeout, the rest of the boot doesn't wait at all;
//  * a path that timed out is left exactly where it is (never expunged)
//    and handed back by `BootTrashScope::finish`, so the daemon can trash
//    it in the background once it is ready (`trash_later`).

/// The longest one boot-time trash call may hold the boot.
pub const BOOT_TRASH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Test builds only: override [`BOOT_TRASH_TIMEOUT`] in milliseconds.
pub const TEST_BOOT_TRASH_TIMEOUT_MS_ENV: &str = "K2_TEST_BOOT_TRASH_TIMEOUT_MS";

/// Test builds only: `1` makes every trash call in this process hang, like
/// a Finder child that never returns (the daemon binary's boot test).
pub const TEST_TRASH_HANG_ENV: &str = "K2_TEST_TRASH_HANG";

#[derive(Default)]
struct BootScopeState {
    /// A call timed out: the rest of this boot defers without waiting.
    tripped: bool,
    /// Paths left in place for [`trash_later`].
    deferred: Vec<std::path::PathBuf>,
}

thread_local! {
    static BOOT_SCOPE: std::cell::RefCell<Option<BootScopeState>> = const { std::cell::RefCell::new(None) };
}

/// While alive, every trash call made on THIS thread is boot-safe (see the
/// section comment). The daemon holds one around its synchronous boot
/// migrations; hold it across no `.await`.
pub struct BootTrashScope {
    _not_send: std::marker::PhantomData<*const ()>,
}

impl BootTrashScope {
    pub fn enter() -> Self {
        BOOT_SCOPE.with(|b| *b.borrow_mut() = Some(BootScopeState::default()));
        Self { _not_send: std::marker::PhantomData }
    }

    /// Leave the scope; the paths whose trash timed out (still in place).
    pub fn finish(self) -> Vec<std::path::PathBuf> {
        let out = BOOT_SCOPE.with(|b| b.borrow_mut().as_mut().map(|s| std::mem::take(&mut s.deferred)));
        drop(self);
        out.unwrap_or_default()
    }
}

impl Drop for BootTrashScope {
    fn drop(&mut self) {
        BOOT_SCOPE.with(|b| *b.borrow_mut() = None);
    }
}

/// [`BOOT_TRASH_TIMEOUT`], or the test override.
pub fn boot_trash_timeout() -> std::time::Duration {
    #[cfg(any(test, feature = "test-util"))]
    {
        if let Some(ms) = std::env::var(TEST_BOOT_TRASH_TIMEOUT_MS_ENV).ok().and_then(|v| v.trim().parse::<u64>().ok()) {
            return std::time::Duration::from_millis(ms);
        }
    }
    BOOT_TRASH_TIMEOUT
}

fn boot_deferred(path: &Path, why: &str) -> String {
    BOOT_SCOPE.with(|b| {
        if let Some(s) = b.borrow_mut().as_mut() {
            s.tripped = true;
            s.deferred.push(path.to_path_buf());
        }
    });
    format!("{why}; left in place, trashed in the background once the daemon is ready")
}

fn boot_trash(path: &Path) -> Result<(), String> {
    if BOOT_SCOPE.with(|b| b.borrow().as_ref().is_some_and(|s| s.tripped)) {
        return Err(boot_deferred(path, "the Trash did not answer earlier in this boot"));
    }
    let timeout = boot_trash_timeout();
    let owned = path.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("k2-boot-trash".into())
        .spawn(move || {
            let _ = tx.send(trash_without_finder(&owned));
        })
        .map_err(|e| format!("could not start the trash helper thread: {e}"))?;
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => Err(boot_deferred(path, &format!("the Trash did not answer within {} ms", timeout.as_millis()))),
    }
}

/// The OS Trash without a Finder child: `NSFileManager` on macOS (no
/// "Put Back", but no AppleEvents, consent prompt or GUI session needed),
/// the `trash` crate's default elsewhere. Test builds: the test trash dir.
fn trash_without_finder(path: &Path) -> Result<(), String> {
    #[cfg(any(test, feature = "test-util"))]
    {
        test_trash(path)
    }
    #[cfg(all(not(any(test, feature = "test-util")), target_os = "macos"))]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(path).map_err(|e| e.to_string())
    }
    #[cfg(all(not(any(test, feature = "test-util")), not(target_os = "macos")))]
    {
        trash::delete(path).map_err(|e| e.to_string())
    }
}

/// Trash the paths a [`BootTrashScope`] deferred, on a background thread,
/// without Finder. A path already gone counts as done; a failure is logged
/// and the path stays where it is (never expunged).
pub fn trash_later(paths: Vec<std::path::PathBuf>) -> Option<std::thread::JoinHandle<()>> {
    if paths.is_empty() {
        return None;
    }
    std::thread::Builder::new()
        .name("k2-deferred-trash".into())
        .spawn(move || {
            for p in paths {
                if !p.exists() {
                    continue;
                }
                match trash_without_finder(&p) {
                    Ok(()) => crate::log_debug!("[safe_delete] deferred trash done: {}", p.display()),
                    Err(e) => crate::log_debug!(
                        "[safe_delete] deferred trash {} failed: {e} (left in place; never deleted)",
                        p.display()
                    ),
                }
            }
        })
        .ok()
}

/// Test builds only: paths whose trash hangs until released (in-process
/// tests; the daemon binary uses [`TEST_TRASH_HANG_ENV`]).
#[cfg(any(test, feature = "test-util"))]
static TEST_HANG_PATHS: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Test builds only: make the trash of `path` hang until [`test_release_trash_hang`].
#[cfg(any(test, feature = "test-util"))]
pub fn test_hang_trash_of(path: &Path) {
    TEST_HANG_PATHS.lock().expect("hang paths").push(path.to_path_buf());
}

/// Test builds only: release a [`test_hang_trash_of`] path; its hung call
/// then fails without moving anything.
#[cfg(any(test, feature = "test-util"))]
pub fn test_release_trash_hang(path: &Path) {
    TEST_HANG_PATHS.lock().expect("hang paths").retain(|p| p != path);
}

#[cfg(any(test, feature = "test-util"))]
fn test_trash(path: &Path) -> Result<(), String> {
    if std::env::var(TEST_TRASH_HANG_ENV).is_ok_and(|v| v == "1") {
        // A Finder child that never returns.
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    if TEST_HANG_PATHS.lock().expect("hang paths").iter().any(|p| p == path) {
        while TEST_HANG_PATHS.lock().expect("hang paths").iter().any(|p| p == path) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        return Err(format!("test hang released for {}", path.display()));
    }
    let dir = std::env::var_os(TEST_TRASH_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("k2-test-trash"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("test trash dir {}: {e}", dir.display()))?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".into());
    let dest = dir.join(format!("{name}-{}", uuid::Uuid::new_v4().simple()));
    match std::fs::rename(path, &dest) {
        Ok(()) => Ok(()),
        // Another filesystem (a temp HOME on one volume, the item on
        // another): copy then remove, like a Trash move across volumes.
        Err(_) => {
            copy_recursive(path, &dest).map_err(|e| format!("test trash copy {}: {e}", path.display()))?;
            if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            }
            .map_err(|e| format!("test trash remove {}: {e}", path.display()))
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Send a path (file or directory) to the OS recycle bin. Returns
/// an error if the trash operation fails (most commonly: missing
/// trash service on minimal Linux, permission denied, path doesn't
/// exist). Callers should surface the error rather than silently
/// fall back to permanent deletion — the whole point of this
/// helper is "don't permanently destroy user data."
pub fn trash<P: AsRef<Path>>(path: P) -> Result<(), String> {
    let path_ref = path.as_ref();
    if !path_ref.exists() {
        // Mirror `fs::remove_*` semantics: not-existing is success.
        return Ok(());
    }
    os_trash(path_ref).map_err(|e| {
        format!(
            "trash {} failed: {} (path NOT permanently deleted; intervene manually)",
            path_ref.display(),
            e
        )
    })
}

/// Best-effort trash. Swallows errors with a `log_debug!` line.
/// Use when the caller previously did `let _ = fs::remove_*(...)`
/// — keeps the same fire-and-forget semantics but routes through
/// the recycle bin. Returns `true` if the path was trashed (or
/// didn't exist), `false` if the trash op failed.
pub fn try_trash<P: AsRef<Path>>(path: P) -> bool {
    match trash(&path) {
        Ok(()) => true,
        Err(e) => {
            crate::log_debug!("[safe_delete] {e}");
            false
        }
    }
}

/// "Trash-preferred, fallback to permanent on failure" — for paths
/// where the content was K2SO-scaffolded with no user-authored
/// material (a SKILL.md file we wrote, a launchd plist we
/// installed). The directive "never permanent-delete user content"
/// still holds; this helper is for content where the worst-case
/// outcome is "K2SO has to re-scaffold a file it created."
///
/// Behavior:
/// 1. Try to trash. If success, return Ok.
/// 2. If trash fails (e.g. CI without Finder, AppleScript timeout,
///    Linux without freedesktop.org Trash service), log + fall
///    back to `fs::remove_*`. The path is gone either way.
///
/// **DO NOT use this for user-authored content** — use plain
/// `trash()` and let the error bubble. If trash fails on user
/// content, the safe answer is "leave it in place and let the
/// user investigate," not "delete it anyway."
pub fn trash_or_remove<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    let path_ref = path.as_ref();
    if !path_ref.exists() {
        return Ok(());
    }
    match os_trash(path_ref) {
        Ok(()) => Ok(()),
        Err(e) => {
            crate::log_debug!(
                "[safe_delete] trash {} failed ({e}); falling back to permanent delete (k2so-scratch path)",
                path_ref.display()
            );
            if path_ref.is_dir() {
                std::fs::remove_dir_all(path_ref)
            } else {
                std::fs::remove_file(path_ref)
            }
        }
    }
}

#[cfg(test)]
mod test_trash_tests {
    use super::*;

    /// Tests never reach the real Trash: the path moves into the test
    /// trash folder, content intact.
    #[test]
    fn trash_in_tests_moves_into_the_test_trash_dir() {
        let home = crate::test_env::TempHome::new();
        let trash_dir = std::path::PathBuf::from(
            std::env::var_os(TEST_TRASH_DIR_ENV).expect("TempHome sets the test trash dir"),
        );
        assert_eq!(trash_dir, home.path().join(".Trash"));
        let f = home.path().join("note.md");
        std::fs::write(&f, "keep me").unwrap();
        trash(&f).expect("trash");
        assert!(!f.exists(), "the file left its folder");
        let moved: Vec<_> = std::fs::read_dir(&trash_dir).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(moved.len(), 1, "{moved:?}");
        assert_eq!(std::fs::read_to_string(&moved[0]).unwrap(), "keep me", "moved, never unlinked");

        let d = home.path().join("folder");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/x"), "x").unwrap();
        trash_or_remove(&d).expect("trash_or_remove");
        assert!(!d.exists());
        assert_eq!(std::fs::read_dir(&trash_dir).unwrap().count(), 2);
    }

    /// 0.46.4: inside a boot scope a hung Trash holds the caller for at
    /// most the boot timeout, the path stays in place (never deleted), the
    /// rest of the boot doesn't wait at all, and `finish` hands the paths
    /// back for `trash_later`.
    #[test]
    fn boot_scope_never_waits_on_a_hung_trash_and_never_deletes() {
        let home = crate::test_env::TempHome::new();
        let hung = home.path().join("work");
        std::fs::create_dir_all(hung.join("inbox")).unwrap();
        std::fs::write(hung.join("inbox/a.md"), "keep me").unwrap();
        let second = home.path().join("agents");
        std::fs::create_dir_all(&second).unwrap();
        test_hang_trash_of(&hung);

        let scope = BootTrashScope::enter();
        let t0 = std::time::Instant::now();
        let err = trash(&hung).expect_err("a hung trash must not report success");
        let first = t0.elapsed();
        assert!(first >= BOOT_TRASH_TIMEOUT - std::time::Duration::from_millis(50), "waited the timeout: {first:?}");
        assert!(first < BOOT_TRASH_TIMEOUT + std::time::Duration::from_secs(5), "bounded: {first:?}");
        assert!(err.contains("left in place"), "{err}");
        assert!(err.contains("NOT permanently deleted"), "{err}");
        assert_eq!(std::fs::read_to_string(hung.join("inbox/a.md")).unwrap(), "keep me", "never deleted");

        // Tripped: the next call defers at once, its path untouched too.
        let t1 = std::time::Instant::now();
        let err2 = trash(&second).expect_err("tripped scope defers");
        assert!(t1.elapsed() < std::time::Duration::from_secs(1), "no second wait: {:?}", t1.elapsed());
        assert!(err2.contains("did not answer earlier in this boot"), "{err2}");
        assert!(second.is_dir(), "never deleted");

        let deferred = scope.finish();
        assert_eq!(deferred, vec![hung.clone(), second.clone()]);

        // Outside the scope trash is the normal path again (no deferral).
        let f = home.path().join("plain.md");
        std::fs::write(&f, "x").unwrap();
        trash(&f).expect("normal trash after the scope");
        assert!(!f.exists());

        // The background pass trashes what was deferred (moved, not unlinked).
        test_release_trash_hang(&hung);
        trash_later(deferred).expect("a thread for two paths").join().expect("deferred thread");
        assert!(!hung.exists() && !second.exists(), "deferred paths went to the Trash");
        let trash_dir = home.path().join(".Trash");
        let names: Vec<String> = std::fs::read_dir(&trash_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n.starts_with("work-")), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("agents-")), "{names:?}");
    }

    /// A scope that never sees a hang trashes normally (no deferral).
    #[test]
    fn boot_scope_trashes_normally_when_the_trash_answers() {
        let home = crate::test_env::TempHome::new();
        let d = home.path().join("leftover");
        std::fs::create_dir_all(&d).unwrap();
        let scope = BootTrashScope::enter();
        trash(&d).expect("boot trash");
        assert!(!d.exists());
        assert!(scope.finish().is_empty());
        assert!(trash_later(Vec::new()).is_none());
    }
}
