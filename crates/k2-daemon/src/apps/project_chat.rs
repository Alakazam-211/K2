//! Project chat as an App surface (0.46.3, Rosson 2026-10-20; parity
//! research G12 / group H). **App only**: no Garden bridge rows for
//! projects (Rosson: no Projects on the bridge for now).
//!
//! ```text
//! App transport (a k2skn_ pass; catalog rows, generated app_surface.rs)
//!   GET  /cli/skin/projects                     project.list      thread:read (per member room)
//!   GET  /cli/skin/projects/messages ?group=&after_seq=&before_seq=&limit=
//!                                               project.read      thread:read (per member room)
//!   POST /cli/skin/projects/post  {group, body} project.post      project:write (per member room)
//!   POST /cli/skin/projects/read  {group, seq?} project.markRead  thread:read; a person only
//!   WS   /cli/skin/projects/events ?group=      project.events    thread:read (per member room)
//! ```
//!
//! **Who is a member (the all-members rule).** A project is a named set of
//! agent workspaces; for an App user, a room is an agent. A pass sees a
//! project's chat only when the project has at least one member and the
//! pass holds `thread:read` in **every** member room. One member it can't
//! reach and the project doesn't exist for it: it isn't listed, and every
//! verb answers the same 403 `skin_room` (same bytes) as a project that
//! doesn't exist (R3: no counts, no names). Membership is read on every
//! request and every live frame, so adding an unreachable agent hides the
//! chat at once. An explicit per-project grant (a project the pass may see
//! without every member room) is not built: nothing stores one today.
//!
//! **Posting** needs `project:write`, a new per-room cap (owner/role
//! granted, never a default, in no App core: person-held until a Projects
//! core exists), in every member room as well. It is its own cap and not
//! `thread:post`: a project line reaches the point-of-contact agent AND
//! every other member's people, so a guest who may talk to each agent
//! one-to-one hasn't been allowed to speak in the shared room.
//!
//! **Author.** A guest's line is stored as `guest:<username>` with
//! `authorKind: "guest"`, so no client that reads only `author` can take it
//! for `owner`, a Connect login or an agent. It wakes the point of contact
//! like any line (`[from guest:<username>]`) and pushes to every person.
//!
//! **Read state** is per person and shared across surfaces (Rosson Q11):
//! a guest is the person `guest:<principalId>` (`read_routes::PassReader`,
//! the one key its Thread read state uses too), on every App, door, device
//! and session. An App's own pass (`app:<serviceId>`, a bookmark, not a
//! person) and a platform pass get no project read state here:
//! `unread`/`muted`/`lastReadSeq` are null and `project.markRead` is 403
//! `not_exposed`. Project rows add no totals: a total would mix in
//! conversations the Thread badge already counts over the pass's rooms.
//! A guest's cursor moving (a mark-read or its own post, on any surface)
//! goes out as `read_live::guest_moved(key, groupId)`; its project sockets
//! send `{"collection":"read", id, groupId, lastReadSeq, lastSeq, unread,
//! waiting, muted}` ([`read_frame_wire`]), the overlay socket's read-frame
//! shape. Overlay and activity sockets ignore it (a group id is never
//! their pinned Chat).
//!
//! **Frames.** Every stored project line (any author, any door) goes out on
//! this module's bus ([`publish`], from `project_group_routes::
//! store_and_deliver`). A frame is shaped like a Thread frame plus the
//! project id: `{collection:"project", seq, id, groupId, doc}` (`doc` = the
//! message as `project.read` returns it). [`subscribe`], [`frame_wire`] and
//! [`can_read`] are the pieces an App-wide stream includes project frames
//! with; [`serve_events_connection`] is the per-project socket.

use std::collections::HashMap;
use std::sync::OnceLock;

use futures_util::{SinkExt, StreamExt};
use k2_core::project_groups::{self, ProjectGroup, ProjectGroupMessage};
use k2_core::read_state::{self, Person, KIND_PROJECT};
use k2_core::skin::{SkinPass, CAP_PROJECT_WRITE, CAP_THREAD_READ};
use serde_json::{json, Value as J};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

use crate::cli_response::CliResponse;

/// `project.list`.
pub const LIST_ROUTE: &str = "/cli/skin/projects";
/// `project.read`.
pub const MESSAGES_ROUTE: &str = "/cli/skin/projects/messages";
/// `project.post`.
pub const POST_ROUTE: &str = "/cli/skin/projects/post";
/// `project.markRead`.
pub const READ_ROUTE: &str = "/cli/skin/projects/read";
/// `project.events` (a socket).
pub const EVENTS_WS_PATH: &str = "/cli/skin/projects/events";
/// The HTTP routes (the dispatcher's skin arm).
pub const PASS_ROUTES: &[&str] = &[LIST_ROUTE, MESSAGES_ROUTE, POST_ROUTE, READ_ROUTE];
/// POST-only among [`PASS_ROUTES`].
pub const POST_ROUTES: &[&str] = &[POST_ROUTE, READ_ROUTE];
/// The `/boot-status` key.
pub const FEATURE: &str = "app-project-chat-v1";
/// The frame collection.
pub const COLLECTION: &str = "project";
/// The longest line a guest may post (bytes).
pub const MAX_BODY_BYTES: usize = 32 * 1024;

const BUS_CAP: usize = 1024;

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

// ── who sees what ────────────────────────────────────────────────────

fn holds_in_every(pass: &SkinPass, rooms: &[String], cap: &str) -> bool {
    !rooms.is_empty() && rooms.iter().all(|r| pass.has_cap_in_room(r, cap))
}

/// The member rooms (workspace ids) of a project, oldest-joined first.
fn member_rooms(group: &ProjectGroup) -> Result<Vec<String>, String> {
    match &group.member_workspace_ids {
        Some(ids) => Ok(ids.clone()),
        None => Ok(project_groups::list_members(&group.id)?.into_iter().map(|m| m.workspace_id).collect()),
    }
}

/// May `pass` read project `group_id`'s chat right now (the all-members
/// rule)? Fails closed: an unknown project or a failed read is `false`.
pub fn can_read(pass: &SkinPass, group_id: &str) -> bool {
    match project_groups::list_members(group_id) {
        Ok(m) => {
            let rooms: Vec<String> = m.into_iter().map(|m| m.workspace_id).collect();
            holds_in_every(pass, &rooms, CAP_THREAD_READ)
        }
        Err(_) => false,
    }
}

/// The projects `pass` may read, in the nav order.
pub fn visible_groups(pass: &SkinPass) -> Result<Vec<(ProjectGroup, Vec<String>)>, String> {
    let mut out = Vec::new();
    for g in project_groups::list_groups()? {
        let rooms = member_rooms(&g)?;
        if holds_in_every(pass, &rooms, CAP_THREAD_READ) {
            out.push((g, rooms));
        }
    }
    Ok(out)
}

/// The project a selector names among the ones `pass` may read: its exact
/// id, else its exact name (case-insensitive). `None` for an unknown and
/// for an invisible project alike (no prefixes: never an oracle).
fn visible_group(pass: &SkinPass, selector: &str) -> Result<Option<(ProjectGroup, Vec<String>)>, String> {
    let sel = selector.trim();
    let groups = visible_groups(pass)?;
    if let Some(i) = groups.iter().position(|(g, _)| g.id == sel) {
        return Ok(Some(groups.into_iter().nth(i).expect("index from position")));
    }
    Ok(groups.into_iter().find(|(g, _)| g.name.eq_ignore_ascii_case(sel)))
}

/// The read-state person of a pass: an App guest (`guest:<principalId>`,
/// `read_routes::PassReader`), else none (an App's own pass is a bookmark,
/// not a person, and gets no project read state).
pub fn person_of(pass: &SkinPass) -> Option<Person> {
    let reader = crate::read_routes::pass_reader(pass);
    reader.person.then(|| Person::app_guest(&reader.key, &pass.username))
}

/// The `read` frame on a project socket: the overlay socket's read-frame
/// shape (`collection: "read"`) for one project, no totals.
pub fn read_frame_wire(it: &read_state::ReadItem) -> String {
    json!({
        "collection": "read",
        "id": it.id,
        "groupId": it.id,
        "lastReadSeq": it.last_read_seq,
        "lastSeq": it.last_seq,
        "unread": it.unread,
        "waiting": it.waiting,
        "muted": it.muted,
    })
    .to_string()
}

/// The guest's current project state (`None`: not a person, or unreadable).
fn read_item(pass: &SkinPass, group_id: &str) -> Option<read_state::ReadItem> {
    let person = person_of(pass)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    if !read_state::tables_exist(&conn) {
        return None;
    }
    read_state::item(&conn, &person, KIND_PROJECT, group_id, read_state::now_secs()).ok()
}

// ── refusals ─────────────────────────────────────────────────────────

fn err(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

fn usage(hint: &str) -> CliResponse {
    err("400 Bad Request", "usage", hint)
}

/// An unknown project and one the pass can't read: the uniform room
/// refusal, byte for byte (R3).
pub fn hidden() -> CliResponse {
    crate::skin_routes::skin_room_response()
}

fn cap_not_granted(cap: &str) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "error": { "code": "cap_not_granted", "cap": cap, "hint": format!("this pass needs {cap} in every room of the project") },
        })
        .to_string(),
    }
}

/// A non-pass credential on a project chat App route.
pub fn forbidden(path: &str) -> CliResponse {
    err("403 Forbidden", "skin_token", &format!("{path} is for a live app or guest pass (k2skn_)"))
}

// ── views ────────────────────────────────────────────────────────────

fn list_row(pass: &SkinPass, person: Option<&Person>, g: &ProjectGroup, rooms: &[String], conn: &rusqlite::Connection) -> Result<J, String> {
    let members: Vec<J> = rooms.iter().map(|r| json!({ "room": r, "handle": super::routes::ws_handle(r) })).collect();
    let poc = g.poc_workspace_id.as_deref().map(super::routes::ws_handle);
    let now = read_state::now_secs();
    let (last_seq, unread, muted, last_read) = match person {
        Some(p) if read_state::tables_exist(conn) => {
            let it = read_state::item(conn, p, KIND_PROJECT, &g.id, now).map_err(|e| e.to_string())?;
            (it.last_seq, json!(it.unread), json!(it.muted), json!(it.last_read_seq))
        }
        _ => {
            let head = read_state::head(conn, KIND_PROJECT, &g.id).map_err(|e| e.to_string())?.unwrap_or(0);
            (head, J::Null, J::Null, J::Null)
        }
    };
    Ok(json!({
        "groupId": g.id,
        "name": g.name,
        "color": g.color,
        "poc": poc,
        "members": members,
        "lastSeq": last_seq,
        "canPost": holds_in_every(pass, rooms, CAP_PROJECT_WRITE),
        "unread": unread,
        "muted": muted,
        "lastReadSeq": last_read,
    }))
}

/// `project.list` for a pass.
pub fn list_view(pass: &SkinPass) -> Result<J, String> {
    let groups = visible_groups(pass)?;
    let person = person_of(pass);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut rows = Vec::with_capacity(groups.len());
    for (g, rooms) in &groups {
        rows.push(list_row(pass, person.as_ref(), g, rooms, &conn)?);
    }
    Ok(json!({ "ok": true, "projects": rows }))
}

// ── the App transport ────────────────────────────────────────────────

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PostBody {
    #[serde(default)]
    group: String,
    #[serde(default)]
    body: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadBody {
    #[serde(default)]
    group: String,
    #[serde(default)]
    seq: Option<i64>,
}

/// `/cli/skin/projects*` for a resolved pass (blocking). The order is
/// fixed so a refusal says nothing about a project the pass can't read:
/// a missing `group` is 400 for everyone, then an unknown or invisible
/// project is the uniform 403 `skin_room`, and only then are the other
/// params and the cap checked.
pub fn handle_pass(path: &str, pass: &SkinPass, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    match path {
        LIST_ROUTE => match list_view(pass) {
            Ok(v) => CliResponse::ok_json(v.to_string()),
            Err(e) => CliResponse::internal_error(e),
        },
        MESSAGES_ROUTE => {
            let sel = params.get("group").map(|s| s.trim()).unwrap_or("");
            if sel.is_empty() {
                return usage("name the project: group=<groupId from project.list>");
            }
            match visible_group(pass, sel) {
                Ok(Some((g, _))) => {
                    // 0.46.5 (item 14): an agent line stored with its
                    // folder as `author` reads as the workspace's name.
                    let mut r = crate::project_group_routes::messages_read(&g.id, params);
                    if r.status.starts_with("200") {
                        if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&r.body) {
                            crate::guest_paths::project_authors(&mut v);
                            r.body = v.to_string();
                        }
                    }
                    r
                }
                Ok(None) => hidden(),
                Err(e) => CliResponse::internal_error(e),
            }
        }
        POST_ROUTE => post(pass, body),
        READ_ROUTE => mark_read(pass, body),
        _ => CliResponse::not_found(),
    }
}

fn post(pass: &SkinPass, body: &[u8]) -> CliResponse {
    let b: PostBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage(&format!("body is {{group, body}}: {e}")),
    };
    if b.group.trim().is_empty() {
        return usage("name the project: group");
    }
    let (g, rooms) = match visible_group(pass, &b.group) {
        Ok(Some(v)) => v,
        Ok(None) => return hidden(),
        Err(e) => return CliResponse::internal_error(e),
    };
    if !holds_in_every(pass, &rooms, CAP_PROJECT_WRITE) {
        return cap_not_granted(CAP_PROJECT_WRITE);
    }
    let text = b.body.trim();
    if text.is_empty() {
        return usage("body is empty");
    }
    if text.len() > MAX_BODY_BYTES {
        return usage(&format!("body is over {MAX_BODY_BYTES} bytes"));
    }
    let author = project_groups::guest_author(&pass.username);
    let posted = match crate::project_group_routes::store_and_deliver(
        &g,
        &g.id,
        &author,
        project_groups::AUTHOR_KIND_GUEST,
        None,
        None,
        text,
    ) {
        Ok(p) => p,
        Err(r) => return r,
    };
    // The guest's own line is read for it (Sew: your own post advances
    // your own cursor). No badge push: guests have no K2 push yet.
    if let Some(person) = person_of(pass) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if read_state::tables_exist(&conn) {
            match read_state::ack(&conn, &person.key, KIND_PROJECT, &g.id, Some(posted.msg.seq)) {
                Ok(true) => crate::read_live::guest_moved(&person.key, &g.id),
                Ok(false) => {}
                Err(e) => k2_core::log_debug!("[apps/project_chat] own-post read for {}: {e}", person.key),
            }
        }
    }
    let m = &posted.msg;
    // `deliveredSessionId` is left out: a guest never sees a session id.
    CliResponse::ok_json(
        json!({
            "ok": true,
            "id": m.id,
            "groupId": g.id,
            "groupName": g.name,
            "author": m.author,
            "authorKind": m.author_kind,
            "body": m.body,
            "createdAt": m.created_at,
            "seq": m.seq,
            "delivered": posted.delivered,
            "deliveryReason": posted.delivery_reason,
        })
        .to_string(),
    )
}

fn mark_read(pass: &SkinPass, body: &[u8]) -> CliResponse {
    let b: ReadBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage(&format!("body is {{group, seq?}}: {e}")),
    };
    if b.group.trim().is_empty() {
        return usage("name the project: group");
    }
    let g = match visible_group(pass, &b.group) {
        Ok(Some((g, _))) => g,
        Ok(None) => return hidden(),
        Err(e) => return CliResponse::internal_error(e),
    };
    let Some(person) = person_of(pass) else {
        return err(
            "403 Forbidden",
            "not_exposed",
            "read state belongs to a person: a guest's login, not an App's own pass or a platform pass",
        );
    };
    if b.seq.is_some_and(|s| s < 0) {
        return usage("seq is a whole number >= 0");
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if !read_state::tables_exist(&conn) {
        return CliResponse::internal_error("read state is not set up on this server yet");
    }
    let changed = match read_state::ack(&conn, &person.key, KIND_PROJECT, &g.id, b.seq) {
        Ok(c) => c,
        Err(e) => return CliResponse::internal_error(e.to_string()),
    };
    if changed {
        // Every project socket of this person, on any surface, hears it.
        crate::read_live::guest_moved(&person.key, &g.id);
    }
    match read_state::item(&conn, &person, KIND_PROJECT, &g.id, read_state::now_secs()) {
        Ok(it) => CliResponse::ok_json(
            json!({
                "ok": true,
                "groupId": g.id,
                "changed": changed,
                "unread": it.unread,
                "muted": it.muted,
                "lastReadSeq": it.last_read_seq,
                "lastSeq": it.last_seq,
            })
            .to_string(),
        ),
        Err(e) => CliResponse::internal_error(e.to_string()),
    }
}

// ── live frames ──────────────────────────────────────────────────────

fn bus() -> &'static broadcast::Sender<ProjectGroupMessage> {
    static TX: OnceLock<broadcast::Sender<ProjectGroupMessage>> = OnceLock::new();
    TX.get_or_init(|| broadcast::channel(BUS_CAP).0)
}

/// A stored project line goes out to every project subscriber (called by
/// `project_group_routes::store_and_deliver` for every author).
pub fn publish(msg: &ProjectGroupMessage) {
    let _ = bus().send(msg.clone());
}

/// Every stored project line from now on. Filter with [`can_read`] per
/// pass before sending one on.
pub fn subscribe() -> broadcast::Receiver<ProjectGroupMessage> {
    bus().subscribe()
}

/// The wire frame of one line: a Thread frame's shape plus `groupId`.
pub fn frame_wire(msg: &ProjectGroupMessage) -> String {
    let mut v = json!({
        "collection": COLLECTION,
        "seq": msg.seq,
        "id": msg.id,
        "groupId": msg.group_id,
        "doc": msg,
    });
    // 0.46.5 (item 14): App sockets never carry a host path as `author`.
    crate::guest_paths::project_authors(&mut v["doc"]);
    v.to_string()
}

async fn refuse_http(stream: &mut TcpStream, r: CliResponse) {
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{}",
        r.status,
        r.content_type,
        r.body.len(),
        k2_core::frame_policy::CONNECT_HEADER_LINES,
        r.body
    );
    crate::routes::http::refuse_upgrade(stream, &resp).await;
}

/// `WS /cli/skin/projects/events?group=` for a resolved pass. The project
/// is checked before the upgrade (400 without `group`, the uniform 403
/// `skin_room` for an unknown or invisible one). After that, each frame
/// re-resolves the pass from `token` and re-reads the members, so a
/// revoked pass, a role edit or a new unreachable member closes the
/// socket instead of sending another line. A subscriber that lags the bus
/// is closed (catch up with `project.read` afterSeq), never skipped.
pub async fn serve_events_connection(stream: &mut TcpStream, params: HashMap<String, String>, pass: SkinPass, token: String) {
    let sel = params.get("group").map(|s| s.trim().to_string()).unwrap_or_default();
    if sel.is_empty() {
        refuse_http(stream, usage("name the project: group=<groupId from project.list>")).await;
        return;
    }
    let group_id = match visible_group(&pass, &sel) {
        Ok(Some((g, _))) => g.id,
        Ok(None) => {
            refuse_http(stream, hidden()).await;
            return;
        }
        Err(e) => {
            refuse_http(stream, CliResponse::internal_error(e)).await;
            return;
        }
    };
    let mut rx = subscribe();
    // The guest's own cursor moving in this project, on any surface.
    let read_key = person_of(&pass).map(|p| p.key);
    let mut read_rx = crate::read_live::subscribe_guest();
    let ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            k2_core::log_debug!("[apps/project_chat] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    let mut keepalive = tokio::time::interval(crate::overlay_ws::ping_interval());
    keepalive.tick().await;
    loop {
        tokio::select! {
            biased;
            _ = keepalive.tick() => {
                if write.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            incoming = read.next() => match incoming {
                Some(Ok(Message::Ping(p))) => {
                    if write.send(Message::Pong(p)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            moved = crate::read_live::next_guest_move(&mut read_rx, read_key.as_deref()) => {
                if moved.conversation_id != group_id {
                    continue;
                }
                // Resolve here: a link's context is task-local.
                let Some(fresh) = crate::apps::gateway::resolve_pass(&token) else {
                    let _ = write.send(Message::Close(None)).await;
                    break;
                };
                let gid = group_id.clone();
                let frame = tokio::task::spawn_blocking(move || {
                    if !can_read(&fresh, &gid) {
                        return None;
                    }
                    read_item(&fresh, &gid).map(|it| read_frame_wire(&it))
                })
                .await
                .ok()
                .flatten();
                if let Some(text) = frame {
                    if write.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
            }
            event = rx.recv() => match event {
                Ok(msg) => {
                    if msg.group_id != group_id {
                        continue;
                    }
                    let still = crate::apps::gateway::resolve_pass(&token).is_some_and(|p| can_read(&p, &group_id));
                    if !still {
                        let _ = write.send(Message::Close(None)).await;
                        break;
                    }
                    if write.send(Message::Text(frame_wire(&msg))).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    k2_core::log_debug!("[apps/project_chat] subscriber lagged {n} frames; closing for catch-up");
                    let _ = write.send(Message::Close(None)).await;
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}
