//! Workbench W1 (0.46.7): worktree sidecars, daemon half
//! (`.k2/prds/research-t3code-ui-as-garden-v1-vs-live.md` slices B1–B3;
//! the route contract is `.k2/prds/workbench-w1-contract.md`).
//!
//! - `POST /cli/sidecar/new {worktree: true, base?}` (in
//!   [`crate::sidecar_routes`]) makes the worktree through [`NewWorktree`]:
//!   `.worktrees/k2-<name>` on branch `k2/<name>`, a `workspaces` row of
//!   type `worktree` (so the Changes tab and `git_changed` see it), and the
//!   sidecar runs there. Anything failing before the sidecar runs rolls it
//!   back.
//! - `GET /cli/sidecar/review` — what Land would do: commits not on the
//!   base, changed files, uncommitted files, and why Land would refuse.
//! - `POST /cli/sidecar/interrupt` (B2) — stop the current turn (the
//!   harness's Esc / Ctrl+C), never the process.
//! - `POST /cli/sidecar/land` (B3) — cherry-pick onto the base in the main
//!   checkout, bound to the tip the person reviewed; then Retire by default.
//! - `POST /cli/sidecar/retire` (B3) — stop, worktree to the Trash (only
//!   through B0's guarded `git::remove_worktree`), branch deleted only when
//!   landed or discarded, `workspaces` row dropped, columns stamped. Refuses
//!   commits not on the base or uncommitted files unless `discard`.
//!
//! **Who.** People (the owner token, a Connect login at the route's floor,
//! a K2 shell tab) land and retire. An agent never does: Land and Retire
//! answer it `403 land_needs_person` / `retire_needs_person`, which the
//! permission-card walls teach as `git.land` / `sidecar.retire` (the
//! person's Allow runs [`land_entry`] / [`retire_entry`] as that person).
//! Review is a read (agents: their own workspace). Interrupt follows
//! `sidecar stop`: an agent needs "Allow hiring and managing agents", must
//! be the main session, and may interrupt only sidecars its workspace
//! opened.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use k2_core::log_debug;
use k2_core::sidecar::{self as core, SidecarError};
use k2_core::worktree_task::{self as wt, store, WtError};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::{
    agent_gate, audit, err, json_body, live_session, require_credential, status_line, str_field, tab_row_for_key,
    target_workspace, workspace_lock, Caller, Workspace,
};

/// An agent's Land refusal (the `git.land` card's wall).
pub(crate) const LAND_NEEDS_PERSON: &str = "land_needs_person";
/// An agent's Retire refusal (the `sidecar.retire` card's wall).
pub(crate) const RETIRE_NEEDS_PERSON: &str = "retire_needs_person";
pub(crate) const LAND_ROUTE: &str = "/cli/sidecar/land";
pub(crate) const RETIRE_ROUTE: &str = "/cli/sidecar/retire";

// ── responses ─────────────────────────────────────────────────────────

/// A refusal: status + the `{"error": {code, hint, …}}` body.
#[derive(Debug)]
pub(crate) struct Refused {
    status: u16,
    body: Value,
}

impl Refused {
    fn new(code: &str, status: u16, hint: impl Into<String>) -> Self {
        Self { status, body: json!({ "error": { "code": code, "hint": hint.into() } }) }
    }
    fn code(&self) -> String {
        self.body.pointer("/error/code").and_then(Value::as_str).unwrap_or("error").to_string()
    }
    fn set(mut self, key: &str, v: Value) -> Self {
        self.body["error"][key] = v;
        self
    }
}

impl From<SidecarError> for Refused {
    fn from(e: SidecarError) -> Self {
        Refused::new(e.code, e.status, e.hint)
    }
}

impl From<WtError> for Refused {
    fn from(e: WtError) -> Self {
        let mut r = Refused::new(e.code, e.status, e.hint);
        if !e.files.is_empty() {
            r = r.set("files", json!(e.files));
        }
        if !e.commits.is_empty() {
            r = r.set("commits", json!(e.commits));
        }
        r
    }
}

fn respond(r: Result<Value, Refused>) -> CliResponse {
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => CliResponse { status: status_line(e.status), content_type: "application/json", body: e.body.to_string() },
    }
}

/// Route a Workbench sidecar request (called from `sidecar_routes::handle`).
pub(crate) fn handle(
    path: &str,
    caller: &Caller,
    body: &[u8],
    params: &HashMap<String, String>,
    ingress: &str,
) -> CliResponse {
    let (r, event) = match path {
        "/cli/sidecar/review" => return respond(handle_review(caller, params)),
        "/cli/sidecar/interrupt" => (handle_interrupt(caller, body), "sidecar-interrupt"),
        LAND_ROUTE => (handle_land(caller, body), "sidecar-land"),
        RETIRE_ROUTE => (handle_retire(caller, body), "sidecar-retire"),
        _ => return CliResponse::not_found(),
    };
    let outcome = match &r {
        Ok(v) => format!("ok {}", v["address"].as_str().unwrap_or("-")),
        Err(e) => format!("refused:{}", e.code()),
    };
    audit(event, caller, &outcome, ingress);
    respond(r)
}

// ── new: the worktree (B1) ───────────────────────────────────────────

/// A worktree made for a new sidecar. Dropped while armed (anything failed
/// before the sidecar ran), it is rolled back: the folder goes to the Trash
/// through the guarded `git::remove_worktree`, the branch (K2-made, no
/// commits yet) is deleted, the `workspaces` row and the name binding go.
pub(crate) struct NewWorktree {
    root: String,
    project_id: String,
    pub(crate) created: wt::Created,
    pub(crate) ws_row_id: String,
    /// The conversation key the sidecar's name was bound to, once bound.
    pub(crate) name_key: Option<String>,
    armed: bool,
}

/// The worktree of a sidecar that runs.
pub(crate) struct KeptWorktree {
    pub(crate) created: wt::Created,
    pub(crate) ws_row_id: String,
}

impl NewWorktree {
    /// `git worktree add` + the `workspaces` row + the Changes watch.
    pub(crate) fn create(ws: &Workspace, slug: &str, base: Option<&str>) -> Result<Self, SidecarError> {
        let created = wt::create(Path::new(&ws.path), slug, base).map_err(|e| err(e.code, e.status, e.hint))?;
        let ws_row_id = uuid::Uuid::new_v4().to_string();
        let mut me = Self {
            root: ws.path.clone(),
            project_id: ws.id.clone(),
            created,
            ws_row_id: ws_row_id.clone(),
            name_key: None,
            armed: true,
        };
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let order = k2_core::db::schema::Workspace::list(&conn, &ws.id)
                .unwrap_or_default()
                .iter()
                .map(|w| w.tab_order)
                .max()
                .unwrap_or(-1)
                + 1;
            if let Err(e) = k2_core::db::schema::Workspace::create(
                &conn,
                &ws_row_id,
                &ws.id,
                None,
                "worktree",
                Some(&me.created.branch),
                &me.created.branch,
                order,
                Some(&me.created.path),
            ) {
                // Not registered: nothing to drop but the worktree itself.
                me.ws_row_id.clear();
                return Err(err("spawn_failed", 500, format!("register the worktree: {e}")));
            }
        }
        crate::fs_live::resync_watches();
        Ok(me)
    }

    /// The sidecar runs: keep everything.
    pub(crate) fn keep(mut self) -> KeptWorktree {
        self.armed = false;
        KeptWorktree { created: self.created.clone(), ws_row_id: self.ws_row_id.clone() }
    }
}

impl Drop for NewWorktree {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let root = Path::new(&self.root);
        if let Err(e) = k2_core::git::remove_worktree(&self.root, &self.created.path, false) {
            log_debug!("[sidecar-worktree] rollback: worktree {} stays: {e}", self.created.path);
        } else if let Err(e) = wt::delete_branch(root, &self.created.branch) {
            log_debug!("[sidecar-worktree] rollback: branch {} stays: {e}", self.created.branch);
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        if !self.ws_row_id.is_empty() {
            let _ = k2_core::db::schema::Workspace::delete(&conn, &self.ws_row_id);
        }
        if let Some(key) = &self.name_key {
            let _ = conn.execute("DELETE FROM chat_session_names WHERE session_id = ?1", rusqlite::params![key]);
        }
        drop(conn);
        crate::fs_live::resync_watches();
        log_debug!("[sidecar-worktree] rolled back {} in {}", self.created.branch, self.project_id);
    }
}

// ── shared resolution ────────────────────────────────────────────────

/// One sidecar of a workspace, by name.
struct Sidecar {
    row: k2_core::db::schema::WorkspaceTabSession,
    handle: String,
    address: String,
    /// `None` for a plain sidecar.
    meta: Option<store::WorktreeMeta>,
}

impl Sidecar {
    fn worktree(&self) -> Result<&store::WorktreeMeta, Refused> {
        self.meta.as_ref().ok_or_else(|| {
            Refused::new(
                "not_a_worktree_sidecar",
                409,
                format!("{} runs in the workspace itself, not a worktree; only worktree sidecars land and retire", self.address),
            )
        })
    }
}

fn resolve_sidecar(ws: &Workspace, raw: Option<&str>) -> Result<Sidecar, Refused> {
    let raw = raw.unwrap_or("").trim().to_string();
    if raw.is_empty() {
        return Err(Refused::new("no_such_sidecar", 404, "name the sidecar"));
    }
    // `<ws>/<name>` or the bare name.
    let bare = match raw.split_once('/') {
        Some((w, n)) if w == ws.handle => n.to_string(),
        Some(_) => {
            return Err(Refused::new("no_such_sidecar", 404, format!("{raw} is not a sidecar of {}", ws.handle)));
        }
        None => raw.clone(),
    };
    let token = k2_core::workspace_session_handles::slugify_custom_name(&bare).unwrap_or_else(|_| bare.to_lowercase());
    let missing = || Refused::new("no_such_sidecar", 404, format!("no sidecar '{raw}' in {}", ws.handle));
    let key = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace_session_handles::resolve_handle(&conn, &ws.id, &token)
    }
    .map_err(|_| missing())?;
    let row = tab_row_for_key(&ws.id, &key)
        .filter(|r| r.command.as_deref().and_then(core::supported_provider).is_some())
        .ok_or_else(missing)?;
    let (handle, meta) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (
            core::handle_read_only(&conn, &ws.id, &key).unwrap_or(token.clone()),
            store::get(&conn, &ws.id, &row.pane_group_id),
        )
    };
    let address = k2_core::workspace_session_handles::format_address(&ws.handle, Some(&handle));
    Ok(Sidecar { row, handle, address, meta })
}

/// The branch Land puts the work on: the recorded base, else the
/// workspace's default branch.
fn onto_of(ws: &Workspace, meta: &store::WorktreeMeta) -> Result<String, Refused> {
    meta.base_ref.clone().or_else(|| wt::default_branch(Path::new(&ws.path))).ok_or_else(|| {
        Refused::new("stale", 409, "this workspace has no branch to land on; check out its main branch first")
    })
}

fn worktree_path(ws: &Workspace, meta: &store::WorktreeMeta) -> PathBuf {
    Path::new(&ws.path).join(&meta.dir)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Is a live session mid-turn (or waiting on a person)? From the activity
/// store; an idle or unknown row is not busy.
fn activity_of(session: Option<&k2_core::terminal::DaemonPtySession>) -> Option<&'static str> {
    let s = session?;
    crate::activity_store::with_row(&s.session_id.to_string(), |r| r.display.as_str())
}

fn busy(activity: Option<&str>) -> bool {
    matches!(activity, Some("working" | "monitoring" | "waiting"))
}

/// A K2 line on the sidecar's own Thread (no wake, no push), and the same
/// line into its session when it is live, so the agent hears it.
fn tell(ws: &Workspace, s: &Sidecar, text: &str) {
    if let Err(e) = crate::overlay_routes::post_workspace_notice(&s.address, &ws.handle, text) {
        log_debug!("[sidecar-worktree] Thread note for {} failed: {e}", s.address);
    }
    if let Some(live) = crate::session_lookup::lookup_any(&s.row.agent_name) {
        // The inject waits for the session's input lock: never on the route.
        let line = text.to_string();
        std::thread::spawn(move || {
            let _ = crate::workspace_msg::inject_notice(&live, &line);
        });
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

// ── list extras (E9) ─────────────────────────────────────────────────

/// `worktree`, `taskState` and `activity` for one `sidecar list` row;
/// `None` for a plain sidecar.
pub(crate) fn list_extras(
    ws: &Workspace,
    pane_group_id: &str,
    session: Option<&k2_core::terminal::DaemonPtySession>,
) -> Option<Value> {
    let meta = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get(&conn, &ws.id, pane_group_id)
    }?;
    let activity = activity_of(session);
    let root = Path::new(&ws.path);
    let (unlanded, dirty, onto) = if meta.retired_at.is_some() {
        (0usize, false, meta.base_ref.clone())
    } else {
        let onto = meta.base_ref.clone().or_else(|| wt::default_branch(root));
        let unlanded = onto
            .as_deref()
            .and_then(|o| wt::review(root, None, o, &meta.branch).ok())
            .map(|r| r.commits.len())
            .unwrap_or(0);
        let path = worktree_path(ws, &meta);
        let dirty = path.is_dir() && wt::dirty_files(&path).map(|d| !d.is_empty()).unwrap_or(false);
        (unlanded, dirty, onto)
    };
    let state = wt::task_state(&meta, busy(activity), unlanded, dirty);
    Some(json!({
        "worktree": {
            "branch": meta.branch,
            "dir": meta.dir,
            "onto": onto,
            "ahead": unlanded,
            "dirty": dirty,
            "landedAt": meta.landed_at,
            "landedHead": meta.landed_head,
            "retiredAt": meta.retired_at,
            "retiredHow": meta.retired_how,
        },
        "taskState": state.as_str(),
        "activity": activity,
    }))
}

// ── review ───────────────────────────────────────────────────────────

fn handle_review(caller: &Caller, params: &HashMap<String, String>) -> Result<Value, Refused> {
    require_credential(caller)?;
    let ws = target_workspace(caller, params.get("workspace").map(String::as_str))?;
    let s = resolve_sidecar(&ws, params.get("name").map(String::as_str))?;
    let meta = s.worktree()?.clone();
    let session = live_session(&s.row.agent_name);
    let activity = activity_of(session.as_deref());
    let mut out = json!({
        "ok": true,
        "address": s.address,
        "workspace": ws.handle,
        "handle": s.handle,
        "branch": meta.branch,
        "dir": meta.dir,
        "landedAt": meta.landed_at,
        "landedHead": meta.landed_head,
        "retiredAt": meta.retired_at,
        "retiredHow": meta.retired_how,
        "activity": activity,
    });
    if meta.retired_at.is_some() {
        out["onto"] = json!(meta.base_ref);
        out["commits"] = json!([]);
        out["files"] = json!([]);
        out["dirty"] = json!([]);
        out["canLand"] = json!(false);
        out["blocked"] = json!("sidecar_retired");
        out["taskState"] = json!(wt::task_state(&meta, false, 0, false).as_str());
        return Ok(out);
    }
    let onto = onto_of(&ws, &meta)?;
    let path = worktree_path(&ws, &meta);
    let r = wt::review(Path::new(&ws.path), Some(&path), &onto, &meta.branch)?;
    let state = wt::task_state(&meta, busy(activity), r.commits.len(), !r.dirty.is_empty());
    out["onto"] = json!(r.onto);
    out["ontoHead"] = json!(r.onto_head);
    out["tip"] = json!(r.tip);
    out["commits"] = json!(r.commits);
    out["files"] = json!(r.files);
    out["dirty"] = json!(r.dirty);
    out["mainBranch"] = json!(r.main_branch);
    out["mainDirty"] = json!(r.main_dirty);
    out["canLand"] = json!(r.blocked.is_none());
    out["blocked"] = json!(r.blocked);
    out["taskState"] = json!(state.as_str());
    Ok(out)
}

// ── interrupt (B2) ───────────────────────────────────────────────────

fn handle_interrupt(caller: &Caller, body: &[u8]) -> Result<Value, Refused> {
    require_credential(caller)?;
    let v = json_body(body)?;
    let ws = target_workspace(caller, str_field(&v, "workspace"))?;
    agent_gate(caller, &ws)?;
    let s = resolve_sidecar(&ws, str_field(&v, "name"))?;
    if let Caller::Agent(_) = caller {
        let own = format!("agent:{}", ws.handle);
        let made_by = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            core::meta(&conn, &ws.id, &s.row.pane_group_id).and_then(|m| m.created_by)
        };
        if made_by.as_deref() != Some(own.as_str()) {
            return Err(Refused::new(
                "forbidden",
                403,
                "an agent may interrupt only sidecars an agent of this workspace opened; ask your human",
            ));
        }
    }
    let provider = s.row.command.as_deref().and_then(core::supported_provider).map(|p| p.provider).unwrap_or("claude");
    let bytes = core::interrupt_bytes(provider);
    let key = if bytes == b"\x03" { "ctrl-c" } else { "esc" };
    let Some(session) = live_session(&s.row.agent_name) else {
        return Ok(json!({ "ok": true, "address": s.address, "interrupted": false, "state": "asleep", "key": key }));
    };
    // DA26: the activity row sees the key as a person's interrupt.
    crate::activity_store::note_client_input(&session.session_id.to_string(), bytes);
    session.write(bytes);
    Ok(json!({ "ok": true, "address": s.address, "interrupted": true, "state": "live", "key": key }))
}

// ── land (B3) ────────────────────────────────────────────────────────

/// The `git.land` card's Allow: the route entry as the person who tapped.
pub(crate) fn land_entry(caller: &Caller, body: &Value) -> CliResponse {
    let r = handle_land(caller, body.to_string().as_bytes());
    let outcome = match &r {
        Ok(v) => format!("ok {}", v["address"].as_str().unwrap_or("-")),
        Err(e) => format!("refused:{}", e.code()),
    };
    audit("sidecar-land", caller, &outcome, "permission-card");
    respond(r)
}

/// The `sidecar.retire` card's Allow.
pub(crate) fn retire_entry(caller: &Caller, body: &Value) -> CliResponse {
    let r = handle_retire(caller, body.to_string().as_bytes());
    let outcome = match &r {
        Ok(v) => format!("ok {}", v["address"].as_str().unwrap_or("-")),
        Err(e) => format!("refused:{}", e.code()),
    };
    audit("sidecar-retire", caller, &outcome, "permission-card");
    respond(r)
}

/// The current tip of a worktree sidecar's branch, short (12 hex: a card
/// value never carries a 40-character token, and Land takes any prefix of
/// at least 7), or "".
fn tip_of(ws: &Workspace, meta: &store::WorktreeMeta) -> String {
    let onto = meta.base_ref.clone().or_else(|| wt::default_branch(Path::new(&ws.path))).unwrap_or_default();
    let tip = wt::review(Path::new(&ws.path), None, &onto, &meta.branch).ok().and_then(|r| r.tip).unwrap_or_default();
    tip[..tip.len().min(12)].to_string()
}

/// The tab row of a sidecar that runs outside the workspace root (a
/// worktree): `v2_session_map::register` only stamps rows for a cwd that
/// is a registered workspace root, so the route writes it, the same shape,
/// from the live session (cwd = the worktree: the `k2 msg` wake and
/// `sidecar new` resume read it back).
pub(crate) fn ensure_tab_row(ws: &Workspace, pane_group_id: &str, agent_name: &str, run_dir: &str) -> Result<(), SidecarError> {
    let session = crate::v2_session_map::lookup_by_agent_name(agent_name)
        .ok_or_else(|| err("spawn_failed", 500, format!("{agent_name} is not live after its spawn")))?;
    let program = session.program.clone();
    let claimed = k2_core::workspace::provider_resume::session_id_from_spawn_argv(program.as_deref().unwrap_or(""), &session.args);
    let row = k2_core::db::schema::WorkspaceTabSession {
        project_id: ws.id.clone(),
        pane_group_id: pane_group_id.to_string(),
        agent_name: agent_name.to_string(),
        session_id: claimed.clone(),
        command: program.clone(),
        args_json: serde_json::to_string(&session.args).ok(),
        cwd: Some(run_dir.to_string()),
        last_seen_at: 0,
        pinned_cols: None,
        pinned_rows: None,
        pinned_set_by: None,
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceTabSession::upsert(&conn, &row).map_err(|e| err("spawn_failed", 500, format!("tab row: {e}")))?;
    let _ = k2_core::workspace_session_handles::ensure_sidecar_handle(
        &conn,
        &ws.id,
        agent_name,
        program.as_deref(),
        claimed.as_deref(),
        pane_group_id,
    );
    Ok(())
}

const ASK_A_PERSON: &str = "only a person lands work on the workspace's main branch. Ask your human: they press Land in \
     Workbench or run `k2 sidecar land {address}` (it shows the commits first). With permission cards on, ask with the \
     command below; otherwise ask on your Thread (k2 thread ask).";

fn bool_field(v: &Value, key: &str, default: bool) -> Result<bool, Refused> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(Refused::new("bad_request", 400, format!("{key} must be true or false"))),
    }
}

fn handle_land(caller: &Caller, body: &[u8]) -> Result<Value, Refused> {
    require_credential(caller)?;
    let v = json_body(body)?;
    let ws = target_workspace(caller, str_field(&v, "workspace"))?;
    let s = resolve_sidecar(&ws, str_field(&v, "name"))?;
    let meta = s.worktree()?.clone();
    if let Caller::Agent(_) = caller {
        return Err(Refused::new(LAND_NEEDS_PERSON, 403, ASK_A_PERSON.replace("{address}", &s.address))
            .set("sidecar", json!(s.handle))
            .set("address", json!(s.address))
            .set("tip", json!(tip_of(&ws, &meta))));
    }
    if meta.retired_at.is_some() {
        return Err(Refused::new("sidecar_retired", 410, format!("{} is retired; nothing to land", s.address)));
    }
    let expect_tip = str_field(&v, "expectTip").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| {
        Refused::new(
            "bad_request",
            400,
            format!("pass expectTip: the branch tip you reviewed (k2 sidecar review {})", s.address),
        )
    })?;
    let expect_onto = str_field(&v, "expectOnto");
    let retire = bool_field(&v, "retire", true)?;
    let onto = onto_of(&ws, &meta)?;

    let lock = workspace_lock(&ws.id);
    let _guard = lock.lock();
    let root = Path::new(&ws.path);
    let landed = match wt::land(root, &onto, &meta.branch, expect_onto, expect_tip) {
        Ok(l) => l,
        Err(e) => {
            if e.code == wt::codes::CONFLICT {
                tell(
                    &ws,
                    &s,
                    &format!(
                        "[K2] Couldn't land {} on {onto}: conflicts in {}. Nothing changed on {onto}. Rebase {} onto {onto} in this worktree, commit, and ask again.",
                        meta.branch,
                        e.files.join(", "),
                        meta.branch
                    ),
                );
            }
            return Err(e.into());
        }
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::mark_landed(&conn, &ws.id, &s.row.pane_group_id, &landed.to, now())
            .map_err(|e| Refused::new("internal", 500, e))?;
    }
    let _ = crate::git_live::changed_now(&ws.path);
    let wt_path = worktree_path(&ws, &meta);
    let _ = crate::git_live::changed_now(&wt_path.to_string_lossy());
    let n = landed.commits.len();
    tell(
        &ws,
        &s,
        &format!(
            "[K2] Landed {n} commit{} from {} on {onto} ({}..{}) for {}.",
            if n == 1 { "" } else { "s" },
            meta.branch,
            short(&landed.from),
            short(&landed.to),
            caller.display()
        ),
    );
    let mut out = json!({
        "ok": true,
        "address": s.address,
        "onto": landed.onto,
        "from": landed.from,
        "to": landed.to,
        "landed": landed.commits,
        "count": n,
        "retired": Value::Null,
        "retireError": Value::Null,
    });
    if retire {
        // Re-read: the row now carries `landed_at`. The land stands
        // whatever the retire says.
        match resolve_sidecar(&ws, Some(&s.handle)).and_then(|s2| retire_locked(&ws, &s2, false, false)) {
            Ok(r) => out["retired"] = r,
            Err(e) => out["retireError"] = e.body["error"].clone(),
        }
    }
    Ok(out)
}

// ── retire (B3) ──────────────────────────────────────────────────────

fn handle_retire(caller: &Caller, body: &[u8]) -> Result<Value, Refused> {
    require_credential(caller)?;
    let v = json_body(body)?;
    let ws = target_workspace(caller, str_field(&v, "workspace"))?;
    let s = resolve_sidecar(&ws, str_field(&v, "name"))?;
    s.worktree()?;
    let discard = bool_field(&v, "discard", false)?;
    let keep_branch = bool_field(&v, "keepBranch", false)?;
    if let Caller::Agent(_) = caller {
        return Err(Refused::new(
            RETIRE_NEEDS_PERSON,
            403,
            format!(
                "only a person retires a worktree sidecar. Ask your human: they press Retire (or Discard) in Workbench or run `k2 sidecar retire {}{}`. With permission cards on, ask with the command below; otherwise ask on your Thread (k2 thread ask).",
                s.address,
                if discard { " --discard" } else { "" }
            ),
        )
        .set("sidecar", json!(s.handle))
        .set("address", json!(s.address))
        .set("discard", json!(if discard { "yes" } else { "no" })));
    }
    let lock = workspace_lock(&ws.id);
    let _guard = lock.lock();
    retire_locked(&ws, &s, discard, keep_branch)
}

/// Retire with the workspace lock held. Order: refuse unlanded work
/// (unless discard) → stop the session → worktree to the Trash (guarded)
/// → branch (only when landed or discarded) → `workspaces` row → stamp.
fn retire_locked(ws: &Workspace, s: &Sidecar, discard: bool, keep_branch: bool) -> Result<Value, Refused> {
    let meta = s.worktree()?.clone();
    if meta.retired_at.is_some() {
        return Err(Refused::new(
            "already_retired",
            409,
            format!("{} was retired already ({})", s.address, meta.retired_how.as_deref().unwrap_or("retired")),
        ));
    }
    let root = Path::new(&ws.path);
    let path = worktree_path(ws, &meta);
    let onto = meta.base_ref.clone().or_else(|| wt::default_branch(root));
    let unlanded: Vec<wt::Commit> = match &onto {
        Some(o) => wt::review(root, None, o, &meta.branch)?.commits,
        None => Vec::new(),
    };
    let dirty = if path.is_dir() { wt::dirty_files(&path)? } else { Vec::new() };
    if (!unlanded.is_empty() || !dirty.is_empty()) && !discard {
        let mut why = Vec::new();
        if !unlanded.is_empty() {
            why.push(format!("{} commit(s) not on {}", unlanded.len(), onto.as_deref().unwrap_or("the base")));
        }
        if !dirty.is_empty() {
            why.push(format!("uncommitted files ({})", dirty.join(", ")));
        }
        return Err(Refused::new(
            wt::codes::UNLANDED_WORK,
            409,
            format!(
                "{} has {}; land it first (k2 sidecar land {}), or pass --discard to throw it away (the worktree goes to the Trash, the branch is deleted)",
                s.address,
                why.join(" and "),
                s.address
            ),
        )
        .set("commits", json!(unlanded))
        .set("files", json!(dirty)));
    }

    // Stop the session (the person decided; a viewer doesn't hold it).
    let stopped = match live_session(&s.row.agent_name) {
        Some(session) => {
            crate::v2_spawn::record_closed_tab(&s.row.agent_name);
            crate::v2_session_map::unregister(&s.row.agent_name);
            session.kill();
            true
        }
        None => false,
    };
    match core::remove_layout_tab(&ws.id, &s.row.pane_group_id) {
        Ok(core::LayoutWrite::Wrote { workspace_id, revision }) => {
            let _ = crate::session_events::emit(crate::session_events::SessionEvent::TabOrderChanged {
                workspace_path: ws.path.clone(),
                project: ws.id.clone(),
                workspace: workspace_id,
                revision,
            });
        }
        Ok(_) => {}
        Err(e) => log_debug!("[sidecar-worktree] layout remove failed pg={}: {e}", s.row.pane_group_id),
    }

    // The worktree: only ever through B0's guarded path (a linked
    // worktree of this workspace's repository), to the Trash.
    let trashed = path.exists();
    k2_core::git::remove_worktree(&ws.path, &path.to_string_lossy(), false).map_err(|e| {
        let code = k2_core::git::refusal::code_of(&e).unwrap_or("worktree_remove_failed");
        Refused::new(code, 409, format!("the worktree was not removed: {e}"))
    })?;

    let branch_deleted = if !keep_branch && (unlanded.is_empty() || discard) {
        wt::delete_branch(root, &meta.branch)?;
        true
    } else {
        false
    };
    let how = if meta.landed_at.is_some() && unlanded.is_empty() { store::RETIRED_LANDED } else { store::RETIRED_DISCARDED };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(id) = &meta.worktree_ws_id {
            let _ = k2_core::db::schema::Workspace::delete(&conn, id);
        }
        store::mark_retired(&conn, &ws.id, &s.row.pane_group_id, how, now())
            .map_err(|e| Refused::new("internal", 500, e))?;
    }
    crate::fs_live::resync_watches();
    let _ = crate::git_live::changed_now(&ws.path);
    let tip = unlanded.last().map(|c| c.sha.clone());
    let mut line = format!(
        "[K2] Retired {}: the worktree went to the Trash{}.",
        s.address,
        if branch_deleted { format!(" and {} was deleted", meta.branch) } else { String::new() }
    );
    if discard && !unlanded.is_empty() {
        if let Some(t) = &tip {
            line.push_str(&format!(
                " Discarded {} commit(s); until git cleans up, `git branch {} {}` brings them back.",
                unlanded.len(),
                meta.branch,
                short(t)
            ));
        }
    }
    tell(ws, s, &line);
    Ok(json!({
        "ok": true,
        "address": s.address,
        "retired": true,
        "how": how,
        "stopped": stopped,
        "worktreeTrashed": trashed,
        "branchDeleted": branch_deleted,
        "discardedCommits": if discard { json!(unlanded) } else { json!([]) },
        "discardedFiles": if discard { json!(dirty) } else { json!([]) },
    }))
}
