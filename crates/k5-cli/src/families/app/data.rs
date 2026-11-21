//! An App's data folder, as the shell CLI answers it (`cmd_app_data`,
//! `cli/src/app-updates.sh`): `app data put|ls|list|get|rm|remove|mv|move`.
//! Each is the shell's word loop, then its requests, `--json` as it came,
//! else the shell's Python line for line.
//!
//! - `ls`: one `_compute_call GET /cli/app/data/ls app=…&path=…`.
//! - `get`: `GET /cli/app/data/get …&offset=N` until the daemon says
//!   `eof`, each piece's base64 decoded and appended to a part file next
//!   to the target (`.<name>.k2-part-<pid>`), renamed into place at the
//!   end; a failed get leaves no part file. With `-` the pieces go to
//!   stdout as they come. A size or time that changes between pieces ends
//!   it (exit 3). No daemon answer is put in argv or the environment: the
//!   pieces are read from the response body.
//! - `rm`, `mv`: one `_app_rt_post`, then `_app_grace_done`.
//! - `put`: the local path made absolute as `cd "$(dirname …)" && pwd -P`
//!   makes it, one POST with curl's `--max-time` at 1800 s, then
//!   `_app_grace_done` and `_app_disk_notes`.
//!
//! The shell's where the binary can't do what the shell does there: a
//! part file it couldn't create (the shell prints its own redirect error),
//! a path whose name is empty, and a `put` path whose folder `cd` can't
//! enter.

use super::call::{Cj, Ctx};
use super::grace_set::help;
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{py_len, raised, Raise, Shown};
use crate::out;
use crate::out::json::Value;
use crate::out::py as pyfmt;
use crate::transport::encode::quote;
use k2_client::cli_name;
use std::io::Write;
use std::path::Path;

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const DATA: &str = concat!(
        "usage: ",
        cli_name!(),
        " app data put|ls|get|rm|mv <app-id> \u{2026} (",
        cli_name!(),
        " app data --help)"
    );
    pub const LS_MORE: &str = concat!("usage: ", cli_name!(), " app data ls <app-id> [<path>]");
    pub const LS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app data ls <app-id> [<path>] [--json]"
    );
    pub const GET_TO: &str = "usage: --to <local path> | -";
    pub const GET_MORE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app data get <app-id> <path> [--to <local path> | -]"
    );
    pub const GET: &str = concat!(
        "usage: ",
        cli_name!(),
        " app data get <app-id> <path> [--to <local path> | -] [--json]"
    );
    pub const GET_JSON_STDOUT: &str = concat!(
        cli_name!(),
        " app data get: --json and - (stdout) don't go together"
    );
    pub const RM_MORE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app data rm <app-id> <path> [--recursive]"
    );
    pub const RM: &str = concat!(
        "usage: ",
        cli_name!(),
        " app data rm <app-id> <path> [--recursive] [--json]"
    );
    pub const MV_MORE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app data mv <app-id> <from> <to> [--force]"
    );
    pub const MV: &str = concat!(
        "usage: ",
        cli_name!(),
        " app data mv <app-id> <from> <to> [--force] [--json]"
    );
    pub const PUT: &str = concat!(
        "usage: ",
        cli_name!(),
        " app data put <app-id> <path> [--to <dir>]"
    );
}

/// `_app_data_usage`.
pub const PAGE: &str = "app/data";

/// The line when a piece couldn't be written (exit 3).
const PIECE_FAILED: &str = r#"{"error":"local_write","hint":"writing the piece failed"}"#;

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Page,
    Ls {
        app: String,
        path: String,
        json: bool,
    },
    /// `to` is `-` (stdout) or the final path; `part` the part file next
    /// to it.
    Get {
        app: String,
        path: String,
        to: String,
        part: String,
        json: bool,
    },
    Rm {
        body: String,
        json: bool,
    },
    Mv {
        body: String,
        json: bool,
    },
    Put {
        body: String,
        json: bool,
    },
}

/// The words after `data`. `None`: a help form, or a case the shell
/// answers (see the module's notes).
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    let rest = words.get(1..).unwrap_or_default();
    Some(match words.first().copied().unwrap_or("") {
        "" | "help" => Ok(Ask::Page),
        "put" => return put(rest),
        "ls" | "list" => ls(rest),
        "get" => return get(rest),
        "rm" | "remove" => rm(rest),
        "mv" | "move" => mv(rest),
        _ => Err(Stop::usage(usage::DATA)),
    })
}

fn unknown(w: &str) -> Stop {
    Stop::usage(format!("Unknown flag: {w}"))
}

/// The plain words, at most `max` (the next one is `more`'s line), with
/// the switches seen.
fn plain<'a>(
    words: &[&'a str],
    switches: &[&str],
    max: usize,
    more: &str,
) -> Result<(Vec<&'a str>, Vec<&'a str>), Stop> {
    let (mut got, mut seen) = (Vec::new(), Vec::new());
    for w in words {
        if switches.contains(w) {
            seen.push(*w);
        } else if w.starts_with('-') {
            return Err(unknown(w));
        } else if got.len() == max {
            return Err(Stop::usage(more));
        } else {
            got.push(*w);
        }
    }
    Ok((got, seen))
}

fn ls(words: &[&str]) -> Result<Ask, Stop> {
    let (got, seen) = plain(words, &["--json"], 2, usage::LS_MORE)?;
    let Some(app) = got.first().filter(|a| !a.is_empty()) else {
        return Err(Stop::usage(usage::LS));
    };
    Ok(Ask::Ls {
        app: app.to_string(),
        path: got.get(1).copied().unwrap_or("").to_string(),
        json: !seen.is_empty(),
    })
}

fn rm(words: &[&str]) -> Result<Ask, Stop> {
    let (got, seen) = plain(words, &["-r", "--recursive", "--json"], 2, usage::RM_MORE)?;
    let (app, path) = (
        got.first().copied().unwrap_or(""),
        got.get(1).copied().unwrap_or(""),
    );
    if app.is_empty() || path.is_empty() {
        return Err(Stop::usage(usage::RM));
    }
    let rec = seen.iter().any(|w| *w != "--json");
    Ok(Ask::Rm {
        body: Cj::new()
            .s("app", app)
            .s("path", path)
            .b("recursive", if rec { "1" } else { "" })
            .body(),
        json: seen.contains(&"--json"),
    })
}

fn mv(words: &[&str]) -> Result<Ask, Stop> {
    let (got, seen) = plain(words, &["-f", "--force", "--json"], 3, usage::MV_MORE)?;
    let at = |i: usize| got.get(i).copied().unwrap_or("");
    if at(0).is_empty() || at(1).is_empty() || at(2).is_empty() {
        return Err(Stop::usage(usage::MV));
    }
    let force = seen.iter().any(|w| *w != "--json");
    Ok(Ask::Mv {
        body: Cj::new()
            .s("app", at(0))
            .s("from", at(1))
            .s("to", at(2))
            .b("force", if force { "1" } else { "" })
            .body(),
        json: seen.contains(&"--json"),
    })
}

/// `basename <p>`.
fn basename(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() { String::new() } else { "/".into() };
    }
    t.rsplit('/').next().unwrap_or(t).to_string()
}

/// `dirname <p>`.
fn dirname(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() { ".".into() } else { "/".into() };
    }
    match t.rfind('/') {
        None => ".".into(),
        Some(i) => {
            let d = t[..i].trim_end_matches('/');
            if d.is_empty() {
                "/".into()
            } else {
                d.to_string()
            }
        }
    }
}

/// A folder the binary can make a file in (`: > "$tmp"` would work).
fn writable_dir(dir: &str) -> bool {
    if !Path::new(dir).is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        let Ok(c) = std::ffi::CString::new(dir) else {
            return false;
        };
        // SAFETY: `c` is a NUL-ended path; access only reads it.
        unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn get(words: &[&str]) -> Option<Result<Ask, Stop>> {
    let (mut app, mut path, mut to, mut json) = (None::<&str>, None::<&str>, String::new(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        rest = &rest[1..];
        if w == "--to" {
            let v = rest.first().copied().unwrap_or("");
            if v.is_empty() {
                return Some(Err(Stop::usage(usage::GET_TO)));
            }
            to = v.to_string();
            rest = &rest[1..];
        } else if let Some(v) = w.strip_prefix("--to=") {
            to = v.to_string();
        } else if w == "--json" {
            json = true;
        } else if w == "-" {
            to = "-".into();
        } else if w.starts_with('-') {
            return Some(Err(unknown(w)));
        } else if app.is_none_or(str::is_empty) {
            app = Some(w);
        } else if path.is_none_or(str::is_empty) {
            path = Some(w);
        } else {
            return Some(Err(Stop::usage(usage::GET_MORE)));
        }
    }
    let (app, path) = (app.unwrap_or(""), path.unwrap_or(""));
    if app.is_empty() || path.is_empty() {
        return Some(Err(Stop::usage(usage::GET)));
    }
    if to == "-" && json {
        return Some(Err(Stop::usage(usage::GET_JSON_STDOUT)));
    }
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    if name.is_empty() {
        return None;
    }
    if to.is_empty() {
        to = format!("./{name}");
    }
    if to != "-" && Path::new(&to).is_dir() {
        to = format!("{}/{name}", to.strip_suffix('/').unwrap_or(&to));
    }
    let mut part = String::new();
    if to != "-" {
        let dir = dirname(&to);
        part = format!("{dir}/.{}.k2-part-{}", basename(&to), std::process::id());
        if !writable_dir(&dir) || Path::new(&part).is_dir() {
            return None;
        }
    }
    Some(Ok(Ask::Get {
        app: app.to_string(),
        path: path.to_string(),
        to,
        part,
        json,
    }))
}

fn put(words: &[&str]) -> Option<Result<Ask, Stop>> {
    let (mut app, mut from, mut to, mut dov, mut json) =
        (String::new(), String::new(), String::new(), String::new(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--to" | "--disk-override" => {
                let Some(v) = rest.get(1) else {
                    return Some(Err(Stop::silent(Exit::Error)));
                };
                if w == "--to" {
                    to = v.to_string();
                } else {
                    dov = v.to_string();
                }
                rest = &rest[2..];
                continue;
            }
            "--json" => json = true,
            _ if w.starts_with("--disk-override=") => {
                dov = w["--disk-override=".len()..].to_string();
            }
            _ if w.starts_with('-') => return Some(Err(unknown(w))),
            _ if app.is_empty() => app = w.to_string(),
            _ => from = w.to_string(),
        }
        rest = &rest[1..];
    }
    if app.is_empty() || from.is_empty() {
        return Some(Err(Stop::usage(usage::PUT)));
    }
    if Path::new(&from).exists() {
        // `cd "$(dirname "$from")" && pwd -P`: a folder cd can't enter is
        // the shell's (its own error line).
        let dir = std::fs::canonicalize(dirname(&from)).ok()?;
        if !writable_search(&dir) {
            return None;
        }
        let dir = dir.to_str()?.to_string();
        from = format!("{dir}/{}", basename(&from));
    }
    Some(Ok(Ask::Put {
        body: Cj::new()
            .s("app", &app)
            .s("from", &from)
            .s("to", &to)
            .s("diskOverride", &dov)
            .body(),
        json,
    }))
}

/// Whether `cd` can enter the folder.
fn writable_search(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `c` is a NUL-ended path; access only reads it.
        unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        true
    }
}

fn printed(body: &[u8], json: bool, lines: fn(&[u8], &mut Out) -> Result<(), Raise>) -> Shown {
    if json {
        return raw(body);
    }
    let mut o = Out::default();
    let r = lines(body, &mut o).map(|()| Exit::Ok);
    o.done(r)
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Page => Ok(Shown::out(crate::text::help(PAGE).unwrap_or_default())),
        Ask::Ls { app, path, json } => {
            let q = format!("app={}&path={}", quote(app), quote(path));
            let body = ctx.call(false, "/cli/app/data/ls", &q)?;
            Ok(printed(&body, *json, ls_lines))
        }
        Ask::Get {
            app,
            path,
            to,
            part,
            json,
        } => get_run(ctx, app, path, to, part, *json),
        Ask::Rm { body, json } => {
            let s = ctx.send(true, "/cli/app/data/rm", body, false)?;
            match super::grace::done(ctx, s, *json) {
                Ok(b) => Ok(printed(&b, *json, rm_lines)),
                Err(a) => a,
            }
        }
        Ask::Mv { body, json } => {
            let s = ctx.send(true, "/cli/app/data/mv", body, false)?;
            match super::grace::done(ctx, s, *json) {
                Ok(b) => Ok(printed(&b, *json, mv_lines)),
                Err(a) => a,
            }
        }
        Ask::Put { body, json } => {
            let s = ctx.post_max("/cli/app/data/put", body, 1800)?;
            let b = match super::grace::done(ctx, s, *json) {
                Ok(b) => b,
                Err(a) => return a,
            };
            if *json {
                return Ok(raw(&b));
            }
            let mut o = Out::default();
            super::deploy::disk_notes(&b, &mut o);
            let r = put_lines(&b, &mut o).map(|()| Exit::Ok);
            Ok(o.done(r))
        }
    }
}

/// One piece's numbers, as the reader prints them: `size mtime eof next`.
struct Piece {
    size: String,
    mtime: String,
    eof: bool,
    next: String,
}

/// The piece reader's Python: the data written (to stdout for `-`, else
/// appended to the part file), then its numbers.
fn piece(body: &[u8], part: &str) -> Result<Piece, Raise> {
    let v = py::load(body, 3)?;
    let data = crate::families::mail::reads::b64decode(&py::idx(&v, "data", 4)?)
        .map_err(|e| Raise::at(4, e))?;
    if part.is_empty() {
        let mut so = std::io::stdout().lock();
        if so.write_all(&data).and_then(|()| so.flush()).is_err() {
            return Err(Raise::at(9, "BrokenPipeError: [Errno 32] Broken pipe"));
        }
    } else {
        let written = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(part)
            .and_then(|mut f| f.write_all(&data));
        if let Err(e) = written {
            return Err(Raise::at(12, format!("OSError: {e}")));
        }
    }
    let size = crate::families::py_d(&py::idx(&v, "size", 13)?, 13)?;
    let mtime = crate::families::py_d(&py::idx(&v, "mtime", 13)?, 13)?;
    let eof = py::truthy(&py::idx(&v, "eof", 13)?);
    let next = add(&py::idx(&v, "offset", 13)?, &py::idx(&v, "length", 13)?, 13)?;
    Ok(Piece {
        size,
        mtime,
        eof,
        next,
    })
}

/// `"%d" % (a + b)` for the numbers a piece names.
fn add(a: &Value, b: &Value, line: u32) -> Result<String, Raise> {
    let int = |v: &Value| match v {
        Value::Int(d) => d.parse::<i128>().ok(),
        Value::Bool(x) => Some(i128::from(*x)),
        _ => None,
    };
    match (int(a), int(b)) {
        (Some(x), Some(y)) => Ok((x + y).to_string()),
        _ => match (a, b) {
            (Value::Float(_) | Value::Int(_) | Value::Bool(_), Value::Float(_) | Value::Int(_) | Value::Bool(_)) => {
                let f = |v: &Value| match v {
                    Value::Float(x) => *x,
                    Value::Bool(x) => f64::from(u8::from(*x)),
                    Value::Int(d) => d.parse().unwrap_or(f64::INFINITY),
                    _ => 0.0,
                };
                crate::families::py_d(&Value::Float(f(a) + f(b)), line)
            }
            _ => Err(Raise::at(
                line,
                format!(
                    "TypeError: unsupported operand type(s) for +: '{}' and '{}'",
                    pyfmt::type_name(a),
                    pyfmt::type_name(b)
                ),
            )),
        },
    }
}

/// `_app_data_get`'s loop and its end. The part file goes on any end but
/// the rename.
fn get_run(ctx: &mut Ctx, app: &str, path: &str, to: &str, part: &str, json: bool) -> Answer {
    if !part.is_empty() && std::fs::write(part, b"").is_err() {
        // Checked when the words were read; the shell's line all the same.
        out::stderr_line(&format!(
            "{{\"error\":\"local_write\",\"hint\":\"can't write {to}\"}}"
        ));
        return Err(Exit::Refused);
    }
    let r = pieces(ctx, app, path, part);
    let (size, mtime) = match r {
        Ok(n) => n,
        Err(e) => {
            if !part.is_empty() {
                let _ = std::fs::remove_file(part);
            }
            return Err(e);
        }
    };
    if part.is_empty() {
        return Ok(Shown::out(""));
    }
    if std::fs::rename(part, to).is_err() {
        let _ = std::fs::remove_file(part);
        return Err(Exit::Error);
    }
    if json {
        let b = Cj::new()
            .s("app", app)
            .s("path", path)
            .s("to", to)
            .n("bytes", &size)
            .n("mtime", &mtime)
            .body();
        return Ok(Shown::out(format!("{b}\n")));
    }
    Ok(Shown::out(format!(
        "saved {path} ({size} bytes) from App {app} to {to}\n"
    )))
}

/// Every piece until `eof`: the size and time the first one named, or the
/// exit the command ended with (its lines printed).
fn pieces(ctx: &mut Ctx, app: &str, path: &str, part: &str) -> Result<(String, String), Exit> {
    let mut off = "0".to_string();
    let mut first: Option<(String, String)> = None;
    loop {
        let q = format!("app={}&path={}&offset={off}", quote(app), quote(path));
        let body = ctx.call(false, "/cli/app/data/get", &q)?;
        let p = match piece(&body, part) {
            Ok(p) => p,
            Err(e) => {
                let shown = raised(String::new(), String::new(), &e);
                let _ = out::write_flushed(&mut std::io::stderr().lock(), shown.stderr.as_bytes());
                out::stderr_line(PIECE_FAILED);
                return Err(Exit::Refused);
            }
        };
        match &first {
            None => first = Some((p.size.clone(), p.mtime.clone())),
            Some((size, mtime)) if *size != p.size || *mtime != p.mtime => {
                out::stderr_line(&format!(
                    "{{\"error\":\"data_changed\",\"hint\":\"{path} changed while it was read (size {size} \u{2192} {}): get it again\"}}",
                    p.size
                ));
                return Err(Exit::Refused);
            }
            Some(_) => {}
        }
        if p.eof {
            return Ok(first.unwrap_or_default());
        }
        off = p.next;
    }
}

/// `"%d %s" % (n, u)` or `"%.1f %s"`: `human(n)`, called from line 21.
fn human(n: &Value) -> Result<String, Raise> {
    if *n == Value::Null {
        return Ok("-".into());
    }
    let mut x = match n {
        Value::Int(d) => d.parse::<f64>().unwrap_or(f64::INFINITY),
        Value::Float(f) => *f,
        Value::Bool(b) => f64::from(u8::from(*b)),
        other => {
            return Err(Raise::inside(
                21,
                8,
                "human",
                format!(
                    "TypeError: '<' not supported between instances of '{}' and 'int'",
                    pyfmt::type_name(other)
                ),
            ))
        }
    };
    if x < 1024.0 {
        let d = crate::families::py_d(n, 9).map_err(|e| Raise::inside(21, 9, "human", e.exception))?;
        return Ok(format!("{d} B"));
    }
    for u in ["KiB", "MiB", "GiB"] {
        x /= 1024.0;
        if x < 1024.0 || u == "GiB" {
            return Ok(format!("{} {u}", py::f(x, 0, 1)));
        }
    }
    Ok(String::new())
}

/// `_app_data_ls`' Python.
fn ls_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let ents = py::or(py::dget(&v, "entries", 11)?, py::empty_list());
    let n = py_len(&ents, 12)?;
    o.line(&format!(
        "{} ({}, {n} {})",
        py::s(&py::dget(&v, "at", 12)?),
        py::s(&py::dget(&v, "kind", 12)?),
        if n == 1 { "entry" } else { "entries" }
    ));
    for e in py::iter(&ents, 13)? {
        let t = py::dget(&e, "mtime", 14)?;
        let when = if py::truthy(&t) {
            py::local_minutes(&t, 15)?
        } else {
            "-".to_string()
        };
        let mut name = py::dget_or(&e, "name", py::text(""), 16)?;
        let ty = py::dget(&e, "type", 17)?;
        let tail = if crate::out::pylib::ops::py_eq(&ty, &py::text("dir")) {
            Some(("/", 18))
        } else if crate::out::pylib::ops::py_eq(&py::dget(&e, "type", 19)?, &py::text("link")) {
            Some((" (link: never followed)", 20))
        } else {
            None
        };
        if let Some((tail, line)) = tail {
            name = add_to(&name, tail, line)?;
        }
        let ty = py::dget(&e, "type", 21)?;
        let size = human(&py::dget(&e, "size", 21)?)?;
        o.line(&format!(
            "  {} {}  {}  {}",
            py::left(&ty, 5),
            py::rjust(&size, 10),
            py::rjust(&when, 16),
            py::s(&name)
        ));
    }
    if py::truthy(&py::dget(&v, "truncated", 22)?) {
        o.line(&format!(
            "  \u{2026} more than {} entries: list a sub-folder",
            py::s(&py::dget(&v, "max", 23)?)
        ));
    }
    Ok(())
}

/// `name += tail`: a string grows; a list takes the tail's characters.
fn add_to(name: &Value, tail: &str, line: u32) -> Result<Value, Raise> {
    match name {
        Value::Str(_) => Ok(py::text(&format!("{}{tail}", py::s(name)))),
        Value::Array(items) => {
            let mut items = items.clone();
            items.extend(tail.chars().map(|c| py::text(&c.to_string())));
            Ok(Value::Array(items))
        }
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +=: '{}' and 'str'",
                pyfmt::type_name(other)
            ),
        )),
    }
}

/// `_app_data_rm`'s Python.
fn rm_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let removed = py::dget(&v, "removed", 4)?;
    let kind = py::word_for(
        &[("dir", "folder"), ("link", "link (not what it pointed to)")],
        &removed,
        py::dget(&v, "removed", 4)?,
        4,
    )?;
    o.line(&format!(
        "removed {} {} from App {}",
        py::s(&kind),
        py::s(&py::dget(&v, "at", 5)?),
        py::s(&py::dget(&v, "app", 5)?)
    ));
    Ok(())
}

/// `_app_data_mv`'s Python.
fn mv_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let from = py::dget(&v, "from", 4)?;
    let to = py::dget(&v, "to", 4)?;
    let app = py::dget(&v, "app", 4)?;
    let replaced = if py::truthy(&py::dget(&v, "replaced", 4)?) {
        " (replaced what was there)"
    } else {
        ""
    };
    o.line(&format!(
        "moved {} to {} in App {}{replaced}",
        py::s(&from),
        py::s(&to),
        py::s(&app)
    ));
    Ok(())
}

/// `_app_data_put`'s Python.
fn put_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    o.line(&format!(
        "copied {} file(s) from {} into {} of App {}",
        py::s(&py::dget(&v, "files", 4)?),
        py::s(&py::dget(&v, "from", 4)?),
        py::s(&py::dget(&v, "to", 4)?),
        py::s(&py::dget(&v, "app", 4)?)
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirname_and_basename_as_the_tools_print_them() {
        for (p, d, b) in [
            ("a/b", "a", "b"),
            ("a/b/", "a", "b"),
            ("b", ".", "b"),
            ("/b", "/", "b"),
            ("//b//", "/", "b"),
            ("./x", ".", "x"),
            ("/", "/", "/"),
        ] {
            assert_eq!((dirname(p), basename(p)), (d.to_string(), b.to_string()), "{p}");
        }
    }

    #[test]
    fn the_loops_read_the_words_as_the_shell_does() {
        assert_eq!(parse(&[]), Some(Ok(Ask::Page)));
        assert_eq!(
            parse(&["ls", "a", "p", "q"]),
            Some(Err(Stop::usage(usage::LS_MORE)))
        );
        assert_eq!(
            parse(&["get", "a", "p", "--to"]),
            Some(Err(Stop::usage(usage::GET_TO)))
        );
        assert_eq!(
            parse(&["get", "a", "p", "-", "--json"]),
            Some(Err(Stop::usage(usage::GET_JSON_STDOUT)))
        );
        assert_eq!(
            parse(&["rm", "a", "p", "-r"]),
            Some(Ok(Ask::Rm {
                body: r#"{"app": "a", "path": "p", "recursive": true}"#.into(),
                json: false
            }))
        );
        assert_eq!(
            parse(&["put", "a", "/nowhere/x", "--disk-override=r"]),
            Some(Ok(Ask::Put {
                body: r#"{"app": "a", "from": "/nowhere/x", "diskOverride": "r"}"#.into(),
                json: false
            }))
        );
        assert_eq!(parse(&["put", "a", "x", "--to"]), Some(Err(Stop::silent(Exit::Error))));
        assert!(parse(&["get", "a", "/"]).is_none(), "an empty name");
    }

    #[test]
    fn sizes_read_as_the_python_does() {
        assert_eq!(human(&Value::Null).unwrap(), "-");
        assert_eq!(human(&Value::Int("5".into())).unwrap(), "5 B");
        assert_eq!(human(&Value::Int("1536".into())).unwrap(), "1.5 KiB");
        assert_eq!(human(&Value::Float(2.5)).unwrap(), "2 B");
        assert!(human(&py::text("x")).is_err());
    }
}
