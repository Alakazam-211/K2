//! `k2 doc export` (prd-k2-core-exports-v1 §4.3; vs-live
//! `prd-k2-core-exports-v1-host-vs-live.md` E2): a doc's version as a file,
//! through the kit's export verb (P24: `export` needs read, `export
//! --version` needs history, both bound to `file_now` / `file_at` and
//! `ExportRegistry::export`). People and agents take this one path.
//!
//! | Route | Method | Right | What |
//! |---|---|---|---|
//! | `/cli/docs/export` | POST | read (+ history with `version`; edit where it lands) | start a job; waits up to 20 s for it |
//! | `/cli/docs/export/targets` | GET | read | the targets for this doc (`--list`) |
//! | `/cli/docs/export/status` | GET | the job's caller (or the box owner) | queued / running (done, total) / the result |
//! | `/cli/docs/export/cancel` | POST | the job's caller (or the box owner) | kill it (`MPC-E004`) |
//! | `/cli/docs/export/fetch` | GET | the job's caller (or the box owner) | a fetch job's bytes, base64, 16 MiB a call |
//!
//! **A job** (daemon-owned; the CLI may give up and it still finishes):
//! 1. the source version's verified bytes are copied into
//!    `~/.k2/exports/jobs/<job>/in` (`file_at_to` / `file_now_to`). On any
//!    kit error the job folder is deleted before a decoder sees a byte; a
//!    damaged blob keeps its own id (`MPC-E705 blob_damaged`);
//! 2. `as-is` runs in the daemon's thread (X2: nothing else does); every
//!    other target runs in the `k2-export` worker (`export_worker`);
//! 3. the output goes to a temp name (`.<name>.k2x-<job>.part`) beside
//!    where it lands, and is renamed into place with a no-replace rename
//!    only after a clean finish (X5); on any error the temp file and the
//!    input are deleted and nothing is placed.
//!
//! **Where it lands** (X1, X3, X4): beside the source as
//! `<stem>.v<doc version>.<ext>`, `" (2)"` on a clash, never overwriting
//! (the caller needs edit there); or `out`: inside a workspace only where
//! the caller can edit (an existing file is refused, `exists`); outside
//! every workspace the job keeps the bytes and the CLI writes them itself
//! (`fetch`). `fetch: true` (every `--host` call) does the same.
//!
//! **Audit:** one `doc-exported` line per export, refusals and failures
//! too, metadata only.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use k2_core::log_debug;
use k2_export::proto::{Header, Kind, Limits};
use k2_mpc_base::blob::BlobError;
use k2_mpc_base::errors::{ErrorReport, Refusal};
use k2_mpc_base::export::{ExportCtl, ExportError, ExportRegistry, NoResources, Options, Sink, Source};
use k2_mpc_base::session::{BlobSession, FileCodec, FileCopy, Need, Participant, Session, SessionError, Stay};
use k2_mpc_base::store::DocKey;
use k2_mpc_base::tools::exit;

use super::caller::{refusal, DocCaller};
use super::export_worker;
use super::host::{self, DocRights};
use super::routes::{self, Call};
use super::target::{self, Target, Tier};
use crate::cli_response::CliResponse;

pub const EXPORT: &str = "/cli/docs/export";
pub const TARGETS: &str = "/cli/docs/export/targets";
pub const STATUS: &str = "/cli/docs/export/status";
pub const CANCEL: &str = "/cli/docs/export/cancel";
pub const FETCH: &str = "/cli/docs/export/fetch";

/// Every export route.
pub const ROUTES: &[&str] = &[EXPORT, TARGETS, STATUS, CANCEL, FETCH];
/// The job routes (no doc: the job names it).
pub const JOB_ROUTES: &[&str] = &[STATUS, CANCEL, FETCH];

/// One Access Audit line per export (metadata only).
pub const AUDIT_EXPORTED: &str = "doc-exported";

/// The kit's `as-is` target.
pub const AS_IS: &str = "as-is";

/// How long `POST /cli/docs/export` waits for the job before answering
/// `running` (the CLI then polls `status`).
pub const MAX_WAIT: Duration = Duration::from_secs(20);

/// Largest `fetch` chunk (raw bytes; base64 on the wire).
pub const FETCH_CHUNK: u64 = 16 << 20;

/// A finished job (and a fetch job's bytes) is kept this long.
pub const KEEP: Duration = Duration::from_secs(3600);

/// The temp output's suffix (`.<name>.k2x-<job>.part`): the boot sweep
/// removes only names that match it.
const PART_TAG: &str = ".k2x-";

// ── errors ──────────────────────────────────────────────────────────────

/// HTTP status for a kit export id (vs-live §6).
fn kit_status(id: &str) -> &'static str {
    match id {
        "MPC-E001" | "MPC-E002" => "400 Bad Request",
        "MPC-E003" => "413 Payload Too Large",
        "MPC-E004" => "409 Conflict",
        "MPC-E005" => "422 Unprocessable Entity",
        _ => "500 Internal Server Error",
    }
}

/// A kit report on the wire: `code` is the kit's name; `id`, `exit` (the
/// CLI exits with it), the kit's `next` and `details` ride along.
fn report_response(r: &ErrorReport) -> CliResponse {
    let details: Value = serde_json::from_str(&r.to_json())
        .ok()
        .and_then(|v: Value| v.get("details").cloned())
        .unwrap_or_else(|| json!({}));
    kit_response(r.id(), r.exit(), r.message(), r.next(), details)
}

fn kit_response(id: &str, exit: u8, message: &str, next: &str, details: Value) -> CliResponse {
    let code = k2_mpc_base::errors::lookup(id).map(|e| e.name).unwrap_or("export_internal");
    CliResponse {
        status: kit_status(id),
        content_type: "application/json",
        body: json!({
            "ok": false,
            "error": { "code": code, "id": id, "exit": exit, "hint": message, "next": next, "details": details },
        })
        .to_string(),
    }
}

/// The worker's `E` frame (the kit's report JSON).
fn kit_json_response(v: &Value) -> CliResponse {
    let id = v.get("id").and_then(Value::as_str).unwrap_or("MPC-E007");
    let exit = v.get("exit").and_then(Value::as_u64).and_then(|e| u8::try_from(e).ok()).unwrap_or(exit::NO);
    kit_response(
        id,
        exit,
        v.get("message").and_then(Value::as_str).unwrap_or("the export failed"),
        v.get("next").and_then(Value::as_str).unwrap_or(""),
        v.get("details").cloned().unwrap_or_else(|| json!({})),
    )
}

pub(super) fn export_error(e: &ExportError) -> CliResponse {
    report_response(&e.report())
}

fn run_error(e: k2_export::host::RunError) -> CliResponse {
    use k2_export::host::RunError as R;
    match e {
        R::Kit(v) => kit_json_response(&v),
        R::Limit(l) => export_error(&ExportError::Limit(l)),
        R::Cancelled => export_error(&ExportError::Cancelled),
        R::Internal { worker, message } => report_response(
            &ExportError::Internal { exporter: k2_export::proto::BINARY_NAME.into(), message }.report().with("worker", worker),
        ),
        R::Io(m) => export_error(&ExportError::Io(io::Error::other(m))),
        R::Spawn(m) => worker_unavailable(&m),
    }
}

fn worker_unavailable(why: &str) -> CliResponse {
    routes::with_exit(
        refusal(
            "503 Service Unavailable",
            "export_worker_unavailable",
            format!("the export worker isn't available on this server ({why}); as-is exports still work"),
        ),
        exit::NO,
    )
}

fn job_not_found() -> CliResponse {
    routes::with_exit(
        refusal("404 Not Found", "export_job_not_found", "no such export job (it ended over an hour ago, or isn't yours)"),
        exit::NOT_FOUND,
    )
}

fn place_forbidden(rel: &str) -> CliResponse {
    routes::with_exit(
        refusal(
            "403 Forbidden",
            "export_place_forbidden",
            format!(
                "you can't create {rel} there. Use --out with a path in your own workspace, or a local path (k2 doc export --out FILE)"
            ),
        ),
        exit::REFUSED,
    )
}

fn target_refusal(r: target::Refusal) -> CliResponse {
    let code = match r.status {
        s if s.starts_with("409") => exit::USAGE,
        s if s.starts_with("400") => exit::USAGE,
        s if s.starts_with("403") => exit::REFUSED,
        _ => exit::NOT_FOUND,
    };
    routes::with_exit(refusal(r.status, r.code, r.message), code)
}

/// A copy's kit error: a damaged blob keeps its own id (MPC-E705); a cancel
/// mid-copy is MPC-E004; anything else answers as every doc verb does.
fn copy_error(e: &SessionError, cancelled: bool) -> CliResponse {
    if cancelled {
        return export_error(&ExportError::Cancelled);
    }
    match e {
        SessionError::Blob(BlobError::Damaged(_)) => {
            let r = e.report();
            CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: json!({
                    "ok": false,
                    "error": {
                        "code": "blob_damaged",
                        "id": r.id(),
                        "exit": r.exit(),
                        "hint": "the stored version is damaged (its bytes don't match its id): nothing was exported",
                        "next": r.next(),
                    },
                })
                .to_string(),
            }
        }
        other => routes::session_error(other),
    }
}

// ── the doc ─────────────────────────────────────────────────────────────

/// The doc a job reads: live text, or a tier-0 file.
#[derive(Clone)]
enum Doc {
    Live(Arc<Session>),
    Blob(Arc<BlobSession>),
}

impl Doc {
    fn open(rt: &Arc<super::Runtime>, t: &Target) -> Result<Doc, CliResponse> {
        if t.tier == Tier::Blob {
            return super::tier0::open(rt, t).map(Doc::Blob);
        }
        match routes::open(rt, t) {
            Ok(s) => Ok(Doc::Live(s)),
            // A text file its pack refuses, or one stored as any-file, is tier 0.
            Err(SessionError::Declined(_)) => super::tier0::open(rt, t).map(Doc::Blob),
            Err(SessionError::PackMismatch { stored, .. }) if stored == k2_mpc_base::store::BLOB_DOC_PACK => {
                super::tier0::open(rt, t).map(Doc::Blob)
            }
            Err(e) => Err(routes::session_error(&e)),
        }
    }

    /// The kit's source kind: a live doc by its pack, a tier-0 file by its
    /// lower-case extension.
    fn kind(&self, t: &Target) -> Kind {
        match self {
            Doc::Live(_) => Kind::Pack(t.tier.pack().to_string()),
            Doc::Blob(_) => Kind::Format(split_name(file_name(&t.rel)).1.to_ascii_lowercase()),
        }
    }

    fn tier_number(&self) -> u8 {
        match self {
            Doc::Live(_) => 1,
            Doc::Blob(_) => 0,
        }
    }

    /// The kit's verb registry for this doc's tier (P24's rows live there).
    fn registry(&self) -> &'static k2_mpc_base::parity::Registry {
        match self {
            Doc::Live(_) => routes::registry(),
            Doc::Blob(_) => super::tier0::registry(),
        }
    }

    /// `file_at_to` / `file_now_to`, joined as `who`.
    fn copy(&self, who: Participant, version: Option<u64>, out: &mut dyn Write) -> Result<FileCopy, SessionError> {
        match self {
            Doc::Live(s) => {
                let j = s.join_shared(who, Stay::UntilIdle)?;
                match version {
                    Some(v) => s.file_at_to(j.conn, v, out),
                    None => s.file_now_to(j.conn, out),
                }
            }
            Doc::Blob(s) => {
                let j = s.join_shared(who, Stay::UntilIdle)?;
                match version {
                    Some(v) => s.file_at_to(j.conn, v, out),
                    None => s.file_now_to(j.conn, out),
                }
            }
        }
    }
}

/// The kit verb this export runs: `export`, or `export --version` (P24).
pub fn verb_form(version: bool) -> &'static str {
    if version {
        "export --version"
    } else {
        "export"
    }
}

fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// `(stem, extension)`: the last dot that isn't the first character.
/// `notes.md` → (`notes`, `md`); `a.tar.gz` → (`a.tar`, `gz`); `Makefile`,
/// `.gitignore` → (the name, ``).
pub fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    }
}

/// The output's name (X1, X3): `<stem>.v<doc version>.<ext>`, the target's
/// extension or (empty: as-is) the source's own; `n > 1` adds ` (n)`.
pub fn output_name(source: &str, doc_version: u64, target_ext: &str, n: u32) -> String {
    let (stem, src_ext) = split_name(source);
    let ext = if target_ext.is_empty() { src_ext } else { target_ext };
    let mut base = format!("{stem}.v{doc_version}");
    if n > 1 {
        base.push_str(&format!(" ({n})"));
    }
    if ext.is_empty() {
        base
    } else {
        format!("{base}.{ext}")
    }
}

fn codec_str(c: FileCodec) -> &'static str {
    match c {
        FileCodec::Exact => "exact",
        FileCodec::Current => "current",
        FileCodec::ChangedSince => "changed_since",
        _ => "other",
    }
}

// ── placement ───────────────────────────────────────────────────────────

/// Where a job's output goes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Place {
    /// Beside the source (`dir_rel` is its folder, `""` at the root).
    Beside { project_id: String, root: PathBuf, dir_rel: String },
    /// An explicit `out` inside a workspace: exactly this name.
    Out { project_id: String, rel: String, abs: PathBuf },
    /// Kept in the job; the CLI fetches it and writes it itself.
    Fetch,
}

impl Place {
    fn word(&self) -> &'static str {
        match self {
            Place::Beside { .. } => "beside",
            Place::Out { .. } => "out",
            Place::Fetch => "fetch",
        }
    }
}

/// The deny list for an output name (vault, git's folder, K2's own
/// configuration and generated files, a secret's name): `mv`'s rules,
/// minus its tier-kind rule (an export may be any kind of file).
pub fn deny_output(rel: &str) -> Result<(), target::Refusal> {
    let declined = |code: &'static str, message: String| target::Refusal { status: "422 Unprocessable Entity", code, message };
    if target::is_vault(rel) {
        return Err(declined("protected", format!("{rel} is in the vault folder")));
    }
    if target::is_generated(rel) {
        return Err(declined("generated", format!("K2 writes {rel} itself")));
    }
    // Markdown under .k2/ may be a live doc (as for mv); other files there are K2's own.
    if target::is_git_internal(rel) || (Tier::of(rel) != Tier::Markdown && target::is_k2_config(rel)) {
        return Err(declined("protected", format!("{rel} is protected (git's folder or K2's configuration)")));
    }
    if k2_core::secret_paths::is_secret_rel(rel) {
        return Err(declined("secret", format!("{rel} is a secret's name; an export never lands there")));
    }
    Ok(())
}

fn may_create(pid: &str, project_id: &str, rel: &str) -> bool {
    let store = super::runtime().ok().and_then(|rt| rt.store());
    host::rights_for(pid, &DocKey { space: project_id.to_string(), path: rel.to_string() }, store.as_deref()).edit
}

/// `out`: absolute (any folder), or relative to the source's workspace.
/// Inside a workspace the caller must be able to edit there (X4); outside
/// every workspace the CLI writes it (`Fetch`).
fn place_out(out: &str, src: &Target, pid: &str) -> Result<Place, CliResponse> {
    let out = out.trim();
    if out.is_empty() || out.contains('\0') {
        return Err(routes::bad("out is empty"));
    }
    let p = Path::new(out);
    let (project_id, root, rel_in) = if p.is_absolute() {
        let parent = p
            .parent()
            .and_then(|d| d.canonicalize().ok())
            .ok_or_else(|| target_refusal(target::Refusal { status: "404 Not Found", code: "folder_not_found", message: format!("no folder for {out}") }))?;
        let name = p.file_name().ok_or_else(|| routes::bad("out names no file"))?;
        match target::workspace_holding(&parent) {
            Ok((id, root)) => {
                let abs = parent.join(name);
                let rel = abs
                    .strip_prefix(&root)
                    .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/"))
                    .map_err(|_| routes::bad("out escapes its workspace"))?;
                (id, root, rel)
            }
            // Outside every workspace: the CLI writes it with its own rights.
            Err(_) => return Ok(Place::Fetch),
        }
    } else {
        (src.project_id.clone(), src.root.clone(), out.to_string())
    };
    let root_s = root.to_string_lossy().to_string();
    let abs = crate::fs_routes::jail_rel_path(&root_s, &rel_in, true).map_err(|e| {
        target_refusal(target::Refusal { status: "400 Bad Request", code: "path_escapes", message: e })
    })?;
    if std::fs::symlink_metadata(&abs).is_ok() {
        return Err(target_refusal(target::Refusal {
            status: "409 Conflict",
            code: "exists",
            message: format!("{rel_in} already exists; an export never overwrites (name another file)"),
        }));
    }
    let parent = abs
        .parent()
        .filter(|d| d.is_dir())
        .and_then(|d| d.canonicalize().ok())
        .ok_or_else(|| target_refusal(target::Refusal { status: "404 Not Found", code: "folder_not_found", message: format!("no folder for {rel_in}") }))?;
    let abs = parent.join(abs.file_name().unwrap_or_default());
    let rel = abs
        .strip_prefix(&root)
        .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/"))
        .map_err(|_| routes::bad("out escapes its workspace"))?;
    deny_output(&rel).map_err(target_refusal)?;
    if !may_create(pid, &project_id, &rel) {
        return Err(place_forbidden(&rel));
    }
    Ok(Place::Out { project_id, rel, abs })
}

fn beside(src: &Target) -> Place {
    let dir_rel = src.rel.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
    Place::Beside { project_id: src.project_id.clone(), root: src.root.clone(), dir_rel }
}

fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// Rename `from` to `to` only if `to` doesn't exist (X5): `renameat2`
/// (`RENAME_NOREPLACE`) on Linux, `renamex_np` (`RENAME_EXCL`) on macOS,
/// else (or when the filesystem refuses those) `link` + `unlink`, which
/// fails on an existing name too. Never `std::fs::rename` (it replaces).
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let f = std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(io::Error::other)?;
        let t = std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(io::Error::other)?;
        #[cfg(target_os = "linux")]
        // SAFETY: two NUL-terminated paths; renameat2 reads them only.
        let rc = unsafe {
            libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, f.as_ptr(), libc::AT_FDCWD, t.as_ptr(), libc::RENAME_NOREPLACE) as libc::c_int
        };
        #[cfg(target_os = "macos")]
        // SAFETY: as above.
        let rc = unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) };
        if rc == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::ENOTSUP) => {}
            _ => return Err(e),
        }
    }
    std::fs::hard_link(from, to)?;
    std::fs::remove_file(from)
}

// ── jobs ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum State {
    Queued,
    Running { done: u64, total: u64 },
    Done(Value),
    /// The failure's status and body (a `CliResponse`, kept owned).
    Failed(&'static str, String),
}

struct Job {
    id: String,
    owner: String,
    dir: PathBuf,
    cancel: AtomicBool,
    state: Mutex<State>,
    cv: Condvar,
    /// When it finished (for the 1 h keep).
    finished: Mutex<Option<Instant>>,
    /// A fetch job's output and its size.
    fetch: Mutex<Option<(PathBuf, u64)>>,
}

impl Job {
    fn set(&self, s: State) {
        let done = matches!(s, State::Done(_) | State::Failed(..));
        *self.state.lock().expect("job state") = s;
        if done {
            *self.finished.lock().expect("job finished") = Some(Instant::now());
        }
        self.cv.notify_all();
    }

    fn progress(&self, done: u64, total: u64) {
        let mut st = self.state.lock().expect("job state");
        if matches!(*st, State::Queued | State::Running { .. }) {
            *st = State::Running { done, total };
        }
    }

    /// Wait up to `d` for the end; the state then.
    fn wait(&self, d: Duration) -> State {
        let deadline = Instant::now() + d;
        let mut st = self.state.lock().expect("job state");
        loop {
            if matches!(*st, State::Done(_) | State::Failed(..)) {
                return st.clone();
            }
            let now = Instant::now();
            if now >= deadline {
                return st.clone();
            }
            st = self.cv.wait_timeout(st, deadline - now).expect("job state").0;
        }
    }
}

fn jobs() -> &'static Mutex<HashMap<String, Arc<Job>>> {
    static J: OnceLock<Mutex<HashMap<String, Arc<Job>>>> = OnceLock::new();
    J.get_or_init(|| Mutex::new(HashMap::new()))
}

fn new_job_id() -> String {
    let mut b = [0u8; 16];
    if getrandom::getrandom(&mut b).is_err() {
        // Never a guessable id: a v4 UUID's randomness instead.
        b = *uuid::Uuid::new_v4().as_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A job id is 32 hex characters (never a path).
fn valid_job_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Drop finished jobs past [`KEEP`] (and a fetch job's bytes).
fn prune() {
    let mut j = jobs().lock().expect("export jobs");
    let old: Vec<String> = j
        .iter()
        .filter(|(_, job)| job.finished.lock().ok().and_then(|f| *f).is_some_and(|t| t.elapsed() > KEEP))
        .map(|(k, _)| k.clone())
        .collect();
    for k in old {
        if let Some(job) = j.remove(&k) {
            let _ = std::fs::remove_dir_all(&job.dir);
        }
    }
}

fn find_job(id: &str, caller: &DocCaller) -> Option<Arc<Job>> {
    prune();
    let j = jobs().lock().expect("export jobs");
    let job = j.get(id)?.clone();
    // Another pid's job is "not found" (no oracle), except for the box owner.
    let pid = caller.pid();
    (job.owner == pid || pid == format!("{}{}", host::PERSON_PREFIX, k2_core::notify_prefs::OWNER)).then_some(job)
}

/// At docs start and on the upkeep sweep: job folders this daemon doesn't
/// know (a previous run's) older than an hour go, with the temp output
/// they name (`dest`), only if that name matches `.*.k2x-<job>.part`.
pub fn sweep_leftovers() {
    prune();
    let root = export_worker::jobs_root();
    let Ok(rd) = std::fs::read_dir(&root) else { return };
    let known: Vec<String> = jobs().lock().map(|j| j.keys().cloned().collect()).unwrap_or_default();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !valid_job_id(&name) || known.contains(&name) {
            continue;
        }
        let age = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| SystemTime::now().duration_since(t).ok());
        if age.is_none_or(|a| a < KEEP) {
            continue;
        }
        if let Ok(dest) = std::fs::read_to_string(e.path().join("dest")) {
            let dest = PathBuf::from(dest.trim());
            let ok_name = dest
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .is_some_and(|n| n.starts_with('.') && n.ends_with(&format!("{PART_TAG}{name}.part")));
            if ok_name {
                let _ = std::fs::remove_file(&dest);
            }
        }
        let _ = std::fs::remove_dir_all(e.path());
        log_debug!("[docs] removed a leftover export job folder ({name})");
    }
}

// ── routes ──────────────────────────────────────────────────────────────

/// `status`, `cancel`, `fetch`: the caller's own jobs (the box owner sees all).
pub(super) fn job_route(path: &str, caller: &DocCaller, input: &routes::Input<'_>) -> CliResponse {
    let Some(id) = input.str("job").map(|j| j.trim().to_string()).filter(|j| !j.is_empty()) else {
        return routes::bad("job is required: the id k2 doc export printed");
    };
    if !valid_job_id(&id) {
        return job_not_found();
    }
    let Some(job) = find_job(&id, caller) else { return job_not_found() };
    match path {
        STATUS => state_response(&job, &job.state.lock().expect("job state").clone()),
        CANCEL => {
            let st = job.state.lock().expect("job state").clone();
            let running = matches!(st, State::Queued | State::Running { .. });
            if running {
                job.cancel.store(true, Ordering::Release);
                job.cv.notify_all();
            }
            routes::ok(json!({ "job": job.id, "cancelled": running, "state": state_word(&st) }))
        }
        FETCH => fetch(&job, input),
        _ => CliResponse::not_found(),
    }
}

fn state_word(s: &State) -> &'static str {
    match s {
        State::Queued => "queued",
        State::Running { .. } => "running",
        State::Done(_) => "done",
        State::Failed(..) => "failed",
    }
}

fn state_response(job: &Job, s: &State) -> CliResponse {
    match s {
        State::Done(v) => routes::ok(v.clone()),
        State::Failed(status, body) => CliResponse { status: *status, content_type: "application/json", body: body.clone() },
        State::Queued => routes::ok(json!({ "job": job.id, "state": "queued", "done": 0, "total": 0 })),
        State::Running { done, total } => routes::ok(json!({ "job": job.id, "state": "running", "done": done, "total": total })),
    }
}

fn fetch(job: &Job, input: &routes::Input<'_>) -> CliResponse {
    let Some((path, size)) = job.fetch.lock().expect("job fetch").clone() else {
        return routes::with_exit(
            refusal("409 Conflict", "export_not_fetchable", "this job's output was placed in a workspace (or it isn't done); nothing to fetch"),
            exit::USAGE,
        );
    };
    let offset = match input.u64("offset") {
        Ok(o) => o.unwrap_or(0),
        Err(r) => return r,
    };
    let len = match input.u64("len") {
        Ok(l) => l.unwrap_or(FETCH_CHUNK).clamp(1, FETCH_CHUNK),
        Err(r) => return r,
    };
    if offset > size {
        return routes::bad(format!("offset {offset} is past the end ({size} bytes)"));
    }
    let n = len.min(size - offset);
    let mut buf = vec![0u8; n as usize];
    let read = std::fs::File::open(&path).and_then(|mut f| {
        use std::io::Seek;
        f.seek(io::SeekFrom::Start(offset))?;
        f.read_exact(&mut buf)
    });
    if let Err(e) = read {
        return export_error(&ExportError::Io(e));
    }
    routes::ok(json!({
        "job": job.id,
        "offset": offset,
        "size": size,
        "bytesB64": routes::b64(&buf),
        "eof": offset + n >= size,
    }))
}

/// One target as `--list` shows it, with the extension the output gets.
fn list_row(mut t: Value, src_ext: &str, runs: &str) -> Value {
    let ext = t.get("extension").and_then(Value::as_str).unwrap_or("").to_string();
    t["extension"] = json!(if ext.is_empty() { src_ext.to_string() } else { ext });
    t["runs"] = json!(runs);
    t["available"] = json!(true);
    t
}

/// `export` and `export/targets`, after the caller, target and read checks.
pub(super) fn doc_route(path: &str, rt: &Arc<super::Runtime>, c: &Call) -> CliResponse {
    let doc = match Doc::open(rt, c.target) {
        Ok(d) => d,
        Err(r) => return r,
    };
    if path == TARGETS {
        return targets_route(&doc, c);
    }
    let started = Instant::now();
    let pid = c.caller.pid();
    let audit_refusal = |r: CliResponse, version: Option<u64>, target: &str| -> CliResponse {
        let code = serde_json::from_str::<Value>(&r.body)
            .ok()
            .and_then(|v| v["error"]["code"].as_str().map(str::to_string))
            .unwrap_or_else(|| "error".into());
        audit(c.target, &pid, version, target, "-", 0, "none", started, &code);
        r
    };
    let target_id = match c.input.str("target").map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(t) => t,
        None => return routes::bad("target is required (k2 doc export --list <file> shows them)"),
    };
    let version = match c.input.u64("version") {
        Ok(v) => v,
        Err(r) => return r,
    };
    // The kit's export verb for this tier (P24): what it needs.
    let form = verb_form(version.is_some());
    let Some(spec) = doc.registry().get(form) else {
        return audit_refusal(
            routes::with_exit(
                refusal("400 Bad Request", "not_a_doc_verb", format!("`{form}` isn't a verb for {} (tier {})", c.target.rel, doc.tier_number())),
                exit::USAGE,
            ),
            version,
            &target_id,
        );
    };
    if spec.needs.contains(&Need::History) && !c.rights.history {
        return audit_refusal(
            routes::with_exit(
                refusal("403 Forbidden", "history_forbidden", "you may read this doc but not its history: export it now (no --version)"),
                exit::REFUSED,
            ),
            version,
            &target_id,
        );
    }
    let options = match parse_options(c.input) {
        Ok(o) => o,
        Err(r) => return audit_refusal(r, version, &target_id),
    };
    let kind = doc.kind(c.target);
    let (exporter, target_ext, in_worker) = match pick_target(&kind, &target_id, &options) {
        Ok(x) => x,
        Err(r) => return audit_refusal(r, version, &target_id),
    };
    let place = if c.input.flag("fetch") {
        Place::Fetch
    } else {
        match c.input.str("out").filter(|o| !o.trim().is_empty()) {
            Some(out) => match place_out(&out, c.target, &pid) {
                Ok(p) => p,
                Err(r) => return audit_refusal(r, version, &target_id),
            },
            None => {
                let p = beside(c.target);
                // The name isn't known before the copy (it carries the doc
                // version): check the folder with a provisional one now, the
                // final name again at placement.
                let dir_rel = match &p {
                    Place::Beside { dir_rel, .. } => dir_rel.clone(),
                    _ => String::new(),
                };
                let probe = join_rel(&dir_rel, &output_name(file_name(&c.target.rel), 0, &target_ext, 1));
                if let Err(r) = deny_output(&probe) {
                    return audit_refusal(target_refusal(r), version, &target_id);
                }
                if !may_create(&pid, &c.target.project_id, &probe) {
                    return audit_refusal(place_forbidden(&probe), version, &target_id);
                }
                p
            }
        }
    };
    let Some(ticket) = export_worker::enqueue(&pid) else {
        let r = CliResponse {
            status: "429 Too Many Requests",
            content_type: "application/json",
            body: json!({
                "ok": false,
                "error": {
                    "code": "export_busy",
                    "hint": "too many exports are running or waiting on this server; try again shortly",
                    "retryAfterMs": 2000,
                    "exit": exit::USAGE,
                },
            })
            .to_string(),
        };
        return audit_refusal(r, version, &target_id);
    };
    let id = new_job_id();
    let job = Arc::new(Job {
        id: id.clone(),
        owner: pid.clone(),
        dir: export_worker::jobs_root().join(&id),
        cancel: AtomicBool::new(false),
        state: Mutex::new(State::Queued),
        cv: Condvar::new(),
        finished: Mutex::new(None),
        fetch: Mutex::new(None),
    });
    jobs().lock().expect("export jobs").insert(id.clone(), job.clone());
    let spec = JobSpec {
        rt: rt.clone(),
        doc,
        who: c.caller.participant(c.on_behalf),
        pid,
        target: c.target.clone(),
        rights: c.rights,
        version,
        target_id,
        exporter,
        target_ext,
        in_worker,
        options,
        kind,
        place,
        started,
    };
    let runner = job.clone();
    let spawned = std::thread::Builder::new().name(format!("k2-export-{}", &id[..8])).spawn(move || {
        let slot = export_worker::wait_slot(ticket, &runner.cancel);
        let res = match slot {
            Some(_slot) => run_job(&runner, &spec),
            None => {
                audit(&spec.target, &spec.pid, spec.version, &spec.target_id, &spec.exporter, 0, "none", spec.started, "export_cancelled");
                Err(export_error(&ExportError::Cancelled))
            }
        };
        match res {
            Ok(v) => runner.set(State::Done(v)),
            Err(r) => {
                let _ = std::fs::remove_dir_all(&runner.dir);
                runner.set(State::Failed(r.status, r.body));
            }
        }
    });
    if let Err(e) = spawned {
        jobs().lock().expect("export jobs").remove(&id);
        return refusal("500 Internal Server Error", "export_io", format!("couldn't start the export: {e}"));
    }
    let wait = c
        .input
        .u64("waitMs")
        .ok()
        .flatten()
        .map(Duration::from_millis)
        .unwrap_or(MAX_WAIT)
        .min(MAX_WAIT);
    state_response(&job, &job.wait(wait))
}

/// `options`: a list of `k=v` strings, or an object.
fn parse_options(input: &routes::Input<'_>) -> Result<Vec<(String, String)>, CliResponse> {
    let pairs: Vec<String> = match input.body.get("options") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| routes::bad("options must be a list of NAME=VALUE strings"))?,
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| v.as_str().map(|s| format!("{k}={s}")))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| routes::bad("options values must be strings"))?,
        Some(_) => return Err(routes::bad("options must be a list of NAME=VALUE strings")),
    };
    let parsed = Options::parse(pairs.iter().map(String::as_str)).map_err(|e| export_error(&e))?;
    Ok(parsed.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
}

/// The target's exporter id, its extension, and whether it runs in the
/// worker. Unknown names and options are refused before any copy.
fn pick_target(kind: &Kind, target: &str, options: &[(String, String)]) -> Result<(String, String, bool), CliResponse> {
    let daemon = ExportRegistry::with_as_is();
    let kit_kind = kind.kit();
    if target == AS_IS {
        let (exporter, t) = daemon
            .targets_for(&kit_kind)
            .into_iter()
            .find(|(_, t)| t.id == AS_IS)
            .expect("the kit's registry offers as-is for every kind");
        if let Some((name, _)) = options.first() {
            return Err(export_error(&ExportError::Declined { code: "bad_option", message: format!("{AS_IS} has no option {name:?}") }));
        }
        let worker = export_worker::as_is_in_worker();
        return Ok((exporter.to_string(), t.extension.to_string(), worker));
    }
    let targets = match export_worker::targets(kind) {
        Ok((_, t)) => t,
        Err(why) => return Err(worker_unavailable(&why)),
    };
    let Some(t) = targets.iter().find(|t| t["id"] == target) else {
        return Err(export_error(&ExportError::Unsupported { kind: kind_text(kind), target: target.to_string() }));
    };
    let names: Vec<&str> = t["options"].as_array().map(|a| a.iter().filter_map(|o| o["name"].as_str()).collect()).unwrap_or_default();
    if let Some((name, _)) = options.iter().find(|(k, _)| !names.contains(&k.as_str())) {
        return Err(export_error(&ExportError::Declined { code: "bad_option", message: format!("{target} has no option {name:?}") }));
    }
    Ok((
        t["exporter"].as_str().unwrap_or("k2-export").to_string(),
        t["extension"].as_str().unwrap_or("").to_string(),
        true,
    ))
}

fn kind_text(k: &Kind) -> String {
    match k {
        Kind::Pack(p) => format!("a {p} doc"),
        Kind::Format(f) if f.is_empty() => "a file".to_string(),
        Kind::Format(f) => format!("a {f} file"),
    }
}

fn targets_route(doc: &Doc, c: &Call) -> CliResponse {
    let t = c.target;
    let kind = doc.kind(t);
    let src_ext = split_name(file_name(&t.rel)).1.to_string();
    let mut rows: Vec<Value> = ExportRegistry::with_as_is()
        .targets_for(&kind.kit())
        .iter()
        .map(|(id, tg)| list_row(k2_export::proto::target_json(id, tg), &src_ext, "daemon"))
        .collect();
    let worker = match export_worker::targets(&kind) {
        Ok((hello, wt)) => {
            for tg in wt {
                if !rows.iter().any(|r| r["id"] == tg["id"]) {
                    rows.push(list_row(tg, &src_ext, "worker"));
                }
            }
            json!({ "available": true, "version": hello.version })
        }
        Err(why) => json!({ "available": false, "reason": why }),
    };
    routes::ok(json!({
        "doc": routes::doc_json(t),
        "tier": doc.tier_number(),
        "kind": kind.json(),
        "targets": rows,
        "worker": worker,
        "rights": c.rights.json(),
    }))
}

// ── one job ─────────────────────────────────────────────────────────────

struct JobSpec {
    rt: Arc<super::Runtime>,
    doc: Doc,
    who: Participant,
    pid: String,
    target: Target,
    rights: DocRights,
    version: Option<u64>,
    target_id: String,
    exporter: String,
    target_ext: String,
    in_worker: bool,
    options: Vec<(String, String)>,
    kind: Kind,
    place: Place,
    started: Instant,
}

/// A writer that stops (an error) once the job is cancelled.
struct CancelWriter<'a, W: Write> {
    w: W,
    cancel: &'a AtomicBool,
}

impl<W: Write> Write for CancelWriter<'_, W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Acquire) {
            return Err(io::Error::other("export cancelled"));
        }
        self.w.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

/// The output file, as the kit's sink.
struct FileSink<'a> {
    f: &'a mut std::fs::File,
}

impl Write for FileSink<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.f.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.f.flush()
    }
}

impl Sink for FileSink<'_> {
    fn finish(self: Box<Self>) -> io::Result<()> {
        self.f.sync_all()
    }
}

fn create_private_dir(d: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(d)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(d)
    }
}

/// A new file that must not exist; `private`: 0600 (job files), else the
/// umask's default (a placed export).
fn create_new(p: &Path, private: bool) -> io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    o.open(p)
}

fn run_job(job: &Job, s: &JobSpec) -> Result<Value, CliResponse> {
    let fail = |r: CliResponse, bytes: u64| -> Result<Value, CliResponse> {
        let code = serde_json::from_str::<Value>(&r.body)
            .ok()
            .and_then(|v| v["error"]["code"].as_str().map(str::to_string))
            .unwrap_or_else(|| "error".into());
        audit(&s.target, &s.pid, s.version, &s.target_id, &s.exporter, bytes, "none", s.started, &code);
        Err(r)
    };
    job.progress(0, 0);
    if let Err(e) = create_private_dir(&job.dir) {
        return fail(export_error(&ExportError::Io(e)), 0);
    }
    // 1. The source version's verified bytes, into the job's input. Partial
    // output of a failed copy is discarded here, before any exporter.
    let input = job.dir.join("in");
    let copied = (|| -> Result<FileCopy, CliResponse> {
        for attempt in 0..2 {
            let f = create_new(&input, true).map_err(|e| export_error(&ExportError::Io(e)))?;
            let mut w = CancelWriter { w: io::BufWriter::with_capacity(256 << 10, f), cancel: &job.cancel };
            let doc = if attempt == 0 {
                s.doc.clone()
            } else {
                Doc::open(&s.rt, &s.target)?
            };
            match doc.copy(s.who.clone(), s.version, &mut w) {
                Ok(c) => {
                    let f = w.w.into_inner().map_err(|e| export_error(&ExportError::Io(e.into_error())))?;
                    f.sync_all().map_err(|e| export_error(&ExportError::Io(e)))?;
                    return Ok(c);
                }
                // The pool unloaded the doc between open and the copy: once more.
                Err(SessionError::Unloaded) if attempt == 0 => {
                    drop(w);
                    let _ = std::fs::remove_file(&input);
                }
                Err(e) => {
                    drop(w);
                    let _ = std::fs::remove_file(&input);
                    return Err(copy_error(&e, job.cancel.load(Ordering::Acquire)));
                }
            }
        }
        Err(routes::session_error(&SessionError::Unloaded))
    })();
    let copy = match copied {
        Ok(c) => c,
        Err(r) => return fail(r, 0),
    };
    let size = copy.bytes;
    job.progress(0, size);

    // 2. The output's final name and its temp file beside it.
    let src_file = file_name(&s.target.rel).to_string();
    let first = output_name(&src_file, copy.version, &s.target_ext, 1);
    let (part, out_dir) = match &s.place {
        Place::Beside { project_id, root, dir_rel } => {
            let rel = join_rel(dir_rel, &first);
            if let Err(r) = deny_output(&rel) {
                return fail(target_refusal(r), 0);
            }
            if !may_create(&s.pid, project_id, &rel) {
                return fail(place_forbidden(&rel), 0);
            }
            let dir = if dir_rel.is_empty() { root.clone() } else { root.join(dir_rel) };
            (dir.join(format!(".{first}{PART_TAG}{}.part", job.id)), Some(dir))
        }
        Place::Out { abs, .. } => {
            let dir = abs.parent().map(Path::to_path_buf).unwrap_or_default();
            let name = abs.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            (dir.join(format!(".{name}{PART_TAG}{}.part", job.id)), Some(dir))
        }
        Place::Fetch => (job.dir.join("out"), None),
    };
    // The boot sweep finds a crashed job's temp output by this.
    let _ = std::fs::write(job.dir.join("dest"), part.to_string_lossy().as_bytes());
    let mut out = match create_new(&part, out_dir.is_none()) {
        Ok(f) => f,
        Err(e) => return fail(export_error(&ExportError::Io(e)), 0),
    };

    // 3. The export: as-is in this thread (X2), everything else in the worker.
    let limits = export_worker::limits();
    let progress = |d: u64, t: u64| job.progress(d, t);
    let result: Result<(Value, u64), CliResponse> = if s.in_worker {
        run_in_worker(job, s, &input, size, &limits, &progress, &mut out)
    } else {
        run_in_thread(job, s, &input, size, &limits, &progress, &mut out)
    };
    let (report, bytes) = match result.and_then(|r| out.sync_all().map(|_| r).map_err(|e| export_error(&ExportError::Io(e)))) {
        Ok(r) => r,
        Err(r) => {
            drop(out);
            let _ = std::fs::remove_file(&part);
            let _ = std::fs::remove_file(&input);
            return fail(r, 0);
        }
    };
    drop(out);
    let _ = std::fs::remove_file(&input);

    // 4. Place it: a no-replace rename, only now.
    let placed: Option<(String, String, PathBuf)> = match &s.place {
        Place::Beside { project_id, dir_rel, .. } => {
            let dir = out_dir.clone().unwrap_or_default();
            let mut n = 1u32;
            loop {
                let name = output_name(&src_file, copy.version, &s.target_ext, n);
                match rename_noreplace(&part, &dir.join(&name)) {
                    Ok(()) => break Some((project_id.clone(), join_rel(dir_rel, &name), dir.join(&name))),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < 1000 => n += 1,
                    Err(e) => {
                        let _ = std::fs::remove_file(&part);
                        return fail(export_error(&ExportError::Io(e)), bytes);
                    }
                }
            }
        }
        Place::Out { project_id, rel, abs } => match rename_noreplace(&part, abs) {
            Ok(()) => Some((project_id.clone(), rel.clone(), abs.clone())),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                if e.kind() == io::ErrorKind::AlreadyExists {
                    return fail(
                        target_refusal(target::Refusal {
                            status: "409 Conflict",
                            code: "exists",
                            message: format!("{rel} appeared while exporting; an export never overwrites (name another file)"),
                        }),
                        bytes,
                    );
                }
                return fail(export_error(&ExportError::Io(e)), bytes);
            }
        },
        Place::Fetch => {
            *job.fetch.lock().expect("job fetch") = Some((part.clone(), bytes));
            None
        }
    };
    if !matches!(s.place, Place::Fetch) {
        let _ = std::fs::remove_dir_all(&job.dir);
    }
    audit(&s.target, &s.pid, s.version, &s.target_id, &s.exporter, bytes, s.place.word(), s.started, "ok");
    let name = placed
        .as_ref()
        .map(|(_, rel, _)| file_name(rel).to_string())
        .unwrap_or_else(|| output_name(&src_file, copy.version, &s.target_ext, 1));
    Ok(json!({
        "job": job.id,
        "state": "done",
        "doc": routes::doc_json(&s.target),
        "tier": s.doc.tier_number(),
        "target": s.target_id,
        "exporter": s.exporter,
        // X1: the version id asked for (null: now), and the doc version the
        // bytes are (the name carries it).
        "version": s.version,
        "docVersion": copy.version,
        "codec": codec_str(copy.codec),
        "size": bytes,
        "name": name,
        "placed": placed.map(|(ws, rel, abs)| json!({ "workspace": ws, "path": rel, "abs": abs.to_string_lossy() })),
        "fetch": matches!(s.place, Place::Fetch),
        "report": report,
        "rights": s.rights.json(),
    }))
}

fn run_in_thread(
    job: &Job,
    s: &JobSpec,
    input: &Path,
    size: u64,
    limits: &Limits,
    progress: &dyn Fn(u64, u64),
    out: &mut std::fs::File,
) -> Result<(Value, u64), CliResponse> {
    let mut f = std::fs::File::open(input).map_err(|e| export_error(&ExportError::Io(e)))?;
    let mut read = io::BufReader::with_capacity(256 << 10, &mut f);
    let opts = {
        let mut o = Options::new();
        for (k, v) in &s.options {
            o.set(k, v);
        }
        o
    };
    let ctl = ExportCtl { cancel: &job.cancel, progress, limits: limits.kit(), resources: &NoResources };
    let mut sink = FileSink { f: out };
    let report = ExportRegistry::with_as_is()
        .export(&s.kind.kit(), Source::Bytes { read: &mut read, size, name: &s.target.rel }, &s.target_id, &opts, &mut sink, &ctl)
        .map_err(|e| export_error(&e))?;
    Ok((k2_export::worker::report_json(&report), report.bytes))
}

fn run_in_worker(
    job: &Job,
    s: &JobSpec,
    input: &Path,
    size: u64,
    limits: &Limits,
    progress: &dyn Fn(u64, u64),
    out: &mut std::fs::File,
) -> Result<(Value, u64), CliResponse> {
    let bin = export_worker::binary().map_err(|why| worker_unavailable(&why))?;
    let header = Header::Export {
        job: job.id.clone(),
        kind: s.kind.clone(),
        target: s.target_id.clone(),
        options: s.options.clone(),
        limits: *limits,
        name: s.target.rel.clone(),
        size,
    };
    let spawn = k2_export::host::Spawn { bin, cwd: job.dir.clone() };
    let mut w = io::BufWriter::with_capacity(256 << 10, &mut *out);
    let res = k2_export::host::run(&spawn, &header, Some((input, size)), export_worker::bounds(limits), &job.cancel, progress, &mut w);
    let flushed = w.flush();
    drop(w);
    match res {
        Ok(done) => {
            if !done.stderr.trim().is_empty() {
                log_debug!("[docs] export {} worker stderr: {}", &job.id[..8], done.stderr.trim());
            }
            flushed.map_err(|e| export_error(&ExportError::Io(e)))?;
            Ok((done.report, done.bytes))
        }
        Err(f) => {
            log_debug!("[docs] export {} failed in the worker: {:?} (stderr: {})", &job.id[..8], f.error, f.stderr.trim());
            Err(run_error(f.error))
        }
    }
}

/// One `doc-exported` line: metadata only, paths workspace-relative.
#[allow(clippy::too_many_arguments)]
fn audit(t: &Target, pid: &str, version: Option<u64>, target: &str, exporter: &str, bytes: u64, placed: &str, started: Instant, result: &str) {
    host::audit(
        AUDIT_EXPORTED,
        pid,
        format!(
            "{}:{} v={} target={target} exporter={exporter} bytes={bytes} placed={placed} ms={} result={result}",
            t.project_id,
            t.rel,
            version.map(|v| v.to_string()).unwrap_or_else(|| "now".into()),
            started.elapsed().as_millis()
        ),
    );
}

/// For `routes::handle`: an export refusal before the doc opened (rights,
/// history) is audited too.
pub(super) fn audit_early_refusal(t: Option<&Target>, pid: &str, input: &routes::Input<'_>, r: &CliResponse) {
    let code = serde_json::from_str::<Value>(&r.body)
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(str::to_string))
        .unwrap_or_else(|| "error".into());
    let label = t.map(|t| format!("{}:{}", t.project_id, t.rel)).unwrap_or_else(|| "-".into());
    host::audit(
        AUDIT_EXPORTED,
        pid,
        format!(
            "{label} v={} target={} exporter=- bytes=0 placed=none ms=0 result={code}",
            input.str("version").unwrap_or_else(|| "now".into()),
            input.str("target").unwrap_or_default()
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_names_carry_the_doc_version_and_the_right_extension() {
        assert_eq!(output_name("notes.md", 17, "", 1), "notes.v17.md");
        assert_eq!(output_name("notes.md", 17, "", 2), "notes.v17 (2).md");
        assert_eq!(output_name("notes.md", 17, "html", 1), "notes.v17.html");
        // X3: as-is keeps the source's extension; none stays none.
        assert_eq!(output_name("Makefile", 4, "", 1), "Makefile.v4");
        assert_eq!(output_name("a.tar.gz", 4, "", 1), "a.tar.v4.gz");
        assert_eq!(output_name(".gitignore", 3, "", 1), ".gitignore.v3");
        assert_eq!(output_name("photo.PNG", 2, "", 3), "photo.v2 (3).PNG");
    }

    #[test]
    fn the_deny_list_guards_output_names_without_the_tier_rule() {
        for (rel, code) in [
            (".k2/vault/x.md", "protected"),
            (".git/hooks/pre-commit", "protected"),
            (".k2/settings.v3.json", "protected"),
            (".k2/AGENTS.md", "generated"),
            (".env", "secret"),
        ] {
            assert_eq!(deny_output(rel).expect_err(rel).code, code, "{rel}");
        }
        // Any kind of file may be an export (no tier-kind rule), in the doc dirs too.
        for rel in ["notes.v3.md", "photo.v2.png", ".k2/wiki/Home.v4.md", ".k2/prds/a.v1.pdf"] {
            deny_output(rel).unwrap_or_else(|e| panic!("{rel}: {e:?}"));
        }
    }

    #[test]
    fn a_no_replace_rename_never_overwrites() {
        let d = std::env::temp_dir().join(format!("k2-export-rn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).expect("dir");
        let (a, b) = (d.join("a.part"), d.join("b"));
        std::fs::write(&a, b"new").expect("a");
        std::fs::write(&b, b"old").expect("b");
        let e = rename_noreplace(&a, &b).expect_err("b exists");
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "{e}");
        assert_eq!(std::fs::read(&b).expect("b"), b"old", "the existing file is untouched");
        assert!(a.exists(), "the temp file is still there to delete");
        rename_noreplace(&a, &d.join("c")).expect("c is free");
        assert_eq!(std::fs::read(d.join("c")).expect("c"), b"new");
        assert!(!a.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn job_ids_are_128_bit_hex_and_never_paths() {
        let id = new_job_id();
        assert!(valid_job_id(&id), "{id}");
        assert_ne!(id, new_job_id());
        for bad in ["../x", "", "ABCDEF0123456789ABCDEF0123456789", &"a".repeat(33)] {
            assert!(!valid_job_id(bad), "{bad}");
        }
    }

    #[test]
    fn the_kits_export_ids_map_to_k2_statuses_and_exits() {
        // vs-live §6: (error, HTTP, exit).
        let cases: Vec<(ExportError, &str, &str, u8)> = vec![
            (ExportError::Unsupported { kind: "a png file".into(), target: "pdf".into() }, "export_unsupported", "400", 2),
            (ExportError::Declined { code: "bad_option", message: "x".into() }, "export_declined", "400", 2),
            (ExportError::Limit("memory"), "export_limit", "413", 2),
            (ExportError::Cancelled, "export_cancelled", "409", 1),
            (ExportError::Decode("x".into()), "export_decode", "422", 1),
            (ExportError::Io(io::Error::other("x")), "export_io", "500", 1),
            (ExportError::Internal { exporter: "e".into(), message: "x".into() }, "export_internal", "500", 1),
        ];
        for (e, code, status, ex) in cases {
            let r = export_error(&e);
            assert!(r.status.starts_with(status), "{code}: {}", r.status);
            let v: Value = serde_json::from_str(&r.body).expect("json");
            assert_eq!(v["error"]["code"], json!(code));
            assert_eq!(v["error"]["exit"], json!(ex), "{code}");
            assert!(v["error"]["id"].as_str().is_some_and(|i| i.starts_with("MPC-E00")), "{v}");
        }
        // A worker kill maps the same way.
        use k2_export::host::RunError as R;
        let v: Value = serde_json::from_str(&run_error(R::Limit("time")).body).expect("json");
        assert_eq!((v["error"]["code"].as_str(), v["error"]["details"]["limit"].as_str()), (Some("export_limit"), Some("time")));
        let v: Value =
            serde_json::from_str(&run_error(R::Internal { worker: "signal 11".into(), message: "crash".into() }).body).expect("json");
        assert_eq!((v["error"]["id"].as_str(), v["error"]["details"]["worker"].as_str()), (Some("MPC-E007"), Some("signal 11")));
        let r = run_error(R::Spawn("gone".into()));
        assert!(r.status.starts_with("503"), "{}", r.status);
    }

    #[test]
    fn the_kits_export_verb_is_on_both_tiers_and_needs_history_only_with_a_version() {
        for reg in [routes::registry(), super::super::tier0::registry()] {
            let now = reg.get(verb_form(false)).expect("export");
            assert!(now.needs.contains(&Need::Read) && !now.needs.contains(&Need::History), "{:?}", now.needs);
            let past = reg.get(verb_form(true)).expect("export --version");
            assert!(past.needs.contains(&Need::History), "{:?}", past.needs);
        }
    }
}
