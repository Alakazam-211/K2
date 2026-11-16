//! Agent names in Activity Monitor (owner switch `agentProcessNames`, off by
//! default).
//!
//! **Why a hard link, not a symlink.** macOS names a process after the file
//! the kernel finally exec'd, AFTER symlinks are resolved: `p_comm` (16
//! bytes, what `ps -c`/`top` show) and `p_name` (32 bytes, what `proc_name`
//! and Activity Monitor show) both come from that file's basename; `argv[0]`
//! changes neither. Claude Code's native installer keeps each version as a
//! plain file named by its version (`~/.local/share/claude/versions/2.1.322`)
//! and `~/.local/bin/claude` is a symlink to it, so every agent shows as
//! `2.1.322`. A per-agent symlink would show the same. A hard link is a second
//! NAME for the same file, so the kernel names the process after the link.
//! (Linux names a process after the path given to `execve`, symlink or not;
//! a hard link works there too, so one path serves both.)
//!
//! **Why it is safe for Claude Code** (checked against 2.1.325's own source):
//! * The install type stays `native`: a standalone build is `native` unless
//!   its path is under npm / Homebrew Caskroom / mise / asdf or owned by a
//!   system package. `~/.k2/bin/agents/` is none of those.
//! * Updates only write `versions/` and re-point `~/.local/bin/claude`;
//!   nothing here is touched, and K2 never touches those.
//! * A process started from a link doesn't take the "running version" lock
//!   (its path isn't under `versions/`), so the installer may delete that
//!   version's file. The link keeps the file's data alive, so the running
//!   agent is unaffected; the space comes back when the link is re-pointed.
//! * Logins: Claude Code reads the keychain through `/usr/bin/security`, so
//!   the keychain item's access list never sees this path.
//! * App-bundle builds (`versions/bundles/…/claude.app/Contents/MacOS/claude`)
//!   are never linked: a bundle's main program belongs inside its bundle.
//!
//! **Which harnesses.** Only [`VERIFIED_HARNESSES`], and only when the
//! resolved file is a native program (Mach-O or ELF). Node-script CLIs
//! (Codex's and Gemini's npm launchers) run as `node`; the kernel names them
//! after the interpreter, and nothing outside the tool can rename it on
//! macOS. Grok ships a native build with its own updater that hasn't been
//! checked yet, so it isn't on the list.
//!
//! **Re-pointed on every spawn.** Each spawn links the CURRENT resolved
//! binary under the agent's name (atomic rename over the old link), so an
//! update is followed by the next session. Sessions already running keep
//! the version they started with, as they do today.
//!
//! Any failure (another volume, permissions, a missing binary) falls back to
//! the program unchanged: the switch can only change a process's name.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The settings wire key (`AppSettings::agent_process_names`).
pub const SETTING_KEY: &str = "agentProcessNames";

/// macOS `MAXCOMLEN`: `p_comm` (what `ps -c` and `top` show) keeps 16
/// bytes. Linux keeps 15 in `comm`. Names are cut to this so every tool shows
/// the same name.
pub const NAME_MAX: usize = 16;

/// Harness basenames checked to run correctly from a hard link.
pub const VERIFIED_HARNESSES: &[&str] = &["claude"];

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Mirror of the owner switch (boot + every settings write).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether new agent sessions launch through a link named after the agent.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// `<home>/.k2/bin/agents`: the links. A folder of its own, never on PATH.
pub fn link_dir_at(home: &Path) -> PathBuf {
    home.join(".k2").join("bin").join("agents")
}

/// The process name for an agent's session: the agent's name, plus
/// `.<sidecar>` for a sidecar, cut to [`NAME_MAX`] bytes. Only
/// `[A-Za-z0-9._-]`; anything else becomes `-`. `None` when nothing usable
/// is left.
pub fn process_name(primary: &str, sidecar: Option<&str>) -> Option<String> {
    let primary = sanitize(primary);
    if primary.is_empty() {
        return None;
    }
    let mut name = match sidecar.map(sanitize).filter(|s| !s.is_empty()) {
        Some(s) => format!("{primary}.{s}"),
        None => primary,
    };
    // ASCII only after sanitize, so a byte cut is a char cut.
    name.truncate(NAME_MAX);
    let name = name.trim_end_matches(['-', '.', '_']).to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn sanitize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.trim().chars() {
        let c = if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '-' };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    out.trim_matches(['-', '.']).to_string()
}

/// The agent a spawn belongs to, from the identity env the daemon stamps
/// (`K2_PRIMARY` + `K2_SIDECAR_NAME` for a sidecar; `K2_AGENT_NAME` on a
/// host session).
pub fn name_from_env(env: &HashMap<String, String>) -> Option<String> {
    let primary = env
        .get("K2_PRIMARY")
        .or_else(|| env.get("K2_AGENT_NAME"))
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())?;
    let sidecar = match env.get("K2_CELL").map(String::as_str) {
        Some("sidecar") => env.get("K2_SIDECAR_NAME").map(String::as_str),
        _ => None,
    };
    process_name(primary, sidecar)
}

/// Why a spawn did or didn't get a named link (logged by the caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The switch is off: the program is unchanged.
    Off,
    /// Not a harness on [`VERIFIED_HARNESSES`].
    NotVerified,
    /// No agent identity on the spawn (a plain shell, a login cell…).
    NoAgentName,
    /// The program isn't on PATH (the spawn itself reports that).
    NotFound,
    /// A script (`#!…`) or anything that isn't a native program.
    NotNative(PathBuf),
    /// The main program of an app bundle.
    InAppBundle(PathBuf),
    /// Linking failed (another volume, permissions…): program unchanged.
    LinkFailed(String),
    /// Launch this path instead.
    Linked(PathBuf),
}

/// Pick the program to exec for `program`. `Some(path)` only when a link
/// was made; every other case keeps `program` as it is.
#[cfg(unix)]
pub fn apply(program: &str, env: &HashMap<String, String>, path: &str) -> (Option<String>, Outcome) {
    if !enabled() {
        return (None, Outcome::Off);
    }
    let Some(home) = crate::paths::home_dir() else {
        return (None, Outcome::LinkFailed("no home dir".into()));
    };
    let outcome = plan(program, env, path, &link_dir_at(&home));
    let linked = match &outcome {
        Outcome::Linked(p) => Some(p.to_string_lossy().into_owned()),
        _ => None,
    };
    (linked, outcome)
}

/// Windows names a process by its image file's name too, but there is
/// nothing checked there yet: unchanged.
#[cfg(not(unix))]
pub fn apply(_program: &str, _env: &HashMap<String, String>, _path: &str) -> (Option<String>, Outcome) {
    (None, Outcome::Off)
}

/// [`apply`] without the switch and with an explicit link folder (tests).
pub fn plan(program: &str, env: &HashMap<String, String>, path: &str, dir: &Path) -> Outcome {
    let base = Path::new(program).file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !VERIFIED_HARNESSES.contains(&base) {
        return Outcome::NotVerified;
    }
    let Some(name) = name_from_env(env) else {
        return Outcome::NoAgentName;
    };
    let Some(found) = resolve_on_path(program, path) else {
        return Outcome::NotFound;
    };
    // The real file behind `~/.local/bin/claude` and any other symlinks.
    let target = match std::fs::canonicalize(&found) {
        Ok(t) => t,
        Err(_) => return Outcome::NotFound,
    };
    if in_app_bundle(&target) {
        return Outcome::InAppBundle(target);
    }
    if !is_native_program(&target) {
        return Outcome::NotNative(target);
    }
    match ensure_link(dir, &name, &target) {
        Ok(link) => Outcome::Linked(link),
        Err(e) => Outcome::LinkFailed(format!("{}: {e}", dir.join(&name).display())),
    }
}

/// `program` as execvp would find it: a path with a `/` as is, else the
/// first executable file named `program` on `path`.
pub fn resolve_on_path(program: &str, path: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let p = PathBuf::from(program);
        return is_executable_file(&p).then_some(p);
    }
    std::env::split_paths(path)
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join(program))
        .find(|p| is_executable_file(p))
}

fn is_executable_file(p: &Path) -> bool {
    let Ok(m) = std::fs::metadata(p) else {
        return false;
    };
    if !m.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// `…/X.app/Contents/…`: part of an app bundle.
fn in_app_bundle(p: &Path) -> bool {
    let parts: Vec<_> = p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    parts.windows(2).any(|w| w[0].ends_with(".app") && w[1] == "Contents")
}

/// Mach-O (thin or universal, either byte order) or ELF.
pub fn is_native_program(p: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 4];
    let Ok(mut f) = std::fs::File::open(p) else {
        return false;
    };
    if f.read_exact(&mut head).is_err() {
        return false;
    }
    matches!(
        head,
        [0xfe, 0xed, 0xfa, 0xce]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xcf, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
            | [0x7f, b'E', b'L', b'F']
    )
}

/// `<dir>/<name>` as a hard link to `target`. Left alone when it already is
/// one; otherwise a fresh link is renamed over it (atomic: a spawn racing
/// this one execs the old file or the new one, never nothing).
#[cfg(unix)]
pub fn ensure_link(dir: &Path, name: &str, target: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad link name"));
    }
    std::fs::create_dir_all(dir)?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let link = dir.join(name);
    let want = std::fs::metadata(target)?;
    if let Ok(have) = std::fs::symlink_metadata(&link) {
        if have.is_file() && have.dev() == want.dev() && have.ino() == want.ino() {
            return Ok(link);
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".{name}.{}.{nanos}.tmp", std::process::id()));
    std::fs::hard_link(target, &tmp)?;
    if let Err(e) = std::fs::rename(&tmp, &link) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(link)
}

#[cfg(not(unix))]
pub fn ensure_link(_dir: &Path, _name: &str, _target: &Path) -> io::Result<PathBuf> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "unix only"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// A fake native program: an ELF magic plus padding, executable.
    fn fake_native(path: &Path, tag: &str) {
        let mut bytes = vec![0x7f, b'E', b'L', b'F'];
        bytes.extend_from_slice(tag.as_bytes());
        std::fs::write(path, bytes).expect("write fake program");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    /// A temp dir removed on drop.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let p = crate::test_env::unique_temp_path(tag);
            std::fs::create_dir_all(&p).expect("scratch dir");
            Scratch(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The native installer's layout: `versions/<v>` + `bin/claude -> versions/<v>`.
    struct Install {
        root: Scratch,
    }
    impl Install {
        fn new(version: &str) -> Self {
            let root = Scratch::new("agent-pname");
            std::fs::create_dir_all(root.path().join("versions")).expect("versions");
            std::fs::create_dir_all(root.path().join("bin")).expect("bin");
            let i = Install { root };
            i.update_to(version);
            i
        }
        fn version_path(&self, v: &str) -> PathBuf {
            self.root.path().join("versions").join(v)
        }
        /// What an update does: a new version file, the symlink re-pointed.
        fn update_to(&self, v: &str) {
            fake_native(&self.version_path(v), v);
            let bin = self.root.path().join("bin").join("claude");
            let _ = std::fs::remove_file(&bin);
            std::os::unix::fs::symlink(self.version_path(v), &bin).expect("symlink");
        }
        fn path(&self) -> String {
            format!("/nonexistent-k2-dir:{}", self.root.path().join("bin").display())
        }
        fn links(&self) -> PathBuf {
            self.root.path().join("links")
        }
    }

    fn ino(p: &Path) -> (u64, u64) {
        let m = std::fs::metadata(p).expect("metadata");
        (m.dev(), m.ino())
    }

    #[test]
    fn names_are_the_agent_cut_to_sixteen_bytes() {
        assert_eq!(process_name("cortana", None).as_deref(), Some("cortana"));
        assert_eq!(process_name("k2-design", Some("review")).as_deref(), Some("k2-design.review"));
        assert_eq!(process_name("K2 Onboarding", None).as_deref(), Some("K2-Onboarding"));
        assert_eq!(process_name("k2-onboarding", Some("review")).as_deref(), Some("k2-onboarding.re"));
        assert_eq!(process_name("a/b\\c:d", None).as_deref(), Some("a-b-c-d"));
        assert_eq!(process_name("..hidden", None).as_deref(), Some("hidden"));
        assert_eq!(process_name("ünïcode agent", None).as_deref(), Some("n-code-agent"));
        assert_eq!(process_name("abcdefghijklmno-x", None).as_deref(), Some("abcdefghijklmno"), "no trailing dash after the cut");
        assert_eq!(process_name("   ", None), None);
        assert_eq!(process_name("***", None), None);
        assert_eq!(process_name("cortana", Some("  ")).as_deref(), Some("cortana"));
        for n in ["cortana", "k2-design.review", "k2-onboarding.re"] {
            assert!(n.len() <= NAME_MAX);
        }
    }

    #[test]
    fn the_name_comes_from_the_spawn_identity_env() {
        assert_eq!(name_from_env(&env(&[("K2_PRIMARY", "cortana"), ("K2_CELL", "canonical")])).as_deref(), Some("cortana"));
        assert_eq!(
            name_from_env(&env(&[("K2_PRIMARY", "cortana"), ("K2_CELL", "sidecar"), ("K2_SIDECAR_NAME", "mail")])).as_deref(),
            Some("cortana.mail")
        );
        // A stray sidecar name on a canonical cell is ignored.
        assert_eq!(
            name_from_env(&env(&[("K2_PRIMARY", "cortana"), ("K2_CELL", "canonical"), ("K2_SIDECAR_NAME", "mail")])).as_deref(),
            Some("cortana")
        );
        assert_eq!(name_from_env(&env(&[("K2_AGENT_NAME", "scout")])).as_deref(), Some("scout"));
        assert_eq!(name_from_env(&env(&[])), None);
    }

    #[test]
    fn a_spawn_links_the_resolved_version_under_the_agent_name() {
        let i = Install::new("2.1.322");
        let e = env(&[("K2_PRIMARY", "cortana"), ("K2_CELL", "canonical")]);
        let out = plan("claude", &e, &i.path(), &i.links());
        let Outcome::Linked(link) = out else { panic!("expected a link, got {out:?}") };
        assert_eq!(link, i.links().join("cortana"));
        assert!(!std::fs::symlink_metadata(&link).expect("link").file_type().is_symlink(), "a hard link, not a symlink");
        assert_eq!(ino(&link), ino(&i.version_path("2.1.322")), "the same file as the installed version");
        let mode = std::fs::metadata(i.links()).expect("dir").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn the_link_follows_an_update_on_the_next_spawn() {
        let i = Install::new("2.1.322");
        let e = env(&[("K2_PRIMARY", "cortana")]);
        let Outcome::Linked(link) = plan("claude", &e, &i.path(), &i.links()) else { panic!("first spawn") };
        assert_eq!(ino(&link), ino(&i.version_path("2.1.322")));

        i.update_to("2.1.325");
        let Outcome::Linked(again) = plan("claude", &e, &i.path(), &i.links()) else { panic!("second spawn") };
        assert_eq!(again, link, "same name");
        assert_eq!(ino(&again), ino(&i.version_path("2.1.325")), "re-pointed at the new version");

        // The installer deleting the old version doesn't touch the link.
        std::fs::remove_file(i.version_path("2.1.322")).expect("cleanup old version");
        assert_eq!(ino(&again), ino(&i.version_path("2.1.325")));
        let left: Vec<_> = std::fs::read_dir(i.links()).expect("links").map(|e| e.expect("entry").file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("cortana")], "no temp files left behind");
    }

    #[test]
    fn a_link_already_right_is_left_alone() {
        let i = Install::new("2.1.322");
        let e = env(&[("K2_PRIMARY", "cortana")]);
        let Outcome::Linked(link) = plan("claude", &e, &i.path(), &i.links()) else { panic!("first") };
        let stamp = |p: &Path| {
            let m = std::fs::symlink_metadata(p).expect("link");
            (m.ctime(), m.ctime_nsec())
        };
        let before = stamp(&link);
        let Outcome::Linked(_) = plan("claude", &e, &i.path(), &i.links()) else { panic!("second") };
        let after = stamp(&link);
        assert_eq!(before, after, "no relink when it already points at the current version");
    }

    #[test]
    fn sidecars_get_their_own_link() {
        let i = Install::new("2.1.322");
        let a = plan("claude", &env(&[("K2_PRIMARY", "cortana")]), &i.path(), &i.links());
        let b = plan(
            "claude",
            &env(&[("K2_PRIMARY", "cortana"), ("K2_CELL", "sidecar"), ("K2_SIDECAR_NAME", "mail")]),
            &i.path(),
            &i.links(),
        );
        assert_eq!(a, Outcome::Linked(i.links().join("cortana")));
        assert_eq!(b, Outcome::Linked(i.links().join("cortana.mail")));
    }

    #[test]
    fn everything_else_keeps_the_program_unchanged() {
        let i = Install::new("2.1.322");
        let agent = env(&[("K2_PRIMARY", "cortana")]);
        assert_eq!(plan("codex", &agent, &i.path(), &i.links()), Outcome::NotVerified);
        assert_eq!(plan("/bin/zsh", &agent, &i.path(), &i.links()), Outcome::NotVerified);
        assert_eq!(plan("claude", &env(&[]), &i.path(), &i.links()), Outcome::NoAgentName);
        assert_eq!(plan("claude", &agent, "/nonexistent-k2-dir", &i.links()), Outcome::NotFound);

        // A node-style script named claude (an npm install).
        let npm = Scratch::new("agent-pname-npm");
        let script = npm.path().join("claude");
        std::fs::write(&script, "#!/usr/bin/env node\n").expect("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let out = plan("claude", &agent, &npm.path().to_string_lossy(), &i.links());
        assert!(matches!(out, Outcome::NotNative(_)), "{out:?}");

        // An app-bundle build.
        let app = Scratch::new("agent-pname-app");
        let macos = app.path().join("claude.app").join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).expect("bundle");
        fake_native(&macos.join("claude"), "bundle");
        let out = plan("claude", &agent, &macos.to_string_lossy(), &i.links());
        assert!(matches!(out, Outcome::InAppBundle(_)), "{out:?}");

        assert!(!i.links().exists(), "no link folder for a spawn that wasn't linked");
    }

    #[test]
    fn a_link_that_cannot_be_made_is_reported_not_fatal() {
        let i = Install::new("2.1.322");
        // The link folder's place is taken by a file.
        std::fs::write(i.links(), "not a folder").expect("block");
        let out = plan("claude", &env(&[("K2_PRIMARY", "cortana")]), &i.path(), &i.links());
        assert!(matches!(out, Outcome::LinkFailed(_)), "{out:?}");
    }

    #[test]
    fn the_switch_is_off_by_default_and_off_changes_nothing() {
        assert!(!enabled(), "off unless the owner turns it on");
        let (prog, out) = apply("claude", &env(&[("K2_PRIMARY", "cortana")]), "/usr/bin:/bin");
        assert_eq!(prog, None);
        assert_eq!(out, Outcome::Off);
    }

    /// The point of it all, end to end on Linux: a real program run from the
    /// link is named after the agent (`/proc/<pid>/comm`), while the
    /// installer's own symlink names it after the version file.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_linked_program_is_named_after_the_agent() {
        let i = Install::new("2.1.322");
        // A real native program in the version file's place.
        std::fs::copy("/bin/sleep", i.version_path("2.1.322")).expect("copy sleep");
        let e = env(&[("K2_PRIMARY", "cortana")]);
        let Outcome::Linked(link) = plan("claude", &e, &i.path(), &i.links()) else { panic!("link") };
        let comm_of = |prog: &Path| {
            // std's spawn returns once the exec has happened (or failed).
            let mut child = crate::bg_proc::command(prog).arg("30").spawn().expect("spawn");
            let comm = std::fs::read_to_string(format!("/proc/{}/comm", child.id())).expect("comm");
            child.kill().expect("kill");
            let _ = child.wait();
            comm.trim().to_string()
        };
        assert_eq!(comm_of(&link), "cortana");
        assert_eq!(comm_of(&i.version_path("2.1.322")), "2.1.322");
    }

    #[test]
    fn native_magic_is_recognised() {
        let d = Scratch::new("agent-pname-magic");
        let cases: &[(&[u8], bool)] = &[
            (&[0xcf, 0xfa, 0xed, 0xfe, 0], true),
            (&[0xca, 0xfe, 0xba, 0xbe, 0], true),
            (&[0x7f, b'E', b'L', b'F', 0], true),
            (b"#!/bin/sh\n", false),
            (b"ab", false),
        ];
        for (n, (bytes, want)) in cases.iter().enumerate() {
            let p = d.path().join(format!("f{n}"));
            std::fs::write(&p, bytes).expect("write");
            assert_eq!(is_native_program(&p), *want, "case {n}");
        }
    }
}
