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

use std::io::Read;
use std::path::{Path, PathBuf};

use k2_node_proto::crypto;
use k2_node_proto::frames::{ArtifactChunk, ArtifactFailed, ArtifactGet, Frame, TRANSFER_CHUNK};
use k2_node_proto::results::{self, ArtifactInfo, Outputs, TestSummary};

use crate::paths::{safe_relative, Layout};

/// Where to look.
pub struct Roots<'a> {
    /// The job's working directory (source root + plan cwd, or the job dir).
    pub cwd: &'a Path,
    /// The job's `CARGO_TARGET_DIR` (warm slot or job-private).
    pub target: Option<&'a Path>,
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
            notes.push(format!("{g} matched nothing"));
        }
        for h in hits {
            if !out.iter().any(|o| o.name == h.name) {
                out.push(h);
            }
        }
    }
    out
}

/// `artifacts/<job>-g<gen>/` under the node home.
pub fn dir(layout: &Layout, job: &str, generation: u32) -> PathBuf {
    layout.home.join("artifacts").join(format!("{job}-g{generation}"))
}

/// Copy `src` to `dst` and hash it (one read).
fn copy_hash(src: &Path, dst: &Path) -> Result<(String, u64), String> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("create {}: {e}", p.display()))?;
    }
    let mut f = std::fs::File::open(src).map_err(|e| format!("open {}: {e}", src.display()))?;
    let mut w = std::fs::File::create(dst).map_err(|e| format!("create {}: {e}", dst.display()))?;
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
    Outputs { artifacts, tests, notes }.capped()
}

/// Remove artifact dirs older than [`results::KEEP_SECS`].
pub fn sweep(layout: &Layout, now: i64) {
    let root = layout.home.join("artifacts");
    let Ok(rd) = std::fs::read_dir(&root) else { return };
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

/// Answer one `artifact_get`: stream chunks (or one `artifact_failed`)
/// through `send`, which returns false once the session is gone. The
/// chunks carry the hash the node reported; the controller checks the
/// bytes it got against it. `known` is the attempt's reported artifacts
/// (from the ledger). Blocking.
pub fn answer(layout: &Layout, g: &ArtifactGet, known: Option<&[ArtifactInfo]>, send: &mut dyn FnMut(Frame) -> bool) {
    let failed = |code: &str, message: String| Frame::ArtifactFailed(ArtifactFailed { request_id: g.request_id.clone(), code: code.into(), message });
    macro_rules! fail {
        ($code:expr, $msg:expr) => {{
            send(failed($code, $msg));
            return;
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
    let path = dir(layout, &g.job_id, g.generation).join(rel);
    let mut f = match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_file() => match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => fail!("artifact_unreadable", format!("open: {e}")),
        },
        _ => fail!("artifact_gone", "the node no longer keeps it (artifacts are kept 7 days)".into()),
    };
    let total = f.metadata().map(|m| m.len()).unwrap_or(0);
    if total != info.bytes {
        fail!("artifact_changed", "the kept file no longer matches its size".into());
    }
    let mut buf = vec![0u8; TRANSFER_CHUNK];
    let mut seq = 0u32;
    let mut sent = 0u64;
    loop {
        // Fill the buffer (a short read isn't the end of the file).
        let mut n = 0;
        while n < buf.len() {
            match f.read(&mut buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) => fail!("artifact_unreadable", format!("read: {e}")),
            }
        }
        sent += n as u64;
        let last = sent >= total || n < buf.len();
        let ok = send(Frame::ArtifactChunk(ArtifactChunk {
            request_id: g.request_id.clone(),
            seq,
            data: crypto::b64(&buf[..n]),
            last,
            total_bytes: total,
            sha256: info.sha256.clone(),
        }));
        if !ok || last {
            return;
        }
        seq += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            &Roots { cwd: &cwd, target: Some(&target) },
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
    }

    #[test]
    fn answer_streams_chunks_and_refuses_unknown_or_changed_files() {
        let d = crate::util::temp_dir("artifacts-answer");
        let layout = Layout::new(d.join("home"), d.join("config"));
        let keep = dir(&layout, "j1", 1);
        let body = vec![7u8; TRANSFER_CHUNK + 10];
        std::fs::create_dir_all(keep.join("out")).unwrap();
        std::fs::write(keep.join("out/a.bin"), &body).unwrap();
        let info = ArtifactInfo { name: "out/a.bin".into(), sha256: crypto::sha256_hex(&body), bytes: body.len() as u64 };
        let g = |name: &str| ArtifactGet { request_id: "r".into(), job_id: "j1".into(), generation: 1, name: name.into() };
        let run = |name: &str, known: Option<&[ArtifactInfo]>| {
            let mut frames = Vec::new();
            answer(&layout, &g(name), known, &mut |f| {
                frames.push(f);
                true
            });
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
}
