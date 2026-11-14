//! The repo a run ships when no `--repo` names one and the caller's
//! folder (`repoDir`) decides it (`compute_routes::repo_pick`). The rule
//! lives in `k2_core::compute::caller_repo`; this file turns it into a
//! [`RepoPick`] with the one line the caller sees, or a refusal.

use std::path::{Path, PathBuf};

use k2_core::compute::caller_repo::{caller_repo, CallerRepo};
use k2_core::compute::sync::NestedRepo;

use crate::compute_routes::{nested_pick, no, RepoPick, Ws, R, REPO_INVALID};

/// The workspace folder is a repo (`home`): its own code, unless the
/// caller runs from another repo. One inside the workspace ships instead,
/// with a note; one that can't ship is refused (`repo_invalid`, exit 2)
/// with the way in. Never the workspace's code in place of another repo's.
pub(crate) fn in_repo_workspace(ws: &Ws, home: PathBuf, repo_dir: Option<&str>) -> R<RepoPick> {
    match repo_dir.filter(|d| !d.is_empty()).map(|d| caller_repo(Path::new(&ws.path), &home, Path::new(d))) {
        Some(CallerRepo::Other(n)) => {
            let note = format!("shipping {} (the repo you're in), not the workspace repo", n.rel);
            Ok(RepoPick { note: Some(note), ..nested_pick(n, true) })
        }
        Some(CallerRepo::Refused(why)) => Err(no(REPO_INVALID, 400, why)),
        _ => Ok(RepoPick { home, named: None, rel: None, auto: false, note: None }),
    }
}

/// The workspace folder isn't a repo: the nested repo the caller runs
/// from, with the note that says so.
pub(crate) fn in_plain_workspace(ws: &Ws, n: NestedRepo) -> RepoPick {
    let note = (!n.rel.is_empty()).then(|| format!("workspace {} isn't a git repo: shipping its repo {} (the one you're in; --repo picks another)", ws.handle, n.rel));
    RepoPick { note, ..nested_pick(n, true) }
}

#[cfg(test)]
#[path = "compute_caller_repo_tests.rs"]
mod caller_repo_tests;
