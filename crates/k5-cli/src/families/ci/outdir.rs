//! What `ci evidence` and `ci provenance` write under `--out`, as the
//! shell's `_CI_OUT_PY` (`cli/src/compute-provenance.sh`) does it:
//!
//! - [`check`] (its `check` mode): every label and file name the answer
//!   puts under the folder, checked before anything is written. One that
//!   is absolute, has a `..` part, holds a NUL or a newline, goes through
//!   a link or lands outside the folder is refused with exit 3 and one
//!   `code: hint` line (`ci_name_unsafe`), and nothing is written.
//! - [`put`] (its `put`): one file made fresh. The folders under `--out`
//!   are opened one at a time from the `--out` folder's descriptor with
//!   `O_NOFOLLOW` (no link is followed), a plain file already there is
//!   removed, and the new one is created with
//!   `O_CREAT|O_EXCL|O_NOFOLLOW`.
//!
//! The line numbers are the program's, for its tracebacks.

use super::files::join;
use crate::exit::Exit;
use crate::families::compute::pyrt as p;
use crate::families::compute::writes::{dumps, str_value};
use crate::families::Raise;
use crate::out;
use crate::out::json::{Text, Value};
use std::collections::BTreeSet;

/// A dict's value at `k`, or `None` (Python's `obj(x).get(k)`).
fn member(v: &Value, k: &str) -> Value {
    match v {
        Value::Object(_) => p::get(v, k).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// A dict's keys (`set(obj(x))`), none for anything else.
fn keys(v: &Value) -> Vec<Text> {
    match v {
        Value::Object(pairs) => pairs.iter().map(|(k, _)| k.clone()).collect(),
        _ => Vec::new(),
    }
}

/// A string as Python's `str` holds it.
fn text(t: &Text) -> String {
    p::str_of(&Value::Str(t.clone()))
}

/// `names(verb)`: (what, name, its path under the folder) for each name
/// the answer puts there, then the fixed files written beside them (an
/// empty name).
fn names(v: &Value, verb: &str) -> Vec<(&'static str, Text, String)> {
    let b = member(v, "bundle");
    let t = member(v, "trust");
    let mut got = Vec::new();
    if verb == "evidence" {
        if let Value::Str(f) = member(v, "file") {
            let rel = text(&f);
            got.push(("file name", f, rel));
        }
    }
    let st: BTreeSet<Text> = keys(&member(v, "statements"))
        .into_iter()
        .chain(keys(&member(&b, "provenance")))
        .collect();
    for k in st {
        let rel = format!("provenance/{}.intoto.jsonl", text(&k));
        got.push(("statement label", k, rel));
    }
    let mut nk = keys(&member(&t, "nodeKeys"));
    nk.sort();
    for k in nk {
        let rel = format!("trust/nodes/{}.pem", text(&k));
        got.push(("node key", k, rel));
    }
    if let (Value::Array(owners), Value::Array(rosters)) =
        (member(&t, "owners"), member(&t, "rosters"))
    {
        for (o, _) in owners.iter().zip(rosters.iter()) {
            if let Value::Str(fp) = member(o, "rosterKeyFp") {
                let rel = format!("trust/rosters/{}.json", text(&fp));
                got.push(("roster key", fp, rel));
            }
        }
    }
    if verb == "evidence" {
        let labels: BTreeSet<Text> = keys(&member(v, "jobs"))
            .into_iter()
            .chain(keys(&member(&b, "receipts")))
            .collect();
        for k in labels {
            let rel = text(&k);
            got.push(("job label", k, rel));
        }
    }
    if let Value::Array(statements) = member(&t, "statements") {
        for one in &statements {
            if let Value::Str(body) = member(one, "body") {
                let sum = crate::families::fed::pubkey::sha256(text(&body).as_bytes());
                let hex: String = sum.iter().map(|x| format!("{x:02x}")).collect();
                got.push(("path", Text::from(""), format!("trust/statements/{hex}.json")));
            }
        }
    }
    got.push(("path", Text::from(""), "trust/owners.json".to_string()));
    got
}

/// An exception raised in `link_on`, called by `why`.
fn in_link_on(exception: String) -> Raise {
    Raise {
        frames: vec![(111, "<module>"), (100, "why"), (86, "link_on")],
        exception,
    }
}

/// `link_on(rel)`: the first link on the way to `rel` under `dir`.
fn link_on(rel: &str, dir: &str) -> Result<Option<String>, Raise> {
    let mut path = dir.to_string();
    for part in rel.split('/').filter(|x| !x.is_empty()) {
        path = join(&path, part);
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => return Ok(Some(path)),
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(None)
            }
            Err(e) => {
                return Err(in_link_on(
                    crate::out::pylib::json::os_error(&e, &path).last_line(),
                ))
            }
        }
    }
    Ok(None)
}

/// `os.path.realpath(path)`: each link on the way resolved, `.` and `..`
/// taken as written.
fn realpath(path: &str) -> std::path::PathBuf {
    let mut got = if path.starts_with('/') {
        std::path::PathBuf::from("/")
    } else {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"))
    };
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                got.pop();
                continue;
            }
            _ => got.push(part),
        }
        let is_link = std::fs::symlink_metadata(&got).is_ok_and(|m| m.file_type().is_symlink());
        if is_link {
            if let Ok(real) = std::fs::canonicalize(&got) {
                got = real;
            }
        }
    }
    got
}

/// `why(name, rel)`: why the name is refused, if it is.
fn why(name: &Text, rel: &str, dir: &str) -> Result<Option<String>, Raise> {
    let n = text(name);
    if n.starts_with('/') {
        return Ok(Some("is an absolute path".into()));
    }
    if n.split('/').any(|x| x == "..") {
        return Ok(Some("has a .. part".into()));
    }
    if n.contains('\0') || n.contains('\n') {
        return Ok(Some("holds a NUL or a newline".into()));
    }
    if name.as_str().is_none() {
        // A lone surrogate: no file name can hold it.
        return Err(in_link_on(
            "UnicodeEncodeError: 'utf-8' codec can't encode character: surrogates not allowed"
                .into(),
        ));
    }
    if let Some(link) = link_on(rel, dir)? {
        return Ok(Some(format!("goes through the link {link}")));
    }
    let root = realpath(dir);
    if !realpath(&join(dir, rel)).starts_with(&root) {
        return Ok(Some(format!("lands outside {dir}")));
    }
    Ok(None)
}

/// A string's JSON text without its quotes: pieces escaped one by one
/// join to the whole escaped.
fn escaped(v: &Value) -> String {
    let d = dumps(v);
    d[1..d.len() - 1].to_string()
}

/// `check <verb> <dir>` on the answer: `Err` with the exit when a name is
/// refused (its line is printed) or the answer can't be read (the
/// traceback is printed).
pub fn check(body: &[u8], verb: &str, dir: &str) -> Result<(), Exit> {
    let v = match crate::out::pylib::json::loads(&String::from_utf8_lossy(body)) {
        Ok(v) => v,
        Err(f) => return Err(super::files::tb(&Raise::at(4, f.last_line()))),
    };
    for (what, name, rel) in names(&v, verb) {
        let bad = match why(&name, &rel, dir) {
            Ok(Some(bad)) => bad,
            Ok(None) => continue,
            Err(r) => return Err(super::files::tb(&r)),
        };
        let tail = str_value(&format!(" {bad}: refused, nothing written to {dir}"));
        let shown = if text(&name).is_empty() {
            escaped(&str_value(&rel))
        } else {
            format!(
                "{}{}",
                escaped(&str_value(&format!("{what} "))),
                escaped(&Value::Str(name))
            )
        };
        out::stderr_line(&format!(
            "{{\"error\": \"ci_name_unsafe\", \"hint\": \"{shown}{}\"}}",
            escaped(&tail)
        ));
        return Err(Exit::Refused);
    }
    Ok(())
}

/// Where `put` raised: its frames below the module's line, and the
/// exception.
pub struct PutFailed {
    frames: Vec<(u32, &'static str)>,
    exception: String,
}

impl PutFailed {
    fn new(frames: Vec<(u32, &'static str)>, exception: String) -> PutFailed {
        PutFailed { frames, exception }
    }

    /// The traceback, `put` called from line `from` of the module.
    pub fn raise(self, from: u32) -> Raise {
        let mut frames = vec![(from, "<module>")];
        frames.extend(self.frames);
        Raise {
            frames,
            exception: self.exception,
        }
    }
}

#[cfg(unix)]
mod at {
    use super::PutFailed;
    use crate::out::pylib::json::os_error;
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::OpenOptionsExt;

    fn name_c(name: &str) -> Result<CString, String> {
        CString::new(name).map_err(|_| "ValueError: embedded null byte".to_string())
    }

    fn last(name: &str) -> String {
        os_error(&std::io::Error::last_os_error(), name).last_line()
    }

    /// `folder(rel, make)`: the open folder that holds `rel` under `dir`,
    /// and the last part of `rel`. The first `make` folders (every one
    /// when `make < 0`) are made when missing. No link is followed.
    fn folder(dir: &str, rel: &str, make: i64) -> Result<(OwnedFd, String), (u32, String)> {
        let root = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY)
            .open(dir)
            .map_err(|e| (11, os_error(&e, dir).last_line()))?;
        let mut fd = OwnedFd::from(root);
        let parts: Vec<&str> = rel.split('/').collect();
        let Some((last_part, head)) = parts.split_last() else {
            return Ok((fd, String::new()));
        };
        for (i, part) in head.iter().enumerate() {
            if part.is_empty() {
                continue;
            }
            let c = name_c(part).map_err(|e| (19, e))?;
            if make < 0 || (i as i64) < make {
                // SAFETY: a folder descriptor this function owns and a C
                // string that lives through the call.
                let rc = unsafe { libc::mkdirat(fd.as_raw_fd(), c.as_ptr(), 0o777) };
                if rc != 0 {
                    let e = std::io::Error::last_os_error();
                    if e.raw_os_error() != Some(libc::EEXIST) {
                        return Err((19, os_error(&e, part).last_line()));
                    }
                }
            }
            // SAFETY: as above.
            let sub = unsafe {
                libc::openat(
                    fd.as_raw_fd(),
                    c.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if sub < 0 {
                return Err((22, last(part)));
            }
            // SAFETY: `sub` is a descriptor openat just gave us.
            fd = unsafe { OwnedFd::from_raw_fd(sub) };
        }
        Ok((fd, (*last_part).to_string()))
    }

    /// `put(rel, text, make)` up to the open file.
    pub fn create(dir: &str, rel: &str, make: i64) -> Result<(std::fs::File, String), PutFailed> {
        let (d, name) = folder(dir, rel, make)
            .map_err(|(line, e)| PutFailed::new(vec![(32, "put"), (line, "folder")], e))?;
        let c = name_c(&name).map_err(|e| PutFailed::new(vec![(35, "put")], e))?;
        // SAFETY: a zeroed `stat` is a valid value for fstatat to fill.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: the folder descriptor and C string live through the call.
        let rc = unsafe {
            libc::fstatat(d.as_raw_fd(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW)
        };
        if rc == 0 {
            if (st.st_mode & libc::S_IFMT) == libc::S_IFREG {
                // SAFETY: as above.
                if unsafe { libc::unlinkat(d.as_raw_fd(), c.as_ptr(), 0) } != 0 {
                    return Err(PutFailed::new(vec![(36, "put")], last(&name)));
                }
            }
        } else {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ENOENT) {
                return Err(PutFailed::new(
                    vec![(35, "put")],
                    os_error(&e, &name).last_line(),
                ));
            }
        }
        // SAFETY: as above.
        let fd = unsafe {
            libc::openat(
                d.as_raw_fd(),
                c.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o666 as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(PutFailed::new(vec![(39, "put")], last(&name)));
        }
        // SAFETY: `fd` is a descriptor openat just gave us.
        Ok((unsafe { std::fs::File::from_raw_fd(fd) }, name))
    }
}

/// `put(rel, text, make)`: `text` as the file `rel` under `dir`, made
/// fresh.
#[cfg(unix)]
pub fn put(dir: &str, rel: &str, text: &Value, make: i64) -> Result<(), PutFailed> {
    let (mut f, name) = at::create(dir, rel, make)?;
    super::files::write_str(&mut f, text, &name).map_err(|e| PutFailed::new(vec![(43, "put")], e))
}

/// Off Unix [`super::run`] hands `evidence` and `provenance` to the shell.
#[cfg(not(unix))]
pub fn put(_dir: &str, _rel: &str, _text: &Value, _make: i64) -> Result<(), PutFailed> {
    Err(PutFailed::new(
        vec![(39, "put")],
        "OSError: no openat here".into(),
    ))
}
