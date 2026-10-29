//! Changes tab v1.1 (C5) — review notes routes (`.k2/prds/prd-changes-tab-v1.md`
//! "C5"). The store, anchors and message format are
//! `k2_core::git_notes`; this file is auth, validation, events and send.
//!
//! | Route | Method | Humans (owner / Member+) | Agent passport (cell) |
//! |---|---|---|---|
//! | `/cli/git/notes` | GET | list | list, own workspace |
//! | `/cli/git/notes/add` | POST | yes | no |
//! | `/cli/git/notes/update` | POST | author or Admin+ | no |
//! | `/cli/git/notes/resolve` | POST | yes | own workspace |
//! | `/cli/git/notes/delete` | POST | author or Admin+ | no |
//! | `/cli/git/notes/send` | POST | yes | no |
//!
//! Agents only read the notes left for them and resolve them when fixed:
//! a note is a person's review, so an agent never writes one or sends
//! notes on a person's behalf. Every change emits `git_notes_changed`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use serde::Deserialize;

use crate::cli_response::CliResponse;
use k2_core::git_changes;
use k2_core::git_notes::{self, ReviewNote, Status};

pub const LIST_ROUTE: &str = "/cli/git/notes";
pub const POST_ROUTES: &[&str] = &[
    "/cli/git/notes/add",
    "/cli/git/notes/delete",
    "/cli/git/notes/resolve",
    "/cli/git/notes/send",
    "/cli/git/notes/update",
];
/// What an agent passport may call (cell socket, own workspace).
pub const AGENT_ROUTES: &[&str] = &["/cli/git/notes", "/cli/git/notes/resolve"];

pub fn is_route(path: &str) -> bool {
    path == LIST_ROUTE || POST_ROUTES.contains(&path)
}

/// Who is calling, derived from the credential (never the request body).
#[derive(Debug, Clone)]
pub enum Caller {
    /// The owner token or a Connect login (Member and up).
    Human {
        /// `owner-token` or `user:<name>` (routes::http::actor_label).
        actor: String,
        /// What notes and the sent message show.
        name: String,
        /// Owner or Admin: may edit or delete anyone's note.
        admin: bool,
    },
    /// An agent passport on its cell socket.
    Agent {
        /// `agent:<handle>`.
        actor: String,
        /// The passport's workspace folder.
        own_workspace: String,
    },
}

impl Caller {
    fn actor(&self) -> &str {
        match self {
            Caller::Human { actor, .. } | Caller::Agent { actor, .. } => actor,
        }
    }
}

/// The human caller behind an owner token or a Connect login.
pub fn human_caller(actor: Option<String>, role: Option<k2_core::connect_users::Role>) -> Caller {
    let actor = actor.unwrap_or_else(|| "owner-token".to_string());
    let name = match actor.strip_prefix("user:") {
        Some(u) => u.to_string(),
        None => crate::workspace_msg::resolve_owner_from(),
    };
    let admin = role.is_some_and(|r| r >= k2_core::connect_users::Role::Admin);
    Caller::Human { actor, name, admin }
}

/// The caller for the TCP list (a read: who asks doesn't change the answer;
/// the token gate already ran).
pub fn list_caller() -> Caller {
    Caller::Human { actor: "owner-token".to_string(), name: String::new(), admin: false }
}

// ── Responses ─────────────────────────────────────────────────────────

fn err(status: &'static str, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "error": hint.to_string(), "code": code }).to_string(),
    }
}

fn bad_request(hint: impl std::fmt::Display) -> CliResponse {
    err("400 Bad Request", "bad_request", hint)
}

fn forbidden(hint: impl std::fmt::Display) -> CliResponse {
    err("403 Forbidden", "forbidden", hint)
}

fn not_found(hint: impl std::fmt::Display) -> CliResponse {
    err("404 Not Found", "not_found", hint)
}

fn db_error(e: rusqlite::Error) -> CliResponse {
    CliResponse::internal_error(format!("review notes: {e}"))
}

fn changes_error(e: git_changes::ChangesError) -> CliResponse {
    err("400 Bad Request", e.code, e.hint)
}

fn ok(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn trim_ws(path: &str) -> String {
    let t = path.trim();
    if t.len() > 1 { t.trim_end_matches('/').to_string() } else { t.to_string() }
}

// ── Workspaces ────────────────────────────────────────────────────────

/// The K2 workspace (projects row) that owns `path`: an exact project
/// root, a registered worktree checkout, else the deepest project root
/// above it. `(project id, project root)`.
pub fn owning_project(path: &str) -> Option<(String, String)> {
    let path = trim_ws(path);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let exact: Option<(String, String)> = conn
        .query_row("SELECT id, path FROM projects WHERE path = ?1", [&path], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok();
    if exact.is_some() {
        return exact;
    }
    let wt: Option<(String, String)> = conn
        .query_row(
            "SELECT p.id, p.path FROM workspaces w JOIN projects p ON p.id = w.project_id WHERE w.worktree_path = ?1",
            [&path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    if wt.is_some() {
        return wt;
    }
    let mut stmt = conn.prepare("SELECT id, path FROM projects").ok()?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).ok()?;
    let mut best: Option<(String, String)> = None;
    for (id, p) in rows.flatten() {
        let root = trim_ws(&p);
        if root.is_empty() || root == "/" || !k2_core::paths::is_under(&path, &root) || path == root {
            continue;
        }
        if best.as_ref().is_none_or(|(_, b)| b.len() < root.len()) {
            best = Some((id, root));
        }
    }
    best
}

/// Is `child` the folder `parent` or inside it (canonical spellings)?
fn inside(child: &str, parent: &str) -> bool {
    let canon = |p: &str| dunce::canonicalize(&p).unwrap_or_else(|_| std::path::PathBuf::from(p));
    canon(child).starts_with(canon(parent))
}

/// Note JSON plus where it is now (`outdated`, `currentStartLine`,
/// `currentEndLine`). `texts` caches each file side's texts per call.
fn annotated(n: &ReviewNote, texts: &mut HashMap<(String, String), Vec<String>>) -> serde_json::Value {
    let key = (n.path.clone(), n.side.clone());
    let t = texts.entry(key).or_insert_with(|| git_notes::side_texts(&n.workspace_path, &n.path, &n.side));
    let place = git_notes::place(n, t);
    let mut v = serde_json::to_value(n).unwrap_or_else(|_| serde_json::json!({}));
    if let (Some(obj), Ok(serde_json::Value::Object(p))) = (v.as_object_mut(), serde_json::to_value(place)) {
        obj.extend(p);
    }
    v
}

fn emit_for(paths: impl IntoIterator<Item = String>) {
    let unique: BTreeSet<String> = paths.into_iter().collect();
    for p in unique {
        crate::session_events::emit_git_notes_changed(&p);
    }
}

// ── GET /cli/git/notes ────────────────────────────────────────────────

/// `path` (required for people; an agent's defaults to its workspace),
/// `status=open|unsent|sent|resolved|all` (default open), `file_path`.
pub fn handle_list(params: &HashMap<String, String>, caller: &Caller) -> CliResponse {
    let requested = params.get("path").cloned().unwrap_or_default();
    let path = match caller {
        Caller::Agent { own_workspace, .. } => match crate::git_routes::agent_scope(&requested, own_workspace) {
            Ok(p) => trim_ws(&p),
            Err(r) => return r,
        },
        Caller::Human { .. } => {
            if requested.trim().is_empty() {
                return bad_request("Missing 'path' parameter");
            }
            trim_ws(&requested)
        }
    };
    let Some(status) = git_notes::Status::parse(params.get("status").map(String::as_str)) else {
        return bad_request("status must be open, unsent, sent, resolved or all");
    };
    let file = match params.get("file_path").map(|f| f.trim()).filter(|f| !f.is_empty()) {
        None => None,
        Some(f) => match git_changes::repo_key_and_root(&path) {
            Some((_, root)) => match git_changes::normalize_rel_path(std::path::Path::new(&root), f) {
                Ok(r) => Some(r),
                Err(e) => return changes_error(e),
            },
            None => Some(f.to_string()),
        },
    };
    let (rows, counts) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let rows = match git_notes::list(&conn, &path, status, file.as_deref()) {
            Ok(r) => r,
            Err(e) => return db_error(e),
        };
        match git_notes::counts(&conn, &path) {
            Ok(c) => (rows, c),
            Err(e) => return db_error(e),
        }
    };
    let mut texts = HashMap::new();
    let notes: Vec<serde_json::Value> = rows.iter().map(|n| annotated(n, &mut texts)).collect();
    ok(serde_json::json!({ "workspacePath": path, "counts": counts, "notes": notes }))
}

// ── POST bodies ───────────────────────────────────────────────────────

fn parse<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, CliResponse> {
    serde_json::from_slice(body).map_err(|e| bad_request(format!("invalid JSON: {e}")))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddBody {
    path: String,
    file_path: String,
    #[serde(default)]
    side: Option<String>,
    start_line: i64,
    #[serde(default)]
    end_line: Option<i64>,
    body: String,
    #[serde(default)]
    anchor_text: Option<String>,
    #[serde(default)]
    rev: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdsBody {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    ids: Vec<String>,
    /// resolve only: false reopens.
    #[serde(default)]
    resolved: Option<bool>,
    /// An agent's optional workspace hint (ignored for scoping).
    #[serde(default)]
    #[allow(dead_code)]
    path: Option<String>,
}

impl IdsBody {
    fn all(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for id in self.id.iter().chain(self.ids.iter()) {
            let id = id.trim().to_string();
            if !id.is_empty() && !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }
}

#[derive(Deserialize)]
struct UpdateBody {
    id: String,
    body: String,
}

#[derive(Deserialize)]
struct SendBody {
    path: String,
    #[serde(default)]
    ids: Vec<String>,
    #[serde(default)]
    wake: Option<bool>,
    /// 0.46.7 Workbench: a sidecar's Thread address (`ws/name`) in the
    /// workspace that holds `path`, so a worktree agent's notes reach that
    /// agent, not the workspace's main agent. Absent: the workspace.
    #[serde(default)]
    to: Option<String>,
}

/// `to` when it is a sidecar of the workspace `project_id` (`<its handle>/<name>`).
fn send_target(to: &str, project_id: &str) -> Result<String, CliResponse> {
    let to = to.trim().to_ascii_lowercase();
    let handle = k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_default();
    match to.split_once('/') {
        Some((ws, name)) if !handle.is_empty() && ws == handle && !name.is_empty() && !name.contains('/') => Ok(to),
        _ => Err(bad_request(format!("'to' must be a sidecar of this workspace ({handle}/<name>)"))),
    }
}

fn humans_only(caller: &Caller) -> Option<CliResponse> {
    match caller {
        Caller::Human { .. } => None,
        Caller::Agent { .. } => Some(forbidden(
            "agents can list review notes and resolve them; notes are written and sent by people",
        )),
    }
}

fn may_edit(caller: &Caller, n: &ReviewNote) -> bool {
    match caller {
        Caller::Human { actor, admin, .. } => *admin || &n.author == actor,
        Caller::Agent { .. } => false,
    }
}

/// Dispatch one POST notes route. `deliver` sends the batched message
/// (`deliver_to_agent` in production; a fake in tests).
pub fn dispatch_post(path: &str, body: &[u8], caller: &Caller, deliver: &Deliver) -> CliResponse {
    match path {
        "/cli/git/notes/add" => handle_add(body, caller),
        "/cli/git/notes/update" => handle_update(body, caller),
        "/cli/git/notes/resolve" => handle_resolve(body, caller),
        "/cli/git/notes/delete" => handle_delete(body, caller),
        "/cli/git/notes/send" => handle_send(body, caller, deliver),
        _ => CliResponse::not_found(),
    }
}

fn handle_add(body: &[u8], caller: &Caller) -> CliResponse {
    if let Some(r) = humans_only(caller) {
        return r;
    }
    let Caller::Human { actor, name, .. } = caller else { unreachable!("checked above") };
    let b: AddBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let ws = trim_ws(&b.path);
    if ws.is_empty() {
        return bad_request("Missing 'path'");
    }
    let Some((project_id, _)) = owning_project(&ws) else {
        return not_found(format!("no K2 workspace holds {ws}"));
    };
    let Some((rev_key, root)) = git_changes::repo_key_and_root(&ws) else {
        return err("400 Bad Request", git_changes::codes::NOT_A_REPO, format!("{ws} is not in a git repository"));
    };
    let rel = match git_changes::normalize_rel_path(std::path::Path::new(&root), &b.file_path) {
        Ok(r) => r,
        Err(e) => return changes_error(e),
    };
    let side = b.side.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("new").to_string();
    if side != "new" && side != "old" {
        return bad_request("side must be new or old");
    }
    let start = b.start_line;
    let end = b.end_line.unwrap_or(start);
    if start < 1 || end < start {
        return bad_request("startLine must be 1 or more and endLine at least startLine");
    }
    if end - start + 1 > git_notes::MAX_LINE_SPAN {
        return bad_request(format!("a note covers at most {} lines", git_notes::MAX_LINE_SPAN));
    }
    let text = git_notes::clean_body(&b.body);
    if text.is_empty() {
        return bad_request("the note is empty");
    }
    if text.chars().count() > git_notes::MAX_BODY_CHARS {
        return bad_request(format!("a note is at most {} characters", git_notes::MAX_BODY_CHARS));
    }
    // The anchor: what the client saw at those lines (when it has that many
    // lines), else the lines from the side's current text.
    let span = (end - start + 1) as usize;
    let given = b.anchor_text.filter(|a| a.split('\n').count() == span && a.len() <= git_notes::MAX_BODY_CHARS * 10);
    let texts = git_notes::side_texts(&root, &rel, &side);
    let anchor = match given {
        Some(a) => Some(a),
        None => {
            let from_text = texts.iter().find_map(|t| git_notes::anchor_from_text(t, start, end));
            if from_text.is_none() && !texts.is_empty() {
                let lines = git_notes::split_lines(&texts[0]).len();
                return bad_request(format!("line {end} is past the end of {rel} ({lines} lines on the {side} side)"));
            }
            from_text
        }
    };
    let now = git_notes::now_ms();
    let note = ReviewNote {
        id: git_notes::new_id(),
        workspace_id: project_id,
        workspace_path: ws.clone(),
        path: rel.clone(),
        side: side.clone(),
        start_line: start,
        end_line: end,
        body: text,
        author: actor.clone(),
        author_name: name.clone(),
        created_at: now,
        updated_at: now,
        sent_at: None,
        delivery_id: None,
        resolved_at: None,
        resolved_by: None,
        rev: b.rev.filter(|r| !r.trim().is_empty()).or_else(|| Some(git_changes::current_rev(&rev_key))),
        blob: git_notes::side_blob_id(&root, &rel, &side),
        anchor_text: anchor,
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match git_notes::counts(&conn, &ws) {
            Ok(c) if c.open >= git_notes::MAX_OPEN_PER_WORKSPACE => {
                return err(
                    "409 Conflict",
                    "too_many_notes",
                    format!("{ws} already has {} open notes; resolve some first", c.open),
                )
            }
            Ok(_) => {}
            Err(e) => return db_error(e),
        }
        if let Err(e) = git_notes::insert(&conn, &note) {
            return db_error(e);
        }
    }
    crate::session_events::emit_git_notes_changed(&ws);
    let mut texts_cache = HashMap::new();
    texts_cache.insert((rel, side), texts);
    ok(serde_json::json!({ "success": true, "note": annotated(&note, &mut texts_cache) }))
}

fn load(id: &str) -> Result<ReviewNote, CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match git_notes::get(&conn, id) {
        Ok(Some(n)) => Ok(n),
        Ok(None) => Err(not_found(format!("no review note {id}"))),
        Err(e) => Err(db_error(e)),
    }
}

fn handle_update(body: &[u8], caller: &Caller) -> CliResponse {
    if let Some(r) = humans_only(caller) {
        return r;
    }
    let b: UpdateBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let note = match load(b.id.trim()) {
        Ok(n) => n,
        Err(r) => return r,
    };
    if !may_edit(caller, &note) {
        return err("403 Forbidden", "not_author", "only the note's author (or an Admin) can edit it");
    }
    let text = git_notes::clean_body(&b.body);
    if text.is_empty() {
        return bad_request("the note is empty");
    }
    if text.chars().count() > git_notes::MAX_BODY_CHARS {
        return bad_request(format!("a note is at most {} characters", git_notes::MAX_BODY_CHARS));
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = git_notes::update_body(&conn, &note.id, &text, git_notes::now_ms()) {
            return db_error(e);
        }
    }
    crate::session_events::emit_git_notes_changed(&note.workspace_path);
    let updated = match load(&note.id) {
        Ok(n) => n,
        Err(r) => return r,
    };
    ok(serde_json::json!({ "success": true, "note": annotated(&updated, &mut HashMap::new()) }))
}

fn handle_resolve(body: &[u8], caller: &Caller) -> CliResponse {
    let b: IdsBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let ids = b.all();
    if ids.is_empty() {
        return bad_request("give 'id' or 'ids'");
    }
    let resolve = b.resolved.unwrap_or(true);
    if !resolve && matches!(caller, Caller::Agent { .. }) {
        return forbidden("agents resolve notes; reopening one is for people");
    }
    // All-or-nothing checks first.
    let mut notes = Vec::with_capacity(ids.len());
    for id in &ids {
        match load(id) {
            Ok(n) => notes.push(n),
            Err(r) => return r,
        }
    }
    if let Caller::Agent { own_workspace, .. } = caller {
        if let Some(n) = notes.iter().find(|n| !inside(&n.workspace_path, own_workspace)) {
            return forbidden(format!("note {} is on {}, outside your workspace ({own_workspace})", n.id, n.workspace_path));
        }
    }
    let now = git_notes::now_ms();
    let mut changed = Vec::new();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for n in &notes {
            let already = n.resolved_at.is_some() == resolve;
            if already {
                continue;
            }
            let by = resolve.then(|| caller.actor().to_string());
            match git_notes::set_resolved(&conn, &n.id, by.as_deref(), now) {
                Ok(true) => changed.push(n.id.clone()),
                Ok(false) => {}
                Err(e) => return db_error(e),
            }
        }
    }
    emit_for(notes.iter().filter(|n| changed.contains(&n.id)).map(|n| n.workspace_path.clone()));
    ok(serde_json::json!({
        "success": true,
        "resolved": resolve,
        "changed": changed,
        "unchanged": ids.iter().filter(|i| !changed.contains(i)).collect::<Vec<_>>(),
    }))
}

fn handle_delete(body: &[u8], caller: &Caller) -> CliResponse {
    if let Some(r) = humans_only(caller) {
        return r;
    }
    let b: IdsBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let ids = b.all();
    if ids.is_empty() {
        return bad_request("give 'id' or 'ids'");
    }
    let mut notes = Vec::with_capacity(ids.len());
    for id in &ids {
        match load(id) {
            Ok(n) => notes.push(n),
            Err(r) => return r,
        }
    }
    if let Some(n) = notes.iter().find(|n| !may_edit(caller, n)) {
        return err("403 Forbidden", "not_author", format!("only the author of {} (or an Admin) can delete it", n.id));
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for n in &notes {
            if let Err(e) = git_notes::delete(&conn, &n.id) {
                return db_error(e);
            }
        }
    }
    emit_for(notes.iter().map(|n| n.workspace_path.clone()));
    ok(serde_json::json!({ "success": true, "deleted": ids }))
}

// ── Send ──────────────────────────────────────────────────────────────

/// One delivery of the batched message to a workspace's agent.
pub struct DeliverRequest<'a> {
    /// The owning K2 workspace's root (the `k2 msg` target).
    pub target: &'a str,
    pub text: &'a str,
    /// The person's name, for the `[from <name>]` stamp.
    pub from: &'a str,
    /// Credential-derived actor for the delivery record.
    pub actor: &'a str,
    pub wake: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DeliverResult {
    pub success: bool,
    pub delivery_id: Option<String>,
    pub reason: Option<String>,
    pub hint: Option<String>,
}

pub type Deliver = dyn Fn(&DeliverRequest<'_>) -> DeliverResult + Send + Sync;

/// Production delivery: the `/cli/workspace/msg` path (recorded delivery
/// id in the `[from <name> · m_…]` stamp, live inject with its checks,
/// wake a dormant agent unless `wake` is false).
pub fn deliver_to_agent(req: &DeliverRequest<'_>) -> DeliverResult {
    let sender = crate::hire_routes::Sender {
        workspace_id: None,
        via: "owner",
        actor: req.actor.to_string(),
        on_behalf_of: None,
    };
    let (resp, id) = crate::hire_routes::deliver_recorded(
        req.target,
        req.text,
        req.from,
        "",
        req.wake,
        crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
        "msg",
        &sender,
    );
    DeliverResult { success: resp.success, delivery_id: id, reason: resp.reason, hint: resp.hint }
}

/// Workspaces with a send in flight: a second send waits its turn by
/// refusing (409 `send_in_progress`), so a note never goes out twice.
static SENDING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

struct SendGuard(String);

impl Drop for SendGuard {
    fn drop(&mut self) {
        SENDING.lock().unwrap_or_else(|p| p.into_inner()).remove(&self.0);
    }
}

fn handle_send(body: &[u8], caller: &Caller, deliver: &Deliver) -> CliResponse {
    if let Some(r) = humans_only(caller) {
        return r;
    }
    let Caller::Human { actor, name, .. } = caller else { unreachable!("checked above") };
    let b: SendBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let ws = trim_ws(&b.path);
    if ws.is_empty() {
        return bad_request("Missing 'path'");
    }
    let Some((project_id, project_root)) = owning_project(&ws) else {
        return not_found(format!("no K2 workspace holds {ws}"));
    };
    let target = match b.to.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(to) => match send_target(to, &project_id) {
            Ok(t) => t,
            Err(r) => return r,
        },
        None => project_root.clone(),
    };
    if !SENDING.lock().unwrap_or_else(|p| p.into_inner()).insert(ws.clone()) {
        return err("409 Conflict", "send_in_progress", "these notes are being sent right now");
    }
    let _guard = SendGuard(ws.clone());
    let pending: Vec<ReviewNote> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match git_notes::list(&conn, &ws, Status::Unsent, None) {
            Ok(r) => r,
            Err(e) => return db_error(e),
        }
    };
    let wanted: Vec<String> = b.ids.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let pending: Vec<ReviewNote> =
        if wanted.is_empty() { pending } else { pending.into_iter().filter(|n| wanted.contains(&n.id)).collect() };
    if pending.is_empty() {
        return err("409 Conflict", "nothing_to_send", "no open notes waiting to be sent");
    }
    let (text, take) = git_notes::format_message(&pending, &ws);
    let res = deliver(&DeliverRequest { target: &target, text: &text, from: name, actor, wake: b.wake.unwrap_or(true) });
    if !res.success {
        let reason = res.reason.clone().unwrap_or_else(|| "not_delivered".to_string());
        let hint = res.hint.clone().unwrap_or_else(|| format!("the message did not reach the agent ({reason})"));
        return CliResponse {
            status: "502 Bad Gateway",
            content_type: "application/json",
            body: serde_json::json!({ "error": hint, "code": "not_delivered", "reason": reason }).to_string(),
        };
    }
    let ids: Vec<String> = pending[..take].iter().map(|n| n.id.clone()).collect();
    let marked = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match git_notes::mark_sent(&conn, &ids, res.delivery_id.as_deref(), git_notes::now_ms()) {
            Ok(n) => n,
            Err(e) => return db_error(e),
        }
    };
    crate::session_events::emit_git_notes_changed(&ws);
    ok(serde_json::json!({
        "success": true,
        "sent": marked,
        "ids": ids,
        "remaining": pending.len() - take,
        "deliveryId": res.delivery_id,
        "target": target,
    }))
}

#[cfg(test)]
#[path = "git_notes_routes_tests.rs"]
mod tests;
