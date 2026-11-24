//! Remote-side unpack: extract a clone bundle at recomputed paths and
//! register + configure the migrated workspace.
//!
//! Runs on the DESTINATION machine. The source bundle stored every file
//! under a per-class prefix (`workspace/`, `memory/`, `sessions/`,
//! `providers/`) so the unpack can fan files back out WITHOUT re-deriving
//! the class:
//!
//! - `workspace/<rel>`  → `<dest_path>/<rel>`
//! - `memory/<rel>`     → `<remote-home>/.claude/projects/<remote-slug>/memory/<rel>`
//! - `sessions/<rel>`   → `<remote-home>/.claude/projects/<remote-slug-rerooted>/<rel>`
//! - `providers/<rest>` → `<remote-home>/.<rest>` (with Cursor MD5 / Grok
//!   percent-encode re-key; Hermes export merges into state.db), only for
//!   the fixed provider list and session folders in `reroot_provider`
//!
//! Every entry must be a regular file at a relative path of plain names
//! (checked for the whole bundle before anything is written), and every
//! file is written without following a link below the folder it lands in.
//!
//! `dest_path = <dest_parent>/<source-name>` (collision-safe `name (1)`).
//! The remote slug is recomputed PURELY from the final dest path
//! (`claude_project_hash`) — the source slug is never trusted. Session rel
//! paths keep their `<source-slug>[-branch]/<id>.jsonl` shape, so we
//! re-root the leading `<source-slug>` segment onto the recomputed remote
//! slug, preserving any `-<branch>` worktree suffix.

use super::providers::{
    gemini_slug_from_rest, import_hermes_export, merge_gemini_projects_json, provider_is_cursor_store_db,
    provider_needs_path_rewrite, reroot_provider, ProviderDest,
};
use super::CloneManifest;
use crate::chat_history::claude_project_hash;
use std::collections::HashSet;
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
    /// `threads/threads.json` bytes, captured during extraction (the
    /// bundle is deleted before the project is registered, CL10). `None`
    /// when the bundle carries no Threads.
    pub threads_json: Option<Vec<u8>>,
}

/// Extract a clone bundle at `<dest_parent>/<source-name>` (collision-safe)
/// and place memory/sessions under the recomputed remote slug dir. Does NOT
/// touch the DB — registration + settings live in the daemon route so this
/// stays a pure filesystem op (the manifest is returned for the caller to
/// apply settings).
///
/// `home` is the remote home dir (`crate::paths::home_dir()` in production, a temp
/// dir in tests) under which `.claude/projects/<slug>/` and provider trees
/// are rooted.
pub fn unpack_bundle(
    bundle_path: &Path,
    dest_parent: &Path,
    home: &Path,
) -> Result<(UnpackResult, CloneManifest), String> {
    unpack_bundle_detailed(bundle_path, dest_parent, home).map_err(|e| e.to_string())
}

/// [`unpack_bundle`] with the failure's parts kept apart: the message, and
/// whether the folder the unpack claimed was deleted (a failure partway
/// deletes it).
pub fn unpack_bundle_detailed(
    bundle_path: &Path,
    dest_parent: &Path,
    home: &Path,
) -> Result<(UnpackResult, CloneManifest), UnpackError> {
    let manifest = super::read_manifest_from_bundle(bundle_path)?;

    // Every entry is checked before anything is made or written: a bundle
    // with one entry that is not a regular file at a relative path of
    // plain names unpacks nothing.
    check_bundle_entries(bundle_path)?;

    // ── source name = basename of the source project path ─────────────
    let source_name = Path::new(&manifest.source_project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| is_plain_folder_name(n))
        .unwrap_or_else(|| "workspace".to_string());

    // ── claim a new dest folder (collision-safe) ──────────────────────
    // Made with `create_dir`, so a folder or link already there is never
    // adopted (`name (1)`, `name (2)`, … instead). It is made UP FRONT so
    // the slug below is computed from a path that EXISTS —
    // `claude_project_hash` canonicalizes symlinks
    // (GH#25), and canonicalize only resolves an existing path. On macOS the
    // dest commonly lives under a symlinked prefix (`/tmp`→`/private/tmp`,
    // `/var`→`/private/var`); if we slug the not-yet-created path we'd get the
    // pre-canonical form, while Claude + every later caller (the registered
    // project path → resume/exists-check/#23 repair) slug the canonical form
    // — sessions would land in a dir Claude never reads. Creating it first
    // makes the unpack slug match what the rest of the system computes.
    let dest_path = claim_dest_dir(dest_parent, &source_name).map_err(UnpackError::from)?;

    // A failure from here on deletes the claimed folder: nothing half
    // filled is left behind.
    match fill_claimed(bundle_path, home, &manifest, dest_path.clone(), source_name) {
        Ok(result) => Ok((result, manifest)),
        Err(message) => Err(remove_claimed(dest_path, message)),
    }
}

/// Why an unpack failed, and what it did about the folder it claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackError {
    pub message: String,
    /// The claimed folder, deleted after a failure partway. `None` when
    /// the failure came before the claim, or the delete failed.
    pub removed_folder: Option<PathBuf>,
    /// The delete of the claimed folder failed: its path and the error.
    pub removal_error: Option<String>,
}

impl From<String> for UnpackError {
    fn from(message: String) -> Self {
        Self { message, removed_folder: None, removal_error: None }
    }
}

impl std::fmt::Display for UnpackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)?;
        if let Some(p) = &self.removed_folder {
            write!(f, "; the new folder {} was deleted", p.display())?;
        }
        if let Some(e) = &self.removal_error {
            write!(f, "; the new folder could not be deleted ({e})")?;
        }
        Ok(())
    }
}

/// Fill the claimed folder (and the home-side session and provider
/// folders) from the bundle.
fn fill_claimed(
    bundle_path: &Path,
    home: &Path,
    manifest: &CloneManifest,
    dest_path: PathBuf,
    source_name: String,
) -> Result<UnpackResult, String> {
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

    // Track Gemini slugs we write so we can merge projects.json once.
    let mut gemini_slugs: HashSet<String> = HashSet::new();

    // ── extract every bundle entry by class prefix ────────────────────
    let file = std::fs::File::open(bundle_path)
        .map_err(|e| format!("open bundle {}: {e}", bundle_path.display()))?;
    let dec = flate2::read::GzDecoder::new(file);
    let mut ar = tar::Archive::new(dec);
    let mut threads_json: Option<Vec<u8>> = None;

    for entry in ar.entries().map_err(|e| format!("read bundle: {e}"))? {
        let mut entry = entry.map_err(|e| format!("read bundle entry: {e}"))?;
        let archive_path = entry
            .path()
            .map_err(|e| format!("entry path: {e}"))?
            .to_path_buf();
        // Checked again here (this is the archive's second read).
        check_entry(entry.header(), &archive_path)?;

        // manifest.json lives at the root — already parsed above.
        if archive_path == Path::new("manifest.json") {
            continue;
        }

        // threads/threads.json: kept in memory for the Threads import (the
        // caller imports after registration). Capped; anything else under
        // `threads/` is an unknown file and skipped like any other prefix.
        if archive_path == Path::new(crate::overlay::transfer::THREADS_ARCHIVE_PATH) {
            threads_json = Some(read_threads_entry(&mut entry)?);
            continue;
        }

        // The source file's permission bits (an executable script stays
        // executable on the clone).
        let mode = entry_mode(entry.header());
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut buf)
            .map_err(|e| format!("read entry {}: {e}", archive_path.display()))?;

        // ── providers/ ────────────────────────────────────────────────
        if is_providers_entry(&archive_path) {
            let rest = strip_first_component(&archive_path);
            match reroot_provider(
                &rest,
                home,
                &source_project_path,
                &dest_project_path,
            ) {
                Some(ProviderDest::HermesExport) => {
                    // Row-level merge only — never ship/replace whole state.db.
                    match import_hermes_export(
                        home,
                        &buf,
                        &source_project_path,
                        &dest_project_path,
                    ) {
                        Ok(_n) => {}
                        Err(e) => {
                            // Locked / missing schema / busy — log + continue
                            // (never fail the whole unpack for Hermes).
                            crate::log_debug!(
                                "[clone/unpack] hermes import skipped: {e}"
                            );
                        }
                    }
                }
                Some(ProviderDest::File { root, rel }) => {
                    // SOURCE→DEST rewrite for text session files; best-effort
                    // byte rewrite inside Cursor store.db.
                    if provider_needs_path_rewrite(&archive_path)
                        || provider_is_cursor_store_db(&archive_path)
                    {
                        session_rewrite.apply_in_place(&mut buf);
                    }
                    write_under(&root, &rel, &buf, mode)?;
                    if let Some(slug) = gemini_slug_from_rest(&rest) {
                        gemini_slugs.insert(slug);
                    }
                }
                None => {
                    crate::log_debug!(
                        "[clone/unpack] skipped {}: not a provider session path this {} places",
                        archive_path.display(),
                        crate::brand::PRODUCT_NAME
                    );
                }
            }
            continue;
        }

        let (root, rel) = match reroot(
            &archive_path,
            &dest_path,
            &projects_dir,
            &source_slug,
            &remote_slug,
        ) {
            Some(p) => p,
            None => continue, // unknown prefix or shape — skip defensively.
        };

        // GH#23: rewrite embedded SOURCE→DEST workspace paths inside session
        // `.jsonl` files only (the `sessions/` class). A no-op when the
        // rewrite is disabled (source == dest, or no source path on the
        // manifest). Raw byte substring replace — handles spaces/odd chars
        // and covers `cwd`, `originalCwd`, worktree paths, and tool-call
        // `file_path` args in a single pass.
        if is_session_entry(&archive_path) {
            session_rewrite.apply_in_place(&mut buf);
        }

        write_under(&root, &rel, &buf, mode)?;
    }

    // Merge Gemini dest_path → slug into projects.json (additive).
    for slug in &gemini_slugs {
        if let Err(e) = merge_gemini_projects_json(home, &dest_project_path, slug) {
            crate::log_debug!("[clone/unpack] gemini projects.json merge skipped: {e}");
        }
    }

    Ok(UnpackResult {
        dest_path,
        remote_slug,
        source_name,
        threads_json,
    })
}

/// Delete the folder this unpack claimed, after a failure partway. Its
/// contents are the bundle's files, so they are removed outright, not moved
/// to the Trash. `remove_dir_all` removes a link it meets (the claimed path
/// itself included) without following it.
fn remove_claimed(dest_path: PathBuf, message: String) -> UnpackError {
    match std::fs::remove_dir_all(&dest_path) {
        Ok(()) => UnpackError { message, removed_folder: Some(dest_path), removal_error: None },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            UnpackError { message, removed_folder: Some(dest_path), removal_error: None }
        }
        Err(e) => UnpackError {
            message,
            removed_folder: None,
            removal_error: Some(format!("{}: {e}", dest_path.display())),
        },
    }
}

/// The permission bits a bundle entry carries for a regular file, without
/// setuid, setgid and sticky (a clone never makes a privileged file).
/// `None` for anything else, or a header without a readable mode.
pub(crate) fn entry_mode(header: &tar::Header) -> Option<u32> {
    if !header.entry_type().is_file() {
        return None;
    }
    header.mode().ok().map(|m| m & 0o777)
}

/// Write one unpacked file at `<root>/<rel>`. `root` is a folder this
/// unpack picked every part of (the claimed workspace folder, the new
/// slug's memory folder, `~/.claude/projects`, or a listed provider's
/// session folder), made here if missing; `rel` comes from the archive and
/// is walked without following a link ([`crate::confined_fs`]).
/// `mode` gives the file the bits it had on the source (`fs::write` made
/// new files 0666 & umask: every script lost its +x before 0.46.3).
fn write_under(root: &Path, rel: &Path, bytes: &[u8], mode: Option<u32>) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("create dir {}: {e}", root.display()))?;
    crate::confined_fs::write_file(root, rel, bytes, mode)
        .map_err(|e| format!("write {} under {}: {e}", rel.display(), root.display()))
}

/// Read every entry header of the bundle and check it ([`check_entry`])
/// without writing anything.
fn check_bundle_entries(bundle_path: &Path) -> Result<(), String> {
    let file = std::fs::File::open(bundle_path)
        .map_err(|e| format!("open bundle {}: {e}", bundle_path.display()))?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in ar.entries().map_err(|e| format!("read bundle: {e}"))? {
        let entry = entry.map_err(|e| format!("read bundle entry: {e}"))?;
        let archive_path = entry.path().map_err(|e| format!("entry path: {e}"))?.to_path_buf();
        check_entry(entry.header(), &archive_path)?;
    }
    Ok(())
}

/// A clone bundle holds only regular files (the builder appends each
/// file's bytes, never a link or a folder entry), each at a relative path
/// of plain names. Anything else refuses the whole bundle: a `..`, `.`,
/// root or drive part, an absolute path, or a symlink, hard link, folder or
/// device entry.
fn check_entry(header: &tar::Header, archive_path: &Path) -> Result<(), String> {
    let kind = header.entry_type();
    if !kind.is_file() {
        let what = if kind.is_symlink() {
            "a symlink"
        } else if kind.is_hard_link() {
            "a hard link"
        } else if kind.is_dir() {
            "a folder entry"
        } else {
            "not a regular file"
        };
        return Err(format!(
            "clone bundle entry {} is {what}; a clone bundle holds only regular files, so nothing was unpacked",
            archive_path.display()
        ));
    }
    if archive_path.is_absolute() || crate::confined_fs::check_rel(archive_path).is_err() {
        return Err(format!(
            "clone bundle entry {} is not a relative path of plain names, so nothing was unpacked",
            archive_path.display()
        ));
    }
    Ok(())
}

/// A folder name from the manifest that is one plain name on this OS (on
/// Windows also no `:` or `\`).
fn is_plain_folder_name(name: &str) -> bool {
    let odd = name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']);
    #[cfg(windows)]
    let odd = odd || name.contains([':', '\\']);
    !odd
}

/// Read `threads/threads.json` from the archive, refusing more than the
/// export cap (bounds memory on unpack).
fn read_threads_entry<R: std::io::Read>(entry: &mut tar::Entry<'_, R>) -> Result<Vec<u8>, String> {
    let cap = crate::overlay::transfer::MAX_EXPORT_BYTES as u64;
    let size = entry.header().size().map_err(|e| format!("threads entry size: {e}"))?;
    if size > cap {
        return Err(format!("threads/threads.json is {size} bytes, over the {cap} cap"));
    }
    let mut buf = Vec::with_capacity(size as usize);
    std::io::Read::read_to_end(&mut std::io::Read::take(entry, cap + 1), &mut buf)
        .map_err(|e| format!("read threads/threads.json: {e}"))?;
    if buf.len() as u64 > cap {
        return Err(format!("threads/threads.json is over the {cap} cap"));
    }
    Ok(buf)
}

/// Map a bundle archive path to `(root, rel)` by class prefix: the file
/// lands at `<root>/<rel>`, with `rel` walked without following a link.
/// Returns `None` for an unrecognized prefix (including `providers/`,
/// which is handled separately) and for a `sessions/` entry that is not
/// `<source-slug>[-branch]/<file>.jsonl`.
fn reroot(
    archive_path: &Path,
    dest_path: &Path,
    projects_dir: &Path,
    source_slug: &str,
    remote_slug: &str,
) -> Option<(PathBuf, PathBuf)> {
    let mut comps = archive_path.components();
    let first = comps.next()?.as_os_str().to_string_lossy().to_string();
    let rest: PathBuf = comps.as_path().to_path_buf();
    if rest.as_os_str().is_empty() {
        // prefix dir with no file under it — nothing to place.
        return None;
    }

    match first.as_str() {
        // workspace/<rel> → <dest_path>/<rel>
        "workspace" => Some((dest_path.to_path_buf(), rest)),
        // memory/<rel> → <remote-slug>/memory/<rel>
        "memory" => Some((projects_dir.join(remote_slug).join("memory"), rest)),
        // sessions/<source-slug[-branch]>/<id>.jsonl → re-root the leading
        // <source-slug> segment onto the recomputed remote slug, keeping
        // any -<branch> worktree suffix.
        "sessions" => Some((projects_dir.to_path_buf(), reroot_session_rel(&rest, source_slug, remote_slug)?)),
        _ => None,
    }
}

/// Drop the first path component (`providers/…` → `…`).
fn strip_first_component(path: &Path) -> PathBuf {
    let mut comps = path.components();
    let _ = comps.next();
    comps.as_path().to_path_buf()
}

/// Re-root a session rel path (`<source-slug>[-branch]/<id>.jsonl`) onto the
/// recomputed remote slug. The first path segment is the source slug dir
/// (possibly with a `-<branch>` suffix); swap its `<source-slug>` prefix for
/// `<remote-slug>` and keep the file name. `None` for any other shape (the
/// source only bundles that one), so a session never lands in another
/// project's folder.
fn reroot_session_rel(rel: &Path, source_slug: &str, remote_slug: &str) -> Option<PathBuf> {
    let mut comps = rel.components();
    let dir_seg = comps.next()?.as_os_str().to_string_lossy().to_string();
    let file = comps.next()?.as_os_str().to_os_string();
    if comps.next().is_some() || Path::new(&file).extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return None;
    }

    // dir_seg is `<source-slug>` or `<source-slug>-<branch>`.
    let new_dir = if dir_seg == source_slug {
        remote_slug.to_string()
    } else {
        let branch_suffix = dir_seg.strip_prefix(&format!("{source_slug}-"))?;
        format!("{remote_slug}-{branch_suffix}")
    };
    Some(PathBuf::from(new_dir).join(file))
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

fn is_providers_entry(archive_path: &Path) -> bool {
    archive_path
        .components()
        .next()
        .map(|c| c.as_os_str() == std::ffi::OsStr::new("providers"))
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
    from: String,
    to: String,
}

impl SessionPathRewrite {
    fn new(from: &str, to: &str) -> Self {
        Self { from: from.to_string(), to: to.to_string() }
    }

    /// Replace every occurrence of `from` with `to` in `buf`, in place.
    /// No-op when disabled or when `from` doesn't appear.
    fn apply_in_place(&self, buf: &mut Vec<u8>) {
        if let Some(rewritten) = replace_path_bytes(buf, &self.from, &self.to) {
            *buf = rewritten;
        }
    }
}

/// Rewrite the workspace path `from` → `to` in session bytes, both as
/// written raw and as a JSON string body spells it. The two differ when the
/// path holds a `\` or `"`: every Windows path (`D:\\src\\app` inside a
/// `.jsonl`). `None` when nothing matched, or `from` is empty or equals `to`.
pub(super) fn replace_path_bytes(haystack: &[u8], from: &str, to: &str) -> Option<Vec<u8>> {
    if from.is_empty() || from == to {
        return None;
    }
    let json_body = |s: &str| {
        let q = serde_json::to_string(s).expect("a str always serializes");
        q[1..q.len() - 1].to_string()
    };
    let (esc_from, esc_to) = (json_body(from), json_body(to));
    // Longest first, so the escaped spelling wins where both could start.
    let mut pairs: Vec<(&[u8], &[u8])> = Vec::with_capacity(2);
    if esc_from != from {
        pairs.push((esc_from.as_bytes(), esc_to.as_bytes()));
    }
    pairs.push((from.as_bytes(), to.as_bytes()));
    if pairs.len() == 1 {
        return replace_bytes(haystack, from.as_bytes(), to.as_bytes());
    }
    let mut out = Vec::with_capacity(haystack.len());
    let (mut i, mut hit) = (0, false);
    'scan: while i < haystack.len() {
        for (needle, repl) in &pairs {
            if haystack[i..].starts_with(needle) {
                out.extend_from_slice(repl);
                i += needle.len();
                hit = true;
                continue 'scan;
            }
        }
        out.push(haystack[i]);
        i += 1;
    }
    hit.then_some(out)
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

/// Make and return `<parent>/<name>`, or `<parent>/name (1)`, `name (2)`,
/// … when taken (the PRD's collision-safe naming). Each try is one
/// `create_dir`, so a name already used by a folder, file or link is
/// skipped, never adopted.
fn claim_dest_dir(parent: &Path, name: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(parent).map_err(|e| format!("create dest parent {}: {e}", parent.display()))?;
    let candidates = std::iter::once(name.to_string())
        .chain((1..=9999).map(|n| format!("{name} ({n})")))
        // Pathological fallback — extremely unlikely.
        .chain(std::iter::once(format!("{name} ({})", uuid_like())));
    for candidate in candidates {
        let path = parent.join(candidate);
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("create dest dir {}: {e}", path.display())),
        }
    }
    Err(format!("no free folder name for {name} in {}", parent.display()))
}

fn uuid_like() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos}")
}
