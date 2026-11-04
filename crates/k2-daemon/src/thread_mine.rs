//! "Is this Thread post mine?", answered by the daemon for the person
//! reading it (thread-mine-v1, 0.46.8).
//!
//! A person's post carries `author: {kind, login}`, stamped from the
//! post's credential (`overlay_routes::handle_post`). Clients used to
//! decide "mine" on their own by comparing that stamp with a viewer they
//! looked up separately (`GET /cli/notify/prefs`, then `auth/whoami`, then
//! the saved login). When that lookup missed on a remote server (Rosson
//! 2026-11-04, a desktop on another Mac signed in as the owner's linked
//! login), every own post drew as someone else's: the pending bubble was
//! blue, the server's copy grey.
//!
//! So every door that serves a Thread doc to a PERSON now says it: the
//! doc gets `mine: true|false`, computed here from the same credential
//! that reads it.
//! - `GET /cli/thread` pages, deltas and catch-up entries
//!   (`thread_items_view`);
//! - every `WS /cli/overlay/events` frame (`overlay_ws::wire_json_for`,
//!   the socket's own viewer);
//! - `GET /cli/thread/latest` rows (`mine` on the row);
//! - the `POST /cli/thread/post` answer (`mine` on the body: a person's
//!   post is stamped from that very request, so it is always theirs).
//!
//! The rule is the person key (`k2_core::notify_prefs`): the owner token
//! and every login the owner linked are one person, `owner`; any other
//! login is itself. A reserved-name login (`owner`, made before 0.46.6)
//! is nobody, so it owns nothing and nothing it posted is anyone's.
//! App guests, imported lines and agent posts are never a person's here.
//!
//! Only docs with an `author` are marked, and only for a person viewer:
//! an agent passport or an App guest gets the doc as before, and a doc
//! from before the field keeps the client's old rule (a `via=compose` post
//! with no author reads as yours).

use k2_core::notify_prefs::{People, OWNER};
use k2_core::overlay::doc::{AUTHOR_OWNER, AUTHOR_USER};
use k2_core::permission::card::Viewer;
use serde_json::Value;

/// The doc / row key.
pub const MINE_KEY: &str = "mine";

/// The person a viewer is, or `None` when it is not a person we mark for
/// (an agent passport, an App guest, a reserved-name login, nobody).
pub(crate) fn viewer_person(people: &People, viewer: &Viewer) -> Option<String> {
    match viewer {
        Viewer::OwnerToken => Some(OWNER.to_string()),
        Viewer::Login { user, .. } => people.login_person_key(user),
        Viewer::Guest { .. } | Viewer::Passport { .. } => None,
    }
}

/// The person who wrote a post, from its `author` JSON, or `None` for
/// anything that is not a person on this server (a guest, an imported
/// line, an unknown kind, a reserved-name login).
fn author_person(people: &People, author: &Value) -> Option<String> {
    let kind = author.get("kind").and_then(Value::as_str).unwrap_or("").trim();
    let login = author.get("login").and_then(Value::as_str).unwrap_or("").trim();
    match kind {
        AUTHOR_OWNER => Some(OWNER.to_string()),
        AUTHOR_USER if !login.is_empty() => people.login_person_key(login),
        _ => None,
    }
}

/// Is a post by `author` (its JSON) the person `me`'s?
pub(crate) fn is_mine(people: &People, me: &str, author: &Value) -> bool {
    author_person(people, author).is_some_and(|a| a == me)
}

fn has_author(doc: &Value) -> bool {
    doc.get("author").is_some_and(|a| a.is_object())
}

/// Write `mine` on one doc object that has an `author`.
fn mark_doc(people: &People, me: &str, doc: &mut Value) {
    if !has_author(doc) {
        return;
    }
    let mine = is_mine(people, me, &doc["author"]);
    if let Some(map) = doc.as_object_mut() {
        map.insert(MINE_KEY.into(), Value::Bool(mine));
    }
}

/// Mark every `{.., doc: {author}}` item in a JSON array for `viewer`.
/// Reads the people list only when there is someone to mark.
pub(crate) fn mark_items(items: &mut Value, viewer: &Viewer) {
    let Some(list) = items.as_array_mut() else { return };
    if !viewer.is_person() || !list.iter().any(|it| it.get("doc").is_some_and(has_author)) {
        return;
    }
    let Some((people, me)) = load_viewer(viewer) else { return };
    for it in list.iter_mut() {
        if let Some(doc) = it.get_mut("doc") {
            mark_doc(&people, &me, doc);
        }
    }
}

/// [`mark_items`] for a frame's JSON text (`{collection, seq, id, doc}`).
/// Anything without a person doc comes back unchanged.
pub(crate) fn mark_frame_json(json: String, viewer: &Viewer) -> String {
    if !viewer.is_person() || !json.contains("\"author\"") {
        return json;
    }
    let Ok(mut v) = serde_json::from_str::<Value>(&json) else { return json };
    if !v.get("doc").is_some_and(has_author) {
        return json;
    }
    let Some((people, me)) = load_viewer(viewer) else { return json };
    if let Some(doc) = v.get_mut("doc") {
        mark_doc(&people, &me, doc);
    }
    v.to_string()
}

/// `mine` for one `thread/latest` row (its `author` is on the row).
pub(crate) fn mark_row(row: &mut Value, viewer: &Viewer) {
    if !viewer.is_person() || !has_author(row) {
        return;
    }
    let Some((people, me)) = load_viewer(viewer) else { return };
    mark_doc(&people, &me, row);
}

fn load_viewer(viewer: &Viewer) -> Option<(People, String)> {
    let people = match People::load() {
        Ok(p) => p,
        Err(e) => {
            // Leave the docs unmarked: the client keeps its own rule.
            k2_core::log_debug!("[thread-mine] people list unreadable: {e}");
            return None;
        }
    };
    let me = viewer_person(&people, viewer)?;
    Some((people, me))
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::connect_users::ConnectUserView;
    use serde_json::json;

    fn people(linked: &[&str]) -> People {
        People { users: Vec::<ConnectUserView>::new(), linked: linked.iter().map(|s| s.to_string()).collect() }
    }

    fn login(user: &str) -> Viewer {
        Viewer::Login { user: user.to_string(), role: "owner".to_string() }
    }

    /// The report: the owner's linked login on another computer posts as
    /// `user/z3thon`; that login reading it is the owner person, and so is
    /// the owner token. An unlinked login is only itself.
    #[test]
    fn a_linked_login_and_the_owner_token_are_one_person() {
        let p = people(&["z3thon"]);
        let by_login = json!({"kind": "user", "login": "z3thon"});
        let by_token = json!({"kind": "owner", "login": "owner"});
        for viewer in [login("z3thon"), login("Z3thon"), Viewer::OwnerToken] {
            let me = viewer_person(&p, &viewer).expect("a person");
            assert_eq!(me, OWNER, "{viewer:?}");
            assert!(is_mine(&p, &me, &by_login), "{viewer:?} wrote it from the linked login");
            assert!(is_mine(&p, &me, &by_token), "{viewer:?} wrote it from the owner token");
        }
        let unlinked = people(&[]);
        let me = viewer_person(&unlinked, &login("z3thon")).expect("a person");
        assert_eq!(me, "z3thon");
        assert!(is_mine(&unlinked, &me, &by_login));
        assert!(!is_mine(&unlinked, &me, &by_token), "an unlinked login is not the owner");
        let owner = viewer_person(&unlinked, &Viewer::OwnerToken).expect("the owner");
        assert!(!is_mine(&unlinked, &owner, &by_login));
        assert!(is_mine(&unlinked, &owner, &by_token));
    }

    /// Someone else's post, a guest's, an imported line and a post by a
    /// login named `owner` (made before 0.46.6) are never the viewer's.
    #[test]
    fn other_people_guests_imported_lines_and_reserved_logins_are_not_mine() {
        let p = people(&["z3thon"]);
        let me = viewer_person(&p, &login("z3thon")).expect("a person");
        assert!(!is_mine(&p, &me, &json!({"kind": "user", "login": "alice"})));
        assert!(!is_mine(&p, &me, &json!({"kind": "guest", "login": "z3thon"})));
        assert!(!is_mine(&p, &me, &json!({"kind": "imported", "login": "owner", "name": "Rosson"})));
        assert!(!is_mine(&p, &me, &json!({"kind": "user", "login": "owner"})), "a reserved-name login is nobody");
        assert!(!is_mine(&p, &me, &json!({"kind": "user", "login": ""})));
        assert_eq!(viewer_person(&p, &login("owner")), None, "a reserved-name login owns nothing");
        assert_eq!(viewer_person(&p, &Viewer::Guest { app: None }), None);
        assert_eq!(viewer_person(&p, &Viewer::Passport { workspace_id: "w".into() }), None);
    }

    /// The doors' JSON: person docs get `mine`, a doc without an author and
    /// any non-person viewer get nothing (the client's old rule stays).
    #[test]
    fn marks_only_person_docs_for_a_person_viewer() {
        let _home = crate::test_support::TempHome::new();
        let mut items = json!([
            {"id": "a", "doc": {"author": {"kind": "owner", "login": "owner"}, "via": "compose"}},
            {"id": "b", "doc": {"author": {"kind": "user", "login": "alice"}, "via": "compose"}},
            {"id": "c", "doc": {"via": "thread", "from": "k2"}},
            {"id": "d", "doc": {"via": "compose"}},
        ]);
        let agent = Viewer::Passport { workspace_id: "w".into() };
        let before = items.clone();
        mark_items(&mut items, &agent);
        assert_eq!(items, before, "an agent passport's read is unchanged");
        mark_items(&mut items, &Viewer::Guest { app: None });
        assert_eq!(items, before, "an App guest's read is unchanged");
        mark_items(&mut items, &Viewer::OwnerToken);
        assert_eq!(items[0]["doc"]["mine"], json!(true));
        assert_eq!(items[1]["doc"]["mine"], json!(false));
        assert!(items[2]["doc"].get("mine").is_none(), "an agent post has no author: unmarked");
        assert!(items[3]["doc"].get("mine").is_none(), "an old compose doc: unmarked");

        let frame = json!({"collection": "thread", "seq": 4, "id": "e", "doc": {"author": {"kind": "user", "login": "alice"}}}).to_string();
        let marked: Value = serde_json::from_str(&mark_frame_json(frame.clone(), &login("alice"))).expect("json");
        assert_eq!(marked["doc"]["mine"], json!(true), "alice's own post on alice's socket");
        let theirs: Value = serde_json::from_str(&mark_frame_json(frame.clone(), &Viewer::OwnerToken)).expect("json");
        assert_eq!(theirs["doc"]["mine"], json!(false), "alice's post on the owner's socket");
        assert_eq!(mark_frame_json(frame.clone(), &Viewer::Guest { app: None }), frame, "a guest socket's frame is unchanged");
        let agent_frame = json!({"collection": "thread", "seq": 5, "id": "f", "doc": {"from": "k2"}}).to_string();
        assert_eq!(mark_frame_json(agent_frame.clone(), &Viewer::OwnerToken), agent_frame);

        let mut row = json!({"addr": "k2", "author": {"kind": "owner", "login": "owner"}});
        mark_row(&mut row, &Viewer::OwnerToken);
        assert_eq!(row["mine"], json!(true));
        let mut empty = json!({"addr": "k2", "author": null});
        mark_row(&mut empty, &Viewer::OwnerToken);
        assert!(empty.get("mine").is_none());
    }
}
