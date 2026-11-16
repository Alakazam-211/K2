//! Thread authorship (0.46.13): every post and card says who sent it, from
//! the credential, never the body. Before 0.46.13 a person (the owner token
//! or any Connect login) posting with `via` other than `compose` (`k2
//! thread` with no passport, a direct POST) got no author and the body's
//! `from`, so the row read as the agent's, under any name.

use super::*;

fn room(prefix: &str) -> (String, String) {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    (handle, project_id)
}

fn post_as(handle: &str, body: serde_json::Value, session_author: &str) -> CliResponse {
    let mut b = body;
    b["addr"] = serde_json::json!(handle);
    dispatch_post_as("/cli/thread/post", &HashMap::new(), b.to_string().as_bytes(), session_author)
}

fn stored(handle: &str, id: &str) -> serde_json::Value {
    let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle)])).expect("GET thread"));
    snap["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|i| i["id"] == id)
        .unwrap_or_else(|| panic!("no stored doc {id}: {snap}"))["doc"]
        .clone()
}

fn id_of(resp: &CliResponse) -> String {
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    json_body(resp)["id"].as_str().expect("id").to_string()
}

/// The owner token posting with `via=thread` (or no `via`) and a body
/// `from` naming the agent is the owner's post: author `owner`, `from` the
/// owner's display name. A Connect login is itself. The agent's name in
/// the body changes nothing.
#[test]
fn a_persons_non_compose_post_is_theirs_never_the_agents() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, _) = room("authp");
    let owner_name = crate::workspace_msg::resolve_owner_from();
    for (text, via) in [("owner-thread", Some("thread")), ("owner-no-via", None), ("owner-msg", Some("msg"))] {
        let mut body = serde_json::json!({ "text": text, "from": handle, "author": {"kind": "agent", "login": handle} });
        if let Some(v) = via {
            body["via"] = serde_json::json!(v);
        }
        let resp = post_as(&handle, body, crate::session_author::OWNER_TOKEN);
        let id = id_of(&resp);
        assert_eq!(json_body(&resp)["author"], serde_json::json!({"kind": "owner", "login": "owner"}), "{}", resp.body);
        let doc = stored(&handle, &id);
        assert_eq!(doc["author"], serde_json::json!({"kind": "owner", "login": "owner"}), "{text}: {doc}");
        assert_eq!(doc["from"], serde_json::json!(owner_name), "{text}: the body's from names nobody: {doc}");
    }
    for from in ["k2", "owner", "someone-else"] {
        let resp = post_as(&handle, serde_json::json!({ "text": format!("alice-as-{from}"), "via": "thread", "from": from }), "alice");
        let doc = stored(&handle, &id_of(&resp));
        assert_eq!(doc["author"], serde_json::json!({"kind": "user", "login": "alice"}), "{doc}");
        assert_eq!(doc["from"], "alice", "{doc}");
    }
}

/// A person's cards (`thread/ask`, `thread/secret`) are theirs too, with
/// the same author and `from`, whatever the body says.
#[test]
fn a_persons_cards_are_theirs_never_the_agents() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, _) = room("authc");
    let owner_name = crate::workspace_msg::resolve_owner_from();

    let ask = dispatch_post_as(
        "/cli/thread/ask",
        &HashMap::new(),
        serde_json::json!({ "addr": handle, "prompt": "Ship?", "options": "Ship,Wait", "from": handle }).to_string().as_bytes(),
        crate::session_author::OWNER_TOKEN,
    );
    let doc = stored(&handle, &id_of(&ask));
    assert_eq!(doc["author"], serde_json::json!({"kind": "owner", "login": "owner"}), "{doc}");
    assert_eq!(doc["from"], serde_json::json!(owner_name), "{doc}");

    let ask = dispatch_post_as(
        "/cli/thread/ask",
        &HashMap::new(),
        serde_json::json!({ "addr": handle, "prompt": "Ship?", "options": "Ship,Wait", "from": handle }).to_string().as_bytes(),
        "mallory",
    );
    let doc = stored(&handle, &id_of(&ask));
    assert_eq!(doc["author"], serde_json::json!({"kind": "user", "login": "mallory"}), "{doc}");
    assert_eq!(doc["from"], "mallory", "{doc}");

    let secret = dispatch_post_as(
        "/cli/thread/secret",
        &HashMap::new(),
        serde_json::json!({ "addr": handle, "name": "AUTHOR_TEST_KEY", "for": "api.example.com", "from": handle })
            .to_string()
            .as_bytes(),
        "mallory",
    );
    let doc = stored(&handle, &id_of(&secret));
    assert_eq!(doc["author"], serde_json::json!({"kind": "user", "login": "mallory"}), "{doc}");
    assert_eq!(doc["from"], "mallory", "{doc}");
}

/// An App guest is unchanged (always `guest`, its pass username), and a
/// body naming the agent or an agent author changes nothing.
#[test]
fn a_guest_is_always_a_guest() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("authg");
    let pass = skin_pass(&[project_id.clone()]);
    let resp = with_request_skin(Some(pass), || {
        dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "text": "hi", "from": handle, "author": {"kind": "agent", "login": handle} })
                .to_string()
                .as_bytes(),
        )
    });
    let doc = stored(&handle, &id_of(&resp));
    assert_eq!(doc["author"], serde_json::json!({"kind": "guest", "login": "guest"}), "{doc}");
    assert_eq!(doc["from"], "guest", "{doc}");
}

/// thread-author-agent-v1: an agent's post, its cards and a sidecar's post
/// carry `author {kind: "agent", login: <its stamped address>}`, from the
/// passport, never the body.
#[test]
fn an_agents_posts_and_cards_carry_the_agent_author() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("autha");
    let agent = serde_json::json!({"kind": "agent", "login": handle});
    let spoof = serde_json::json!({"kind": "owner", "login": "owner"});

    let post = post_json(
        "/cli/thread/post",
        &passport_params(&project_id, &handle),
        serde_json::json!({ "addr": handle, "text": "agent words", "from": "mallory", "author": spoof }),
    );
    let doc = stored(&handle, &id_of(&post));
    assert_eq!(doc["author"], agent, "{doc}");
    assert_eq!(doc["from"], serde_json::json!(handle), "{doc}");

    let ask = post_json(
        "/cli/thread/ask",
        &passport_params(&project_id, &handle),
        serde_json::json!({ "addr": handle, "prompt": "Ship?", "options": "Ship,Wait", "from": "mallory" }),
    );
    let doc = stored(&handle, &id_of(&ask));
    assert_eq!(doc["author"], agent, "{doc}");

    let secret = post_json(
        "/cli/thread/secret",
        &passport_params(&project_id, &handle),
        serde_json::json!({ "addr": handle, "name": "AGENT_AUTHOR_KEY", "for": "api.example.com" }),
    );
    let doc = stored(&handle, &id_of(&secret));
    assert_eq!(doc["author"], agent, "{doc}");

    // A sidecar: its own stamped address.
    let conv = uuid::Uuid::new_v4().to_string();
    sidecar(&project_id, &conv, "reviewer");
    let side = format!("{handle}/reviewer");
    let post = post_json("/cli/thread/post", &passport_params(&project_id, &side), serde_json::json!({ "addr": side, "text": "sidecar words" }));
    let doc = stored(&side, &id_of(&post));
    assert_eq!(doc["author"], serde_json::json!({"kind": "agent", "login": side}), "{doc}");
}

/// The delete rule keeps an agent's rows out of reach, whether stamped
/// (0.46.13) or author-less (older rows): even the owner can't delete them.
/// A person's non-compose post is theirs, so they may.
#[test]
fn agent_rows_old_and_new_are_never_deletable_and_a_persons_shell_post_is() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("authd");
    let stamped = post_json("/cli/thread/post", &passport_params(&project_id, &handle), serde_json::json!({ "addr": handle, "text": "stamped" }));
    let stamped_id = id_of(&stamped);
    let resolved = resolve_addr(&handle).unwrap_or_else(|r| panic!("resolve {handle}: {}", r.body));
    let old_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let (item, _) = k2_core::overlay::post_thread(&conn, &resolved.conversation_id, &resolved.project_id, &handle, &handle, "old agent row", "thread")
            .expect("old row");
        item.id
    };
    for id in [&stamped_id, &old_id] {
        let r = dispatch_post_as(
            "/cli/thread/message/delete",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "id": id }).to_string().as_bytes(),
            crate::session_author::OWNER_TOKEN,
        );
        assert_error_code(&r, "409 Conflict", "not_a_persons_message");
    }
    let shell = post_as(&handle, serde_json::json!({ "text": "from my shell", "via": "thread" }), "ada");
    let r = dispatch_post_as(
        "/cli/thread/message/delete",
        &HashMap::new(),
        serde_json::json!({ "addr": handle, "id": id_of(&shell) }).to_string().as_bytes(),
        "ada",
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
}

/// K2's own notes and notices (written inside the daemon) stay author-less,
/// from the room.
#[test]
fn k2s_own_notes_stay_author_less() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, _) = room("authk");
    let id = post_workspace_notice(&handle, &handle, "a notice from K2").expect("notice");
    let doc = stored(&handle, &id);
    assert!(doc.get("author").is_none(), "{doc}");
    assert_eq!(doc["from"], serde_json::json!(handle), "{doc}");
    post_workspace_note(&handle, "a CI note", "ci").expect("note");
    let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET thread"));
    let note = snap["items"].as_array().expect("items").iter().find(|i| i["doc"]["body"] == "a CI note").expect("note")["doc"].clone();
    assert!(note.get("author").is_none(), "{note}");
}
