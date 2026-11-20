//! Python 3.12's `posixpath`: `normpath`, `realpath` (not strict),
//! `abspath`, `relpath`, `split` and `join`, on `/` paths, as text.

use std::path::Path;

/// `os.path.normpath(p)`.
pub fn normpath(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let initial = if p.starts_with('/') {
        if p.starts_with("//") && !p.starts_with("///") {
            2
        } else {
            1
        }
    } else {
        0
    };
    let mut out: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if (initial == 0 && out.is_empty()) || out.last() == Some(&"..") {
                    out.push("..");
                } else if !out.is_empty() {
                    out.pop();
                }
            }
            c => out.push(c),
        }
    }
    let joined = out.join("/");
    let s = format!("{}{joined}", "/".repeat(initial));
    if s.is_empty() {
        ".".into()
    } else {
        s
    }
}

/// `os.path.join(a, b)`.
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `os.path.split(p)`.
pub fn split(p: &str) -> (String, String) {
    let i = p.rfind('/').map_or(0, |i| i + 1);
    let (head, tail) = (&p[..i], &p[i..]);
    let head = if !head.is_empty() && head.chars().any(|c| c != '/') {
        head.trim_end_matches('/')
    } else {
        head
    };
    (head.to_string(), tail.to_string())
}

/// `os.path.abspath(p)`: from the physical current folder.
pub fn abspath(p: &str) -> String {
    if p.starts_with('/') {
        normpath(p)
    } else {
        let cwd = std::env::current_dir()
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default();
        normpath(&join(&cwd, p))
    }
}

/// `os.path.realpath(p)` (Python 3.12, not strict): each link that is
/// there resolved, the rest by its words.
pub fn realpath(p: &str) -> String {
    let mut seen = std::collections::HashMap::new();
    let (path, _) = join_real("", p, &mut seen);
    abspath(&path)
}

fn join_real(
    path: &str,
    rest: &str,
    seen: &mut std::collections::HashMap<String, Option<String>>,
) -> (String, bool) {
    let mut path = path.to_string();
    let mut rest = rest;
    if rest.starts_with('/') {
        rest = &rest[1..];
        path = "/".into();
    }
    while !rest.is_empty() {
        let (name, after) = rest.split_once('/').unwrap_or((rest, ""));
        rest = after;
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            if path.is_empty() {
                path = "..".into();
            } else {
                let (head, tail) = split(&path);
                path = if tail == ".." {
                    join(&join(&head, ".."), "..")
                } else {
                    head
                };
            }
            continue;
        }
        let newpath = join(&path, name);
        let is_link = std::fs::symlink_metadata(&newpath).is_ok_and(|m| m.file_type().is_symlink());
        if !is_link {
            path = newpath;
            continue;
        }
        if let Some(done) = seen.get(&newpath) {
            match done {
                Some(p) => {
                    path = p.clone();
                    continue;
                }
                None => return (join(&newpath, rest), false),
            }
        }
        seen.insert(newpath.clone(), None);
        let target = std::fs::read_link(&newpath)
            .map(|t| t.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (p, ok) = join_real(&path, &target, seen);
        if !ok {
            return (join(&p, rest), false);
        }
        seen.insert(newpath, Some(p.clone()));
        path = p;
    }
    (path, true)
}

/// `os.path.relpath(p, start)`.
pub fn relpath(p: &str, start: &str) -> String {
    let parts = |x: &str| -> Vec<String> {
        abspath(x)
            .split('/')
            .filter(|c| !c.is_empty())
            .map(str::to_string)
            .collect()
    };
    let (sl, pl) = (parts(start), parts(p));
    let i = sl.iter().zip(&pl).take_while(|(a, b)| a == b).count();
    let mut out: Vec<String> = vec!["..".into(); sl.len() - i];
    out.extend(pl[i..].iter().cloned());
    if out.is_empty() {
        ".".into()
    } else {
        out.join("/")
    }
}

/// Whether `p` names something (`os.path.exists`).
pub fn exists(p: &str) -> bool {
    Path::new(p).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned from Python 3.12's posixpath.
    #[test]
    fn as_python_reads_them() {
        for (p, want) in [
            ("", "."),
            ("a/./b/../c/", "a/c"),
            ("//a//b", "//a/b"),
            ("///a/../..", "/"),
            ("../a/../../b", "../../b"),
            ("/..", "/"),
            ("./", "."),
        ] {
            assert_eq!(normpath(p), want, "{p:?}");
        }
        assert_eq!(split("/a/b"), ("/a".into(), "b".into()));
        assert_eq!(split("//x"), ("//".into(), "x".into()));
        assert_eq!(split("a/b//"), ("a/b".into(), "".into()));
        assert_eq!(split("x"), ("".into(), "x".into()));
        assert_eq!(relpath("/a/b/c", "/a"), "b/c");
        assert_eq!(relpath("/a", "/a/b"), "..");
        assert_eq!(relpath("/a", "/a"), ".");
        assert_eq!(realpath("/no/such/../thing"), "/no/thing");
    }
}
