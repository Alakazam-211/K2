//! How bash names folders, for the families that answer what the shell
//! CLI's `cd` and `pwd` would: one copy of each rule.
//!
//! - [`canonpath`]: how a logical `cd` (and bash at start, for `PWD`)
//!   tidies an absolute path: empty parts and `.` dropped, `..` taking
//!   off the part before it, each step checked against the file system.
//! - [`cdpath`]: what `cd <dir>` does with `CDPATH` set. bash searches it
//!   only for a relative name that is not `.`, `..`, or one starting with
//!   `./` or `../`. Each entry (split on `:`; an empty one is the current
//!   folder) is tried in turn; the first `<entry>/<dir>` that `cd` can
//!   enter is where it goes, and when that entry is not empty (`.` too)
//!   bash prints the new folder on stdout, so `$(cd "$d" && pwd)` holds
//!   two lines. When no entry has it, `cd` goes on as with no `CDPATH`,
//!   and prints nothing.
//! - [`cd_dash`]: a folder whose name starts with `-` reaches `cd` as
//!   options. A letter `cd` doesn't take is bash's usage error: two lines
//!   on stderr ([`cd_refusal`]), and under `set -e` the run ends there.

use std::ffi::OsStr;
use std::path::PathBuf;

/// The bytes of `s` (Unix: as they are).
#[cfg(unix)]
pub fn bytes(s: &OsStr) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes()
}

#[cfg(not(unix))]
pub fn bytes(s: &OsStr) -> &[u8] {
    s.as_encoded_bytes()
}

#[cfg(unix)]
fn os(b: &[u8]) -> &OsStr {
    use std::os::unix::ffi::OsStrExt;
    OsStr::from_bytes(b)
}

#[cfg(not(unix))]
fn os(b: &[u8]) -> &OsStr {
    // SAFETY: only ever bytes cut from an `OsStr` at an ASCII `/`.
    unsafe { OsStr::from_encoded_bytes_unchecked(b) }
}

/// `[ -d <p> ]` (bash's `_path_isdir`: `stat` follows links).
fn is_dir(p: &[u8]) -> bool {
    std::fs::metadata(os(p)).is_ok_and(|m| m.is_dir())
}

/// bash's `sh_canonpath(path, PATH_CHECKDOTDOT | PATH_CHECKEXISTS)` for an
/// absolute `path` (bash 3.2 and 5.x alike): a leading `//` (two, not
/// three) is kept, as POSIX leaves it; empty parts and `.` are dropped;
/// `..` takes off the part before it once what is there so far is a
/// folder (`/..` is `/`); each other part is kept only when what is there
/// so far, with it, is a folder. `None` where a check fails, or for a
/// relative `path`.
pub fn canonpath_bytes(path: &[u8]) -> Option<Vec<u8>> {
    if !path.starts_with(b"/") {
        return None;
    }
    let double = path.len() >= 2 && path[1] == b'/' && path.get(2) != Some(&b'/');
    let root: &[u8] = if double { b"//" } else { b"/" };
    let mut out: Vec<u8> = root.to_vec();
    // Where `..` stops: just past the root.
    let base = out.len();
    for el in path[root.len()..].split(|b| *b == b'/') {
        match el {
            b"" | b"." => {}
            b".." => {
                if out.len() > base {
                    if !is_dir(&out) {
                        return None;
                    }
                    let cut = out[base..]
                        .iter()
                        .rposition(|b| *b == b'/')
                        .map_or(base, |i| base + i);
                    out.truncate(cut);
                }
            }
            el => {
                if out.len() > base {
                    out.push(b'/');
                }
                out.extend_from_slice(el);
                if !is_dir(&out) {
                    return None;
                }
            }
        }
    }
    Some(out)
}

/// [`canonpath_bytes`] for text.
pub fn canonpath(path: &str) -> Option<String> {
    canonpath_bytes(path.as_bytes()).map(|b| String::from_utf8(b).expect("cut at ASCII bytes"))
}

/// [`canonpath_bytes`] for an `OsStr`.
pub fn canonpath_os(path: &OsStr) -> Option<PathBuf> {
    canonpath_bytes(bytes(path)).map(|b| PathBuf::from(os(&b)))
}

/// `dirname <p>` as the system's `dirname` prints it (POSIX's steps):
/// trailing `/`s off, then the last part, then the `/`s before it; `/`
/// when nothing is left of a rooted path, `.` of one with no `/`. A
/// leading `//` is kept where the platform's `dirname` keeps it.
pub fn dirname(p: &[u8]) -> &[u8] {
    let mut end = p.len();
    while end > 1 && p[end - 1] == b'/' {
        end -= 1;
    }
    if end == 1 && p[0] == b'/' {
        return b"/";
    }
    let Some(slash) = p[..end].iter().rposition(|b| *b == b'/') else {
        return b".";
    };
    let mut cut = slash;
    while cut > 0 && p[cut - 1] == b'/' {
        cut -= 1;
    }
    if cut == 0 {
        return if DIRNAME_KEEPS_DOUBLE && slash == 1 {
            b"//"
        } else {
            b"/"
        };
    }
    &p[..cut]
}

/// Whether `dirname //a` is `//` (else `/`).
const DIRNAME_KEEPS_DOUBLE: bool = false;

/// What `CDPATH` does to one `cd <dir>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdPath {
    /// `CDPATH` is not read: it is unset or empty, or `dir` is absolute,
    /// `.`, `..`, or starts with `./` or `../`.
    NotSearched,
    /// Searched, and no entry has `dir`: `cd` goes on as with no `CDPATH`.
    NoMatch,
    /// An entry has `dir`: `cd` goes to `path` (`<entry>/<dir>`, before
    /// bash tidies it), and prints the new folder when `prints`.
    Found { path: String, prints: bool },
    /// The binary can't tell (a `CDPATH` that is not UTF-8).
    Unknown,
}

/// bash's `absolute_pathname`: `/…`, `.`, `./…`, `..` and `../…`.
fn absolute(dir: &str) -> bool {
    dir.starts_with('/')
        || dir == "."
        || dir == ".."
        || dir.starts_with("./")
        || dir.starts_with("../")
}

/// What `cd <dir>` does with `CDPATH` being `cdpath` (the variable, when
/// it is set). `enterable` says whether `cd` can enter a path (a folder
/// with search permission; relative ones from the current folder).
pub fn cdpath_with(dir: &str, cdpath: Option<&OsStr>, enterable: impl Fn(&str) -> bool) -> CdPath {
    let Some(cdpath) = cdpath.filter(|c| !c.is_empty()) else {
        return CdPath::NotSearched;
    };
    // `cd ''` is bash's own matter; no site sends it.
    if dir.is_empty() || absolute(dir) {
        return CdPath::NotSearched;
    }
    let Some(cdpath) = cdpath.to_str() else {
        return CdPath::Unknown;
    };
    for entry in cdpath.split(':') {
        // `sh_makepath(entry, dir, MP_DOTS)`: an empty entry is `.`.
        let path = match entry {
            "" => format!("./{dir}"),
            e if e.ends_with('/') => format!("{e}{dir}"),
            e => format!("{e}/{dir}"),
        };
        if enterable(&path) {
            return CdPath::Found {
                path,
                prints: !entry.is_empty(),
            };
        }
    }
    CdPath::NoMatch
}

/// [`cdpath_with`] for this process: its `CDPATH`, and `cd`'s test of a
/// folder it can enter.
pub fn cdpath(dir: &str) -> CdPath {
    cdpath_with(dir, std::env::var_os("CDPATH").as_deref(), enterable)
}

/// `CDPATH` leaves `cd <dir>` as it is with none: not searched, or no
/// entry has `dir`.
pub fn cd_unmoved(dir: &str) -> bool {
    matches!(cdpath(dir), CdPath::NotSearched | CdPath::NoMatch)
}

/// The letters bash's `cd` takes as options: 3.2's (macOS's `/bin/bash`),
/// or 4.4 and later's (`-@` only where the system has extended
/// attributes on folders, which Linux doesn't).
const CD_OPTIONS: &[u8] = if cfg!(target_os = "macos") {
    b"LP"
} else {
    b"eLP"
};

/// `cd`'s usage line, as `builtin_usage` prints it.
pub const CD_USAGE: &str = if cfg!(target_os = "macos") {
    "cd: usage: cd [-L|-P] [dir]"
} else {
    "cd: usage: cd [-L|[-P [-e]] [-@]] [dir]"
};

/// How the run ends after `x=$(cd "$d" && pwd)` fails on a bad option
/// under `set -e`: `cd`'s status (3.2: 1; 4.4 and later: 2, `EX_USAGE`).
pub const CD_USAGE_EXIT: crate::exit::Exit = if cfg!(target_os = "macos") {
    crate::exit::Exit::Error
} else {
    crate::exit::Exit::Usage
};

/// What `cd` makes of a word as its first argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdDash {
    /// No option: a folder name (it doesn't start with `-`).
    Folder,
    /// Options, and this letter is one `cd` doesn't take: its usage error.
    Invalid(char),
    /// Something else (`-`, `--`, options `cd` takes, a byte that is not
    /// ASCII): the binary leaves it to the shell.
    Other,
}

/// `cd <dir>`'s reading of `dir` (bash's `internal_getopt`): the first
/// letter after `-` that is not one of [`CD_OPTIONS`] is invalid.
pub fn cd_dash(dir: &str) -> CdDash {
    let Some(rest) = dir.strip_prefix('-') else {
        return CdDash::Folder;
    };
    if rest.is_empty() || rest.starts_with('-') {
        return CdDash::Other;
    }
    match rest.bytes().find(|b| !CD_OPTIONS.contains(b)) {
        Some(b) if b.is_ascii() => CdDash::Invalid(char::from(b)),
        _ => CdDash::Other,
    }
}

/// What bash prints for `cd <dir>` with an invalid option `c`: its error
/// (bash puts the script's name and line before it: `<path>: line <n>: `;
/// the binary doesn't, K2's ruling, the named difference
/// `cd-option-prefix`) and the usage line.
pub fn cd_refusal(c: char) -> String {
    format!("cd: -{c}: invalid option\n{CD_USAGE}\n")
}

/// Whether the binary leaves `x=$(cd <dir> && pwd)` to the shell, for a
/// `dir` that is a folder: options `cd` takes or another odd word
/// ([`CdDash::Other`]), or a folder name `CDPATH` moves `cd` away from
/// ([`cd_unmoved`]). An invalid option is the binary's: [`cd_refused`].
pub fn cd_to_shell(dir: &str) -> bool {
    match cd_dash(dir) {
        CdDash::Folder => !cd_unmoved(dir),
        CdDash::Invalid(_) => false,
        CdDash::Other => true,
    }
}

/// `x=$(cd <dir> && pwd)` under `set -e` with an option `cd` doesn't
/// take: bash's two lines on stderr ([`cd_refusal`]) and the run's end
/// ([`CD_USAGE_EXIT`]). `None` for any other `dir`.
pub fn cd_refused(dir: &str) -> Option<crate::exit::Exit> {
    let CdDash::Invalid(c) = cd_dash(dir) else {
        return None;
    };
    // As bash's own write: one that fails is not an error of its own.
    let _ = crate::out::write_flushed(&mut std::io::stderr().lock(), cd_refusal(c).as_bytes());
    Some(CD_USAGE_EXIT)
}

/// `chdir` would enter `p`: a folder (links followed) with search
/// permission.
#[cfg(unix)]
pub fn enterable(p: &str) -> bool {
    if !std::fs::metadata(p).is_ok_and(|m| m.is_dir()) {
        return false;
    }
    let Ok(c) = std::ffi::CString::new(p) else {
        return false;
    };
    // SAFETY: `c` is a valid C string for the call.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

#[cfg(not(unix))]
pub fn enterable(_p: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(dir: &str, cdpath: Option<&str>, here: &[&str]) -> CdPath {
        let here: Vec<String> = here.iter().map(|s| s.to_string()).collect();
        cdpath_with(dir, cdpath.map(OsStr::new), |p| here.iter().any(|h| h == p))
    }

    /// `dirname`'s steps, pinned from macOS's and GNU's.
    #[test]
    fn dirname_as_the_system_s() {
        for (p, d) in [
            ("/a/b", "/a"),
            ("/a/", "/"),
            ("/a", "/"),
            ("/", "/"),
            ("//", "/"),
            ("///", "/"),
            ("///a", "/"),
            ("//a", "/"),
            ("//a/b", "//a"),
            ("/a//b//", "/a"),
            ("/a/.", "/a"),
            ("/a/..", "/a"),
            ("a", "."),
            (".", "."),
            ("a/b", "a"),
        ] {
            assert_eq!(dirname(p.as_bytes()), d.as_bytes(), "{p}");
        }
    }

    /// `sh_canonpath`: a leading `//` (only two) kept, `.` and empty parts
    /// dropped, `..` only from a folder, every part a folder.
    #[test]
    fn canonpath_is_bash_s() {
        let d = std::env::temp_dir().join(format!("k5-shellpath-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::write(d.join("file"), "x").unwrap();
        let d = d.to_str().unwrap().to_string();
        let c = |p: &str| canonpath(p);
        assert_eq!(c("/"), Some("/".into()));
        assert_eq!(c("/.."), Some("/".into()));
        assert_eq!(c("//"), Some("//".into()));
        assert_eq!(c("///"), Some("/".into()));
        assert_eq!(c(&format!("{d}/a/./b/")), Some(format!("{d}/a/b")));
        assert_eq!(c(&format!("{d}//a/b/..")), Some(format!("{d}/a")));
        assert_eq!(c(&format!("{d}/a/../a/b")), Some(format!("{d}/a/b")));
        assert_eq!(c(&format!("/{d}/a")), Some(format!("/{d}/a")));
        assert_eq!(c(&format!("//{d}/a")), Some(format!("{d}/a")));
        assert_eq!(c(&format!("{d}/file/..")), None, "file is no folder");
        assert_eq!(c(&format!("{d}/nope/../a")), None);
        assert_eq!(c(&format!("{d}/file")), None);
        assert_eq!(c("rel"), None);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A word starting with `-` is options to `cd`.
    #[test]
    fn a_dash_word_is_options_to_cd() {
        assert_eq!(cd_dash("ws"), CdDash::Folder);
        assert_eq!(cd_dash("a-b"), CdDash::Folder);
        assert_eq!(cd_dash("-ws"), CdDash::Invalid('w'));
        assert_eq!(cd_dash("-Lw"), CdDash::Invalid('w'));
        assert_eq!(cd_dash("-@"), CdDash::Invalid('@'));
        assert_eq!(cd_dash("-L"), CdDash::Other);
        assert_eq!(cd_dash("-LP"), CdDash::Other);
        assert_eq!(cd_dash("-"), CdDash::Other);
        assert_eq!(cd_dash("--"), CdDash::Other);
        assert_eq!(cd_dash("--x"), CdDash::Other);
        assert_eq!(cd_dash("-\u{e9}"), CdDash::Other);
        let e = if cfg!(target_os = "macos") {
            CdDash::Invalid('e')
        } else {
            CdDash::Other
        };
        assert_eq!(cd_dash("-e"), e);
        assert_eq!(
            cd_refusal('w'),
            format!("cd: -w: invalid option\n{CD_USAGE}\n")
        );
    }

    /// Pinned from bash 3.2 and 5.2: which names `CDPATH` is read for,
    /// which entry wins, and when `cd` prints the folder.
    #[test]
    fn cd_reads_cdpath_as_bash_does() {
        let found = |path: &str, prints| CdPath::Found {
            path: path.to_string(),
            prints,
        };
        assert_eq!(with("ws", None, &[]), CdPath::NotSearched);
        assert_eq!(with("ws", Some(""), &[]), CdPath::NotSearched);
        for dir in ["/a", ".", "..", "./ws", "../ws", ""] {
            assert_eq!(with(dir, Some("/c"), &["/c/ws"]), CdPath::NotSearched, "{dir}");
        }
        // `.k2` is a relative name: searched.
        assert_eq!(with(".k2", Some("/c"), &["/c/ws"]), CdPath::NoMatch);
        assert_eq!(with(".k2", Some("/c"), &["/c/.k2"]), found("/c/.k2", true));
        assert_eq!(with("ws", Some("/c"), &["/c/ws"]), found("/c/ws", true));
        assert_eq!(with("ws", Some("/c/"), &["/c/ws"]), found("/c/ws", true));
        assert_eq!(with("ws", Some("/n:/c"), &["/c/ws"]), found("/c/ws", true));
        // `.` is an entry like any other: it prints.
        assert_eq!(with("ws", Some(".:/c"), &["./ws", "/c/ws"]), found("./ws", true));
        // An empty entry is the current folder, and prints nothing.
        assert_eq!(with("ws", Some(":/c"), &["./ws", "/c/ws"]), found("./ws", false));
        assert_eq!(with("ws", Some("/c:"), &["./ws"]), found("./ws", false));
        assert_eq!(with("ws", Some("/c"), &["./ws"]), CdPath::NoMatch);
    }
}
