//! Artifacts and test results (PRD §11.4; research G1/G2).
//!
//! After a job's command exits, the plan's `--artifact` globs are matched
//! (relative to the job's working directory; `target/…` also reads the
//! node's cargo target dir, since the node moves cargo's target into a
//! warm slot), copied into `artifacts/<job>-g<gen>/`, hashed, and JUnit
//! reports among them are parsed into a test summary. The controller
//! pulls a file only when someone fetches it (`artifact_get`). Kept
//! [`results::KEEP_SECS`] (7 days).
//!
//! Symlinks are never followed or kept: a job can't make the node read
//! outside its own tree.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use k2_node_proto::crypto;
use k2_node_proto::frames::{ArtifactChunk, ArtifactFailed, ArtifactGet, Frame, TRANSFER_CHUNK};
use k2_node_proto::results::{self, ArtifactInfo, Outputs, TestSummary};

use crate::paths::safe_relative;

/// Where to look.
pub struct Roots<'a> {
    /// The job's working directory (source root + plan cwd, or the job dir).
    pub cwd: &'a Path,
    /// The job's `CARGO_TARGET_DIR` (warm slot or job-private).
    pub target: Option<&'a Path>,
    /// How the job's working directory reads in a note (`<job>/src/sub`,
    /// CE-23); `None`: the note names no folder (older wording).
    pub cwd_label: Option<&'a str>,
}

/// CE-23: the no-match note names the folders searched, and that a glob
/// is relative to the job's working directory, not a folder its command
/// `cd`s into.
pub fn no_match_note(glob: &str, cwd_label: Option<&str>, target_too: bool) -> String {
    match cwd_label {
        None => format!("{glob} matched nothing"),
        Some(l) => {
            let also = if target_too { " or in the node's cargo target dir" } else { "" };
            format!("{glob} matched nothing under {l}{also} (globs are relative to the job's working directory, not a folder its command cd's into)")
        }
    }
}

/// One matched file before it is kept.
struct Hit {
    name: String,
    path: PathBuf,
    bytes: u64,
}

/// Walk `dir` for the glob `segs`, adding files to `out` as `prefix/…`.
fn walk(dir: &Path, prefix: &str, segs: &[&str], out: &mut Vec<Hit>, visits: &mut usize) {
    if *visits >= results::MAX_VISITS {
        return;
    }
    let Some((seg, rest)) = segs.split_first() else { return };
    let join = |p: &str, n: &str| if p.is_empty() { n.to_string() } else { format!("{p}/{n}") };
    if *seg == "**" {
        // Zero directories, then each subdirectory with `**` still in front.
        walk(dir, prefix, rest, out, visits);
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            *visits += 1;
            if *visits >= results::MAX_VISITS {
                return;
            }
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if ft.is_dir() && !name.starts_with('.') {
                walk(&e.path(), &join(prefix, &name), segs, out, visits);
            }
        }
        return;
    }
    if results::is_literal(seg) {
        let p = dir.join(seg);
        *visits += 1;
        let Ok(m) = std::fs::symlink_metadata(&p) else { return };
        if rest.is_empty() {
            if m.file_type().is_file() {
                out.push(Hit { name: join(prefix, seg), path: p, bytes: m.len() });
            }
        } else if m.file_type().is_dir() {
            walk(&p, &join(prefix, seg), rest, out, visits);
        }
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        *visits += 1;
        if *visits >= results::MAX_VISITS {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if !results::segment_match(seg, &name) {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if rest.is_empty() {
            if ft.is_file() {
                let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(Hit { name: join(prefix, &name), path: e.path(), bytes });
            }
        } else if ft.is_dir() {
            walk(&e.path(), &join(prefix, &name), rest, out, visits);
        }
    }
}

/// Match every glob; files in glob order, each name once.
fn matches(globs: &[String], roots: &Roots<'_>, notes: &mut Vec<String>) -> Vec<Hit> {
    let mut out: Vec<Hit> = Vec::new();
    for g in globs {
        if let Some(code) = results::glob_refusal(g) {
            notes.push(format!("{code}: {g}"));
            continue;
        }
        let segs = results::segments(g);
        let mut hits = Vec::new();
        let mut visits = 0usize;
        walk(roots.cwd, "", &segs, &mut hits, &mut visits);
        if segs.first() == Some(&"target") {
            if let Some(t) = roots.target.filter(|t| *t != roots.cwd.join("target")) {
                walk(t, "target", &segs[1..], &mut hits, &mut visits);
            }
        }
        if visits >= results::MAX_VISITS {
            notes.push(format!("{g}: stopped after {} directory entries", results::MAX_VISITS));
        }
        if hits.is_empty() {
            let target_too = segs.first() == Some(&"target") && roots.target.is_some();
            notes.push(no_match_note(g, roots.cwd_label, target_too));
        }
        for h in hits {
            if !out.iter().any(|o| o.name == h.name) {
                out.push(h);
            }
        }
    }
    out
}

/// `<root>/<job>-g<gen>/`, `root` being the tenant's artifacts dir
/// ([`crate::paths::TenantLayout::artifacts`]: `artifacts/` under the home
/// for the owner).
pub fn dir(root: &Path, job: &str, generation: u32) -> PathBuf {
    root.join(format!("{job}-g{generation}"))
}

/// Copy `src` to `dst` and hash it (one read).
fn copy_hash(src: &Path, dst: &Path) -> Result<(String, u64), String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    // Kept artifacts are the node's alone (0700 dirs, 0600 files): the
    // owner's `artifacts/` sits in the node home every owner-slot job
    // (another workspace's included) can traverse (review B-H1).
    if let Some(p) = dst.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(p).map_err(|e| format!("create {}: {e}", p.display()))?;
    }
    let mut f = std::fs::File::open(src).map_err(|e| format!("open {}: {e}", src.display()))?;
    let mut w = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(dst)
        .map_err(|e| format!("create {}: {e}", dst.display()))?;
    let mut h = crypto::Sha256Stream::default();
    let mut buf = vec![0u8; 1 << 16];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("read {}: {e}", src.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        std::io::Write::write_all(&mut w, &buf[..n]).map_err(|e| format!("write {}: {e}", dst.display()))?;
        n_total += n as u64;
    }
    Ok((h.finish_hex(), n_total))
}

/// Collect the plan's artifacts and parse JUnit among them. Blocking.
pub fn collect(globs: &[String], roots: &Roots<'_>, keep: &Path) -> Outputs {
    let mut notes = Vec::new();
    let hits = matches(globs, roots, &mut notes);
    let _ = std::fs::remove_dir_all(keep);
    let mut artifacts: Vec<ArtifactInfo> = Vec::new();
    let mut tests: Option<TestSummary> = None;
    let mut total = 0u64;
    for h in hits {
        if artifacts.len() >= results::MAX_FILES {
            notes.push(format!("kept the first {} files", results::MAX_FILES));
            break;
        }
        if h.bytes > results::MAX_FILE_BYTES {
            notes.push(format!("{}: {} MB is over the {} MB per-file cap", h.name, h.bytes >> 20, results::MAX_FILE_BYTES >> 20));
            continue;
        }
        if total + h.bytes > results::MAX_TOTAL_BYTES {
            notes.push(format!("{}: over the {} GB per-job cap", h.name, results::MAX_TOTAL_BYTES >> 30));
            continue;
        }
        let Some(rel) = safe_relative(&h.name) else {
            notes.push(format!("{}: not a plain relative name", h.name));
            continue;
        };
        let dst = keep.join(rel);
        match copy_hash(&h.path, &dst) {
            Ok((sha, bytes)) => {
                total += bytes;
                if h.name.ends_with(".xml") && bytes <= results::MAX_JUNIT_BYTES {
                    if let Ok(text) = std::fs::read(&dst) {
                        if results::looks_like_junit(&text) {
                            match results::parse_junit(&String::from_utf8_lossy(&text), &h.name) {
                                Ok(t) => tests.get_or_insert_with(TestSummary::default).absorb(&t),
                                Err(e) => notes.push(format!("JUnit: {e}")),
                            }
                        }
                    }
                }
                artifacts.push(ArtifactInfo { name: h.name, sha256: sha, bytes });
            }
            Err(e) => notes.push(e),
        }
    }
    Outputs { artifacts, tests, notes, ..Default::default() }.capped()
}

/// Remove artifact dirs under `root` older than [`results::KEEP_SECS`].
pub fn sweep(root: &Path, now: i64) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|d| now - d.as_secs() as i64 > results::KEEP_SECS);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// `artifact_delete` (protocol 9, V27): remove one attempt's kept artifacts
/// under `root` (the asking server's own tenant dir) when `known` (its
/// ledger has the attempt) and `job` is one plain name. True when a dir was
/// there and is gone now. Blocking.
pub fn delete(root: &Path, job: &str, generation: u32, known: bool) -> bool {
    let plain = !job.is_empty() && job.len() <= 128 && job.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !known || !plain {
        return false;
    }
    let d = dir(root, job, generation);
    match std::fs::symlink_metadata(&d) {
        Ok(m) if m.file_type().is_dir() => std::fs::remove_dir_all(&d).is_ok(),
        _ => false,
    }
}

/// Most pulls one session paces at once (a controller asks for one file
/// at a time per fetch; this only bounds a confused one).
const MAX_FLOWS: usize = 64;

/// GH#80 (`artifact-flow`): the pulls one session is sending, each paced
/// by the controller's `artifact_ack`s and stopped by its
/// `artifact_cancel`. The session's reader feeds it; [`answer`]'s
/// blocking thread waits on it. Closed when the session ends.
#[derive(Default)]
pub struct Flows {
    m: Mutex<FlowMap>,
    cv: Condvar,
}

#[derive(Default)]
struct FlowMap {
    closed: bool,
    reqs: HashMap<String, Flow>,
}

struct Flow {
    /// The controller holds the file up to here.
    next: u64,
    cancelled: bool,
    /// The last ack that moved `next` (or the start).
    moved: Instant,
}

/// Why a paced pull stopped before its last chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Cancelled,
    SessionGone,
    Stalled,
    Busy,
}

impl Stop {
    fn words(self) -> &'static str {
        match self {
            Stop::Cancelled => "cancelled by the server",
            Stop::SessionGone => "session ended",
            Stop::Stalled => "no ack from the server",
            Stop::Busy => "too many pulls at once",
        }
    }
}

impl Flows {
    fn lock(&self) -> std::sync::MutexGuard<'_, FlowMap> {
        self.m.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Start pacing `id`, the controller holding the file up to `from`.
    pub fn open(&self, id: &str, from: u64) -> Result<(), Stop> {
        let mut m = self.lock();
        if m.closed {
            return Err(Stop::SessionGone);
        }
        if m.reqs.len() >= MAX_FLOWS && !m.reqs.contains_key(id) {
            return Err(Stop::Busy);
        }
        m.reqs.insert(id.to_string(), Flow { next: from, cancelled: false, moved: Instant::now() });
        Ok(())
    }

    /// `artifact_ack`: the controller holds `id` up to `next`.
    pub fn ack(&self, id: &str, next: u64) {
        let mut m = self.lock();
        if let Some(f) = m.reqs.get_mut(id) {
            if next > f.next {
                f.next = next;
                f.moved = Instant::now();
            }
        }
        drop(m);
        self.cv.notify_all();
    }

    /// `artifact_cancel`: stop sending `id`.
    pub fn cancel(&self, id: &str) {
        if let Some(f) = self.lock().reqs.get_mut(id) {
            f.cancelled = true;
        }
        self.cv.notify_all();
    }

    /// The session ended: every paced pull stops.
    pub fn close(&self) {
        self.lock().closed = true;
        self.cv.notify_all();
    }

    /// `id` is done (sent, failed or stopped).
    pub fn done(&self, id: &str) {
        self.lock().reqs.remove(id);
    }

    /// Block until the byte at `pos` may go: `pos < next + window`. Stops
    /// on a cancel, the session's end, or `stall` with no ack moving.
    pub fn wait_room(&self, id: &str, pos: u64, window: u64, stall: Duration) -> Result<(), Stop> {
        let mut m = self.lock();
        loop {
            if m.closed {
                return Err(Stop::SessionGone);
            }
            let Some(f) = m.reqs.get(id) else { return Err(Stop::Cancelled) };
            if f.cancelled {
                return Err(Stop::Cancelled);
            }
            if pos < f.next.saturating_add(window) {
                return Ok(());
            }
            let quiet = f.moved.elapsed();
            if quiet >= stall {
                return Err(Stop::Stalled);
            }
            m = self.cv.wait_timeout(m, stall - quiet).map(|(g, _)| g).unwrap_or_else(|p| p.into_inner().0);
        }
    }
}

/// How one `artifact_get` went, for the node's log (one line per pull).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    /// Where the pull started (a resumed pull: past 0).
    pub offset: u64,
    /// Bytes this pull put on the wire.
    pub sent: u64,
    /// The file's size (0 when the node never opened it).
    pub total: u64,
    /// `sent`, `failed <code>`, or why it stopped.
    pub outcome: String,
}

/// Answer one `artifact_get`: stream chunks (or one `artifact_failed`)
/// through `send`, which returns false once the session is gone. The
/// chunks carry the hash the node reported; the controller checks the
/// bytes it got against it. `known` is the attempt's reported artifacts
/// (from the ledger). A get with `window` is paced by `flows` (GH#80):
/// at most `window` bytes past the controller's last ack, stopped by a
/// cancel or `stall` without an ack. One with `offset` starts there.
/// Blocking.
pub fn answer(
    root: &Path,
    g: &ArtifactGet,
    known: Option<&[ArtifactInfo]>,
    send: &mut dyn FnMut(Frame) -> bool,
    flows: Option<(&Flows, Duration)>,
) -> Answered {
    let offset = g.offset.unwrap_or(0);
    let paced = g.window.zip(flows).map(|(w, (f, stall))| (w.max(TRANSFER_CHUNK as u64), f, stall));
    let r = answer_inner(root, g, known, send, offset, paced);
    if let Some((_, f, _)) = paced {
        f.done(&g.request_id);
    }
    r
}

fn answer_inner(
    root: &Path,
    g: &ArtifactGet,
    known: Option<&[ArtifactInfo]>,
    send: &mut dyn FnMut(Frame) -> bool,
    offset: u64,
    paced: Option<(u64, &Flows, Duration)>,
) -> Answered {
    let failed = |code: &str, message: String| Frame::ArtifactFailed(ArtifactFailed { request_id: g.request_id.clone(), code: code.into(), message });
    let mut out = Answered { offset, sent: 0, total: 0, outcome: String::new() };
    macro_rules! fail {
        ($code:expr, $msg:expr) => {{
            send(failed($code, $msg));
            out.outcome = format!("failed {}", $code);
            return out;
        }};
    }
    let Some(known) = known else {
        fail!("job_not_found", "this node has no record of that attempt".into());
    };
    let Some(info) = known.iter().find(|a| a.name == g.name) else {
        fail!("artifact_not_found", format!("the job kept no artifact named {}", g.name));
    };
    let Some(rel) = safe_relative(&info.name) else {
        fail!("artifact_not_found", "not a plain relative name".into());
    };
    let path = dir(root, &g.job_id, g.generation).join(rel);
    let mut f = match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_file() => match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => fail!("artifact_unreadable", format!("open: {e}")),
        },
        _ => fail!("artifact_gone", "the node no longer keeps it (artifacts are kept 7 days)".into()),
    };
    let total = f.metadata().map(|m| m.len()).unwrap_or(0);
    out.total = total;
    if total != info.bytes {
        fail!("artifact_changed", "the kept file no longer matches its size".into());
    }
    if offset > total {
        fail!("artifact_offset", format!("byte {offset} is past the end of the file ({total} bytes)"));
    }
    if offset > 0 {
        if let Err(e) = std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(offset)) {
            fail!("artifact_unreadable", format!("seek: {e}"));
        }
    }
    if let Some((_, flows, _)) = paced {
        match flows.open(&g.request_id, offset) {
            Ok(()) => {}
            // Said, so the server doesn't wait out its timeout for nothing.
            Err(Stop::Busy) => fail!("artifact_busy", "this node is sending too many files to this server at once; try again".into()),
            Err(stop) => {
                out.outcome = stop.words().to_string();
                return out;
            }
        }
    }
    let mut buf = vec![0u8; TRANSFER_CHUNK];
    let mut seq = 0u32;
    let mut pos = offset;
    loop {
        if let Some((window, flows, stall)) = paced {
            if let Err(stop) = flows.wait_room(&g.request_id, pos, window, stall) {
                out.outcome = stop.words().to_string();
                return out;
            }
        }
        // Fill the buffer (a short read isn't the end of the file).
        let mut n = 0;
        while n < buf.len() {
            match f.read(&mut buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) => fail!("artifact_unreadable", format!("read: {e}")),
            }
        }
        pos += n as u64;
        let last = pos >= total || n < buf.len();
        let ok = send(Frame::ArtifactChunk(ArtifactChunk {
            request_id: g.request_id.clone(),
            seq,
            data: crypto::b64(&buf[..n]),
            last,
            total_bytes: total,
            sha256: info.sha256.clone(),
        }));
        if !ok {
            out.outcome = Stop::SessionGone.words().to_string();
            return out;
        }
        out.sent += n as u64;
        if last {
            out.outcome = "sent".into();
            return out;
        }
        seq += 1;
    }
}

/// The node's one log line for a pull (GH#80): job, file, where it
/// started, bytes sent and how it ended.
pub fn log_line(g: &ArtifactGet, a: &Answered) -> String {
    let from = if a.offset > 0 { format!(" from byte {}", a.offset) } else { String::new() };
    let paced = if g.window.is_some() { "paced" } else { "unpaced" };
    format!("artifact {} g{} {}{from}: sent {} of {} bytes ({paced}), {}", g.job_id, g.generation, g.name, a.sent, a.total, a.outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Layout;

    fn touch(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn globs_read_the_worktree_and_the_moved_target_dir() {
        let d = crate::util::temp_dir("artifacts");
        let cwd = d.join("src");
        let target = d.join("slot");
        touch(&cwd.join("dist/k2.tar.gz"), "tar");
        touch(&cwd.join("dist/sub/no.tar.gz"), "no");
        touch(&cwd.join(".hidden/x.tar.gz"), "no");
        touch(
            &target.join("nextest/ci/junit.xml"),
            r#"<testsuites><testsuite name="k"><testcase name="ok"/><testcase name="bad"><failure message="boom"/></testcase></testsuite></testsuites>"#,
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hosts", cwd.join("dist/link.tar.gz")).unwrap();
        let keep = d.join("keep");
        let o = collect(
            &["dist/*.tar.gz".into(), "target/nextest/**/junit.xml".into(), "nothing/*".into(), "../escape".into()],
            &Roots { cwd: &cwd, target: Some(&target), cwd_label: None },
            &keep,
        );
        let names: Vec<&str> = o.artifacts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["dist/k2.tar.gz", "target/nextest/ci/junit.xml"], "symlinks and subdirs never match: {o:?}");
        assert_eq!(o.artifacts[0].sha256, crypto::sha256_hex(b"tar"));
        assert_eq!(o.artifacts[0].bytes, 3);
        assert!(keep.join("target/nextest/ci/junit.xml").is_file());
        let t = o.tests.expect("junit parsed");
        assert_eq!((t.total, t.passed, t.failed), (2, 1, 1));
        assert_eq!(t.failing[0].name, "bad");
        assert!(o.notes.iter().any(|n| n == "nothing/* matched nothing"), "{:?}", o.notes);
        assert!(o.notes.iter().any(|n| n.starts_with("artifact_glob_escapes")), "{:?}", o.notes);
        // CE-23: with a label the note says where it looked.
        let o2 = collect(&["out/*.json".into(), "target/x/*.xml".into()], &Roots { cwd: &cwd, target: Some(&target), cwd_label: Some("<job>/src/sub") }, &d.join("keep2"));
        assert_eq!(
            o2.notes,
            vec![
                "out/*.json matched nothing under <job>/src/sub (globs are relative to the job's working directory, not a folder its command cd's into)".to_string(),
                "target/x/*.xml matched nothing under <job>/src/sub or in the node's cargo target dir (globs are relative to the job's working directory, not a folder its command cd's into)".to_string(),
            ]
        );
        assert!(o2.notes.iter().all(|n| n.chars().count() <= 300), "fits the note cap");
        // Review B-H1: kept files are the node's alone.
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&keep.join("target/nextest/ci/junit.xml")), 0o600);
        assert_eq!(mode(&keep.join("target/nextest/ci")), 0o700);
        assert_eq!(mode(&keep.join("dist/k2.tar.gz")), 0o600);
    }

    #[test]
    fn answer_streams_chunks_and_refuses_unknown_or_changed_files() {
        let d = crate::util::temp_dir("artifacts-answer");
        let layout = Layout::new(d.join("home"), d.join("config"));
        let keep = dir(&layout.tenant(0).artifacts(), "j1", 1);
        let body = vec![7u8; TRANSFER_CHUNK + 10];
        std::fs::create_dir_all(keep.join("out")).unwrap();
        std::fs::write(keep.join("out/a.bin"), &body).unwrap();
        let info = ArtifactInfo { name: "out/a.bin".into(), sha256: crypto::sha256_hex(&body), bytes: body.len() as u64 };
        let g = |name: &str| ArtifactGet { request_id: "r".into(), job_id: "j1".into(), generation: 1, name: name.into(), offset: None, window: None };
        let run = |name: &str, known: Option<&[ArtifactInfo]>| {
            let mut frames = Vec::new();
            answer(
                &layout.tenant(0).artifacts(),
                &g(name),
                known,
                &mut |f| {
                    frames.push(f);
                    true
                },
                None,
            );
            frames
        };
        let frames = run("out/a.bin", Some(std::slice::from_ref(&info)));
        assert_eq!(frames.len(), 2);
        let mut got = Vec::new();
        for f in &frames {
            let Frame::ArtifactChunk(c) = f else { panic!("{f:?}") };
            assert_eq!(c.sha256, info.sha256);
            got.extend(crypto::unb64(&c.data).unwrap());
        }
        assert_eq!(got, body);
        assert!(matches!(&frames[1], Frame::ArtifactChunk(c) if c.last && c.seq == 1));
        let code = |fs: Vec<Frame>| match &fs[0] {
            Frame::ArtifactFailed(f) => f.code.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(code(run("other", Some(std::slice::from_ref(&info)))), "artifact_not_found");
        assert_eq!(code(run("out/a.bin", None)), "job_not_found");
        std::fs::write(keep.join("out/a.bin"), b"changed").unwrap();
        assert_eq!(code(run("out/a.bin", Some(std::slice::from_ref(&info)))), "artifact_changed");
        // An empty file is one empty last chunk.
        std::fs::write(keep.join("out/e"), b"").unwrap();
        let e = ArtifactInfo { name: "out/e".into(), sha256: crypto::sha256_hex(b""), bytes: 0 };
        let fs = run("out/e", Some(std::slice::from_ref(&e)));
        assert!(matches!(&fs[..], [Frame::ArtifactChunk(c)] if c.last && c.data.is_empty()), "{fs:?}");
    }

    /// A kept file of `chunks` full chunks plus 10 bytes, its info, and the
    /// artifacts root.
    fn kept(tag: &str, chunks: usize) -> (PathBuf, ArtifactInfo, Vec<u8>) {
        let d = crate::util::temp_dir(tag);
        let root = Layout::new(d.join("home"), d.join("config")).tenant(0).artifacts();
        let body: Vec<u8> = (0..TRANSFER_CHUNK * chunks + 10).map(|i| (i % 251) as u8).collect();
        let keep = dir(&root, "j1", 1);
        std::fs::create_dir_all(keep.join("out")).unwrap();
        std::fs::write(keep.join("out/a.bin"), &body).unwrap();
        let info = ArtifactInfo { name: "out/a.bin".into(), sha256: crypto::sha256_hex(&body), bytes: body.len() as u64 };
        (root, info, body)
    }

    fn get(offset: Option<u64>, window: Option<u64>) -> ArtifactGet {
        ArtifactGet { request_id: "r".into(), job_id: "j1".into(), generation: 1, name: "out/a.bin".into(), offset, window }
    }

    /// Run `answer` on its own thread (a paced one blocks); the chunks it
    /// sends come out of the receiver.
    fn spawn_answer(
        root: PathBuf,
        g: ArtifactGet,
        info: ArtifactInfo,
        flows: std::sync::Arc<Flows>,
        stall: Duration,
    ) -> (std::sync::mpsc::Receiver<Frame>, std::thread::JoinHandle<Answered>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let h = std::thread::spawn(move || answer(&root, &g, Some(std::slice::from_ref(&info)), &mut |f| tx.send(f).is_ok(), Some((&*flows, stall))));
        (rx, h)
    }

    fn chunk_of(f: Frame) -> ArtifactChunk {
        match f {
            Frame::ArtifactChunk(c) => c,
            other => panic!("{other:?}"),
        }
    }

    /// GH#80: with `window` the node sends no more than that past the
    /// controller's last ack, goes on as acks come, and stops at a cancel.
    /// Before, it queued the whole file at once and nothing could stop it:
    /// every retry over a slow relay added another full copy ahead of the
    /// next one.
    #[test]
    fn a_paced_pull_waits_for_acks_and_stops_on_cancel() {
        let (root, info, body) = kept("artifacts-paced", 4);
        let flows = std::sync::Arc::new(Flows::default());
        let window = TRANSFER_CHUNK as u64;
        let (rx, h) = spawn_answer(root, get(None, Some(window)), info, flows.clone(), Duration::from_secs(30));
        let first = chunk_of(rx.recv_timeout(Duration::from_secs(5)).expect("the first chunk"));
        assert_eq!((first.seq, first.last), (0, false));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "nothing past the window before an ack");
        flows.ack("r", window);
        let second = chunk_of(rx.recv_timeout(Duration::from_secs(5)).expect("the next chunk after the ack"));
        assert_eq!(second.seq, 1);
        assert_eq!(crypto::unb64(&second.data).unwrap(), body[TRANSFER_CHUNK..2 * TRANSFER_CHUNK]);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "and waits again");
        flows.cancel("r");
        let a = h.join().unwrap();
        assert_eq!((a.sent, a.total, a.outcome.as_str()), (2 * TRANSFER_CHUNK as u64, body.len() as u64, "cancelled by the server"));
        assert!(rx.try_recv().is_err(), "nothing after the cancel");
        assert!(flows.lock().reqs.is_empty(), "the pull is forgotten");
    }

    /// GH#80: a paced pull acked all the way runs to its last chunk; one
    /// the controller stops acking ends after the stall time; one whose
    /// session ends stops at once.
    #[test]
    fn a_paced_pull_ends_sent_stalled_or_with_its_session() {
        let (root, info, body) = kept("artifacts-paced-end", 2);
        let flows = std::sync::Arc::new(Flows::default());
        let (rx, h) = spawn_answer(root.clone(), get(None, Some(TRANSFER_CHUNK as u64)), info.clone(), flows.clone(), Duration::from_secs(30));
        let mut got = Vec::new();
        loop {
            let c = chunk_of(rx.recv_timeout(Duration::from_secs(5)).expect("a chunk"));
            got.extend(crypto::unb64(&c.data).unwrap());
            flows.ack("r", got.len() as u64);
            if c.last {
                break;
            }
        }
        assert_eq!(got, body);
        let a = h.join().unwrap();
        assert_eq!((a.sent, a.outcome.as_str()), (body.len() as u64, "sent"));

        let (rx, h) = spawn_answer(root.clone(), get(None, Some(TRANSFER_CHUNK as u64)), info.clone(), flows.clone(), Duration::from_millis(300));
        chunk_of(rx.recv_timeout(Duration::from_secs(5)).expect("the first chunk"));
        let a = h.join().unwrap();
        assert_eq!((a.sent, a.outcome.as_str()), (TRANSFER_CHUNK as u64, "no ack from the server"));

        let (rx, h) = spawn_answer(root, get(None, Some(TRANSFER_CHUNK as u64)), info, flows.clone(), Duration::from_secs(30));
        chunk_of(rx.recv_timeout(Duration::from_secs(5)).expect("the first chunk"));
        flows.close();
        assert_eq!(h.join().unwrap().outcome, "session ended");
        assert_eq!(flows.open("other", 0), Err(Stop::SessionGone), "a closed session paces nothing new");
    }

    /// GH#80: `offset` resumes a pull: the bytes from there, `seq` from 0,
    /// the last chunk marked; past the end is refused.
    #[test]
    fn a_pull_with_an_offset_sends_the_rest_only() {
        let (root, info, body) = kept("artifacts-offset", 2);
        let from = TRANSFER_CHUNK as u64 + 5;
        let mut frames = Vec::new();
        let a = answer(
            &root,
            &get(Some(from), None),
            Some(std::slice::from_ref(&info)),
            &mut |f| {
                frames.push(f);
                true
            },
            None,
        );
        let chunks: Vec<ArtifactChunk> = frames.into_iter().map(chunk_of).collect();
        assert_eq!(chunks.iter().map(|c| c.seq).collect::<Vec<_>>(), vec![0, 1]);
        assert!(chunks[1].last && !chunks[0].last);
        let got: Vec<u8> = chunks.iter().flat_map(|c| crypto::unb64(&c.data).unwrap()).collect();
        assert_eq!(got, body[from as usize..]);
        assert_eq!((a.offset, a.sent, a.outcome.as_str()), (from, body.len() as u64 - from, "sent"));
        assert_eq!(
            log_line(&get(Some(from), None), &a),
            format!("artifact j1 g1 out/a.bin from byte {from}: sent {} of {} bytes (unpaced), sent", a.sent, body.len())
        );

        let mut frames = Vec::new();
        let a = answer(
            &root,
            &get(Some(body.len() as u64 + 1), None),
            Some(std::slice::from_ref(&info)),
            &mut |f| {
                frames.push(f);
                true
            },
            None,
        );
        assert!(matches!(&frames[..], [Frame::ArtifactFailed(f)] if f.code == "artifact_offset"), "{frames:?}");
        assert_eq!(a.outcome, "failed artifact_offset");
    }

    /// V27: a fetched secret job's attempt is deleted, that attempt only,
    /// only when the ledger knows it, never outside the tenant's dir.
    #[test]
    fn delete_removes_one_known_attempt_only() {
        let d = crate::util::temp_dir("artifacts-delete");
        let layout = Layout::new(d.join("home"), d.join("config"));
        let root = layout.tenant(1).artifacts();
        for (job, gen) in [("j1", 1), ("j1", 2), ("j2", 1)] {
            let keep = dir(&root, job, gen);
            std::fs::create_dir_all(keep.join("dist")).unwrap();
            std::fs::write(keep.join("dist/app.tar.gz"), b"baked-in").unwrap();
        }
        let outside = d.join("home").join("precious");
        std::fs::create_dir_all(&outside).unwrap();
        assert!(!delete(&root, "j1", 1, false), "an attempt the ledger doesn't know");
        assert!(dir(&root, "j1", 1).is_dir());
        for bad in ["", "../precious", "j1/../../precious", "a b"] {
            assert!(!delete(&root, bad, 1, true), "{bad:?}");
        }
        assert!(outside.is_dir());
        assert!(delete(&root, "j1", 1, true));
        assert!(!dir(&root, "j1", 1).exists());
        assert!(dir(&root, "j1", 2).is_dir() && dir(&root, "j2", 1).is_dir(), "other attempts stay");
        assert!(!delete(&root, "j1", 1, true), "already gone");
    }
}
