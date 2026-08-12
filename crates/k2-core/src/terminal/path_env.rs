//! Portable PATH string helpers.
//!
//! Uses [`std::env::split_paths`] / [`std::env::join_paths`] so the
//! platform path list separator is correct (`:` on Unix, `;` on
//! Windows) without hardcoding either. All helpers are pure (no I/O,
//! no process env mutation) so spawn-path construction is unit-testable.

use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Split a PATH-like string into components using the host separator.
pub fn split(path: &str) -> Vec<PathBuf> {
    env::split_paths(path).collect()
}

/// Join path components into a PATH-like string using the host separator.
///
/// Returns an empty string when `paths` is empty, or when `join_paths`
/// rejects an entry (e.g. a component that embeds the separator).
pub fn join<I, P>(paths: I) -> String
where
    I: IntoIterator<Item = P>,
    P: AsRef<std::ffi::OsStr>,
{
    match env::join_paths(paths) {
        Ok(s) => s.to_string_lossy().into_owned(),
        Err(_) => String::new(),
    }
}

/// Count non-empty PATH entries (for spawn-failure diagnostics).
pub fn entry_count(path: &str) -> usize {
    split(path)
        .into_iter()
        .filter(|p| !p.as_os_str().is_empty())
        .count()
}

/// Prepend `dir` to a PATH string so it wins `execvp` / `SearchPath`
/// lookup order. Idempotent: if `dir` is already any segment, return
/// `path` unchanged. An empty `path` yields just `dir` (never a
/// leading separator, which shells treat as CWD).
pub fn prepend(path: &str, dir: &Path) -> String {
    let dir_os = dir.as_os_str();
    if path.is_empty() {
        return dir.to_string_lossy().into_owned();
    }
    if env::split_paths(path).any(|p| p.as_os_str() == dir_os) {
        return path.to_string();
    }
    let mut parts = Vec::with_capacity(16);
    parts.push(dir.to_path_buf());
    parts.extend(env::split_paths(path));
    join(parts)
}

/// Merge multiple PATH strings, first-occurrence-wins, skipping empty
/// segments. Later sources only contribute entries not already seen.
pub fn merge(sources: &[&str]) -> String {
    let mut seen: HashSet<OsString> = HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for src in sources {
        push_path_entries(&mut seen, &mut out, src);
    }
    join(out)
}

/// De-duplicated union of login-shell PATH, known install dirs, and the
/// process's inherited PATH — first occurrence wins, empty segments
/// dropped. Shared by [`super::login_path::merge_path`].
pub fn merge_login_known_inherited(
    login_path: Option<&str>,
    known_dirs: &[PathBuf],
    inherited: &str,
) -> String {
    let mut seen: HashSet<OsString> = HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();

    if let Some(lp) = login_path {
        push_path_entries(&mut seen, &mut out, lp);
    }
    for dir in known_dirs {
        push_one(&mut seen, &mut out, dir.clone());
    }
    push_path_entries(&mut seen, &mut out, inherited);

    join(out)
}

fn push_path_entries(seen: &mut HashSet<OsString>, out: &mut Vec<PathBuf>, path: &str) {
    for entry in env::split_paths(path) {
        push_one(seen, out, entry);
    }
}

fn push_one(seen: &mut HashSet<OsString>, out: &mut Vec<PathBuf>, entry: PathBuf) {
    if entry.as_os_str().is_empty() {
        return;
    }
    let key = entry.as_os_str().to_os_string();
    if seen.insert(key) {
        out.push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(parts: &[&str]) -> String {
        join(parts.iter().copied().map(PathBuf::from))
    }

    #[test]
    fn split_join_roundtrip_host_sep() {
        let original = joined(&["/usr/bin", "/bin", "/usr/sbin"]);
        let parts = split(&original);
        assert_eq!(parts.len(), 3);
        assert_eq!(join(parts), original);
    }

    #[test]
    fn merge_first_wins_dedup() {
        let a = joined(&["/a", "/shared"]);
        let b = joined(&["/shared", "/b"]);
        let c = joined(&["/shared", "/c"]);
        assert_eq!(merge(&[&a, &b, &c]), joined(&["/a", "/shared", "/b", "/c"]));
    }

    #[test]
    fn merge_skips_empty_segments() {
        // Leading/trailing/consecutive separators → empty segments.
        #[cfg(windows)]
        let messy = ";C:\\bin;;D:\\tools;";
        #[cfg(not(windows))]
        let messy = ":/usr/bin::/bin:";
        #[cfg(windows)]
        let expected = joined(&["C:\\bin", "D:\\tools"]);
        #[cfg(not(windows))]
        let expected = joined(&["/usr/bin", "/bin"]);
        assert_eq!(merge(&[messy, ""]), expected);
    }

    #[test]
    fn merge_login_known_inherited_ordering() {
        let login = joined(&["/login/bin"]);
        let known = vec![PathBuf::from("/known/bin")];
        let inherited = joined(&["/inherited/bin"]);
        assert_eq!(
            merge_login_known_inherited(Some(&login), &known, &inherited),
            joined(&["/login/bin", "/known/bin", "/inherited/bin"])
        );
    }

    #[test]
    fn merge_login_known_inherited_dedups() {
        let login = joined(&["/a", "/shared"]);
        let known = vec![PathBuf::from("/shared"), PathBuf::from("/b")];
        let inherited = joined(&["/shared", "/c"]);
        assert_eq!(
            merge_login_known_inherited(Some(&login), &known, &inherited),
            joined(&["/a", "/shared", "/b", "/c"])
        );
    }

    #[test]
    fn prepend_puts_dir_first_and_is_idempotent() {
        let bin = Path::new("/home/u/.k2/bin");
        let base = joined(&["/usr/bin", "/bin"]);
        assert_eq!(
            prepend(&base, bin),
            joined(&["/home/u/.k2/bin", "/usr/bin", "/bin"])
        );
        let already = joined(&["/usr/bin", "/home/u/.k2/bin", "/bin"]);
        assert_eq!(prepend(&already, bin), already);
        assert_eq!(prepend("", bin), "/home/u/.k2/bin");
    }

    #[test]
    fn entry_count_skips_empties() {
        #[cfg(windows)]
        let path = "C:\\a;;C:\\b;";
        #[cfg(not(windows))]
        let path = "/a::/b:";
        assert_eq!(entry_count(path), 2);
        assert_eq!(entry_count(""), 0);
    }

    /// Windows-shaped PATH: semicolon separators + drive letters.
    /// Only meaningful under `cfg(windows)` where the host separator is `;`.
    #[cfg(windows)]
    #[test]
    fn windows_semicolon_and_drive_letters() {
        let user = "C:\\Users\\u\\.local\\bin;C:\\Users\\u\\.cargo\\bin";
        let machine = "C:\\Windows\\System32;C:\\Windows";
        let known = vec![PathBuf::from("C:\\Users\\u\\AppData\\Roaming\\npm")];
        let merged = merge_login_known_inherited(Some(user), &known, machine);
        assert_eq!(
            merged,
            "C:\\Users\\u\\.local\\bin;C:\\Users\\u\\.cargo\\bin;C:\\Users\\u\\AppData\\Roaming\\npm;C:\\Windows\\System32;C:\\Windows"
        );
        // Prepend with drive-letter dir.
        let with_shim = prepend(&merged, Path::new("C:\\Users\\u\\.k2\\bin"));
        assert!(with_shim.starts_with("C:\\Users\\u\\.k2\\bin;"));
        assert_eq!(entry_count(&with_shim), 6);
    }

    /// Synthetic `;`-joined string without host separators: treated as
    /// a single segment on Unix (we never hardcode split-on-`;`).
    #[cfg(not(windows))]
    #[test]
    fn semicolon_only_string_is_single_segment_on_unix() {
        // No `:` in the string → split_paths yields exactly one entry
        // even though it looks Windows-shaped (`;` separators).
        let windows_shaped = r"D\Users\u\.local\bin;D\Users\u\.cargo\bin";
        let parts = split(windows_shaped);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], PathBuf::from(windows_shaped));

        // Known dirs without embedded `:` join cleanly with inherited.
        let known = vec![PathBuf::from("/home/u/.local/bin")];
        let merged = merge_login_known_inherited(None, &known, "/usr/bin:/bin");
        assert_eq!(merged, "/home/u/.local/bin:/usr/bin:/bin");

        // join_paths rejects components that embed the host separator
        // (drive-letter `C:` on Unix) — our join returns "" rather than
        // panicking.
        let bad = join([PathBuf::from(r"C:\Users\u\.local\bin")]);
        assert_eq!(bad, "", "drive-letter path embeds ':' on Unix");
    }
}
