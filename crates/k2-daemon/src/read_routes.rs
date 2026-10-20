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
//! [`handle_guest`]): `POST /cli/thread/read` and `GET /cli/thread/unread`
//! only, on their own cursors (`guest:<principal>` / `app:<App>`, never a
//! person's), for the pinned Chats of rooms where they hold `thread:read`.
//! Mute and project chat stay people-only.

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

/// The read-state key of an App guest (its login principal) or an App's
/// own pass (its App). Never collides with a person key (those are
/// `owner` or a Connect username, which never holds a `:`).
pub fn guest_person_key(pass: &k2_core::skin::SkinPass) -> String {
    if let Some(p) = pass.principal_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return format!("guest:{p}");
    }
    // A link pass carries its App's service id; it rotates hourly, the App
    // doesn't. (Never the link's task-local: handlers run on blocking
    // threads that don't see it, and the key must be one everywhere.)
    if !pass.app.trim().is_empty() {
        return format!("app:{}", pass.app.trim());
    }
    format!("pass:{}", pass.id)
}

fn guest_person(pass: &k2_core::skin::SkinPass) -> Person {
    let user = pass.username.trim();
    Person::app_guest(&guest_person_key(pass), if user.is_empty() { "skin" } else { user })
}

/// A guest's own post (or card answer) advances its own cursor, as a
/// person's does. Runs under the caller's DB lock.
pub fn note_guest_thread_read(conn: &rusqlite::Connection, pass: &k2_core::skin::SkinPass, conversation_id: &str, seq: i64) {
    if !read_state::tables_exist(conn) {
        return;
    }
    if let Err(e) = read_state::ack(conn, &guest_person_key(pass), KIND_THREAD, conversation_id, Some(seq)) {
        k2_core::log_debug!("[read] guest own-post advance in {conversation_id}: {e}");
    }
}

/// The state of a room Thread with no messages yet.
fn empty_thread_state(conv: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": KIND_THREAD, "id": conv, "seq": 0, "lastReadSeq": 0, "lastSeq": 0,
        "unread": 0, "waiting": 0, "muted": false,
    })
}

/// thread-read for an App guest / App pass (`pass` is bound on this thread
/// by the dispatcher). `POST /cli/thread/read {addr|conversationId, seq?}`
/// → `{ok, changed, …State}`; `GET /cli/thread/unread` → `{ok, total,
/// waitingTotal, byConversation:[State + addr]}` over its readable rooms.
/// Mute and project chat: 403 (people only). No push, no live frames: a
/// guest has neither.
pub fn handle_guest(is_post: bool, path: &str, params: &HashMap<String, String>, body: &[u8], pass: &k2_core::skin::SkinPass) -> CliResponse {
    if is_post_only(path) && !is_post {
        return CliResponse::method_not_allowed();
    }
    if path == UNREAD_PATH && is_post {
        return CliResponse::method_not_allowed();
    }
    if path != READ_PATH && path != UNREAD_PATH {
        return not_a_person();
    }
    let fields = match Fields::parse(params, body) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let person = guest_person(pass);
    let now = read_state::now_secs();
    if path == UNREAD_PATH {
        let rooms = crate::overlay_routes::guest_room_threads(pass);
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut rows = Vec::new();
        let (mut total, mut waiting) = (0i64, 0i64);
        for (addr, conv) in rooms {
            match read_state::item(&conn, &person, KIND_THREAD, &conv, now) {
                Ok(item) => {
                    if !item.muted {
                        total += item.unread;
                    }
                    waiting += item.waiting;
                    let mut v = state_json(&item);
                    v["addr"] = serde_json::json!(addr);
                    rows.push(v);
                }
                // A pinned Chat with no Thread yet: nothing unread.
                Err(ReadError::NotFound(_)) => {
                    let mut v = empty_thread_state(&conv);
                    v["addr"] = serde_json::json!(addr);
                    rows.push(v);
                }
                Err(e) => return read_error(e),
            }
        }
        return CliResponse::ok_json(
            serde_json::json!({ "ok": true, "total": total, "waitingTotal": waiting, "byConversation": rows }).to_string(),
        );
    }
    let Some(addr) = fields.str(&["addr", "conversationId", "conversation_id", "id"]) else {
        return usage("missing addr (or conversationId)");
    };
    let conv = match crate::overlay_routes::guest_thread_target(&addr) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let seq = match fields.int("seq") {
        Ok(s) => s,
        Err(r) => return r,
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let changed = match read_state::ack(&conn, &person.key, KIND_THREAD, &conv, seq) {
        Ok(c) => c,
        // Its room's pinned Chat has no Thread yet: nothing to read.
        Err(ReadError::NotFound(_)) => {
            let mut v = empty_thread_state(&conv);
            v["ok"] = serde_json::json!(true);
            v["changed"] = serde_json::json!(false);
            v["addr"] = serde_json::json!(addr);
            return CliResponse::ok_json(v.to_string());
        }
        Err(e) => return read_error(e),
    };
    match read_state::item(&conn, &person, KIND_THREAD, &conv, now) {
        Ok(item) => {
            let mut v = state_json(&item);
            v["ok"] = serde_json::json!(true);
            v["changed"] = serde_json::json!(changed);
            v["addr"] = serde_json::json!(addr);
            CliResponse::ok_json(v.to_string())
        }
        Err(e) => read_error(e),
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
