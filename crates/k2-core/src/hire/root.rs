//! The owner-set hire root (T2: one root per box). Every workspace an API
//! key creates lives under it; a key may be pinned to a sub-folder of it.
//! The owner token and Owner/Admin logins may still name any path on the
//! converge doors (`/v1/w`, `k2 agent hire <dir>`): that is their own box.

use std::path::{Component, Path, PathBuf};

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

/// Is `path` (absolute, normalized or not yet existing) strictly inside
/// `base`? Lexical check after resolving the deepest existing ancestor, so
/// a symlink inside the root can't point the new folder outside it.
pub fn is_under(path: &Path, base: &Path) -> bool {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    let base_c = dunce::canonicalize(&base).unwrap_or_else(|_| base.to_path_buf());
    let resolved = resolve_existing_prefix(path);
    resolved != base_c && resolved.starts_with(&base_c)
}

/// Canonicalize the deepest existing ancestor and re-append the rest.
fn resolve_existing_prefix(path: &Path) -> PathBuf {
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
}
