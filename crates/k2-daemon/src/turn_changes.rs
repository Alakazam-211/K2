//! The changed-files card per turn, daemon side (workbench-v1 B7, vs-live
//! E8, research-t3code-ui-as-garden-v1 §4.3 C).
//!
//! Follows the activity store's rows ([`crate::activity_store::subscribe`]):
//! - **Turn start.** A row whose `turnStartedAt` moves past the last turn
//!   this module saw is a new agent turn (its prompt, or a Thread send it
//!   took up). If the session is a **worktree agent** — a sidecar (never
//!   the workspace's canonical) whose cwd is a git checkout under
//!   `.worktrees/` — K2 takes a [`k2_core::turn_changes::mark`] of it.
//! - **Turn end.** The row's own turn end ([`RowEvent::turn_ended`])
//!   compares the worktree with that mark. When files or commits changed,
//!   K2 posts one `changes` card on the sidecar's Thread (`<ws>/<handle>`),
//!   from `k2` ([`crate::overlay_routes::post_changes_card`]): no inject,
//!   no wake, no push. Nothing changed: no card.
//!
//! Only git is read (start and end pictures), never the transcript. The
//! state is in memory: after a restart no PTY is live, so no turn is open.

use std::collections::HashMap;
use std::path::PathBuf;

use k2_core::log_debug;
use k2_core::turn_changes::TurnMark;

/// A session K2 makes cards for: its worktree and its Thread address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeAgent {
    pub cwd: PathBuf,
    pub address: String,
}

/// What the tracker needs from the daemon (a fake in tests).
pub trait Env {
    /// The session's worktree agent, or None (canonical, not a sidecar,
    /// not a K2 worktree, gone).
    fn agent(&self, session_id: &str, agent_name: &str) -> Option<WorktreeAgent>;
    fn mark(&self, cwd: &std::path::Path) -> Option<TurnMark> {
        k2_core::turn_changes::mark(cwd)
    }
    fn changes(&self, cwd: &std::path::Path, start: &TurnMark) -> Option<k2_core::overlay::ThreadChanges> {
        k2_core::turn_changes::changes_since(cwd, start)
    }
}

/// One card to post.
#[derive(Debug, Clone, PartialEq)]
pub struct Post {
    pub address: String,
    pub changes: k2_core::overlay::ThreadChanges,
}

struct Open {
    started_at: i64,
    agent: WorktreeAgent,
    mark: TurnMark,
}

/// Per-session turns: the last turn start seen and the open mark.
#[derive(Default)]
pub struct Tracker {
    seen: HashMap<String, i64>,
    open: HashMap<String, Open>,
}

impl Tracker {
    /// One row change. `turn_started_at` is the row's `turnStartedAt`;
    /// `ended` is true on the row's own turn end; `removed` when the row
    /// went away (an open turn is dropped, no card).
    pub fn on_row(
        &mut self,
        env: &dyn Env,
        session_id: &str,
        agent_name: &str,
        turn_started_at: Option<i64>,
        ended: bool,
        removed: bool,
    ) -> Option<Post> {
        if removed {
            self.seen.remove(session_id);
            self.open.remove(session_id);
            return None;
        }
        // A turn end first: the end event still carries its turn's start.
        let post = if ended { self.end(env, session_id) } else { None };
        if let Some(t) = turn_started_at {
            let last = self.seen.get(session_id).copied();
            if last.is_none_or(|l| t > l) {
                self.seen.insert(session_id.to_string(), t);
                if !ended {
                    self.start(env, session_id, agent_name, t);
                }
            }
        }
        post
    }

    fn start(&mut self, env: &dyn Env, session_id: &str, agent_name: &str, at: i64) {
        // A new turn replaces one that never saw its end.
        self.open.remove(session_id);
        let Some(agent) = env.agent(session_id, agent_name) else { return };
        let Some(mark) = env.mark(&agent.cwd) else { return };
        self.open.insert(session_id.to_string(), Open { started_at: at, agent, mark });
    }

    fn end(&mut self, env: &dyn Env, session_id: &str) -> Option<Post> {
        let open = self.open.remove(session_id)?;
        let changes = env.changes(&open.agent.cwd, &open.mark)?;
        log_debug!(
            "[turn-changes] {} turn from {}: {} files, {} commits",
            open.agent.address,
            open.started_at,
            changes.total,
            changes.commits
        );
        Some(Post { address: open.agent.address, changes })
    }

    /// Open marks (tests).
    pub fn open_turns(&self) -> usize {
        self.open.len()
    }
}

/// The daemon's [`Env`]: the live PTY's cwd, the activity row's workspace,
/// and the workspace's sidecar list.
struct Live;

impl Env for Live {
    fn agent(&self, session_id: &str, agent_name: &str) -> Option<WorktreeAgent> {
        let sid = k2_core::session::SessionId::parse(session_id)?;
        let session = crate::v2_session_map::lookup_by_session_id(&sid)?;
        let cwd = session.cwd.clone()?;
        let project_id = crate::activity_store::with_row(session_id, |r| r.facts.project_id.clone()).flatten()?;
        worktree_agent(&project_id, agent_name, cwd)
    }
}

/// A sidecar of `project_id` whose tab key is `agent_name`, working in a
/// K2 worktree at `cwd`: its Thread address. Never the canonical.
pub fn worktree_agent(project_id: &str, agent_name: &str, cwd: PathBuf) -> Option<WorktreeAgent> {
    if agent_name == project_id || !k2_core::turn_changes::is_k2_worktree(&cwd) {
        return None;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = k2_core::sidecar::list_rows(&conn, project_id).ok()?.into_iter().find(|r| r.agent_name == agent_name)?;
    let handle = row.handle?;
    let ws = k2_core::workspace_session_handles::workspace_address_name(&conn, project_id).ok()?;
    Some(WorktreeAgent { cwd, address: k2_core::workspace_session_handles::format_address(&ws, Some(&handle)) })
}

fn tracker() -> &'static parking_lot::Mutex<Tracker> {
    static T: std::sync::OnceLock<parking_lot::Mutex<Tracker>> = std::sync::OnceLock::new();
    T.get_or_init(|| parking_lot::Mutex::new(Tracker::default()))
}

/// Start following the activity rows (once). Git work runs on this
/// module's own thread, never on the store's.
pub fn spawn() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        use tokio::sync::broadcast::error::RecvError;
        let mut rows = crate::activity_store::subscribe();
        let t = std::thread::Builder::new().name("turn-changes".into()).spawn(move || loop {
            match rows.blocking_recv() {
                Ok(ev) => {
                    let started = ev.row.as_ref().and_then(|r| r.get("turnStartedAt")).and_then(serde_json::Value::as_i64);
                    let post = tracker().lock().on_row(
                        &Live,
                        &ev.session_id,
                        &ev.agent_name,
                        started,
                        ev.turn_ended.is_some(),
                        ev.row.is_none(),
                    );
                    if let Some(p) = post {
                        if let Err(e) = crate::overlay_routes::post_changes_card(&p.address, p.changes) {
                            log_debug!("[turn-changes] card for {} not posted: {e}", p.address);
                        }
                    }
                }
                // A missed start leaves that turn without a card; a
                // missed end drops at the next start.
                Err(RecvError::Lagged(n)) => log_debug!("[turn-changes] row bus lagged {n}"),
                Err(RecvError::Closed) => return,
            }
        });
        if let Err(e) = t {
            log_debug!("[turn-changes] could not start: {e}");
        }
    });
}

#[cfg(test)]
#[path = "turn_changes_tests.rs"]
mod tests;
