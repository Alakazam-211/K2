//! Delete message (thread-message-delete-v1, 0.46.11). Rosson: "If a user
//! enters something sensitive into the threads, we need to give them a way
//! to delete the sensitive message. If they delete a sensitive message, we
//! can tell the agent about it as well."
//!
//! - The sender deletes: no store doc holds the text, the read and every
//!   open view get the tombstone (one frame), the preview says "Message
//!   deleted", the uploaded copy and its thumbnail go, the compose history
//!   forgets it, and the agent gets one line that never quotes it.
//! - Deleting twice is fine and tells the agent once.
//! - Someone else, an agent passport and a guest on another's post are
//!   refused and nothing changes; the owner may delete anyone's post.
//! - A delivery of the post still waiting on the lane is dropped.

use super::*;

const ROUTE: &str = "/cli/thread/message/delete";

fn ws(prefix: &str) -> (String, String, std::path::PathBuf) {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    let root = std::path::PathBuf::from(crate::thread_attachments::project_root(&project_id).expect("root"));
    std::fs::create_dir_all(root.join(".k2/downloads")).expect("downloads dir");
    (handle, project_id, root)
}

fn post_as(who: &str, handle: &str, text: &str, attachments: Option<serde_json::Value>) -> serde_json::Value {
    let mut body = serde_json::json!({ "addr": handle, "text": text, "via": "compose" });
    if let Some(a) = attachments {
        body["attachments"] = a;
    }
    let r = dispatch_post_as("/cli/thread/post", &HashMap::new(), body.to_string().as_bytes(), who);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    json_body(&r)
}

fn delete_as(who: &str, params: &HashMap<String, String>, body: serde_json::Value) -> CliResponse {
    dispatch_post_as(ROUTE, params, body.to_string().as_bytes(), who)
}

fn served_doc(handle: &str, id: &str) -> serde_json::Value {
    let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle)])).expect("GET /cli/thread"));
    snap["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|i| i["id"] == id)
        .map(|i| i["doc"].clone())
        .unwrap_or_else(|| panic!("{id} not in the Thread read: {snap}"))
}

fn notes_for(handle: &str) -> Vec<String> {
    recorded_injects()
        .into_iter()
        .filter(|l| l.contains(&format!("[thread:{handle}]")) && l.contains(" deleted a message "))
        .collect()
}

#[test]
fn the_sender_deletes_and_every_trace_goes() {
    let _home = crate::test_support::TempHome::new();
    let (handle, project_id, root) = ws("ovldel");
    let secret = format!("hunter2-{}", uuid::Uuid::new_v4());
    let rel = format!(".k2/downloads/notes-{}.txt", &uuid::Uuid::new_v4().to_string()[..8]);
    std::fs::write(root.join(&rel), "lunch order").expect("upload copy");
    let kept_rel = format!("plan-{}.md", &uuid::Uuid::new_v4().to_string()[..8]);
    std::fs::write(root.join(&kept_rel), "the plan").expect("workspace file");
    let posted = post_as(
        "owner",
        &handle,
        &format!("my password is {secret}"),
        Some(serde_json::json!([{ "path": rel }, { "path": kept_rel }])),
    );
    let id = posted["id"].as_str().expect("id").to_string();
    let seq = posted["seq"].as_i64().expect("seq");
    assert!(k2_core::overlay::store::debug_docs_contain(&secret).expect("scan"), "stored before the delete");
    assert!(
        k2_core::workspace_compose_history::list_compose_send_history(&project_id).expect("history").iter().any(|e| e.body.contains(&secret)),
        "the send is in the compose history before the delete"
    );
    // A thumbnail made for the upload while it was on screen.
    let meta = std::fs::metadata(root.join(&rel)).expect("meta");
    let thumb_key = k2_core::overlay::thumbs::cache_key(&project_id, &rel, meta.len(), k2_core::overlay::attachments::modified_ms(&meta), k2_core::overlay::thumbs::PX_SMALL);
    let thumbs = k2_core::overlay::thumbs::cache_dir();
    std::fs::create_dir_all(&thumbs).expect("thumb dir");
    std::fs::write(thumbs.join(format!("{thumb_key}.png")), b"png").expect("thumb");
    let frames = crate::overlay_ws::subscribe();
    let notes_before = notes_for(&handle).len();

    let r = delete_as("owner", &HashMap::new(), serde_json::json!({ "addr": handle, "seq": seq }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = json_body(&r);
    assert_eq!((v["id"].as_str(), v["seq"].as_i64(), v["deleted"].as_bool(), v["already"].as_bool()), (Some(id.as_str()), Some(seq), Some(true), Some(false)), "{v}");

    assert!(!k2_core::overlay::store::debug_docs_contain(&secret).expect("scan"), "no store doc holds the deleted text");
    let doc = served_doc(&handle, &id);
    assert_eq!(doc["deleted"], true, "{doc}");
    assert!(doc.get("body").is_none() && doc.get("attachments").is_none(), "the read serves no content: {doc}");
    assert!(doc["deletedBy"]["kind"] == "owner" && doc["deletedAt"].is_i64(), "{doc}");
    assert_eq!(doc["author"]["kind"], "owner", "whose it was stays: {doc}");

    let stored = k2_core::overlay::get_doc(&id).expect("get").expect("doc");
    assert_eq!(thread_preview(&stored), "Message deleted", "the list preview never shows the text");

    let mut frames = frames;
    let mut framed = false;
    loop {
        match frames.try_recv() {
            Ok(f) => {
                if f.id == id && f.doc.as_ref().is_some_and(|d| d.deleted && d.body.is_none()) {
                    framed = true;
                }
            }
            // Other tests publish on the same bus.
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }
    assert!(framed, "every open view gets the tombstone frame");

    assert!(!root.join(&rel).exists(), "the copy K2 uploaded for the message is removed");
    assert!(root.join(&kept_rel).exists(), "a workspace file the message only pointed at stays");
    assert!(!thumbs.join(format!("{thumb_key}.png")).exists(), "its thumbnail is removed");
    assert!(
        !k2_core::workspace_compose_history::list_compose_send_history(&project_id).expect("history").iter().any(|e| e.body.contains(&secret)),
        "the compose history forgets it"
    );

    let notes = notes_for(&handle);
    assert_eq!(notes.len(), notes_before + 1, "the agent is told once: {notes:?}");
    let note = notes.last().expect("note");
    assert!(note.contains("deleted a message they sent at") && note.contains("Don't repeat, store or act on what it said."), "{note}");
    for line in recorded_injects().iter().filter(|l| l.contains(" deleted a message ")) {
        assert!(!line.contains(&secret) && !line.contains("password"), "a note never carries the deleted text: {line}");
    }

    // Idempotent: a second delete (by id) answers `already` and tells nobody.
    let again = delete_as("owner", &HashMap::new(), serde_json::json!({ "addr": handle, "id": id }));
    assert_eq!(again.status, "200 OK", "{}", again.body);
    assert_eq!(json_body(&again)["already"], true);
    assert_eq!(notes_for(&handle).len(), notes_before + 1, "no second note");
}

#[test]
fn only_the_sender_or_the_owner_deletes_and_agents_never_do() {
    let _home = crate::test_support::TempHome::new();
    let (handle, project_id, _root) = ws("ovldel2");
    let owners = post_as("owner", &handle, "owner's words", None);
    let owners_id = owners["id"].as_str().expect("id").to_string();
    let adas = post_as("ada", &handle, "ada's words", None);
    let adas_seq = adas["seq"].as_i64().expect("seq");

    // GET is 405 and changes nothing.
    let get = dispatch(ROUTE, &params_of(&[("addr", &handle), ("id", &owners_id)])).expect("routed");
    assert_eq!(get.status, "405 Method Not Allowed", "{}", get.body);

    // Another login can't delete the owner's post.
    let r = delete_as("eve", &HashMap::new(), serde_json::json!({ "addr": handle, "id": owners_id }));
    assert_error_code(&r, "403 Forbidden", "not_your_message");
    // An agent passport can't delete a person's post, even its owner's.
    let r = delete_as("owner", &passport_params(&project_id, &handle), serde_json::json!({ "addr": handle, "id": owners_id }));
    assert_error_code(&r, "403 Forbidden", "person_only");
    assert!(json_body(&r)["error"]["hint"].as_str().expect("hint").contains("only the person who sent it can delete it"), "{}", r.body);
    assert_eq!(served_doc(&handle, &owners_id)["body"], "owner's words", "a refusal changes nothing");
    // An agent's own reply isn't a person's message.
    let reply = post_json("/cli/thread/post", &passport_params(&project_id, &handle), serde_json::json!({ "addr": handle, "text": "agent words" }));
    assert_eq!(reply.status, "200 OK", "{}", reply.body);
    let r = delete_as("owner", &HashMap::new(), serde_json::json!({ "addr": handle, "id": json_body(&reply)["id"] }));
    assert_error_code(&r, "409 Conflict", "not_a_persons_message");
    // Unknown message, and a bad request.
    let r = delete_as("owner", &HashMap::new(), serde_json::json!({ "addr": handle, "seq": 999 }));
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
    let r = delete_as("owner", &HashMap::new(), serde_json::json!({ "addr": handle }));
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);

    // Ada deletes her own; the owner may delete anyone's (the note names whose).
    let r = delete_as("ada", &HashMap::new(), serde_json::json!({ "addr": handle, "seq": adas_seq }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(notes_for(&handle).iter().any(|l| l.contains("[from ada]") && l.contains("ada deleted a message they sent at")), "{:?}", notes_for(&handle));
    let r = delete_as("ada", &HashMap::new(), serde_json::json!({ "addr": handle, "id": owners_id }));
    assert_error_code(&r, "403 Forbidden", "not_your_message");
    let late = post_as("ada", &handle, "ada again", None);
    let r = delete_as("owner", &HashMap::new(), serde_json::json!({ "addr": handle, "id": late["id"] }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(notes_for(&handle).iter().any(|l| l.contains("deleted a message ada sent at")), "{:?}", notes_for(&handle));
}

#[test]
fn an_app_guest_deletes_only_its_own_posts() {
    let _home = crate::test_support::TempHome::new();
    let (handle, project_id, _root) = ws("ovldel3");
    let owners = post_as("owner", &handle, "owner's words", None);
    let pass = skin_pass(&[project_id.clone()]);
    let guest_post = with_request_skin(Some(pass.clone()), || {
        dispatch_post("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": handle, "text": "guest words" }).to_string().as_bytes())
    });
    assert_eq!(guest_post.status, "200 OK", "{}", guest_post.body);
    let guest_id = json_body(&guest_post)["id"].as_str().expect("id").to_string();

    let r = with_request_skin(Some(pass.clone()), || delete_as("guest", &HashMap::new(), serde_json::json!({ "addr": handle, "id": owners["id"] })));
    assert_error_code(&r, "403 Forbidden", "not_your_message");
    let r = with_request_skin(Some(pass.clone()), || delete_as("guest", &HashMap::new(), serde_json::json!({ "addr": handle, "id": guest_id })));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(served_doc(&handle, &guest_id)["deleted"], true);
    let note = recorded_injects().into_iter().rev().find(|l| l.contains(&format!("[thread:{handle}]")) && l.contains(" deleted a message ")).expect("guest note");
    assert!(note.starts_with("[guest "), "a guest's note keeps the guest stamp: {note}");
    assert!(!note.contains("guest words"), "{note}");
}

/// A post's delivery still waiting on the agent's lane when the person
/// deletes it is never typed in (or retried, or quoted in a notice).
#[test]
fn a_waiting_delivery_of_a_deleted_post_is_dropped() {
    let _home = crate::test_support::TempHome::new();
    let (handle, _project_id, _root) = ws("ovldel4");
    let secret = format!("waiting-{}", uuid::Uuid::new_v4());
    let posted = post_as("owner", &handle, &secret, None);
    let id = posted["id"].as_str().expect("id").to_string();
    let resolved = match resolve_thread_addr(&handle) {
        Ok(r) => r,
        Err(e) => panic!("resolve {handle}: {}", e.body),
    };
    let job = ThreadDeliveryJob::new(
        &resolved,
        format!("[thread:{handle}] {secret} (again)"),
        "owner",
        "compose",
        "",
        Some(ThreadTurn::new(&resolved, &id, 0)),
    );
    let r = delete_as("owner", &HashMap::new(), serde_json::json!({ "addr": handle, "id": id }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let out = job.clone().run();
    assert!(!out.success, "not delivered");
    assert!(
        !recorded_deliveries(&handle).iter().any(|p| p.contains(&format!("{secret} (again)"))),
        "the waiting line never reaches the agent"
    );
    assert!(job.into_retry().source_deleted(), "the retry knows it too");
}

/// An upload swapped for a link to a workspace file after it was sent: the
/// delete never follows the link into the workspace.
#[cfg(unix)]
#[test]
fn a_link_in_downloads_never_deletes_the_file_it_points_at() {
    let _home = crate::test_support::TempHome::new();
    let (handle, _project_id, root) = ws("ovldel5");
    let rel = format!(".k2/downloads/notes-{}.txt", &uuid::Uuid::new_v4().to_string()[..8]);
    std::fs::write(root.join(&rel), "lunch order").expect("upload copy");
    let posted = post_as("owner", &handle, "see the notes", Some(serde_json::json!([{ "path": rel }])));
    let target = root.join("keep-me.md");
    std::fs::write(&target, "workspace file").expect("workspace file");
    std::fs::remove_file(root.join(&rel)).expect("rm upload");
    std::os::unix::fs::symlink(&target, root.join(&rel)).expect("link");
    let r = delete_as("owner", &HashMap::new(), serde_json::json!({ "addr": handle, "id": posted["id"] }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(std::fs::read_to_string(&target).expect("still there"), "workspace file");
}

#[test]
fn the_note_names_people_and_a_time_never_the_text() {
    assert_eq!(
        message_delete::agent_note("ada", None, 0),
        "ada deleted a message they sent at 1970-01-01 00:00 UTC. Don't repeat, store or act on what it said."
    );
    assert_eq!(
        message_delete::agent_note("Rosson", Some("ada"), 60),
        "Rosson deleted a message ada sent at 1970-01-01 00:01 UTC. Don't repeat, store or act on what it said."
    );
    assert!(message_delete::is_uploaded_copy(".k2/downloads/a.png"));
    assert!(message_delete::is_uploaded_copy(".k2so/downloads/a.png"));
    assert!(!message_delete::is_uploaded_copy("src/main.rs"));
    assert!(!message_delete::is_uploaded_copy(".k2/downloads/../../src/main.rs"));
}
