//! The Garden bridge's person rows (garden-person-rows-v1), through
//! `zen_data_routes::handle` so the door's gates run too: a temp HOME with
//! one Garden placing `board`, a scoped DB with three agents (`env()`),
//! one of them with real files.

use std::collections::HashMap;

use serde_json::{json, Value};

use k2_core::db::schema::WorkspaceSession;
use k2_core::garden_data::authors::{self, CallerKind, StampSource};
use k2_core::garden_data::MadeBy;
use k2_core::zen::{ZenFile, ZenFiles};

use crate::garden_data::agent_routes::tests::{env, Env};
use crate::zen_data_routes::{handle, DataWho};

const APP: &str = "POST /cli/zen/call HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: tauri://localhost\r\n\r\n";

const PAGE: &str = "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"b1\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = 0\n";

const ALL: [&str; 5] = ["thread:read", "tickets:read", "tickets:post", "files:read", "activity:read"];

struct Door {
    _env: Env,
    f: ZenFiles,
    garden: String,
    /// The agent with real files: `garden-files` (`w-files`).
    files_root: std::path::PathBuf,
}

fn manifest(caps: &[&str], tag: &str) -> String {
    let caps: Vec<String> = caps.iter().map(|c| format!("\"{c}\"")).collect();
    format!(
        "{{\n  \"schema\": 1,\n  \"name\": \"Board {tag}\",\n  \"entry\": \"index.html\",\n  \"caps\": [{}]\n}}\n",
        caps.join(", ")
    )
}

fn door(caps: &[&str]) -> Door {
    let e = env();
    let f = ZenFiles::local();
    f.setup().expect("zen setup");
    f.new_widget("board", Some("hello")).expect("widget folder");
    let garden = f.gardens()[0].id.clone();
    std::fs::write(f.path_of(&ZenFile::Garden(garden.clone())), PAGE).expect("garden page");
    let files_root = k2_core::paths::k2_home().join("garden-files-room");
    std::fs::create_dir_all(files_root.join("docs")).expect("room dir");
    std::fs::write(files_root.join("docs/notes.md"), "# Notes\n").expect("notes");
    std::fs::write(files_root.join("docs/pixel.bin"), [0u8, 1, 2, 255]).expect("bin");
    let files_root = std::fs::canonicalize(&files_root).expect("canonical");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES ('w-files', 'garden-files', ?1, 'garden-files')",
            rusqlite::params![files_root.to_string_lossy()],
        )
        .expect("files agent");
    }
    let d = Door { _env: e, f, garden, files_root };
    d.set(caps, MadeBy::Owner);
    d
}

impl Door {
    fn hash(&self) -> String {
        let h = self.f.garden_widget_status(&self.garden, "board").hash().to_string();
        assert_eq!(h.len(), 64, "board bundles clean");
        h
    }

    /// New manifest caps (a new hash), signed by `maker` (`Unclaimed`: not signed).
    fn set(&self, caps: &[&str], maker: MadeBy) {
        let tag = uuid::Uuid::new_v4().simple().to_string();
        std::fs::write(self.f.widget_dir("board").join("manifest.json"), manifest(caps, &tag[..8])).expect("manifest");
        // New code, so a new hash its maker signs first (the first signer of
        // a hash keeps it).
        let index = self.f.widget_dir("board").join("index.html");
        let html = std::fs::read_to_string(&index).expect("index.html");
        std::fs::write(&index, format!("{html}\n<!-- version {tag} -->\n")).expect("index.html");
        if maker == MadeBy::Unclaimed {
            return;
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        let kind = if maker == MadeBy::Owner { CallerKind::Owner } else { CallerKind::Agent };
        authors::stamp(&conn, "board", &self.hash(), &maker, StampSource::Sign, kind, 1).expect("stamp");
    }

    fn raw(&self, verb: &str, args: Value) -> (u16, Value) {
        let body = json!({ "garden": self.garden, "placement": "b1", "hash": self.hash(), "verb": verb, "args": args });
        let r = handle("/cli/zen/call", DataWho::Owner, body.to_string().as_bytes(), &HashMap::new(), APP);
        let code: u16 = r.status.split(' ').next().unwrap().parse().unwrap();
        (code, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{verb}: {e}: {}", r.body)))
    }

    fn call(&self, verb: &str, args: Value) -> Value {
        let (code, v) = self.raw(verb, args);
        assert_eq!(code, 200, "{verb}: {v}");
        assert_eq!(v["ok"], true, "{v}");
        v["result"].clone()
    }

    fn refuse(&self, verb: &str, args: Value) -> String {
        let (code, v) = self.raw(verb, args);
        assert_ne!(code, 200, "{verb} should refuse: {v}");
        v["error"]["code"].as_str().unwrap_or_else(|| panic!("{verb}: no error code: {v}")).to_string()
    }
}

/// Pin `board-keeper`'s Chat and have its agent post `n` lines.
fn keeper_thread(n: usize) -> (String, Vec<i64>) {
    let conv = format!("pr-conv-{}", uuid::Uuid::new_v4());
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, "ws-pr", "w-keeper", None, Some(&conv), "claude", "system", "running").expect("pin");
    let seqs = (0..n)
        .map(|i| k2_core::overlay::post_thread(&conn, &conv, "w-keeper", "keeper", "keeper", &format!("line {i}"), "thread").expect("post").0.seq)
        .collect();
    (conv, seqs)
}

fn no_banned_fields(v: &Value, at: &str) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                assert!(
                    !k2_core::contract::catalog().banned_fields.iter().any(|b| b == k),
                    "{at}: banned field {k} on a Garden answer: {v}"
                );
                no_banned_fields(x, &format!("{at}.{k}"));
            }
        }
        Value::Array(a) => a.iter().for_each(|x| no_banned_fields(x, at)),
        _ => {}
    }
}

#[test]
fn each_row_needs_its_cap_and_bad_arguments_are_refused() {
    let d = door(&["store:read"]);
    for (verb, args) in [
        ("thread.unread", json!([])),
        ("thread.mute", json!([{ "addr": "board-keeper", "muted": true }])),
        ("thread.catchup", json!([{ "threads": [{ "addr": "board-keeper", "afterSeq": 0 }] }])),
        ("tickets.list", json!([{ "project": "board-keeper" }])),
        ("tickets.create", json!([{ "project": "board-keeper", "title": "x" }])),
        ("activity.snapshot", json!([{ "workspace": "board-keeper" }])),
        ("files.list", json!([{ "workspace": "garden-files", "relPath": "." }])),
    ] {
        assert_eq!(d.refuse(verb, args), "cap_not_granted", "{verb}");
    }
    d.set(&ALL, MadeBy::Owner);
    assert_eq!(d.refuse("files.list", json!([{ "workspace": "garden-files", "relPath": ".", "path": "/" }])), "bad_request");
    assert_eq!(d.refuse("files.read", json!([{ "workspace": "garden-files" }])), "bad_request", "relPath is required");
    assert_eq!(d.refuse("thread.unread", json!([{ "kind": "project" }])), "bad_request", "no project rows on the bridge");
    // No project chat entries in a Garden catch-up (the App row's args).
    assert_eq!(d.refuse("thread.catchup", json!([{ "threads": [], "groups": [{ "group": "pg-q4", "afterSeq": 0 }] }])), "bad_request");
    assert_eq!(d.refuse("tickets.list", json!([{ "project": "nowhere" }])), "not_bound");
    assert_eq!(d.refuse("files.list", json!([{ "workspace": "/etc", "relPath": "." }])), "not_bound");
}

#[test]
fn files_answer_the_apps_room_relative_shape() {
    let d = door(&ALL);
    let entries = d.call("files.list", json!([{ "workspace": "garden-files", "relPath": "docs" }]));
    let names: Vec<&str> = entries.as_array().expect("entries").iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["notes.md", "pixel.bin"], "{entries}");
    assert_eq!(entries[0]["relPath"], "docs/notes.md", "{entries}");
    no_banned_fields(&entries, "files.list");
    let text = d.call("files.read", json!([{ "workspace": "garden-files", "relPath": "docs/notes.md" }]));
    assert_eq!((text["content"].as_str(), text["relPath"].as_str()), (Some("# Notes\n"), Some("docs/notes.md")), "{text}");
    no_banned_fields(&text, "files.read");
    let bin = d.call("files.readBinary", json!([{ "workspace": "garden-files", "relPath": "docs/pixel.bin" }]));
    assert_eq!(bin, json!({ "base64": "AAEC/w==" }));
    // Outside the room: refused, and the answer never names a host path.
    let (code, v) = d.raw("files.read", json!([{ "workspace": "garden-files", "relPath": "../../etc/passwd" }]));
    assert_ne!(code, 200, "{v}");
    assert!(!v.to_string().contains(&*d.files_root.to_string_lossy()), "no host path in a refusal: {v}");
}

/// 0.46.6 (prd-ticket-attachments-v1 TK16/TA8): a widget files and reads
/// ticket files as the person; the manifest's `files:read` is the gate
/// (without it every file is `hidden`), `tickets.attachment` opens one, and
/// an unknown key is still refused.
#[test]
fn ticket_files_follow_the_widgets_file_caps() {
    // files:download is an App cap (a widget manifest can't hold it), so a
    // Garden reads ticket files but never gets the save.
    let d = door(&["tickets:read", "tickets:post", "files:read"]);
    let made = d.call(
        "tickets.create",
        json!([{ "project": "garden-files", "title": "Notes ok?", "attachments": [{ "relPath": "docs/notes.md" }] }]),
    );
    let id = made["id"].as_str().unwrap_or_else(|| panic!("an id: {made}")).to_string();
    assert_eq!(made["attachments"][0]["name"], "notes.md", "the echo survives the projection: {made}");
    no_banned_fields(&made, "tickets.create");
    let said = d.call("tickets.comment", json!([{ "id": id, "body": "", "attachments": [{ "relPath": "docs/pixel.bin" }] }]));
    assert_eq!(said["attachments"][0]["name"], "pixel.bin", "{said}");
    let shown = d.call("tickets.show", json!([{ "id": id }]));
    no_banned_fields(&shown, "tickets.show");
    let seed = &shown["comments"][0];
    let comment_id = seed["id"].as_str().unwrap_or_else(|| panic!("comment ids survive the projection: {shown}")).to_string();
    assert_eq!((seed["attachments"][0]["state"].as_str(), seed["attachments"][0]["canDownload"].as_bool()), (Some("ok"), Some(false)), "{shown}");
    let meta = d.call("tickets.attachment", json!([{ "id": id, "commentId": comment_id, "n": 0, "mode": "preview" }]));
    assert_eq!(meta["text"], "# Notes\n", "{meta}");
    let (code, v) = d.raw("tickets.attachment", json!([{ "id": id, "commentId": comment_id, "n": 0, "mode": "download" }]));
    assert_eq!((code, v["error"]["cap"].as_str()), (403, Some("files:download")), "{v}");
    assert_eq!(d.refuse("tickets.comment", json!([{ "id": id, "body": "x", "grant": true }])), "bad_request", "unknown keys are still refused");

    // Without files:read in the manifest, files are hidden and don't open.
    d.set(&["tickets:read", "tickets:post"], MadeBy::Owner);
    let shown = d.call("tickets.show", json!([{ "id": id }]));
    assert_eq!(shown["comments"][0]["attachments"][0], json!({ "kind": "file", "state": "hidden" }), "{shown}");
    let meta = d.call("tickets.attachment", json!([{ "id": id, "commentId": comment_id, "n": 0 }]));
    assert_eq!(meta["state"], "hidden", "{meta}");
    let (code, v) = d.raw("tickets.attachment", json!([{ "id": id, "commentId": comment_id, "n": 0, "mode": "preview" }]));
    assert_eq!((code, v["error"]["code"].as_str()), (403, Some("cap_not_granted")), "{v}");
}

#[test]
fn tickets_read_and_write_as_the_person() {
    let d = door(&ALL);
    let made = d.call("tickets.create", json!([{ "project": "board-keeper", "title": "Quote for J1042?" }]));
    let id = made["id"].as_str().unwrap_or_else(|| panic!("an id: {made}")).to_string();
    assert_eq!(made["projectId"], "w-keeper", "{made}");
    no_banned_fields(&made, "tickets.create");
    let list = d.call("tickets.list", json!([{ "project": "board-keeper" }]));
    assert!(list["items"].as_array().unwrap().iter().any(|t| t["id"] == id.as_str()), "{list}");
    no_banned_fields(&list, "tickets.list");
    let said = d.call("tickets.comment", json!([{ "id": id, "body": "Looks right" }]));
    assert_eq!((said["author"].as_str(), said["ok"].as_bool()), (Some("owner"), Some(true)), "the person's own comment: {said}");
    let shown = d.call("tickets.show", json!([{ "id": id }]));
    // The agent's question first, then the person's own comment.
    let comments = shown["comments"].as_array().expect("comments");
    let last = comments.last().expect("a comment");
    assert_eq!((last["author"].as_str(), last["body"].as_str()), (Some("owner"), Some("Looks right")), "{shown}");
    assert_eq!(shown["status"], "needs_discussion", "free text moves it: {shown}");
    let done = d.call("tickets.resolve", json!([{ "id": id }]));
    assert_eq!(done["status"], "resolved", "{done}");
    assert_eq!(d.refuse("tickets.show", json!([{ "id": "no-such-ticket" }])), "not_found");
    let snap = d.call("activity.snapshot", json!([{ "workspace": "board-keeper" }]));
    assert_eq!((snap["workspace"].as_str(), snap["sessions"].is_array()), (Some("board-keeper"), true), "{snap}");
    // A paused placement can't write; reads still work.
    crate::zen_routes::pause_for_writes(&d.garden, "b1");
    assert_eq!(d.refuse("tickets.create", json!([{ "project": "board-keeper", "title": "x" }])), "paused");
    d.call("tickets.list", json!([{ "project": "board-keeper" }]));
}

#[test]
fn an_agent_made_widget_acts_in_its_makers_room_only() {
    let d = door(&ALL);
    let theirs = d.call("tickets.create", json!([{ "project": "board-keeper", "title": "keeper's" }]))["id"].as_str().unwrap().to_string();
    d.set(&ALL, MadeBy::Agent { project_id: "w-builder".into() });
    d.call("tickets.list", json!([{ "project": "board-builder" }]));
    let mine = d.call("tickets.create", json!([{ "project": "board-builder", "title": "builder's" }]));
    assert_eq!(mine["projectId"], "w-builder", "{mine}");
    assert_eq!(d.refuse("tickets.list", json!([{ "project": "board-keeper" }])), "not_bound");
    assert_eq!(d.refuse("tickets.show", json!([{ "id": theirs }])), "not_found", "the same as a missing ticket");
    assert_eq!(d.refuse("tickets.comment", json!([{ "id": theirs, "body": "x" }])), "not_found");
    assert_eq!(d.refuse("files.list", json!([{ "workspace": "garden-files", "relPath": "." }])), "not_bound");
    assert_eq!(d.refuse("activity.snapshot", json!([{ "workspace": "board-keeper" }])), "not_bound");
    // The person's own Thread state isn't a room's: no maker rule.
    d.call("thread.unread", json!([]));
    // An unsigned version: author_unknown before any room.
    d.set(&ALL, MadeBy::Unclaimed);
    assert_eq!(d.refuse("tickets.list", json!([{ "project": "board-builder" }])), "author_unknown");
}

#[test]
fn thread_rows_are_the_persons_own_state() {
    let d = door(&ALL);
    let (conv, seqs) = keeper_thread(3);
    let badge = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::read_state::badge_key_for(&conn, "owner").expect("badge key")
    };
    let u = d.call("thread.unread", json!([]));
    assert_eq!(u["badgeKey"], badge.as_str(), "the person's own key, the same as on their desktop and phone: {u}");
    let row = u["byConversation"].as_array().unwrap().iter().find(|r| r["id"] == conv.as_str()).cloned().unwrap_or_else(|| panic!("{u}"));
    assert_eq!((row["addr"].as_str(), row["unread"].as_i64(), row["lastSeq"].as_i64()), (Some("board-keeper"), Some(3), Some(seqs[2])), "{u}");
    let before = u["total"].as_i64().unwrap();
    assert!(before >= 3, "{u}");
    no_banned_fields(&u, "thread.unread");

    // Mute is the person's: the desktop's own unread route sees it.
    let m = d.call("thread.mute", json!([{ "addr": "board-keeper", "muted": true }]));
    assert_eq!((m["muted"].as_bool(), m["changed"].as_bool(), m["addr"].as_str()), (Some(true), Some(true), Some("board-keeper")), "{m}");
    let desk = crate::read_routes::handle(false, crate::read_routes::UNREAD_PATH, &HashMap::new(), b"", "owner");
    let desk: Value = serde_json::from_str(&desk.body).unwrap();
    assert!(desk["muted"].as_array().unwrap().iter().any(|x| x["id"] == conv.as_str()), "{desk}");
    let u = d.call("thread.unread", json!([]));
    assert_eq!(u["total"].as_i64(), Some(before - 3), "a muted Thread leaves the badge: {u}");
    assert!(u["muted"].as_array().unwrap().iter().any(|x| x["id"] == conv.as_str() && x["addr"] == "board-keeper"), "{u}");
    let m = d.call("thread.mute", json!([{ "addr": "board-keeper", "muted": false }]));
    assert_eq!(m["muted"], false, "{m}");
    assert_eq!(d.refuse("thread.mute", json!([{ "addr": "nobody-here", "muted": true }])), "not_found");

    // Catch-up: the person's read of the Thread, in the App's names.
    let c = d.call("thread.catchup", json!([{ "threads": [{ "addr": "board-keeper", "afterSeq": seqs[0] }] }]));
    assert!(c.get("rev").is_none() && c.get("instanceId").is_none(), "{c}");
    let e = &c["threads"][0];
    assert_eq!(e["conversation"], conv.as_str(), "{c}");
    let got: Vec<i64> = e["items"].as_array().unwrap().iter().map(|i| i["seq"].as_i64().unwrap()).collect();
    assert_eq!(got, seqs[1..].to_vec(), "{c}");
    no_banned_fields(&c, "thread.catchup");
}

#[test]
fn project_strips_to_the_schema_and_reads_wire_names() {
    use std::collections::BTreeMap;
    let schema = json!({
        "type": "object",
        "properties": {
            "relPath": { "type": "string" },
            "items": { "type": "array", "items": { "type": "object", "properties": { "conversation": { "type": "string" } } } },
            "value": { "type": "json" }
        }
    });
    let names: BTreeMap<String, String> =
        [("relPath".to_string(), "path".to_string()), ("conversation".to_string(), "conversation_id".to_string())].into();
    let v = json!({ "path": "a/b", "root": "/home/x", "items": [{ "conversation_id": "c1", "rev": 9 }], "value": { "any": [1] } });
    assert_eq!(
        super::project(&schema, &v, &names),
        json!({ "relPath": "a/b", "items": [{ "conversation": "c1" }], "value": { "any": [1] } })
    );
}
