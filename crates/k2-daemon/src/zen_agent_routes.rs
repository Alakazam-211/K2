//! Garden and widget edits through the daemon, and the proposal queue
//! (agents run Gardens end to end, 0.46.0, migration 0181).
//!
//! `zen_routes::handle` has already applied Zen's rules (this computer's
//! owner token, or a LOCAL agent / K2 shell passport; a Connect login, an
//! app pass or a sandboxed agent gets 403 `zen_local_only`), refused a GET
//! on a POST row, and refused a put that names a locked Garden (423).
//!
//! | route                         | person | agent with the switch | other agent |
//! |-------------------------------|--------|-----------------------|-------------|
//! | POST garden/put, widget/put   | writes | writes (audited)      | proposes    |
//! | GET  proposals, proposals/show| yes    | yes                   | yes         |
//! | POST proposals/approve        | yes    | yes (audited)         | 403         |
//! | POST proposals/reject         | yes    | yes (audited)         | own only (withdraw) |
//! | POST widget/folder/put        | writes | writes (audited)      | proposes (code files, ONE proposal for the folder, 0.46.3) |
//! | POST widget/folder/check, garden/check | yes | yes            | yes (writes nothing) |
//! | POST widget/preview           | yes    | yes (audited)         | 403     |
//!
//! The widget dev loop (0.46.1 S3, no migration): a whole staged folder in
//! one body (`k2 zen widget put <name> --dir`, `k2 zen validate --dir`,
//! `k2 zen widget preview --dir`), read by the CLI, never by the daemon
//! from a path. A preview lands on K2's own "Widget preview" Garden only.
//!
//! "The switch" is "Manage Gardens and data sources" (`gardens_manage`,
//! [`crate::gardens_manage`]). A write is checked before it lands: the
//! Garden must exist, be unlocked and check clean; a widget must be a user
//! folder (never K2's built-in, frozen `k2:<name>`), one code file. A
//! proposal is checked the same way when it is made, and again when it is
//! approved, which also needs the file unchanged since (409
//! `proposal_stale`). Nothing here writes zen.toml, themes, sync state,
//! `.defaults/` or a template.

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::zen::agent_edit::{self, NewProposal, Proposal, ProposalKind};
use k2_core::zen::widget_folder::{self, StagedFile};
use k2_core::zen::{ZenError, ZenFiles};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

pub const POST_ROUTES: &[&str] = &[
    "/cli/zen/garden/check",
    "/cli/zen/garden/put",
    "/cli/zen/proposals/approve",
    "/cli/zen/proposals/reject",
    "/cli/zen/widget/folder/check",
    "/cli/zen/widget/folder/put",
    "/cli/zen/widget/preview",
    "/cli/zen/widget/put",
];

pub const GET_ROUTES: &[&str] = &["/cli/zen/proposals", "/cli/zen/proposals/show"];

/// The widget dev loop's folder routes (0.46.1 S3): a whole staged folder
/// in one body, so they take up to `widget_folder::MAX_FOLDER_BODY`.
pub const FOLDER_ROUTES: &[&str] = &["/cli/zen/widget/folder/check", "/cli/zen/widget/folder/put", "/cli/zen/widget/preview"];

pub fn is_route(p: &str) -> bool {
    POST_ROUTES.contains(&p) || GET_ROUTES.contains(&p)
}

fn resp(status: &'static str, body: J) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: body.to_string() }
}

fn refuse(status: &'static str, code: &str, message: impl Into<String>) -> CliResponse {
    resp(status, json!({ "ok": false, "error": code, "message": message.into() }))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn files() -> ZenFiles {
    ZenFiles::local()
}

/// Who is asking, as far as these routes care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// This computer's owner token (the person).
    Person,
    /// A local passport whose workspace has "Manage Gardens and data
    /// sources".
    Manager,
    /// Any other local passport.
    Agent,
}

fn role(owner_token: bool, caller: &Caller) -> Role {
    if owner_token {
        Role::Person
    } else if crate::gardens_manage::caller_may(caller) {
        Role::Manager
    } else {
        Role::Agent
    }
}

fn project_of(caller: &Caller) -> Option<&str> {
    match caller {
        Caller::Agent(a) => Some(&a.project_id),
        Caller::Shell { project_id } => Some(project_id),
        _ => None,
    }
}

/// The calling passport's session id (for the proposal row and audit).
fn session_of(caller: &Caller) -> Option<String> {
    match caller {
        Caller::Agent(a) => a
            .map_key
            .as_deref()
            .and_then(crate::v2_session_map::lookup_by_agent_name)
            .map(|s| s.session_id.to_string()),
        _ => None,
    }
}

/// One Access Audit line for a passport's act; a person's put or decision
/// gets one too (`owner`), so the queue's history is in one place.
fn audit(event: &str, caller: &Caller, target: &str) {
    let user = caller.audit_user();
    crate::gardens_manage::audit(event, &user, target, None, session_of(caller).as_deref());
}

/// `ZenError` → the same JSON `zen_routes` answers.
fn zen_err(e: ZenError) -> CliResponse {
    crate::zen_routes::error_response(e)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GardenPut {
    garden: String,
    text: String,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WidgetPut {
    widget: String,
    file: String,
    text: String,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdBody {
    id: String,
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8], shape: &str) -> Result<T, CliResponse> {
    serde_json::from_slice(body).map_err(|e| refuse("400 Bad Request", "bad_request", format!("{shape}: {e}")))
}

fn clean_note(note: Option<String>) -> Result<Option<String>, CliResponse> {
    let n = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    if n.as_ref().is_some_and(|n| n.chars().count() > agent_edit::MAX_NOTE_CHARS || n.chars().any(|c| c.is_control() && c != '\n')) {
        return Err(refuse("400 Bad Request", "bad_request", "a note is one paragraph of at most 500 characters"));
    }
    Ok(n)
}

/// Run one route. `owner_token`: this computer's owner token presented
/// (the person). Answers 404 for a path that isn't this module's.
pub fn handle(path: &str, owner_token: bool, caller: &Caller, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    let r = role(owner_token, caller);
    match path {
        "/cli/zen/garden/put" => garden_put(r, caller, body),
        "/cli/zen/garden/check" => garden_check(r, body),
        "/cli/zen/widget/put" => widget_put(r, caller, body),
        "/cli/zen/widget/folder/check" => folder_check(body),
        "/cli/zen/widget/folder/put" => folder_put(r, caller, body),
        "/cli/zen/widget/preview" => preview(r, caller, body),
        "/cli/zen/proposals" => list(params),
        "/cli/zen/proposals/show" => show(params),
        "/cli/zen/proposals/approve" => approve(r, caller, body),
        "/cli/zen/proposals/reject" => reject(r, caller, body),
        _ => refuse("404 Not Found", "not_found", "no such zen route"),
    }
}

fn after_write() {
    if let Err(e) = crate::zen_routes::refresh_and_emit() {
        k2_core::log_debug!("[zen/agent] refresh after write: {e}");
    }
}

// ── "Keep as mine" (0.46.3, Garden gaps) ──────────────────────────────────
//
// A widget version reads its data with its maker's rights
// (contract-garden-data-v1 §4): when an agent changes a widget whose live
// version was the person's, the new version is nobody's (or the agent's)
// until the person keeps it, and the widget quietly loses its data. The
// rule stays (only the person claims); K2 now TELLS them: one inbox note
// per widget, pointing at the one-click It's mine in Settings → Gardens →
// Local Data. Nothing here claims anything.

/// The live version (bundle hash) of a user widget and its maker, before a
/// write. `None`: no good version, a built-in, or no such widget.
fn live_maker(widget: &str) -> Option<(String, k2_core::garden_data::MadeBy)> {
    let name = widget.trim();
    let hash = files().widget_status(name).bundle.map(|b| b.hash)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let maker = k2_core::garden_data::authors::maker_of(&conn, name, &hash).ok()?;
    Some((hash, maker))
}

/// The inbox note's title for `widget` (also its dedupe key).
pub(crate) fn keep_title(widget: &str) -> String {
    format!("Keep your widget {widget} as yours?")
}

/// After a write: the person's widget has a new live version that isn't
/// theirs → `true`, and, when an agent wrote it (`note`), one fyi note to
/// the owner (none while one is open). The person's own approval gets no
/// note, only the answer's `keepAsMine` (the CLI prints where to keep it).
fn keep_as_mine_notice(widget: &str, before: Option<(String, k2_core::garden_data::MadeBy)>, caller: &Caller, note: bool) -> bool {
    use k2_core::garden_data::MadeBy;
    let Some((old_hash, MadeBy::Owner)) = before else { return false };
    let Some((new_hash, maker)) = live_maker(widget) else { return false };
    if new_hash == old_hash || maker == MadeBy::Owner {
        return false;
    }
    if !note {
        return true;
    }
    let Some(project) = project_of(caller) else { return true };
    let title = keep_title(widget);
    let open = k2_core::feedback::list_for_project(project, &k2_core::feedback::ListFilter::Open).unwrap_or_default();
    if open.iter().any(|i| i.title == title) {
        return true;
    }
    let who = caller.audit_user();
    let body = format!(
        "{who} changed your widget {widget} (version {} → {}). The new version isn't signed as yours, so it can't read or write the widget's data until you keep it.\n\n\
         One click: Settings → Gardens → Local Data → {widget} → It's mine. Or leave it, and the widget runs without its data.\n\
         Every change is in Settings → Logs → Access Audit.",
        short(&old_hash),
        short(&new_hash)
    );
    if let Err(e) = crate::feedback_routes::file_owner_notice(project, &who, &title, &body) {
        k2_core::log_debug!("[zen/agent] keep-as-mine note for {widget}: {e}");
    }
    true
}

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

/// Save one proposal. `base_hash`: [`agent_edit::text_hash`] of the file
/// it is made against, or a folder proposal's own fingerprint.
fn propose(caller: &Caller, kind: ProposalKind, target: &str, file: Option<&str>, text: &str, base_hash: &str, note: Option<&str>) -> CliResponse {
    let Some(project) = project_of(caller) else {
        return refuse("403 Forbidden", "zen_local_only", "only a person or a local agent may change Gardens");
    };
    let session = session_of(caller);
    let made_by = format!("agent:{project}");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let p = agent_edit::insert(
        &conn,
        &NewProposal {
            kind,
            target,
            file,
            content: text,
            base_hash,
            note,
            made_by: &made_by,
            made_by_project: project,
            made_by_session: session.as_deref(),
            now: now(),
        },
    );
    drop(conn);
    match p {
        Ok(p) => {
            audit("zen-propose", caller, &format!("{} {}", p.id, describe(&p)));
            resp(
                "200 OK",
                json!({
                    "ok": true,
                    "proposed": true,
                    "proposal": p.summary(),
                    "message": format!(
                        "Proposed {}. A person approves it (Settings → Gardens, or k2 zen proposals approve {} outside K2), or an agent with \"Manage Gardens and data sources\".",
                        p.id, p.id
                    ),
                }),
            )
        }
        Err(e) => refuse("500 Internal Server Error", "db_error", format!("the proposal wasn't saved: {e}")),
    }
}

fn describe(p: &Proposal) -> String {
    match (&p.kind, &p.file) {
        (ProposalKind::Folder, _) => {
            let files = p.folder().map(|f| f.files.iter().map(|x| x.name.clone()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            format!("widgets/{}/ ({files})", p.target)
        }
        (_, Some(f)) => format!("widgets/{}/{f}", p.target),
        (_, None) => format!("gardens/{}.toml", p.target),
    }
}

fn garden_put(r: Role, caller: &Caller, body: &[u8]) -> CliResponse {
    let b: GardenPut = match parse(body, "garden/put takes {garden, text, note?}") {
        Ok(b) => b,
        Err(e) => return e,
    };
    let note = match clean_note(b.note) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let f = files();
    match r {
        Role::Person | Role::Manager => match f.put_garden(&b.garden, &b.text, r == Role::Person) {
            Ok(v) => {
                after_write();
                if r == Role::Manager {
                    audit("zen-garden-put", caller, v["file"].as_str().unwrap_or(&b.garden));
                }
                resp("200 OK", v)
            }
            Err(e) => zen_err(e),
        },
        Role::Agent => {
            let id = match f.check_garden_put(&b.garden, &b.text, false) {
                Ok(id) => id,
                Err(e) => return zen_err(e),
            };
            let base = match f.garden_text(&id) {
                Ok((_, t)) => t,
                Err(e) => return zen_err(e),
            };
            propose(caller, ProposalKind::Garden, &id, None, &b.text, &agent_edit::text_hash(base.as_deref()), note.as_deref())
        }
    }
}

fn widget_put(r: Role, caller: &Caller, body: &[u8]) -> CliResponse {
    let b: WidgetPut = match parse(body, "widget/put takes {widget, file, text, note?}") {
        Ok(b) => b,
        Err(e) => return e,
    };
    let note = match clean_note(b.note) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let f = files();
    match r {
        Role::Person | Role::Manager => {
            let before = live_maker(&b.widget);
            match f.put_widget_file(&b.widget, &b.file, &b.text) {
                Ok(mut v) => {
                    after_write();
                    if r == Role::Manager {
                        audit("zen-widget-put", caller, v["file"].as_str().unwrap_or(&b.widget));
                        v["keepAsMine"] = json!(keep_as_mine_notice(&b.widget, before, caller, true));
                    }
                    resp("200 OK", v)
                }
                Err(e) => zen_err(e),
            }
        }
        Role::Agent => {
            let name = match f.check_widget_put(&b.widget, &b.file, &b.text) {
                Ok(n) => n,
                Err(e) => return zen_err(e),
            };
            let base = match f.widget_file_text(&name, &b.file) {
                Ok((_, t)) => t,
                Err(e) => return zen_err(e),
            };
            propose(caller, ProposalKind::Widget, &name, Some(&b.file), &b.text, &agent_edit::text_hash(base.as_deref()), note.as_deref())
        }
    }
}

// ── the widget dev loop (0.46.1 S3) ─────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GardenCheck {
    garden: String,
    text: String,
}

/// `{widget?, fallback?, files}`: `widget` names it, else the manifest's
/// `id`, else `fallback` (the staged folder's own name, from the CLI).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderCheck {
    #[serde(default)]
    widget: Option<String>,
    #[serde(default)]
    fallback: Option<String>,
    files: Vec<StagedFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderPut {
    widget: String,
    files: Vec<StagedFile>,
    #[serde(default)]
    prune: bool,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewBody {
    #[serde(default)]
    widget: Option<String>,
    #[serde(default)]
    fallback: Option<String>,
    #[serde(default)]
    files: Vec<StagedFile>,
    #[serde(default)]
    config: BTreeMap<String, String>,
    #[serde(default)]
    clear: bool,
}

/// POST `garden/check {garden, text}`: check a staged Garden page against
/// that Garden without writing it (`k2 zen validate --garden <g> --from
/// <file>`). Any caller Zen admits; an agent naming a locked Garden gets
/// 423 like a put would.
fn garden_check(r: Role, body: &[u8]) -> CliResponse {
    let b: GardenCheck = match parse(body, "garden/check takes {garden, text}") {
        Ok(b) => b,
        Err(e) => return e,
    };
    let f = files();
    match f.check_garden_put(&b.garden, &b.text, r == Role::Person) {
        Ok(id) => resp(
            "200 OK",
            json!({ "ok": true, "status": "ok", "garden": id, "errors": [], "warnings": [], "files": [format!("staged gardens/{id}.toml")] }),
        ),
        Err(ZenError::BadRequest(m)) => resp(
            "200 OK",
            json!({ "ok": false, "status": "failed", "garden": b.garden, "errors": [{ "file": "staged", "line": 1, "col": 1, "message": m }], "warnings": [], "files": ["staged"] }),
        ),
        Err(e) => zen_err(e),
    }
}

/// POST `widget/folder/check`: check a staged folder, write nothing. Zen
/// needn't be set up (an agent checks before anything is added).
fn folder_check(body: &[u8]) -> CliResponse {
    let b: FolderCheck = match parse(body, "widget/folder/check takes {widget?, fallback?, files: [{name, text|base64}]}") {
        Ok(b) => b,
        Err(e) => return e,
    };
    let staged = match widget_folder::staged_files(&b.files) {
        Ok(s) => s,
        Err(e) => return zen_err(e),
    };
    let name = match widget_folder::staged_name(b.widget.as_deref(), &staged, b.fallback.as_deref()) {
        Ok(n) => n,
        Err(e) => return zen_err(e),
    };
    resp("200 OK", widget_folder::check_staged(&name, &staged))
}

/// POST `widget/folder/put`: the whole folder in one checked write. The
/// person and an agent with the switch write (a folder with errors is
/// refused, nothing written); any other agent's put becomes ONE proposal
/// for the folder (0.46.3; its changed code files, images and fonts need a
/// writer, no prune), approved and written as one.
fn folder_put(r: Role, caller: &Caller, body: &[u8]) -> CliResponse {
    let b: FolderPut = match parse(body, "widget/folder/put takes {widget, files: [{name, text|base64}], prune?, note?}") {
        Ok(b) => b,
        Err(e) => return e,
    };
    let note = match clean_note(b.note) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let staged = match widget_folder::staged_files(&b.files) {
        Ok(s) => s,
        Err(e) => return zen_err(e),
    };
    let f = files();
    match r {
        Role::Person | Role::Manager => {
            let before = live_maker(&b.widget);
            match f.put_widget_folder(&b.widget, &staged, b.prune) {
                Ok(mut v) => {
                    after_write();
                    if r == Role::Manager {
                        audit("zen-widget-folder-put", caller, v["file"].as_str().unwrap_or(&b.widget));
                        v["keepAsMine"] = json!(keep_as_mine_notice(&b.widget, before, caller, true));
                    }
                    resp("200 OK", v)
                }
                Err(e) => zen_err(e),
            }
        }
        Role::Agent => {
            if b.prune {
                return refuse(
                    "400 Bad Request",
                    "bad_request",
                    "--prune removes files; a proposal can't. Leave it out (extra files stay), or ask a person or an agent with \"Manage Gardens and data sources\".",
                );
            }
            let name = match f.check_widget_folder_put(&b.widget, &staged) {
                Ok(n) => n,
                Err(e) => return zen_err(e),
            };
            let proposal = match f.widget_folder_proposal(&name, &staged) {
                Ok(Some(p)) => p,
                Ok(None) => {
                    return resp("200 OK", json!({ "ok": true, "widget": name, "proposed": false, "changed": false, "proposals": [] }));
                }
                Err(e) => return zen_err(e),
            };
            // 0.46.3: the whole folder is ONE proposal, approved (and
            // written) as one, never file by file.
            let content = match serde_json::to_string(&proposal) {
                Ok(c) => c,
                Err(e) => return refuse("500 Internal Server Error", "internal", format!("the folder proposal didn't encode: {e}")),
            };
            let names: Vec<&str> = proposal.files.iter().map(|x| x.name.as_str()).collect();
            let r = propose(caller, ProposalKind::Folder, &name, None, &content, &proposal.base_hash(), note.as_deref());
            if r.status != "200 OK" {
                return r;
            }
            let v: J = match serde_json::from_str(&r.body) {
                Ok(v) => v,
                Err(e) => return refuse("500 Internal Server Error", "internal", format!("the proposal answer isn't JSON: {e}")),
            };
            let id = v["proposal"]["id"].as_str().unwrap_or_default().to_string();
            resp(
                "200 OK",
                json!({
                    "ok": true,
                    "widget": name,
                    "proposed": true,
                    "proposal": v["proposal"],
                    "proposals": [v["proposal"]],
                    "message": format!(
                        "Proposed one change to widgets/{name}/ as {id} ({} file(s): {}). A person approves it (Settings → Gardens, or k2 zen proposals approve {id} outside K2), or an agent with \"Manage Gardens and data sources\"; approving writes the whole folder at once. Until then the widget keeps running its last good version.",
                        names.len(),
                        names.join(", ")
                    ),
                }),
            )
        }
    }
}

/// POST `widget/preview`: the folder on K2's own "Widget preview" Garden
/// (never one of the person's Gardens), or `clear`. A preview places code
/// where it runs, so it is the person's or an agent with the switch's.
fn preview(r: Role, caller: &Caller, body: &[u8]) -> CliResponse {
    if r == Role::Agent {
        return refuse(
            "403 Forbidden",
            "person_only",
            "A preview places the widget on a Garden where it runs. Ask a person, or an agent with \"Manage Gardens and data sources\". Meanwhile check it with k2 zen validate --dir <folder>.",
        );
    }
    let b: PreviewBody = match parse(body, "widget/preview takes {widget?, fallback?, files, config?} or {clear: true}") {
        Ok(b) => b,
        Err(e) => return e,
    };
    let f = files();
    if b.clear {
        if !b.files.is_empty() {
            return refuse("400 Bad Request", "bad_request", "clear takes no files");
        }
        return match f.clear_widget_previews() {
            Ok(v) => {
                after_write();
                if r == Role::Manager {
                    audit("zen-widget-preview-clear", caller, widget_folder::PREVIEW_GARDEN);
                }
                resp("200 OK", v)
            }
            Err(e) => zen_err(e),
        };
    }
    let staged = match widget_folder::staged_files(&b.files) {
        Ok(s) => s,
        Err(e) => return zen_err(e),
    };
    match f.preview_widget_folder(b.widget.as_deref(), b.fallback.as_deref(), &staged, &b.config, r == Role::Person) {
        Ok(v) => {
            after_write();
            if r == Role::Manager {
                audit("zen-widget-preview", caller, v["preview"].as_str().unwrap_or("-"));
            }
            resp("200 OK", v)
        }
        Err(e) => zen_err(e),
    }
}

fn list(params: &HashMap<String, String>) -> CliResponse {
    let all = params.get("all").is_some_and(|v| v == "1" || v == "true");
    let db = k2_core::db::shared();
    let conn = db.lock();
    match agent_edit::list(&conn, all) {
        Ok(rows) => resp("200 OK", json!({ "ok": true, "proposals": rows.iter().map(Proposal::summary).collect::<Vec<_>>() })),
        Err(e) => refuse("500 Internal Server Error", "db_error", e.to_string()),
    }
}

fn load(id: &str) -> Result<Proposal, CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match agent_edit::get(&conn, id.trim()) {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Err(refuse("404 Not Found", "proposal_not_found", format!("no proposal {id} (k2 zen proposals)"))),
        Err(e) => Err(refuse("500 Internal Server Error", "db_error", e.to_string())),
    }
}

/// The file's text now (`None`: no file).
fn current_text(f: &ZenFiles, p: &Proposal) -> Result<Option<String>, ZenError> {
    match (&p.kind, &p.file) {
        (ProposalKind::Widget, Some(file)) => f.widget_file_text(&p.target, file).map(|(_, t)| t),
        _ => f.garden_text(&p.target).map(|(_, t)| t),
    }
}

fn show(params: &HashMap<String, String>) -> CliResponse {
    let Some(id) = params.get("id").filter(|s| !s.is_empty()) else {
        return refuse("400 Bad Request", "bad_request", "proposals/show needs id=<proposal>");
    };
    let p = match load(id) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let f = files();
    if p.kind == ProposalKind::Folder {
        return show_folder(&f, &p);
    }
    let current = current_text(&f, &p).ok().flatten();
    let stale = agent_edit::text_hash(current.as_deref()) != p.base_hash;
    let mut v = p.summary();
    v["content"] = json!(p.content);
    v["current"] = json!(current);
    v["stale"] = json!(stale);
    v["path"] = json!(describe(&p));
    resp("200 OK", json!({ "ok": true, "proposal": v }))
}

/// `show` for a folder proposal: `content` is its changed files
/// (`[{name, text, base}]`), `current` each one's text on disk now (`null`:
/// no file), `staleFiles` the files (changed or kept) that moved since.
fn show_folder(f: &ZenFiles, p: &Proposal) -> CliResponse {
    let fp = match p.folder() {
        Ok(fp) => fp,
        Err(m) => return refuse("500 Internal Server Error", "internal", m),
    };
    let mut current = serde_json::Map::new();
    for c in &fp.files {
        let now = f.widget_file_text(&p.target, &c.name).ok().and_then(|(_, t)| t);
        current.insert(c.name.clone(), json!(now));
    }
    let stale_files = f.folder_proposal_stale(&p.target, &fp).unwrap_or_else(|e| vec![format!("(can't check: {e})")]);
    let mut v = p.summary();
    v["content"] = json!(fp.files);
    v["current"] = J::Object(current);
    v["stale"] = json!(!stale_files.is_empty());
    v["staleFiles"] = json!(stale_files);
    v["path"] = json!(describe(p));
    resp("200 OK", json!({ "ok": true, "proposal": v }))
}

fn decided_by(caller: &Caller) -> String {
    caller.audit_user()
}

fn approve(r: Role, caller: &Caller, body: &[u8]) -> CliResponse {
    if r == Role::Agent {
        return refuse(
            "403 Forbidden",
            "person_only",
            "A person approves Garden proposals (Settings → Gardens, or k2 zen proposals approve outside K2), or an agent with \"Manage Gardens and data sources\".",
        );
    }
    let b: IdBody = match parse(body, "proposals/approve takes {id}") {
        Ok(b) => b,
        Err(e) => return e,
    };
    let p = match load(&b.id) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if p.status != "pending" {
        return refuse("409 Conflict", "not_pending", format!("{} is {}, not waiting", p.id, p.status));
    }
    let f = files();
    let before = if p.kind == ProposalKind::Garden { None } else { live_maker(&p.target) };
    let applied = if p.kind == ProposalKind::Folder {
        // The whole folder at once (0.46.3), or nothing.
        let fp = match p.folder() {
            Ok(fp) => fp,
            Err(m) => return refuse("500 Internal Server Error", "internal", m),
        };
        match f.apply_folder_proposal(&p.target, &fp) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(stale)) => {
                return refuse(
                    "409 Conflict",
                    "proposal_stale",
                    format!(
                        "widgets/{}/ changed since this was proposed ({}); nothing was written. Reject it and ask for a new one",
                        p.target,
                        stale.join(", ")
                    ),
                );
            }
            Err(e) => Err(e),
        }
    } else {
        let current = match current_text(&f, &p) {
            Ok(t) => t,
            Err(e) => return zen_err(e),
        };
        if agent_edit::text_hash(current.as_deref()) != p.base_hash {
            return refuse(
                "409 Conflict",
                "proposal_stale",
                format!("{} changed since this was proposed; reject it and ask for a new one", describe(&p)),
            );
        }
        match (&p.kind, &p.file) {
            (ProposalKind::Widget, Some(file)) => f.put_widget_file(&p.target, file, &p.content),
            _ => f.put_garden(&p.target, &p.content, r == Role::Person),
        }
    };
    let v = match applied {
        Ok(v) => v,
        Err(e) => return zen_err(e),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let closed = agent_edit::decide(&conn, &p.id, "approved", &decided_by(caller), now());
    drop(conn);
    if !matches!(closed, Ok(true)) {
        k2_core::log_debug!("[zen/agent] {} applied but was already decided: {closed:?}", p.id);
    }
    after_write();
    audit("zen-proposal-approve", caller, &format!("{} {}", p.id, describe(&p)));
    let keep = p.kind != ProposalKind::Garden && keep_as_mine_notice(&p.target, before, caller, r == Role::Manager);
    resp("200 OK", json!({ "ok": true, "id": p.id, "status": "approved", "applied": v, "keepAsMine": keep, "target": p.target }))
}

fn reject(r: Role, caller: &Caller, body: &[u8]) -> CliResponse {
    let b: IdBody = match parse(body, "proposals/reject takes {id}") {
        Ok(b) => b,
        Err(e) => return e,
    };
    let p = match load(&b.id) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let status = match r {
        Role::Person | Role::Manager => "rejected",
        Role::Agent if project_of(caller) == Some(p.made_by_project.as_str()) => "withdrawn",
        Role::Agent => {
            return refuse("403 Forbidden", "not_own", "an agent can only withdraw its own proposals");
        }
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let closed = agent_edit::decide(&conn, &p.id, status, &decided_by(caller), now());
    drop(conn);
    match closed {
        Ok(true) => {
            audit(&format!("zen-proposal-{}", if status == "withdrawn" { "withdraw" } else { "reject" }), caller, &p.id);
            resp("200 OK", json!({ "ok": true, "id": p.id, "status": status }))
        }
        Ok(false) => refuse("409 Conflict", "not_pending", format!("{} is {}, not waiting", p.id, p.status)),
        Err(e) => refuse("500 Internal Server Error", "db_error", e.to_string()),
    }
}

#[cfg(test)]
#[path = "zen_agent_routes_tests.rs"]
mod tests;
