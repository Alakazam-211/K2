//! The 0.46.4 release gate (prd-app-caps-0464-v1 AC37): no widening.
//!
//! A role holds every 0.46.3 cap combination that touches files
//! (`files:read`, `files:write`, `files:download`, `files:agentconfig`),
//! unscoped and with a folder scope. For each, one battery of requests
//! walks every guest files door and the Thread attachment door. Then the
//! owner adds download entries to the same role line (the 0.46.4 field a
//! person sets), and the battery runs again: every pre-0.46.4 door must
//! answer byte for byte the same (timestamps dropped: a reset folder gets
//! a new mtime). The new `files.download` door must give nothing to a
//! holder without `files:download`, and nothing without entries or
//! `files:read`.
//!
//! Drop folders are not "inert" by design (they wall the line, a
//! narrowing), so they are covered by `app_files_drop_integration`.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::path::Path;

use app_fixture::*;
use base64::Engine as _;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";

async fn owner_ok(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Put the folders the battery writes into back as they started.
fn reset_mutable(root: &Path) {
    for rel in ["jobs/J1/in", "jobs/J2/x.txt", "jobs/J2/u.bin", ".k2/x.txt", "made.txt", "v.bin"] {
        let p = root.join(rel);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&p).unwrap(),
            Ok(_) => std::fs::remove_file(&p).unwrap(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("reset {rel}: {e}"),
        }
    }
    std::fs::create_dir_all(root.join("jobs/J1/in")).unwrap();
}

/// Drop time fields (a reset folder gets a new mtime) so the compare is
/// about access, not clocks.
fn strip_times(v: &mut Value) {
    match v {
        Value::Object(o) => {
            o.retain(|k, _| {
                let k = k.to_ascii_lowercase();
                !(k.contains("modified") || k.contains("mtime") || k.contains("created") || k.contains("accessed"))
            });
            for x in o.values_mut() {
                strip_times(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(strip_times),
        _ => {}
    }
}

fn normalized(r: &Resp) -> String {
    match serde_json::from_str::<Value>(&r.body) {
        Ok(mut v) => {
            strip_times(&mut v);
            format!("{} {v}", r.status)
        }
        Err(_) => format!("{} {}", r.status, r.body),
    }
}

/// Every guest files door and the Thread attachment door, in a fixed
/// order. Returns `(label, normalized answer)`.
async fn battery(port: u16, pass: &str, ws: &Ws, addr: &str, doc: &str) -> Vec<(String, String)> {
    reset_mutable(&ws.path);
    let h = ws.handle.clone();
    let q = |p: &str| format!("workspace={}&path={}", enc(&h), enc(p));
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"bytes");
    let mut out: Vec<(String, String)> = Vec::new();
    let reads: &[(&str, &str)] = &[
        ("/cli/fs/read-dir", "."),
        ("/cli/fs/read-dir", "jobs/J1"),
        ("/cli/fs/read-dir", "jobs/J2"),
        ("/cli/fs/read-file", "top.txt"),
        ("/cli/fs/read-file", "jobs/J1/a.txt"),
        ("/cli/fs/read-file", "jobs/J2/secret.txt"),
        ("/cli/fs/read-file", ".k2/AGENT.md"),
        ("/cli/fs/read-binary", "jobs/J1/a.txt"),
        ("/cli/fs/read-binary", "jobs/J2/secret.txt"),
        ("/cli/fs/stat", "jobs/J1/a.txt"),
        ("/cli/fs/stat", "top.txt"),
    ];
    for (route, rel) in reads {
        let r = get(port, route, pass, &q(rel)).await;
        out.push((format!("GET {route} {rel}"), normalized(&r)));
    }
    for rel in ["jobs/J1/a.txt", "jobs/J2/secret.txt"] {
        let r = get(port, "/cli/fs/read-range", pass, &format!("{}&offset=0&len=4", q(rel))).await;
        out.push((format!("GET read-range {rel}"), normalized(&r)));
    }
    let r = get(port, "/cli/workspace/resources", pass, &format!("workspace={}", enc(&h))).await;
    out.push(("GET resources".into(), normalized(&r)));
    let r = post(port, "/cli/fs/resolve-links", pass, json!({ "workspace": h, "paths": ["top.txt", "jobs/J1/a.txt", "jobs/J2/secret.txt"] })).await;
    out.push(("POST resolve-links".into(), normalized(&r)));
    let writes: Vec<(&str, Value)> = vec![
        ("write-file", json!({ "workspace": h, "path": "jobs/J1/in/w.txt", "content": "w" })),
        ("write-file", json!({ "workspace": h, "path": "jobs/J2/x.txt", "content": "x" })),
        ("write-file", json!({ "workspace": h, "path": ".k2/x.txt", "content": "x" })),
        ("write-file", json!({ "workspace": h, "path": "made.txt", "content": "m" })),
        ("create", json!({ "workspace": h, "path": "jobs/J1/in/dir", "is_directory": true })),
        ("rename", json!({ "workspace": h, "path": "jobs/J1/in/w.txt", "new_name": "w2.txt" })),
        ("copy", json!({ "workspace": h, "sources": ["jobs/J1/a.txt"], "destination": "jobs/J1/in" })),
        ("copy", json!({ "workspace": h, "sources": ["jobs/J2/secret.txt"], "destination": "jobs/J1/in" })),
        ("move", json!({ "workspace": h, "sources": ["jobs/J1/in/a.txt"], "destination": "jobs/J1/in/dir" })),
        ("upload-binary", json!({ "workspace": h, "dir": "jobs/J1/in", "filename": "u.bin", "base64": b64 })),
        ("upload-binary", json!({ "workspace": h, "filename": "v.bin", "base64": b64 })),
        ("upload-binary", json!({ "workspace": h, "dir": "jobs/J2", "filename": "u.bin", "base64": b64 })),
        ("upload-chunk", json!({ "workspace": h, "dir": "jobs/J1/in", "filename": "c.bin", "upload_id": "u1", "offset": 0, "base64": b64, "is_last": true })),
    ];
    for (route, body) in writes {
        let r = post(port, &format!("/cli/fs/{route}"), pass, body.clone()).await;
        out.push((format!("POST {route} {body}"), normalized(&r)));
    }
    for (n, mode) in [(0, "meta"), (0, "preview"), (0, "range"), (0, "download"), (1, "meta"), (1, "range"), (1, "download")] {
        let r = get(port, "/cli/thread/attachment", pass, &format!("addr={}&id={}&n={n}&mode={mode}", enc(addr), enc(doc))).await;
        out.push((format!("GET attachment n={n} mode={mode}"), normalized(&r)));
    }
    let r = get(port, "/cli/thread", pass, &format!("addr={}", enc(addr))).await;
    let atts = if r.status == 200 {
        let v = r.json();
        let mut atts = v["items"].as_array().expect("items").iter().find(|i| i["id"] == doc).map(|i| i["doc"]["attachments"].clone()).unwrap_or(Value::Null);
        strip_times(&mut atts);
        format!("200 {atts}")
    } else {
        normalized(&r)
    };
    out.push(("GET thread attachments".into(), atts));
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_entries_never_change_a_pre_0464_files_door() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("gate{tag}"));
    write(&ws.path, "top.txt", b"top");
    write(&ws.path, "jobs/J1/a.txt", b"job one");
    write(&ws.path, "jobs/J2/secret.txt", b"job two");
    write(&ws.path, ".k2/AGENT.md", b"agent");
    // The room's pinned Chat, with a post that attaches two files.
    let conv = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    }
    let addr = ws.handle.clone();
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "files", "attachments": [{ "path": "top.txt" }, { "path": "jobs/J1/a.txt" }] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let doc = r.json()["id"].as_str().expect("id").to_string();

    let combos: &[&[&str]] = &[
        &[],
        &["files:read"],
        &["files:write"],
        &["files:download"],
        &["files:read", "files:write"],
        &["files:read", "files:download"],
        &["files:write", "files:download"],
        &["files:read", "files:write", "files:download"],
        &["files:write", "files:agentconfig"],
        &["files:read", "files:write", "files:agentconfig"],
        &["files:read", "files:write", "files:download", "files:agentconfig"],
    ];
    let mut checked = 0;
    for (i, combo) in combos.iter().enumerate() {
        for scoped in [false, true] {
            let role = format!("g{i}{}{tag}", if scoped { "s" } else { "u" });
            let mut caps = vec!["thread:read", "thread:post"];
            caps.extend_from_slice(combo);
            owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
            owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
            if scoped {
                owner_ok(
                    port,
                    "/cli/skin/roles/paths",
                    json!({ "name": role, "handle": ws.handle, "read": ["jobs/J1"], "write": ["jobs/J1/in"], "uploadDir": "jobs/J1/in" }),
                )
                .await;
            }
            let user = format!("u{i}{}{tag}", if scoped { "s" } else { "u" });
            owner_ok(port, "/cli/skin/users", json!({ "username": user })).await;
            owner_ok(port, "/cli/skin/users/password", json!({ "username": user, "password": PW })).await;
            owner_ok(port, "/cli/skin/roles/assign", json!({ "username": user, "role": role })).await;
            let body = json!({ "username": user, "password": PW }).to_string();
            let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
            assert_eq!(r.status, 200, "{}", r.body);
            let pass = r.json()["token"].as_str().expect("token").to_string();

            let has = |c: &str| combo.contains(&c);
            let dl = |rel: &'static str| {
                let pass = pass.clone();
                let h = ws.handle.clone();
                async move { get(port, "/cli/fs/download", &pass, &format!("workspace={}&path={}", enc(&h), enc(rel))).await }
            };
            // Before entries: files.download gives only what files:read
            // could already read, and only with files:download.
            for rel in ["top.txt", "jobs/J2/secret.txt", "jobs/J1/a.txt"] {
                let r = dl(rel).await;
                let readable = has("files:read") && (!scoped || rel.starts_with("jobs/J1/"));
                let want = if has("files:download") && readable { 200 } else { 403 };
                assert_eq!(r.status, want, "{combo:?} scoped={scoped} before entries: {rel}: {}", r.body);
            }

            let before = battery(port, &pass, &ws, &addr, &doc).await;
            // A person adds download entries to the same line (a
            // download-only body: the read/write folders stay).
            owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": ws.handle, "download": ["top.txt", "jobs/J2/secret.txt"] })).await;
            let after = battery(port, &pass, &ws, &addr, &doc).await;
            assert_eq!(before.len(), after.len());
            for ((label, b), (_, a)) in before.iter().zip(after.iter()) {
                assert_eq!(a, b, "{combo:?} scoped={scoped}: {label} changed after download entries were added");
                checked += 1;
            }

            // After entries: only a files:download holder gains, and only
            // the entries (plus what it could read before).
            for rel in ["top.txt", "jobs/J2/secret.txt", "jobs/J1/a.txt"] {
                let r = dl(rel).await;
                let readable = has("files:read") && (!scoped || rel.starts_with("jobs/J1/"));
                let entry = rel != "jobs/J1/a.txt";
                let want = if has("files:download") && (entry || readable) { 200 } else { 403 };
                assert_eq!(r.status, want, "{combo:?} scoped={scoped} after entries: {rel}: {}", r.body);
                if want == 403 {
                    assert_eq!(r.json(), json!({ "error": "missing capability files:download" }), "{rel}");
                }
            }
        }
    }
    assert!(checked >= combos.len() * 2 * 30, "the battery shrank: {checked} answers compared");
}
