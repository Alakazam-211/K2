//! Route tests for the Changes tab v1.1 review notes (C5): CRUD, role
//! gates, agent access, outdated / moved anchors, send (format, deliver
//! once, mark sent, failure keeps notes), the `git_notes_changed` event,
//! plus `open-changes` and the route-policy rows.

use super::*;
use crate::session_events::SessionEvent;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-notes-{tag}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&d).expect("mkdir temp");
    d
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=K2 Test", "-c", "user.email=k2-test@example.com", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A committed repo registered as a K2 workspace (projects row) in the
/// thread's scoped DB. a.txt = one..five, then line 2 edited.
fn workspace(tag: &str) -> (PathBuf, String) {
    let d = tmp(tag);
    git(&d, &["init", "-q"]);
    std::fs::write(d.join("a.txt"), "one\ntwo\nthree\nfour\nfive\n").expect("a");
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "init"]);
    std::fs::write(d.join("a.txt"), "one\nTWO\nthree\nfour\nfive\n").expect("edit");
    let path = d.to_str().expect("utf8").to_string();
    let id = format!("p-{}", uuid::Uuid::new_v4().simple());
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)", rusqlite::params![id, tag, path])
        .expect("project row");
    (d, path)
}

fn json(r: &CliResponse) -> serde_json::Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {} {}", r.status, r.body))
}

fn human(actor: &str, admin: bool) -> Caller {
    Caller::Human { actor: actor.to_string(), name: actor.trim_start_matches("user:").to_string(), admin }
}

fn owner() -> Caller {
    human("owner-token", true)
}

fn agent(own: &str) -> Caller {
    Caller::Agent { actor: "agent:k2".into(), own_workspace: own.to_string() }
}

fn no_deliver(_: &DeliverRequest<'_>) -> DeliverResult {
    panic!("this test must not deliver")
}

fn post(path: &str, body: serde_json::Value, caller: &Caller) -> CliResponse {
    dispatch_post(path, body.to_string().as_bytes(), caller, &no_deliver)
}

fn list(ws: &str, caller: &Caller, extra: &[(&str, &str)]) -> serde_json::Value {
    let mut p: HashMap<String, String> = extra.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    p.insert("path".into(), ws.to_string());
    let r = handle_list(&p, caller);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    json(&r)
}

fn add(ws: &str, file: &str, line: i64, body: &str, caller: &Caller) -> String {
    let r = post(
        "/cli/git/notes/add",
        serde_json::json!({ "path": ws, "filePath": file, "startLine": line, "body": body }),
        caller,
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    json(&r)["note"]["id"].as_str().expect("id").to_string()
}

fn notes_events(rx: &mut tokio::sync::broadcast::Receiver<SessionEvent>, ws: &str) -> usize {
    let mut n = 0;
    while let Ok(ev) = rx.try_recv() {
        if let SessionEvent::GitNotesChanged { workspace_path } = ev {
            if workspace_path == ws {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn people_add_list_edit_resolve_delete_with_events() {
    let _db = k2_core::db::scoped_for_test();
    let (_d, ws) = workspace("crud");
    let mut rx = crate::session_events::subscribe();
    while rx.try_recv().is_ok() {}

    let ana = human("user:ana", false);
    let r = post(
        "/cli/git/notes/add",
        serde_json::json!({ "path": format!("{ws}/"), "filePath": "a.txt", "startLine": 2, "endLine": 3, "body": "  Why upper case?\r\n" }),
        &ana,
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let note = json(&r)["note"].clone();
    let id = note["id"].as_str().expect("id").to_string();
    assert!(id.starts_with("n_"), "{id}");
    assert_eq!(note["workspacePath"], ws, "trailing slash trimmed");
    assert_eq!(note["path"], "a.txt");
    assert_eq!(note["side"], "new");
    assert_eq!((note["startLine"].as_i64(), note["endLine"].as_i64()), (Some(2), Some(3)));
    assert_eq!(note["body"], "Why upper case?");
    assert_eq!(note["author"], "user:ana");
    assert_eq!(note["authorName"], "ana");
    assert_eq!(note["anchorText"], "TWO\nthree", "anchored on the working-tree text");
    assert_eq!(note["outdated"], false);
    assert!(note["rev"].as_str().is_some_and(|r| !r.is_empty()));
    assert_eq!(note["blob"].as_str().map(str::len), Some(40), "{note}");
    assert_eq!(notes_events(&mut rx, &ws), 1);

    // A second note, on the old side, from someone else.
    let bo = human("user:bo", false);
    let r = post(
        "/cli/git/notes/add",
        serde_json::json!({ "path": ws, "filePath": format!("{ws}/a.txt"), "side": "old", "startLine": 2, "body": "was lower" }),
        &bo,
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json(&r)["note"]["anchorText"], "two", "old side = HEAD text");
    let id2 = json(&r)["note"]["id"].as_str().expect("id").to_string();

    let l = list(&ws, &ana, &[]);
    assert_eq!(l["notes"].as_array().expect("notes").len(), 2);
    assert_eq!(l["counts"]["open"], 2);
    assert_eq!(l["counts"]["unsent"], 2);
    assert_eq!(list(&ws, &ana, &[("file_path", "a.txt")])["notes"].as_array().expect("n").len(), 2);
    assert_eq!(list(&ws, &ana, &[("file_path", "b.txt")])["notes"].as_array().expect("n").len(), 0);

    // Edit: author yes, another Member no, an Admin yes.
    let r = post("/cli/git/notes/update", serde_json::json!({ "id": id, "body": "Why UPPER?" }), &bo);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert_eq!(json(&r)["code"], "not_author");
    let r = post("/cli/git/notes/update", serde_json::json!({ "id": id, "body": "Why UPPER?" }), &ana);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json(&r)["note"]["body"], "Why UPPER?");
    let r = post("/cli/git/notes/update", serde_json::json!({ "id": id, "body": "  " }), &ana);
    assert_eq!(r.status, "400 Bad Request");
    assert_eq!(post("/cli/git/notes/update", serde_json::json!({ "id": id, "body": "x" }), &human("user:admin", true)).status, "200 OK");

    // Resolve (anyone), reopen, resolve again.
    let r = post("/cli/git/notes/resolve", serde_json::json!({ "id": id }), &bo);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json(&r)["changed"], serde_json::json!([id]));
    let l = list(&ws, &ana, &[("status", "resolved")]);
    assert_eq!(l["notes"][0]["resolvedBy"], "user:bo");
    assert_eq!(list(&ws, &ana, &[])["notes"].as_array().expect("open").len(), 1);
    let r = post("/cli/git/notes/resolve", serde_json::json!({ "id": id, "resolved": false }), &ana);
    assert_eq!(json(&r)["changed"], serde_json::json!([id]));
    assert_eq!(list(&ws, &ana, &[("status", "unsent")])["notes"].as_array().expect("n").len(), 2);

    // Delete: author or Admin.
    let r = post("/cli/git/notes/delete", serde_json::json!({ "ids": [id2] }), &ana);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let r = post("/cli/git/notes/delete", serde_json::json!({ "ids": [id2] }), &bo);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(list(&ws, &ana, &[("status", "all")])["notes"].as_array().expect("n").len(), 1);
    // second add + update + admin update + resolve + reopen + delete = 6.
    assert_eq!(notes_events(&mut rx, &ws), 6);

    // Unknown ids and bad filters.
    assert_eq!(post("/cli/git/notes/resolve", serde_json::json!({ "id": "n_nope" }), &ana).status, "404 Not Found");
    assert_eq!(post("/cli/git/notes/resolve", serde_json::json!({}), &ana).status, "400 Bad Request");
    let mut p = HashMap::new();
    p.insert("path".to_string(), ws.clone());
    p.insert("status".to_string(), "closed".to_string());
    assert_eq!(handle_list(&p, &ana).status, "400 Bad Request");
    assert_eq!(handle_list(&HashMap::new(), &ana).status, "400 Bad Request", "people name the workspace");
}

#[test]
fn add_validates_the_workspace_file_lines_and_body() {
    let _db = k2_core::db::scoped_for_test();
    let (d, ws) = workspace("addval");
    let o = owner();
    let bad = |body: serde_json::Value| post("/cli/git/notes/add", body, &o);

    let r = bad(serde_json::json!({ "path": ws, "filePath": "../../etc/passwd", "startLine": 1, "body": "x" }));
    assert_eq!((r.status, json(&r)["code"].clone()), ("400 Bad Request", serde_json::json!("bad_path")));
    let r = bad(serde_json::json!({ "path": ws, "filePath": "a.txt", "startLine": 0, "body": "x" }));
    assert_eq!(r.status, "400 Bad Request");
    let r = bad(serde_json::json!({ "path": ws, "filePath": "a.txt", "startLine": 3, "endLine": 2, "body": "x" }));
    assert_eq!(r.status, "400 Bad Request");
    let r = bad(serde_json::json!({ "path": ws, "filePath": "a.txt", "startLine": 9, "body": "x" }));
    assert_eq!(r.status, "400 Bad Request");
    assert!(json(&r)["error"].as_str().expect("hint").contains("past the end"), "{}", r.body);
    let r = bad(serde_json::json!({ "path": ws, "filePath": "a.txt", "startLine": 1, "body": "\u{1b}\u{7} " }));
    assert_eq!(r.status, "400 Bad Request", "control chars only = empty");
    let r = bad(serde_json::json!({ "path": ws, "filePath": "a.txt", "side": "left", "startLine": 1, "body": "x" }));
    assert_eq!(r.status, "400 Bad Request");
    let r = bad(serde_json::json!({ "path": ws, "filePath": "a.txt", "startLine": 1, "body": "x".repeat(git_notes::MAX_BODY_CHARS + 1) }));
    assert_eq!(r.status, "400 Bad Request");

    // Not a K2 workspace, and a registered folder that is not a repo.
    let stray = tmp("stray");
    let r = bad(serde_json::json!({ "path": stray.to_str().expect("utf8"), "filePath": "a.txt", "startLine": 1, "body": "x" }));
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
    let plain = d.join("sub-not-repo");
    std::fs::create_dir_all(&plain).expect("mkdir");

    // A client-sent anchor of the right size wins; a wrong-size one is ignored.
    let r = bad(serde_json::json!({ "path": ws, "filePath": "a.txt", "startLine": 1, "endLine": 2, "body": "x", "anchorText": "one\ntwo", "rev": "r:client" }));
    assert_eq!(json(&r)["note"]["anchorText"], "one\ntwo");
    assert_eq!(json(&r)["note"]["rev"], "r:client");
    let r = bad(serde_json::json!({ "path": ws, "filePath": "a.txt", "startLine": 1, "body": "x", "anchorText": "a\nb\nc" }));
    assert_eq!(json(&r)["note"]["anchorText"], "one");

    // A worktree-like folder inside the workspace belongs to it.
    let r = bad(serde_json::json!({ "path": format!("{ws}/sub-not-repo"), "filePath": "a.txt", "startLine": 1, "body": "x" }));
    assert_eq!(r.status, "200 OK", "a sub-folder of the repo resolves to the repo: {}", r.body);
}

#[test]
fn notes_follow_moved_lines_and_flag_rewritten_ones() {
    let _db = k2_core::db::scoped_for_test();
    let (d, ws) = workspace("anchor");
    let o = owner();
    let id_moved = add(&ws, "a.txt", 4, "four", &o);
    let id_gone = add(&ws, "a.txt", 2, "upper two", &o);

    std::fs::write(d.join("a.txt"), "zero\nzero\none\ntwo again\nthree\nfour\nfive\n").expect("rewrite");
    let l = list(&ws, &o, &[]);
    let by_id = |id: &str| l["notes"].as_array().expect("notes").iter().find(|n| n["id"] == id).expect("note").clone();
    let moved = by_id(&id_moved);
    assert_eq!(moved["outdated"], false);
    assert_eq!((moved["currentStartLine"].as_i64(), moved["currentEndLine"].as_i64()), (Some(6), Some(6)));
    assert_eq!(moved["startLine"], 4, "the stored line is the one it was made on");
    let gone = by_id(&id_gone);
    assert_eq!(gone["outdated"], true);
    assert!(gone["currentStartLine"].is_null());

    // A staged row's new side is the index: a note made there stays current
    // while the working tree moves on.
    std::fs::write(d.join("a.txt"), "one\nTWO\nthree\nfour\nfive\n").expect("back");
    git(&d, &["add", "a.txt"]);
    let staged = add(&ws, "a.txt", 2, "staged TWO", &o);
    std::fs::write(d.join("a.txt"), "one\nchanged again\nthree\nfour\nfive\n").expect("work edit");
    let l = list(&ws, &o, &[]);
    let n = l["notes"].as_array().expect("notes").iter().find(|n| n["id"] == staged).expect("note").clone();
    assert_eq!(n["outdated"], false, "found in the index copy: {n}");
}

#[test]
fn agents_list_and_resolve_their_own_notes_only() {
    let _db = k2_core::db::scoped_for_test();
    let (_d, ws) = workspace("agent");
    let (_o, other) = workspace("agent-other");
    let o = owner();
    let mine = add(&ws, "a.txt", 2, "fix this", &o);
    let theirs = add(&other, "a.txt", 2, "not yours", &o);
    let a = agent(&ws);

    // List with the path left out = own workspace.
    let r = handle_list(&HashMap::new(), &a);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json(&r)["notes"][0]["id"], mine);
    let mut p = HashMap::new();
    p.insert("path".to_string(), other.clone());
    assert_eq!(handle_list(&p, &a).status, "403 Forbidden");

    // Agents never write or send notes.
    for (route, body) in [
        ("/cli/git/notes/add", serde_json::json!({ "path": ws, "filePath": "a.txt", "startLine": 1, "body": "x" })),
        ("/cli/git/notes/update", serde_json::json!({ "id": mine, "body": "x" })),
        ("/cli/git/notes/delete", serde_json::json!({ "id": mine })),
        ("/cli/git/notes/send", serde_json::json!({ "path": ws })),
    ] {
        let r = post(route, body, &a);
        assert_eq!(r.status, "403 Forbidden", "{route}: {}", r.body);
        assert_eq!(json(&r)["code"], "forbidden");
    }
    // Resolve: own yes, another workspace's no (all-or-nothing), reopen no.
    let r = post("/cli/git/notes/resolve", serde_json::json!({ "ids": [mine, theirs] }), &a);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert_eq!(list(&ws, &o, &[])["counts"]["open"], 1, "nothing changed");
    let r = post("/cli/git/notes/resolve", serde_json::json!({ "id": mine }), &a);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(list(&ws, &o, &[("status", "resolved")])["notes"][0]["resolvedBy"], "agent:k2");
    assert_eq!(post("/cli/git/notes/resolve", serde_json::json!({ "id": mine, "resolved": false }), &a).status, "403 Forbidden");

    // Through the cell-socket entry point (git_routes::handle_agent_route_as).
    let r = crate::git_routes::handle_agent_route_as("/cli/git/notes", false, &HashMap::new(), b"", &ws, "agent:k2");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json(&r)["counts"]["resolved"], 1);
    let r = crate::git_routes::handle_agent_route_as("/cli/git/notes/resolve", false, &HashMap::new(), b"", &ws, "agent:k2");
    assert_eq!(r.status, "405 Method Not Allowed");
    let r = crate::git_routes::handle_agent_route_as("/cli/git/notes/add", true, &HashMap::new(), b"{}", &ws, "agent:k2");
    assert_eq!(r.status, "404 Not Found", "add is not an agent route");
    for p in ["/cli/git/notes", "/cli/git/notes/resolve", "/cli/git/open-changes"] {
        assert!(crate::session_token::is_agent_verb(p), "{p}");
    }
    for p in ["/cli/git/notes/add", "/cli/git/notes/send", "/cli/git/notes/update", "/cli/git/notes/delete"] {
        assert!(!crate::session_token::is_agent_verb(p), "{p}");
    }
}

#[test]
fn send_batches_open_unsent_notes_into_one_message_and_delivers_once() {
    let _db = k2_core::db::scoped_for_test();
    let (_d, ws) = workspace("send");
    let o = human("user:ana", false);
    let n1 = add(&ws, "a.txt", 2, "Why upper case?", &o);
    let n2 = add(&ws, "a.txt", 4, "Explain\nthis line", &o);
    let resolved = add(&ws, "a.txt", 5, "done already", &o);
    assert_eq!(post("/cli/git/notes/resolve", serde_json::json!({ "id": resolved }), &o).status, "200 OK");

    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::<(String, String, String, String, bool)>::new()));
    let (c, s) = (calls.clone(), seen.clone());
    let fake = move |req: &DeliverRequest<'_>| -> DeliverResult {
        c.fetch_add(1, Ordering::SeqCst);
        s.lock().expect("seen").push((
            req.target.to_string(),
            req.text.to_string(),
            req.from.to_string(),
            req.actor.to_string(),
            req.wake,
        ));
        DeliverResult { success: true, delivery_id: Some("m_test1".into()), reason: None, hint: None }
    };
    let mut rx = crate::session_events::subscribe();
    while rx.try_recv().is_ok() {}

    let body = serde_json::json!({ "path": ws }).to_string();
    let r = dispatch_post("/cli/git/notes/send", body.as_bytes(), &o, &fake);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = json(&r);
    assert_eq!(v["sent"], 2);
    assert_eq!(v["ids"], serde_json::json!([n1, n2]));
    assert_eq!(v["deliveryId"], "m_test1");
    assert_eq!(v["remaining"], 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let (target, text, from, actor, wake) = seen.lock().expect("seen")[0].clone();
    assert_eq!(target, ws, "the workspace's agent");
    assert_eq!((from.as_str(), actor.as_str(), wake), ("ana", "user:ana", true));
    assert!(text.starts_with(&format!("Review notes on your changes in {ws} (2 notes).")), "{text}");
    assert!(text.contains(&format!("File: a.txt\nLine: 2\nComment: Why upper case?\nNote id: {n1}")), "{text}");
    assert!(text.contains(&format!("File: a.txt\nLine: 4\nComment: Explain\n  this line\nNote id: {n2}")), "{text}");
    assert!(!text.contains("done already"), "resolved notes stay out");
    assert_eq!(notes_events(&mut rx, &ws), 1);

    let l = list(&ws, &o, &[("status", "sent")]);
    assert_eq!(l["notes"].as_array().expect("sent").len(), 2);
    assert_eq!(l["notes"][0]["deliveryId"], "m_test1");
    assert!(l["notes"][0]["sentAt"].as_i64().is_some());

    // Nothing left: no second delivery.
    let r = dispatch_post("/cli/git/notes/send", body.as_bytes(), &o, &fake);
    assert_eq!(r.status, "409 Conflict", "{}", r.body);
    assert_eq!(json(&r)["code"], "nothing_to_send");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "delivered once");

    // A failed delivery keeps the note unsent and says why.
    let n3 = add(&ws, "a.txt", 3, "third", &o);
    let failing = |_: &DeliverRequest<'_>| DeliverResult {
        success: false,
        delivery_id: None,
        reason: Some("dormant_no_wake".into()),
        hint: Some("the agent is asleep".into()),
    };
    let r = dispatch_post("/cli/git/notes/send", serde_json::json!({ "path": ws, "wake": false }).to_string().as_bytes(), &o, &failing);
    assert_eq!(r.status, "502 Bad Gateway", "{}", r.body);
    assert_eq!(json(&r)["code"], "not_delivered");
    assert_eq!(json(&r)["reason"], "dormant_no_wake");
    assert_eq!(list(&ws, &o, &[("status", "unsent")])["notes"][0]["id"], n3);

    // `ids` picks a subset; a send while one is running is refused.
    let n4 = add(&ws, "a.txt", 1, "fourth", &o);
    let ws2 = ws.clone();
    let o2 = o.clone();
    let reentrant = move |_: &DeliverRequest<'_>| -> DeliverResult {
        let inner = dispatch_post("/cli/git/notes/send", serde_json::json!({ "path": ws2 }).to_string().as_bytes(), &o2, &no_deliver);
        assert_eq!(inner.status, "409 Conflict", "{}", inner.body);
        assert_eq!(json(&inner)["code"], "send_in_progress");
        DeliverResult { success: true, delivery_id: Some("m_test2".into()), reason: None, hint: None }
    };
    let r = dispatch_post("/cli/git/notes/send", serde_json::json!({ "path": ws, "ids": [n4] }).to_string().as_bytes(), &o, &reentrant);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json(&r)["ids"], serde_json::json!([n4]));
    assert_eq!(list(&ws, &o, &[("status", "unsent")])["notes"][0]["id"], n3, "n3 still waits");

    // Bad requests.
    assert_eq!(dispatch_post("/cli/git/notes/send", b"{}", &o, &no_deliver).status, "400 Bad Request");
    let stray = tmp("send-stray");
    let r = dispatch_post(
        "/cli/git/notes/send",
        serde_json::json!({ "path": stray.to_str().expect("utf8") }).to_string().as_bytes(),
        &o,
        &no_deliver,
    );
    assert_eq!(r.status, "404 Not Found");
}

#[test]
fn open_changes_emits_for_the_folder_and_is_post_only() {
    let d = tmp("open");
    let ws = d.to_str().expect("utf8").to_string();
    let mut rx = crate::session_events::subscribe();
    while rx.try_recv().is_ok() {}
    let r = crate::git_routes::dispatch_unit4_git_post("/cli/git/open-changes", serde_json::json!({ "path": format!("{ws}/") }).to_string().as_bytes());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json(&r)["workspacePath"], ws);
    let mut got = 0;
    while let Ok(ev) = rx.try_recv() {
        if let SessionEvent::OpenChanges { workspace_path } = ev {
            if workspace_path == ws {
                got += 1;
            }
        }
    }
    assert_eq!(got, 1);
    let r = crate::git_routes::dispatch_unit4_git_post("/cli/git/open-changes", b"{}");
    assert_eq!(r.status, "400 Bad Request");
    let r = crate::git_routes::dispatch_unit4_git_post("/cli/git/open-changes", br#"{"path":"/no/such/k2/folder"}"#);
    assert_eq!(r.status, "404 Not Found");
    // GET (the read dispatch) → 405, for open-changes and every notes write.
    for p in ["/cli/git/open-changes", "/cli/git/notes/add", "/cli/git/notes/send", "/cli/git/notes/resolve"] {
        let r = crate::cli::dispatch(p, &HashMap::new());
        assert_eq!(r.status, "405 Method Not Allowed", "{p}");
    }
    // Agents: the path left out = own workspace.
    let r = crate::git_routes::handle_agent_route_as("/cli/git/open-changes", true, &HashMap::new(), b"{}", &ws, "agent:k2");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let other = tmp("open-other");
    let body = serde_json::json!({ "path": other.to_str().expect("utf8") }).to_string();
    let r = crate::git_routes::handle_agent_route_as("/cli/git/open-changes", true, &HashMap::new(), body.as_bytes(), &ws, "agent:k2");
    assert_eq!(r.status, "403 Forbidden");
}

/// C5 scope add: `git/init` at a workspace root only, once, then the
/// snapshot is a repo and `git_changed` fired; `git/info` reports git.
#[test]
fn git_init_makes_the_workspace_root_a_repo_once() {
    let _db = k2_core::db::scoped_for_test();
    let d = tmp("init");
    std::fs::write(d.join("readme.md"), "hi\n").expect("file");
    let ws = d.to_str().expect("utf8").to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, 'init', ?2)", rusqlite::params![format!("p-{}", uuid::Uuid::new_v4().simple()), ws])
            .expect("project");
    }
    let info = json(&crate::cli::dispatch("/cli/git/info", &[("path".to_string(), ws.clone())].into_iter().collect()));
    assert_eq!(info["isRepo"], false);
    assert_eq!(info["gitAvailable"], true, "{info}");
    assert!(info["gitCli"].is_boolean());
    let snap = json(&crate::cli::dispatch("/cli/git/snapshot", &[("path".to_string(), ws.clone())].into_iter().collect()));
    assert_eq!(snap["isRepo"], false);

    let post_init = |p: &str| crate::git_routes::dispatch_unit4_git_post("/cli/git/init", serde_json::json!({ "path": p }).to_string().as_bytes());
    // Not the root: a sub-folder, and a folder that is no workspace.
    std::fs::create_dir_all(d.join("sub")).expect("sub");
    let r = post_init(&format!("{ws}/sub"));
    assert_eq!((r.status, json(&r)["code"].clone()), ("400 Bad Request", serde_json::json!("bad_path")), "{}", r.body);
    let stray = tmp("init-stray");
    assert_eq!(post_init(stray.to_str().expect("utf8")).status, "404 Not Found");
    assert!(!stray.join(".git").exists() && !d.join("sub/.git").exists());

    let mut rx = crate::session_events::subscribe();
    while rx.try_recv().is_ok() {}
    let r = post_init(&format!("{ws}/"));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(d.join(".git").is_dir());
    let mut fired = false;
    while let Ok(ev) = rx.try_recv() {
        if let SessionEvent::GitChanged { workspace_path, .. } = ev {
            fired |= workspace_path == ws;
        }
    }
    assert!(fired, "git_changed for the workspace");
    let snap = json(&crate::cli::dispatch("/cli/git/snapshot", &[("path".to_string(), ws.clone())].into_iter().collect()));
    assert_eq!(snap["isRepo"], true, "{snap}");
    assert_eq!(snap["unborn"], true);
    assert_eq!(snap["counts"]["untracked"], 1);

    let r = post_init(&ws);
    assert_eq!((r.status, json(&r)["code"].clone()), ("409 Conflict", serde_json::json!("already_a_repo")));
    assert_eq!(crate::cli::dispatch("/cli/git/init", &HashMap::new()).status, "405 Method Not Allowed");
    assert_eq!(post_init("").status, "400 Bad Request");

    // Agents: their own workspace root only.
    let (_o, other) = workspace("init-agent-other");
    let body = serde_json::json!({ "path": other }).to_string();
    let r = crate::git_routes::handle_agent_route_as("/cli/git/init", true, &HashMap::new(), body.as_bytes(), &ws, "agent:k2");
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let fresh = tmp("init-agent");
    let fresh_s = fresh.to_str().expect("utf8").to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, 'fresh', ?2)", rusqlite::params![format!("p-{}", uuid::Uuid::new_v4().simple()), fresh_s])
            .expect("project");
    }
    let r = crate::git_routes::handle_agent_route_as("/cli/git/init", true, &HashMap::new(), b"{}", &fresh_s, "agent:k2");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(fresh.join(".git").is_dir());
    assert!(crate::session_token::is_agent_verb("/cli/git/init"));
}

#[test]
fn route_policy_rows_for_notes_and_open_changes() {
    use crate::routes::route_policy::{lookup, post_allowed, Floor};
    let list = lookup("/cli/git/notes").expect("notes list classified");
    assert_eq!((list.get, list.post), (Some(Floor::Member), None));
    for p in POST_ROUTES.iter().chain(["/cli/git/open-changes", "/cli/git/init"].iter()) {
        let row = lookup(p).unwrap_or_else(|| panic!("{p} classified"));
        assert_eq!((row.get, row.post), (None, Some(Floor::Member)), "{p}: POST-only, Member");
        assert!(post_allowed(p));
        assert!(crate::routes::route_policy::get_refused(p, ""), "{p}: GET refused");
    }
}
