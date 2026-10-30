//! The one workspace session list (prd-workspace-sessions-cli-v1 WS-R4, G6).
//!
//! [`workspace_roster`] answers "who is working in this workspace": the
//! main (pinned) chat, sidecars, extra chat tabs a person opened, and
//! heartbeat runs, each with the address `k2 msg` takes. It is the single
//! source for `GET /cli/sessions/roster` (`k2 sessions`), the "This
//! workspace" section of `k2 connections`, `k2 whoami`'s `others:` count
//! and Workbench W2's agent list (contract:
//! `.k2/prds/workspace-sessions-contract.md`).
//!
//! It reads memory and SQLite only (WS-R5): the live session map (a
//! session belongs to the longest registered project prefix of its cwd,
//! like `list-for-workspace`), the activity store, the durable tab rows
//! with their handles and sidecar columns, and `workspace_sessions`. No
//! transcript is opened.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use k2_core::terminal::DaemonPtySession;
use serde_json::{json, Value};

use crate::v2_session_map;

/// What to include.
#[derive(Debug, Clone, Default)]
pub struct RosterOpts {
    /// Include asleep rows (tab rows with no runner; main when asleep).
    pub all: bool,
    /// Include `api-*` host sessions (role `api`; the owner's view).
    pub include_api: bool,
    /// The calling session's map key, to mark `you`.
    pub caller_map_key: Option<String>,
}

/// One session in the list (the contract's row).
#[derive(Debug, Clone, PartialEq)]
pub struct RosterRow {
    pub address: Option<String>,
    pub handle: Option<String>,
    /// `main | sidecar | worktree | chat | heartbeat | api`.
    pub role: &'static str,
    pub cli: Option<String>,
    pub preset: Option<String>,
    /// `working | waiting | idle | asleep`.
    pub state: &'static str,
    pub activity: Option<Value>,
    pub last_active_at: Option<i64>,
    pub title: Option<String>,
    pub brief: Option<String>,
    pub conversation_id: Option<String>,
    pub session_id: Option<String>,
    pub agent_name: Option<String>,
    pub pane_group_id: Option<String>,
    pub transport: &'static str,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    pub opened_by: Option<String>,
    pub attached: bool,
    pub you: bool,
}

impl RosterRow {
    pub fn to_json(&self) -> Value {
        json!({
            "address": self.address,
            "handle": self.handle,
            "role": self.role,
            "cli": self.cli,
            "preset": self.preset,
            "state": self.state,
            "activity": self.activity,
            "lastActiveAt": self.last_active_at,
            "title": self.title,
            "brief": self.brief,
            "conversationId": self.conversation_id,
            "sessionId": self.session_id,
            "agentName": self.agent_name,
            "paneGroupId": self.pane_group_id,
            "transport": self.transport,
            "worktree": self.worktree,
            "branch": self.branch,
            "openedBy": self.opened_by,
            "attached": self.attached,
            "you": self.you,
        })
    }

    pub fn is_live(&self) -> bool {
        self.state != "asleep"
    }
}

/// The list for one workspace.
#[derive(Debug, Clone)]
pub struct Roster {
    pub workspace_id: String,
    pub workspace_path: String,
    pub workspace_handle: String,
    /// The caller's own address, when the caller is a session here.
    pub you: Option<String>,
    pub sidecars_live: usize,
    pub cap: usize,
    /// Asleep rows (counted even when `all` is off).
    pub asleep: usize,
    pub rows: Vec<RosterRow>,
}

impl Roster {
    pub fn to_json(&self) -> Value {
        json!({
            "ok": true,
            "workspace": { "handle": self.workspace_handle, "path": self.workspace_path, "id": self.workspace_id },
            "you": self.you,
            "sidecars": { "live": self.sidecars_live, "cap": self.cap },
            "asleep": self.asleep,
            "sessions": self.rows.iter().map(RosterRow::to_json).collect::<Vec<_>>(),
        })
    }

    pub fn live_count(&self) -> usize {
        self.rows.iter().filter(|r| r.is_live()).count()
    }
}

/// The activity store's display word → the roster's state word.
pub fn state_for_display(display: Option<&str>) -> &'static str {
    match display {
        Some("working") | Some("monitoring") => "working",
        Some("waiting") => "waiting",
        _ => "idle",
    }
}

/// The brief's first non-empty line, at most 80 characters.
pub fn brief_title(project_path: &str, rel: &str) -> Option<String> {
    let text = std::fs::read_to_string(std::path::Path::new(project_path).join(rel)).ok()?;
    let line = text
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())?;
    Some(line.chars().take(80).collect())
}

fn secs(ms_or_s: i64) -> i64 {
    if ms_or_s > 100_000_000_000 {
        ms_or_s / 1000
    } else {
        ms_or_s
    }
}

/// `(state, activity json, last evidence seconds)` for a live session.
fn live_activity(session_id: &str) -> (&'static str, Option<Value>, Option<i64>) {
    let Some(row) = crate::activity_store::row_json(session_id) else {
        return ("idle", None, None);
    };
    let display = row.get("display").and_then(Value::as_str);
    let since = row
        .get("turnStartedAt")
        .and_then(Value::as_i64)
        .or_else(|| row.pointer("/lead/since").and_then(Value::as_i64))
        .map(secs);
    let evidence = row.get("evidenceAt").and_then(Value::as_i64).map(secs);
    let subagents = row.pointer("/counts/subagents").and_then(Value::as_u64).unwrap_or(0);
    (
        state_for_display(display),
        Some(json!({ "display": display, "since": since, "subagents": subagents })),
        evidence,
    )
}

fn provider_of(program: Option<&str>) -> Option<String> {
    program
        .and_then(k2_core::workspace::provider_resume::provider_resume_for_command)
        .map(|p| p.provider.to_string())
}

fn program_name(program: Option<&str>) -> Option<String> {
    program.map(|p| {
        std::path::Path::new(p)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string())
    })
}

/// Does a live session belong to this workspace?
fn session_in_workspace(
    key: &str,
    session: &DaemonPtySession,
    ws_id: &str,
    ws_path: &str,
    project_paths: &[String],
    tab_keys: &HashSet<String>,
) -> bool {
    if key == ws_id || key.starts_with(&format!("{ws_id}:hb:")) || tab_keys.contains(key) {
        return true;
    }
    let cwd = session
        .cwd
        .as_ref()
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_default();
    !cwd.is_empty() && crate::session_events::cwd_matches_workspace(&cwd, ws_path, project_paths)
}

/// Build the list for `project_id`.
pub fn workspace_roster(project_id: &str, opts: &RosterOpts) -> Result<Roster, String> {
    let (ws_path, ws_handle, tab_rows, main_row, worktrees) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let project = k2_core::db::schema::Project::get(&conn, project_id)
            .map_err(|e| format!("workspace {project_id}: {e}"))?;
        let handle = k2_core::workspace_session_handles::workspace_address_name(&conn, project_id)
            .unwrap_or_else(|_| project.id.clone());
        let tab_rows = k2_core::sidecar::list_rows(&conn, project_id)?;
        let main_row = k2_core::db::schema::WorkspaceSession::get(&conn, project_id)
            .map_err(|e| format!("workspace_sessions: {e}"))?;
        // Workbench W1's worktree columns (0251), per tab row.
        let worktrees: HashMap<String, k2_core::worktree_task::store::WorktreeMeta> = tab_rows
            .iter()
            .filter_map(|r| {
                k2_core::worktree_task::store::get(&conn, project_id, &r.pane_group_id)
                    .map(|m| (r.pane_group_id.clone(), m))
            })
            .collect();
        (project.path, handle, tab_rows, main_row, worktrees)
    };
    let project_paths = k2_core::projects_ops::projects_list()
        .map_err(|e| format!("projects_list: {e}"))?
        .into_iter()
        .map(|p| p.path)
        .collect::<Vec<_>>();
    let tab_keys: HashSet<String> = tab_rows.iter().map(|r| r.agent_name.clone()).collect();
    let live: HashMap<String, Arc<DaemonPtySession>> = v2_session_map::snapshot()
        .into_iter()
        .filter(|(key, s)| {
            s.is_child_alive()
                && session_in_workspace(key, s, project_id, &ws_path, &project_paths, &tab_keys)
        })
        .collect();

    let mut rows: Vec<RosterRow> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let main_conversation = main_row
        .as_ref()
        .and_then(|r| r.session_id.clone())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // Main (the pinned chat).
    let main_live = live.get(project_id).cloned();
    if main_live.is_some() || main_row.is_some() {
        let (state, activity, evidence) = match &main_live {
            Some(s) => live_activity(&s.session_id.to_string()),
            None => ("asleep", None, None),
        };
        let conversation_id = match &main_live {
            Some(s) => crate::v2_spawn::conversation_id_from_args(s.program.as_deref(), &s.args)
                .or_else(|| main_conversation.clone()),
            None => main_conversation.clone(),
        };
        let title = conversation_id.as_deref().and_then(|cid| {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace_session_handles::custom_name_for_session_id(&conn, cid)
                .ok()
                .flatten()
        });
        let program = main_live.as_ref().and_then(|s| s.program.clone());
        let cli = provider_of(program.as_deref())
            .or_else(|| main_row.as_ref().map(|r| r.harness.clone()).filter(|h| !h.is_empty()));
        rows.push(RosterRow {
            address: Some(ws_handle.clone()),
            handle: None,
            role: "main",
            cli,
            preset: program_name(program.as_deref()),
            state,
            activity,
            last_active_at: evidence.or_else(|| main_row.as_ref().and_then(|r| r.last_activity_at).map(secs)),
            title,
            brief: None,
            conversation_id,
            session_id: main_live.as_ref().map(|s| s.session_id.to_string()),
            agent_name: Some(project_id.to_string()),
            pane_group_id: None,
            transport: "terminal",
            worktree: None,
            branch: None,
            opened_by: None,
            attached: main_live.as_ref().is_some_and(|s| s.subscriber_count() > 0),
            you: opts.caller_map_key.as_deref() == Some(project_id),
        });
        seen.insert(project_id.to_string());
    }

    // Tab rows: sidecars and chat tabs, live or asleep.
    for r in &tab_rows {
        let session = live.get(&r.agent_name).cloned();
        seen.insert(r.agent_name.clone());
        let (state, activity, evidence) = match &session {
            Some(s) => live_activity(&s.session_id.to_string()),
            None => ("asleep", None, None),
        };
        let custom = r.name.clone().filter(|n| !n.trim().is_empty());
        let label = session
            .as_ref()
            .map(|s| s.label().trim().to_string())
            .filter(|l| !l.is_empty());
        let brief_line = r.brief_path.as_deref().and_then(|rel| brief_title(&ws_path, rel));
        let title = brief_line.map(|b| format!("brief: {b}")).or(custom).or(label);
        // A worktree sidecar (W1, `k2 sidecar new --worktree`) keeps its
        // folder and branch after it is retired, so the list can still say
        // what it was.
        let wt = worktrees.get(&r.pane_group_id);
        let role = if wt.is_some() {
            "worktree"
        } else if r.created_by.is_some() {
            "sidecar"
        } else {
            "chat"
        };
        rows.push(RosterRow {
            address: r
                .handle
                .as_deref()
                .map(|h| k2_core::workspace_session_handles::format_address(&ws_handle, Some(h))),
            handle: r.handle.clone(),
            role,
            cli: Some(r.provider.to_string()),
            preset: program_name(Some(r.command.as_str())),
            state,
            activity,
            last_active_at: evidence.or(Some(secs(r.last_seen_at))),
            title,
            brief: r.brief_path.clone(),
            conversation_id: r.conversation_id.clone(),
            session_id: session.as_ref().map(|s| s.session_id.to_string()),
            agent_name: Some(r.agent_name.clone()),
            pane_group_id: Some(r.pane_group_id.clone()),
            transport: "terminal",
            worktree: wt.map(|m| m.dir.clone()).filter(|d| !d.is_empty()),
            branch: wt.map(|m| m.branch.clone()).filter(|b| !b.is_empty()),
            opened_by: r.created_by.as_deref().map(crate::sidecar_routes::created_by_display),
            attached: session.as_ref().is_some_and(|s| s.subscriber_count() > 0),
            you: opts.caller_map_key.as_deref() == Some(r.agent_name.as_str()),
        });
    }

    // Live harness sessions with no tab row yet: heartbeats, api, a tab
    // that has not written its row.
    let mut extra: Vec<(&String, &Arc<DaemonPtySession>)> =
        live.iter().filter(|(k, _)| !seen.contains(*k)).collect();
    extra.sort_by(|a, b| a.0.cmp(b.0));
    for (key, s) in extra {
        if !k2_core::workspace_session_handles::is_harness_command(s.program.as_deref()) {
            continue;
        }
        let is_api = k2_core::workspace_session_handles::is_api_agent_name(key);
        if is_api && !opts.include_api {
            continue;
        }
        let conversation_id = crate::v2_spawn::conversation_id_from_args(s.program.as_deref(), &s.args);
        if conversation_id.is_some() && conversation_id == main_conversation && !is_api {
            // A runner on the main conversation outside the main slot is
            // the duplicate the one-runner rule refuses; it shows as main's.
            continue;
        }
        let (role, handle, address) = if let Some(hb) = key.strip_prefix(&format!("{project_id}:hb:")) {
            ("heartbeat", Some(format!("hb:{hb}")), None)
        } else if is_api {
            ("api", None, None)
        } else {
            let pane = key.strip_prefix("tab-").unwrap_or(key);
            let ck = k2_core::workspace_session_handles::conversation_key_for(conversation_id.as_deref(), pane);
            let handle = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::sidecar::handle_read_only(&conn, project_id, &ck)
            };
            let address = handle
                .as_deref()
                .map(|h| k2_core::workspace_session_handles::format_address(&ws_handle, Some(h)));
            ("chat", handle, address)
        };
        let (state, activity, evidence) = live_activity(&s.session_id.to_string());
        let label = s.label().trim().to_string();
        rows.push(RosterRow {
            address,
            handle,
            role,
            cli: provider_of(s.program.as_deref()),
            preset: program_name(s.program.as_deref()),
            state,
            activity,
            last_active_at: evidence,
            title: (!label.is_empty()).then_some(label),
            brief: None,
            conversation_id,
            session_id: Some(s.session_id.to_string()),
            agent_name: Some(key.clone()),
            pane_group_id: key.strip_prefix("tab-").map(str::to_string),
            transport: "terminal",
            worktree: None,
            branch: None,
            opened_by: None,
            attached: s.subscriber_count() > 0,
            you: opts.caller_map_key.as_deref() == Some(key.as_str()),
        });
    }

    let asleep = rows.iter().filter(|r| !r.is_live()).count();
    let sidecars_live = rows
        .iter()
        .filter(|r| r.is_live() && matches!(r.role, "sidecar" | "worktree" | "chat"))
        .count();
    let you = rows
        .iter()
        .find(|r| r.you)
        .and_then(|r| r.address.clone().or_else(|| r.handle.clone()));
    if !opts.all {
        rows.retain(RosterRow::is_live);
    }
    rows.sort_by(|a, b| {
        let rank = |r: &RosterRow| (r.role != "main", !r.is_live());
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.last_active_at.unwrap_or(0).cmp(&a.last_active_at.unwrap_or(0)))
            .then_with(|| a.address.cmp(&b.address))
    });
    Ok(Roster {
        workspace_id: project_id.to_string(),
        workspace_path: ws_path,
        workspace_handle: ws_handle,
        you,
        sidecars_live,
        cap: k2_core::sidecar::cap(),
        asleep,
        rows,
    })
}

/// The map key of a live session by its PTY id.
pub fn map_key_for_session(session_id: Option<&str>) -> Option<String> {
    let sid = k2_core::session::SessionId::parse(session_id?.trim())?;
    v2_session_map::agent_name_for_session_id(&sid)
}

/// The workspace id of a project path.
fn project_id_for_path(path: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace::agent_identity::resolve_project_id(&conn, path)
}

/// `thisWorkspace` for `k2 connections` (WS-C1): the live rows, the
/// caller's own address and the asleep count, or `{error}`.
pub fn this_workspace(project_path: &str, caller_session_id: Option<&str>) -> Value {
    let Some(pid) = project_id_for_path(project_path) else {
        return json!({ "error": format!("{project_path} is not a registered workspace") });
    };
    let opts = RosterOpts { all: false, include_api: false, caller_map_key: map_key_for_session(caller_session_id) };
    match workspace_roster(&pid, &opts) {
        Ok(r) => json!({
            "handle": r.workspace_handle,
            "you": r.you,
            "sessions": r.rows.iter().map(RosterRow::to_json).collect::<Vec<_>>(),
            "asleep": r.asleep,
            "sidecars": { "live": r.sidecars_live, "cap": r.cap },
        }),
        Err(e) => json!({ "error": e }),
    }
}

/// Add `thisWorkspace` to a connections-list JSON body (a non-object body
/// is returned as is).
pub fn attach_this_workspace(body: &str, project_path: &str, caller_session_id: Option<&str>) -> String {
    let Ok(mut v) = serde_json::from_str::<Value>(body) else {
        return body.to_string();
    };
    let Some(obj) = v.as_object_mut() else {
        return body.to_string();
    };
    obj.insert("thisWorkspace".to_string(), this_workspace(project_path, caller_session_id));
    v.to_string()
}

/// `whoami`'s `siblings` (WS-D1): live sessions here other than the
/// caller, and asleep ones.
pub fn siblings(project_id: &str, caller_session_id: Option<&str>) -> Value {
    let opts = RosterOpts { all: false, include_api: false, caller_map_key: map_key_for_session(caller_session_id) };
    match workspace_roster(project_id, &opts) {
        Ok(r) => json!({
            "live": r.rows.iter().filter(|row| row.is_live() && !row.you).count(),
            "asleep": r.asleep,
        }),
        Err(e) => json!({ "error": e }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_words_fold_to_four_states() {
        assert_eq!(state_for_display(Some("working")), "working");
        assert_eq!(state_for_display(Some("monitoring")), "working");
        assert_eq!(state_for_display(Some("waiting")), "waiting");
        assert_eq!(state_for_display(Some("idle")), "idle");
        assert_eq!(state_for_display(Some("unverifiable")), "idle");
        assert_eq!(state_for_display(None), "idle");
    }

    #[test]
    fn seconds_from_either_unit() {
        assert_eq!(secs(1_761_741_200_000), 1_761_741_200);
        assert_eq!(secs(1_761_741_200), 1_761_741_200);
    }

    #[test]
    fn brief_title_is_the_first_line() {
        let dir = std::env::temp_dir().join(format!("k2-roster-brief-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join(".k2/sidecars/r")).expect("mkdir");
        std::fs::write(dir.join(".k2/sidecars/r/BRIEF.md"), "\n\n# Review the W1 diff, report to k2\nmore\n").expect("write");
        assert_eq!(
            brief_title(&dir.to_string_lossy(), ".k2/sidecars/r/BRIEF.md").as_deref(),
            Some("Review the W1 diff, report to k2")
        );
        assert_eq!(brief_title(&dir.to_string_lossy(), ".k2/sidecars/none/BRIEF.md"), None);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
