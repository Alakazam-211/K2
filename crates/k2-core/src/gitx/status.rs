//! `git status` through gix, merged per path the way libgit2 reported it.
//!
//! gix reports a path's HEAD-vs-index change and its index-vs-worktree
//! change as two separate items (research §7.3); K2 counts paths, so
//! [`statuses`] folds them into one [`PathStatus`] per path, with git2's
//! flag names. Every mode is set explicitly, never read from the person's
//! `status.*` config: untracked files (none, collapsed folders, or every
//! file), staged renames (on or off), worktree renames (always off, as
//! libgit2's default) and submodules.
//!
//! Reads never write: the stat refreshes gix finds are dropped, never
//! written back to `.git/index` (the file watcher would see a write as a
//! change and loop).

use std::collections::BTreeMap;

use super::EntryKind;

/// git2's status flags, the ones K2 reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags(u32);

impl Flags {
    pub const INDEX_NEW: Flags = Flags(1);
    pub const INDEX_MODIFIED: Flags = Flags(1 << 1);
    pub const INDEX_DELETED: Flags = Flags(1 << 2);
    pub const INDEX_RENAMED: Flags = Flags(1 << 3);
    pub const INDEX_TYPECHANGE: Flags = Flags(1 << 4);
    pub const WT_NEW: Flags = Flags(1 << 7);
    pub const WT_MODIFIED: Flags = Flags(1 << 8);
    pub const WT_DELETED: Flags = Flags(1 << 9);
    pub const WT_TYPECHANGE: Flags = Flags(1 << 10);
    pub const CONFLICTED: Flags = Flags(1 << 15);

    pub const INDEX_ANY: Flags = Flags(0b1_1111);
    pub const WT_ANY: Flags = Flags(0b1111 << 7);

    pub fn contains(self, f: Flags) -> bool {
        self.0 & f.0 == f.0
    }
    pub fn intersects(self, f: Flags) -> bool {
        self.0 & f.0 != 0
    }
    pub fn insert(&mut self, f: Flags) {
        self.0 |= f.0;
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Flags {
    type Output = Flags;
    fn bitor(self, o: Flags) -> Flags {
        Flags(self.0 | o.0)
    }
}

/// One path's merged status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathStatus {
    /// Repo-relative, `/`-separated. An untracked folder (collapsed mode)
    /// ends with `/`, as libgit2 reported it.
    pub path: String,
    pub flags: Flags,
    /// The HEAD path of a staged rename.
    pub renamed_from: Option<String>,
}

/// Untracked files: none, folders collapsed (`git status`'s default), or
/// every file (`-uall`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Untracked {
    None,
    Collapsed,
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub untracked: Untracked,
    /// Pair staged deletions + additions into renames (50% similarity,
    /// renames only: libgit2's `find_similar` default).
    pub staged_renames: bool,
    /// Look inside submodules (a dirty submodule is WT_MODIFIED, as
    /// configured by `submodule.<name>.ignore`); off skips them.
    pub submodules: bool,
}

impl Options {
    /// The Changes tab / `git status` view.
    pub fn changes() -> Self {
        Options { untracked: Untracked::Collapsed, staged_renames: true, submodules: true }
    }
}

/// The default rename settings libgit2 used: renames (no copies) at 50%.
pub fn rename_rules() -> gix::diff::Rewrites {
    gix::diff::Rewrites { copies: None, percentage: Some(0.5), limit: 1000, track_empty: false }
}

fn slash(p: &gix::bstr::BStr) -> String {
    p.to_string().replace('\\', "/")
}

/// The status of every changed path in `repo`'s work tree, sorted by path.
/// `head_tree` overrides HEAD's tree as the "HEAD" side (a base view);
/// None = HEAD (empty when unborn).
pub fn statuses_against(
    repo: &gix::Repository,
    opts: Options,
    head_tree: Option<gix::ObjectId>,
) -> Result<Vec<PathStatus>, String> {
    use gix::status::index_worktree::Item as Wt;
    use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};

    let untracked = match opts.untracked {
        Untracked::None => gix::status::UntrackedFiles::None,
        Untracked::Collapsed => gix::status::UntrackedFiles::Collapsed,
        Untracked::Files => gix::status::UntrackedFiles::Files,
    };
    let renames = if opts.staged_renames {
        gix::status::tree_index::TrackRenames::Given(rename_rules())
    } else {
        gix::status::tree_index::TrackRenames::Disabled
    };
    let submodules = opts.submodules.then_some(gix::status::Submodule::AsConfigured { check_dirty: true });
    let mut platform = repo
        .status(gix::progress::Discard)
        .map_err(|e| e.to_string())?
        .untracked_files(untracked)
        .index_worktree_rewrites(None)
        .tree_index_track_renames(renames)
        .index_worktree_submodules(submodules);
    if let Some(t) = head_tree {
        platform = platform.head_tree(t);
    }
    let iter = platform.into_iter(Vec::<gix::bstr::BString>::new()).map_err(|e| e.to_string())?;

    let mut by_path: BTreeMap<String, PathStatus> = BTreeMap::new();
    let mut add = |path: String, f: Flags, from: Option<String>| {
        let e = by_path.entry(path.clone()).or_insert(PathStatus { path, flags: Flags::default(), renamed_from: None });
        e.flags.insert(f);
        if from.is_some() {
            e.renamed_from = from;
        }
    };
    for item in iter {
        let item = item.map_err(|e| e.to_string())?;
        match item {
            gix::status::Item::TreeIndex(change) => {
                use gix::diff::index::ChangeRef as C;
                match change {
                    C::Addition { location, .. } => add(slash(location.as_ref()), Flags::INDEX_NEW, None),
                    C::Deletion { location, .. } => add(slash(location.as_ref()), Flags::INDEX_DELETED, None),
                    C::Modification { location, previous_entry_mode, entry_mode, .. } => {
                        let typechange = EntryKind::from_index_mode(previous_entry_mode)
                            .is_typechange(EntryKind::from_index_mode(entry_mode));
                        let f = if typechange { Flags::INDEX_TYPECHANGE } else { Flags::INDEX_MODIFIED };
                        add(slash(location.as_ref()), f, None)
                    }
                    C::Rewrite { source_location, location, copy, .. } => {
                        if copy {
                            add(slash(location.as_ref()), Flags::INDEX_NEW, None)
                        } else {
                            add(slash(location.as_ref()), Flags::INDEX_RENAMED, Some(slash(source_location.as_ref())))
                        }
                    }
                }
            }
            gix::status::Item::IndexWorktree(Wt::Modification { rela_path, status, .. }) => {
                let f = match status {
                    EntryStatus::Conflict { .. } => Flags::CONFLICTED,
                    EntryStatus::Change(Change::Removed) => Flags::WT_DELETED,
                    EntryStatus::Change(Change::Type { .. }) => Flags::WT_TYPECHANGE,
                    EntryStatus::Change(Change::Modification { .. }) => Flags::WT_MODIFIED,
                    EntryStatus::Change(Change::SubmoduleModification(_)) => Flags::WT_MODIFIED,
                    // Stat-only differences (the iterator drops most of
                    // them) and intent-to-add entries: nothing changed.
                    EntryStatus::NeedsUpdate(_) | EntryStatus::IntentToAdd => continue,
                };
                add(slash(rela_path.as_ref()), f, None)
            }
            gix::status::Item::IndexWorktree(Wt::DirectoryContents { entry, .. }) => {
                if entry.status != gix::dir::entry::Status::Untracked {
                    continue;
                }
                let mut p = slash(entry.rela_path.as_ref());
                let is_dir = matches!(
                    entry.disk_kind,
                    Some(gix::dir::entry::Kind::Directory | gix::dir::entry::Kind::Repository)
                );
                if is_dir && !p.ends_with('/') {
                    p.push('/');
                }
                add(p, Flags::WT_NEW, None)
            }
            // Worktree rename tracking is off.
            gix::status::Item::IndexWorktree(Wt::Rewrite { .. }) => {}
        }
    }
    Ok(by_path.into_values().collect())
}

/// [`statuses_against`] HEAD.
pub fn statuses(repo: &gix::Repository, opts: Options) -> Result<Vec<PathStatus>, String> {
    statuses_against(repo, opts, None)
}
