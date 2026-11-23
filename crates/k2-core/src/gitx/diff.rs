//! Diffs on gix (research §2, S2): line counts and unified hunks of two
//! buffers (Myers, with git's line-slider post-processing), tree → tree
//! file changes, and the combined tree → work tree (with index) diff that
//! libgit2 did in one call (`diff_tree_to_workdir_with_index`).
//!
//! No textconv or external diff ever runs: the buffers are the blobs' raw
//! bytes, or a work-tree file cleaned by the repo's built-in eol/ident
//! filters only (the door strips every filter and diff driver).

use std::collections::BTreeMap;
use std::path::Path;

use super::status::{Flags, Options, Untracked};
use super::EntryKind;

/// Bytes git sniffs for a NUL to call a buffer binary.
pub const BINARY_SNIFF: usize = 8000;

/// git's binary rule: a NUL in the first 8000 bytes.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF)].contains(&0)
}

fn with_diff<R>(a: &[u8], b: &[u8], f: impl FnOnce(&gix::diff::blob::InternedInput<&[u8]>, &gix::diff::blob::Diff) -> R) -> R {
    let input = gix::diff::blob::InternedInput::new(a, b);
    let mut diff = gix::diff::blob::Diff::compute(gix::diff::blob::Algorithm::Myers, &input);
    diff.postprocess_lines(&input);
    f(&input, &diff)
}

/// `(additions, deletions)` in lines between two buffers.
pub fn line_stats(a: &[u8], b: &[u8]) -> (u32, u32) {
    with_diff(a, b, |_, d| (d.count_additions(), d.count_removals()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkLine {
    pub kind: LineKind,
    /// The line with its `\n` (none on a last line without one).
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<HunkLine>,
}

/// Unified-diff hunks with `context` lines, numbered like `git diff -U<n>`
/// (changes closer than `2 * context` lines share a hunk).
pub fn hunks(a: &[u8], b: &[u8], context: u32) -> Vec<Hunk> {
    with_diff(a, b, |input, diff| {
        let changes: Vec<gix::diff::blob::Hunk> = diff.hunks().collect();
        let old_len = input.before.len() as u32;
        let tok = |t: gix::diff::blob::Token| -> Vec<u8> { input.interner[t].to_vec() };
        let mut out = Vec::new();
        let mut i = 0;
        while i < changes.len() {
            let mut j = i;
            while j + 1 < changes.len() && changes[j + 1].before.start - changes[j].before.end <= 2 * context {
                j += 1;
            }
            let (first, last) = (&changes[i], &changes[j]);
            let old_from = first.before.start.saturating_sub(context);
            let old_to = (last.before.end + context).min(old_len);
            let new_from = first.after.start - (first.before.start - old_from);
            let new_to = last.after.end + (old_to - last.before.end);
            let mut lines = Vec::new();
            let mut cur = old_from;
            for ch in &changes[i..=j] {
                for k in cur..ch.before.start {
                    lines.push(HunkLine { kind: LineKind::Context, content: tok(input.before[k as usize]) });
                }
                for k in ch.before.clone() {
                    lines.push(HunkLine { kind: LineKind::Remove, content: tok(input.before[k as usize]) });
                }
                for k in ch.after.clone() {
                    lines.push(HunkLine { kind: LineKind::Add, content: tok(input.after[k as usize]) });
                }
                cur = ch.before.end;
            }
            for k in cur..old_to {
                lines.push(HunkLine { kind: LineKind::Context, content: tok(input.before[k as usize]) });
            }
            let (old_count, new_count) = (old_to - old_from, new_to - new_from);
            out.push(Hunk {
                old_start: if old_count == 0 { old_from } else { old_from + 1 },
                old_count,
                new_start: if new_count == 0 { new_from } else { new_from + 1 },
                new_count,
                lines,
            });
            i = j + 1;
        }
        out
    })
}

// ── File-level changes ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Deleted,
    Modified,
    Renamed,
    Typechange,
    Conflicted,
}

/// Where one side of a change lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    Absent,
    /// An object in the repository.
    Object { kind: EntryKind, id: gix::ObjectId },
    /// The work-tree file at the change's path.
    Worktree { kind: EntryKind },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// The new path (the old one for a deletion).
    pub path: String,
    /// The old path of a rename.
    pub old_path: Option<String>,
    pub kind: ChangeKind,
    pub old: Side,
    pub new: Side,
}

/// Every file (blob, link, submodule) of a tree, by path.
fn tree_files(repo: &gix::Repository, tree: Option<gix::ObjectId>) -> Result<BTreeMap<String, (EntryKind, gix::ObjectId)>, String> {
    let Some(tree) = tree else { return Ok(BTreeMap::new()) };
    let idx = repo.index_from_tree(&tree).map_err(|e| e.to_string())?;
    Ok(idx
        .entries()
        .iter()
        .map(|e| (e.path(&idx).to_string(), (EntryKind::from_index_mode(e.mode), e.id)))
        .collect())
}

/// Files that differ between two trees (None = empty), sorted by path, no
/// rename pairing (libgit2's `diff_tree_to_tree` default).
pub fn tree_to_tree(repo: &gix::Repository, old: Option<gix::ObjectId>, new: Option<gix::ObjectId>) -> Result<Vec<FileChange>, String> {
    let a = tree_files(repo, old)?;
    let b = tree_files(repo, new)?;
    let mut out = Vec::new();
    for (p, (ka, ia)) in &a {
        match b.get(p) {
            None => out.push(FileChange {
                path: p.clone(),
                old_path: None,
                kind: ChangeKind::Deleted,
                old: Side::Object { kind: *ka, id: *ia },
                new: Side::Absent,
            }),
            Some((kb, ib)) if kb != ka || ib != ia => out.push(FileChange {
                path: p.clone(),
                old_path: None,
                kind: if ka.is_typechange(*kb) { ChangeKind::Typechange } else { ChangeKind::Modified },
                old: Side::Object { kind: *ka, id: *ia },
                new: Side::Object { kind: *kb, id: *ib },
            }),
            Some(_) => {}
        }
    }
    for (p, (kb, ib)) in &b {
        if !a.contains_key(p) {
            out.push(FileChange {
                path: p.clone(),
                old_path: None,
                kind: ChangeKind::Added,
                old: Side::Absent,
                new: Side::Object { kind: *kb, id: *ib },
            });
        }
    }
    out.sort_by(|x, y| x.path.cmp(&y.path));
    Ok(out)
}

/// What the work-tree file at `rel` is, as git would store it.
pub fn worktree_kind(repo: &gix::Repository, workdir: &Path, rel: &str, index_kind: Option<EntryKind>) -> Option<EntryKind> {
    let meta = std::fs::symlink_metadata(workdir.join(rel)).ok()?;
    if meta.file_type().is_symlink() {
        return Some(EntryKind::Link);
    }
    if meta.is_dir() {
        // A checked-out submodule, or a folder.
        return Some(if index_kind == Some(EntryKind::Commit) { EntryKind::Commit } else { EntryKind::Tree });
    }
    let filemode = repo.config_snapshot().boolean("core.fileMode").unwrap_or(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if filemode {
            return Some(if meta.permissions().mode() & 0o111 != 0 { EntryKind::Executable } else { EntryKind::Blob });
        }
    }
    let _ = filemode;
    Some(match index_kind {
        Some(EntryKind::Executable) => EntryKind::Executable,
        _ => EntryKind::Blob,
    })
}

/// Reads work-tree files the way git would store them: a link's target, or
/// a file's bytes through the built-in eol/ident filters (`convert_to_git`;
/// no driver programs, the door removed them).
pub struct WorktreeReader<'r> {
    repo: &'r gix::Repository,
    workdir: std::path::PathBuf,
    pipeline: Option<(gix::filter::Pipeline<'r>, gix::worktree::IndexPersistedOrInMemory)>,
}

impl<'r> WorktreeReader<'r> {
    pub fn new(repo: &'r gix::Repository) -> Option<Self> {
        let workdir = repo.workdir()?.to_path_buf();
        Some(Self { repo, workdir, pipeline: None })
    }

    /// The file's git-side bytes, or None when it can't be read. `max`
    /// caps what is read (None past it).
    pub fn bytes(&mut self, rel: &str, max: Option<u64>) -> Option<Vec<u8>> {
        let full = self.workdir.join(rel);
        let meta = std::fs::symlink_metadata(&full).ok()?;
        if meta.file_type().is_symlink() {
            let t = std::fs::read_link(&full).ok()?;
            return Some(gix::path::into_bstr(t).ok()?.as_ref().to_vec());
        }
        if !meta.is_file() || max.is_some_and(|m| meta.len() > m) {
            return None;
        }
        let raw = std::fs::read(&full).ok()?;
        if self.pipeline.is_none() {
            self.pipeline = self.repo.filter_pipeline(None).ok();
        }
        let Some((pipe, index)) = self.pipeline.as_mut() else { return Some(raw) };
        let mut out = Vec::with_capacity(raw.len());
        match pipe.convert_to_git(raw.as_slice(), Path::new(rel), index) {
            Ok(mut converted) => {
                use std::io::Read;
                converted.read_to_end(&mut out).ok()?;
                Some(out)
            }
            Err(_) => Some(raw),
        }
    }

    /// The file's blob id as git would store it.
    pub fn blob_id(&mut self, rel: &str) -> Option<gix::ObjectId> {
        super::hash_blob(&self.bytes(rel, None)?)
    }
}

/// The diff from tree `tree` (None = empty) to the work tree, using the
/// index for paths the work tree doesn't change: libgit2's
/// `diff_tree_to_workdir_with_index`. Untracked files are not part of it.
/// `renames` pairs a deletion with an addition (staged renames, 50%).
/// Sorted by path.
pub fn tree_to_worktree(repo: &gix::Repository, tree: Option<gix::ObjectId>, renames: bool) -> Result<Vec<FileChange>, String> {
    let workdir = repo.workdir().ok_or("bare repository")?.to_path_buf();
    let empty = repo.empty_tree().id;
    let opts = Options { untracked: Untracked::None, staged_renames: renames, submodules: true };
    let rows = super::status::statuses_against(repo, opts, Some(tree.unwrap_or(empty)))?;
    let index = repo.index_or_empty().map_err(|e| e.to_string())?;
    let tree_entry = |rel: &str| tree.and_then(|t| super::tree_entry(repo, t, rel));
    let mut reader = WorktreeReader::new(repo).ok_or("bare repository")?;
    let mut out = Vec::new();
    for r in rows {
        let f = r.flags;
        let old_rel = r.renamed_from.clone().unwrap_or_else(|| r.path.clone());
        let old = match tree_entry(&old_rel) {
            Some((kind, id)) => Side::Object { kind, id },
            None => Side::Absent,
        };
        if f.contains(Flags::CONFLICTED) {
            let new = match worktree_kind(repo, &workdir, &r.path, None) {
                Some(kind) => Side::Worktree { kind },
                None => Side::Absent,
            };
            out.push(FileChange { path: r.path, old_path: None, kind: ChangeKind::Conflicted, old, new });
            continue;
        }
        if f.contains(Flags::WT_DELETED) {
            if old == Side::Absent {
                continue; // added to the index, then deleted: nothing vs the tree
            }
            out.push(FileChange { path: old_rel, old_path: None, kind: ChangeKind::Deleted, old, new: Side::Absent });
            continue;
        }
        let index_entry = super::index_entry(&index, &r.path);
        let new = if f.intersects(Flags::WT_MODIFIED | Flags::WT_TYPECHANGE) {
            match worktree_kind(repo, &workdir, &r.path, index_entry.map(|e| e.0)) {
                Some(kind) => Side::Worktree { kind },
                None => Side::Absent,
            }
        } else if f.contains(Flags::INDEX_DELETED) {
            Side::Absent
        } else {
            match index_entry {
                Some((kind, id)) => Side::Object { kind, id },
                None => Side::Absent,
            }
        };
        // A work-tree file that came back to the tree's content is no change.
        if let (Side::Object { kind: ko, id: io }, Side::Worktree { kind: kn }) = (&old, &new) {
            if ko == kn && kn.is_blob() || *ko == EntryKind::Link && *kn == EntryKind::Link {
                if reader.blob_id(&r.path).as_ref() == Some(io) && r.renamed_from.is_none() {
                    continue;
                }
            }
        }
        if let (Side::Object { kind: ko, id: io }, Side::Object { kind: kn, id: inn }) = (&old, &new) {
            if ko == kn && io == inn && r.renamed_from.is_none() {
                continue;
            }
        }
        let side_kind = |s: &Side| match s {
            Side::Absent => None,
            Side::Object { kind, .. } | Side::Worktree { kind } => Some(*kind),
        };
        let kind = match (&old, &new) {
            (Side::Absent, Side::Absent) => continue,
            (Side::Absent, _) => ChangeKind::Added,
            (_, Side::Absent) => ChangeKind::Deleted,
            _ if r.renamed_from.is_some() => ChangeKind::Renamed,
            _ => match (side_kind(&old), side_kind(&new)) {
                (Some(a), Some(b)) if a.is_typechange(b) => ChangeKind::Typechange,
                _ => ChangeKind::Modified,
            },
        };
        let (path, old_path) = match kind {
            ChangeKind::Deleted => (old_rel, None),
            ChangeKind::Renamed => (r.path, r.renamed_from),
            _ => (r.path, None),
        };
        out.push(FileChange { path, old_path, kind, old, new });
    }
    out.sort_by(|x, y| x.path.cmp(&y.path));
    Ok(out)
}

/// HEAD's tree (or `tree`) → the index: the staged changes, renames
/// paired when `renames` (libgit2's `diff_tree_to_index` + `find_similar`).
/// Conflicted paths are left out. Sorted by path.
pub fn tree_to_index(repo: &gix::Repository, tree: Option<gix::ObjectId>, renames: bool) -> Result<Vec<FileChange>, String> {
    let empty = repo.empty_tree().id;
    let opts = Options { untracked: Untracked::None, staged_renames: renames, submodules: false };
    let rows = super::status::statuses_against(repo, opts, Some(tree.unwrap_or(empty)))?;
    let index = repo.index_or_empty().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        let f = r.flags;
        if f.contains(Flags::CONFLICTED) || !f.intersects(Flags::INDEX_ANY) {
            continue;
        }
        let old_rel = r.renamed_from.clone().unwrap_or_else(|| r.path.clone());
        let old = match tree.and_then(|t| super::tree_entry(repo, t, &old_rel)) {
            Some((kind, id)) => Side::Object { kind, id },
            None => Side::Absent,
        };
        let new = match super::index_entry(&index, &r.path) {
            Some((kind, id)) if !f.contains(Flags::INDEX_DELETED) => Side::Object { kind, id },
            _ => Side::Absent,
        };
        let kind = if f.contains(Flags::INDEX_NEW) {
            ChangeKind::Added
        } else if f.contains(Flags::INDEX_DELETED) {
            ChangeKind::Deleted
        } else if f.contains(Flags::INDEX_RENAMED) {
            ChangeKind::Renamed
        } else if f.contains(Flags::INDEX_TYPECHANGE) {
            ChangeKind::Typechange
        } else {
            ChangeKind::Modified
        };
        let old_path = (kind == ChangeKind::Renamed).then(|| old_rel.clone());
        out.push(FileChange { path: r.path, old_path, kind, old, new });
    }
    out.sort_by(|x, y| x.path.cmp(&y.path));
    Ok(out)
}

/// The index → the work tree: the unstaged changes of tracked files (no
/// untracked files, no renames; libgit2's `diff_index_to_workdir`).
/// Conflicted paths are left out. Sorted by path.
pub fn index_to_worktree(repo: &gix::Repository) -> Result<Vec<FileChange>, String> {
    let workdir = repo.workdir().ok_or("bare repository")?.to_path_buf();
    let opts = Options { untracked: Untracked::None, staged_renames: false, submodules: true };
    let rows = super::status::statuses(repo, opts)?;
    let index = repo.index_or_empty().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        let f = r.flags;
        if f.contains(Flags::CONFLICTED) || !f.intersects(Flags::WT_ANY) {
            continue;
        }
        let entry = super::index_entry(&index, &r.path);
        let old = match entry {
            Some((kind, id)) => Side::Object { kind, id },
            None => continue,
        };
        let (kind, new) = if f.contains(Flags::WT_DELETED) {
            (ChangeKind::Deleted, Side::Absent)
        } else {
            match worktree_kind(repo, &workdir, &r.path, entry.map(|e| e.0)) {
                Some(k) => (
                    if f.contains(Flags::WT_TYPECHANGE) { ChangeKind::Typechange } else { ChangeKind::Modified },
                    Side::Worktree { kind: k },
                ),
                None => (ChangeKind::Deleted, Side::Absent),
            }
        };
        out.push(FileChange { path: r.path, old_path: None, kind, old, new });
    }
    Ok(out)
}

/// The bytes of one side (None: absent, unreadable or over `max`).
pub fn side_bytes(repo: &gix::Repository, reader: &mut WorktreeReader<'_>, path: &str, side: &Side, max: u64) -> Option<Vec<u8>> {
    match side {
        Side::Absent => Some(Vec::new()),
        Side::Object { kind: EntryKind::Commit, id } | Side::Object { kind: EntryKind::Tree, id } => {
            Some(format!("Subproject commit {id}\n").into_bytes())
        }
        Side::Object { id, .. } => {
            if super::object_size(repo, *id)? > max {
                return None;
            }
            super::blob_bytes(repo, *id)
        }
        Side::Worktree { kind: EntryKind::Commit | EntryKind::Tree } => None,
        Side::Worktree { .. } => reader.bytes(path, Some(max)),
    }
}

/// The size of one side, without reading it (worktree: the file's size).
pub fn side_size(repo: &gix::Repository, workdir: &Path, path: &str, side: &Side) -> u64 {
    match side {
        Side::Absent => 0,
        Side::Object { id, .. } => super::object_size(repo, *id).unwrap_or(0),
        Side::Worktree { .. } => std::fs::symlink_metadata(workdir.join(path)).map(|m| m.len()).unwrap_or(0),
    }
}

/// Line stats for a change: `Some((adds, dels, binary))`; `(None, None,
/// false)` past `max` bytes on either side; `(None, None, true)` binary.
pub fn change_stats(
    repo: &gix::Repository,
    reader: &mut WorktreeReader<'_>,
    c: &FileChange,
    max: u64,
) -> (Option<u32>, Option<u32>, bool) {
    let old_path = c.old_path.as_deref().unwrap_or(&c.path);
    let (Some(a), Some(b)) = (side_bytes(repo, reader, old_path, &c.old, max), side_bytes(repo, reader, &c.path, &c.new, max)) else {
        return (None, None, false);
    };
    if is_binary(&a) || is_binary(&b) {
        return (None, None, true);
    }
    let (adds, dels) = line_stats(&a, &b);
    (Some(adds), Some(dels), false)
}
