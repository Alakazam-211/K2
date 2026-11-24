//! compute_routes' `repo_pick` (which repo of the workspace a job ships),
//! moved out as it was (0.46.18 queue, ratchet).

use super::*;

/// Which repo of the workspace a job ships: `--repo <subdir>` (a nested
/// repo, jailed like a caller's worktree: `sync::nested_repo`), else the
/// workspace's own repo (run from another repo: that one, as `--repo`
/// would, or refused when it can't ship), else (a multi-repo workspace whose root isn't a
/// repo) the nested repo the caller runs from. Never a repo outside the
/// workspace, never `.k2/`.
pub(crate) fn repo_pick(ws: &Ws, v: &serde_json::Value) -> R<RepoPick> {
    let ws_dir = Path::new(&ws.path);
    if let Some(rel) = s(v, "repo").filter(|r| !r.is_empty()) {
        let n = compute::sync::nested_repo(ws_dir, rel).map_err(|e| no(REPO_INVALID, 400, e))?;
        return Ok(nested_pick(n, false));
    }
    let fenced = crate::agent_isolation::workspace_and_fence(&ws.id).1;
    match compute::sync::workspace_repo_root(ws_dir, fenced) {
        Ok(home) => crate::compute_repo_pick::in_repo_workspace(ws, home, s(v, "repoDir")),
        // M2 (S0): a fenced workspace inside a larger repo ships only a repo
        // of its own (`--repo`), never the larger one.
        Err(e) if fenced && compute::sync::repo_root(ws_dir).is_ok() => Err(no(REPO_INVALID, 400, e)),
        Err(_) => {
            let from_caller = s(v, "repoDir")
                .filter(|d| !d.is_empty())
                .map(|d| compute::sync::nested_repo_of_caller(ws_dir, Path::new(d)))
                .transpose()
                .map_err(|e| no(REPO_INVALID, 400, e))?
                .flatten();
            match from_caller {
                Some(n) => Ok(crate::compute_repo_pick::in_plain_workspace(ws, n)),
                None => Err(no(
                    codes::NOT_A_REPO,
                    400,
                    format!(
                        "workspace {} isn't a git repo; run from inside one of its repos, pass --repo <folder> (a git repo inside the workspace), or --no-source to run without code",
                        ws.handle
                    ),
                )),
            }
        }
    }
}
