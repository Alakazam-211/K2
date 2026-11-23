//! The owner-set hire root (T2: one root per box). Every workspace an API
//! key creates lives under it; a key may be pinned to a sub-folder of it.
//! The owner token and Owner/Admin logins may still name any path on the
//! converge doors (`/v1/w`, `k2 agent hire <dir>`): that is their own box.

use std::path::{Component, Path, PathBuf};
use std::sync::RwLock;

use super::HireError;

/// Folder name under `$HOME` when the owner never set a root.
pub const DEFAULT_ROOT_DIR: &str = "k2-agents";

/// The stored root, or `None` when unset.
pub fn stored_root(conn: &rusqlite::Connection) -> Option<String> {
    conn.query_row("SELECT root FROM hire_settings WHERE id = 1", [], |r| {
        r.get::<_, Option<String>>(0)
    })
    .ok()
    .flatten()
    .filter(|s| !s.trim().is_empty())
}

/// The effective root: the stored one, else `~/k2-agents`.
pub fn hire_root() -> Result<PathBuf, HireError> {
    let stored = {
        let db = crate::db::shared();
        let conn = db.lock();
        stored_root(&conn)
    };
    match stored {
        Some(s) => Ok(PathBuf::from(s)),
        None => crate::paths::home_dir()
            .map(|h| h.join(DEFAULT_ROOT_DIR))
            .ok_or_else(|| HireError::internal("no home folder to put the hire root in; set one: k2 agent hire-root set <dir>")),
    }
}

/// Owner: set the root. Absolute, not `/`, no `..`. Stored as given
/// (canonicalized when it exists already).
pub fn set_hire_root(raw: &str) -> Result<PathBuf, HireError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(HireError::usage("missing root folder"));
    }
    let expanded = if let Some(rest) = t.strip_prefix("~/") {
        crate::paths::home_dir()
            .ok_or_else(|| HireError::usage("cannot resolve ~"))?
            .join(rest)
    } else {
        PathBuf::from(t)
    };
    if !expanded.is_absolute() {
        return Err(HireError::usage("the hire root must be an absolute path (or ~/…)"));
    }
    if expanded.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(HireError::usage("the hire root may not contain '..'"));
    }
    if expanded.parent().is_none() {
        return Err(HireError::usage("the hire root can't be /"));
    }
    if expanded.exists() && !expanded.is_dir() {
        return Err(HireError::usage(format!("{} exists and is not a folder", expanded.display())));
    }
    let stored = dunce::canonicalize(&expanded).unwrap_or(expanded);
    check_root_safe(&stored)?;
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO hire_settings (id, root, updated_at) VALUES (1, ?1, unixepoch()) \
         ON CONFLICT(id) DO UPDATE SET root = excluded.root, updated_at = excluded.updated_at",
        rusqlite::params![stored.to_string_lossy()],
    )
    .map_err(|e| HireError::internal(format!("save hire root: {e}")))?;
    Ok(stored)
}

/// A key's sub-folder limit: one or more plain segments (`acme`, `acme/jobs`).
pub fn validate_sub_folder(raw: &str) -> Result<String, HireError> {
    let t = raw.trim().trim_matches('/');
    if t.is_empty() {
        return Ok(String::new());
    }
    for seg in t.split('/') {
        let ok = !seg.is_empty()
            && seg != "."
            && seg != ".."
            && seg.len() <= 64
            && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
            && !seg.starts_with('.');
        if !ok {
            return Err(HireError::usage(format!(
                "folder '{raw}': use plain names (letters, digits, - _ .), separated by /"
            )));
        }
    }
    Ok(t.to_string())
}

/// The folder new members go into: root, plus the key's sub-folder.
pub fn base_dir(sub_folder: Option<&str>) -> Result<PathBuf, HireError> {
    let root = hire_root()?;
    match sub_folder.map(str::trim).filter(|s| !s.is_empty()) {
        Some(sub) => Ok(root.join(validate_sub_folder(sub)?)),
        None => Ok(root),
    }
}

/// Can a user other than the server's change a folder with this owner and
/// mode? Only the server's user (and root, when `root_ok`) may own it, and
/// no group or other write bit may be set, sticky or not: the sticky bit
/// stops another user deleting a name, not making the one a hire is about
/// to use.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn changeable_by_others(uid: u32, mode: u32, me: u32, root_ok: bool) -> bool {
    let owned = uid == me || (root_ok && uid == 0);
    !owned || mode & 0o022 != 0
}

/// The server's user (the effective uid).
#[cfg(unix)]
pub(crate) fn server_uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

static OWNER_BASE_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Tests: one more folder that counts as the owner's base for
/// [`unsafe_folder_on_the_way`] (a folder the test made, only its owner can
/// write). The daemon never sets it.
pub fn set_owner_base_override(dir: Option<PathBuf>) {
    *OWNER_BASE_OVERRIDE.write().unwrap_or_else(|p| p.into_inner()) = dir;
}

/// Where the walk down to a hire root starts: the deepest owner's base the
/// root is inside (the home folder), else `/`.
#[cfg_attr(not(unix), allow(dead_code))]
fn walk_start(real: &Path) -> PathBuf {
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Some(h) = crate::paths::home_dir() {
        bases.push(resolve_existing_prefix(&h));
    }
    if let Some(o) = OWNER_BASE_OVERRIDE.read().unwrap_or_else(|p| p.into_inner()).clone() {
        bases.push(resolve_existing_prefix(&o));
    }
    bases
        .into_iter()
        .filter(|b| real.starts_with(b))
        .max_by_key(|b| b.components().count())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The first folder from the owner's base (or `/`) down to `dir` that
/// another user could change: not owned by the server's user or root, or
/// writable by a group or everyone (see [`changeable_by_others`]). Only the
/// folders that exist are looked at.
#[cfg(unix)]
pub fn unsafe_folder_on_the_way(dir: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let real = resolve_existing_prefix(dir);
    let start = walk_start(&real);
    let me = server_uid();
    let mut cur = PathBuf::new();
    for part in real.components() {
        cur.push(part);
        if !cur.starts_with(&start) {
            continue;
        }
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() || changeable_by_others(m.uid(), m.mode(), me, true) => return Some(cur),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

#[cfg(not(unix))]
pub fn unsafe_folder_on_the_way(_dir: &Path) -> Option<PathBuf> {
    None
}

/// Make a hire root (or a key's folder in it) and the folders above it when
/// it isn't there yet, before a hire walks down from it. Each folder made
/// here is the server user's with mode 0700, set after it is made (never
/// left to the umask). A folder that is there already keeps its mode; the
/// owner-and-mode check reports one another user could change.
pub fn ensure_root(root: &Path) -> Result<(), HireError> {
    if root.is_dir() {
        return Ok(());
    }
    let mut missing = Vec::new();
    let mut cur = root.to_path_buf();
    while std::fs::symlink_metadata(&cur).is_err() {
        missing.push(cur.clone());
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => break,
        }
    }
    for dir in missing.iter().rev() {
        make_private_dir(dir)?;
    }
    Ok(())
}

#[cfg(unix)]
fn make_private_dir(dir: &Path) -> Result<(), HireError> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::DirBuilderExt;
    let fail = |e: std::io::Error| HireError::usage(format!("make the hire root {}: {e}", dir.display()));
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        // Made by someone else since: left as it is (the check reports it).
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(e) => return Err(fail(e)),
    }
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).map_err(|_| HireError::usage("a hire root path can't hold a NUL"))?;
    // SAFETY: a valid C string; the fd is owned below.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(fail(std::io::Error::last_os_error()));
    }
    // SAFETY: open returned a new fd that nothing else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: a valid fd; stat is plain data.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fchmod(fd.as_raw_fd(), 0o700) } != 0 || unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(fail(std::io::Error::last_os_error()));
    }
    if st.st_uid != server_uid() {
        return Err(unsafe_folder_error(dir));
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_private_dir(dir: &Path) -> Result<(), HireError> {
    match std::fs::create_dir(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(HireError::usage(format!("make the hire root {}: {e}", dir.display()))),
    }
}

/// `hire_root_unsafe` when another user could change a folder on the way
/// to `dir` (a hire root, or a key's folder in it).
pub fn check_root_safe(dir: &Path) -> Result<(), HireError> {
    match unsafe_folder_on_the_way(dir) {
        Some(d) => Err(unsafe_folder_error(&d)),
        None => Ok(()),
    }
}

/// The hire root's doctor notes, one line each: a folder another user
/// could change on the way to the root, each workspace already under it
/// with the folder on its way that another user could change (with
/// `workspaces`), and, for information, a root other users can read.
pub fn doctor_notes(workspaces: bool) -> Vec<String> {
    let Ok(root) = hire_root() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(d) = unsafe_folder_on_the_way(&root) {
        out.push(format!("hire root {}: {}", root.display(), unsafe_folder_error(&d).hint));
    } else if let Some(mode) = readable_by_others(&root) {
        out.push(format!("hire root {}: other users can read it (mode {mode:o}); 0700 is tighter (chmod 700)", root.display()));
    }
    if workspaces {
        let rows: Vec<(String, String)> = {
            let db = crate::db::shared();
            let conn = db.lock();
            let Ok(mut stmt) = conn.prepare("SELECT name, path FROM projects ORDER BY path") else {
                return out;
            };
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map(|it| it.flatten().collect())
                .unwrap_or_default();
            rows
        };
        for (name, path) in rows {
            if !is_under(Path::new(&path), &root) {
                continue;
            }
            if let Some(d) = unsafe_folder_on_the_way(Path::new(&path)) {
                out.push(format!("workspace {name} ({path}) is under the hire root: {}", unsafe_folder_error(&d).hint));
            }
        }
    }
    out
}

#[cfg(unix)]
fn readable_by_others(dir: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(dir).ok()?.permissions().mode() & 0o7777;
    (mode & 0o077 != 0).then_some(mode)
}

#[cfg(not(unix))]
fn readable_by_others(_dir: &Path) -> Option<u32> {
    None
}

/// `hire_root_unsafe`: `dir` can be changed by another user.
pub fn unsafe_folder_error(dir: &Path) -> HireError {
    HireError::new(
        403,
        "hire_root_unsafe",
        format!("{} can be changed by another user: make it yours and not group or world writable (chmod go-w)", dir.display()),
    )
}

/// Is `path` (absolute, normalized or not yet existing) strictly inside
/// `base`? Lexical check after resolving the deepest existing ancestor, so
/// a symlink inside the root can't point the new folder outside it.
pub fn is_under(path: &Path, base: &Path) -> bool {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    // Both sides the same way: a root that doesn't exist yet still resolves
    // its existing ancestors (on macOS /tmp and /var are symlinks).
    let base_c = resolve_existing_prefix(base);
    let resolved = resolve_existing_prefix(path);
    resolved != base_c && resolved.starts_with(&base_c)
}

/// Canonicalize the deepest existing ancestor and re-append the rest.
pub(crate) fn resolve_existing_prefix(path: &Path) -> PathBuf {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = path.to_path_buf();
    loop {
        if let Ok(c) = dunce::canonicalize(&cur) {
            let mut out = c;
            for t in tail.iter().rev() {
                out.push(t);
            }
            return out;
        }
        match (cur.file_name().map(|s| s.to_os_string()), cur.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                cur = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The folder rule on its own (no chown as a plain user): another
    /// owner, a group or other write bit (sticky too) is refused; root may
    /// own a folder above the hire's own.
    #[test]
    fn a_folder_another_user_can_change() {
        let me = 501;
        assert!(!changeable_by_others(me, 0o40700, me, false), "mine, 0700");
        assert!(!changeable_by_others(me, 0o40755, me, false), "mine, 0755");
        assert!(changeable_by_others(502, 0o40700, me, true), "another user's");
        assert!(changeable_by_others(0, 0o40755, me, false), "root's, where only mine will do");
        assert!(!changeable_by_others(0, 0o40755, me, true), "root's, above the folder");
        assert!(changeable_by_others(me, 0o40775, me, false), "group writable");
        assert!(changeable_by_others(me, 0o40757, me, false), "world writable");
        assert!(changeable_by_others(0, 0o41777, me, true), "sticky and world writable (/tmp)");
    }

    /// The umask, put back on drop.
    #[cfg(unix)]
    struct Umask(libc::mode_t);

    #[cfg(unix)]
    impl Drop for Umask {
        fn drop(&mut self) {
            // SAFETY: umask has no preconditions.
            unsafe { libc::umask(self.0) };
        }
    }

    /// With a umask that leaves group write on, every folder the daemon
    /// makes for a hire root is still the server user's with mode 0700, and
    /// the root passes the owner-and-mode check; a root that is there
    /// already keeps its mode.
    #[cfg(unix)]
    #[test]
    fn a_hire_root_the_daemon_makes_is_0700_whatever_the_umask() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let home = crate::test_env::TempHome::new();
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700)).expect("chmod home");
        let top = home.path().join("made");
        let root = top.join("by").join("k2-agents");
        {
            // SAFETY: umask has no preconditions.
            let _umask = Umask(unsafe { libc::umask(0o002) });
            ensure_root(&root).expect("make the root");
        }
        for d in [&top, &top.join("by"), &root] {
            let m = std::fs::symlink_metadata(d).expect("stat");
            assert_eq!(m.permissions().mode() & 0o7777, 0o700, "{}", d.display());
            assert_eq!(m.uid(), server_uid(), "{}", d.display());
        }
        assert!(check_root_safe(&root).is_ok());
        let kept = home.path().join("kept");
        std::fs::create_dir(&kept).expect("mkdir");
        std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        ensure_root(&kept).expect("there already");
        assert_eq!(std::fs::metadata(&kept).expect("stat").permissions().mode() & 0o7777, 0o755, "an existing root keeps its mode");
    }

    #[test]
    fn sub_folder_rules() {
        assert_eq!(validate_sub_folder("acme").unwrap(), "acme");
        assert_eq!(validate_sub_folder("/acme/jobs/").unwrap(), "acme/jobs");
        assert_eq!(validate_sub_folder("").unwrap(), "");
        for bad in ["..", "a/../b", ".hidden", "a b", "a//b", "x/./y"] {
            assert!(validate_sub_folder(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn is_under_refuses_escape_and_the_root_itself() {
        let base = std::env::temp_dir().join(format!("k2-hire-root-under-{}", std::process::id()));
        std::fs::create_dir_all(&base).expect("mkdir base");
        assert!(is_under(&base.join("a"), &base));
        assert!(is_under(&base.join("a/b"), &base));
        assert!(!is_under(&base, &base), "the root itself is not a workspace folder");
        assert!(!is_under(&base.join("../x"), &base));
        assert!(!is_under(Path::new("/etc/x"), &base));
        #[cfg(unix)]
        {
            let link = base.join("out");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink("/tmp", &link).expect("symlink");
            assert!(!is_under(&link.join("x"), &base), "a symlink may not lead out of the root");
            let _ = std::fs::remove_file(&link);
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A hire root that doesn't exist yet, under a folder reached through a
    /// symlink (macOS's /tmp and /var are symlinks to /private/...): a new
    /// workspace inside it is inside, whichever spelling each side uses.
    #[cfg(unix)]
    #[test]
    fn is_under_a_root_not_made_yet_under_a_symlinked_folder() {
        let top = std::env::temp_dir().join(format!("k2-hire-root-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        let real = top.join("real");
        std::fs::create_dir_all(&real).expect("mkdir real");
        let link = top.join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        for base in [link.join("k2-agents"), real.join("k2-agents")] {
            assert!(!base.exists(), "the root isn't made yet");
            for path in [link.join("k2-agents/hire-b"), real.join("k2-agents/hire-b"), link.join("k2-agents/team/hire-c")] {
                assert!(is_under(&path, &base), "{} is inside {}", path.display(), base.display());
            }
            assert!(!is_under(&base, &base), "the root itself is not a workspace folder");
            assert!(!is_under(&real.join("other/hire-b"), &base), "a sibling folder is outside");
            assert!(!is_under(&link.join("k2-agents/../other"), &base));
        }
        let _ = std::fs::remove_dir_all(&top);
    }
}
