//! The Workbench's one door to Lane W1's daemon core (0.46.7,
//! `.k2/prds/workbench-w1-contract.md`: worktree sidecars, Stop, Review,
//! Land, Retire, migration 0251).
//!
//! **Every Workbench action goes through this file and nothing else.** The
//! Garden rows (`workbench_rows`) check the widget, the press, the caps and
//! K2's sheet; this file runs W1's own `/cli/sidecar/*` route as the person
//! (the owner) on this computer, so the floors, the per-repo Land lock, the
//! Thread posts and the rollbacks are W1's, never a second copy.
//!
//! Discard is always `POST /cli/sidecar/retire {discard: true}`, never a
//! tab close (closing a worktree agent's tab would orphan its worktree).
//!
//! Agent parity: each action is a W1 CLI verb (`k2 sidecar new --worktree`,
//! `k2 sidecar interrupt`, `k2 sidecar land`, `k2 sidecar retire
//! [--discard]`); an agent never lands or retires (403 `land_needs_person` /
//! `retire_needs_person`, which the `git.land` / `sidecar.retire` cards ask).

use std::collections::HashMap;

use serde_json::{json, Value as J};

use super::person_rows::passthrough;
use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

/// Whether W1's routes are wired here.
pub const WIRED: bool = true;

/// The preset a New task uses when the person names none.
pub const DEFAULT_HARNESS: &str = "claude";

/// The audit ingress for Garden calls.
const INGRESS: &str = "garden";

fn post(path: &str, body: J) -> Result<J, (u16, J, CliResponse)> {
    let r = crate::sidecar_routes::handle(path, Caller::Owner, body.to_string().as_bytes(), &HashMap::new(), INGRESS);
    let code: u16 = r.status.split(' ').next().and_then(|c| c.parse().ok()).unwrap_or(500);
    let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
    if code == 200 {
        Ok(v)
    } else {
        Err((code, v, r))
    }
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
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
    // W1's `sidecar new` needs a preset; the Workbench's default is Claude.
    let harness = req.harness.as_deref().unwrap_or(DEFAULT_HARNESS);
    let mut body = json!({ "workspace": req.workspace_handle, "name": req.name, "brief": req.brief, "worktree": true, "harness": harness });
    if let Some(b) = &req.base {
        body["base"] = json!(b);
    }
    let v = post("/cli/sidecar/new", body).map_err(|(_, _, r)| passthrough(r))?;
    let thread = v["address"].as_str().unwrap_or_default().to_string();
    Ok(json!({
        "address": format!("{thread}::local"),
        "thread": thread,
        "branch": v["worktree"]["branch"],
        "worktree": v["worktree"]["dir"],
    }))
}

/// Stop the agent's current turn (`POST /cli/sidecar/interrupt`).
pub fn stop(workspace_handle: &str, sidecar: &str) -> Result<J, CliResponse> {
    let v = post("/cli/sidecar/interrupt", json!({ "workspace": workspace_handle, "name": sidecar })).map_err(|(_, _, r)| passthrough(r))?;
    Ok(json!({ "stopped": v["interrupted"].as_bool().unwrap_or(false) }))
}

/// W1's Land sheet read (`GET /cli/sidecar/review`): the commits, files,
/// tips and why Land would refuse.
pub fn review(workspace_handle: &str, sidecar: &str) -> Result<J, CliResponse> {
    let mut params = HashMap::new();
    params.insert("workspace".to_string(), workspace_handle.to_string());
    params.insert("name".to_string(), sidecar.to_string());
    let r = crate::sidecar_routes::handle("/cli/sidecar/review", Caller::Owner, b"", &params, INGRESS);
    if r.status != "200 OK" {
        return Err(passthrough(r));
    }
    serde_json::from_str(&r.body).map_err(|e| super::person_rows::no(500, "failed", format!("sidecar review: {e}")))
}

/// Land onto the base the sheet showed (`POST /cli/sidecar/land`, bound to
/// the tips the person saw: a moved tip is W1's `stale`). A conflict is an
/// answer, not a refusal: the widget hears `{landed: false, conflict}`.
pub fn land(workspace_handle: &str, sidecar: &str, expect_onto: &str, expect_tip: &str) -> Result<J, CliResponse> {
    let mut body = json!({ "workspace": workspace_handle, "name": sidecar, "expectTip": expect_tip, "retire": true });
    if !expect_onto.is_empty() {
        body["expectOnto"] = json!(expect_onto);
    }
    match post("/cli/sidecar/land", body) {
        Ok(v) => Ok(json!({
            "landed": true,
            "commits": v["count"].as_i64().unwrap_or(0),
            "head": v["to"].as_str().map(short),
            "conflict": [],
        })),
        Err((409, v, _)) if v["error"]["code"] == "conflict" => Ok(json!({
            "landed": false,
            "commits": 0,
            "head": J::Null,
            "conflict": v["error"]["files"].as_array().cloned().unwrap_or_default(),
        })),
        Err((_, _, r)) => Err(passthrough(r)),
    }
}

/// Retire (or discard) a worktree agent (`POST /cli/sidecar/retire`).
pub fn settle(workspace_handle: &str, sidecar: &str, discard: bool) -> Result<J, CliResponse> {
    let v = post("/cli/sidecar/retire", json!({ "workspace": workspace_handle, "name": sidecar, "discard": discard }))
        .map_err(|(_, _, r)| passthrough(r))?;
    Ok(json!({ "settled": v["retired"].as_bool().unwrap_or(false), "discarded": v["how"] == "discarded" }))
}

/// W1's own facts on a sidecar (0251's columns and the daemon's task
/// state), in the Workbench's words: needs-you, working, ready, landed,
/// retired, or null for a new task.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFacts {
    pub base: Option<String>,
    pub branch: Option<String>,
    pub dir: Option<String>,
    /// Commits not on `onto`, by patch-id.
    pub ahead: Option<i64>,
    pub task_state: Option<String>,
    pub landed_at: Option<i64>,
    pub retired_at: Option<i64>,
}

/// W1's `taskState` (+ activity) → the Workbench's.
pub fn workbench_state(task_state: &str, activity: Option<&str>) -> Option<&'static str> {
    match task_state {
        "working" if activity == Some("waiting") => Some("needs-you"),
        "working" => Some("working"),
        "review" => Some("ready"),
        "landed" => Some("landed"),
        "discarded" => Some("retired"),
        _ => None,
    }
}

pub fn task_facts(ws_id: &str, ws_path: &str, ws_handle: &str, pane_group_id: &str, agent_name: Option<&str>) -> TaskFacts {
    if pane_group_id.is_empty() {
        return TaskFacts::default();
    }
    let ws = crate::sidecar_routes::Workspace { id: ws_id.to_string(), path: ws_path.to_string(), handle: ws_handle.to_string() };
    let session = agent_name.and_then(crate::v2_session_map::lookup_by_agent_name).filter(|s| s.is_child_alive());
    let Some(x) = crate::sidecar_worktree::list_extras(&ws, pane_group_id, session.as_deref()) else {
        return TaskFacts::default();
    };
    let w = &x["worktree"];
    TaskFacts {
        base: w["onto"].as_str().map(str::to_string),
        branch: w["branch"].as_str().map(str::to_string),
        dir: w["dir"].as_str().filter(|_| w["retiredAt"].is_null()).map(str::to_string),
        ahead: w["ahead"].as_i64(),
        task_state: x["taskState"].as_str().and_then(|t| workbench_state(t, x["activity"].as_str())).map(str::to_string),
        landed_at: w["landedAt"].as_i64(),
        retired_at: w["retiredAt"].as_i64(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn w1_states_read_in_the_workbenchs_words() {
        assert_eq!(workbench_state("working", Some("waiting")), Some("needs-you"));
        assert_eq!(workbench_state("working", Some("working")), Some("working"));
        assert_eq!(workbench_state("review", None), Some("ready"));
        assert_eq!(workbench_state("landed", None), Some("landed"));
        assert_eq!(workbench_state("discarded", None), Some("retired"));
        assert_eq!(workbench_state("new", None), None);
        assert!(WIRED);
        assert_eq!(task_facts("p", "/nowhere", "k2", "", None), TaskFacts::default());
    }
}
