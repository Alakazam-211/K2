//! Each deployment's own log file, kept by the engine outside the
//! container's storage.
//!
//! Quadlet runs every container with `--rm` and removes it again after the
//! unit stops, and Podman's default `k8s-file` log lives in the container's
//! own folder: an App that exited on its first start (a missing secret
//! file, a bad flag) took its only output with it, and `k2 app logs`
//! showed nothing. So the unit's log goes to
//! `<state>/logs/<app>/<deployment>.log` (`--log-opt=path=`, still capped
//! by `max-size`), written by conmon from the App's first byte of stdout
//! and stderr, and kept after the container is gone: the `logs` verb
//! reads it for a running, an exited and a failed start alike.
//!
//! The newest [`KEEP_PER_APP`] files of an App stay (a crash loop's starts
//! each get one); the rest go at the next start, and all of them with the
//! App (`remove-app`).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use k2_root_proto::apps::AppId;

/// Deployment logs kept per App (newest first by change time).
pub const KEEP_PER_APP: usize = 16;

/// The most of a log's end the `logs` verb reads (the tail it answers is
/// cut to the wire's output cap after that).
const READ_TAIL_BYTES: u64 = 8 << 20;

pub fn dir(state: &Path, app: &AppId) -> PathBuf {
    state.join("logs").join(app.as_str())
}

pub fn path(state: &Path, app: &AppId, deployment: &str) -> PathBuf {
    dir(state, app).join(format!("{deployment}.log"))
}

/// Make the App's log folder (0700) and drop its oldest logs past
/// [`KEEP_PER_APP`], never `deployment`'s own. Returns its log's path.
pub fn prepare(state: &Path, app: &AppId, deployment: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let d = dir(state, app);
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
    let own = format!("{deployment}.log");
    prune(&d, KEEP_PER_APP.saturating_sub(1), &own);
    Ok(d.join(own))
}

/// Keep the newest `keep` `*.log` files in `d` besides `except`.
pub fn prune(d: &Path, keep: usize, except: &str) {
    let Ok(rd) = std::fs::read_dir(d) else { return };
    let mut logs: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.ends_with(".log") && n != except
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    logs.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in logs.into_iter().skip(keep) {
        let _ = std::fs::remove_file(p);
    }
}

pub fn remove_app(state: &Path, app: &AppId) {
    let _ = std::fs::remove_dir_all(dir(state, app));
}

/// The last `lines` lines of the App's output in a `k8s-file` log, cut to
/// `max_bytes` from the end. `None` when there is no such file (a
/// deployment an older engine started logs in the container's own folder).
pub fn read_tail(p: &Path, lines: usize, max_bytes: usize) -> Option<String> {
    let mut f = std::fs::File::open(p).ok()?;
    let len = f.metadata().ok()?.len();
    let from = len.saturating_sub(READ_TAIL_BYTES);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut raw = Vec::new();
    f.read_to_end(&mut raw).ok()?;
    let mut text = String::from_utf8_lossy(&raw).to_string();
    if from > 0 {
        // The first line read is a cut one.
        text = text.split_once('\n').map(|(_, rest)| rest.to_string()).unwrap_or_default();
    }
    Some(tail(&k8s_text(&text), lines, max_bytes))
}

/// The App's own text from `k8s-file` lines
/// (`<time> <stdout|stderr> <F|P> <text>`; `P` = a partial line that the
/// next one continues). A line in no such shape is kept as it is.
pub fn k8s_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for l in raw.lines() {
        let mut it = l.splitn(4, ' ');
        match (it.next(), it.next(), it.next(), it.next()) {
            (Some(_), Some("stdout" | "stderr"), Some(tag @ ("F" | "P")), text) => {
                out.push_str(text.unwrap_or(""));
                if tag == "F" {
                    out.push('\n');
                }
            }
            _ => {
                out.push_str(l);
                out.push('\n');
            }
        }
    }
    out
}

/// The last `lines` lines of `text`, then its last `max_bytes` bytes (on a
/// character boundary).
pub fn tail(text: &str, lines: usize, max_bytes: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let mut s = all[all.len().saturating_sub(lines)..].join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    if s.len() > max_bytes {
        let mut cut = s.len() - max_bytes;
        while !s.is_char_boundary(cut) {
            cut += 1;
        }
        s = s[cut..].to_string();
    }
    s
}

#[cfg(test)]
#[path = "app_logs_tests.rs"]
mod tests;
