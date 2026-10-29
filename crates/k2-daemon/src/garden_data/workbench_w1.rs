//! The Workbench's one door to Lane W1's daemon core (0.46.7, vs-live
//! B1–B3: worktree sidecars, Stop, Land, Retire, migration 0251).
//!
//! **Every Workbench action goes through this file and nothing else.** The
//! Garden rows (`workbench_rows`) check the widget, the press, the caps and
//! K2's sheet; this file only runs the action, as the person (the owner) on
//! this computer. Until W1's routes are on this branch each action answers
//! `verb_unavailable` with words a person can act on, so wiring them later
//! is a change to this file only (`.k2/prds/workbench-w1-contract.md`).
//!
//! Agent parity: each action is a W1 CLI verb an agent runs with its own
//! passport (`k2 sidecar new --worktree`, `k2 interrupt`, `k2 land`,
//! `k2 sidecar retire`); an agent never confirms its own Land.

use serde_json::Value as J;

use super::person_rows::no;
use crate::cli_response::CliResponse;

/// Whether W1's routes are wired here. Tests and the widget's own copy read
/// the refusal, never this flag.
pub const WIRED: bool = false;

/// What K2 says while an action isn't wired yet.
pub fn not_yet(action: &str) -> CliResponse {
    no(
        503,
        "verb_unavailable",
        format!(
            "{action} arrives with K2's worktree agents (Workbench lane W1). Until then, ask the agent in its Thread, \
             or run it yourself in a terminal."
        ),
    )
}

/// A worktree agent to start (`POST /cli/sidecar/new {worktree: true}`).
#[derive(Debug, Clone)]
pub struct StartReq {
    pub workspace_id: String,
    pub workspace_handle: String,
    pub name: String,
    pub brief: String,
    pub harness: Option<String>,
    pub base: Option<String>,
}

/// `{address, thread, branch, worktree}` (the catalog's WorkStarted).
pub fn start(req: &StartReq) -> Result<J, CliResponse> {
    let _ = req;
    Err(not_yet("Starting an agent in a new worktree"))
}

/// Stop the agent's current turn (`POST /cli/session/interrupt`).
pub fn stop(workspace_handle: &str, sidecar: &str) -> Result<J, CliResponse> {
    let _ = (workspace_handle, sidecar);
    Err(not_yet("Stop"))
}

/// Land `branch` onto the base the sheet showed (`POST /cli/git/land`,
/// bound to the tips the person saw: a moved tip is `conflict`).
pub fn land(workspace_handle: &str, sidecar: &str, expect_onto: &str, expect_tip: &str) -> Result<J, CliResponse> {
    let _ = (workspace_handle, sidecar, expect_onto, expect_tip);
    Err(not_yet("Land"))
}

/// Retire (or discard) a worktree agent (`POST /cli/sidecar/retire`).
pub fn settle(workspace_handle: &str, sidecar: &str, discard: bool) -> Result<J, CliResponse> {
    let _ = (workspace_handle, sidecar, discard);
    Err(not_yet(if discard { "Discard" } else { "Retire" }))
}

/// W1's own facts on a sidecar (0251: `base_ref`, `landed_at`,
/// `retired_at`, and E9's `taskState`), when it has them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFacts {
    pub base: Option<String>,
    pub task_state: Option<String>,
    pub landed_at: Option<i64>,
    pub retired_at: Option<i64>,
}

pub fn task_facts(project_id: &str, pane_group_id: &str) -> TaskFacts {
    let _ = (project_id, pane_group_id);
    TaskFacts::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_says_it_is_not_wired_yet() {
        assert!(!WIRED);
        let req = StartReq {
            workspace_id: "p".into(),
            workspace_handle: "k2".into(),
            name: "fix".into(),
            brief: "b".into(),
            harness: None,
            base: None,
        };
        for r in [start(&req), stop("k2", "fix"), land("k2", "fix", "a", "b"), settle("k2", "fix", true)] {
            let e = r.expect_err("not wired");
            assert_eq!(e.status, "503 Service Unavailable");
            let v: J = serde_json::from_str(&e.body).expect("json");
            assert_eq!(v["error"]["code"], "verb_unavailable");
            assert!(v["error"]["hint"].as_str().expect("hint").contains("W1"), "{}", e.body);
        }
        assert_eq!(task_facts("p", "pg"), TaskFacts::default());
    }
}
