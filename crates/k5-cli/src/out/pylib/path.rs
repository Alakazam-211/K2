//! Python 3.12's `posixpath`: `normpath`, `realpath` (not strict),
//! `abspath`, `relpath`, `split`, `join` and `expanduser`, on `/` paths,
//! as text.

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

/// `os.path.expanduser(p)`, `home` being `os.environ.get("HOME")`: `~`
/// and `~/...` take `HOME` (with none, the password database's folder
/// for this user), `~name/...` that user's folder; a name the database
/// has no entry for leaves `p` as it is, and so does no entry for this
/// user. `None`: the binary can't tell what Python would give (a folder
/// that is not UTF-8, or no password database here).
#[cfg(unix)]
pub fn expanduser(p: &str, home: Option<&str>) -> Option<String> {
    let Some(rest) = p.strip_prefix('~') else {
        return Some(p.to_string());
    };
    let (name, tail) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let userhome = match (name, home) {
        ("", Some(h)) => Some(h.to_string()),
        ("", None) => passwd_home(None)?,
        (name, _) => passwd_home(Some(name))?,
    };
    let Some(userhome) = userhome else {
        return Some(p.to_string());
    };
    let joined = format!("{}{tail}", userhome.trim_end_matches('/'));
    Some(if joined.is_empty() {
        "/".into()
    } else {
        joined
    })
}

/// `os.path.expanduser(p)` on Windows (`ntpath`): `~` and `~/...` (or
/// `~\...`) take `USERPROFILE`, else `HOMEDRIVE` and `HOMEPATH`; `HOME`
/// doesn't count there (`home` is not read). `~name` is the folder named
/// `name` beside this user's, unless it is this user's own. With no user
/// folder known, `p` as it is.
#[cfg(not(unix))]
pub fn expanduser(p: &str, _home: Option<&str>) -> Option<String> {
    let Some(rest) = p.strip_prefix('~') else {
        return Some(p.to_string());
    };
    let cut = rest.find(['/', '\\']).unwrap_or(rest.len());
    let (name, tail) = rest.split_at(cut);
    let var = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    let userhome = match var("USERPROFILE") {
        Some(h) => h,
        None => match (var("HOMEDRIVE"), var("HOMEPATH")) {
            (drive, Some(path)) => format!("{}{path}", drive.unwrap_or_default()),
            _ => return Some(p.to_string()),
        },
    };
    let userhome = if name.is_empty() {
        userhome
    } else {
        let trimmed = userhome.trim_end_matches(['/', '\\']);
        let (dir, base) = trimmed
            .rfind(['/', '\\'])
            .map_or(("", trimmed), |i| (&trimmed[..i], &trimmed[i + 1..]));
        if base == name {
            userhome
        } else {
            format!("{dir}\\{name}")
        }
    };
    Some(format!("{userhome}{tail}"))
}

/// The home folder in the password database for `name` (this user's
/// with `None`), as Python's `pwd` reads it: `Some(None)` for no entry
/// (Python's `KeyError`); `None` for a folder that is not UTF-8, a
/// lookup out of memory, or no database here.
#[cfg(unix)]
pub(crate) fn passwd_home(name: Option<&str>) -> Option<Option<String>> {
    use std::ffi::{CStr, CString};
    let name = match name {
        Some(n) => Some(CString::new(n).ok()?),
        None => None,
    };
    let mut buf = vec![0u8; 4096];
    loop {
        // SAFETY: a plain C struct, all zeroes, filled in by the call.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: the buffer, the entry and the result pointer live
        // through the call, and the length is the buffer's.
        let rc = unsafe {
            match &name {
                Some(n) => libc::getpwnam_r(
                    n.as_ptr(),
                    &mut entry,
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    &mut found,
                ),
                None => libc::getpwuid_r(
                    libc::getuid(),
                    &mut entry,
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    &mut found,
                ),
            }
        };
        if rc == libc::ERANGE && buf.len() < 1 << 20 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if found.is_null() {
            return (rc != libc::ENOMEM).then_some(None);
        }
        // SAFETY: the entry the call filled in points into `buf`, which
        // is still here.
        let dir = unsafe { CStr::from_ptr(entry.pw_dir) };
        return dir.to_str().ok().map(|d| Some(d.to_string()));
    }
}


/// No password database to read here: the binary can't tell.
#[cfg(not(unix))]
pub(crate) fn passwd_home(_name: Option<&str>) -> Option<Option<String>> {
    None
}

/// `cd <dir> && pwd` off Unix, where no shell runs it: `dir` from `pwd`
/// unless it is absolute, `.` parts gone and each `..` taking one back,
/// when that is a folder.
#[cfg(not(unix))]
pub fn cd_pwd(pwd: &Path, dir: &str) -> Option<String> {
    use std::path::{Component, PathBuf};
    let d = Path::new(dir);
    let base = if d.is_absolute() {
        d.to_path_buf()
    } else {
        pwd.join(d)
    };
    let mut out = PathBuf::new();
    for c in base.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.is_dir().then(|| out.to_string_lossy().into_owned())
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

    /// `ntpath.expanduser`: the user's own folder from USERPROFILE, a
    /// name beside it, and both slashes.
    #[cfg(not(unix))]
    #[test]
    fn expanduser_as_ntpath_reads_it() {
        let me = std::env::var("USERPROFILE").expect("USERPROFILE is set on Windows");
        let trimmed = me.trim_end_matches('\\');
        let (dir, base) = trimmed.rsplit_once('\\').expect("a folder in a folder");
        assert_eq!(expanduser("a/~b", None).as_deref(), Some("a/~b"));
        assert_eq!(expanduser("~", None).as_deref(), Some(me.as_str()));
        assert_eq!(expanduser("~/x", Some("/ignored")), Some(format!("{me}/x")));
        assert_eq!(expanduser("~\\x", None), Some(format!("{me}\\x")));
        assert_eq!(
            expanduser("~k5-no-such-user-7171/x", None),
            Some(format!("{dir}\\k5-no-such-user-7171/x"))
        );
        assert_eq!(expanduser(&format!("~{base}/x"), None), Some(format!("{me}/x")));
    }

    #[cfg(unix)]
    #[test]
    fn expanduser_as_python_reads_it() {
        let home = Some("/home/ada/");
        assert_eq!(expanduser("a/~b", home).as_deref(), Some("a/~b"));
        assert_eq!(expanduser("~", home).as_deref(), Some("/home/ada"));
        assert_eq!(expanduser("~/x/y", home).as_deref(), Some("/home/ada/x/y"));
        assert_eq!(expanduser("~/x", Some("")).as_deref(), Some("/x"));
        assert_eq!(expanduser("~", Some("/")).as_deref(), Some("/"));
        // A name the password database has no entry for: as it is.
        let none = "~k5-no-such-user-7171/x";
        assert_eq!(expanduser(none, home).as_deref(), Some(none));
        // root's folder, whatever this machine has.
        let root = expanduser("~root/x", home).expect("root's entry");
        assert!(root.ends_with("/x") && root.starts_with('/'), "{root}");
    }
}
