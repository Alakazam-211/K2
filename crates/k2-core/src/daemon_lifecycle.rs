//! Headless-friendly abstractions for daemon lifecycle wiring.
//!
//! **K2 Connect prep**: this module owns the *logic* of "where does the
//! daemon binary live?" and "what arguments would launchctl need to
//! load / kick / unload the daemon plist?" — independent of any actual
//! filesystem-touching or process-spawning. K2 Connect's headless flow
//! (Tauri-less daemons running on remote hosts) calls into the same
//! plist generator + arg builders here so a remote-daemon scenario
//! can produce a launchd plist + bootstrap script without dragging in
//! Tauri.
//!
//! `crate::wake` owns the FS write + actual `launchctl load/unload/list`
//! invocations. This module owns the **shape** of those calls: the
//! canonical label, the relative path where K2SO.app keeps the daemon
//! binary, the launchctl kickstart target string. The Tauri commands
//! in `src-tauri/src/commands/daemon.rs` are now thin wrappers around
//! these helpers + a plain `Command::new("launchctl")` invocation.
//!
//! Why a separate module from `wake`?
//! - `wake` is the implementation surface for "install + unload + check
//!   loaded state." Heavy because it writes files and shells out.
//! - `daemon_lifecycle` is the **pure** surface: no FS, no subprocess.
//!   Pure data + arg-vec construction. Cheap to test in isolation and
//!   trivial to call from K2 Connect orchestration code that doesn't
//!   want to accidentally trigger an actual install.

use std::path::{Path, PathBuf};

/// Reverse-DNS launch agent label used by the daemon plist on macOS.
///
/// Mirrors `crate::wake::DaemonPlist::canonical().label`. Re-exported
/// here so callers building `launchctl` arg vectors (kickstart target,
/// `list <label>` probes) don't have to import `wake::DaemonPlist`
/// just for the string.
pub const DAEMON_LAUNCH_AGENT_LABEL: &str = "dev.k2.daemon";

/// Filename of the daemon binary as bundled inside `K2SO.app`.
///
/// Used by `bundled_daemon_path` to resolve `K2SO.app/Contents/MacOS/k2so-daemon`
/// from the current Tauri binary path. Centralized so the build script,
/// the Tauri "Install daemon" command, and K2 Connect's "where's the
/// daemon binary?" probe all agree.
pub const DAEMON_BINARY_NAME: &str = "k2-daemon";

/// Pure-logic resolution: given the path of a Tauri executable
/// (typically `K2SO.app/Contents/MacOS/k2so`), return the expected
/// path of the bundled `k2so-daemon` binary alongside it. This does
/// **not** touch the filesystem — callers (Tauri `daemon_install` or
/// K2 Connect's headless bootstrap) check `.exists()` themselves.
///
/// Returns `None` when `tauri_exe` has no parent directory (only
/// happens for pathological inputs like `/` or relative-empty paths).
///
/// Why split path-resolution from the FS check?
/// - Tests can drive this against synthetic paths without needing a
///   real binary on disk.
/// - K2 Connect can compute the "if I were to install a daemon plist
///   pointing at this Tauri install, this is the binary path it would
///   reference" without actually performing the install.
pub fn bundled_daemon_path(tauri_exe: &Path) -> Option<PathBuf> {
    tauri_exe.parent().map(|d| d.join(DAEMON_BINARY_NAME))
}

/// Build the `launchctl kickstart -k <target>` argument vector that
/// SIGTERMs the running daemon and lets launchd respawn it (because
/// `KeepAlive: true`). `uid` is the Unix UID the daemon plist is
/// loaded into — typically `unsafe { libc::getuid() }` from the caller.
///
/// Pure: returns the arg vector; the caller invokes `Command::new`.
/// Separating this out means K2 Connect can compose the same exact
/// command for a remote-machine kickstart via SSH without dragging in
/// `std::process::Command`.
pub fn launchctl_kickstart_args(uid: u32) -> Vec<String> {
    vec![
        "kickstart".to_string(),
        "-k".to_string(),
        format!("gui/{}/{}", uid, DAEMON_LAUNCH_AGENT_LABEL),
    ]
}

/// Build the `launchctl list <label>` argument vector used to probe
/// whether the daemon plist is currently loaded. Exit-code zero means
/// loaded. Mirror of [`launchctl_kickstart_args`] for symmetry — no FS,
/// no spawning, just the arg vec.
pub fn launchctl_list_args() -> Vec<String> {
    vec!["list".to_string(), DAEMON_LAUNCH_AGENT_LABEL.to_string()]
}

/// Generate the plist XML body the canonical daemon launch agent would
/// install. Thin re-export of `crate::wake::DaemonPlist::canonical(...).to_xml()`
/// — exposed at this path so K2 Connect callers can grab "what plist
/// would K2SO write for this daemon binary?" without going through the
/// `wake` module's install-shaped surface.
///
/// `program` is the path the plist's `ProgramArguments` will point at.
/// On a local install that's `~/Applications/K2SO.app/Contents/MacOS/k2so-daemon`;
/// on a K2 Connect remote install it's whatever path the remote-side
/// daemon binary lives at.
pub fn generate_plist_content(program: PathBuf) -> String {
    crate::wake::DaemonPlist::canonical(program).to_xml()
}

/// True when `p` lives in a *transient* macOS location — i.e. one that
/// disappears or moves out from under launchd, so baking it into a
/// `KeepAlive`/`RunAtLoad` plist would respawn a stale binary forever
/// (GitHub #14).
///
/// Two transient cases:
/// 1. A mounted DMG / removable volume: anything under `/Volumes/`.
///    First-running K2SO straight from the mounted installer records
///    `/Volumes/K2SO/K2SO.app/Contents/MacOS/k2so-daemon`; once the
///    user ejects the DMG that path is gone but launchd keeps trying.
/// 2. Gatekeeper App Translocation: when an app is launched from a
///    quarantined/random location, macOS copies it into a randomized,
///    read-only mount under `/private/var/folders/.../AppTranslocation/...`.
///    That path also vanishes after the launching process exits.
///
/// Stable locations (`/Applications/...`, a user's home dir, a dev
/// `…/target/release/…` build) return `false` — we trust those and
/// will happily bake them into the plist.
///
/// Pure string/path classification: no FS touch. Lives here so the
/// install-time guard (`src-tauri/src/lib.rs`) and the self-heal path
/// share one source of truth and can be unit-tested without mounting a
/// real DMG.
pub fn is_transient_exe_location(p: &Path) -> bool {
    let s = p.to_string_lossy();
    // Mounted volumes (DMGs, external disks). The startup volume is
    // `/` not `/Volumes/...`, and `/Applications` is on the startup
    // volume, so this does not catch normal installs.
    if s.starts_with("/Volumes/") {
        return true;
    }
    // App Translocation: macOS mounts a randomized read-only copy under
    // `/private/var/folders/<…>/AppTranslocation/<uuid>/d/K2SO.app/...`.
    // Match the marker component anywhere in the path rather than a
    // brittle prefix — the `/private/var/folders` root and the random
    // segment both vary.
    if s.contains("/AppTranslocation/") {
        return true;
    }
    false
}

/// Decide whether the daemon LaunchAgent plist's recorded program path
/// should be rewritten to `desired` (GitHub #14).
///
/// The bug: launchd baked a transient path (mounted DMG / AppTranslocation)
/// into `ProgramArguments[0]`; after the app moves to `/Applications` and
/// upgrades, launchd respawns that stale/missing binary forever and the
/// version-mismatch check kickstarts the same bad path, so it never
/// converges.
///
/// Rules (conservative — must not churn the dev-box case where the
/// daemon legitimately runs from `…/target/release/k2so-daemon`):
/// - If the *current* exe is itself transient, never rewrite — we can't
///   trust the path we'd write, and a real `/Applications` launch will
///   fix it later.
/// - Otherwise rewrite only when the recorded path is itself transient,
///   or the recorded plist program is missing on disk. Those are the
///   genuinely-broken states.
/// - A recorded path that is a *different but stable and existing* path
///   (e.g. a dev box that legitimately points at `…/target/release/…`)
///   is left alone — we do NOT rewrite merely because `recorded !=
///   desired`. This keeps dev boxes from fighting the desktop app over
///   the plist on every launch.
///
/// Pure decision logic: all FS facts (`recorded_exists`,
/// `current_is_transient`) are passed in by the caller so this is unit
/// testable without touching disk or mounting volumes.
pub fn should_rewrite_plist(
    recorded: &Path,
    desired: &Path,
    recorded_exists: bool,
    current_is_transient: bool,
) -> bool {
    // Never trust a transient current exe to seed the plist.
    if current_is_transient {
        return false;
    }
    // Recorded path points at a transient (DMG / translocated) location
    // → definitely broken, rewrite to our stable `desired`.
    if is_transient_exe_location(recorded) {
        return true;
    }
    // Recorded binary is gone from disk → broken, rewrite.
    if !recorded_exists {
        return true;
    }
    // Already pointing at us → nothing to do.
    if recorded == desired {
        return false;
    }
    // Recorded is a *different* path that is stable AND exists — leave
    // it alone. This is the dev-box case (`…/target/release/k2so-daemon`)
    // and we must not churn it on every launch.
    false
}

/// Extract `ProgramArguments[0]` (the daemon program path) from a
/// launchd plist XML body. Pure string parse — no FS, no plist library
/// — so the self-heal path in `src-tauri/src/lib.rs` can read the
/// recorded program out of the on-disk plist and feed it to
/// [`should_rewrite_plist`] without pulling in a plist crate.
///
/// Returns `None` if there's no `ProgramArguments` array or it has no
/// first `<string>` entry. Mirrors the writer in
/// [`crate::wake::DaemonPlist::to_xml`], which emits the program as the
/// first `<string>` inside the `ProgramArguments` `<array>`.
pub fn parse_plist_program(xml: &str) -> Option<PathBuf> {
    // Find the ProgramArguments key, then the array opened after it,
    // then the first <string>…</string> inside that array.
    let key_pos = xml.find("<key>ProgramArguments</key>")?;
    let after_key = &xml[key_pos..];
    let arr_open = after_key.find("<array>")?;
    let after_arr = &after_key[arr_open..];
    let arr_close_rel = after_arr.find("</array>").unwrap_or(after_arr.len());
    let array_body = &after_arr[..arr_close_rel];
    let s_open = array_body.find("<string>")? + "<string>".len();
    let s_close = array_body[s_open..].find("</string>")? + s_open;
    let raw = &array_body[s_open..s_close];
    let unescaped = xml_unescape(raw);
    if unescaped.is_empty() {
        return None;
    }
    Some(PathBuf::from(unescaped))
}

/// Inverse of `wake::xml_escape` for the minimal entity set the writer
/// emits (`&lt; &gt; &amp;`). Order matters: decode `&amp;` last so a
/// literal `&lt;` in the source isn't mangled.
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_plist_content_includes_correct_label() {
        let xml = generate_plist_content(PathBuf::from("/opt/k2so-daemon"));
        assert!(
            xml.contains(&format!("<string>{DAEMON_LAUNCH_AGENT_LABEL}</string>")),
            "plist missing daemon label: {xml}"
        );
    }

    #[test]
    fn generate_plist_content_points_to_daemon_binary() {
        let xml = generate_plist_content(PathBuf::from("/Applications/K2SO.app/Contents/MacOS/k2so-daemon"));
        assert!(
            xml.contains("<string>/Applications/K2SO.app/Contents/MacOS/k2so-daemon</string>"),
            "plist missing daemon binary path: {xml}"
        );
    }

    #[test]
    fn launchctl_kickstart_arg_format() {
        let args = launchctl_kickstart_args(501);
        assert_eq!(
            args,
            vec![
                "kickstart".to_string(),
                "-k".to_string(),
                "gui/501/dev.k2.daemon".to_string(),
            ],
            "kickstart args shape changed; bumping launchctl is wire-incompatible"
        );
    }

    #[test]
    fn launchctl_list_arg_format() {
        let args = launchctl_list_args();
        assert_eq!(
            args,
            vec!["list".to_string(), "dev.k2.daemon".to_string()],
        );
    }

    #[test]
    fn bundled_daemon_path_resolves_next_to_tauri_exe() {
        let path = bundled_daemon_path(Path::new(
            "/Applications/K2.app/Contents/MacOS/k2",
        ));
        assert_eq!(
            path,
            Some(PathBuf::from("/Applications/K2.app/Contents/MacOS/k2-daemon")),
        );
    }

    #[test]
    fn bundled_daemon_path_returns_none_for_pathological_input() {
        // `Path::new("").parent()` returns None — `bundled_daemon_path`
        // must surface that as None instead of panicking.
        let path = bundled_daemon_path(Path::new(""));
        assert!(path.is_none(), "empty path should resolve to None, got {path:?}");
    }

    // ── #14: transient-location classification ──────────────────────

    #[test]
    fn transient_true_under_volumes() {
        // First-run straight from the mounted DMG.
        assert!(is_transient_exe_location(Path::new(
            "/Volumes/K2SO/K2SO.app/Contents/MacOS/k2so-daemon"
        )));
    }

    #[test]
    fn transient_true_for_app_translocation() {
        // Gatekeeper-translocated randomized read-only copy.
        assert!(is_transient_exe_location(Path::new(
            "/private/var/folders/qz/abc123/T/AppTranslocation/9F1C-UUID/d/K2SO.app/Contents/MacOS/k2so-daemon"
        )));
    }

    #[test]
    fn transient_false_for_applications() {
        assert!(!is_transient_exe_location(Path::new(
            "/Applications/K2SO.app/Contents/MacOS/k2so-daemon"
        )));
    }

    #[test]
    fn transient_false_for_home_dir() {
        assert!(!is_transient_exe_location(Path::new(
            "/Users/rosson/Applications/K2SO.app/Contents/MacOS/k2so-daemon"
        )));
    }

    #[test]
    fn transient_false_for_dev_target_release() {
        // Dev-box build path must be treated as STABLE.
        assert!(!is_transient_exe_location(Path::new(
            "/Users/rosson/DevProjects/K2SO/target/release/k2so-daemon"
        )));
    }

    // ── #14: should_rewrite_plist decision matrix ───────────────────

    const APPLE_DAEMON: &str = "/Applications/K2SO.app/Contents/MacOS/k2so-daemon";

    #[test]
    fn rewrite_when_recorded_is_transient() {
        // Recorded points at a vanished DMG; current is the stable
        // /Applications binary → rewrite to converge.
        assert!(should_rewrite_plist(
            Path::new("/Volumes/K2SO/K2SO.app/Contents/MacOS/k2so-daemon"),
            Path::new(APPLE_DAEMON),
            /* recorded_exists */ false,
            /* current_is_transient */ false,
        ));
    }

    #[test]
    fn rewrite_when_recorded_missing() {
        // Recorded is a plausible stable path but the binary is gone.
        assert!(should_rewrite_plist(
            Path::new("/Applications/Old.app/Contents/MacOS/k2so-daemon"),
            Path::new(APPLE_DAEMON),
            /* recorded_exists */ false,
            /* current_is_transient */ false,
        ));
    }

    #[test]
    fn no_rewrite_when_recorded_matches_desired() {
        assert!(!should_rewrite_plist(
            Path::new(APPLE_DAEMON),
            Path::new(APPLE_DAEMON),
            /* recorded_exists */ true,
            /* current_is_transient */ false,
        ));
    }

    #[test]
    fn no_rewrite_for_different_stable_existing_recorded() {
        // Dev-box: plist legitimately points at …/target/release; the
        // desktop app must NOT fight it on every launch.
        assert!(!should_rewrite_plist(
            Path::new("/Users/rosson/DevProjects/K2SO/target/release/k2so-daemon"),
            Path::new(APPLE_DAEMON),
            /* recorded_exists */ true,
            /* current_is_transient */ false,
        ));
    }

    #[test]
    fn no_rewrite_when_current_is_transient() {
        // Even though recorded is transient + missing, we can't trust a
        // transient current exe to seed the plist.
        assert!(!should_rewrite_plist(
            Path::new("/Volumes/K2SO/K2SO.app/Contents/MacOS/k2so-daemon"),
            Path::new("/Volumes/K2SO/K2SO.app/Contents/MacOS/k2so-daemon"),
            /* recorded_exists */ false,
            /* current_is_transient */ true,
        ));
    }

    // ── #14: plist program parsing (round-trip with the writer) ─────

    #[test]
    fn parse_program_round_trips_writer_output() {
        let xml = generate_plist_content(PathBuf::from(APPLE_DAEMON));
        let got = parse_plist_program(&xml);
        assert_eq!(got, Some(PathBuf::from(APPLE_DAEMON)));
    }

    #[test]
    fn parse_program_unescapes_special_chars() {
        let weird = "/tmp/has<weird>&stuff/k2so-daemon";
        let xml = generate_plist_content(PathBuf::from(weird));
        let got = parse_plist_program(&xml);
        assert_eq!(got, Some(PathBuf::from(weird)));
    }

    #[test]
    fn parse_program_none_without_program_arguments() {
        assert_eq!(parse_plist_program("<plist><dict></dict></plist>"), None);
    }

    #[test]
    fn daemon_label_constant_matches_wake_canonical() {
        // Guards against the constant drifting from
        // `DaemonPlist::canonical`'s label. Keeps the two sources
        // synchronized without runtime coupling.
        let p = crate::wake::DaemonPlist::canonical(PathBuf::from("/opt/k2so-daemon"));
        assert_eq!(p.label, DAEMON_LAUNCH_AGENT_LABEL);
    }
}
