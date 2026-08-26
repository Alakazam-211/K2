//! Dump/restore jail — copy of mail `resolve_out_path` / `resolve_in_path`.
//! Workspace-rooted; reject `..` / abs / symlink escape. Not `/cli/fs`.

use std::path::{Component, Path, PathBuf};

pub fn resolve_out_path(ws_root: &str, out: &str) -> Result<PathBuf, String> {
    let out = out.trim();
    if out.is_empty() {
        return Err("empty 'out' path".to_string());
    }
    let rel = Path::new(out);
    if rel.is_absolute() {
        return Err(format!(
            "'out' must be a path inside the workspace (relative), got absolute: {out}"
        ));
    }
    if rel.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        return Err(format!("'out' must not contain '..' components: {out}"));
    }
    let root = Path::new(ws_root)
        .canonicalize()
        .map_err(|e| format!("workspace root unavailable: {e}"))?;
    let target = root.join(rel);
    let parent = target
        .parent()
        .ok_or_else(|| "'out' has no parent directory".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| format!("create '{out}' parent: {e}"))?;
    let parent = parent
        .canonicalize()
        .map_err(|e| format!("resolve '{out}' parent: {e}"))?;
    if !parent.starts_with(&root) {
        return Err(format!("'out' escapes the workspace: {out}"));
    }
    let file_name = target
        .file_name()
        .ok_or_else(|| format!("'out' has no file name: {out}"))?;
    Ok(parent.join(file_name))
}

#[derive(Debug)]
pub enum InPathError {
    Usage(String),
    NotFound(String),
}

pub fn resolve_in_path(ws_root: &str, path: &str) -> Result<PathBuf, InPathError> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err(InPathError::Usage("empty dump path".to_string()));
    }
    let rel = Path::new(raw);
    if rel.is_absolute() {
        return Err(InPathError::Usage(format!(
            "dump path must be inside the workspace (relative), got absolute: {raw}"
        )));
    }
    if rel.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        return Err(InPathError::Usage(format!(
            "dump path must not contain '..' components: {raw}"
        )));
    }
    let root = Path::new(ws_root)
        .canonicalize()
        .map_err(|e| InPathError::Usage(format!("workspace root unavailable: {e}")))?;
    let target = root.join(rel);
    let canon = match target.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return Err(InPathError::NotFound(format!(
                "dump file not found: {raw}"
            )))
        }
    };
    if !canon.starts_with(&root) {
        return Err(InPathError::Usage(format!(
            "dump path escapes the workspace: {raw}"
        )));
    }
    if !canon.is_file() {
        return Err(InPathError::NotFound(format!(
            "dump file not found: {raw}"
        )));
    }
    Ok(canon)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!(
            "k2-sql-jail-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let s = dir.canonicalize().unwrap().to_string_lossy().into_owned();
        (dir, s)
    }

    #[test]
    fn dump_jail_rejects_dotdot_and_abs() {
        let (keep, root_s) = temp_root();
        let p = resolve_out_path(&root_s, ".k2/db/dumps/x.dump").expect("plain");
        assert!(p.starts_with(&root_s));
        assert!(resolve_out_path(&root_s, "/etc/passwd").is_err());
        assert!(resolve_out_path(&root_s, "../outside.dump").is_err());
        assert!(resolve_out_path(&root_s, "a/../../outside.dump").is_err());
        assert!(matches!(
            resolve_in_path(&root_s, "../outside.dump"),
            Err(InPathError::Usage(_))
        ));
        assert!(matches!(
            resolve_in_path(&root_s, "/etc/passwd"),
            Err(InPathError::Usage(_))
        ));
        let _ = std::fs::remove_dir_all(keep);
    }
}
