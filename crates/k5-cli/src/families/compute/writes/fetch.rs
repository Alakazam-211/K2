//! `k5 compute fetch <job> [NAME...] [--out DIR] [--json] [--over-relay]
//! [--attempt N]`, as `cmd_compute_fetch` answers it
//! (`cli/src/compute-jobs.sh`): the job's kept artifacts, then each named
//! one (or all), 4 MiB at a time from `/cli/compute/jobs/artifact` into
//! DIR under its relative name, asking again while the server still pulls
//! it from the node, each file checked against the sha256 and size the
//! node reported. Then one line per file, or `--json`'s list.
//!
//! Every answer is read here from its body; nothing the daemon sent goes
//! in argv or the environment (the shell's own fetch passed the growing
//! list in argv until 0.46.17; it reads it on stdin now, as here).
//!
//! The shell's readers end the command with 3 on any failure, a Python
//! traceback included (`|| exit 3`).

use super::{dumps, int_of, load, object, stopped, str_value, Cp, Words};
use crate::exit::Exit;
use crate::families::{get, py_iter, Raise, Shown};
use crate::out::json::{Options, Text, Value};
use crate::out::{self, json, py};
use crate::transport::encode::quote;
use std::io::Write;
use std::path::Path;

/// The route.
pub const ENDPOINT: &str = "/cli/compute/jobs/artifact";
/// `$fusage`.
pub const USAGE: &str = concat!(
    "usage: ",
    k2_client::cli_name!(),
    " compute fetch <job> [NAME...] [--out DIR] [--json] [--over-relay] [--attempt N]"
);
/// The most one answer carries.
pub const CHUNK: u64 = 4_194_304;

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    /// The list's reader.
    pub mod list {
        pub const LOAD: u32 = 3;
        pub const HAVE: u32 = 5;
        pub const MISSING: u32 = 8;
        pub const FOR: u32 = 10;
    }
    /// One step's reader.
    pub mod step {
        pub const LOAD: u32 = 3;
        pub const PENDING: u32 = 4;
        pub const MB: u32 = 6;
        pub const NAME: u32 = 9;
        pub const DATA: u32 = 15;
        pub const ROOT: u32 = 29;
        pub const MKDIR: u32 = 33;
        pub const FOLDER: u32 = 37;
        pub const STAT: u32 = 48;
        pub const UNLINK: u32 = 49;
        pub const OPEN: u32 = 56;
        pub const WRITE: u32 = 64;
        pub const PRINT: u32 = 65;
    }
    /// The sha256 check's reader.
    pub mod check {
        pub const OPEN: u32 = 12;
        pub const INT: u32 = 18;
    }
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub job: String,
    pub names: Vec<String>,
    pub out: String,
    pub json: bool,
    pub over_relay: bool,
    pub attempt: String,
}

impl Default for Ask {
    fn default() -> Self {
        Ask {
            job: String::new(),
            names: Vec::new(),
            out: ".".into(),
            json: false,
            over_relay: false,
            attempt: String::new(),
        }
    }
}

/// The words after `fetch`, as the shell's loop and checks read them.
pub fn parse(words: &[&str]) -> Words<Ask> {
    let mut a = Ask::default();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--out" | "-o" | "--attempt" => {
                let v = words.get(i + 1).copied().unwrap_or("").to_string();
                if w == "--attempt" {
                    a.attempt = v;
                } else {
                    a.out = v;
                }
                if i + 2 > words.len() {
                    return Words::ShiftFailed(words[i].to_string());
                }
                if w == "--attempt"
                    && (a.attempt.is_empty()
                        || a.attempt == "0"
                        || !a.attempt.bytes().all(|b| b.is_ascii_digit()))
                {
                    return Words::Usage(format!(
                        "{USAGE}  (--attempt N: an attempt number from 1)"
                    ));
                }
                i += 2;
                continue;
            }
            "--json" => a.json = true,
            "--over-relay" => a.over_relay = true,
            w if w.starts_with('-') => return Words::Usage(USAGE.to_string()),
            w if a.job.is_empty() => a.job = w.to_string(),
            w => a.names.push(w.to_string()),
        }
        i += 1;
    }
    if a.job.is_empty() || a.out.is_empty() {
        return Words::Usage(USAGE.to_string());
    }
    Words::Ask(a)
}

fn raised(r: &Raise) -> Exit {
    stopped(String::new(), r).print();
    Exit::Refused
}

/// One file fetched: what the check printed.
struct Got {
    name: String,
    path: String,
    bytes: Value,
    sha: String,
}

/// The requests, the files and what they print.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let jq = quote(&a.job);
    let aq = if a.attempt.is_empty() {
        String::new()
    } else {
        format!("&attempt={}", a.attempt)
    };
    let body = match cp.call(false, ENDPOINT, &format!("job={jq}{aq}"), false) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let list = match list_reader(&body, &a.names) {
        Ok(Ok(list)) => list,
        Ok(Err(line)) => {
            out::stderr_line(&line);
            return Exit::Refused;
        }
        Err(r) => return raised(&r),
    };
    // `run` hands the shell a folder `mkdir -p` would refuse, whose line
    // is the system's own; this is the race after that check.
    if std::fs::create_dir_all(&a.out).is_err() {
        out::stderr_line(&format!(
            "{{\"error\":\"local_write\",\"hint\":\"can't create {}\"}}",
            a.out
        ));
        return Exit::Refused;
    }
    let mut got: Vec<Got> = Vec::new();
    // `while IFS= read -r name ... done <<<"$list"`: one name a line.
    for name in held_lines(&list) {
        if name.is_empty() {
            continue;
        }
        let (mut offset, mut first) = ("0".to_string(), true);
        let last: Step;
        loop {
            let q = format!(
                "job={jq}&name={}&offset={offset}&max={CHUNK}&wait=20&over_relay={}{aq}",
                quote(&name),
                u8::from(a.over_relay)
            );
            let body = match cp.call(false, ENDPOINT, &q, false) {
                Ok(b) => b,
                Err(e) => return e,
            };
            let step = match step(&body, &a.out, first) {
                Ok(Some(s)) => s,
                Ok(None) => continue,
                Err(Stop::Line(l)) => {
                    out::stderr_line(&l);
                    return Exit::Refused;
                }
                Err(Stop::Raise(r)) => return raised(&r),
            };
            offset = step.offset.clone();
            first = false;
            if step.eof == "1" {
                last = step;
                break;
            }
        }
        match check(&a.out, &name, &last) {
            Ok(g) => got.push(g),
            Err(Stop::Line(l)) => {
                out::stderr_line(&l);
                return Exit::Refused;
            }
            Err(Stop::Raise(r)) => return raised(&r),
        }
    }
    let text = if a.json {
        let files = got
            .iter()
            .map(|g| {
                object(vec![
                    ("name", str_value(&g.name)),
                    ("path", str_value(&g.path)),
                    ("bytes", g.bytes.clone()),
                    ("sha256", str_value(&g.sha)),
                ])
            })
            .collect();
        let doc = object(vec![
            ("job", str_value(&a.job)),
            ("files", Value::Array(files)),
        ]);
        let opts = Options {
            indent: Some(2),
            ..Options::default()
        };
        format!("{}\n", json::to_string(&doc, &opts).unwrap_or_default())
    } else if got.is_empty() {
        "(no artifacts kept for this job)\n".to_string()
    } else {
        got.iter()
            .map(|g| {
                format!(
                    "{}  {} bytes  sha256 {}…\n",
                    g.path,
                    py::str_of(&g.bytes),
                    g.sha.chars().take(12).collect::<String>()
                )
            })
            .collect()
    };
    Shown::out(text).print()
}

/// An I/O error as the C library words it.
fn plain_io(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

/// The lines `read -r` takes from `<<<"$list"` (what `$(…)` held).
fn held_lines(list: &str) -> Vec<String> {
    list.trim_end_matches('\n')
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// The list's reader: the names to fetch, one a line (`Ok(Ok)`), or the
/// `artifact_not_found` line (`Ok(Err)`).
pub fn list_reader(body: &[u8], want: &[String]) -> Result<Result<String, String>, Raise> {
    use line::list as l;
    let v = load(body, l::LOAD)?;
    let Value::Object(pairs) = &v else {
        return Err(Raise::no_attribute(l::HAVE, &v, "get"));
    };
    let arts = match get(pairs, "artifacts") {
        Some(x) if py::truthy(x) => x.clone(),
        _ => Value::Array(Vec::new()),
    };
    let mut have: Vec<String> = Vec::new();
    for a in py_iter(&arts, l::HAVE)? {
        // Python 3.12 runs a comprehension in its caller's frame.
        let name = super::subscript(&a, "name").map_err(|e| Raise::at(l::HAVE, e))?;
        if matches!(name, Value::Array(_) | Value::Object(_)) {
            return Err(Raise::at(
                l::HAVE,
                format!("TypeError: unhashable type: '{}'", py::type_name(&name)),
            ));
        }
        let n = py::str_of(&name);
        if !have.contains(&n) {
            have.push(n);
        }
    }
    let mut sorted = have.clone();
    sorted.sort();
    let missing: Vec<&String> = want.iter().filter(|w| !have.contains(w)).collect();
    if !missing.is_empty() {
        let kept = if sorted.is_empty() {
            "(nothing)".to_string()
        } else {
            sorted.join(", ")
        };
        let short = super::str_at(get(pairs, "short"));
        return Ok(Err(dumps(&object(vec![
            ("error", str_value("artifact_not_found")),
            ("hint", str_value(&format!("job {short} kept: {kept}"))),
        ]))));
    }
    let names = if want.is_empty() { &sorted } else { want };
    let mut out = String::new();
    for n in names {
        out.push_str(n);
        out.push('\n');
    }
    Ok(Ok(out))
}

/// What a step's reader printed: `<next offset> <eof> <sha> <bytes>
/// <path>`, as `read -r` splits it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub offset: String,
    pub eof: String,
    pub sha: String,
    pub bytes: String,
    pub path: String,
}

/// Why a reader stopped: its own line, or Python's raise.
pub enum Stop {
    Line(String),
    Raise(Raise),
}

impl From<Raise> for Stop {
    fn from(r: Raise) -> Stop {
        Stop::Raise(r)
    }
}

fn key<'a>(pairs: &'a [(Text, Value)], k: &str, line: u32) -> Result<&'a Value, Raise> {
    get(pairs, k).ok_or_else(|| Raise::at(line, format!("KeyError: '{k}'")))
}

/// `x >> 20`.
fn mb(v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Int(_) | Value::Bool(_) => {
            let d = int_of(v).map_err(|e| Raise::at(line, e))?;
            Ok(match d.parse::<i128>() {
                Ok(i) => (i >> 20).to_string(),
                Err(_) => d,
            })
        }
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for >>: '{}' and 'int'",
                py::type_name(other)
            ),
        )),
    }
}

/// One step's reader: `None` while the file is still on its way (its line
/// on stderr); else the chunk written to its file, and the step.
pub fn step(body: &[u8], out_dir: &str, first: bool) -> Result<Option<Step>, Stop> {
    use line::step as l;
    let v = load(body, l::LOAD)?;
    let Value::Object(pairs) = &v else {
        return Err(Raise::no_attribute(l::PENDING, &v, "get").into());
    };
    if get(pairs, "pending").is_some_and(py::truthy) {
        let name = key(pairs, "name", l::MB)?;
        let got = get(pairs, "got").cloned().unwrap_or(Value::Int("0".into()));
        let got = mb(&got, l::MB)?;
        let bytes = mb(key(pairs, "bytes", l::MB)?, l::MB)?;
        let _ = out::write_flushed(
            &mut std::io::stderr().lock(),
            format!(
                "{}: {} of {} MB on the server so far\n",
                py::str_of(name),
                got,
                bytes
            )
            .as_bytes(),
        );
        return Ok(None);
    }
    let name = key(pairs, "name", l::NAME)?;
    let Value::Str(_) = name else {
        return Err(Raise::at(
            l::NAME + 1,
            format!(
                "AttributeError: '{}' object has no attribute 'split'",
                py::type_name(name)
            ),
        )
        .into());
    };
    let name = py::str_of(name);
    let parts: Vec<&str> = name.split('/').collect();
    if name.starts_with('/') || parts.iter().any(|p| matches!(*p, "" | "." | "..")) {
        return Err(Stop::Line(dumps(&object(vec![
            ("error", str_value("artifact_name_unsafe")),
            ("hint", str_value(&name)),
        ]))));
    }
    let path = join(out_dir, &parts);
    let data = match key(pairs, "data", l::DATA)? {
        Value::Str(t) => {
            b64decode(&py::str_of(&Value::Str(t.clone()))).map_err(|e| Raise::at(l::DATA, e))?
        }
        other => {
            return Err(Raise::at(
                l::DATA,
                format!(
                    "TypeError: argument should be a bytes-like object or ASCII string, not '{}'",
                    py::type_name(other)
                ),
            )
            .into())
        }
    };
    let mut file = nofollow::open(out_dir, &name, &parts, first)?;
    if let Err(e) = file.write_all(&data) {
        return Err(Raise::at(l::WRITE, os_error(&e, &path)).into());
    }
    let offset = key(pairs, "offset", l::PRINT)?;
    let next = match offset {
        Value::Int(_) | Value::Bool(_) => {
            let d = int_of(offset).map_err(|e| Raise::at(l::PRINT, e))?;
            match d.parse::<i128>() {
                Ok(i) => (i + data.len() as i128).to_string(),
                Err(_) => d,
            }
        }
        other => {
            return Err(Raise::at(
                l::PRINT,
                format!(
                    "TypeError: unsupported operand type(s) for +: '{}' and 'int'",
                    py::type_name(other)
                ),
            )
            .into())
        }
    };
    let eof = if py::truthy(key(pairs, "eof", l::PRINT)?) {
        "1"
    } else {
        "0"
    };
    let sha = py::str_of(key(pairs, "sha256", l::PRINT)?);
    let bytes = py::str_of(key(pairs, "bytes", l::PRINT)?);
    // `read -r offset eof sha bytes path <<<"$step"`.
    Ok(Some(read_step(&format!(
        "{next} {eof} {sha} {bytes} {path}"
    ))))
}

/// `read -r offset eof sha bytes path` on the step's line (what `$(…)`
/// held): split on spaces, tabs and newlines; the last name takes the
/// rest, its spaces at each end gone.
fn read_step(printed: &str) -> Step {
    let held = printed.trim_end_matches('\n');
    let line = held.split('\n').next().unwrap_or("");
    let ws = |c: char| c == ' ' || c == '\t';
    let mut rest = line.trim_start_matches(ws);
    let mut fields = Vec::new();
    for _ in 0..4 {
        let end = rest.find(ws).unwrap_or(rest.len());
        fields.push(rest[..end].to_string());
        rest = rest[end..].trim_start_matches(ws);
    }
    Step {
        offset: fields[0].clone(),
        eof: fields[1].clone(),
        sha: fields[2].clone(),
        bytes: fields[3].clone(),
        path: rest.trim_end_matches(ws).to_string(),
    }
}

/// `os.path.join(out, *parts)`.
fn join(out: &str, parts: &[&str]) -> String {
    let mut p = out.to_string();
    for part in parts {
        if !p.is_empty() && !p.ends_with('/') {
            p.push('/');
        }
        p.push_str(part);
    }
    p
}

/// Python's `OSError` line for a path.
fn os_error(e: &std::io::Error, path: &str) -> String {
    let (name, errno) = match e.kind() {
        std::io::ErrorKind::PermissionDenied => ("PermissionError", 13),
        std::io::ErrorKind::NotFound => ("FileNotFoundError", 2),
        std::io::ErrorKind::AlreadyExists => ("FileExistsError", 17),
        _ => ("OSError", e.raw_os_error().unwrap_or(0)),
    };
    format!(
        "{name}: [Errno {errno}] {}: {}",
        plain_io(e),
        py::repr(&str_value(path))
    )
}

/// The check of one file: its sha256 and size against what the node
/// reported, then the line it adds to the list.
fn check(out_dir: &str, name: &str, s: &Step) -> Result<Got, Stop> {
    let parts: Vec<&str> = name.split('/').collect();
    let data = nofollow::read(out_dir, &parts)
        .map_err(|(e, at)| Stop::Raise(Raise::at(line::check::OPEN, os_error(&e, at))))?;
    let h = k2_client::release_sums::sha256_hex(&data);
    let changed = || {
        Stop::Line(dumps(&object(vec![
            ("error", str_value("artifact_changed")),
            (
                "hint",
                str_value(&format!(
                    "{} does not match the sha256 the node reported; fetch it again",
                    s.path
                )),
            ),
        ])))
    };
    if h != s.sha {
        return Err(changed());
    }
    let want =
        int_of(&str_value(&s.bytes)).map_err(|e| Stop::Raise(Raise::at(line::check::INT, e)))?;
    if data.len().to_string() != want {
        return Err(changed());
    }
    Ok(Got {
        name: name.to_string(),
        path: s.path.clone(),
        bytes: Value::Int(want),
        sha: h,
    })
}

/// `base64.b64decode(s)`: characters outside the alphabet are dropped,
/// then the padding must be right.
pub fn b64decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.is_ascii() {
        return Err("ValueError: string argument should contain only ASCII characters".to_string());
    }
    let val = |c: u8| -> Option<u8> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::new();
    let mut quad: Vec<u8> = Vec::new();
    let mut pads = 0;
    let mut done = false;
    for c in s.bytes() {
        if done {
            break;
        }
        if c == b'=' {
            if quad.len() >= 2 {
                pads += 1;
                if quad.len() + pads == 4 {
                    done = true;
                }
            }
            continue;
        }
        let Some(v) = val(c) else { continue };
        if pads > 0 {
            // Data after padding: binascii keeps reading as if none.
            pads = 0;
        }
        quad.push(v);
        if quad.len() == 4 {
            out.push((quad[0] << 2) | (quad[1] >> 4));
            out.push((quad[1] << 4) | (quad[2] >> 2));
            out.push((quad[2] << 6) | quad[3]);
            quad.clear();
        }
    }
    match (quad.len(), done) {
        (0, _) => Ok(out),
        (2, true) => {
            out.push((quad[0] << 2) | (quad[1] >> 4));
            Ok(out)
        }
        (3, true) => {
            out.push((quad[0] << 2) | (quad[1] >> 4));
            out.push((quad[1] << 4) | (quad[2] >> 2));
            Ok(out)
        }
        (1, _) => Err(format!(
            "binascii.Error: Invalid base64-encoded string: number of data characters ({}) cannot be 1 more than a multiple of 4",
            s.bytes().filter(|c| val(*c).is_some()).count()
        )),
        _ => Err("binascii.Error: Incorrect padding".to_string()),
    }
}

/// Whether `mkdir -p <out>` can make (or finds) the folder: it is one,
/// or the nearest part of it that is there is a folder this user may
/// write in.
pub fn creatable(out: &str) -> bool {
    let mut p = Path::new(out);
    loop {
        match std::fs::metadata(p) {
            Ok(m) => return m.is_dir() && (p == Path::new(out) || writable(p)),
            Err(_) => match p.parent() {
                Some(up) if !up.as_os_str().is_empty() => p = up,
                _ => return writable(Path::new(".")),
            },
        }
    }
}

#[cfg(unix)]
fn writable(d: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(d.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: a valid NUL-terminated path and a plain mode.
    unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

#[cfg(not(unix))]
fn writable(d: &Path) -> bool {
    d.is_dir()
}

/// The `artifact_path_unsafe` line: the link at `parts[..=i]` under
/// `out`, met where it was opened.
#[cfg_attr(not(unix), allow(dead_code))]
fn refused(name: &str, out: &str, parts: &[&str], i: usize) -> Stop {
    Stop::Line(dumps(&object(vec![
        ("error", str_value("artifact_path_unsafe")),
        (
            "hint",
            str_value(&format!(
                "{name}: {} is a link, not followed; nothing written there",
                join(out, &parts[..=i])
            )),
        ),
    ])))
}

/// The writes of one step, as the shell's reader makes them: the folders
/// under `--out` opened one at a time from its descriptor with
/// `O_NOFOLLOW` (each made when missing), the file's first chunk made
/// fresh (a plain file already there removed, then
/// `O_CREAT|O_EXCL|O_NOFOLLOW`), the rest appended with `O_NOFOLLOW`. An
/// open that meets a link (there before, or put there since) is refused
/// ([`refused`]).
#[cfg(unix)]
mod nofollow {
    use super::{line::step as l, os_error, refused, Raise, Stop};
    use std::ffi::CString;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::OpenOptionsExt;

    fn c(name: &str, line: u32) -> Result<CString, Raise> {
        CString::new(name).map_err(|_| Raise::at(line, "ValueError: embedded null byte"))
    }

    fn last(line: u32, name: &str) -> Raise {
        Raise::at(line, os_error(&std::io::Error::last_os_error(), name))
    }

    /// `linked(d, i)`: whether `name` in the folder `d` is a link.
    fn linked(d: &OwnedFd, name: &CString) -> bool {
        // SAFETY: a zeroed `stat` is a valid value for fstatat to fill.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: the folder descriptor and C string live through the call.
        let rc = unsafe {
            libc::fstatat(
                d.as_raw_fd(),
                name.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        rc == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFLNK
    }

    /// `openat(d, name, flags)`, or the OS error.
    fn at(d: &OwnedFd, name: &CString, flags: libc::c_int) -> Result<OwnedFd, std::io::Error> {
        // SAFETY: the folder descriptor and C string live through the call.
        let fd =
            unsafe { libc::openat(d.as_raw_fd(), name.as_ptr(), flags, 0o666 as libc::c_uint) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` is a descriptor openat just gave us.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn root(out: &str) -> std::io::Result<OwnedFd> {
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY)
            .open(out)
            .map(OwnedFd::from)
    }

    const DIR: libc::c_int =
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

    /// The artifact's file under `out`, open for its chunk.
    pub fn open(out: &str, name: &str, parts: &[&str], first: bool) -> Result<std::fs::File, Stop> {
        let mut d = root(out).map_err(|e| Raise::at(l::ROOT, os_error(&e, out)))?;
        let Some((file, folders)) = parts.split_last() else {
            return Err(Raise::at(l::OPEN, "FileNotFoundError: no name").into());
        };
        for (i, part) in folders.iter().enumerate() {
            let p = c(part, l::MKDIR)?;
            // SAFETY: a folder descriptor this function owns and a C string
            // that lives through the call.
            if unsafe { libc::mkdirat(d.as_raw_fd(), p.as_ptr(), 0o777) } != 0 {
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() != Some(libc::EEXIST) {
                    return Err(Raise::at(l::MKDIR, os_error(&e, part)).into());
                }
            }
            d = match at(&d, &p, DIR) {
                Ok(sub) => sub,
                Err(_) if linked(&d, &p) => return Err(refused(name, out, parts, i)),
                Err(e) => return Err(Raise::at(l::FOLDER, os_error(&e, part)).into()),
            };
        }
        let n = c(file, l::OPEN)?;
        let i = parts.len() - 1;
        let flags = if first {
            if linked(&d, &n) {
                return Err(refused(name, out, parts, i));
            }
            // SAFETY: a zeroed `stat` is a valid value for fstatat to fill.
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: the folder descriptor and C string live through the call.
            let rc = unsafe {
                libc::fstatat(
                    d.as_raw_fd(),
                    n.as_ptr(),
                    &mut st,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if rc == 0 {
                // SAFETY: as above.
                if (st.st_mode & libc::S_IFMT) == libc::S_IFREG
                    && unsafe { libc::unlinkat(d.as_raw_fd(), n.as_ptr(), 0) } != 0
                {
                    return Err(last(l::UNLINK, file).into());
                }
            } else {
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() != Some(libc::ENOENT) {
                    return Err(Raise::at(l::STAT, os_error(&e, file)).into());
                }
            }
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC
        } else {
            libc::O_WRONLY | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_CLOEXEC
        };
        match at(&d, &n, flags) {
            Ok(fd) => Ok(std::fs::File::from(fd)),
            Err(_) if linked(&d, &n) => Err(refused(name, out, parts, i)),
            Err(e) => Err(Raise::at(l::OPEN, os_error(&e, file)).into()),
        }
    }

    /// The file `parts` under `out`, read from the folders down, each
    /// opened with `O_NOFOLLOW`; the OS error and the name it was on.
    pub fn read<'a>(out: &'a str, parts: &[&'a str]) -> Result<Vec<u8>, (std::io::Error, &'a str)> {
        let mut d = root(out).map_err(|e| (e, out))?;
        let Some((file, folders)) = parts.split_last() else {
            return Err((std::io::Error::from(std::io::ErrorKind::NotFound), out));
        };
        let cs = |s: &'a str| {
            CString::new(s).map_err(|_| (std::io::Error::from(std::io::ErrorKind::InvalidInput), s))
        };
        for part in folders {
            d = at(&d, &cs(part)?, DIR).map_err(|e| (e, *part))?;
        }
        let fd = at(
            &d,
            &cs(file)?,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
        .map_err(|e| (e, *file))?;
        let mut data = Vec::new();
        std::fs::File::from(fd)
            .read_to_end(&mut data)
            .map_err(|e| (e, *file))?;
        Ok(data)
    }
}

/// Elsewhere: the folders made, the file made fresh or appended to.
#[cfg(not(unix))]
mod nofollow {
    use super::{line::step as l, os_error, Raise, Stop};

    pub fn open(
        out: &str,
        _name: &str,
        parts: &[&str],
        first: bool,
    ) -> Result<std::fs::File, Stop> {
        let path = super::join(out, parts);
        if let Some(dir) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Raise::at(l::MKDIR, os_error(&e, &dir.to_string_lossy())))?;
        }
        if first {
            let _ = std::fs::remove_file(&path);
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(first)
            .append(!first)
            .open(&path)
            .map_err(|e| Raise::at(l::OPEN, os_error(&e, &path)).into())
    }

    pub fn read<'a>(out: &'a str, parts: &[&'a str]) -> Result<Vec<u8>, (std::io::Error, &'a str)> {
        std::fs::read(super::join(out, parts)).map_err(|e| (e, out))
    }
}
