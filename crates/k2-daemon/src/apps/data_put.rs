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
//!   its wall) in base64 chunks, then `tar -x` into `/data`. The App must
//!   be running (a live container). On the unwalled process engine (dev
//!   boxes) the data folder is the daemon user's: a plain copy.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use k2_core::deploy::{self, codes};
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, json_body, need_build, no, s, ws_by_id, Who, R};

pub const ROUTE: &str = "/cli/app/data/put";
/// Largest put, compressed.
pub const MAX_BYTES: usize = 64 << 20;
/// Base64 characters per exec (well under Linux's 128 KiB per argument).
pub const CHUNK: usize = 96 * 1024;
const EXEC_TIMEOUT: u64 = 120;

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

/// Put `tgz` into `root/to` through `exec` (base64 chunks appended to a
/// temp file under `root`, then decoded and untarred). Returns the number
/// of exec calls. The temp file is removed on success and on failure.
pub fn put_via_exec(tgz: &[u8], root: &str, to: &str, exec: Exec<'_>) -> Result<usize, String> {
    let id = &uuid::Uuid::new_v4().simple().to_string()[..12];
    let tmp = format!("{root}/.k2-put-{id}.b64");
    let dest = if to.is_empty() { root.to_string() } else { format!("{root}/{to}") };
    let b64 = base64::engine::general_purpose::STANDARD.encode(tgz);
    let mut calls = 0usize;
    let cleanup = |calls: &mut usize| {
        *calls += 1;
        let _ = exec(&sv(&["rm", "-f", &tmp]));
    };
    let run = |argv: Vec<String>, calls: &mut usize| -> Result<(), String> {
        *calls += 1;
        match exec(&argv) {
            Ok((0, _)) => Ok(()),
            Ok((code, out)) => Err(format!("exit {code}: {}", out.trim())),
            Err(e) => Err(e),
        }
    };
    for chunk in b64.as_bytes().chunks(CHUNK) {
        let c = std::str::from_utf8(chunk).map_err(|e| e.to_string())?;
        if let Err(e) = run(sv(&["sh", "-c", "printf '%s' \"$1\" >> \"$2\"", "k2-put", c, &tmp]), &mut calls) {
            cleanup(&mut calls);
            return Err(format!("writing the upload failed ({e})"));
        }
    }
    let script = "set -e; mkdir -p \"$2\"; base64 -d < \"$1\" | tar -xzf - -C \"$2\" --no-same-owner; rm -f \"$1\"";
    if let Err(e) = run(sv(&["sh", "-c", script, "k2-put", &tmp, &dest]), &mut calls) {
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
        let app_id = app.app_id.clone();
        let exec = move |argv: &[String]| -> Result<(i64, String), String> {
            let r = super::guarded::exec_once(&app_id, &dep, argv, EXEC_TIMEOUT).map_err(|e| e.message)?;
            Ok((r["exitCode"].as_i64().unwrap_or(-1), r["output"].as_str().unwrap_or("").to_string()))
        };
        put_via_exec(&tgz, "/data", &to, &exec).map_err(|e| no("data_put_failed", 409, e))?;
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
            &json!({ "from": from, "to": into, "files": files, "bytes": bytes }),
        );
    }
    let mut out = json!({ "ok": true, "app": app.app_id, "from": from, "to": into, "files": files, "compressedBytes": bytes });
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

    /// The walled path end to end, with `exec` running each command on this
    /// machine against a scratch "data" folder (the commands are exactly
    /// what runs inside the App's wall): chunks, decode, untar, temp file
    /// gone; a bad unpack cleans up and fails loudly.
    #[test]
    fn put_via_exec_lands_the_files_and_cleans_up() {
        let d = Scratch::new();
        let src = d.path().join("legacy-data");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("jobs.db"), vec![7u8; 300 * 1024]).unwrap();
        std::fs::write(src.join("sub/notes.txt"), "hello").unwrap();
        let root = d.path().join("data");
        std::fs::create_dir_all(&root).unwrap();
        let (tgz, files) = tar_gz(&src).unwrap();
        assert_eq!(files, 2);
        let local = |argv: &[String]| -> Result<(i64, String), String> {
            let o = std::process::Command::new(&argv[0]).args(&argv[1..]).output().map_err(|e| e.to_string())?;
            Ok((o.status.code().unwrap_or(-1) as i64, String::from_utf8_lossy(&o.stderr).to_string()))
        };
        let calls = put_via_exec(&tgz, &root.to_string_lossy(), "import", &local).unwrap();
        assert!(calls >= 2, "chunks, then the unpack: {calls}");
        // Bytes that don't compress take several chunks.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let noise: Vec<u8> = (0..400_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        std::fs::write(src.join("noise.bin"), &noise).unwrap();
        let (tgz2, _) = tar_gz(&src).unwrap();
        let calls = put_via_exec(&tgz2, &root.to_string_lossy(), "again", &local).unwrap();
        assert!(calls > 3, "several chunks: {calls}");
        assert_eq!(std::fs::read(root.join("again/noise.bin")).unwrap(), noise);
        std::fs::remove_file(src.join("noise.bin")).unwrap();
        std::fs::remove_dir_all(root.join("again")).unwrap();
        assert_eq!(std::fs::read(root.join("import/jobs.db")).unwrap(), vec![7u8; 300 * 1024]);
        assert_eq!(std::fs::read_to_string(root.join("import/sub/notes.txt")).unwrap(), "hello");
        let left: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(left, vec!["import".to_string()], "the temp file is gone");
        // A single file lands by its name at the root.
        let (one, _) = tar_gz(&src.join("sub/notes.txt")).unwrap();
        put_via_exec(&one, &root.to_string_lossy(), "", &local).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("notes.txt")).unwrap(), "hello");
        // Garbage fails loudly and leaves no temp file.
        let e = put_via_exec(b"not a tarball", &root.to_string_lossy(), "x", &local).unwrap_err();
        assert!(e.contains("unpacking"), "{e}");
        assert!(!std::fs::read_dir(&root).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(".k2-put-")));
    }

    #[test]
    fn chunks_fit_one_argument() {
        assert!(CHUNK < 128 * 1024);
    }
}
