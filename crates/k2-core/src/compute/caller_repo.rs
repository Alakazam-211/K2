//! The repo a run's caller is in, when the workspace folder is itself a
//! repo: `caller_dir` is the top of the checkout the CLI ran from
//! (`repoDir`). A different repo must never fall back to the workspace's
//! own code without a word. One inside the workspace that `--repo` accepts
//! ships instead; one it can't ship is refused, with the way to make it
//! work. The workspace's own checkout and its worktrees keep their path
//! ([`super::sync::caller_checkout`]).
//!
//! Never runs git in a folder outside the workspace: whether an outside
//! folder is one of the workspace repo's worktrees comes from that repo's
//! own worktree list.

use std::path::{Path, PathBuf};

use super::sync::{self, NestedRepo};

/// What the caller's folder means for the code a run ships.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerRepo {
    /// The workspace's own repo or one of its worktrees (inside the
    /// workspace or not), or nothing to go on (not an absolute folder, not
    /// on this machine, not in a repo): the workspace path, as before.
    Workspace,
    /// Another repo inside the workspace that `--repo` would accept.
    Other(NestedRepo),
    /// Another repo that can't ship (outside the workspace, a worktree of
    /// a repo outside it, under `.k2/`): why, ending with [`inside_hint`].
    Refused(String),
}

/// The way to ship a repo that isn't inside the workspace: every refusal
/// of one ends with it.
pub fn inside_hint(ws_dir: &Path) -> String {
    format!(
        "a job ships only code inside the workspace: clone the repo into it (e.g. {}/.worktrees/<name>, gitignored) and pass --repo .worktrees/<name>, or run from inside that clone",
        ws_dir.display()
    )
}

fn canon(p: &Path) -> Option<PathBuf> {
    dunce::canonicalize(p).ok()
}

/// The checkouts of the repo at `root` (its main one and every linked
/// worktree), canonical, from `git worktree list` run in `root`.
fn checkouts_of(root: &Path) -> Vec<PathBuf> {
    crate::git::list_worktrees(&root.to_string_lossy()).iter().filter_map(|w| canon(Path::new(&w.path))).collect()
}

/// See [`CallerRepo`]. `ws_root` is the workspace's own repo (the top of
/// the checkout the workspace folder is in).
pub fn caller_repo(ws_dir: &Path, ws_root: &Path, caller_dir: &Path) -> CallerRepo {
    if !caller_dir.is_absolute() {
        return CallerRepo::Workspace;
    }
    let (Some(caller), Some(ws), Some(ws_root)) = (canon(caller_dir), canon(ws_dir), canon(ws_root)) else {
        return CallerRepo::Workspace;
    };
    if caller == ws_root {
        return CallerRepo::Workspace;
    }
    if !caller.starts_with(&ws) {
        if checkouts_of(&ws_root).contains(&caller) {
            return CallerRepo::Workspace;
        }
        return CallerRepo::Refused(format!("{} (the repo you're in) is outside the workspace; {}", caller_dir.display(), inside_hint(&ws)));
    }
    let Some(top) = sync::repo_root(&caller).ok().and_then(|t| canon(&t)) else { return CallerRepo::Workspace };
    if top == ws_root {
        return CallerRepo::Workspace;
    }
    let shared = |p: &Path| sync::common_dir(p).ok().and_then(|c| canon(&c));
    if shared(&top).is_some_and(|c| Some(c) == shared(&ws_root)) {
        return CallerRepo::Workspace;
    }
    match sync::nested_repo_of_caller(&ws, &top) {
        Ok(Some(n)) if !n.rel.is_empty() => CallerRepo::Other(n),
        Ok(_) => CallerRepo::Workspace,
        Err(e) => {
            let mut why = format!("{} (the repo you're in) isn't the workspace's repo and can't ship: {e}", top.display());
            if !why.contains(".worktrees/<name>") {
                why = format!("{why}; {}", inside_hint(&ws));
            }
            CallerRepo::Refused(why)
        }
    }
}
