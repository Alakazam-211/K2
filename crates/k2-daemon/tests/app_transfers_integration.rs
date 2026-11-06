//! Resumable transfers (0.46.9, `k2 study app-transfers`), headless: the
//! real dispatcher in-process on a temp `$HOME`, over loopback, with App
//! guests' session passes. The pass side (`/cli/fs/upload/*`, the download
//! tokens) and the token door (`/_k2/transfers/…`, here on the box's own
//! port as on its address).
//!
//! - Ranges in any order, a range resent, a PUT cut off mid-body and sent
//!   again, HEAD and GET status, complete, the published file.
//! - Checksums: a range's own sha256 and the whole file's (a mismatch
//!   drops what K2 held).
//! - Tokens: one upload only, never a download's, expiry and a fresh
//!   token, the session's own expiry, the sweep.
//! - The person behind it: another guest gets a 404, a revoked pass is 401
//!   `pass_invalid`, a cap taken away is 403 at the next range.
//! - Quota: the 10 GiB ceiling, the drop caps counting unfinished uploads.
//! - Downloads: a file token and an attachment token; Range, If-Range,
//!   ETag, 416, HEAD, `?token=`; a changed file; `files.download`'s etag.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const PW: &str = "s3cret-horse-staple";

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn owner_ok(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

/// A guest with its own role holding `caps` in `ws`: (pass, role name).
async fn guest(port: u16, ws: &Ws, name: &str, caps: &[&str]) -> (String, String) {
    let role = format!("r-{name}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    owner_ok(port, "/cli/skin/users", json!({ "username": name })).await;
    k2_daemon::test_harness::guest_with_own_password(name, PW);
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": name, "role": role })).await;
    let body = json!({ "username": name, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "login {name}: {}", r.body);
    (r.json()["token"].as_str().expect("token").to_string(), role)
}

struct DoorResp {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl DoorResp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", String::from_utf8_lossy(&self.body)))
    }
    fn code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no code: {:?}", String::from_utf8_lossy(&self.body))).to_string()
    }
    fn h(&self, k: &str) -> Option<&str> {
        self.headers.get(&k.to_ascii_lowercase()).map(String::as_str)
    }
}

fn door_raw(port: u16, raw: &[u8]) -> DoorResp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    s.write_all(raw).unwrap();
    let mut out = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => panic!("door read: {e}"),
        }
    }
    let end = out.windows(4).position(|w| w == b"\r\n\r\n").expect("a head") + 4;
    let head = String::from_utf8_lossy(&out[..end]).to_string();
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).expect("status");
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())))
        .collect();
    DoorResp { status, headers, body: out[end..].to_vec() }
}

async fn door(port: u16, method: &str, path: &str, token: Option<&str>, headers: &[(&str, String)], body: &[u8]) -> DoorResp {
    let mut raw = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(t) = token {
        raw.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    for (k, v) in headers {
        raw.push_str(&format!("{k}: {v}\r\n"));
    }
    if method == "PUT" || method == "POST" {
        raw.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    raw.push_str("\r\n");
    let mut bytes = raw.into_bytes();
    bytes.extend_from_slice(body);
    tokio::task::spawn_blocking(move || door_raw(port, &bytes)).await.unwrap()
}

async fn put(port: u16, url: &str, token: &str, start: usize, bytes: &[u8], total: usize) -> DoorResp {
    let range = format!("bytes {start}-{}/{total}", start + bytes.len() - 1);
    door(port, "PUT", url, Some(token), &[("Content-Range", range)], bytes).await
}

fn session_file(id: &str) -> std::path::PathBuf {
    k2_core::fs_transfer::uploads_dir().join(format!("{id}.json"))
}

fn edit_session(id: &str, f: impl FnOnce(&mut Value)) {
    let p = session_file(id);
    let mut v: Value = serde_json::from_slice(&std::fs::read(&p).expect("session file")).expect("session json");
    f(&mut v);
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upload_takes_ranges_in_any_order_resumes_and_lands() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("tx{tag}"));
    let (pass, _) = guest(port, &ws, &format!("ann{tag}"), &["thread:read", "files:write"]).await;
    let (other, _) = guest(port, &ws, &format!("bob{tag}"), &["thread:read", "files:write"]).await;

    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
    let r = post(
        port,
        "/cli/fs/upload/create",
        &pass,
        json!({ "workspace": ws.handle, "dir": "in", "filename": "big.bin", "size": data.len(), "sha256": sha(&data) }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let up = r.json();
    let id = up["uploadId"].as_str().unwrap().to_string();
    let url = up["url"].as_str().unwrap().to_string();
    let token = up["transferToken"].as_str().unwrap().to_string();
    assert_eq!(url, format!("/_k2/transfers/uploads/{id}"));
    assert!(token.starts_with(&format!("k2up_{id}.")), "{token}");
    assert_eq!(up["missing"], json!([[0, 299_999]]));
    assert_eq!(up["dir"], "in");
    assert_eq!(up["partMax"], 16 * 1024 * 1024);
    assert!(up["tokenExpiresAt"].as_u64().unwrap() > up["expiresAt"].as_u64().unwrap() / 2, "both in milliseconds");

    // Last third first, then the first, then the first again (idempotent).
    let r = put(port, &url, &token, 200_000, &data[200_000..], data.len()).await;
    assert_eq!(r.status, 200, "{:?}", String::from_utf8_lossy(&r.body));
    let r = put(port, &url, &token, 0, &data[..100_000], data.len()).await;
    assert_eq!(r.status, 200);
    let r = put(port, &url, &token, 0, &data[..100_000], data.len()).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["missing"], json!([[100_000, 199_999]]));
    // HEAD: the same, as headers, no body.
    let h = door(port, "HEAD", &url, Some(&token), &[], b"").await;
    assert_eq!(h.status, 200);
    assert_eq!(h.h("K2-Upload-Received"), Some("0-99999,200000-299999"));
    assert_eq!(h.h("K2-Upload-Size"), Some("300000"));
    assert_eq!(h.h("K2-Upload-Done"), Some("0"));
    assert!(h.body.is_empty(), "HEAD carries no body");
    // A PUT cut off mid-body (the app killed) records nothing…
    let cut = {
        let mut raw = format!(
            "PUT {url} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Range: bytes 100000-199999/300000\r\nContent-Length: 100000\r\n\r\n"
        )
        .into_bytes();
        raw.extend_from_slice(&data[100_000..150_000]);
        raw
    };
    tokio::task::spawn_blocking(move || {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(&cut).unwrap();
        s.shutdown(std::net::Shutdown::Both).unwrap();
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let s = get(port, "/cli/fs/upload/status", &pass, &format!("uploadId={id}")).await;
    assert_eq!(s.status, 200, "{}", s.body);
    assert_eq!(s.json()["missing"], json!([[100_000, 199_999]]), "the cut range isn't counted");
    // …another person can't see it, and complete before the end says what's missing.
    let s = get(port, "/cli/fs/upload/status", &other, &format!("uploadId={id}")).await;
    assert_eq!(s.status, 404, "{}", s.body);
    let c = door(port, "POST", &format!("{url}/complete"), Some(&token), &[], b"").await;
    assert_eq!((c.status, c.code()), (409, "upload_incomplete".to_string()));
    assert_eq!(c.json()["missing"], json!([[100_000, 199_999]]));
    // A range whose own sha256 is wrong writes nothing.
    let r = door(
        port,
        "PUT",
        &url,
        Some(&token),
        &[("Content-Range", "bytes 100000-199999/300000".into()), ("K2-Content-Sha256", sha(b"nope"))],
        &data[100_000..200_000],
    )
    .await;
    assert_eq!((r.status, r.code()), (422, "checksum_mismatch".to_string()));
    assert_eq!(r.json()["range"], json!(true));
    // Bad ranges.
    for (cr, body) in [("bytes 0-9/299999", &data[..10]), ("bytes 0-9/300000", &data[..9]), ("bytes 299999-300000/300000", &data[..2])] {
        let r = door(port, "PUT", &url, Some(&token), &[("Content-Range", cr.into())], body).await;
        assert_eq!((r.status, r.code()), (400, "bad_range".to_string()), "{cr}");
    }
    let r = door(port, "PUT", &url, Some(&token), &[], &data[..10]).await;
    assert_eq!(r.status, 400, "no Content-Range");
    // The gap, with its own sha256, then complete.
    let r = door(
        port,
        "PUT",
        &url,
        Some(&token),
        &[("Content-Range", "bytes 100000-199999/300000".into()), ("K2-Content-Sha256", sha(&data[100_000..200_000]))],
        &data[100_000..200_000],
    )
    .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["complete"], json!(true));
    let c = door(port, "POST", &format!("{url}/complete"), Some(&token), &[], json!({ "sha256": sha(&data) }).to_string().as_bytes()).await;
    assert_eq!(c.status, 200, "{:?}", String::from_utf8_lossy(&c.body));
    assert_eq!(c.json()["relPath"], "in/big.bin");
    assert_eq!(std::fs::read(ws.path.join("in/big.bin")).unwrap(), data);
    let leftovers: Vec<_> = std::fs::read_dir(ws.path.join("in")).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leftovers.len(), 1, "no part left behind: {leftovers:?}");
    // Again: the same answer (the pass side too); a range after: 409.
    let again = post(port, "/cli/fs/upload/complete", &pass, json!({ "uploadId": id })).await;
    assert_eq!(again.status, 200, "{}", again.body);
    assert_eq!(again.json()["relPath"], "in/big.bin");
    let r = put(port, &url, &token, 0, &data[..10], data.len()).await;
    assert_eq!((r.status, r.code()), (409, "upload_done".to_string()));

    // A whole-file mismatch drops what K2 held.
    let r = post(port, "/cli/fs/upload/create", &pass, json!({ "workspace": ws.handle, "dir": "in", "filename": "two.bin", "size": 10 })).await;
    let up = r.json();
    let (url2, tok2) = (up["url"].as_str().unwrap().to_string(), up["transferToken"].as_str().unwrap().to_string());
    assert_eq!(put(port, &url2, &tok2, 0, b"0123456789", 10).await.status, 200);
    let c = door(port, "POST", &format!("{url2}/complete"), Some(&tok2), &[], json!({ "sha256": sha(b"different!") }).to_string().as_bytes()).await;
    assert_eq!((c.status, c.code()), (422, "checksum_mismatch".to_string()));
    let h = door(port, "GET", &url2, Some(&tok2), &[], b"").await;
    assert_eq!(h.json()["receivedBytes"], 0, "send it again");
    assert!(!ws.path.join("in/two.bin").exists());
    // Abort through the pass side: the part goes, the token stops.
    let a = post(port, "/cli/fs/upload/abort", &pass, json!({ "uploadId": up["uploadId"] })).await;
    assert_eq!(a.status, 200, "{}", a.body);
    let h = door(port, "GET", &url2, Some(&tok2), &[], b"").await;
    assert_eq!((h.status, h.code()), (401, "transfer_token_invalid".to_string()));

    // The owner token isn't an App guest.
    let r = post(port, "/cli/fs/upload/create", OWNER, json!({ "workspace": ws.handle, "filename": "x", "size": 1 })).await;
    assert_eq!(r.status, 400, "{}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tokens_are_scoped_expire_and_follow_the_person() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("tk{tag}"));
    let (pass, role) = guest(port, &ws, &format!("cat{tag}"), &["thread:read", "files:write"]).await;
    let mk = |name: &'static str| {
        let pass = pass.clone();
        let handle = ws.handle.clone();
        async move {
            let r = post(port, "/cli/fs/upload/create", &pass, json!({ "workspace": handle, "dir": "up", "filename": name, "size": 4, "tokenTtlSecs": 120 })).await;
            assert_eq!(r.status, 200, "{}", r.body);
            let v = r.json();
            (v["uploadId"].as_str().unwrap().to_string(), v["url"].as_str().unwrap().to_string(), v["transferToken"].as_str().unwrap().to_string())
        }
    };
    let (a_id, a_url, a_tok) = mk("a.bin").await;
    let (_b_id, b_url, b_tok) = mk("b.bin").await;

    // One upload per token; K2's own passes never count here.
    let r = put(port, &b_url, &a_tok, 0, b"abcd", 4).await;
    assert_eq!((r.status, r.code()), (401, "transfer_token_invalid".to_string()));
    let r = put(port, &a_url, &pass, 0, b"abcd", 4).await;
    assert_eq!((r.status, r.code()), (401, "transfer_token_invalid".to_string()), "a session pass isn't a transfer token");
    let r = door(port, "PUT", &a_url, None, &[("Content-Range", "bytes 0-3/4".into())], b"abcd").await;
    assert_eq!((r.status, r.code()), (401, "transfer_token_required".to_string()));
    let r = door(port, "GET", &format!("{a_url}?token={a_tok}"), None, &[], b"").await;
    assert_eq!(r.status, 401, "an upload token never rides in the query");
    assert_eq!(put(port, &b_url, &b_tok, 0, b"abcd", 4).await.status, 200);

    // The token expires; the upload lives on and a fresh token works.
    edit_session(&a_id, |v| {
        for t in v["tokens"].as_array_mut().unwrap() {
            t["expires_at"] = json!(1);
        }
    });
    let r = put(port, &a_url, &a_tok, 0, b"abcd", 4).await;
    assert_eq!((r.status, r.code()), (401, "transfer_token_expired".to_string()));
    let t = post(port, "/cli/fs/upload/token", &pass, json!({ "uploadId": a_id, "tokenTtlSecs": 300 })).await;
    assert_eq!(t.status, 200, "{}", t.body);
    let fresh = t.json()["transferToken"].as_str().unwrap().to_string();
    assert_eq!(put(port, &a_url, &fresh, 0, b"ab", 4).await.status, 200);

    // A cap taken away stops the next range; given back, it goes on.
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read"] })).await;
    let r = put(port, &a_url, &fresh, 2, b"cd", 4).await;
    assert_eq!((r.status, r.code()), (403, "cap_not_granted".to_string()));
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read", "files:write"] })).await;
    assert_eq!(put(port, &a_url, &fresh, 2, b"cd", 4).await.status, 200);

    // A revoked pass ends it.
    let pass_id = k2_core::skin::resolve_skin_token(&pass).expect("live pass").id;
    assert!(k2_core::skin::revoke_token(&pass_id).expect("revoke"));
    let r = door(port, "POST", &format!("{a_url}/complete"), Some(&fresh), &[], b"").await;
    assert_eq!((r.status, r.code()), (401, "pass_invalid".to_string()));

    // The session's own expiry, then the sweep.
    let b_id = b_url.rsplit('/').next().unwrap().to_string();
    edit_session(&b_id, |v| v["expires_at"] = json!(1));
    let r = door(port, "GET", &b_url, Some(&b_tok), &[], b"").await;
    assert_eq!((r.status, r.code()), (410, "transfer_expired".to_string()));
    let part = std::fs::read_dir(ws.path.join("up")).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with(".k2-upload-g-")).count();
    assert_eq!(part, 2, "two parts before the sweep");
    let swept = k2_core::fs_transfer::sweep_expired(
        &k2_core::fs_transfer::uploads_dir(),
        &k2_core::fs_transfer::downloads_dir(),
        &k2_core::fs_guest_upload::ledger_dir(),
        k2_core::fs_transfer::now_secs(),
    )
    .expect("sweep");
    assert_eq!(swept.uploads, 1);
    assert!(!session_file(&b_id).exists());
    assert!(session_file(&a_id).exists(), "a live session stays");
    let part = std::fs::read_dir(ws.path.join("up")).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with(".k2-upload-g-")).count();
    assert_eq!(part, 1, "the expired session's part went with it");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quotas_are_todays_upload_quotas() {
    let env = setup(&[("K2_DROP_MAX_BYTES", "1000")]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("q{tag}"));
    let user = format!("dee{tag}");
    let (pass, role) = guest(port, &ws, &user, &["thread:read", "files:upload"]).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": ws.handle, "upload": ["Inbox/"], "uploadDir": "Inbox" })).await;

    let r = post(port, "/cli/fs/upload/create", &pass, json!({ "workspace": ws.handle, "filename": "huge.bin", "size": 10u64 * 1024 * 1024 * 1024 + 1 })).await;
    assert_eq!(r.status, 413, "over the 10 GiB ceiling: {}", r.body);
    let r = post(port, "/cli/fs/upload/create", &pass, json!({ "workspace": ws.handle, "filename": "a.bin", "size": 600 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["dir"], format!("Inbox/{user}"), "an upload-only drop lands in the person's own folder");
    // The first upload's 600 bytes count while it is unfinished.
    let r = post(port, "/cli/fs/upload/create", &pass, json!({ "workspace": ws.handle, "filename": "b.bin", "size": 600 })).await;
    assert_eq!(r.status, 413, "the drop cap counts unfinished uploads: {}", r.body);
    // Outside the drop folder: the plain missing-cap 403.
    let r = post(port, "/cli/fs/upload/create", &pass, json!({ "workspace": ws.handle, "dir": ".", "filename": "c.bin", "size": 1 })).await;
    assert_eq!(r.status, 403, "{}", r.body);
}

fn pin_chat(ws: &Ws) -> String {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    conv
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_tokens_serve_ranges_with_if_range_and_recheck_rights() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("dl{tag}"));
    let _conv = pin_chat(&ws);
    let file: Vec<u8> = (0..50_000u32).map(|i| (i % 241) as u8).collect();
    write(&ws.path, "public/report.bin", &file);
    let (pass, role) = guest(port, &ws, &format!("eve{tag}"), &["thread:read", "files:read", "files:download"]).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": ws.handle, "download": ["public/"] })).await;

    let t = post(port, "/cli/fs/download/token", &pass, json!({ "workspace": ws.handle, "path": "public/report.bin" })).await;
    assert_eq!(t.status, 200, "{}", t.body);
    let tv = t.json();
    let (url, tok, etag) = (tv["url"].as_str().unwrap().to_string(), tv["transferToken"].as_str().unwrap().to_string(), tv["etag"].as_str().unwrap().to_string());
    assert_eq!(tv["size"], 50_000);
    assert_eq!(tv["name"], "report.bin");
    assert!(tok.starts_with("k2dn_"));

    let full = door(port, "GET", &url, Some(&tok), &[], b"").await;
    assert_eq!(full.status, 200);
    assert_eq!(full.body, file);
    assert_eq!(full.h("ETag"), Some(etag.as_str()));
    assert_eq!(full.h("Accept-Ranges"), Some("bytes"));
    assert!(full.h("Content-Disposition").unwrap().contains("report.bin"));
    let part = door(port, "GET", &url, Some(&tok), &[("Range", "bytes=30000-".into()), ("If-Range", etag.clone())], b"").await;
    assert_eq!(part.status, 206);
    assert_eq!(part.h("Content-Range"), Some("bytes 30000-49999/50000"));
    assert_eq!(part.body, &file[30_000..]);
    let stale = door(port, "GET", &url, Some(&tok), &[("Range", "bytes=30000-".into()), ("If-Range", "\"other\"".into())], b"").await;
    assert_eq!(stale.status, 200, "If-Range missed: the whole file");
    assert_eq!(stale.body.len(), 50_000);
    let past = door(port, "GET", &url, Some(&tok), &[("Range", "bytes=60000-".into())], b"").await;
    assert_eq!(past.status, 416);
    assert_eq!(past.h("Content-Range"), Some("bytes */50000"));
    let head = door(port, "HEAD", &url, Some(&tok), &[], b"").await;
    assert_eq!((head.status, head.h("Content-Length")), (200, Some("50000")));
    assert!(head.body.is_empty());
    // A browser link carries the token in the query.
    let q = door(port, "GET", &format!("{url}?token={tok}"), None, &[("Range", "bytes=0-9".into())], b"").await;
    assert_eq!((q.status, q.body.as_slice()), (206, &file[..10]));
    // An upload token is never a download's.
    let r = door(port, "GET", &url, Some(&tok.replacen("k2dn_", "k2up_", 1)), &[], b"").await;
    assert_eq!(r.status, 401);

    // files.download answers an etag, and refuses a stale one.
    let c = get(port, "/cli/fs/download", &pass, &format!("workspace={}&path=public/report.bin&len=10", ws.handle)).await;
    assert_eq!(c.status, 200, "{}", c.body);
    assert_eq!(c.json()["etag"], etag);
    std::thread::sleep(Duration::from_millis(20));
    write(&ws.path, "public/report.bin", b"a new version");
    let c = get(port, "/cli/fs/download", &pass, &format!("workspace={}&path=public/report.bin&len=10&etag={}", ws.handle, enc(&etag))).await;
    assert_eq!(c.status, 412, "{}", c.body);
    let changed = door(port, "GET", &url, Some(&tok), &[("Range", "bytes=5-".into()), ("If-Range", etag.clone())], b"").await;
    assert_eq!((changed.status, changed.body.as_slice()), (200, &b"a new version"[..]), "a changed file restarts");
    assert_ne!(changed.h("ETag"), Some(etag.as_str()));

    // Rights read again: files:download taken away stops the token.
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read", "files:read"] })).await;
    let r = door(port, "GET", &url, Some(&tok), &[], b"").await;
    assert_eq!((r.status, r.code()), (403, "cap_not_granted".to_string()));
    let r = post(port, "/cli/fs/download/token", &pass, json!({ "workspace": ws.handle, "path": "public/report.bin" })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read", "files:read", "files:download"] })).await;

    // An attachment's token: the save's rights.
    let addr = ws.handle.clone();
    write(&ws.path, "docs/plan.txt", b"the plan");
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "plan", "attachments": [{ "path": "docs/plan.txt" }] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let doc = r.json()["id"].as_str().unwrap().to_string();
    let t = post(port, "/cli/thread/attachment/token", &pass, json!({ "addr": addr, "id": doc, "n": 0 })).await;
    assert_eq!(t.status, 200, "{}", t.body);
    let tv = t.json();
    let got = door(port, "GET", tv["url"].as_str().unwrap(), Some(tv["transferToken"].as_str().unwrap()), &[], b"").await;
    assert_eq!((got.status, got.body.as_slice()), (200, &b"the plan"[..]));
    let (viewer, _) = guest(port, &ws, &format!("fay{tag}"), &["thread:read", "files:read"]).await;
    let r = post(port, "/cli/thread/attachment/token", &viewer, json!({ "addr": addr, "id": doc, "n": 0 })).await;
    assert_eq!(r.status, 403, "no files:download, no token: {}", r.body);
    let owner_t = post(port, "/cli/thread/attachment/token", OWNER, json!({ "addr": addr, "id": doc, "n": 0 })).await;
    assert_eq!(owner_t.status, 200, "a person may save what they may open: {}", owner_t.body);
}
