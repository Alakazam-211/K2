//! thread-read-v1 routes (0.46.1): a PERSON's unread counts, read
//! cursors and mutes for Threads and project chats. The state and rules
//! live in [`k2_core::read_state`]; this module is the HTTP face.
//!
//! - `GET  /cli/thread/unread[?kind=thread|project]` →
//!   `{ok, person, badgeKey, total, waitingTotal, byConversation:[State…], muted:[{kind,id,mutedUntil?}…]}`
//!   (`byConversation` lists conversations with something unread, a card
//!   waiting, or a mute; `kind` filters the lists, never the totals).
//! - `POST /cli/thread/read {conversationId | addr, seq?}` and
//!   `POST /cli/project-group/read {group, seq?}` → `{ok, changed, …State, total, waitingTotal}`.
//!   Forward only; `seq` absent = the newest item now.
//! - `POST /cli/thread/mute {kind?, id | conversationId | addr | group, muted, until?}`
//!   → the same shape.
//!
//! `State` = `{kind, id, projectId?, seq, lastReadSeq, lastSeq, unread,
//! waiting, muted, mutedUntil?}` (`seq` = `lastReadSeq`).
//!
//! Who: the dispatcher resolves the credential to a LOGIN (owner token →
//! `owner`, a live Connect session → its username) and refuses anything
//! else with 403 (agent passports, API keys): agents have no read state.
//! Each login sees and changes only its own person's state.
//! A GET on a POST route is 405 (house rule), checked before auth.
//!
//! **App guests and App passes** (App parity, 0.46.3, Rosson 2026-10-20;
//! [`handle_guest`]): `POST /cli/thread/read`, `GET /cli/thread/unread`
//! and (a guest only) `POST /cli/thread/mute`, on their own cursors, for
//! the pinned Chats of rooms where they hold `thread:read` (R3: counts and
//! rows over those rooms only). The key scheme is [`PassReader`]: an App
//! guest is the person `guest:<principalId>` on every App, door, device
//! and session (with a `badgeKey`, mutes and live `read` frames on its
//! overlay and activity sockets, app-read-state-v1); an App's own pass
//! is a service account (`app:<serviceId>`), not a person. Project chat
//! stays people-only.
//!
//! **The Garden bridge** (garden-person-rows-v1): [`garden_unread`] and
//! [`garden_thread_mute`] answer `thread.unread` / `thread.mute` for the
//! person on this computer in the App's shape, Threads only.

use std::cell::RefCell;
use std::collections::HashMap;

use k2_core::notify_prefs::People;
use k2_core::read_state::{self, Person, ReadError, ReadItem, Totals, KIND_PROJECT, KIND_THREAD};

use crate::cli_response::CliResponse;

pub const UNREAD_PATH: &str = "/cli/thread/unread";
pub const READ_PATH: &str = "/cli/thread/read";
pub const MUTE_PATH: &str = "/cli/thread/mute";
pub const PROJECT_READ_PATH: &str = "/cli/project-group/read";

/// One of the thread-read-v1 routes.
pub fn is_read_route(path: &str) -> bool {
    matches!(path, UNREAD_PATH | READ_PATH | MUTE_PATH | PROJECT_READ_PATH)
}

/// The POST-only ones.
pub fn is_post_only(path: &str) -> bool {
    matches!(path, READ_PATH | MUTE_PATH | PROJECT_READ_PATH)
}

thread_local! {
    static READ_LOGIN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Run `f` with the caller's person login bound (the dispatcher's arms
/// for `thread/latest` and the project list), so those reads can add the
/// caller's own `unread` / `waiting` / `muted`. Never a request param: a
/// param could be forged; this binding is set only from the credential.
pub fn with_read_login<T>(login: Option<String>, f: impl FnOnce() -> T) -> T {
    READ_LOGIN.with(|slot| {
        let prev = slot.replace(login);
        let out = f();
        slot.replace(prev);
        out
    })
}

/// The person login bound by [`with_read_login`], if any.
pub fn request_login() -> Option<String> {
    READ_LOGIN.with(|slot| slot.borrow().clone())
}

/// The person a login is.
pub fn person_of_login(login: &str) -> Result<Person, String> {
    Ok(Person::of_login(&People::load()?, login))
}

/// The person key of a person post's author (`owner` kind → the owner;
/// `user` kind → that login's person). `None` for an app guest, an
/// imported line or an agent post.
pub fn person_key_of_author(author: &k2_core::overlay::PostAuthor) -> Option<String> {
    use k2_core::overlay::doc::{AUTHOR_OWNER, AUTHOR_USER};
    let login = match author.kind.as_str() {
        AUTHOR_OWNER => "owner",
        AUTHOR_USER => author.login.as_str(),
        _ => return None,
    };
    match People::load() {
        Ok(people) => Some(people.person_key(login)),
        Err(e) => {
            k2_core::log_debug!("[read] people load failed: {e}");
            None
        }
    }
}

/// A person wrote (or answered) up to `seq` in a Thread: they have read
/// everything up to there (Sew: your own post advances your own cursor).
/// Their phones get a corrected badge. Runs under the caller's DB lock.
pub fn note_own_thread_read(conn: &rusqlite::Connection, author: Option<&k2_core::overlay::PostAuthor>, conversation_id: &str, seq: i64) {
    let Some(person) = author.and_then(person_key_of_author) else { return };
    note_own_read(conn, &person, KIND_THREAD, conversation_id, seq);
}

/// [`note_own_thread_read`] for any kind, with the person key known.
pub fn note_own_read(conn: &rusqlite::Connection, person: &str, kind: &str, id: &str, seq: i64) {
    if !read_state::tables_exist(conn) {
        return;
    }
    match read_state::ack(conn, person, kind, id, Some(seq)) {
        Ok(true) => {
            crate::read_live::person_changed(person, kind, id);
            crate::badge_push::trigger(person);
        }
        Ok(false) => {}
        Err(e) => k2_core::log_debug!("[read] own-post advance for {person} in {kind} {id}: {e}"),
    }
}

fn error_json(status: &'static str, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint.to_string() } }).to_string(),
    }
}

fn usage(hint: impl std::fmt::Display) -> CliResponse {
    error_json("400 Bad Request", "usage", hint)
}

fn read_error(e: ReadError) -> CliResponse {
    match e {
        ReadError::NotFound(h) => error_json("404 Not Found", "not_found", h),
        ReadError::Usage(h) => usage(h),
        ReadError::Store(h) => error_json("500 Internal Server Error", "store", h),
    }
}

/// 403 for a credential that is not a person.
pub fn not_a_person() -> CliResponse {
    error_json(
        "403 Forbidden",
        "forbidden",
        "read state belongs to a person: use the owner token or a K2 login (agents and app passes have none)",
    )
}

/// One conversation's state, the wire shape every route and frame shares.
pub fn state_json(item: &ReadItem) -> serde_json::Value {
    let mut v = serde_json::json!({
        "kind": item.kind,
        "id": item.id,
        "seq": item.last_read_seq,
        "lastReadSeq": item.last_read_seq,
        "lastSeq": item.last_seq,
        "unread": item.unread,
        "waiting": item.waiting,
        "muted": item.muted,
    });
    if let Some(p) = &item.project_id {
        v["projectId"] = serde_json::json!(p);
    }
    if let Some(u) = item.muted_until {
        v["mutedUntil"] = serde_json::json!(u);
    }
    v
}

fn with_totals(mut v: serde_json::Value, t: Totals) -> serde_json::Value {
    v["total"] = serde_json::json!(t.total);
    v["waitingTotal"] = serde_json::json!(t.waiting_total);
    v
}

/// The request's fields: a JSON object body, or (no body) the query.
struct Fields(serde_json::Map<String, serde_json::Value>);

impl Fields {
    fn parse(params: &HashMap<String, String>, body: &[u8]) -> Result<Self, CliResponse> {
        let trimmed = std::str::from_utf8(body).map(str::trim).unwrap_or("");
        if !trimmed.is_empty() {
            return match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(serde_json::Value::Object(m)) => Ok(Self(m)),
                Ok(_) => Err(usage("body must be a JSON object")),
                Err(e) => Err(usage(format!("invalid JSON body: {e}"))),
            };
        }
        let mut m = serde_json::Map::new();
        for (k, v) in params {
            if k != "token" {
                m.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
        }
        Ok(Self(m))
    }

    fn str(&self, keys: &[&str]) -> Option<String> {
        keys.iter().find_map(|k| match self.0.get(*k) {
            Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => None,
        })
    }

    /// An integer field (a JSON number or a decimal string). `Err` when it
    /// is present but not an integer.
    fn int(&self, key: &str) -> Result<Option<i64>, CliResponse> {
        match self.0.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::Number(n)) => n.as_i64().map(Some).ok_or_else(|| usage(format!("{key} must be an integer"))),
            Some(serde_json::Value::String(s)) if s.trim().is_empty() => Ok(None),
            Some(serde_json::Value::String(s)) => {
                s.trim().parse::<i64>().map(Some).map_err(|_| usage(format!("{key} must be an integer, got '{s}'")))
            }
            Some(_) => Err(usage(format!("{key} must be an integer"))),
        }
    }

    fn bool(&self, key: &str) -> Result<Option<bool>, CliResponse> {
        match self.0.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::Bool(b)) => Ok(Some(*b)),
            Some(serde_json::Value::String(s)) => match s.trim() {
                "true" | "1" => Ok(Some(true)),
                "false" | "0" => Ok(Some(false)),
                other => Err(usage(format!("{key} must be true or false, got '{other}'"))),
            },
            Some(_) => Err(usage(format!("{key} must be true or false"))),
        }
    }
}

/// A Thread named by `conversationId` / `id`, or by `addr`.
fn thread_id(f: &Fields) -> Result<String, CliResponse> {
    if let Some(id) = f.str(&["conversationId", "conversation_id", "id"]) {
        return Ok(id);
    }
    match f.str(&["addr"]) {
        Some(addr) => crate::overlay_routes::conversation_for_addr(&addr)
            .ok_or_else(|| error_json("404 Not Found", "not_found", format!("unknown Thread address '{addr}'"))),
        None => Err(usage("missing conversationId (or addr)")),
    }
}

/// A project chat named by `group` / `id` (a name, id or unique prefix).
fn group_id(f: &Fields) -> Result<String, CliResponse> {
    let Some(sel) = f.str(&["group", "groupId", "id"]) else {
        return Err(usage("missing group (a project name, id, or unique prefix)"));
    };
    k2_core::project_groups::resolve_group(&sel).map_err(|e| match e {
        k2_core::project_groups::ResolveError::NotFound => {
            error_json("404 Not Found", "not_found", format!("no project matches '{sel}'"))
        }
        k2_core::project_groups::ResolveError::Ambiguous(names) => {
            usage(format!("'{sel}' matches several projects: {}", names.join(", ")))
        }
    })
}

// ── App guests and App passes (0.46.3) ───────────────────────────────

/// Read-state key prefix of an App guest: a PERSON (`guest:<principalId>`).
pub const GUEST_KEY_PREFIX: &str = "guest:";
/// Read-state key prefix of an App's own pass: a service account, not a
/// person (`app:<serviceId>`).
pub const APP_KEY_PREFIX: &str = "app:";

/// Who reads on an App pass: the read-state key scheme (app-read-state-v1,
/// 0.46.3; Rosson 2026-10-20: "read state belongs to the signed-in
/// person", shared across desktop, Companion, Garden and custom Apps).
///
/// - **An App guest is a person**: `guest:<principalId>`. One key for
///   that person whatever surface they read on: any App they sign in to
///   (the box login or a published App), any door (the App's helper, the
///   daemon's main port, the App's own link with the guest's session,
///   AR50), any device, any session. Principal ids are v4 uuids and are
///   never reused, so a removed and re-added username is a new person.
///   A person has a `badgeKey`, mutes and live `read` frames.
/// - **An App's own pass is not a person**: a link pass, a remote pass or
///   a platform token keeps the App's own bookmark, `app:<serviceId>`
///   (`pass:<tokenId>` for a platform token bound to no App). It reads and
///   marks its cursor (0.46.3 parity behaviour) but has no `badgeKey` and
///   no mute.
/// - Neither collides with a K2 person key (`owner` or a Connect username,
///   which never holds a `:`), so a guest named `adam` never reads the
///   Connect user `adam`'s state (research R4). The research's draft name
///   `app:<principalId>` was not used: `app:` is the service namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassReader {
    pub key: String,
    /// A person (an App guest), not an App's own pass.
    pub person: bool,
    username: String,
}

/// The reader an App pass is ([`PassReader`]).
pub fn pass_reader(pass: &k2_core::skin::SkinPass) -> PassReader {
    let user = pass.username.trim();
    let username = if user.is_empty() { "skin".to_string() } else { user.to_string() };
    if let Some(p) = pass.principal_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return PassReader { key: format!("{GUEST_KEY_PREFIX}{p}"), person: true, username };
    }
    // A link pass carries its App's service id; it rotates hourly, the App
    // doesn't. (Never the link's task-local: handlers run on blocking
    // threads that don't see it, and the key must be one everywhere.)
    if !pass.app.trim().is_empty() {
        return PassReader { key: format!("{APP_KEY_PREFIX}{}", pass.app.trim()), person: false, username };
    }
    PassReader { key: format!("pass:{}", pass.id), person: false, username }
}

/// The read-state key of an App pass ([`pass_reader`]).
pub fn guest_person_key(pass: &k2_core::skin::SkinPass) -> String {
    pass_reader(pass).key
}

/// The read-state key of an App guest by principal id.
pub fn guest_key_of_principal(principal_id: &str) -> String {
    format!("{GUEST_KEY_PREFIX}{}", principal_id.trim())
}

fn guest_person(reader: &PassReader) -> Person {
    Person::app_guest(&reader.key, &reader.username)
}

/// A removed App guest's read state goes with them (TP11 for guests).
pub fn forget_guest(principal_id: &str) {
    let id = principal_id.trim();
    if id.is_empty() {
        return;
    }
    let Some(db) = k2_core::db::try_shared() else { return };
    let conn = db.lock();
    if let Err(e) = read_state::delete_person(&conn, &guest_key_of_principal(id)) {
        k2_core::log_debug!("[read] forget guest {id}: {e}");
    }
}

/// A guest's own post (or card answer) advances its own cursor, as a
/// person's does, and its sockets hear it. Runs under the caller's DB lock.
pub fn note_guest_thread_read(conn: &rusqlite::Connection, pass: &k2_core::skin::SkinPass, conversation_id: &str, seq: i64) {
    if !read_state::tables_exist(conn) {
        return;
    }
    let key = guest_person_key(pass);
    match read_state::ack(conn, &key, KIND_THREAD, conversation_id, Some(seq)) {
        Ok(true) => crate::read_live::guest_moved(&key, conversation_id),
        Ok(false) => {}
        Err(e) => k2_core::log_debug!("[read] guest own-post advance in {conversation_id}: {e}"),
    }
}

/// The state of a room Thread with no messages yet.
fn empty_thread_state(conv: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": KIND_THREAD, "id": conv, "seq": 0, "lastReadSeq": 0, "lastSeq": 0,
        "unread": 0, "waiting": 0, "muted": false,
    })
}

/// One row per room Thread this pass may read (`(addr, conversation)`,
/// R3: never another room), its totals (the badge numbers over those rooms
/// only: unread across unmuted rooms plus every waiting card) and its
/// mutes. Takes the DB lock itself.
type GuestRows = (Vec<serde_json::Value>, Totals, Vec<serde_json::Value>);

fn guest_rows(person: &Person, rooms: &[(String, String)], now: i64) -> Result<GuestRows, ReadError> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut rows = Vec::with_capacity(rooms.len());
    let mut totals = Totals::default();
    let mut muted = Vec::new();
    for (addr, conv) in rooms {
        let mut v = match read_state::item(&conn, person, KIND_THREAD, conv, now) {
            Ok(item) => {
                if !item.muted {
                    totals.total += item.unread;
                }
                totals.total += item.waiting;
                totals.waiting_total += item.waiting;
                if item.muted {
                    let mut m = serde_json::json!({ "id": item.id, "addr": addr });
                    if let Some(u) = item.muted_until {
                        m["mutedUntil"] = serde_json::json!(u);
                    }
                    muted.push(m);
                }
                state_json(&item)
            }
            // A pinned Chat with no Thread yet: nothing unread.
            Err(ReadError::NotFound(_)) => empty_thread_state(conv),
            Err(e) => return Err(e),
        };
        v["addr"] = serde_json::json!(addr);
        rows.push(v);
    }
    Ok((rows, totals, muted))
}

/// One room Thread's state for an App pass plus its totals over its rooms
/// (R3), the shape `POST /cli/thread/read` / `mute` answer and the `read`
/// frame carries: `{kind, id, addr, seq, lastReadSeq, lastSeq, unread,
/// waiting, muted, mutedUntil?, total, waitingTotal}`. `None` when `conv`
/// isn't one of the pass's readable room Threads.
pub fn guest_state(pass: &k2_core::skin::SkinPass, conv: &str) -> Result<Option<serde_json::Value>, ReadError> {
    let reader = pass_reader(pass);
    let person = guest_person(&reader);
    let rooms = crate::overlay_routes::guest_room_threads(pass);
    let (rows, totals, _) = guest_rows(&person, &rooms, read_state::now_secs())?;
    Ok(rows.into_iter().find(|r| r["id"] == conv).map(|r| with_totals(r, totals)))
}

/// The live `read` frame's body for a guest socket (app-read-state-v1):
/// [`guest_state`] without `kind` and `seq` (the socket's own `kind` /
/// `collection` names the frame). `None`: not one of its Threads, or the
/// state couldn't be read (nothing is sent; the App re-reads on reconnect).
pub fn guest_read_frame(pass: &k2_core::skin::SkinPass, conv: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    match guest_state(pass, conv) {
        Ok(Some(serde_json::Value::Object(mut m))) => {
            m.remove("kind");
            m.remove("seq");
            Some(m)
        }
        Ok(_) => None,
        Err(e) => {
            k2_core::log_debug!("[read] guest frame for {conv}: {e}");
            None
        }
    }
}

fn mute_is_people_only() -> CliResponse {
    error_json(
        "403 Forbidden",
        "forbidden",
        "mute belongs to a person (an App guest's sign-in); an App's own pass has no mutes",
    )
}

/// thread-read for an App guest / App pass (`pass` is bound on this thread
/// by the dispatcher), on its own key ([`PassReader`]), over the pinned
/// Chats of rooms where it holds `thread:read` (R3: never another room,
/// never a count or name of one):
/// - `GET /cli/thread/unread` → `{ok, total, waitingTotal, byConversation:
///   [State + addr], muted:[{id, addr, mutedUntil?}], badgeKey?}`
///   (`badgeKey` for a person only, the key a push to them carries);
/// - `POST /cli/thread/read {addr|conversationId, seq?}` → `{ok, changed,
///   …State, addr, total, waitingTotal}`;
/// - `POST /cli/thread/mute {addr|conversationId, muted, until?}` (a
///   person only; `kind: project` is refused) → the same shape.
///
/// A moved cursor or mute sends a `read` frame to every socket of the same
/// reader (`read_live::guest_moved`). Project chat stays people-only (403).
pub fn handle_guest(is_post: bool, path: &str, params: &HashMap<String, String>, body: &[u8], pass: &k2_core::skin::SkinPass) -> CliResponse {
    if is_post_only(path) && !is_post {
        return CliResponse::method_not_allowed();
    }
    if path == UNREAD_PATH && is_post {
        return CliResponse::method_not_allowed();
    }
    if !matches!(path, READ_PATH | UNREAD_PATH | MUTE_PATH) {
        return not_a_person();
    }
    let fields = match Fields::parse(params, body) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let reader = pass_reader(pass);
    let person = guest_person(&reader);
    let now = read_state::now_secs();
    if path == UNREAD_PATH {
        if let Some(k) = fields.str(&["kind"]) {
            if k != KIND_THREAD {
                return usage(format!("an App reads Thread unread only (kind 'thread'), got '{k}'"));
            }
        }
        let rooms = crate::overlay_routes::guest_room_threads(pass);
        let (rows, totals, muted) = match guest_rows(&person, &rooms, now) {
            Ok(x) => x,
            Err(e) => return read_error(e),
        };
        let mut v = serde_json::json!({
            "ok": true,
            "total": totals.total,
            "waitingTotal": totals.waiting_total,
            "byConversation": rows,
            "muted": muted,
        });
        if reader.person {
            let db = k2_core::db::shared();
            let conn = db.lock();
            match read_state::badge_key_for(&conn, &reader.key) {
                Ok(k) => v["badgeKey"] = serde_json::json!(k),
                Err(e) => return read_error(e),
            }
        }
        return CliResponse::ok_json(v.to_string());
    }
    if path == MUTE_PATH {
        if !reader.person {
            return mute_is_people_only();
        }
        if let Some(k) = fields.str(&["kind"]) {
            if k != KIND_THREAD {
                return not_a_person();
            }
        }
    }
    let Some(addr) = fields.str(&["addr", "conversationId", "conversation_id", "id"]) else {
        return usage("missing addr (or conversationId)");
    };
    let conv = match crate::overlay_routes::guest_thread_target(&addr) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let changed = if path == READ_PATH {
        let seq = match fields.int("seq") {
            Ok(s) => s,
            Err(r) => return r,
        };
        let db = k2_core::db::shared();
        let conn = db.lock();
        match read_state::ack(&conn, &reader.key, KIND_THREAD, &conv, seq) {
            Ok(c) => c,
            // Its room's pinned Chat has no Thread yet: nothing to read.
            Err(ReadError::NotFound(_)) => false,
            Err(e) => return read_error(e),
        }
    } else {
        let muted = match fields.bool("muted") {
            Ok(Some(m)) => m,
            Ok(None) => return usage("missing muted (true or false)"),
            Err(r) => return r,
        };
        let until = match fields.int("until") {
            Ok(Some(u)) if u <= 0 => return usage("until is unix seconds"),
            Ok(u) => u,
            Err(r) => return r,
        };
        let db = k2_core::db::shared();
        let conn = db.lock();
        match read_state::set_mute(&conn, &reader.key, KIND_THREAD, &conv, muted, until) {
            Ok(c) => c,
            Err(ReadError::NotFound(_)) => {
                return error_json("404 Not Found", "not_found", "this room's Chat has no messages yet; mute it once it has one");
            }
            Err(e) => return read_error(e),
        }
    };
    if changed {
        crate::read_live::guest_moved(&reader.key, &conv);
    }
    match guest_state(pass, &conv) {
        Ok(Some(mut v)) => {
            v["ok"] = serde_json::json!(true);
            v["changed"] = serde_json::json!(changed);
            v["addr"] = serde_json::json!(addr);
            CliResponse::ok_json(v.to_string())
        }
        // Resolved above, so readable: only a race with a room change.
        Ok(None) => crate::skin_routes::skin_room_response(),
        Err(e) => read_error(e),
    }
}

/// `thread.unread` for the person in a Garden (the Garden bridge,
/// garden-person-rows-v1): the person's own Thread state in the App's
/// shape (no project chats on the bridge, Rosson 2026-10-20). Rows:
/// Threads with something unread, a card waiting, or a mute; totals over
/// Threads only.
pub fn garden_unread(login: &str) -> Result<serde_json::Value, ReadError> {
    let person = person_of_login(login).map_err(ReadError::Store)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let summary = read_state::summary(&conn, &person, read_state::now_secs())?;
    let badge_key = read_state::badge_key_for(&conn, &person.key)?;
    let mut rows = Vec::new();
    let mut muted = Vec::new();
    let mut totals = Totals::default();
    for i in summary.items.iter().filter(|i| i.kind == KIND_THREAD) {
        if !i.muted {
            totals.total += i.unread;
        }
        totals.total += i.waiting;
        totals.waiting_total += i.waiting;
        let addr = crate::overlay_routes::address_of_conversation(&conn, &i.id);
        let mut v = state_json(i);
        if let Some(a) = &addr {
            v["addr"] = serde_json::json!(a);
        }
        if i.muted {
            let mut m = serde_json::json!({ "id": i.id });
            if let Some(a) = &addr {
                m["addr"] = serde_json::json!(a);
            }
            if let Some(u) = i.muted_until {
                m["mutedUntil"] = serde_json::json!(u);
            }
            muted.push(m);
        }
        rows.push(v);
    }
    Ok(serde_json::json!({
        "ok": true,
        "total": totals.total,
        "waitingTotal": totals.waiting_total,
        "byConversation": rows,
        "muted": muted,
        "badgeKey": badge_key,
    }))
}

/// `thread.mute` for the person in a Garden: [`handle`] as `login` on a
/// Thread (never a project chat), answered in the App's shape (`addr`
/// added when given).
pub fn garden_thread_mute(login: &str, args: &serde_json::Map<String, serde_json::Value>) -> CliResponse {
    let addr = args.get("addr").and_then(|a| a.as_str()).map(str::to_string);
    let mut body = args.clone();
    body.insert("kind".into(), serde_json::json!(KIND_THREAD));
    let raw = serde_json::Value::Object(body).to_string();
    let r = handle(true, MUTE_PATH, &HashMap::new(), raw.as_bytes(), login);
    if r.status != "200 OK" {
        return r;
    }
    match (serde_json::from_str::<serde_json::Value>(&r.body), addr) {
        (Ok(mut v), Some(a)) => {
            v["addr"] = serde_json::json!(a);
            CliResponse::ok_json(v.to_string())
        }
        _ => r,
    }
}

/// Route entry. `login` is the dispatcher-resolved person login.
pub fn handle(is_post: bool, path: &str, params: &HashMap<String, String>, body: &[u8], login: &str) -> CliResponse {
    if is_post_only(path) && !is_post {
        return CliResponse::method_not_allowed();
    }
    if path == UNREAD_PATH && is_post {
        return CliResponse::method_not_allowed();
    }
    let person = match person_of_login(login) {
        Ok(p) => p,
        Err(e) => return error_json("500 Internal Server Error", "store", e),
    };
    let fields = match Fields::parse(params, body) {
        Ok(f) => f,
        Err(r) => return r,
    };
    match path {
        UNREAD_PATH => handle_unread(&person, &fields),
        READ_PATH => match thread_id(&fields) {
            Ok(id) => handle_ack(&person, KIND_THREAD, &id, &fields),
            Err(r) => r,
        },
        PROJECT_READ_PATH => match group_id(&fields) {
            Ok(id) => handle_ack(&person, KIND_PROJECT, &id, &fields),
            Err(r) => r,
        },
        MUTE_PATH => handle_mute(&person, &fields),
        _ => CliResponse::not_found(),
    }
}

fn handle_unread(person: &Person, f: &Fields) -> CliResponse {
    let kind = f.str(&["kind"]);
    if let Some(k) = &kind {
        if !read_state::valid_kind(k) {
            return usage(format!("kind must be 'thread' or 'project', got '{k}'"));
        }
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let summary = match read_state::summary(&conn, person, read_state::now_secs()) {
        Ok(s) => s,
        Err(e) => return read_error(e),
    };
    let badge_key = match read_state::badge_key_for(&conn, &person.key) {
        Ok(k) => k,
        Err(e) => return read_error(e),
    };
    drop(conn);
    let items: Vec<&ReadItem> = summary.items.iter().filter(|i| kind.as_deref().is_none_or(|k| i.kind == k)).collect();
    let muted: Vec<serde_json::Value> = items
        .iter()
        .filter(|i| i.muted)
        .map(|i| {
            let mut v = serde_json::json!({ "kind": i.kind, "id": i.id });
            if let Some(u) = i.muted_until {
                v["mutedUntil"] = serde_json::json!(u);
            }
            v
        })
        .collect();
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "person": person.key,
            // The key every push for this person carries (never a name).
            "badgeKey": badge_key,
            "total": summary.totals.total,
            "waitingTotal": summary.totals.waiting_total,
            "byConversation": items.iter().map(|i| state_json(i)).collect::<Vec<_>>(),
            "muted": muted,
        })
        .to_string(),
    )
}

/// Compute one conversation's state + the person's totals after a change.
fn state_after(person: &Person, kind: &str, id: &str, changed: bool) -> CliResponse {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let now = read_state::now_secs();
    let item = match read_state::item(&conn, person, kind, id, now) {
        Ok(i) => i,
        Err(e) => return read_error(e),
    };
    let totals = match read_state::totals(&conn, person, now) {
        Ok(t) => t,
        Err(e) => return read_error(e),
    };
    drop(conn);
    if changed {
        // Every socket of this person (desktop and phone) follows, and
        // their phones get a corrected badge.
        crate::read_live::person_changed(&person.key, kind, id);
        crate::badge_push::trigger(&person.key);
    }
    let mut v = with_totals(state_json(&item), totals);
    v["ok"] = serde_json::json!(true);
    v["changed"] = serde_json::json!(changed);
    CliResponse::ok_json(v.to_string())
}

fn handle_ack(person: &Person, kind: &str, id: &str, f: &Fields) -> CliResponse {
    let seq = match f.int("seq") {
        Ok(s) => s,
        Err(r) => return r,
    };
    let changed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match read_state::ack(&conn, &person.key, kind, id, seq) {
            Ok(c) => c,
            Err(e) => return read_error(e),
        }
    };
    state_after(person, kind, id, changed)
}

fn handle_mute(person: &Person, f: &Fields) -> CliResponse {
    let kind = f.str(&["kind"]).unwrap_or_else(|| KIND_THREAD.to_string());
    if !read_state::valid_kind(&kind) {
        return usage(format!("kind must be 'thread' or 'project', got '{kind}'"));
    }
    let id = if kind == KIND_PROJECT { group_id(f) } else { thread_id(f) };
    let id = match id {
        Ok(id) => id,
        Err(r) => return r,
    };
    let muted = match f.bool("muted") {
        Ok(Some(m)) => m,
        Ok(None) => return usage("missing muted (true or false)"),
        Err(r) => return r,
    };
    let until = match f.int("until") {
        Ok(Some(u)) if u <= 0 => return usage("until is unix seconds"),
        Ok(u) => u,
        Err(r) => return r,
    };
    let changed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match read_state::set_mute(&conn, &person.key, &kind, &id, muted, until) {
            Ok(c) => c,
            Err(e) => return read_error(e),
        }
    };
    state_after(person, &kind, &id, changed)
}

/// Boot: see every person this server knows (each starts fully read).
pub fn boot() {
    let people = match People::load() {
        Ok(p) => p,
        Err(e) => {
            k2_core::log_debug!("[read] boot: people load failed: {e}");
            return;
        }
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    if !read_state::tables_exist(&conn) {
        return;
    }
    match read_state::ensure_people(&conn, &people) {
        Ok(0) => {}
        Ok(n) => k2_core::log_debug!("[read] boot: {n} person(s) start fully read"),
        Err(e) => k2_core::log_debug!("[read] boot: {e}"),
    }
}
