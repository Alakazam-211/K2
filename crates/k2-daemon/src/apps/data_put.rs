//! `k2 app data put <app> <path> [--to <dir>]` (0.46.3): an agent copies
//! its legacy service's data from its own workspace into its App's data
//! folder (`$K2_APP_DATA`, `/data` in the wall), with no terminal on the box.
//!
//! ```text
//! POST /cli/app/data/put {app, from, to?}
//! ```
//!
//! - **Who:** the owner, an Admin, a shell tab, or the App's workspace
//!   agent with "Build and run Apps" (else a held `manage` request).
//! - **What:** `from` is a file or folder inside the App's own workspace
//!   folder (for every caller: a put never reads anything else on the
//!   box); `to` is a relative folder under the data folder (default its
//!   root). Files that exist are overwritten; nothing is deleted. 64 MiB
//!   compressed at most per put (larger data: several puts).
//! - **How:** a walled App's data folder belongs to the `k2app` user, so
//!   the daemon can't write it: the folder goes as a gzip'd tar through
//!   the App's own exec (`k2 app exec`'s engine path, as the App, inside
//!   its wall), then `tar -x` into `/data`. The App must be running (a
//!   live container). On the unwalled process engine (dev boxes) the data
//!   folder is the daemon user's: a plain copy.
//! - **The bytes (0.46.11):** an engine with `exec-stdin` takes the tar on
//!   the exec's stdin: one exec, `tar -x` reading it ([`Way::Stdin`]). An
//!   older engine has no stdin, so the tar goes as base64 argv words of at
//!   most [`PIECE`] bytes (the engine's per-word cap, `exec_argv_ok`),
//!   [`PIECES_PER_EXEC`] to an exec, appended to a temp file, then decoded
//!   and untarred ([`Way::Pieces`]): slower, never refused. Before 0.46.11
//!   the words were 96 KiB, past the cap, so anything that didn't
//!   compress below 4 KiB failed with a bare `App engine: argv`.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use k2_core::deploy::{self, codes};
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, json_body, need_build, no, s, ws_by_id, Who, R};

pub const ROUTE: &str = "/cli/app/data/put";
/// Largest put, compressed (one exec's stdin cap).
pub const MAX_BYTES: usize = k2_root_proto::engine::MAX_EXEC_STDIN as usize;
/// Base64 characters per argv word on an engine without `exec-stdin`: the
/// engine's own per-word cap (`exec_argv_ok`).
pub const PIECE: usize = 4096;
/// Argv words per exec on that path: the engine takes 64 words, five are
/// the script's (`sh -c <script> k2-put <tmp>`).
pub const PIECES_PER_EXEC: usize = 59;
/// One piece exec, and the stdin exec (up to 64 MiB through `tar -x`).
const EXEC_TIMEOUT: u64 = 120;
const STDIN_TIMEOUT: u64 = k2_root_proto::engine::EXEC_TIMEOUT_BOUNDS.1;

/// How a put's bytes reach the App (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    Stdin,
    Pieces,
}

impl Way {
    pub fn as_str(self) -> &'static str {
        match self {
            Way::Stdin => "stdin",
            Way::Pieces => "pieces",
        }
    }
}

/// `to`: relative, no `..`, no NUL.
pub fn check_to(to: &str) -> Result<String, String> {
    let t = to.trim().trim_matches('/');
    if to.trim().starts_with('/') {
        return Err("--to is a folder inside the App's data folder (relative, like uploads or db)".into());
    }
    if t.split('/').any(|p| p == ".." || p == ".") || t.contains('\0') || t.contains('\\') {
        return Err("--to can't climb out of the data folder ('..')".into());
    }
    Ok(t.to_string())
}

/// `from` resolved inside the workspace folder `root` (`Err` = outside).
pub fn resolve_from(root: &Path, raw: &str) -> Result<PathBuf, String> {
    let p = Path::new(raw.trim());
    let p = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
    let canon = dunce::canonicalize(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let root = dunce::canonicalize(&root).unwrap_or_else(|_| root.to_path_buf());
    if !canon.starts_with(&root) {
        return Err(format!("{} is outside the App's workspace folder ({}): a put copies only the workspace's own files", canon.display(), root.display()));
    }
    Ok(canon)
}

/// A gzip'd tar of `from` (a folder's contents, or one file by its name).
/// Symlinks are stored as links, never followed out of the folder.
pub fn tar_gz(from: &Path) -> Result<(Vec<u8>, usize), String> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut b = tar::Builder::new(enc);
    b.follow_symlinks(false);
    let mut files = 0usize;
    if from.is_dir() {
        for e in walk(from)? {
            let rel = e.strip_prefix(from).map_err(|e| e.to_string())?;
            b.append_path_with_name(&e, rel).map_err(|err| format!("{}: {err}", e.display()))?;
            if !e.is_dir() {
                files += 1;
            }
        }
    } else {
        let name = from.file_name().ok_or("the file has no name")?;
        b.append_path_with_name(from, name).map_err(|e| format!("{}: {e}", from.display()))?;
        files = 1;
    }
    let enc = b.into_inner().map_err(|e| e.to_string())?;
    let bytes = enc.finish().map_err(|e| e.to_string())?;
    Ok((bytes, files))
}

fn walk(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut rd: Vec<_> = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten().collect();
        rd.sort_by_key(|e| e.file_name());
        for e in rd {
            let p = e.path();
            let ft = e.file_type().map_err(|e| e.to_string())?;
            out.push(p.clone());
            if ft.is_dir() {
                stack.push(p);
            }
        }
    }
    Ok(out)
}

/// Run one command as the App: `(exit code, output)`.
pub type Exec<'a> = &'a dyn Fn(&[String]) -> Result<(i64, String), String>;

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// Run one command as the App with an optional stdin: `(exit code,
/// output)`.
pub type ExecIn<'a> = &'a dyn Fn(&[String], Option<&[u8]>) -> Result<(i64, String), String>;

/// The output's last lines, for an error (tar's complaint, not a page).
fn tail(out: &str) -> String {
    let t = out.trim();
    let lines: Vec<&str> = t.lines().collect();
    let keep = lines[lines.len().saturating_sub(6)..].join(" | ");
    let n = keep.chars().count();
    if n > 600 {
        format!("…{}", keep.chars().skip(n - 600).collect::<String>())
    } else {
        keep
    }
}

/// Put `tgz` into `root/to` through `exec`, the [`Way`] the engine takes.
/// Returns the number of exec calls. Every error says which step failed
/// and why (the command's exit code and output, or the engine's refusal).
pub fn put_via_exec(tgz: &[u8], root: &str, to: &str, way: Way, exec: ExecIn<'_>) -> Result<usize, String> {
    if tgz.len() > MAX_BYTES {
        return Err(format!("the upload is {} bytes; one put carries {} MiB at most", tgz.len(), MAX_BYTES >> 20));
    }
    let dest = if to.is_empty() { root.to_string() } else { format!("{root}/{to}") };
    let run = |argv: Vec<String>, stdin: Option<&[u8]>, calls: &mut usize| -> Result<(), String> {
        *calls += 1;
        match exec(&argv, stdin) {
            Ok((0, _)) => Ok(()),
            Ok((code, out)) => Err(format!("exit {code}: {}", tail(&out))),
            Err(e) => Err(e),
        }
    };
    let mut calls = 0usize;
    if way == Way::Stdin {
        let script = "set -e; mkdir -p \"$1\"; tar -xzf - -C \"$1\" --no-same-owner";
        run(sv(&["sh", "-c", script, "k2-put", &dest]), Some(tgz), &mut calls)
            .map_err(|e| format!("unpacking {} bytes into {dest} on the App's stdin failed ({e})", tgz.len()))?;
        return Ok(calls);
    }
    // An engine without exec-stdin: base64 pieces under its argv cap.
    let id = &uuid::Uuid::new_v4().simple().to_string()[..12];
    let tmp = format!("{root}/.k2-put-{id}.b64");
    let b64 = base64::engine::general_purpose::STANDARD.encode(tgz);
    let cleanup = |calls: &mut usize| {
        *calls += 1;
        let _ = exec(&sv(&["rm", "-f", &tmp]), None);
    };
    let pieces: Vec<&str> = b64.as_bytes().chunks(PIECE).map(|c| std::str::from_utf8(c).expect("base64 is ASCII")).collect();
    let groups = pieces.len().div_ceil(PIECES_PER_EXEC);
    for (i, group) in pieces.chunks(PIECES_PER_EXEC).enumerate() {
        let mut argv = sv(&["sh", "-c", "t=\"$1\"; shift; printf '%s' \"$@\" >> \"$t\"", "k2-put", &tmp]);
        argv.extend(group.iter().map(|p| p.to_string()));
        if let Err(e) = run(argv, None, &mut calls) {
            cleanup(&mut calls);
            return Err(format!(
                "writing part {} of {groups} of the upload into the App failed ({e}); this box's App engine predates piped input, \
                 so the put goes in small parts: the owner can install the current engine with k2 app engine install",
                i + 1
            ));
        }
    }
    let script = "set -e; mkdir -p \"$2\"; base64 -d < \"$1\" | tar -xzf - -C \"$2\" --no-same-owner; rm -f \"$1\"";
    if let Err(e) = run(sv(&["sh", "-c", script, "k2-put", &tmp, &dest]), None, &mut calls) {
        cleanup(&mut calls);
        return Err(format!("unpacking into {dest} failed ({e})"));
    }
    Ok(calls)
}

/// The unwalled engine's data folder is the daemon user's: copy.
fn copy_into(from: &Path, dest: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    if from.is_file() {
        std::fs::copy(from, dest.join(from.file_name().ok_or("no file name")?)).map_err(|e| e.to_string())?;
        return Ok(1);
    }
    let mut n = 0;
    for e in walk(from)? {
        let rel = e.strip_prefix(from).map_err(|e| e.to_string())?;
        let target = dest.join(rel);
        let meta = std::fs::symlink_metadata(&e).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        } else if meta.is_file() {
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            std::fs::copy(&e, &target).map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    Ok(n)
}

/// POST `/cli/app/data/put {app, from, to?}`.
pub(crate) fn route(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let to = check_to(s(&v, "to").unwrap_or("")).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    let raw = s(&v, "from").ok_or_else(|| no(codes::BAD_REQUEST, 400, "from is required (a file or folder in your workspace)"))?;
    let ws = ws_by_id(&app.owner_workspace).ok_or_else(|| no(codes::FORBIDDEN, 404, "the App's workspace is not registered"))?;
    let from = resolve_from(Path::new(&ws.path), raw).map_err(|e| no(codes::FOLDER_OUTSIDE_WORKSPACE, 403, e))?;
    // C5 (disk guard): the App's data lands on ~/.k2 (process engine) or
    // the wall's tree.
    let walled = !super::supervisor::runs_unwalled(&app);
    let paths = if walled { super::resources::app_paths(true).split_off(1) } else { super::resources::app_paths(false) };
    let need = super::resources::size_capped(&from, MAX_BYTES as u64);
    let extra = match super::resources::app_gate(who, "/cli/app/data/put", "data put", &app.app_id, &app.owner_workspace, Some(&app), &paths, need, &v, false)? {
        super::resources::Gate::Go(x) => x,
        super::resources::Gate::Held(h) => return Ok(h),
    };
    let by = actor(who);
    let mut way: Option<Way> = None;
    let mut execs = 0usize;
    let (files, bytes, into) = if super::supervisor::runs_unwalled(&app) {
        let dest = deploy::app_dir(&app.app_id).join("data").join(&to);
        let n = copy_into(&from, &dest).map_err(|e| no("data_put_failed", 409, e))?;
        (n, 0usize, dest.to_string_lossy().to_string())
    } else {
        let dep = super::exec::live_guarded(&app)?;
        let (tgz, n) = tar_gz(&from).map_err(|e| no("data_put_failed", 409, e))?;
        if tgz.len() > MAX_BYTES {
            return Err(no(
                codes::BAD_REQUEST,
                413,
                format!("{} compresses to {} MiB; a put carries {} MiB at most: put its parts one by one", from.display(), tgz.len() >> 20, MAX_BYTES >> 20),
            ));
        }
        let w = match super::guarded::engine_has_feature(k2_root_proto::engine::FEATURE_EXEC_STDIN) {
            Ok(true) => Way::Stdin,
            Ok(false) => Way::Pieces,
            Err(e) => return Err(no(e.code, 503, format!("asking the App engine what it supports failed: {}", e.message))),
        };
        let app_id = app.app_id.clone();
        let exec = move |argv: &[String], stdin: Option<&[u8]>| -> Result<(i64, String), String> {
            let secs = if stdin.is_some() { STDIN_TIMEOUT } else { EXEC_TIMEOUT };
            let r = super::guarded::exec_once(&app_id, &dep, argv, secs, stdin).map_err(|e| format!("{} [{}]", e.message, e.code))?;
            if r["timedOut"].as_bool() == Some(true) {
                return Err(format!("the command ran past {secs} s and was stopped"));
            }
            Ok((r["exitCode"].as_i64().unwrap_or(-1), r["output"].as_str().unwrap_or("").to_string()))
        };
        execs = put_via_exec(&tgz, "/data", &to, w, &exec).map_err(|e| no("data_put_failed", 409, e))?;
        way = Some(w);
        (n, tgz.len(), if to.is_empty() { "/data".to_string() } else { format!("/data/{to}") })
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = deploy::audit::append(
            &conn,
            deploy::audit::SUBJECT_APP,
            &app.app_id,
            "data_put",
            &by,
            None,
            &json!({ "from": from, "to": into, "files": files, "bytes": bytes, "way": way.map(Way::as_str), "execs": execs }),
        );
    }
    let mut out = json!({ "ok": true, "app": app.app_id, "from": from, "to": into, "files": files, "compressedBytes": bytes });
    if let Some(w) = way {
        out["way"] = json!(w.as_str());
        out["execs"] = json!(execs);
    }
    super::resources::decorate(&mut out, extra);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Scratch;

    #[test]
    fn to_stays_inside_the_data_folder() {
        assert_eq!(check_to("").unwrap(), "");
        assert_eq!(check_to("uploads/2026/").unwrap(), "uploads/2026");
        for bad in ["/etc", "../x", "a/../../b", "a/./b"] {
            assert!(check_to(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn from_must_be_inside_the_workspace() {
        let d = Scratch::new();
        let ws = d.path().join("ws");
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::write(d.path().join("secret.txt"), "x").unwrap();
        assert!(resolve_from(&ws, "data").is_ok(), "relative to the workspace");
        assert!(resolve_from(&ws, &ws.join("data").to_string_lossy()).is_ok());
        let e = resolve_from(&ws, &d.path().join("secret.txt").to_string_lossy()).unwrap_err();
        assert!(e.contains("outside"), "{e}");
        assert!(resolve_from(&ws, "../secret.txt").unwrap_err().contains("outside"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.path().join("secret.txt"), ws.join("link")).unwrap();
            assert!(resolve_from(&ws, "link").unwrap_err().contains("outside"), "a symlink out is outside");
        }
    }

    /// Bytes that don't compress (a scanned PDF, a photo, a tarball).
    #[cfg(unix)]
    fn noise(n: usize, seed: u64) -> Vec<u8> {
        let mut x = seed | 1;
        let mut out = Vec::with_capacity(n + 8);
        while out.len() < n {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            out.extend_from_slice(&x.to_le_bytes());
        }
        out.truncate(n);
        out
    }

    /// Runs each command on this machine the way the engine runs it in the
    /// App's wall: it refuses exactly the argv the engine refuses
    /// (`exec_argv_ok`, 4096 bytes a word), and pipes `stdin` in. `calls`
    /// counts execs; `stdin_ok` false plays an engine without `exec-stdin`
    /// (a put must then never send stdin).
    #[cfg(unix)]
    struct Engine {
        stdin_ok: bool,
        calls: std::cell::Cell<usize>,
    }

    #[cfg(unix)]
    impl Engine {
        fn new(stdin_ok: bool) -> Self {
            Self { stdin_ok, calls: std::cell::Cell::new(0) }
        }
        fn run(&self, argv: &[String], stdin: Option<&[u8]>) -> Result<(i64, String), String> {
            use std::io::Write as _;
            self.calls.set(self.calls.get() + 1);
            if !k2_root_proto::engine::exec_argv_ok(argv) {
                return Err("App engine: argv refused: 1 to 64 words, each 1 to 4096 bytes [bad_request]".into());
            }
            assert!(stdin.is_none() || self.stdin_ok, "stdin sent to an engine without exec-stdin");
            let mut c = std::process::Command::new(&argv[0]);
            c.args(&argv[1..]).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
            c.stdin(if stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() });
            let mut child = c.spawn().map_err(|e| e.to_string())?;
            let writer = stdin.map(|s| {
                let mut sin = child.stdin.take().unwrap();
                let s = s.to_vec();
                std::thread::spawn(move || {
                    let _ = sin.write_all(&s);
                })
            });
            let o = child.wait_with_output().map_err(|e| e.to_string())?;
            if let Some(w) = writer {
                w.join().unwrap();
            }
            let mut out = o.stdout;
            out.extend_from_slice(&o.stderr);
            Ok((o.status.code().unwrap_or(-1) as i64, String::from_utf8_lossy(&out).to_string()))
        }
        fn way(&self) -> Way {
            if self.stdin_ok {
                Way::Stdin
            } else {
                Way::Pieces
            }
        }
        /// Put `from` into `root/to`; the number of execs.
        fn put(&self, from: &Path, root: &Path, to: &str) -> Result<usize, String> {
            let (tgz, _) = tar_gz(from)?;
            let run = |a: &[String], s: Option<&[u8]>| self.run(a, s);
            put_via_exec(&tgz, &root.to_string_lossy(), to, self.way(), &run)
        }
    }

    /// The walled path end to end, both ways, with the commands that run
    /// inside the App's wall run here against a scratch "data" folder:
    /// files land, a single file lands by its name, no temp file is left,
    /// and garbage fails loudly naming the step and tar's own complaint.
    #[cfg(unix)]
    #[test]
    fn put_via_exec_lands_the_files_and_cleans_up() {
        for stdin_ok in [true, false] {
            let d = Scratch::new();
            let src = d.path().join("legacy-data");
            std::fs::create_dir_all(src.join("sub")).unwrap();
            std::fs::write(src.join("jobs.db"), vec![7u8; 300 * 1024]).unwrap();
            std::fs::write(src.join("sub/notes.txt"), "hello").unwrap();
            let root = d.path().join("data");
            std::fs::create_dir_all(&root).unwrap();
            let e = Engine::new(stdin_ok);
            let calls = e.put(&src, &root, "import").unwrap();
            if stdin_ok {
                assert_eq!(calls, 1, "one exec carries the whole tar");
            } else {
                assert!(calls >= 2, "pieces, then the unpack: {calls}");
            }
            assert_eq!(std::fs::read(root.join("import/jobs.db")).unwrap(), vec![7u8; 300 * 1024]);
            assert_eq!(std::fs::read_to_string(root.join("import/sub/notes.txt")).unwrap(), "hello");
            let left: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
            assert_eq!(left, vec!["import".to_string()], "no temp file ({stdin_ok})");
            e.put(&src.join("sub/notes.txt"), &root, "").unwrap();
            assert_eq!(std::fs::read_to_string(root.join("notes.txt")).unwrap(), "hello");
            let run = |a: &[String], s: Option<&[u8]>| e.run(a, s);
            let err = put_via_exec(b"not a tarball", &root.to_string_lossy(), "x", e.way(), &run).unwrap_err();
            assert!(err.contains("unpacking") && err.contains("exit "), "names the step and the exit: {err}");
            assert!(err.to_lowercase().contains("gzip") || err.to_lowercase().contains("tar"), "carries tar's own complaint: {err}");
            assert!(!std::fs::read_dir(&root).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(".k2-put-")));
        }
    }

    /// 0.46.11 field report: a put of anything that doesn't compress failed
    /// with `App engine: argv` (96 KiB words past the engine's 4096-byte
    /// cap). Random bytes of 4 KB, 1 MB and 20 MB and a folder of binaries
    /// now land byte for byte, through stdin and, on an older engine,
    /// through pieces that fit the cap.
    #[cfg(unix)]
    #[test]
    fn binary_puts_are_byte_identical_both_ways() {
        for stdin_ok in [true, false] {
            let d = Scratch::new();
            let src = d.path().join("src");
            std::fs::create_dir_all(src.join("scans/2026")).unwrap();
            let root = d.path().join("data");
            std::fs::create_dir_all(&root).unwrap();
            let e = Engine::new(stdin_ok);
            for (i, n) in [4096usize, 1 << 20, 20 << 20].into_iter().enumerate() {
                let body = noise(n, 11 + i as u64);
                let name = format!("random-{n}.bin");
                std::fs::write(src.join(&name), &body).unwrap();
                let calls = e.put(&src.join(&name), &root, "one").unwrap_or_else(|err| panic!("{n} bytes ({stdin_ok}): {err}"));
                if stdin_ok {
                    assert_eq!(calls, 1, "{n} bytes: one exec");
                }
                assert!(std::fs::read(root.join("one").join(&name)).unwrap() == body, "{n} bytes byte for byte ({stdin_ok})");
                std::fs::remove_file(src.join(&name)).unwrap();
            }
            // A folder of binaries, nested.
            let mut want = Vec::new();
            for (i, (rel, n)) in [("a.pdf", 57_207usize), ("b.jpg", 300_000), ("scans/2026/c.pdf", 2_500_000), ("scans/d.bin", 1)].iter().enumerate() {
                let body = noise(*n, 100 + i as u64);
                std::fs::write(src.join(rel), &body).unwrap();
                want.push((rel.to_string(), body));
            }
            e.put(&src, &root, "folder").unwrap_or_else(|err| panic!("folder ({stdin_ok}): {err}"));
            for (rel, body) in &want {
                assert!(std::fs::read(root.join("folder").join(rel)).unwrap() == *body, "{rel} byte for byte ({stdin_ok})");
            }
            assert!(!std::fs::read_dir(&root).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(".k2-put-")));
        }
    }

    /// The reporter's sizes (no real files): one 18,316,059-byte file that
    /// doesn't compress (a scanned PDF) in one put, then eleven puts that
    /// total 88,001,156 bytes (57,207 the smallest), each read back byte
    /// for byte. Prints how long each part takes (`--no-capture`).
    #[cfg(unix)]
    #[test]
    fn a_scanned_pdf_sized_put_and_eleven_puts_of_88_mb() {
        let d = Scratch::new();
        let src = d.path().join("pdfs");
        std::fs::create_dir_all(&src).unwrap();
        let root = d.path().join("data");
        std::fs::create_dir_all(&root).unwrap();
        let big = noise(18_316_059, 42);
        std::fs::write(src.join("scan.pdf"), &big).unwrap();
        for stdin_ok in [true, false] {
            let e = Engine::new(stdin_ok);
            let t0 = std::time::Instant::now();
            let calls = e.put(&src.join("scan.pdf"), &root, e.way().as_str()).unwrap_or_else(|err| panic!("18.3 MB ({stdin_ok}): {err}"));
            assert!(std::fs::read(root.join(e.way().as_str()).join("scan.pdf")).unwrap() == big, "18.3 MB byte for byte ({stdin_ok})");
            eprintln!("[timing] 18,316,059-byte random file, way {}: {calls} exec(s), {:?}", e.way().as_str(), t0.elapsed());
        }
        std::fs::remove_file(src.join("scan.pdf")).unwrap();
        // Eleven files, 88,001,156 bytes in all.
        let total: usize = 88_001_156;
        let mut sizes = vec![18_316_059usize, 57_207];
        let rest = total - sizes.iter().sum::<usize>();
        for i in 0..9 {
            sizes.push(if i < 8 { rest / 9 + i * 1000 } else { 0 });
        }
        let last = total - sizes.iter().sum::<usize>();
        *sizes.last_mut().unwrap() = last;
        assert_eq!(sizes.iter().sum::<usize>(), total);
        assert_eq!(sizes.len(), 11);
        let e = Engine::new(true);
        let t0 = std::time::Instant::now();
        for (i, n) in sizes.iter().enumerate() {
            let body = noise(*n, 1000 + i as u64);
            let name = format!("doc-{i:02}.pdf");
            std::fs::write(src.join(&name), &body).unwrap();
            e.put(&src.join(&name), &root, "docs").unwrap_or_else(|err| panic!("put {i} ({n} bytes): {err}"));
            assert!(std::fs::read(root.join("docs").join(&name)).unwrap() == body, "put {i} ({n} bytes) byte for byte");
            std::fs::remove_file(src.join(&name)).unwrap();
        }
        assert_eq!(e.calls.get(), 11, "one exec a put");
        eprintln!("[timing] 11 puts, {total} bytes, way stdin: {:?}", t0.elapsed());
    }

    /// A put that fails names its cause: the engine's own refusal and the
    /// step, never a bare "argv"; past 64 MiB it never starts.
    #[cfg(unix)]
    #[test]
    fn a_failed_put_says_why() {
        let refuse = |_: &[String], _: Option<&[u8]>| -> Result<(i64, String), String> {
            Err("App engine: deployment k2app-jobs--d-0123456789ab isn't running [engine_not_found]".into())
        };
        let err = put_via_exec(b"x", "/data", "", Way::Stdin, &refuse).unwrap_err();
        assert!(err.contains("on the App's stdin failed") && err.contains("isn't running"), "{err}");
        let err = put_via_exec(b"x", "/data", "", Way::Pieces, &refuse).unwrap_err();
        assert!(err.contains("part 1 of 1") && err.contains("isn't running") && err.contains("k2 app engine install"), "{err}");
        let huge = vec![0u8; MAX_BYTES + 1];
        let never = |_: &[String], _: Option<&[u8]>| -> Result<(i64, String), String> { panic!("no exec past the cap") };
        assert!(put_via_exec(&huge, "/data", "", Way::Stdin, &never).unwrap_err().contains("64 MiB"));
    }

    /// The fallback's words fit the engine's argv rules exactly.
    #[test]
    fn pieces_fit_the_engine_argv_cap() {
        let argv: Vec<String> = ["sh", "-c", "t=\"$1\"; shift; printf '%s' \"$@\" >> \"$t\"", "k2-put", "/data/.k2-put-0123456789ab.b64"]
            .iter()
            .map(|s| s.to_string())
            .chain((0..PIECES_PER_EXEC).map(|_| "A".repeat(PIECE)))
            .collect();
        assert!(k2_root_proto::engine::exec_argv_ok(&argv), "a full piece exec passes the engine's check");
        let mut over = argv.clone();
        over.push("A".into());
        assert!(!k2_root_proto::engine::exec_argv_ok(&over), "and is the most it takes");
        assert_eq!(MAX_BYTES, 64 << 20);
    }

    /// A stand-in engine on a temp socket, speaking the real wire: `version`
    /// lists `exec-stdin` only when `new`; `exec` checks the argv the way the
    /// engine does, refuses `stdinLen` when not `new` (an older engine's
    /// strict parse), reads the body as stdin and runs the argv here.
    #[cfg(unix)]
    fn fake_engine(tag: &str, new: bool) -> std::path::PathBuf {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::os::unix::net::UnixListener;
        let dir = std::path::PathBuf::from(format!("/tmp/k2dp-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("e.sock");
        let l = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let c = c.unwrap();
                std::thread::spawn(move || {
                    let mut r = BufReader::new(c.try_clone().unwrap());
                    let mut line = String::new();
                    r.read_line(&mut line).unwrap();
                    let mut w = c;
                    let req: k2_root_proto::engine::Request = serde_json::from_str(line.trim()).unwrap();
                    let ans = match req {
                        k2_root_proto::engine::Request::Version => {
                            let f: Vec<&str> = if new { vec!["build", "exec-stdin"] } else { vec!["build"] };
                            json!({ "ok": true, "data": { "protocol": 3, "features": f } })
                        }
                        k2_root_proto::engine::Request::Exec { stdin_len: Some(_), .. } if !new => {
                            json!({ "ok": false, "code": "bad_request", "message": "unknown or malformed request" })
                        }
                        k2_root_proto::engine::Request::Exec { argv, stdin_len, .. } => {
                            if !k2_root_proto::engine::exec_argv_ok(&argv) {
                                json!({ "ok": false, "code": "bad_request", "message": "argv refused: 1 to 64 words, each 1 to 4096 bytes" })
                            } else {
                                let stdin = stdin_len.map(|n| {
                                    let mut b = vec![0u8; n as usize];
                                    r.read_exact(&mut b).unwrap();
                                    b
                                });
                                let (code, out) = Engine::new(stdin.is_some()).run(&argv, stdin.as_deref()).unwrap();
                                json!({ "ok": true, "data": { "exitCode": code, "timedOut": false, "output": out } })
                            }
                        }
                        _ => json!({ "ok": false, "code": "bad_request", "message": "not in this stand-in" }),
                    };
                    let mut b = serde_json::to_vec(&ans).unwrap();
                    b.push(b'\n');
                    let _ = w.write_all(&b);
                });
            }
        });
        sock
    }

    /// The daemon's side over a socket (0.46.11): it reads the engine's
    /// features; with `exec-stdin` a put is one exec whose body is the tar,
    /// and `k2 app exec`'s stdin reaches the command (1 MiB → `wc -c`);
    /// against an older engine the put falls back to pieces under its argv
    /// cap (byte for byte), and an exec with input is refused with what to
    /// do, not sent.
    #[cfg(unix)]
    #[test]
    fn over_the_engine_socket_new_and_old_engines() {
        use super::super::guarded::{engine_has_feature_at, exec_once_at};
        use k2_root_proto::engine::FEATURE_EXEC_STDIN;
        let d = Scratch::new();
        let src = d.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let root = d.path().join("data");
        std::fs::create_dir_all(&root).unwrap();
        let body = noise(1 << 20, 77);
        std::fs::write(src.join("scan.pdf"), &body).unwrap();
        let (tgz, _) = tar_gz(&src.join("scan.pdf")).unwrap();
        for (tag, new) in [("new", true), ("old", false)] {
            let sock = fake_engine(tag, new);
            let has = engine_has_feature_at(&sock, FEATURE_EXEC_STDIN).unwrap();
            assert_eq!(has, new, "{tag}: features read from version");
            let way = if has { Way::Stdin } else { Way::Pieces };
            let exec = |argv: &[String], stdin: Option<&[u8]>| -> Result<(i64, String), String> {
                let r = exec_once_at(&sock, "jobs", "d-0123456789ab", argv, 60, stdin).map_err(|e| e.message)?;
                Ok((r["exitCode"].as_i64().unwrap(), r["output"].as_str().unwrap().to_string()))
            };
            let calls = put_via_exec(&tgz, &root.to_string_lossy(), tag, way, &exec).unwrap_or_else(|e| panic!("{tag}: {e}"));
            assert!(std::fs::read(root.join(tag).join("scan.pdf")).unwrap() == body, "{tag}: byte for byte");
            if new {
                assert_eq!(calls, 1);
                let r = exec_once_at(&sock, "jobs", "d-0123456789ab", &["sh".into(), "-c".into(), "wc -c".into()], 60, Some(&body)).unwrap();
                assert_eq!(r["output"].as_str().unwrap().trim(), "1048576", "k2 app exec's stdin reaches the command");
                let r = super::super::exec::stdin_engine_ok(Ok(has));
                assert!(r.is_ok());
            } else {
                assert!(calls > 2, "pieces: {calls}");
                let refused = super::super::exec::stdin_engine_ok(Ok(has)).unwrap_err();
                assert_eq!(refused.code, codes::ENGINE_PROTOCOL_MISMATCH);
                assert!(refused.hint.contains("k2 app engine install") && refused.hint.contains("--no-stdin"), "{}", refused.hint);
                // What the gate saves us from: the older engine refuses it.
                let e = exec_once_at(&sock, "jobs", "d-0123456789ab", &["true".into()], 60, Some(b"x")).unwrap_err();
                assert!(e.message.contains("malformed"), "{}", e.message);
            }
            std::fs::remove_dir_all(sock.parent().unwrap()).unwrap();
        }
    }
}
