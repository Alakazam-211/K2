//! Owner-only permissions for the K2 home (`~/.k2`).
//!
//! The K2 home holds the daemon DB (LLM API keys in plain columns, chat
//! history, grants), `skin.db`, the thread-secrets vault, tokens and logs.
//! Before 0.45.1 the directory was created under the normal umask (0755)
//! and SQLite created `k2so.db` / `-wal` / `-shm` as 0644, so any other
//! OS user on the machine could read them.
//!
//! Two pieces:
//! - [`tighten_home_at`] runs once at daemon boot: the home dir (and a
//!   real legacy `~/.k2so` dir, if one still exists) goes to 0700, every
//!   SQLite / redb database plus its `-wal` / `-shm` / `-journal` goes to
//!   0600, and the dirs that hold secrets go to 0700.
//! - [`prepare_private_db_file`] + [`restrict_db_files`] wrap every open of
//!   a K2-home database, so a NEW database is created 0600 before SQLite
//!   touches it. SQLite copies the main file's mode onto the `-wal`,
//!   `-shm` and `-journal` files it creates later, so they come out 0600
//!   too (covered by a test in `db`).
//!
//! Deliberately NOT a process-wide `umask(077)`: the umask is inherited by
//! every child the daemon spawns (agent PTYs, published services, Caddy,
//! frpc) and applies to workspace files the daemon writes, which must keep
//! their normal modes. Only paths inside the K2 home are touched here.
//!
//! The change only ever REMOVES group/other bits; owner bits are kept, so
//! an executable stays executable for its owner. Unix only; no-op elsewhere.

use std::path::{Path, PathBuf};

/// Database file extensions the boot sweep treats as private.
const DB_EXTS: &[&str] = &["db", "sqlite", "sqlite3", "redb"];

/// SQLite side files that hold database pages (or the index to them).
pub const DB_SIDECARS: &[&str] = &["-wal", "-shm", "-journal"];

/// Directories directly under the K2 home that hold secrets or private
/// message bodies. Forced to 0700 (owner bits kept) at boot.
pub const SECRET_DIRS: &[&str] = &[
    "thread-secrets",
    "llm-accounts",
    "usage",
    "certs",
    "run",
    "mail-outbound",
    "federation-outbox",
    "wiki-public-chat",
    "skin",
];

/// Subtrees the DB sweep never walks. The sandbox roots hold dirs chowned
/// to per-session cell uids (not ours to chmod); `clone-tmp` holds
/// workspace bundles being unpacked, whose modes must survive the copy.
const SKIP_WALK: &[&str] = &[
    "sandbox-sessions",
    "sandbox-overlays",
    "sandbox-homes",
    "clone-tmp",
];

/// How deep the DB sweep walks below the home dir (`~/.k2/a/b/file.db`).
const MAX_DEPTH: usize = 3;

/// What the boot sweep changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TightenReport {
    /// `(path, old_mode, new_mode)` for every path whose mode changed.
    pub tightened: Vec<(PathBuf, u32, u32)>,
    /// `(path, error)` for every chmod that failed.
    pub failed: Vec<(PathBuf, String)>,
}

impl TightenReport {
    /// One log line, or `None` when nothing changed and nothing failed.
    pub fn log_line(&self, root: &Path) -> Option<String> {
        if self.tightened.is_empty() && self.failed.is_empty() {
            return None;
        }
        let show = |p: &Path| -> String {
            p.strip_prefix(root)
                .ok()
                .filter(|r| !r.as_os_str().is_empty())
                .map(|r| r.display().to_string())
                .unwrap_or_else(|| p.display().to_string())
        };
        let mut parts: Vec<String> = self
            .tightened
            .iter()
            .map(|(p, old, new)| format!("{} {:o}->{:o}", show(p), old, new))
            .collect();
        parts.extend(
            self.failed
                .iter()
                .map(|(p, e)| format!("{} FAILED ({e})", show(p))),
        );
        Some(format!(
            "[daemon/boot] owner-only permissions in {}: tightened {}, failed {}: {}",
            root.display(),
            self.tightened.len(),
            self.failed.len(),
            parts.join(", ")
        ))
    }
}

/// True for `k2so.db`, `k2so.db-wal`, `tokens.sqlite-shm`, `k2-threads.redb`, …
pub fn is_db_file_name(name: &str) -> bool {
    let base = DB_SIDECARS
        .iter()
        .find_map(|s| name.strip_suffix(s))
        .unwrap_or(name);
    match base.rsplit_once('.') {
        Some((stem, ext)) => !stem.is_empty() && DB_EXTS.contains(&ext),
        None => false,
    }
}

/// `path` with `suffix` appended to its file name (`k2so.db` → `k2so.db-wal`).
pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

/// Boot sweep against the real `$HOME`. See module docs.
pub fn tighten_home() -> TightenReport {
    match dirs::home_dir() {
        Some(home) => tighten_home_at(&home),
        None => TightenReport::default(),
    }
}

/// Testable core of [`tighten_home`]: operates on an explicit `$HOME`.
pub fn tighten_home_at(home: &Path) -> TightenReport {
    let mut report = TightenReport::default();
    #[cfg(unix)]
    {
        let canonical = home.join(".k2");
        // `~/.k2` itself may be a symlink an operator made to another disk;
        // follow it (chmod applies to the target dir, which is ours).
        if canonical.is_dir() {
            sweep_root(&canonical, &mut report);
        }
        // Legacy `~/.k2so`: normally a compat SYMLINK to `~/.k2` (nothing
        // to do — the target was just swept). A REAL dir is a mixed-version
        // leftover that still holds an old k2so.db; tighten it too.
        let legacy = home.join(".k2so");
        let legacy_is_real_dir = std::fs::symlink_metadata(&legacy)
            .map(|m| m.file_type().is_dir())
            .unwrap_or(false);
        if legacy_is_real_dir {
            sweep_root(&legacy, &mut report);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = home;
    }
    report
}

#[cfg(unix)]
fn sweep_root(root: &Path, report: &mut TightenReport) {
    strip_group_other(root, true, report);
    for name in SECRET_DIRS {
        let dir = root.join(name);
        let is_real_dir = std::fs::symlink_metadata(&dir)
            .map(|m| m.file_type().is_dir())
            .unwrap_or(false);
        if is_real_dir {
            strip_group_other(&dir, false, report);
        }
    }
    walk_dbs(root, 0, report);
}

#[cfg(unix)]
fn walk_dbs(dir: &Path, depth: usize, report: &mut TightenReport) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if ft.is_dir() {
            if depth + 1 < MAX_DEPTH && !(depth == 0 && SKIP_WALK.contains(&name.as_ref())) {
                walk_dbs(&entry.path(), depth + 1, report);
            }
        } else if ft.is_file() && is_db_file_name(&name) {
            strip_group_other(&entry.path(), false, report);
        }
    }
}

/// Remove group/other bits from `path` (never follows a symlink, except
/// for the root itself when `follow` is set). Records the change or the
/// failure in `report`; a path already owner-only is left alone.
#[cfg(unix)]
fn strip_group_other(path: &Path, follow: bool, report: &mut TightenReport) {
    use std::os::unix::fs::PermissionsExt;
    let meta = if follow {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    };
    let Ok(meta) = meta else { return };
    if meta.file_type().is_symlink() {
        return;
    }
    let old = meta.permissions().mode() & 0o7777;
    if old & 0o077 == 0 {
        return;
    }
    // Keep owner bits (and setgid/sticky on a dir would be odd here; drop
    // them with the group/other bits — none of K2's dirs use them).
    let new = old & 0o700;
    match std::fs::set_permissions(path, std::fs::Permissions::from_mode(new)) {
        Ok(()) => report.tightened.push((path.to_path_buf(), old, new)),
        Err(e) => report.failed.push((path.to_path_buf(), e.to_string())),
    }
}

/// Create `path` as an empty 0600 file when it does not exist yet, so the
/// SQLite open that follows inherits 0600 for the database and (through
/// SQLite's own mode copy) for its `-wal` / `-shm` / `-journal` files.
/// An existing file is left as is; call [`restrict_db_files`] for that.
/// Only for databases inside the K2 home. Errors are returned for the
/// caller to log; SQLite's own open still works without this step.
pub fn prepare_private_db_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
        {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Strip group/other bits from a database file and every side file that
/// exists next to it. Never follows symlinks. Best-effort; returns the
/// first error for logging.
pub fn restrict_db_files(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        let mut report = TightenReport::default();
        strip_group_other(path, false, &mut report);
        for s in DB_SIDECARS {
            strip_group_other(&sibling(path, s), false, &mut report);
        }
        if let Some((p, e)) = report.failed.first() {
            return Err(format!("chmod {}: {e}", p.display()));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Strip group/other bits from one file or directory (a single-file
/// store such as redb, or the K2 home itself). Never follows a symlink.
/// Best-effort; returns the error for logging.
pub fn restrict_path(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        let mut report = TightenReport::default();
        strip_group_other(path, false, &mut report);
        if let Some((p, e)) = report.failed.first() {
            return Err(format!("chmod {}: {e}", p.display()));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "k2-private-home-{tag}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&p).expect("mkdir temp home");
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn mode(p: &Path) -> u32 {
        std::fs::symlink_metadata(p)
            .unwrap_or_else(|e| panic!("stat {}: {e}", p.display()))
            .permissions()
            .mode()
            & 0o777
    }

    fn set(p: &Path, m: u32) {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(m))
            .unwrap_or_else(|e| panic!("chmod {}: {e}", p.display()));
    }

    fn write(p: &Path, m: u32) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent");
        }
        std::fs::write(p, b"x").unwrap_or_else(|e| panic!("write {}: {e}", p.display()));
        set(p, m);
    }

    #[test]
    fn db_file_names() {
        for yes in [
            "k2so.db",
            "k2so.db-wal",
            "k2so.db-shm",
            "k2.db-journal",
            "skin.db-wal",
            "tokens.sqlite",
            "tokens.sqlite-shm",
            "x.sqlite3",
            "k2-threads.redb",
        ] {
            assert!(is_db_file_name(yes), "{yes} must be a db file");
        }
        for no in [
            "daemon.token",
            "settings.json",
            "frpc.log",
            ".db",
            "db",
            "k2so.db.bak",
            "notes-wal",
            "tunnel-key.pem",
        ] {
            assert!(!is_db_file_name(no), "{no} must not be a db file");
        }
    }

    #[test]
    fn sweep_tightens_open_home_dbs_and_secret_dirs() {
        let t = TempDir::new("sweep");
        let home = &t.0;
        let k2 = home.join(".k2");
        std::fs::create_dir_all(&k2).unwrap();
        set(&k2, 0o755);
        for f in ["k2so.db", "k2so.db-wal", "k2so.db-shm", "k2-threads.redb"] {
            write(&k2.join(f), 0o644);
        }
        // skin.db already private, its WAL/SHM not (the live 0.45.0 shape).
        write(&k2.join("skin.db"), 0o600);
        write(&k2.join("skin.db-wal"), 0o644);
        write(&k2.join("skin.db-shm"), 0o644);
        // A nested DB (usage ledger) and a secret dir left 0755.
        write(&k2.join("usage/tokens.sqlite"), 0o644);
        write(&k2.join("thread-secrets/ws1/name"), 0o600);
        set(&k2.join("thread-secrets"), 0o755);
        set(&k2.join("usage"), 0o755);
        // An executable script keeps its owner exec bit, untouched (not a db).
        write(&k2.join("hooks/notify.sh"), 0o755);
        // Non-db file: left alone by the file sweep (the 0700 dir covers it).
        write(&k2.join("baseline.txt"), 0o644);
        // Skipped subtree: sandbox dirs belong to cell uids.
        write(&k2.join("sandbox-homes/ws/.claude/state.db"), 0o644);

        let report = tighten_home_at(home);
        assert!(report.failed.is_empty(), "no chmod may fail: {:?}", report.failed);

        assert_eq!(mode(&k2), 0o700, "home dir");
        for f in [
            "k2so.db",
            "k2so.db-wal",
            "k2so.db-shm",
            "k2-threads.redb",
            "skin.db",
            "skin.db-wal",
            "skin.db-shm",
            "usage/tokens.sqlite",
        ] {
            assert_eq!(mode(&k2.join(f)), 0o600, "{f}");
        }
        assert_eq!(mode(&k2.join("thread-secrets")), 0o700);
        assert_eq!(mode(&k2.join("usage")), 0o700);
        assert_eq!(mode(&k2.join("hooks/notify.sh")), 0o755, "non-db file untouched");
        assert_eq!(mode(&k2.join("baseline.txt")), 0o644, "non-db file untouched");
        assert_eq!(
            mode(&k2.join("sandbox-homes/ws/.claude/state.db")),
            0o644,
            "sandbox subtree is never walked"
        );

        // skin.db was already 0600: not reported.
        assert!(
            !report.tightened.iter().any(|(p, _, _)| p == &k2.join("skin.db")),
            "an owner-only file is not reported"
        );
        let line = report.log_line(&k2).expect("a log line when something changed");
        assert!(line.contains("k2so.db 644->600"), "{line}");
        assert!(line.contains("tightened"), "{line}");

        // Second run: nothing left to do, no log line.
        let again = tighten_home_at(home);
        assert_eq!(again, TightenReport::default());
        assert!(again.log_line(&k2).is_none());
    }

    #[test]
    fn sweep_leaves_files_outside_the_k2_home_alone() {
        let t = TempDir::new("outside");
        let home = &t.0;
        let k2 = home.join(".k2");
        std::fs::create_dir_all(&k2).unwrap();
        set(&k2, 0o755);
        // A workspace next to the K2 home, with a SQLite DB and a public dir.
        let ws = home.join("projects/site");
        write(&ws.join("app.db"), 0o644);
        write(&ws.join("app.db-wal"), 0o644);
        write(&ws.join("public/index.html"), 0o644);
        set(&ws.join("public"), 0o755);
        set(&ws, 0o755);
        set(home, 0o755);

        tighten_home_at(home);

        assert_eq!(mode(&k2), 0o700);
        assert_eq!(mode(home), 0o755, "$HOME itself untouched");
        assert_eq!(mode(&ws), 0o755, "workspace dir untouched");
        assert_eq!(mode(&ws.join("app.db")), 0o644, "workspace db untouched");
        assert_eq!(mode(&ws.join("app.db-wal")), 0o644, "workspace wal untouched");
        assert_eq!(mode(&ws.join("public")), 0o755, "published root untouched");
        assert_eq!(mode(&ws.join("public/index.html")), 0o644);
    }

    #[test]
    fn sweep_follows_legacy_dir_but_not_legacy_symlink() {
        // Real legacy dir (mixed-version leftover): tightened.
        let t = TempDir::new("legacy-real");
        let home = &t.0;
        std::fs::create_dir_all(home.join(".k2")).unwrap();
        let legacy = home.join(".k2so");
        std::fs::create_dir_all(&legacy).unwrap();
        set(&legacy, 0o755);
        write(&legacy.join("k2so.db"), 0o644);
        tighten_home_at(home);
        assert_eq!(mode(&legacy), 0o700);
        assert_eq!(mode(&legacy.join("k2so.db")), 0o600);

        // Compat symlink: the link itself is skipped; the target (~/.k2) is
        // swept once via its own name.
        let t2 = TempDir::new("legacy-link");
        let home2 = &t2.0;
        let k2 = home2.join(".k2");
        std::fs::create_dir_all(&k2).unwrap();
        set(&k2, 0o755);
        std::os::unix::fs::symlink(&k2, home2.join(".k2so")).unwrap();
        let report = tighten_home_at(home2);
        assert_eq!(mode(&k2), 0o700);
        assert_eq!(
            report.tightened.len(),
            1,
            "the home dir is tightened exactly once: {:?}",
            report.tightened
        );
    }

    #[test]
    fn sweep_does_not_follow_symlinks_out_of_the_home() {
        let t = TempDir::new("symlink-out");
        let home = &t.0;
        let k2 = home.join(".k2");
        std::fs::create_dir_all(&k2).unwrap();
        let outside = home.join("elsewhere");
        write(&outside.join("shared.db"), 0o644);
        set(&outside, 0o755);
        std::os::unix::fs::symlink(outside.join("shared.db"), k2.join("linked.db")).unwrap();
        std::os::unix::fs::symlink(&outside, k2.join("usage")).unwrap();
        tighten_home_at(home);
        assert_eq!(mode(&outside.join("shared.db")), 0o644);
        assert_eq!(mode(&outside), 0o755);
    }

    #[test]
    fn prepare_creates_0600_and_keeps_existing() {
        let t = TempDir::new("prepare");
        let p = t.0.join("new.db");
        prepare_private_db_file(&p).expect("prepare new");
        assert_eq!(mode(&p), 0o600);
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 0);

        let existing = t.0.join("old.db");
        write(&existing, 0o644);
        prepare_private_db_file(&existing).expect("prepare existing");
        assert_eq!(mode(&existing), 0o644, "prepare leaves an existing file as is");
        assert_eq!(std::fs::read(&existing).unwrap(), b"x");

        write(&sibling(&existing, "-wal"), 0o644);
        restrict_db_files(&existing).expect("restrict");
        assert_eq!(mode(&existing), 0o600);
        assert_eq!(mode(&sibling(&existing, "-wal")), 0o600);
    }
}
