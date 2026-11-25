//! The one door for gitoxide (`gix`) in K2 (research-gix-instead-of-libgit2-v1
//! §5). Every repository K2 reads with gix is opened here, never with
//! `gix::open`, `gix::discover` or `ThreadSafeRepository` anywhere else (the
//! grep test below fails on that). The door:
//!
//! - never runs a `git` program: `permissions.config.git_binary` and
//!   `permissions.attributes.git_binary` are always false, so gix never asks
//!   `git config -l --show-origin` or `git --exec-path` for the installation
//!   config. On a Mac without the Command Line Tools that `git` is Apple's
//!   shim, which opens the install dialog (§7.1). The tripwire test puts a
//!   fake `git` first on PATH and proves it never runs.
//! - runs no outside program from config: every `filter.<driver>` section
//!   and every `diff.<driver>` section is removed from the opened repo's
//!   config, so the filter pipeline gix builds for status and diffs has an
//!   empty driver list and no textconv (§7.2). libgit2 never ran them either.
//!   eol/ident conversion is built in and still applies. gix runs no hooks
//!   and, without its `fsmonitor` feature, no fsmonitor.
//! - refuses a repository git itself would refuse: one owned by another
//!   user (gix opens those with reduced trust) unless the person's
//!   `safe.directory` names it, as git does. K2 never sets `safe.directory`.
//!
//! Writes don't go through gix: stage, unstage, commit, discard, branch
//! delete and merge run the person's own `git` through
//! [`crate::git_safe`] (plan of record, 0.46.18). [`diff`] and [`status`]
//! are the read helpers the call sites share.

use std::path::{Path, PathBuf};

pub mod diff;
pub mod status;

/// Why a repository couldn't be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// No repository at (or above, for discover) the path.
    NotARepo(String),
    /// The repository belongs to another user and `safe.directory` doesn't
    /// name it; git refuses it too.
    Untrusted(String),
    /// Opened, but its configuration couldn't be cleaned (no reads happen).
    Config(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NotARepo(m) => write!(f, "not a git repository: {m}"),
            OpenError::Untrusted(m) => write!(f, "repository owned by another user (git refuses it too): {m}"),
            OpenError::Config(m) => write!(f, "can't read the repository's configuration: {m}"),
        }
    }
}

/// The options every K2 open uses (see the module docs).
pub fn open_options() -> gix::open::Options {
    let mut perms = gix::open::Permissions::all();
    perms.config.git_binary = false;
    perms.attributes.git_binary = false;
    gix::open::Options::default().permissions(perms)
}

/// The repository holding `path`, searching upwards like `git rev-parse`.
pub fn discover(path: impl AsRef<Path>) -> Result<gix::Repository, OpenError> {
    let path = path.as_ref();
    let opts = open_options();
    let map = gix::sec::trust::Mapping { full: opts.clone(), reduced: opts };
    let ts = gix::ThreadSafeRepository::discover_opts(path, Default::default(), map)
        .map_err(|e| OpenError::NotARepo(format!("{}: {e}", path.display())))?;
    finish(ts.to_thread_local(), path)
}

/// The repository at exactly `path` (its work tree or its git dir), with no
/// upward search.
pub fn open(path: impl AsRef<Path>) -> Result<gix::Repository, OpenError> {
    let path = path.as_ref();
    let ts = gix::ThreadSafeRepository::open_opts(path.to_path_buf(), open_options())
        .map_err(|e| OpenError::NotARepo(format!("{}: {e}", path.display())))?;
    finish(ts.to_thread_local(), path)
}

/// Is there a repository at or above `path` by gix's path search alone
/// (nothing opened, no config read)? `false` only for gix's "no repository
/// here"; any other failure (IO, an untrusted candidate) is `true`, so a
/// caller that refuses on `true` fails closed ([`crate::git_write_guard`]).
pub fn repository_above(path: &Path) -> bool {
    use gix::discover::upwards::Error as E;
    match gix::discover::upwards(path) {
        Ok(_) => true,
        Err(e) => !e
            .downcast_any_ref::<E>()
            .is_some_and(|e| matches!(e, E::NoGitRepository { .. } | E::NoGitRepositoryWithinCeiling { .. } | E::NoGitRepositoryWithinFs { .. })),
    }
}

fn finish(mut repo: gix::Repository, asked: &Path) -> Result<gix::Repository, OpenError> {
    if repo.git_dir_trust() != gix::sec::Trust::Full {
        return Err(OpenError::Untrusted(asked.display().to_string()));
    }
    strip_drivers(&mut repo).map_err(OpenError::Config)?;
    Ok(repo)
}

/// Config sections that name programs K2's reads must never run.
const DRIVER_SECTIONS: [&str; 2] = ["filter", "diff"];

/// Remove every `filter.<name>.*` and `diff.<name>.*` section (and the
/// subsection-less `[diff]`'s `external`, by dropping driver-bearing
/// sections only: `[diff]` itself has no subsection and stays, but gix never
/// runs `diff.external`).
fn strip_drivers(repo: &mut gix::Repository) -> Result<(), String> {
    let mut snap = repo.config_snapshot_mut();
    for name in DRIVER_SECTIONS {
        let ids: Vec<_> = snap
            .sections_and_ids_by_name(name)
            .map(|it| it.filter(|(s, _)| s.header().subsection_name().is_some()).map(|(_, id)| id).collect())
            .unwrap_or_default();
        for id in ids {
            snap.remove_section_by_id(id);
        }
    }
    snap.commit().map(|_| ()).map_err(|e| e.to_string())
}

/// The names of the driver sections left in `repo`'s config (tests).
pub fn driver_sections(repo: &gix::Repository) -> Vec<String> {
    let snap = repo.config_snapshot();
    let mut out = Vec::new();
    for name in DRIVER_SECTIONS {
        if let Some(it) = snap.plumbing().sections_by_name(name) {
            for s in it {
                if let Some(sub) = s.header().subsection_name() {
                    out.push(format!("{name}.{sub}"));
                }
            }
        }
    }
    out
}

// ── Small shared reads ──────────────────────────────────────────────────

/// The work tree, or None for a bare repository.
pub fn workdir(repo: &gix::Repository) -> Option<PathBuf> {
    repo.workdir().map(Path::to_path_buf)
}

/// `/`-separated repo-relative path as bytes, for index and tree lookups.
pub fn rel_bstr(rel: &str) -> &gix::bstr::BStr {
    use gix::bstr::ByteSlice;
    rel.as_bytes().as_bstr()
}

/// A `refs/heads/x` → `x`, `refs/remotes/o/x` → `o/x`, `refs/tags/t` → `t`
/// shorthand (git2's `shorthand()`).
pub fn short_name(full: &gix::refs::FullNameRef) -> String {
    full.shorten().to_string()
}

/// HEAD as K2 shows it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeadInfo {
    /// The branch HEAD names (also when unborn); None when detached.
    pub branch: Option<String>,
    /// The commit HEAD points at; None when unborn.
    pub oid: Option<gix::ObjectId>,
    pub detached: bool,
    pub unborn: bool,
}

pub fn head_info(repo: &gix::Repository) -> HeadInfo {
    let Ok(head) = repo.head() else { return HeadInfo { unborn: true, ..Default::default() } };
    let detached = head.is_detached();
    let unborn = head.is_unborn();
    let branch = if detached { None } else { head.referent_name().map(short_name) };
    let oid = if unborn { None } else { repo.head_id().ok().map(|id| id.detach()) };
    HeadInfo { branch, oid, detached, unborn }
}

/// git2's `repo.head()?.shorthand()`: the branch name, `HEAD` when
/// detached, None when unborn or unreadable.
pub fn head_shorthand(repo: &gix::Repository) -> Option<String> {
    let h = head_info(repo);
    if h.unborn {
        return None;
    }
    if h.detached {
        return Some("HEAD".to_string());
    }
    h.branch
}

/// The tree HEAD's commit points at; None when unborn.
pub fn head_tree_id(repo: &gix::Repository) -> Option<gix::ObjectId> {
    repo.head_tree_id().ok().map(|id| id.detach())
}

/// The tree of commit `commit`.
pub fn commit_tree_id(repo: &gix::Repository, commit: gix::ObjectId) -> Option<gix::ObjectId> {
    repo.find_commit(commit).ok()?.tree_id().ok().map(|id| id.detach())
}

/// A local branch's tip.
pub fn local_branch_tip(repo: &gix::Repository, branch: &str) -> Option<gix::ObjectId> {
    let mut r = repo.try_find_reference(format!("refs/heads/{branch}").as_str()).ok()??;
    r.peel_to_id().ok().map(|id| id.detach())
}

/// A caller's revision spec resolved to a commit (after
/// [`crate::git::refuse_message_search_revspec`]).
pub fn resolve_commit(repo: &gix::Repository, spec: &str) -> Result<gix::ObjectId, String> {
    let id = repo.rev_parse_single(spec).map_err(|e| e.to_string())?;
    let commit = id.object().map_err(|e| e.to_string())?.peel_to_commit().map_err(|e| e.to_string())?;
    Ok(commit.id)
}

/// Commits reachable from `a` and not from `b`, and the other way round
/// (git2's `graph_ahead_behind(a, b)`).
pub fn ahead_behind(repo: &gix::Repository, a: gix::ObjectId, b: gix::ObjectId) -> Option<(usize, usize)> {
    if a == b {
        return Some((0, 0));
    }
    let count = |tip: gix::ObjectId, hide: gix::ObjectId| -> Option<usize> {
        let walk = repo.rev_walk([tip]).with_hidden([hide]).all().ok()?;
        let mut n = 0usize;
        for info in walk {
            info.ok()?;
            n += 1;
        }
        Some(n)
    };
    Some((count(a, b)?, count(b, a)?))
}

/// The best common ancestor of two commits.
pub fn merge_base(repo: &gix::Repository, a: gix::ObjectId, b: gix::ObjectId) -> Option<gix::ObjectId> {
    repo.merge_base(a, b).ok().flatten().map(|id| id.detach())
}

/// The upstream a local branch tracks, as git2's `Branch::upstream()`:
/// `(shorthand, tip)`, only when the tracking ref exists. A `branch.<b>.remote`
/// of `.` tracks a local branch.
pub fn upstream_of(repo: &gix::Repository, branch: &str) -> Option<(String, gix::ObjectId)> {
    let snap = repo.config_snapshot();
    let cfg = snap.plumbing();
    let remote = cfg.string(format!("branch.{branch}.remote").as_str())?;
    let merge = cfg.string(format!("branch.{branch}.merge").as_str())?;
    let tracking: String = if remote.as_slice() == b"." {
        merge.to_string()
    } else {
        let full = gix::refs::FullName::try_from(format!("refs/heads/{branch}")).ok()?;
        repo.branch_remote_tracking_ref_name(full.as_ref(), gix::remote::Direction::Fetch)?.ok()?.as_bstr().to_string()
    };
    let mut r = repo.try_find_reference(tracking.as_str()).ok()??;
    let tip = r.peel_to_id().ok()?.detach();
    Some((short_name(r.name()), tip))
}

/// git's state names for an operation in progress.
pub fn state_name(repo: &gix::Repository) -> &'static str {
    use gix::state::InProgress as P;
    match repo.state() {
        None => "clean",
        Some(P::Merge) => "merge",
        Some(P::Revert | P::RevertSequence) => "revert",
        Some(P::CherryPick | P::CherryPickSequence) => "cherry_pick",
        Some(P::Bisect) => "bisect",
        Some(P::Rebase | P::RebaseInteractive) => "rebase",
        Some(P::ApplyMailbox | P::ApplyMailboxRebase) => "apply_mailbox",
    }
}

/// What a tree or index entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Blob,
    Executable,
    Link,
    /// A submodule (gitlink).
    Commit,
    Tree,
}

impl EntryKind {
    pub fn from_tree_mode(m: gix::objs::tree::EntryMode) -> Self {
        use gix::objs::tree::EntryKind as K;
        match m.kind() {
            K::Blob => EntryKind::Blob,
            K::BlobExecutable => EntryKind::Executable,
            K::Link => EntryKind::Link,
            K::Commit => EntryKind::Commit,
            K::Tree => EntryKind::Tree,
        }
    }
    pub fn from_index_mode(m: gix::index::entry::Mode) -> Self {
        use gix::index::entry::Mode;
        if m == Mode::SYMLINK {
            EntryKind::Link
        } else if m == Mode::COMMIT {
            EntryKind::Commit
        } else if m == Mode::FILE_EXECUTABLE {
            EntryKind::Executable
        } else if m == Mode::DIR {
            EntryKind::Tree
        } else {
            EntryKind::Blob
        }
    }
    /// A plain or executable file.
    pub fn is_blob(self) -> bool {
        matches!(self, EntryKind::Blob | EntryKind::Executable)
    }
    /// Kinds that differ beyond the exec bit (git's "typechange").
    pub fn is_typechange(self, other: EntryKind) -> bool {
        !(self == other || (self.is_blob() && other.is_blob()))
    }
}

/// A file's entry in tree `tree`: `(kind, id)`.
pub fn tree_entry(repo: &gix::Repository, tree: gix::ObjectId, rel: &str) -> Option<(EntryKind, gix::ObjectId)> {
    let tree = repo.find_tree(tree).ok()?;
    let e = tree.lookup_entry_by_path(rel).ok()??;
    Some((EntryKind::from_tree_mode(e.mode()), e.object_id()))
}

/// The bytes of blob `id`.
pub fn blob_bytes(repo: &gix::Repository, id: gix::ObjectId) -> Option<Vec<u8>> {
    repo.find_blob(id).ok().map(|b| b.data.clone())
}

/// The size of object `id` without reading it whole.
pub fn object_size(repo: &gix::Repository, id: gix::ObjectId) -> Option<u64> {
    repo.find_header(id).ok().map(|h| h.size())
}

/// git's blob id for `bytes` (raw, no filters: `git hash-object --no-filters`).
pub fn hash_blob(bytes: &[u8]) -> Option<gix::ObjectId> {
    gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, bytes).ok()
}

/// git's blob id for the file at `path`, streamed (big files).
pub fn hash_file(path: &Path) -> Option<gix::ObjectId> {
    let meta = std::fs::metadata(path).ok()?;
    let mut f = std::fs::File::open(path).ok()?;
    gix::objs::compute_stream_hash(
        gix::hash::Kind::Sha1,
        gix::objs::Kind::Blob,
        &mut f,
        meta.len(),
        &mut gix::progress::Discard,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .ok()
}

/// The stage-0 entry for `rel` in the repository's index.
pub fn index_entry(index: &gix::index::State, rel: &str) -> Option<(EntryKind, gix::ObjectId)> {
    let e = index.entry_by_path_and_stage(rel_bstr(rel), gix::index::entry::Stage::Unconflicted)?;
    Some((EntryKind::from_index_mode(e.mode), e.id))
}

/// Paths with a conflict stage (1–3) in the index, sorted, once each.
pub fn conflicted_paths(index: &gix::index::State) -> Vec<String> {
    let mut out: Vec<String> = index
        .entries()
        .iter()
        .filter(|e| e.stage() != gix::index::entry::Stage::Unconflicted)
        .map(|e| e.path(index).to_string())
        .collect();
    out.dedup();
    out
}

/// Is `branch` a valid `refs/heads/<branch>` name?
pub fn valid_branch_name(branch: &str) -> bool {
    gix::validate::reference::name(rel_bstr(&format!("refs/heads/{branch}"))).is_ok()
}

/// Answers "is this path ignored" for one repository, as git does
/// (`.gitignore`s, `info/exclude`, `core.excludesFile`).
pub struct Excludes {
    repo: gix::Repository,
    stack: gix::worktree::Stack,
}

impl Excludes {
    pub fn new(repo: gix::Repository) -> Option<Self> {
        let index = repo.index_or_empty().ok()?;
        let stack = repo
            .excludes(&index, None, gix::worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped)
            .ok()?
            .detach();
        Some(Self { repo, stack })
    }

    /// `rel` is repo-relative; `is_dir` says how to match directory-only patterns.
    pub fn is_ignored(&mut self, rel: &str, is_dir: bool) -> bool {
        let mode = is_dir.then_some(gix::index::entry::Mode::DIR);
        match self.stack.at_entry(rel_bstr(rel), mode, &self.repo.objects) {
            Ok(p) => p.is_excluded(),
            Err(_) => false,
        }
    }

    pub fn repo(&self) -> &gix::Repository {
        &self.repo
    }
}

#[cfg(test)]
#[path = "gitx_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "callers_tests.rs"]
mod callers_tests;

#[cfg(test)]
#[path = "oracle_tests.rs"]
mod oracle_tests;
