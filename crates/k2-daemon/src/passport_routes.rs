//! The routes an agent's session passport may use outside the cell socket,
//! scoped to its own workspace.
//!
//! An agent in a session carries its passport (`<sid>.<secret>`). These are
//! the `/cli/*` routes agents are meant to use (their help, skills and
//! templates tell them to) that only the owner token or a person's login
//! reached over TCP before. Every other route keeps refusing a passport.
//!
//! How a passport is admitted over TCP, as `k2 workspace profile` was
//! (`workspace_profile_route`) and as the cell socket stamps one:
//! - the dispatcher's first arm takes a live passport on one of [`ROUTES`]
//!   ([`tcp_passport`]: `session_token::validate_hook`, a live session on
//!   this computer only), so no route's owner-token arm sees it; every other
//!   credential misses that arm and meets the route's arm as before;
//! - never through the tunnel: a request that came in through it misses
//!   the arm whatever it carries (as a chat pass is never minted there), so
//!   a passport there meets the route's own arm, which refuses it;
//! - [`serve_tcp`] strips any identity keys the request sent, keeps its own
//!   `project=` claim (`caller_workspace::keep_client_project`) and stamps
//!   the passport (`caller_workspace::stamp_principal`): `project`,
//!   `project_path`, `project_id` and `from` are the passport's;
//! - before the handler runs, [`refusal`] applies the route's [`Scope`] to
//!   the stamped params (in `cli::dispatch`); after it, [`narrow`] trims a
//!   list to what the passport may see.
//!
//! The cell socket is unchanged: these routes are not agent verbs
//! (`session_token::is_agent_verb`), except the profile, which already was.
//!
//! Naming another workspace never retargets a request: it is refused
//! (`own_workspace_only`), or, where the route names a workspace to look up,
//! the name must name exactly one workspace, its own or a connected one;
//! anything else is a plain not-found that lists no candidates.

use std::collections::HashMap;

use tokio::net::TcpStream;

use crate::caller_workspace::{CLIENT_PROJECT_KEY, PRINCIPAL_BOUND_KEY};
use crate::cli_response::CliResponse;
use crate::routes::http;
use crate::session_token::HookPrincipal;
use crate::workspace_msg::{resolve_workspace_detailed, WorkspaceResolve};

/// How a route is kept to the passport's workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The handler reads `project=` / `project_path=`. The stamp sets both
    /// to the passport's workspace; a claim naming any other is refused.
    Own,
    /// The handler looks up the workspace named by `q=`: it runs only when
    /// `q` names exactly one workspace, the passport's own or a connected
    /// one; anything else is a plain not-found ([`one_reachable_workspace`]).
    Named,
    /// A read with no workspace in it (release notes, the glossary, the
    /// preset and focus-group lists, the context catalog, the settings copy
    /// a Member login reads).
    Shared,
    /// `/cli/agent/list`: every workspace's row, narrowed to the passport's
    /// own and its connected workspaces ([`narrow`]).
    AgentList,
    /// [`Scope::Own`], and never the map of every workspace's wiki
    /// (`wiki_routes::is_fleet_scope`).
    WikiIndex,
    /// [`Scope::Own`], and never a note of another workspace or a node of
    /// the map of every workspace (the ids an App pass is refused too).
    WikiNote,
    /// A ticket by id: only one filed in the passport's own workspace.
    /// Any other answers as an unknown id does.
    Ticket,
}

/// One admitted route. `names` are params that name something inside the
/// workspace's own folders (an agent, a work item, a skill): a passport
/// must send a plain name there, never a path.
pub(crate) struct Route {
    pub path: &'static str,
    pub scope: Scope,
    pub names: &'static [&'static str],
}

const fn route(path: &'static str, scope: Scope) -> Route {
    Route { path, scope, names: &[] }
}

const fn named(path: &'static str, scope: Scope, names: &'static [&'static str]) -> Route {
    Route { path, scope, names }
}

/// Every route this module admits a passport on, sorted by path.
pub(crate) const ROUTES: &[Route] = &[
    // `k2 agent complete` (sub-agent template).
    named("/cli/agent/complete", Scope::Own, &["agent", "file"]),
    // `k2 agent conf|get|set|retire|restart` and `k2 agent hire` read it first.
    route("/cli/agent/conf", Scope::Named),
    route("/cli/agent/list", Scope::AgentList),
    // `k2 skills create|remove|list|profile`: the workspace's own skills.
    named("/cli/agents/create", Scope::Own, &["name"]),
    named("/cli/agents/delete", Scope::Own, &["name"]),
    route("/cli/agents/list", Scope::Own),
    named("/cli/agents/profile", Scope::Own, &["agent"]),
    // `k2 commit [--merge]` (manager skill); a merge is a person's
    // ([`merge_needs_person`]).
    route("/cli/commit", Scope::Own),
    route("/cli/commit-merge", Scope::Own),
    // `k2 agent context list|show|catalog`.
    route("/cli/context/catalog", Scope::Shared),
    route("/cli/context/layers", Scope::Own),
    route("/cli/context/show", Scope::Own),
    // `k2 done` (wakeup templates).
    // Its actor is the passport's own name ([`actor_is_the_passport`]).
    route("/cli/done", Scope::Own),
    // `k2 activity`.
    route("/cli/feed", Scope::Own),
    // `k2 tickets list|show`.
    route("/cli/feedback/list", Scope::Own),
    route("/cli/feedback/show", Scope::Ticket),
    route("/cli/focus-groups/list", Scope::Shared),
    route("/cli/glossary", Scope::Shared),
    route("/cli/glossary/get", Scope::Shared),
    route("/cli/glossary/list", Scope::Shared),
    route("/cli/onboarding/scan", Scope::Own),
    route("/cli/presets/list", Scope::Shared),
    // `k2 review approve|reject|feedback` and `k2 reviews` (manager skill);
    // approving its own branch is a person's ([`merge_needs_person`]).
    named("/cli/review/approve", Scope::Own, &["agent"]),
    named("/cli/review/feedback", Scope::Own, &["agent"]),
    named("/cli/review/reject", Scope::Own, &["agent"]),
    route("/cli/reviews", Scope::Own),
    route("/cli/settings", Scope::Own),
    // `k2 fed status`, `k2 api status`: the copy a Member login reads
    // (credentials taken out); the full one stays the owner token's.
    route("/cli/settings/get", Scope::Shared),
    route("/cli/skills/regenerate", Scope::Own),
    route("/cli/whats_new", Scope::Shared),
    // `k2 wiki index|note|status`.
    route("/cli/wiki/index", Scope::WikiIndex),
    route("/cli/wiki/note", Scope::WikiNote),
    route("/cli/wiki/status", Scope::Own),
    // `k2 workspace profile` (0.46.16; its handler also refuses another
    // workspace: `workspace_profile_route::other_workspace_refusal`).
    route("/cli/workspace/agent-display-name", Scope::Own),
    // Name resolution for `k2 msg`, `k2 workspace …` and friends.
    route("/cli/workspace/resolve", Scope::Named),
    route("/cli/workspace/resume-chat-args", Scope::Own),
];

/// The admitted route for `path`, if any.
pub(crate) fn lookup(path: &str) -> Option<&'static Route> {
    ROUTES.iter().find(|r| r.path == path)
}

/// Whether a passport may use `path` under this module's rules.
pub fn is_route(path: &str) -> bool {
    lookup(path).is_some()
}

/// The passport presented for `path` over TCP, when `path` is one of
/// [`ROUTES`] and the credential is a live passport of a session on this
/// computer ([`local_session`]). `None` for the owner token, a login, an App
/// pass, an API cell's or a sandboxed agent's passport or anything else:
/// those keep the route's existing arm, unchanged.
pub(crate) fn tcp_passport(
    path: &str,
    query: &str,
    bearer: Option<&str>,
    owner_token: &str,
) -> Option<crate::session_token::ValidatedHook> {
    // The owner token and a login keep their arm without a look at the
    // passport store, as `token_or_scoped_hook_auth` does.
    if !is_route(path) || http::token_ok(query, owner_token) {
        return None;
    }
    let presented = bearer
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| http::extract_token(query))
        .unwrap_or("");
    // A passport is `<sid>.<secret>`; nothing else is looked up.
    if !presented.split_once('.').is_some_and(|(sid, secret)| !sid.is_empty() && !secret.is_empty()) {
        return None;
    }
    crate::session_token::validate_hook(presented).filter(|v| local_session(&v.session_id, &v.principal))
}

/// A passport of a live session on this computer: its session is in the
/// session map now, isn't sandboxed (microVM), isn't an API cell or a
/// remote-session grant shell, and the passport isn't a `k2 job` run's. A
/// passport K2 can't place that way is refused, never assumed local.
fn local_session(session_id: &str, principal: &HookPrincipal) -> bool {
    let live = crate::v2_session_map::snapshot().into_iter().find(|(_, s)| s.session_id.to_string() == session_id);
    live.is_some_and(|(key, s)| {
        local_entry(&key, matches!(s.sandbox, k2_core::terminal::SandboxSpec::Microvm), &principal.agent_address)
    })
}

/// [`local_session`] for a live session's map key, its sandbox and the
/// passport's address (pure: tests).
fn local_entry(key: &str, sandboxed: bool, agent_address: &str) -> bool {
    use k2_core::workspace_session_handles::{is_api_agent_name, is_remote_shell_agent_name};
    !sandboxed
        && !is_api_agent_name(key)
        && !is_remote_shell_agent_name(key)
        && !agent_address.starts_with("api-")
        && !agent_address.starts_with("job:")
}

/// The TCP dispatcher's first arm, for a request [`tcp_passport`] admits
/// (no route's owner-token arm sees it): stamp the passport and run the
/// same dispatch the cell socket runs. A passport revoked since the arm was
/// chosen gets the plain 403.
pub(crate) async fn serve_tcp(
    stream: &mut TcpStream,
    buf: &mut [u8],
    is_post: bool,
    path: &str,
    query: &str,
    bearer: Option<&str>,
    owner_token: &str,
) {
    let mut params = http::read_request_params(stream, buf, is_post, path, query).await;
    let Some(principal) = tcp_passport(path, query, bearer, owner_token).map(|v| v.principal) else {
        let r = CliResponse::forbidden();
        http::send_response(stream, r.status, r.content_type, &r.body).await;
        return;
    };
    // Identity keys are the server's, never the request's.
    crate::caller_workspace::strip_request_stamps(&mut params);
    crate::caller_workspace::keep_client_project(&mut params);
    crate::caller_workspace::stamp_principal(&mut params, &principal);
    if let Some(r) = merge_needs_person(path, &params, &principal).or_else(|| actor_is_the_passport(path, &mut params, &principal)) {
        http::send_response(stream, r.status, r.content_type, &r.body).await;
        return;
    }
    let path = path.to_string();
    let r = crate::rt::blocking(|| {
        let r = crate::caller_workspace::with_request_principal(Some(principal.clone()), || crate::cli::dispatch(&path, &params));
        if r.status.starts_with("200") {
            record_reviewer(&path, &params, &principal);
        }
        r
    });
    http::send_response(stream, r.status, r.content_type, &r.body).await;
}

/// The refusal code of a merge a person makes for the agent; the
/// `git.review-approve` and `git.commit-merge` cards teach it.
pub(crate) const MERGE_NEEDS_PERSON: &str = "merge_needs_person";

/// A merge of the agent's own work is a person's (K2's ruling on D1):
/// `k2 commit --merge` always (who wrote what it merges can't be told), and
/// `k2 review approve` of a branch whose author is the passport itself, or
/// can't be told ([`own_branch`]). Approving another agent of the same
/// workspace stays the agent's. `Some(403 merge_needs_person)`, naming the
/// branch, which the permission card asks a person to approve.
fn merge_needs_person(path: &str, params: &HashMap<String, String>, principal: &HookPrincipal) -> Option<CliResponse> {
    let error = match path {
        "/cli/commit-merge" => serde_json::json!({
            "code": MERGE_NEEDS_PERSON,
            "hint": "an agent commits; merging its work is a person's step",
        }),
        "/cli/review/approve" if own_branch(params, principal) => serde_json::json!({
            "code": MERGE_NEEDS_PERSON,
            "hint": "an agent approves another agent's branch, never its own (or one whose author can't be told); a person approves this one",
            "agent": params.get("agent").map(|s| s.trim()).unwrap_or(""),
            "branch": params.get("branch").map(|s| s.trim()).unwrap_or(""),
        }),
        _ => return None,
    };
    Some(CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({ "error": error }).to_string(),
    })
}

/// Whether `branch=` is the passport's own work, or its author can't be
/// told. An agent's review branch is `agent/<author>/<task>`; it is another
/// agent's only when that author is a skill of this workspace, isn't one of
/// the passport's own names (its address, its workspace's handle), and
/// matches `agent=` when the request names one.
fn own_branch(params: &HashMap<String, String>, principal: &HookPrincipal) -> bool {
    let branch = params.get("branch").map(|s| s.trim()).unwrap_or("");
    let Some(author) = branch.strip_prefix("agent/").and_then(|rest| rest.split_once('/')).map(|(a, _)| a).filter(|a| !a.is_empty()) else {
        return true;
    };
    if params.get("agent").map(|s| s.trim()).is_some_and(|a| !a.is_empty() && a != author) {
        return true;
    }
    let mine = [principal.agent_address.as_str(), params.get("from").map(String::as_str).unwrap_or("")];
    if mine.iter().any(|n| !n.trim().is_empty() && n.trim().eq_ignore_ascii_case(author)) {
        return true;
    }
    let Some(project) = params.get("project").filter(|p| !p.is_empty()) else {
        return true;
    };
    let skill = k2_core::workspace::agent::list(project.clone()).is_ok_and(|l| l.iter().any(|a| a.name == author));
    !skill
}

/// `k2 done` records the passport's own name as its actor, never a name
/// the request gives: `agent=` absent takes the passport's, the same name
/// passes, any other is refused (`not_this_agent`), never replaced.
fn actor_is_the_passport(path: &str, params: &mut HashMap<String, String>, principal: &HookPrincipal) -> Option<CliResponse> {
    if path != "/cli/done" {
        return None;
    }
    let me = principal.agent_address.trim();
    match params.get("agent").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(named) if named != me => Some(CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: serde_json::json!({ "error": {
                "code": "not_this_agent",
                "hint": format!("this session is '{me}'; done records the session's own agent: send that name or none"),
                "agent": me,
            } })
            .to_string(),
        }),
        _ => {
            params.insert("agent".to_string(), me.to_string());
            None
        }
    }
}

/// A review the passport made (approve of another agent's branch, reject,
/// feedback) goes on its workspace's activity feed with the passport as
/// the reviewer; `agent=` there names whose work it reviewed.
fn record_reviewer(path: &str, params: &HashMap<String, String>, principal: &HookPrincipal) {
    let (event, verb) = match path {
        "/cli/review/approve" => ("review.approved", "approved"),
        "/cli/review/reject" => ("review.rejected", "rejected"),
        "/cli/review/feedback" => ("review.changes-requested", "asked for changes on"),
        _ => return,
    };
    let me = principal.agent_address.trim();
    let whose = params.get("agent").map(|s| s.trim()).unwrap_or("");
    let branch = params.get("branch").map(|s| s.trim()).filter(|s| !s.is_empty());
    let summary = match branch {
        Some(b) => format!("{me} {verb} {whose}'s branch {b}"),
        None => format!("{me} {verb} {whose}'s work"),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::log_activity(&conn, &principal.workspace_uuid, Some(me), event, Some(me), Some(whose), None, Some(&summary));
}

/// The passport's workspace (`projects.id`) when `params` were stamped from
/// a passport, else `None` (the owner, a login: nothing to refuse).
fn stamped_workspace(params: &HashMap<String, String>) -> Option<&str> {
    if params.get(PRINCIPAL_BOUND_KEY).map(String::as_str) != Some("1") {
        return None;
    }
    Some(params.get("project_id").map(|s| s.trim()).unwrap_or(""))
}

/// Before the handler: `Some(answer)` when a stamped passport request on
/// one of [`ROUTES`] reaches outside its workspace. `None` lets it run (and
/// always for the owner and logins).
pub(crate) fn refusal(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let route = lookup(path)?;
    let ws = stamped_workspace(params)?;
    if ws.is_empty() {
        return Some(own_only("this session's workspace is not known"));
    }
    for key in route.names {
        let v = params.get(*key).map(String::as_str).unwrap_or("");
        let ok = if *key == "file" { is_plain_name(v) && !matches!(v, "" | "." | "..") } else { is_agent_name(v) };
        if !ok {
            return Some(own_only(&format!("'{key}' must be a name inside this workspace (letters, digits, - and _), not a path")));
        }
    }
    if let Some(r) = own_agent_folder(path, params) {
        return Some(r);
    }
    match route.scope {
        Scope::Shared | Scope::AgentList => None,
        Scope::Own => other_workspace(params),
        Scope::WikiIndex => {
            let scope = params.get("scope").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default();
            if crate::wiki_routes::is_fleet_scope(&scope) {
                return Some(own_only("an agent reads its own workspace's wiki; the map of every workspace is a person's"));
            }
            other_workspace(params)
        }
        Scope::WikiNote => {
            let id = params.get("id").map(String::as_str).unwrap_or("");
            if crate::wiki_routes::wiki_id_outside_workspace(id) {
                return Some(own_only("an agent reads its own workspace's wiki notes"));
            }
            other_workspace(params)
        }
        Scope::Ticket => ticket_outside(params, ws),
        Scope::Named => {
            let q = params.get("q").map(|s| s.trim()).unwrap_or("");
            if q.is_empty() {
                return None;
            }
            if one_reachable_workspace(ws, q) {
                None
            } else {
                Some(not_found(q))
            }
        }
    }
}

/// Whether `q` names exactly one workspace, and that one is the passport's
/// own or one it is connected to. Anything else (a miss, a retired handle,
/// a name more than one workspace answers to, a workspace it isn't
/// connected to) is not, so the passport learns nothing about it.
fn one_reachable_workspace(ws: &str, q: &str) -> bool {
    // `sales/reviewer` is a sidecar address: the workspace is `sales`.
    let token = k2_core::workspace_session_handles::split_workspace_handle(q).map(|(w, _)| w).unwrap_or(q);
    let WorkspaceResolve::Found(path) = resolve_workspace_detailed(token) else {
        return false;
    };
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
    };
    id.is_some_and(|id| id == ws || k2_core::connections::are_local_peers(ws, &id))
}

/// The not-found a passport gets for a name it can't reach: no candidates,
/// no "did you mean".
fn not_found(q: &str) -> CliResponse {
    CliResponse {
        status: "404 Not Found",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": "not_found", "hint": format!("no workspace named '{q}' that this session can reach") },
        })
        .to_string(),
    }
}

/// After the handler: trim an answer to what the passport may see. Only
/// [`Scope::AgentList`] has anything to trim; everything else passes as is.
pub(crate) fn narrow(path: &str, params: &HashMap<String, String>, resp: CliResponse) -> CliResponse {
    let Some(route) = lookup(path) else { return resp };
    if route.scope != Scope::AgentList {
        return resp;
    }
    let Some(ws) = stamped_workspace(params) else { return resp };
    if !resp.status.starts_with("200") {
        return resp;
    }
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&resp.body) else {
        return CliResponse::internal_error("the agent list was not JSON");
    };
    let Some(agents) = v.get_mut("agents").and_then(|a| a.as_array_mut()) else {
        return CliResponse::internal_error("the agent list had no agents");
    };
    agents.retain(|a| {
        a.get("id")
            .and_then(|id| id.as_str())
            .is_some_and(|id| !ws.is_empty() && (id == ws || k2_core::connections::are_local_peers(ws, id)))
    });
    CliResponse::ok_json(v.to_string())
}

/// A stamped passport whose own `project=` claim (kept under
/// [`CLIENT_PROJECT_KEY`]) names a workspace other than its own: refused.
/// No claim, or a claim naming its own workspace: `None`.
/// The same rule as `workspace_profile_route::other_workspace_refusal`.
fn other_workspace(params: &HashMap<String, String>) -> Option<CliResponse> {
    let hint = "an agent works in its own workspace only; a person can do this for another";
    let found = |token: &str| match resolve_workspace_detailed(token) {
        WorkspaceResolve::Found(path) => Some(path),
        _ => None,
    };
    let Some(own) = params.get("project").map(|s| s.trim()).filter(|s| !s.is_empty()).and_then(found) else {
        return Some(own_only(hint));
    };
    match params.get(CLIENT_PROJECT_KEY).map(|s| s.trim()).filter(|s| !s.is_empty()) {
        None => None,
        Some(claim) if found(claim).as_deref() == Some(own.as_str()) => None,
        Some(_) => Some(own_only(hint)),
    }
}

/// `GET /cli/feedback/show`: `Some(not found)` unless `id` names one ticket
/// filed in `workspace_id`. Another workspace's ticket, or a prefix that
/// isn't one of its own, answers exactly as an unknown id does. A missing id
/// is the handler's.
fn ticket_outside(params: &HashMap<String, String>, workspace_id: &str) -> Option<CliResponse> {
    use k2_core::feedback::{get_item, resolve_id_prefix, PrefixError};
    let id = params.get("id").map(String::as_str).unwrap_or("");
    if id.is_empty() {
        return None;
    }
    let own = resolve_id_prefix(id).ok().and_then(|full| get_item(&full)).is_some_and(|i| i.project_id == workspace_id);
    (!own).then(|| crate::feedback_routes::prefix_error_response(id, PrefixError::NotFound))
}

/// An agent's or a skill's name, as `agents/create` itself requires
/// (`k2_core::workspace::agent::create`): letters, digits, `-` and `_`.
fn is_agent_name(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Creating, removing or reading a skill never reaches the workspace's own
/// agent folder (`.k2/agent/`): `agents/create` always writes there, and
/// `agents/delete` / `profile` resolve to it whenever the workspace has a
/// persona (`agent_identity::agent_dir`). A person does those.
fn own_agent_folder(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    use k2_core::workspace::agent_identity::{agent_dir, workspace_agent_path};
    let key = match path {
        "/cli/agents/create" | "/cli/agents/delete" => "name",
        "/cli/agents/profile" => "agent",
        _ => return None,
    };
    let project = params.get("project").map(String::as_str).unwrap_or("");
    let own = workspace_agent_path(project);
    let target = if path == "/cli/agents/create" { own.clone() } else { agent_dir(project, params.get(key).map(String::as_str).unwrap_or("")) };
    (target == own).then(|| own_only("this is the workspace's own agent folder; a person changes or reads it"))
}

/// A name, not a path: no separator, no `..`, no NUL.
fn is_plain_name(v: &str) -> bool {
    !(v.contains('/') || v.contains('\\') || v.contains("..") || v.contains('\0'))
}

fn own_only(hint: &str) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "error": { "code": "own_workspace_only", "hint": hint }
        })
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_are_sorted_unique_and_classified() {
        for w in ROUTES.windows(2) {
            assert!(w[0].path < w[1].path, "ROUTES must be sorted and unique: {} then {}", w[0].path, w[1].path);
        }
        for r in ROUTES {
            let row = crate::routes::route_policy::lookup(r.path)
                .unwrap_or_else(|| panic!("{} has no route policy row", r.path));
            // A write takes its params from the query or a form body, which
            // the TCP arm folds in (`read_request_params`): a formerly-GET
            // write, never a JSON-body route.
            if row.get.is_none() {
                assert!(
                    crate::routes::route_policy::FORMERLY_GET_WRITES.contains(&r.path),
                    "{} is POST-only and not a formerly-GET write",
                    r.path
                );
            }
        }
    }

    #[test]
    fn the_cell_socket_is_unchanged_and_owner_routes_stay_out() {
        // Not agent verbs: the cell socket answers these as before. The
        // profile was one before D1.
        for r in ROUTES {
            let before = r.path == "/cli/workspace/agent-display-name";
            assert_eq!(crate::session_token::is_agent_verb(r.path), before, "{}", r.path);
        }
        // The owner-only surface stays closed whatever this table says.
        for p in ["/cli/tunnel/status", "/cli/workspace/set", "/cli/feedback/create", "/cli/context/add", "/cli/projects/list"] {
            assert!(!is_route(p), "{p} must not be admitted here");
        }
    }

    #[test]
    fn only_a_live_local_session_is_admitted() {
        assert!(local_entry("tab-pane-1", false, "Acme"), "a K2 terminal or harness tab");
        assert!(local_entry("0f8a2c1e-0000-4000-8000-000000000000", false, "Acme"), "a pinned chat");
        assert!(!local_entry("tab-pane-1", true, "Acme"), "a microVM agent");
        assert!(!local_entry("api-key1-abc", false, "Acme"), "an API cell");
        assert!(!local_entry("remote-shell-g1-abc", false, "Acme"), "a remote-session grant shell");
        assert!(!local_entry("tab-pane-1", false, "api-key1"), "an API cell's passport");
        assert!(!local_entry("tab-pane-1", false, "job:nightly"), "a k2 job run's passport");
        assert!(!local_session("no-such-session", &HookPrincipal { workspace_uuid: "w".into(), agent_address: "Acme".into() }), "no live session");
    }

    #[test]
    fn agent_names_are_create_rule_names() {
        for ok in ["backend", "agent-1", "a_b", "X9"] {
            assert!(is_agent_name(ok), "{ok}");
        }
        for bad in ["", ".", "..", "a/b", "a b", "a.b", "../x", "x\0"] {
            assert!(!is_agent_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn plain_names_refuse_paths() {
        for ok in ["backend", "agent-1", "task_2.md", "a.b"] {
            assert!(is_plain_name(ok), "{ok}");
        }
        for bad in ["../x", "a/b", "a\\b", "..", "x\0"] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn nothing_is_refused_without_a_stamped_passport() {
        let mut p = HashMap::new();
        p.insert("project".to_string(), "/somewhere/else".to_string());
        p.insert("agent".to_string(), "../../x".to_string());
        assert!(refusal("/cli/agent/conf", &p).is_none(), "the owner's request runs as before");
        p.insert(PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());
        p.insert("project_id".to_string(), String::new());
        let r = refusal("/cli/agent/conf", &p).expect("a passport with no workspace is refused");
        assert!(r.status.starts_with("403"), "{}", r.status);
    }
}
