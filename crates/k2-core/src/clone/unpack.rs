//! Remote-side unpack: extract a clone bundle at recomputed paths and
//! register + configure the migrated workspace.
//!
//! Runs on the DESTINATION machine. The source bundle stored every file
//! under a per-class prefix (`workspace/`, `memory/`, `sessions/`) so the
//! unpack can fan files back out WITHOUT re-deriving the class:
//!
//! - `workspace/<rel>` → `<dest_path>/<rel>`
//! - `memory/<rel>`    → `<remote-home>/.claude/projects/<remote-slug>/memory/<rel>`
//! - `sessions/<rel>`  → `<remote-home>/.claude/projects/<remote-slug-rerooted>/<rel>`
//!
//! `dest_path = <dest_parent>/<source-name>` (collision-safe `name (1)`).
//! The remote slug is recomputed PURELY from the final dest path
//! (`claude_project_hash`) — the source slug is never trusted. Session rel
//! paths keep their `<source-slug>[-branch]/<id>.jsonl` shape, so we
//! re-root the leading `<source-slug>` segment onto the recomputed remote
//! slug, preserving any `-<branch>` worktree suffix.

use super::CloneManifest;
use crate::chat_history::claude_project_hash;
use std::path::{Path, PathBuf};

/// Result of an unpack: where the workspace landed + the recomputed slug.
#[derive(Debug, Clone)]
pub struct UnpackResult {
    /// Final workspace path on this (remote) machine.
    pub dest_path: PathBuf,
    /// The recomputed slug `claude_project_hash(dest_path)`.
    pub remote_slug: String,
    /// Source workspace name (the basename used for the dest folder).
    pub source_name: String,
}

/// Extract a clone bundle at `<dest_parent>/<source-name>` (collision-safe)
/// and place memory/sessions under the recomputed remote slug dir. Does NOT
/// touch the DB — registration + settings live in the daemon route so this
/// stays a pure filesystem op (the manifest is returned for the caller to
/// apply settings).
///
/// `home` is the remote home dir (`dirs::home_dir()` in production, a temp
/// dir in tests) under which `.claude/projects/<slug>/` is rooted.
pub fn unpack_bundle(
    bundle_path: &Path,
    dest_parent: &Path,
    home: &Path,
) -> Result<(UnpackResult, CloneManifest), String> {
    let manifest = super::read_manifest_from_bundle(bundle_path)?;

    // ── source name = basename of the source project path ─────────────
    let source_name = Path::new(&manifest.source_project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "workspace".to_string());

    // ── collision-safe dest path ──────────────────────────────────────
    let dest_path = collision_safe_dir(dest_parent, &source_name);

    // Create the dest workspace dir UP FRONT so the slug below is computed
    // from a path that EXISTS — `claude_project_hash` canonicalizes symlinks
    // (GH#25), and canonicalize only resolves an existing path. On macOS the
    // dest commonly lives under a symlinked prefix (`/tmp`→`/private/tmp`,
    // `/var`→`/private/var`); if we slug the not-yet-created path we'd get the
    // pre-canonical form, while Claude + every later caller (the registered
    // project path → resume/exists-check/#23 repair) slug the canonical form
    // — sessions would land in a dir Claude never reads. Creating it first
    // makes the unpack slug match what the rest of the system computes.
    std::fs::create_dir_all(&dest_path)
        .map_err(|e| format!("create dest dir {}: {e}", dest_path.display()))?;

    // ── recompute remote slug from the FINAL dest path ────────────────
    let remote_slug = claude_project_hash(&dest_path.to_string_lossy());
    let source_slug = manifest.source_slug.clone();

    // ── source/dest ABS workspace paths for the embedded-path rewrite ──
    // GH#23: the slug DIR name is remapped (above), but every session
    // `.jsonl` ALSO embeds the source machine's absolute workspace path in
    // its `cwd` (and `originalCwd`, worktree paths, tool-call `file_path`
    // args). Claude `/resume` filters by matching that embedded `cwd`
    // against the process cwd, so a stale source path → zero sessions on
    // the destination. We rewrite SOURCE→DEST as a raw substring pass over
    // each session line below (issue's recommended option 2 — one pass,
    // covers every embedded reference).
    //
    // `source_project_path` is a non-Option field present on every manifest
    // (version 1+). A pathological empty value (older/hand-built bundle
    // without it) disables the rewrite gracefully — the slug remap still
    // runs, so we degrade to the pre-fix behavior rather than crash.
    let source_project_path = manifest.source_project_path.clone();
    let dest_project_path = dest_path.to_string_lossy().to_string();
    let session_rewrite =
        SessionPathRewrite::new(&source_project_path, &dest_project_path);

    let projects_dir = home.join(".claude").join("projects");
    let remote_slug_dir = projects_dir.join(&remote_slug);

    // ── extract every bundle entry by class prefix ────────────────────
    let file = std::fs::File::open(bundle_path)
        .map_err(|e| format!("open bundle {}: {e}", bundle_path.display()))?;
    let dec = flate2::read::GzDecoder::new(file);
    let mut ar = tar::Archive::new(dec);

    for entry in ar.entries().map_err(|e| format!("read bundle: {e}"))? {
        let mut entry = entry.map_err(|e| format!("read bundle entry: {e}"))?;
        let archive_path = entry
            .path()
            .map_err(|e| format!("entry path: {e}"))?
            .to_path_buf();

        // manifest.json lives at the root — already parsed above.
        if archive_path == Path::new("manifest.json") {
            continue;
        }

        let out = match reroot(&archive_path, &dest_path, &remote_slug_dir, &source_slug, &remote_slug)
        {
            Some(p) => p,
            None => continue, // unknown prefix — skip defensively.
        };

        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create dir {}: {e}", parent.display()))?;
        }
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut buf)
            .map_err(|e| format!("read entry {}: {e}", archive_path.display()))?;

        // GH#23: rewrite embedded SOURCE→DEST workspace paths inside session
        // `.jsonl` files only (the `sessions/` class). A no-op when the
        // rewrite is disabled (source == dest, or no source path on the
        // manifest). Raw byte substring replace — handles spaces/odd chars
        // and covers `cwd`, `originalCwd`, worktree paths, and tool-call
        // `file_path` args in a single pass.
        if is_session_entry(&archive_path) {
            session_rewrite.apply_in_place(&mut buf);
        }

        std::fs::write(&out, &buf)
            .map_err(|e| format!("write {}: {e}", out.display()))?;
    }

    Ok((
        UnpackResult {
            dest_path,
            remote_slug,
            source_name,
        },
        manifest,
    ))
}

/// Map a bundle archive path to its on-disk destination, by class prefix.
/// Returns `None` for an unrecognized prefix.
fn reroot(
    archive_path: &Path,
    dest_path: &Path,
    remote_slug_dir: &Path,
    source_slug: &str,
    remote_slug: &str,
) -> Option<PathBuf> {
    let mut comps = archive_path.components();
    let first = comps.next()?.as_os_str().to_string_lossy().to_string();
    let rest: PathBuf = comps.as_path().to_path_buf();
    if rest.as_os_str().is_empty() {
        // prefix dir with no file under it — nothing to place.
        return None;
    }

    match first.as_str() {
        // workspace/<rel> → <dest_path>/<rel>
        "workspace" => Some(dest_path.join(rest)),
        // memory/<rel> → <remote-slug>/memory/<rel>
        "memory" => Some(remote_slug_dir.join("memory").join(rest)),
        // sessions/<source-slug[-branch]>/<id>.jsonl → re-root the leading
        // <source-slug> segment onto the recomputed remote slug, keeping
        // any -<branch> worktree suffix.
        "sessions" => {
            let projects_dir = remote_slug_dir.parent()?;
            Some(projects_dir.join(reroot_session_rel(&rest, source_slug, remote_slug)))
        }
        _ => None,
    }
}

/// Re-root a session rel path (`<source-slug>[-branch]/<id>.jsonl`) onto the
/// recomputed remote slug. The first path segment is the source slug dir
/// (possibly with a `-<branch>` suffix); swap its `<source-slug>` prefix for
/// `<remote-slug>` and keep the rest verbatim.
fn reroot_session_rel(rel: &Path, source_slug: &str, remote_slug: &str) -> PathBuf {
    let mut comps = rel.components();
    let dir_seg = match comps.next() {
        Some(c) => c.as_os_str().to_string_lossy().to_string(),
        None => return rel.to_path_buf(),
    };
    let remainder: PathBuf = comps.as_path().to_path_buf();

    // dir_seg is `<source-slug>` or `<source-slug>-<branch>`.
    let new_dir = if dir_seg == source_slug {
        remote_slug.to_string()
    } else if let Some(branch_suffix) = dir_seg.strip_prefix(&format!("{source_slug}-")) {
        format!("{remote_slug}-{branch_suffix}")
    } else {
        // Unexpected shape — keep as-is rather than dropping the session.
        dir_seg
    };

    if remainder.as_os_str().is_empty() {
        PathBuf::from(new_dir)
    } else {
        PathBuf::from(new_dir).join(remainder)
    }
}

/// True when a bundle archive path belongs to the `sessions/` class
/// (a `<session-id>.jsonl` whose embedded paths we must rewrite).
fn is_session_entry(archive_path: &Path) -> bool {
    archive_path
        .components()
        .next()
        .map(|c| c.as_os_str() == std::ffi::OsStr::new("sessions"))
        .unwrap_or(false)
}

/// A SOURCE→DEST workspace-path substring rewrite for session `.jsonl`
/// bytes (GH#23). Constructed once per unpack; `apply_in_place` runs over
/// each session file's raw bytes.
///
/// Disabled (a no-op) when `from` is empty (no source path on the
/// manifest — back-compat) or `from == to` (same-machine clone, nothing
/// to rewrite). Operates on raw bytes so paths with spaces or other odd
/// characters are handled verbatim, exactly like the manual `sed` users
/// run today.
struct SessionPathRewrite {
    from: Vec<u8>,
    to: Vec<u8>,
    enabled: bool,
}

impl SessionPathRewrite {
    fn new(from: &str, to: &str) -> Self {
        let enabled = !from.is_empty() && from != to;
        Self {
            from: from.as_bytes().to_vec(),
            to: to.as_bytes().to_vec(),
            enabled,
        }
    }

    /// Replace every occurrence of `from` with `to` in `buf`, in place.
    /// No-op when disabled or when `from` doesn't appear.
    fn apply_in_place(&self, buf: &mut Vec<u8>) {
        if !self.enabled || self.from.is_empty() {
            return;
        }
        if let Some(rewritten) = replace_bytes(buf, &self.from, &self.to) {
            *buf = rewritten;
        }
    }
}

/// Replace every non-overlapping occurrence of `needle` in `haystack`
/// with `replacement`. Returns `Some(new_bytes)` when at least one match
/// was found, `None` when there were none (so the caller can skip the
/// allocation). `needle` is assumed non-empty (caller guarantees it).
pub(super) fn replace_bytes(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Option<Vec<u8>> {
    let mut matches: Vec<usize> = Vec::new();
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            matches.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    if matches.is_empty() {
        return None;
    }

    let mut out =
        Vec::with_capacity(haystack.len() + matches.len() * replacement.len().saturating_sub(needle.len()).max(0));
    let mut prev = 0;
    for &m in &matches {
        out.extend_from_slice(&haystack[prev..m]);
        out.extend_from_slice(replacement);
        prev = m + needle.len();
    }
    out.extend_from_slice(&haystack[prev..]);
    Some(out)
}

/// `<parent>/<name>`, or `<parent>/name (1)`, `name (2)`, … if taken.
/// Mirrors the PRD's collision-safe naming.
fn collision_safe_dir(parent: &Path, name: &str) -> PathBuf {
    let direct = parent.join(name);
    if !direct.exists() {
        return direct;
    }
    for n in 1..=9999 {
        let candidate = parent.join(format!("{name} ({n})"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Pathological fallback — extremely unlikely.
    parent.join(format!("{name} ({})", uuid_like()))
}

fn uuid_like() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos}")
}
