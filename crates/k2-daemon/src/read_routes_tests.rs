//! push-read-hints-v1: which read routes tell the badge coalescer a cursor
//! moved. The coalescer's own tests are in `badge_push`.

use super::*;
use crate::badge_push::test_read_capture::for_chats;
use k2_core::overlay::{self, PostAuthor};
use rusqlite::params;
use serde_json::json;

struct Fx {
    _home: crate::test_support::TempHome,
    _db: k2_core::db::TestDbGuard,
    project: String,
}

fn fx() -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let project = format!("rh-{}", uuid::Uuid::new_v4());
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)",
            params![project, format!("/tmp/{project}")],
        )
        .expect("seed project");
    }
    Fx { _home: home, _db: db, project }
}

fn conv() -> String {
    format!("rh-conv-{}", uuid::Uuid::new_v4())
}

fn agent_posts(conv: &str, project: &str, n: usize) -> i64 {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let mut last = 0;
    for i in 0..n {
        last = overlay::post_thread(&conn, conv, project, "rh-agent", "rh-agent", &format!("m{i}"), "thread").expect("post").0.seq;
    }
    last
}

fn post(path: &str, body: serde_json::Value) -> serde_json::Value {
    let r = handle(true, path, &HashMap::new(), body.to_string().as_bytes(), "owner");
    assert_eq!(r.status, "200 OK", "{path}: {}", r.body);
    serde_json::from_str(&r.body).expect("json")
}

fn noted(person: &str, kind: &str, id: &str, seq: i64) -> (String, String, String, i64) {
    (person.to_string(), kind.to_string(), id.to_string(), seq)
}

#[test]
fn a_thread_read_notes_the_new_cursor_and_only_when_it_moves() {
    let f = fx();
    let c = conv();
    let head = agent_posts(&c, &f.project, 3);

    let v = post(READ_PATH, json!({ "conversationId": c, "seq": head - 1 }));
    assert_eq!(v["changed"], true);
    assert_eq!(for_chats(&[&c]), vec![noted("owner", KIND_THREAD, &c, head - 1)]);

    // Behind or at the cursor: nothing moved, nothing noted.
    let v = post(READ_PATH, json!({ "conversationId": c, "seq": head - 2 }));
    assert_eq!(v["changed"], false);
    assert_eq!(for_chats(&[&c]).len(), 1, "a read behind the cursor notes nothing");

    // No seq means the newest, and past the head is clamped: the entry has
    // the cursor it really moved to.
    post(READ_PATH, json!({ "conversationId": c, "seq": head + 50 }));
    assert_eq!(for_chats(&[&c])[1], noted("owner", KIND_THREAD, &c, head));
}

#[test]
fn a_project_read_notes_the_project_kind() {
    let _f = fx();
    let g = k2_core::project_groups::create_group(&format!("rh-{}", uuid::Uuid::new_v4())).expect("group").id;
    let m = k2_core::project_groups::post_message_as(&g, "rh-agent", "agent", "hi").expect("post");
    post(PROJECT_READ_PATH, json!({ "group": g, "seq": m.seq }));
    assert_eq!(for_chats(&[&g]), vec![noted("owner", KIND_PROJECT, &g, m.seq)]);
}

/// Mute shares `state_after` with the read route, and it isn't a read.
#[test]
fn a_mute_is_never_a_read() {
    let f = fx();
    let c = conv();
    agent_posts(&c, &f.project, 2);
    let v = post(MUTE_PATH, json!({ "conversationId": c, "muted": true }));
    assert_eq!(v["changed"], true);
    assert_eq!(for_chats(&[&c]), Vec::new(), "a mute notes nothing");
    let v = post(MUTE_PATH, json!({ "conversationId": c, "muted": false }));
    assert_eq!(v["changed"], true);
    assert_eq!(for_chats(&[&c]), Vec::new(), "nor does an unmute");
}

/// Every Thread behind its head is noted with its own head (the coalescer
/// keeps the newest 8), and the reply still lists plain ids.
#[test]
fn read_all_notes_every_thread_with_its_own_cursor() {
    let f = fx();
    let convs: Vec<String> = (0..10).map(|_| conv()).collect();
    let mut want = Vec::new();
    for (i, c) in convs.iter().enumerate() {
        want.push((c.as_str(), agent_posts(c, &f.project, i + 1)));
    }
    let v = post(READ_ALL_PATH, json!({ "projectId": f.project }));
    assert_eq!(v["changed"], true);
    let ids: Vec<&str> = convs.iter().map(String::as_str).collect();
    let got = for_chats(&ids);
    assert_eq!(got.len(), 10, "{got:?}");
    for (c, head) in want {
        assert!(got.contains(&noted("owner", KIND_THREAD, c, head)), "{c} at {head}: {got:?}");
    }
    let marked: Vec<&str> = v["marked"].as_array().expect("marked").iter().map(|m| m.as_str().expect("an id")).collect();
    assert_eq!(marked.len(), 10, "marked stays a list of ids: {v}");
    assert!(marked.iter().all(|m| ids.contains(m)), "{v}");

    post(READ_ALL_PATH, json!({ "projectId": f.project }));
    assert_eq!(for_chats(&ids).len(), 10, "nothing behind its head: nothing more noted");
}

/// An own post and a card answer come through here (`overlay_routes` and
/// `project_group_routes`). An agent or App guest has no author person.
#[test]
fn an_own_post_or_card_answer_notes_but_an_agents_does_not() {
    let f = fx();
    let c = conv();
    let head = agent_posts(&c, &f.project, 2);
    let h = k2_core::db::shared();
    let conn = h.lock();

    note_own_thread_read(&conn, Some(&PostAuthor::owner()), &c, head);
    assert_eq!(for_chats(&[&c]), vec![noted("owner", KIND_THREAD, &c, head)]);
    note_own_thread_read(&conn, Some(&PostAuthor::owner()), &c, head);
    assert_eq!(for_chats(&[&c]).len(), 1, "already there: nothing more");

    let c2 = format!("{c}-b");
    let head2 = overlay::post_thread(&conn, &c2, &f.project, "rh-agent", "rh-agent", "hi", "thread").expect("post").0.seq;
    note_own_thread_read(&conn, None, &c2, head2);
    assert_eq!(for_chats(&[&c2]), Vec::new(), "an agent's post or answer moves nobody");
    let guest = PostAuthor { kind: overlay::doc::AUTHOR_GUEST.to_string(), login: "gia".into(), name: None };
    note_own_thread_read(&conn, Some(&guest), &c2, head2);
    assert_eq!(for_chats(&[&c2]), Vec::new(), "nor does an App guest's");
}
