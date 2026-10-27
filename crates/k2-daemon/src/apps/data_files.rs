//! `k2 app data ls|get|rm|mv` (0.46.4): read back, list, remove and
//! rename what an App keeps in its data folder (`$K2_APP_DATA`, `/data` in
//! the wall). The other half of [`super::data_put`]: an App's pages write
//! answers and uploads to `/data`, and the App's workspace agent reads them
//! back to do its work.
//!
//! ```text
//! GET  /cli/app/data/ls  ?app&path?            → {entries, kind, truncated}
//! GET  /cli/app/data/get ?app&path&offset?     → one chunk (base64) of a file
//! POST /cli/app/data/rm  {app, path, recursive?}
//! POST /cli/app/data/mv  {app, from, to, force?}
//! ```
//!
//! - **Who:** as `data put`: the owner, an Admin, a shell tab, or the App's
//!   workspace agent with "Build and run Apps". Without the switch, `rm`
//!   and `mv` become a held `manage` request (`approvals::MANAGE_HELD`);
//!   `ls` and `get` are refused (`apps_manage_off`: a held read would hand
//!   its answer to nobody). Members, App passes, API keys and other
//!   workspaces' agents never reach an App's data.
//! - **Where:** only inside the data folder. A path is relative
//!   (`var/answers.json`) or as the App sees it (`/data/var/answers.json`);
//!   any other absolute path, `..` and control characters are refused
//!   before anything runs. No step of a path may be a symlink: `get` and
//!   `ls` refuse one, `rm` and `mv` act on the link itself, never on what
//!   it points to.
//! - **How:** the same way `data put` writes. A walled App's data folder
//!   belongs to the `k2app` user, so the daemon can't read it: each verb
//!   is one fixed `sh -c` script run through the App's own exec (the
//!   guarded engine's `exec` verb, as the App, inside its wall, `/data` as
//!   root), the path passed as an argument, never spliced into the script.
//!   The App must be running. On the unwalled process engine the data
//!   folder is the daemon user's: `get` opens every step with
//!   `O_NOFOLLOW` (`openat` from the data folder down), the rest check
//!   every step with `lstat`.
//! - **Caps:** `get` moves a file in [`CHUNK`] pieces (the engine's exec
//!   answer is capped at 1 MiB) and refuses a file past [`MAX_GET`]; the
//!   CLI streams the pieces into a temp file and renames it into place.
//!   `ls` lists at most [`MAX_LS`] entries.
//! - **Disk guard:** none of these adds disk on the box (a `get` lands on
//!   the caller's side, `rm` frees, `mv` renames inside one folder), so
//!   none stops at the guard's lines: freeing an App's disk must work on
//!   a full disk.
//! - **Audit:** one line per call (`data_ls`, `data_get` on its first
//!   chunk, `data_rm`, `data_mv`).

use std::path::{Path, PathBuf};

use base64::Engine as _;
use k2_core::deploy::store::AppRow;
use k2_core::deploy::{self, codes};
use serde_json::{json, Value as J};

use super::data_put::Exec;
use super::routes::{actor, app_for, json_body, need_build, no, s, Refusal, Who, R};

pub const ROUTE_LS: &str = "/cli/app/data/ls";
pub const ROUTE_GET: &str = "/cli/app/data/get";
pub const ROUTE_RM: &str = "/cli/app/data/rm";
pub const ROUTE_MV: &str = "/cli/app/data/mv";
pub const ROUTES: &[&str] = &[ROUTE_LS, ROUTE_GET, ROUTE_RM, ROUTE_MV];
pub const POST_ROUTES: &[&str] = &[ROUTE_RM, ROUTE_MV];

/// Bytes of a file per `get` call (base64 + wrapping stays well under the
/// engine's 1 MiB exec answer).
pub const CHUNK: u64 = 512 * 1024;
/// Largest file `get` reads back.
pub const MAX_GET: u64 = 256 << 20;
/// Most entries one `ls` lists.
pub const MAX_LS: usize = 2000;
/// Longest path (the engine takes 4 KiB per argument).
const MAX_PATH: usize = 1024;
const EXEC_TIMEOUT: u64 = 120;

pub const NOT_FOUND: &str = "data_not_found";
pub const SYMLINK: &str = "data_symlink";
pub const IS_FOLDER: &str = "data_is_folder";
pub const NOT_FILE: &str = "data_not_file";
pub const EXISTS: &str = "data_exists";
pub const TOO_LARGE: &str = "data_too_large";
pub const FAILED: &str = "data_failed";

/// A path inside the data folder, as its parts. Relative, or under
/// `/data` (the App's own view); `.` parts drop, `..` refuses. `root_ok`:
/// the data folder itself may be named (ls).
pub fn check_path(raw: &str, what: &str, root_ok: bool) -> Result<Vec<String>, String> {
    let t = raw.trim();
    let t = if t == "/data" {
        ""
    } else if let Some(r) = t.strip_prefix("/data/") {
        r
    } else if t.starts_with('/') {
        return Err(format!(
            "{what} is a path inside the App's data folder: relative (var/answers.json) or under /data (/data/var/answers.json), never another absolute path"
        ));
    } else {
        t
    };
    if t.len() > MAX_PATH {
        return Err(format!("{what} is longer than {MAX_PATH} bytes"));
    }
    if t.chars().any(char::is_control) {
        return Err(format!("{what} has a control character in it"));
    }
    let parts: Vec<String> = t.split('/').filter(|p| !p.is_empty() && *p != ".").map(str::to_string).collect();
    if parts.iter().any(|p| p == "..") {
        return Err(format!("{what} can't climb out of the data folder ('..')"));
    }
    if parts.iter().any(|p| p.len() > 255) {
        return Err(format!("{what} has a name longer than 255 bytes"));
    }
    if parts.is_empty() && !root_ok {
        return Err(format!("{what} names the data folder itself: name a file or folder inside it"));
    }
    Ok(parts)
}

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn fail(code: &'static str, status: u16, hint: impl Into<String>) -> Refusal {
    no(code, status, hint)
}

fn shown(rel: &str) -> String {
    if rel.is_empty() {
        "the data folder".into()
    } else {
        rel.to_string()
    }
}

// ── the walled scripts (one line each: the engine refuses control
//    characters in an argument) ─────────────────────────────────────────

/// `k2w <rel>`: walk `$R/<rel>`'s parent folders, refusing any that is a
/// symlink; `$t` = the full path. `exit` inside a function ends the shell.
const WALK: &str = "k2w() { p=\"$R\"; rest=\"$1\"; while [ \"${rest#*/}\" != \"$rest\" ]; do c=\"${rest%%/*}\"; rest=\"${rest#*/}\"; p=\"$p/$c\"; if [ -L \"$p\" ]; then echo \"k2:symlink $c\"; exit 3; fi; done; t=\"$p/$rest\"; };";

/// `k2s <path>`: size (stat, else wc on a regular file) and mtime.
const SIZE: &str = "k2s() { s=$(stat -c %s -- \"$1\" 2>/dev/null) || s=$(wc -c < \"$1\" 2>/dev/null | tr -d ' '); m=$(stat -c %Y -- \"$1\" 2>/dev/null) || m=0; };";

/// `sh -c GET k2-data <root> <rel> <skip blocks> <block>`.
fn get_script() -> String {
    format!(
        "R=\"$1\"; {WALK} {SIZE} k2w \"$2\"; \
         if [ -L \"$t\" ]; then echo k2:symlink; exit 3; fi; \
         if [ ! -e \"$t\" ]; then echo k2:not_found; exit 3; fi; \
         if [ -d \"$t\" ]; then echo k2:is_dir; exit 3; fi; \
         if [ ! -f \"$t\" ]; then echo k2:not_file; exit 3; fi; \
         k2s \"$t\"; printf 'k2:ok %s %s\\n' \"$s\" \"$m\"; \
         if [ \"$s\" -gt $(($3 * $4)) ]; then dd if=\"$t\" bs=\"$4\" skip=\"$3\" count=1 2>/dev/null | base64; fi"
    )
}

/// `sh -c LS k2-data <root> <rel|.> <max>`: a header line, then
/// `type\tsize\tmtime\tname\0` records (`more` last when cut). The data
/// folder itself rides as [`ROOT_ARG`] (`.`), never `""`: the App engine
/// refuses an empty argv word (`exec_argv_ok`), so a root listing on a
/// guarded App answered `data_failed: App engine: argv` in 0.46.4.
/// `check_path` drops `.` parts, so `.` can't be a real relative path.
fn ls_script() -> String {
    format!(
        "R=\"$1\"; {WALK} {SIZE} \
         e() {{ f=\"$1\"; s=; m=; if [ -L \"$f\" ]; then y=link; elif [ -d \"$f\" ]; then y=dir; elif [ -f \"$f\" ]; then y=file; else y=other; fi; \
         if [ \"$y\" = file ]; then k2s \"$f\"; else m=$(stat -c %Y -- \"$f\" 2>/dev/null) || m=; fi; \
         printf '%s\\t%s\\t%s\\t%s\\000' \"$y\" \"$s\" \"$m\" \"$2\"; }}; \
         if [ \"$2\" = {ROOT_ARG} ] || [ -z \"$2\" ]; then t=\"$R\"; else k2w \"$2\"; fi; \
         if [ -L \"$t\" ]; then printf 'k2:ok link\\n'; e \"$t\" \"${{t##*/}}\"; exit 0; fi; \
         if [ ! -e \"$t\" ]; then echo k2:not_found; exit 3; fi; \
         if [ ! -d \"$t\" ]; then printf 'k2:ok file\\n'; e \"$t\" \"${{t##*/}}\"; exit 0; fi; \
         printf 'k2:ok dir\\n'; n=0; \
         for f in \"$t\"/* \"$t\"/.[!.]* \"$t\"/..?*; do \
         if [ ! -e \"$f\" ] && [ ! -L \"$f\" ]; then continue; fi; \
         if [ \"$n\" -ge \"$3\" ]; then printf 'more\\t\\t\\t\\000'; break; fi; \
         n=$((n + 1)); e \"$f\" \"${{f##*/}}\"; done"
    )
}

/// `sh -c RM k2-data <root> <rel> <0|1 recursive>`.
fn rm_script() -> String {
    format!(
        "R=\"$1\"; {WALK} k2w \"$2\"; \
         if [ -L \"$t\" ]; then rm -f -- \"$t\" || {{ echo k2:failed; exit 3; }}; echo k2:ok link; exit 0; fi; \
         if [ ! -e \"$t\" ]; then echo k2:not_found; exit 3; fi; \
         if [ -d \"$t\" ]; then if [ \"$3\" != 1 ]; then echo k2:is_dir; exit 3; fi; \
         rm -rf -- \"$t\" || {{ echo k2:failed; exit 3; }}; echo k2:ok dir; exit 0; fi; \
         rm -f -- \"$t\" || {{ echo k2:failed; exit 3; }}; echo k2:ok file"
    )
}

/// `sh -c MV k2-data <root> <from> <to> <0|1 force>`. `to` is the new
/// path exactly: never "into" a folder that is there.
fn mv_script() -> String {
    format!(
        "R=\"$1\"; {WALK} k2w \"$2\"; a=\"$t\"; k2w \"$3\"; b=\"$t\"; \
         if [ ! -e \"$a\" ] && [ ! -L \"$a\" ]; then echo k2:not_found; exit 3; fi; \
         r=0; if [ -L \"$b\" ]; then if [ \"$4\" != 1 ]; then echo k2:exists; exit 3; fi; rm -f -- \"$b\" || {{ echo k2:failed; exit 3; }}; r=1; \
         elif [ -d \"$b\" ]; then echo k2:exists_dir; exit 3; \
         elif [ -e \"$b\" ]; then if [ \"$4\" != 1 ]; then echo k2:exists; exit 3; fi; r=1; fi; \
         d=\"${{b%/*}}\"; if [ ! -d \"$d\" ]; then mkdir -p -- \"$d\" || {{ echo k2:failed; exit 3; }}; fi; \
         mv -f -- \"$a\" \"$b\" || {{ echo k2:failed; exit 3; }}; echo \"k2:ok $r\""
    )
}

/// A script's refusal (`k2:<code>` on its first `k2:` line, exit 3) or
/// failure, as the route's answer.
fn script_refusal(exit: i64, out: &str, rel: &str, other: &str) -> Refusal {
    let line = out.lines().find(|l| l.starts_with("k2:")).unwrap_or("");
    let word = line.trim_start_matches("k2:").split_whitespace().next().unwrap_or("");
    let rest = out.lines().filter(|l| !l.starts_with("k2:")).collect::<Vec<_>>().join(" ");
    let rest = rest.trim();
    let detail = if rest.is_empty() { String::new() } else { format!(" ({})", rest.chars().take(300).collect::<String>()) };
    match (exit, word) {
        (3, "symlink") => symlink_refusal(rel),
        (3, "not_found") => fail(NOT_FOUND, 404, format!("{} isn't in the App's data folder (k2 app data ls)", shown(rel))),
        (3, "is_dir") => fail(IS_FOLDER, 409, format!("{} is a folder: list it with k2 app data ls; remove it with --recursive", shown(rel))),
        (3, "not_file") => fail(NOT_FILE, 409, format!("{} isn't a regular file", shown(rel))),
        (3, "exists") => fail(EXISTS, 409, format!("{other} is there already: --force replaces it")),
        (3, "exists_dir") => fail(EXISTS, 409, format!("{other} is a folder that is there already: a move never lands inside it or replaces it")),
        _ => fail(FAILED, 409, format!("the App's exec answered exit {exit}{detail}")),
    }
}

fn symlink_refusal(rel: &str) -> Refusal {
    fail(SYMLINK, 403, format!("{} goes through a symlink: k2 app data never follows one (it could point outside the data folder)", shown(rel)))
}

/// The data folder itself on an ls script's argv (see [`ls_script`]).
const ROOT_ARG: &str = ".";

/// Every script's argv passes the App engine's own check first
/// (`exec_argv_ok`: no empty word, no control characters, at most 64): a
/// word the engine would refuse fails here, loudly and before any exec,
/// never as the engine's bare "App engine: argv".
fn run(exec: Exec<'_>, argv: Vec<String>) -> R<(i64, String)> {
    if !k2_root_proto::engine::exec_argv_ok(&argv) {
        let empty = argv.iter().skip(3).position(String::is_empty);
        return Err(fail(
            FAILED,
            409,
            match empty {
                // skip(3): index i is the script's ${i} ($0 = k2-data).
                Some(i) => format!("k2 app data built an empty argument (${i}) for the App's exec; nothing ran"),
                None => "k2 app data built an argument the App's exec refuses; nothing ran".to_string(),
            },
        ));
    }
    exec(&argv).map_err(|e| fail(FAILED, 409, e))
}

// ── one file's chunk ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub size: u64,
    pub mtime: i64,
    pub bytes: Vec<u8>,
}

/// Bytes `[offset, offset + CHUNK)` of `root/rel` through the App's exec.
pub(crate) fn get_wall(root: &str, rel: &str, offset: u64, exec: Exec<'_>) -> R<Chunk> {
    let (code, out) = run(exec, sv(&["sh", "-c", &get_script(), "k2-data", root, rel, &(offset / CHUNK).to_string(), &CHUNK.to_string()]))?;
    if code != 0 {
        return Err(script_refusal(code, &out, rel, ""));
    }
    let (head, body) = out.split_once('\n').unwrap_or((out.as_str(), ""));
    let mut it = head.strip_prefix("k2:ok ").unwrap_or("").split_whitespace();
    let size = it.next().and_then(|x| x.parse::<u64>().ok());
    let mtime = it.next().and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    let Some(size) = size else {
        return Err(fail(FAILED, 409, format!("the App's exec gave no size for {rel}: {}", head.chars().take(200).collect::<String>())));
    };
    let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| fail(FAILED, 409, format!("the App's exec answered unreadable bytes for {rel} ({e})")))?;
    let want = size.saturating_sub(offset).min(CHUNK);
    if bytes.len() as u64 != want {
        return Err(fail(FAILED, 409, format!("{rel}: read {} bytes at {offset}, expected {want} (did the file change?)", bytes.len())));
    }
    Ok(Chunk { size, mtime, bytes })
}

/// `lstat` every step under `root` (parents must be real folders; the last
/// is returned as is, `None` = missing).
fn lwalk(root: &Path, parts: &[String]) -> R<(PathBuf, Option<std::fs::Metadata>)> {
    let rel = parts.join("/");
    let full: PathBuf = parts.iter().fold(root.to_path_buf(), |p, c| p.join(c));
    let mut p = root.to_path_buf();
    for (i, c) in parts.iter().enumerate() {
        p.push(c);
        let last = i + 1 == parts.len();
        match std::fs::symlink_metadata(&p) {
            Ok(m) if !last && m.file_type().is_symlink() => return Err(symlink_refusal(&rel)),
            Ok(m) if !last && !m.is_dir() => return Err(fail(NOT_FOUND, 404, format!("{rel}: {c} isn't a folder"))),
            Ok(m) if last => return Ok((p, Some(m))),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((full, None)),
            Err(e) => return Err(fail(FAILED, 409, format!("{rel}: {e}"))),
        }
    }
    let m = std::fs::symlink_metadata(&p).ok();
    Ok((p, m))
}

/// Open `root/parts` for reading with no symlink at any step: `openat`
/// from the data folder down, `O_NOFOLLOW` each time.
#[cfg(unix)]
fn open_nofollow(root: &Path, parts: &[String]) -> std::io::Result<std::fs::File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::OpenOptionsExt;
    let mut dir = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(root)?;
    for (i, c) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let name = CString::new(c.as_bytes()).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | if last { libc::O_NONBLOCK } else { libc::O_DIRECTORY };
        // SAFETY: `dir` is an open folder; `name` is NUL-terminated.
        let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` was just opened and is owned by nothing else.
        let f = unsafe { std::fs::File::from_raw_fd(fd) };
        if last {
            return Ok(f);
        }
        dir = f;
    }
    Err(std::io::Error::from(std::io::ErrorKind::InvalidInput))
}

#[cfg(not(unix))]
fn open_nofollow(root: &Path, parts: &[String]) -> std::io::Result<std::fs::File> {
    let mut p = root.to_path_buf();
    for c in parts {
        p.push(c);
        if std::fs::symlink_metadata(&p)?.file_type().is_symlink() {
            return Err(std::io::Error::other("symlink"));
        }
    }
    std::fs::File::open(p)
}

/// The unwalled engine's data folder is the daemon user's: read it.
pub(crate) fn get_local(root: &Path, parts: &[String], offset: u64) -> R<Chunk> {
    use std::io::{Read, Seek, SeekFrom};
    let rel = parts.join("/");
    match lwalk(root, parts)?.1 {
        None => return Err(fail(NOT_FOUND, 404, format!("{} isn't in the App's data folder (k2 app data ls)", shown(&rel)))),
        Some(m) if m.file_type().is_symlink() => return Err(symlink_refusal(&rel)),
        Some(m) if m.is_dir() => return Err(script_refusal(3, "k2:is_dir", &rel, "")),
        Some(m) if !m.is_file() => return Err(script_refusal(3, "k2:not_file", &rel, "")),
        Some(_) => {}
    }
    let mut f = open_nofollow(root, parts).map_err(|e| {
        if e.raw_os_error() == Some(libc_eloop()) {
            symlink_refusal(&rel)
        } else {
            fail(FAILED, 409, format!("{rel}: {e}"))
        }
    })?;
    let m = f.metadata().map_err(|e| fail(FAILED, 409, format!("{rel}: {e}")))?;
    if !m.is_file() {
        return Err(script_refusal(3, "k2:not_file", &rel, ""));
    }
    let size = m.len();
    let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut bytes = Vec::new();
    if offset < size {
        f.seek(SeekFrom::Start(offset)).map_err(|e| fail(FAILED, 409, format!("{rel}: {e}")))?;
        f.take(CHUNK).read_to_end(&mut bytes).map_err(|e| fail(FAILED, 409, format!("{rel}: {e}")))?;
    }
    Ok(Chunk { size, mtime, bytes })
}

#[cfg(unix)]
fn libc_eloop() -> i32 {
    libc::ELOOP
}

#[cfg(not(unix))]
fn libc_eloop() -> i32 {
    -1
}

// ── a listing ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// What the path is: `dir`, `file` or `link`.
    pub kind: String,
    pub entries: Vec<J>,
    pub truncated: bool,
}

fn entry(name: &str, ty: &str, size: Option<u64>, mtime: Option<i64>) -> J {
    json!({ "name": name, "type": ty, "size": size, "mtime": mtime })
}

pub(crate) fn ls_wall(root: &str, rel: &str, exec: Exec<'_>) -> R<Listing> {
    let wire_rel = if rel.is_empty() { ROOT_ARG } else { rel };
    let (code, out) = run(exec, sv(&["sh", "-c", &ls_script(), "k2-data", root, wire_rel, &MAX_LS.to_string()]))?;
    if code != 0 {
        return Err(script_refusal(code, &out, rel, ""));
    }
    let (head, body) = out.split_once('\n').unwrap_or((out.as_str(), ""));
    let kind = head.strip_prefix("k2:ok ").map(str::trim).unwrap_or("");
    if !matches!(kind, "dir" | "file" | "link") {
        return Err(fail(FAILED, 409, format!("the App's exec answered no listing: {}", head.chars().take(200).collect::<String>())));
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    for rec in body.split('\0').filter(|r| !r.is_empty()) {
        let mut f = rec.splitn(4, '\t');
        let (ty, size, mtime, name) = (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""), f.next());
        if ty == "more" {
            truncated = true;
            continue;
        }
        let Some(name) = name else {
            return Err(fail(FAILED, 409, format!("the App's exec answered a broken listing line: {}", rec.chars().take(200).collect::<String>())));
        };
        entries.push(entry(name, ty, size.parse().ok(), mtime.parse().ok()));
    }
    // The shell's glob order depends on its locale: sort like ls_local.
    entries.sort_by(|a, b| a["name"].as_str().unwrap_or("").as_bytes().cmp(b["name"].as_str().unwrap_or("").as_bytes()));
    Ok(Listing { kind: kind.to_string(), entries, truncated })
}

fn local_entry(name: &str, m: &std::fs::Metadata) -> J {
    let ft = m.file_type();
    let ty = if ft.is_symlink() {
        "link"
    } else if ft.is_dir() {
        "dir"
    } else if ft.is_file() {
        "file"
    } else {
        "other"
    };
    let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
    entry(name, ty, (ty == "file").then_some(m.len()), mtime)
}

pub(crate) fn ls_local(root: &Path, parts: &[String]) -> R<Listing> {
    let rel = parts.join("/");
    let (p, m) = if parts.is_empty() {
        (root.to_path_buf(), std::fs::symlink_metadata(root).ok())
    } else {
        lwalk(root, parts)?
    };
    let Some(m) = m else {
        return Err(fail(NOT_FOUND, 404, format!("{} isn't in the App's data folder (k2 app data ls)", shown(&rel))));
    };
    let name = parts.last().cloned().unwrap_or_default();
    if m.file_type().is_symlink() {
        return Ok(Listing { kind: "link".into(), entries: vec![local_entry(&name, &m)], truncated: false });
    }
    if !m.is_dir() {
        return Ok(Listing { kind: "file".into(), entries: vec![local_entry(&name, &m)], truncated: false });
    }
    let mut rd: Vec<_> = std::fs::read_dir(&p).map_err(|e| fail(FAILED, 409, format!("{}: {e}", shown(&rel))))?.flatten().collect();
    rd.sort_by_key(|e| e.file_name());
    let truncated = rd.len() > MAX_LS;
    let entries = rd
        .iter()
        .take(MAX_LS)
        .filter_map(|e| {
            let m = std::fs::symlink_metadata(e.path()).ok()?;
            Some(local_entry(&e.file_name().to_string_lossy(), &m))
        })
        .collect();
    Ok(Listing { kind: "dir".into(), entries, truncated })
}

// ── rm and mv ────────────────────────────────────────────────────────

/// What `rm` removed: `file`, `dir` or `link`.
pub(crate) fn rm_wall(root: &str, rel: &str, recursive: bool, exec: Exec<'_>) -> R<String> {
    let (code, out) = run(exec, sv(&["sh", "-c", &rm_script(), "k2-data", root, rel, if recursive { "1" } else { "0" }]))?;
    if code != 0 {
        return Err(script_refusal(code, &out, rel, ""));
    }
    let kind = out.lines().find_map(|l| l.strip_prefix("k2:ok ")).map(str::trim).unwrap_or("");
    if !matches!(kind, "file" | "dir" | "link") {
        return Err(fail(FAILED, 409, format!("the App's exec didn't say what it removed: {}", out.chars().take(200).collect::<String>())));
    }
    Ok(kind.to_string())
}

pub(crate) fn rm_local(root: &Path, parts: &[String], recursive: bool) -> R<String> {
    let rel = parts.join("/");
    let (p, m) = lwalk(root, parts)?;
    let m = m.ok_or_else(|| fail(NOT_FOUND, 404, format!("{rel} isn't in the App's data folder (k2 app data ls)")))?;
    let io = |e: std::io::Error| fail(FAILED, 409, format!("{rel}: {e}"));
    if m.file_type().is_symlink() {
        std::fs::remove_file(&p).map_err(io)?;
        return Ok("link".into());
    }
    if m.is_dir() {
        if !recursive {
            return Err(script_refusal(3, "k2:is_dir", &rel, ""));
        }
        // std's remove_dir_all never follows a symlink inside the tree.
        std::fs::remove_dir_all(&p).map_err(io)?;
        return Ok("dir".into());
    }
    std::fs::remove_file(&p).map_err(io)?;
    Ok("file".into())
}

/// `true` = it replaced something at `to`.
pub(crate) fn mv_wall(root: &str, from: &str, to: &str, force: bool, exec: Exec<'_>) -> R<bool> {
    let (code, out) = run(exec, sv(&["sh", "-c", &mv_script(), "k2-data", root, from, to, if force { "1" } else { "0" }]))?;
    if code != 0 {
        return Err(script_refusal(code, &out, from, to));
    }
    match out.lines().find_map(|l| l.strip_prefix("k2:ok ")).map(str::trim) {
        Some("1") => Ok(true),
        Some("0") => Ok(false),
        _ => Err(fail(FAILED, 409, format!("the App's exec didn't confirm the move: {}", out.chars().take(200).collect::<String>()))),
    }
}

pub(crate) fn mv_local(root: &Path, from: &[String], to: &[String], force: bool) -> R<bool> {
    let (fr, tr) = (from.join("/"), to.join("/"));
    let (a, am) = lwalk(root, from)?;
    if am.is_none() {
        return Err(fail(NOT_FOUND, 404, format!("{fr} isn't in the App's data folder (k2 app data ls)")));
    }
    let (b, bm) = lwalk(root, to)?;
    let io = |e: std::io::Error| fail(FAILED, 409, format!("{fr} → {tr}: {e}"));
    let replaced = match bm {
        None => false,
        Some(m) if m.is_dir() && !m.file_type().is_symlink() => return Err(script_refusal(3, "k2:exists_dir", &fr, &tr)),
        Some(_) if !force => return Err(script_refusal(3, "k2:exists", &fr, &tr)),
        Some(m) => {
            if m.file_type().is_symlink() {
                std::fs::remove_file(&b).map_err(io)?;
            }
            true
        }
    };
    if let Some(d) = b.parent() {
        if !d.is_dir() {
            std::fs::create_dir_all(d).map_err(io)?;
        }
    }
    std::fs::rename(&a, &b).map_err(io)?;
    Ok(replaced)
}

// ── routes ───────────────────────────────────────────────────────────

/// Where an App's data folder is, and how to reach it.
enum Data {
    /// The unwalled process engine: a folder of the daemon user's.
    Local(PathBuf),
    /// A walled App: `/data` through its own exec.
    Wall(String, String),
}

fn data_of(app: &AppRow) -> R<Data> {
    if super::supervisor::runs_unwalled(app) {
        Ok(Data::Local(deploy::app_dir(&app.app_id).join("data")))
    } else {
        Ok(Data::Wall(app.app_id.clone(), super::exec::live_guarded(app)?))
    }
}

fn with_exec<T>(app_id: &str, dep: &str, f: impl FnOnce(Exec<'_>) -> R<T>) -> R<T> {
    let exec = |argv: &[String]| -> Result<(i64, String), String> {
        let r = super::guarded::exec_once(app_id, dep, argv, EXEC_TIMEOUT).map_err(|e| e.message)?;
        if r["timedOut"].as_bool() == Some(true) {
            return Err(format!("the App's exec timed out after {EXEC_TIMEOUT}s"));
        }
        if r["bytes"].as_u64().is_some_and(|b| b as usize > k2_root_proto::engine::MAX_OUTPUT) {
            return Err("the App's exec answer was cut off at 1 MiB".into());
        }
        Ok((r["exitCode"].as_i64().unwrap_or(-1), r["output"].as_str().unwrap_or("").to_string()))
    };
    f(&exec)
}

fn at(data: &Data, rel: &str) -> String {
    match data {
        Data::Local(root) => root.join(rel).to_string_lossy().trim_end_matches('/').to_string(),
        Data::Wall(..) if rel.is_empty() => "/data".into(),
        Data::Wall(..) => format!("/data/{rel}"),
    }
}

fn audit(app: &AppRow, who: &Who, event: &str, payload: J) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, &app.app_id, event, &actor(who), None, &payload) {
        k2_core::log_debug!("[apps] audit {} {event}: {e}", app.app_id);
    }
}

fn bad(e: String) -> Refusal {
    no(codes::BAD_REQUEST, 400, e)
}

/// GET `/cli/app/data/ls ?app&path?`.
fn ls(who: &Who, params: &std::collections::HashMap<String, String>) -> R<J> {
    need_build(who)?;
    let app = app_for(who, params.get("app").map(String::as_str).map(str::trim).filter(|x| !x.is_empty()))?;
    let parts = check_path(params.get("path").map(String::as_str).unwrap_or(""), "the path", true).map_err(bad)?;
    let rel = parts.join("/");
    let data = data_of(&app)?;
    let l = match &data {
        Data::Local(root) => ls_local(root, &parts)?,
        Data::Wall(id, dep) => with_exec(id, dep, |x| ls_wall("/data", &rel, x))?,
    };
    audit(&app, who, "data_ls", json!({ "path": at(&data, &rel), "entries": l.entries.len() }));
    Ok(json!({ "ok": true, "app": app.app_id, "path": rel, "at": at(&data, &rel), "kind": l.kind, "entries": l.entries, "truncated": l.truncated, "max": MAX_LS }))
}

/// GET `/cli/app/data/get ?app&path&offset?` → one chunk.
fn get(who: &Who, params: &std::collections::HashMap<String, String>) -> R<J> {
    need_build(who)?;
    let app = app_for(who, params.get("app").map(String::as_str).map(str::trim).filter(|x| !x.is_empty()))?;
    let raw = params.get("path").map(String::as_str).unwrap_or("");
    if raw.trim().is_empty() {
        return Err(bad("path is required (a file in the App's data folder)".into()));
    }
    let parts = check_path(raw, "the path", false).map_err(bad)?;
    let rel = parts.join("/");
    let offset: u64 = match params.get("offset").map(|o| o.trim()).filter(|o| !o.is_empty()) {
        None => 0,
        Some(o) => o.parse().map_err(|_| bad(format!("offset is a byte count (a multiple of {CHUNK})")))?,
    };
    if offset % CHUNK != 0 {
        return Err(bad(format!("offset is a multiple of {CHUNK} (the chunk size)")));
    }
    let data = data_of(&app)?;
    let c = match &data {
        Data::Local(root) => get_local(root, &parts, offset)?,
        Data::Wall(id, dep) => with_exec(id, dep, |x| get_wall("/data", &rel, offset, x))?,
    };
    if c.size > MAX_GET {
        return Err(fail(
            TOO_LARGE,
            413,
            format!(
                "{rel} is {} MiB; k2 app data get reads back {} MiB at most: split or compress it inside the App first (k2 app exec {} -- …)",
                c.size >> 20,
                MAX_GET >> 20,
                app.app_id
            ),
        ));
    }
    if offset > 0 && offset >= c.size {
        return Err(bad(format!("offset {offset} is past the end of {rel} ({} bytes)", c.size)));
    }
    if offset == 0 {
        audit(&app, who, "data_get", json!({ "path": at(&data, &rel), "bytes": c.size }));
    }
    let len = c.bytes.len() as u64;
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "path": rel,
        "at": at(&data, &rel),
        "size": c.size,
        "mtime": c.mtime,
        "offset": offset,
        "length": len,
        "eof": offset + len >= c.size,
        "chunk": CHUNK,
        "max": MAX_GET,
        "data": base64::engine::general_purpose::STANDARD.encode(&c.bytes),
    }))
}

/// POST `/cli/app/data/rm {app, path, recursive?}`.
fn rm(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let raw = s(&v, "path").ok_or_else(|| bad("path is required (a file or folder in the App's data folder)".into()))?;
    let parts = check_path(raw, "the path", false).map_err(bad)?;
    let rel = parts.join("/");
    let recursive = super::routes::b(&v, "recursive");
    let data = data_of(&app)?;
    let removed = match &data {
        Data::Local(root) => rm_local(root, &parts, recursive)?,
        Data::Wall(id, dep) => with_exec(id, dep, |x| rm_wall("/data", &rel, recursive, x))?,
    };
    audit(&app, who, "data_rm", json!({ "path": at(&data, &rel), "removed": removed, "recursive": recursive }));
    Ok(json!({ "ok": true, "app": app.app_id, "path": rel, "at": at(&data, &rel), "removed": removed }))
}

/// POST `/cli/app/data/mv {app, from, to, force?}`.
fn mv(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let from = check_path(s(&v, "from").ok_or_else(|| bad("from is required".into()))?, "from", false).map_err(bad)?;
    let to = check_path(s(&v, "to").ok_or_else(|| bad("to is required".into()))?, "to", false).map_err(bad)?;
    let (fr, tr) = (from.join("/"), to.join("/"));
    if fr == tr {
        return Err(bad(format!("from and to are the same path ({fr})")));
    }
    if tr.starts_with(&format!("{fr}/")) {
        return Err(bad(format!("{tr} is inside {fr}: a folder can't move into itself")));
    }
    let force = super::routes::b(&v, "force");
    let data = data_of(&app)?;
    let replaced = match &data {
        Data::Local(root) => mv_local(root, &from, &to, force)?,
        Data::Wall(id, dep) => with_exec(id, dep, |x| mv_wall("/data", &fr, &tr, force, x))?,
    };
    audit(&app, who, "data_mv", json!({ "from": at(&data, &fr), "to": at(&data, &tr), "replaced": replaced }));
    Ok(json!({ "ok": true, "app": app.app_id, "from": fr, "to": tr, "at": at(&data, &tr), "replaced": replaced }))
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &std::collections::HashMap<String, String>) -> R<J> {
    match path {
        ROUTE_LS => ls(who, params),
        ROUTE_GET => get(who, params),
        ROUTE_RM => rm(who, body),
        ROUTE_MV => mv(who, body),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Scratch;

    /// Run each command on this machine: the scripts are exactly what runs
    /// inside the App's wall, against a scratch "data" folder.
    fn local(argv: &[String]) -> Result<(i64, String), String> {
        for a in argv {
            assert!(k2_root_proto::engine::value_ok(a, 4096), "the engine would refuse this argument: {a:?}");
        }
        // The guarded exec's own contract (0.46.5: a root ls sent "").
        assert!(k2_root_proto::engine::exec_argv_ok(argv), "the engine would refuse this argv: {argv:?}");
        let o = std::process::Command::new(&argv[0]).args(&argv[1..]).output().map_err(|e| e.to_string())?;
        let mut out = o.stdout;
        out.extend_from_slice(&o.stderr);
        Ok((o.status.code().unwrap_or(-1) as i64, String::from_utf8_lossy(&out).to_string()))
    }

    fn code(r: Refusal) -> (&'static str, u16) {
        (r.code, r.status)
    }

    #[test]
    fn paths_stay_inside_the_data_folder() {
        assert_eq!(check_path("var/answers.json", "p", false).unwrap(), vec!["var", "answers.json"]);
        assert_eq!(check_path("/data/var/answers.json", "p", false).unwrap(), vec!["var", "answers.json"]);
        assert_eq!(check_path("./var//files/", "p", false).unwrap(), vec!["var", "files"]);
        assert!(check_path("/data", "p", true).unwrap().is_empty());
        assert!(check_path("", "p", true).unwrap().is_empty());
        for bad in ["", "/data", "/", "."] {
            assert!(check_path(bad, "p", false).is_err(), "{bad:?}: the folder itself");
        }
        for bad in ["/etc/passwd", "/datax/a", "../x", "a/../../b", "var/..", "a\nb", "a\tb", "/data/../etc"] {
            assert!(check_path(bad, "p", true).is_err(), "{bad:?}");
        }
        assert!(check_path(&"a/".repeat(600), "p", true).is_err(), "too long");
        assert!(check_path(&"n".repeat(256), "p", true).is_err(), "a name past 255");
    }

    #[test]
    fn scripts_are_one_line_for_the_engine() {
        for s in [get_script(), ls_script(), rm_script(), mv_script()] {
            assert!(k2_root_proto::engine::value_ok(&s, 4096), "{s}");
        }
    }

    #[cfg(unix)]
    fn tree(d: &Scratch) -> (PathBuf, PathBuf) {
        let root = d.path().join("data");
        std::fs::create_dir_all(root.join("var/files/sub")).unwrap();
        std::fs::write(root.join("var/answers.json"), br#"{"q1":"yes"}"#).unwrap();
        std::fs::write(root.join("var/files/a.pdf"), vec![1u8; 10]).unwrap();
        std::fs::write(root.join("var/files/sub/b.txt"), "b").unwrap();
        std::fs::write(root.join(".hidden"), "h").unwrap();
        let outside = d.path().join("secret.txt");
        std::fs::write(&outside, "do not read").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("var/leak")).unwrap();
        std::os::unix::fs::symlink(d.path(), root.join("up")).unwrap();
        (root, outside)
    }

    /// Every verb, both ways (the wall's scripts and the unwalled engine's
    /// code), answers the same.
    #[cfg(unix)]
    #[test]
    fn get_reads_files_and_refuses_symlinks_both_ways() {
        let d = Scratch::new();
        let (root, _) = tree(&d);
        let r = root.to_string_lossy().to_string();
        let parts = |p: &str| check_path(p, "p", false).unwrap();
        let w = get_wall(&r, "var/answers.json", 0, &local).unwrap();
        let l = get_local(&root, &parts("var/answers.json"), 0).unwrap();
        assert_eq!(w.bytes, br#"{"q1":"yes"}"#);
        assert_eq!((w.size, w.bytes.clone()), (l.size, l.bytes));
        assert!(l.mtime > 0);
        if cfg!(target_os = "linux") {
            // GNU/busybox stat; BSD stat has no -c (the size falls back to wc).
            assert_eq!(w.mtime, l.mtime);
        }
        // Empty files come back empty.
        std::fs::write(root.join("empty"), "").unwrap();
        let e = get_wall(&r, "empty", 0, &local).unwrap();
        assert_eq!((e.size, e.bytes.len()), (0, 0));
        assert_eq!(get_local(&root, &parts("empty"), 0).unwrap().size, 0);
        // Symlinks: the last step, or any step on the way.
        for p in ["var/leak", "up/secret.txt"] {
            assert_eq!(code(get_wall(&r, p, 0, &local).unwrap_err()), (SYMLINK, 403), "{p}");
            assert_eq!(code(get_local(&root, &parts(p), 0).unwrap_err()), (SYMLINK, 403), "{p}");
        }
        // Missing, a folder.
        assert_eq!(code(get_wall(&r, "nope", 0, &local).unwrap_err()), (NOT_FOUND, 404));
        assert_eq!(code(get_local(&root, &parts("nope"), 0).unwrap_err()), (NOT_FOUND, 404));
        assert_eq!(code(get_wall(&r, "var/files", 0, &local).unwrap_err()), (IS_FOLDER, 409));
        assert_eq!(code(get_local(&root, &parts("var/files"), 0).unwrap_err()), (IS_FOLDER, 409));
    }

    /// A file past one chunk comes back in pieces that add up, byte for
    /// byte, both ways.
    #[cfg(unix)]
    #[test]
    fn get_moves_big_files_in_chunks() {
        let d = Scratch::new();
        let root = d.path().join("data");
        std::fs::create_dir_all(&root).unwrap();
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let noise: Vec<u8> = (0..(CHUNK * 2 + 12345))
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        std::fs::write(root.join("big.bin"), &noise).unwrap();
        let r = root.to_string_lossy().to_string();
        let (mut wall, mut loc) = (Vec::new(), Vec::new());
        let mut off = 0;
        while off < noise.len() as u64 {
            let c = get_wall(&r, "big.bin", off, &local).unwrap();
            assert_eq!(c.size, noise.len() as u64);
            assert!(c.bytes.len() as u64 <= CHUNK);
            wall.extend_from_slice(&c.bytes);
            loc.extend_from_slice(&get_local(&root, &["big.bin".to_string()], off).unwrap().bytes);
            off += CHUNK;
        }
        assert_eq!(wall, noise);
        assert_eq!(loc, noise);
    }

    #[cfg(unix)]
    #[test]
    fn ls_lists_without_following_links_both_ways() {
        let d = Scratch::new();
        let (root, _) = tree(&d);
        let r = root.to_string_lossy().to_string();
        let names = |l: &Listing| l.entries.iter().map(|e| format!("{}:{}", e["name"].as_str().unwrap(), e["type"].as_str().unwrap())).collect::<Vec<_>>();
        let w = ls_wall(&r, "", &local).unwrap();
        let l = ls_local(&root, &[]).unwrap();
        assert_eq!(w.kind, "dir");
        assert_eq!(names(&w), vec![".hidden:file", "up:link", "var:dir"], "{w:?}");
        assert_eq!(names(&w), names(&l));
        let w = ls_wall(&r, "var", &local).unwrap();
        assert_eq!(names(&w), vec!["answers.json:file", "files:dir", "leak:link"]);
        assert_eq!(w.entries[0]["size"], 12);
        let sizes = |l: &Listing| l.entries.iter().map(|e| e["size"].clone()).collect::<Vec<_>>();
        let lv = ls_local(&root, &["var".into()]).unwrap();
        assert_eq!(sizes(&w), sizes(&lv));
        if cfg!(target_os = "linux") {
            assert_eq!(w.entries, lv.entries, "mtimes too");
        }
        // One file, one link (the link itself, never its target).
        let f = ls_wall(&r, "var/answers.json", &local).unwrap();
        assert_eq!((f.kind.as_str(), names(&f)), ("file", vec!["answers.json:file".to_string()]));
        let k = ls_wall(&r, "var/leak", &local).unwrap();
        assert_eq!((k.kind.as_str(), names(&k)), ("link", vec!["leak:link".to_string()]));
        assert_eq!(ls_local(&root, &["var".into(), "leak".into()]).unwrap().kind, "link");
        // Through a link: refused.
        assert_eq!(code(ls_wall(&r, "up/data", &local).unwrap_err()), (SYMLINK, 403));
        assert_eq!(code(ls_local(&root, &["up".into(), "data".into()]).unwrap_err()), (SYMLINK, 403));
        assert_eq!(code(ls_wall(&r, "nope", &local).unwrap_err()), (NOT_FOUND, 404));
        // Names with spaces and tabs survive.
        std::fs::write(root.join("var/files/odd name\twith tab"), "x").unwrap();
        let w = ls_wall(&r, "var/files", &local).unwrap();
        assert!(names(&w).contains(&"odd name\twith tab:file".to_string()), "{w:?}");
    }

    #[cfg(unix)]
    #[test]
    fn ls_caps_the_entries() {
        let d = Scratch::new();
        let root = d.path().join("data");
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..(MAX_LS + 3) {
            std::fs::write(root.join(format!("f{i:05}")), "").unwrap();
        }
        let w = ls_wall(&root.to_string_lossy(), "", &local).unwrap();
        assert!(w.truncated);
        assert_eq!(w.entries.len(), MAX_LS);
        let l = ls_local(&root, &[]).unwrap();
        assert!(l.truncated);
        assert_eq!(l.entries.len(), MAX_LS);
    }

    #[cfg(unix)]
    #[test]
    fn rm_refuses_folders_unless_recursive_and_never_follows_links() {
        for wall in [true, false] {
            let d = Scratch::new();
            let (root, outside) = tree(&d);
            let r = root.to_string_lossy().to_string();
            let rm = |p: &str, rec: bool| -> R<String> {
                if wall {
                    rm_wall(&r, p, rec, &local)
                } else {
                    rm_local(&root, &check_path(p, "p", false).unwrap(), rec)
                }
            };
            assert_eq!(code(rm("var/files", false).unwrap_err()), (IS_FOLDER, 409), "wall={wall}");
            assert!(root.join("var/files/a.pdf").exists());
            assert_eq!(rm("var/files", true).unwrap(), "dir");
            assert!(!root.join("var/files").exists());
            assert_eq!(rm("var/answers.json", false).unwrap(), "file");
            assert!(!root.join("var/answers.json").exists());
            // The link goes; what it points at stays.
            assert_eq!(rm("var/leak", false).unwrap(), "link");
            assert!(outside.exists());
            // A symlinked folder on the way: refused, nothing outside touched.
            assert_eq!(code(rm("up/secret.txt", false).unwrap_err()), (SYMLINK, 403));
            assert!(outside.exists());
            assert_eq!(rm("up", true).unwrap(), "link", "a link to a folder is removed as a link");
            assert!(d.path().join("secret.txt").exists());
            assert_eq!(code(rm("nope", false).unwrap_err()), (NOT_FOUND, 404));
        }
    }

    #[cfg(unix)]
    #[test]
    fn mv_renames_and_never_overwrites_without_force() {
        for wall in [true, false] {
            let d = Scratch::new();
            let (root, outside) = tree(&d);
            let r = root.to_string_lossy().to_string();
            let mv = |a: &str, b: &str, force: bool| -> R<bool> {
                if wall {
                    mv_wall(&r, a, b, force, &local)
                } else {
                    mv_local(&root, &check_path(a, "p", false).unwrap(), &check_path(b, "p", false).unwrap(), force)
                }
            };
            // A rename into a folder that isn't there yet.
            assert!(!mv("var/answers.json", "archive/2026/answers.json", false).unwrap(), "wall={wall}");
            assert_eq!(std::fs::read(root.join("archive/2026/answers.json")).unwrap(), br#"{"q1":"yes"}"#);
            // Onto a file: refused, then --force.
            std::fs::write(root.join("x"), "x").unwrap();
            std::fs::write(root.join("y"), "y").unwrap();
            assert_eq!(code(mv("x", "y", false).unwrap_err()), (EXISTS, 409));
            assert_eq!(std::fs::read_to_string(root.join("y")).unwrap(), "y");
            assert!(mv("x", "y", true).unwrap());
            assert_eq!(std::fs::read_to_string(root.join("y")).unwrap(), "x");
            assert!(!root.join("x").exists());
            // Onto a folder: never, not even with --force (no "into").
            assert_eq!(code(mv("y", "var/files", true).unwrap_err()), (EXISTS, 409));
            assert!(root.join("y").exists());
            // Onto a link with --force: the link is replaced, its target untouched.
            assert!(mv("y", "var/leak", true).unwrap());
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), "do not read");
            assert_eq!(std::fs::read_to_string(root.join("var/leak")).unwrap(), "x");
            // Through a symlinked folder, either side: refused.
            assert_eq!(code(mv("var/leak", "up/stolen", false).unwrap_err()), (SYMLINK, 403));
            assert!(!d.path().join("stolen").exists());
            assert_eq!(code(mv("up/secret.txt", "mine", false).unwrap_err()), (SYMLINK, 403));
            assert!(outside.exists());
            assert_eq!(code(mv("nope", "z", false).unwrap_err()), (NOT_FOUND, 404));
            // A folder moves whole.
            assert!(!mv("var/files", "uploads", false).unwrap());
            assert_eq!(std::fs::read_to_string(root.join("uploads/sub/b.txt")).unwrap(), "b");
        }
    }

    /// 0.46.5 (Agent NSI Vanilla): `k2 app data ls <app>` on a guarded App
    /// answered `data_failed: App engine: argv`: the root rode as `""` and
    /// the engine refuses an empty argv word. An exec that refuses exactly
    /// like the engine now lists the root (and every path) fine.
    #[cfg(unix)]
    #[test]
    fn root_ls_passes_the_engines_argv_check() {
        let d = Scratch::new();
        let (root, _) = tree(&d);
        let r = root.to_string_lossy().to_string();
        let seen = std::cell::RefCell::new(Vec::<Vec<String>>::new());
        let engine = |argv: &[String]| -> Result<(i64, String), String> {
            seen.borrow_mut().push(argv.to_vec());
            if !k2_root_proto::engine::exec_argv_ok(argv) {
                return Err("App engine: argv".into());
            }
            local(argv)
        };
        let w = ls_wall(&r, "", &engine).unwrap_or_else(|e| panic!("root ls refused: {} {}", e.code, e.hint));
        assert_eq!(w.kind, "dir");
        let names: Vec<_> = w.entries.iter().map(|e| e["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, vec![".hidden", "up", "var"], "{w:?}");
        assert_eq!(seen.borrow()[0][5], ROOT_ARG, "the root rides as {ROOT_ARG}");
        // What the route sends for `k2 app data ls <app>` and `ls /data`.
        for raw in ["", "/data", ".", "/data/"] {
            let parts = check_path(raw, "p", true).unwrap();
            assert!(parts.is_empty(), "{raw:?}");
            assert_eq!(ls_wall(&r, &parts.join("/"), &engine).unwrap().kind, "dir", "{raw:?}");
        }
        assert_eq!(ls_wall(&r, "var", &engine).unwrap().entries.len(), 3);
    }

    /// No data_files script ever hands the engine an empty argv word: each
    /// wall function, for every path the routes can send it, builds an argv
    /// that passes `exec_argv_ok`; and `run` itself refuses one that
    /// wouldn't, before any exec.
    #[test]
    fn no_script_builds_an_empty_argv_word() {
        let argvs = std::cell::RefCell::new(Vec::<Vec<String>>::new());
        let record = |argv: &[String]| -> Result<(i64, String), String> {
            argvs.borrow_mut().push(argv.to_vec());
            Ok((3, "k2:not_found".into()))
        };
        for rel in ["", "a", "var/answers.json"] {
            let _ = ls_wall("/data", rel, &record);
        }
        // get / rm / mv never take the folder itself (check_path root_ok=false).
        for rel in ["a", "var/answers.json"] {
            let _ = get_wall("/data", rel, 0, &record);
            let _ = rm_wall("/data", rel, true, &record);
            let _ = rm_wall("/data", rel, false, &record);
            let _ = mv_wall("/data", rel, "b/c", false, &record);
            let _ = mv_wall("/data", rel, "b", true, &record);
        }
        let argvs = argvs.into_inner();
        assert_eq!(argvs.len(), 3 + 2 * 5, "every call reached the exec");
        for a in &argvs {
            assert!(k2_root_proto::engine::exec_argv_ok(a), "the engine would refuse: {a:?}");
            assert!(a.iter().all(|w| !w.is_empty()), "{a:?}");
        }
        // A slip (an empty path into a script that has no root form) fails
        // loudly in `run`, and the exec never sees it.
        let never = |argv: &[String]| -> Result<(i64, String), String> { panic!("exec reached with {argv:?}") };
        for e in [
            get_wall("/data", "", 0, &never).unwrap_err(),
            rm_wall("/data", "", true, &never).unwrap_err(),
            mv_wall("/data", "", "b", false, &never).unwrap_err(),
            mv_wall("/data", "a", "", false, &never).unwrap_err(),
        ] {
            assert_eq!(e.code, FAILED);
            assert!(e.hint.contains("empty argument") && e.hint.contains("nothing ran"), "{}", e.hint);
        }
    }

    /// A failing exec (the App went away, the engine refused) is a loud
    /// `data_failed`, never a silent empty answer.
    #[test]
    fn a_broken_exec_fails_loudly() {
        let gone = |_: &[String]| -> Result<(i64, String), String> { Err("App engine: no such container".into()) };
        assert_eq!(code(get_wall("/data", "a", 0, &gone).unwrap_err()), (FAILED, 409));
        let odd = |_: &[String]| -> Result<(i64, String), String> { Ok((127, "sh: not found".into())) };
        let e = ls_wall("/data", "", &odd).unwrap_err();
        assert_eq!(e.code, FAILED);
        assert!(e.hint.contains("exit 127") && e.hint.contains("sh: not found"), "{}", e.hint);
        let short = |_: &[String]| -> Result<(i64, String), String> { Ok((0, "k2:ok 10 1\naGk=\n".into())) };
        let e = get_wall("/data", "a", 0, &short).unwrap_err();
        assert!(e.hint.contains("expected 10"), "{}", e.hint);
    }
}
