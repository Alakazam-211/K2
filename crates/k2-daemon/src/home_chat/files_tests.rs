//! Attachments in Home DMs and channels (contract-home-chat-attachments-v1)
//! against the fake Sew: uploads that resume after a cut, the size
//! refusals, one login never reaching another's upload or file, the bytes
//! served with Range, ETag and safe headers, and no Sew URL ever leaving.
//! Each test fails loudly.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::{json, Value};

use super::state::PersonState;
use super::tests::{alex, call, chat_box, done, FakeApi, ALEX, SERIAL};
use super::{files, follower, people, routes};
use crate::voice::bff::Refusal;
use k2_core::connect_users::Role;

const KIM: &str = "k2:connect:7c9e6679-7425-40de-944b-e07fc1f90ae7";
const MIB: u64 = 1 << 20;

fn kim() -> people::Caller {
    people::Caller { ext: KIM.into(), login: "kim".into(), role: Role::Member }
}

fn q(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn as_person(c: &people::Caller, path: &str, p: &HashMap<String, String>, body: &[u8]) -> Result<Value, Refusal> {
    routes::route(path, c, p, body)
}

fn in_an_hour() -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(3600)).to_rfc3339()
}

/// Sew answers `POST /channels/900/uploads` with upload `id` of `size`
/// bytes, and its data route takes ranges from 0.
fn sew_takes_upload(api: &FakeApi, id: &str, size: u64) {
    api.answer(
        "/api/v1/channels/900/uploads",
        200,
        json!({
            "upload_id": id, "upload_url": format!("/api/v1/uploads/{id}/data?exp=9999999999&sig=first"),
            "url_expires_at": in_an_hour(), "chunk_size": MIB, "committed_bytes": 0, "size": size,
            "complete": false, "expires_at": in_an_hour(),
        }),
    );
    api.sim.lock().unwrap().insert(id.to_string(), (0, size));
}

/// Sew's `GET /uploads/{id}` as the uploader: where the simulator stands.
fn sew_reports_upload(api: &FakeApi, id: &str, sig: &str) {
    let (committed, size) = *api.sim.lock().unwrap().get(id).expect("a simulated upload");
    api.answer(
        &format!("/api/v1/uploads/{id}"),
        200,
        json!({
            "upload_id": id, "committed_bytes": committed, "size": size, "complete": committed == size,
            "expires_at": in_an_hour(), "channel_id": "900", "filename": "report.pdf", "chunk_size": MIB,
            "upload_url": format!("/api/v1/uploads/{id}/data?exp=9999999999&sig={sig}"), "url_expires_at": in_an_hour(),
        }),
    );
}

fn no_sew_url(v: &Value) {
    let s = v.to_string();
    assert!(!s.contains("sig=") && !s.contains("upload_url") && !s.contains("/api/v1/"), "a Sew URL left the daemon: {s}");
}

/// Create, two ranges (Content-Range, then offset), a cut whose answer was
/// lost, status says where Sew stands, the rest from there, then the send
/// carries Sew's `upload_ids` with no text.
#[test]
fn an_upload_resumes_after_a_cut_and_rides_the_send() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    let size = 3 * MIB + 10;
    sew_takes_upload(&api, "5001", size);
    let made = call(files::CREATE, json!({"conv": "900", "filename": "report.pdf", "size": size, "contentType": "application/pdf"})).expect("create");
    no_sew_url(&made);
    let up = &made["upload"];
    assert_eq!((up["id"].as_str(), up["committed"].as_u64(), up["chunkBytes"].as_u64()), (Some("5001"), Some(0), Some(MIB)));
    assert_eq!(up["maxRequestBytes"].as_u64(), Some(files::MAX_REQUEST_BYTES as u64));
    let sent = api.calls_to("/api/v1/channels/900/uploads");
    assert_eq!(sent[0].1, json!({"filename": "report.pdf", "size": size, "content_type": "application/pdf"}));
    assert_eq!(sent[0].0.as_deref(), Some("sews_first.x"), "the person's own session");

    // The first 2 MiB by Content-Range (a native client).
    let two = vec![7u8; 2 * MIB as usize];
    let range = format!("bytes 0-{}/{size}", 2 * MIB - 1);
    let r = as_person(&alex(), files::DATA, &q(&[("id", "5001"), (files::RANGE_PARAM, range.as_str())]), &two).expect("range 1");
    assert_eq!(r["upload"]["committed"].as_u64(), Some(2 * MIB));
    let posted = api.bytes_in.lock().unwrap().clone();
    assert_eq!(posted[0], ("/api/v1/uploads/5001/data?exp=9999999999&sig=first".to_string(), range, 2 * MIB as usize));

    // The cut: Sew stored the next MiB, but its answer never came back.
    api.sim.lock().unwrap().get_mut("5001").unwrap().0 = 3 * MIB;
    sew_reports_upload(&api, "5001", "first");
    let st = as_person(&alex(), files::STATUS, &q(&[("id", "5001")]), b"").expect("status");
    no_sew_url(&st);
    assert_eq!((st["upload"]["committed"].as_u64(), st["upload"]["conv"].as_str()), (Some(3 * MIB), Some("900")));
    // Sending from the old offset again: Sew's refusal, its code kept.
    let stale = as_person(&alex(), files::DATA, &q(&[("id", "5001"), ("offset", (2 * MIB).to_string().as_str())]), &vec![1u8; MIB as usize]).unwrap_err();
    assert_eq!((stale.status, stale.code.as_str()), (409, "invalid_range"));
    // From committed (a browser: offset): the last 10 bytes finish it.
    let r = as_person(&alex(), files::DATA, &q(&[("id", "5001"), ("offset", (3 * MIB).to_string().as_str())]), &[9u8; 10]).expect("last range");
    assert_eq!((r["upload"]["committed"].as_u64(), r["upload"]["complete"].as_bool()), (Some(size), Some(true)));
    assert_eq!(api.bytes_in.lock().unwrap().last().unwrap().1, format!("bytes {}-{}/{size}", 3 * MIB, size - 1));

    // The send: no text, the upload rides as Sew's upload_ids.
    api.answer("/api/v1/channels/900/messages", 200, json!({"id": "80", "channel_id": "900", "body": "", "attachments": [{"id": "5001", "filename": "report.pdf", "url": "/api/v1/files/5001?exp=1&sig=s"}]}));
    let m = call("/cli/home-chat/send", json!({"conv": "900", "text": "", "nonce": "f-1", "attachments": ["5001"]})).expect("send");
    no_sew_url(&m);
    assert_eq!(m["message"]["attachments"][0]["url"], json!("/cli/home-chat/file?id=5001"));
    let body = &api.calls_to("/api/v1/channels/900/messages")[0].1;
    assert_eq!((&body["upload_ids"], &body["body"]), (&json!(["5001"]), &json!("")));

    // Eleven files, a repeat, nothing at all: refused before Sew.
    let eleven: Vec<String> = (0..11).map(|i| format!("60{i:02}")).collect();
    let r = call("/cli/home-chat/send", json!({"conv": "900", "text": "x", "nonce": "f-2", "attachments": eleven})).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (400, "too_many_files"));
    let r = call("/cli/home-chat/send", json!({"conv": "900", "text": "x", "nonce": "f-3", "attachments": ["5001", "5001"]})).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (400, "usage"));
    let r = call("/cli/home-chat/send", json!({"conv": "900", "text": " ", "nonce": "f-4"})).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (400, "usage"));
    assert_eq!(api.calls_to("/api/v1/channels/900/messages").len(), 1);
    done();
}

/// A data URL past its time (Sew's 403 url_expired): one fresh URL from
/// `GET /uploads/{id}`, and the range goes again on it.
#[test]
fn an_expired_upload_url_is_renewed_once() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    sew_takes_upload(&api, "5003", 100);
    call(files::CREATE, json!({"conv": "900", "filename": "a.bin", "size": 100})).expect("create");
    api.bytes_queue
        .lock()
        .unwrap()
        .push_back((403, json!({"error": {"code": "policy_denied", "details": {"reason": "url_expired"}}})));
    sew_reports_upload(&api, "5003", "fresh");
    let r = as_person(&alex(), files::DATA, &q(&[("id", "5003"), ("offset", "0")]), &[1u8; 100]).expect("after a renew");
    assert_eq!(r["upload"]["complete"], json!(true));
    let paths: Vec<String> = api.bytes_in.lock().unwrap().iter().map(|b| b.0.clone()).collect();
    assert_eq!(paths.len(), 2, "{paths:?}");
    assert!(paths[0].ends_with("sig=first") && paths[1].ends_with("sig=fresh"), "{paths:?}");
    assert_eq!(api.calls_to("/api/v1/uploads/5003").len(), 1, "renewed once");
    done();
}

/// The size refusals: over READY's limit before Sew is asked; Sew's own
/// 413 keeps its `max`; a request over 8 MiB; an empty file. The status
/// route shows the limits.
#[test]
fn the_size_limits_refuse_with_413() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    follower::hold(ALEX).state.lock().unwrap().files_max_bytes = Some(1000);
    let r = call(files::CREATE, json!({"conv": "900", "filename": "big.mov", "size": 1001})).unwrap_err();
    assert_eq!((r.status, r.code.as_str(), r.details.clone()), (413, "too_large", Some(json!({"max": 1000}))));
    assert!(api.calls_to("/api/v1/channels/900/uploads").is_empty(), "refused before Sew");
    let st = call("/cli/home-chat/status", json!({})).expect("status");
    assert_eq!(st["files"], json!({"maxBytes": 1000, "perMessage": 10, "chunkBytes": MIB, "maxRequestBytes": files::MAX_REQUEST_BYTES}));

    follower::hold(ALEX).state.lock().unwrap().files_max_bytes = None;
    api.answer("/api/v1/channels/900/uploads", 413, json!({"error": {"code": "too_large", "hint": "too big", "details": {"max": 5}}}));
    let r = call(files::CREATE, json!({"conv": "900", "filename": "big.mov", "size": 6})).unwrap_err();
    assert_eq!((r.status, r.code.as_str(), r.details.clone()), (413, "too_large", Some(json!({"max": 5}))));
    let r = call(files::CREATE, json!({"conv": "900", "filename": "none", "size": 0})).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (400, "usage"));

    let r = as_person(&alex(), files::DATA, &q(&[("id", "5001"), ("offset", "0")]), &vec![0u8; files::MAX_REQUEST_BYTES + 1]).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (413, "chunk_too_large"));
    assert!(api.bytes_in.lock().unwrap().is_empty());
    done();
}

/// READY's policy digest (and `install_updated`) set the limit.
#[test]
fn ready_carries_the_upload_limit() {
    let mut st = PersonState::default();
    st.apply_ready(&json!({"member": {"id": "41"}, "install": {"policy_digest": {"files_max_bytes": 1073741824u64}}}));
    assert_eq!(st.files_max_bytes, Some(1 << 30));
    st.apply_event("install_updated", &json!({"policy_digest": {"files_max_bytes": 2048}}), false);
    assert_eq!(st.files_max_bytes, Some(2048));
    st.apply_ready(&json!({"member": {"id": "41"}}));
    assert_eq!(st.files_max_bytes, None);
}

/// Another login never reaches someone's upload or file: K2 keeps each
/// upload URL under its person, and asks Sew as the caller every time
/// (an earlier read by the owner of the file is never theirs).
#[test]
fn another_login_cant_use_or_fetch_anothers_files() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    api.per_person.store(true, Ordering::SeqCst);
    let (alex_tok, kim_tok) = (FakeApi::token_of(ALEX), FakeApi::token_of(KIM));
    assert_ne!(alex_tok, kim_tok);
    sew_takes_upload(&api, "5002", 10);
    call(files::CREATE, json!({"conv": "900", "filename": "a.txt", "size": 10})).expect("Alex's upload");
    let no = json!({"error": {"code": "not_found"}});
    api.by_bearer.lock().unwrap().insert((kim_tok.clone(), "/api/v1/uploads/5002".into()), (404, no.clone()));
    api.by_bearer.lock().unwrap().insert((kim_tok.clone(), "/api/v1/uploads/5002/abort".into()), (404, no.clone()));
    let r = as_person(&kim(), files::DATA, &q(&[("id", "5002"), ("offset", "0")]), &[1u8; 10]).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (404, "upload_not_found"));
    assert!(api.bytes_in.lock().unwrap().is_empty(), "Kim never reached Alex's data URL");
    let r = as_person(&kim(), files::STATUS, &q(&[("id", "5002")]), b"").unwrap_err();
    assert_eq!(r.code, "upload_not_found");
    let r = as_person(&kim(), files::ABORT, &q(&[]), json!({"id": "5002"}).to_string().as_bytes()).unwrap_err();
    assert_eq!(r.code, "upload_not_found");

    api.by_bearer.lock().unwrap().insert(
        (alex_tok.clone(), "/api/v1/files/7001".into()),
        (200, json!({"attachment": {"id": "7001", "filename": "a.pdf", "content_type": "application/pdf", "url": "/api/v1/files/7001?exp=1&sig=s"}, "message_id": "80", "channel_id": "900"})),
    );
    api.by_bearer.lock().unwrap().insert((kim_tok.clone(), "/api/v1/files/7001".into()), (404, no));
    api.files.lock().unwrap().insert("/api/v1/files/7001".into(), ("application/pdf".into(), "\"e1\"".into(), b"%PDF-1.7".to_vec()));
    let mut f = files::fetch(&alex(), &q(&[("id", "7001")]), &[]).expect("Alex reads his file");
    let mut got = Vec::new();
    f.body.read_to_end(&mut got).unwrap();
    assert_eq!(got, b"%PDF-1.7");
    let r = files::fetch(&kim(), &q(&[("id", "7001")]), &[]).map(|_| ()).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (404, "file_not_found"));
    assert_eq!(api.fetches.lock().unwrap().len(), 1, "Kim's fetch never reached the bytes");
    let asked: Vec<Option<String>> = api.calls_to("/api/v1/files/7001").into_iter().map(|c| c.0).collect();
    assert_eq!(asked, vec![Some(alex_tok), Some(kim_tok)], "each asked as themselves");
    done();
}

fn sew_has_file(api: &Arc<FakeApi>, id: &str, filename: &str, ct: &str, thumb: bool, bytes: &[u8]) {
    let mut att = json!({"id": id, "filename": filename, "content_type": ct, "url": format!("/api/v1/files/{id}?exp=1&sig=s")});
    if thumb {
        att["thumbnail_url"] = json!(format!("/api/v1/files/{id}/thumb?exp=1&sig=t"));
        api.files.lock().unwrap().insert(format!("/api/v1/files/{id}/thumb"), ("image/webp".into(), "\"t1\"".into(), b"thumb".to_vec()));
    }
    api.answer(&format!("/api/v1/files/{id}"), 200, json!({"attachment": att, "message_id": "80", "channel_id": "900"}));
    api.files.lock().unwrap().insert(format!("/api/v1/files/{id}"), (ct.into(), "\"e1\"".into(), bytes.to_vec()));
}

fn header<'a>(f: &'a files::Fetched, n: &str) -> Option<&'a str> {
    f.headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str())
}

/// Range, ETag and 304 pass through; every file has nosniff and the
/// sandbox CSP; only safe images and Sew's renders are inline; HTML is
/// octet-stream; a missing variant is `no_variant`.
#[test]
fn a_file_streams_with_range_etag_and_safe_headers() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    sew_has_file(&api, "7002", "notes.pdf", "application/pdf", false, b"0123456789");
    let fwd = files::forwarded_headers("GET /cli/home-chat/file?id=7002 HTTP/1.1\r\nRange: bytes=2-5\r\nHost: x\r\n\r\nRange: bytes=0-0");
    assert_eq!(fwd, vec![("range".to_string(), "bytes=2-5".to_string())], "only the head's headers");
    let mut f = files::fetch(&alex(), &q(&[("id", "7002")]), &fwd).expect("a range");
    let mut got = Vec::new();
    f.body.read_to_end(&mut got).unwrap();
    assert_eq!((f.status, got.as_slice()), (206, b"2345".as_slice()));
    assert_eq!(header(&f, "content-range"), Some("bytes 2-5/10"));
    assert_eq!(header(&f, "etag"), Some("\"e1\""));
    assert_eq!(header(&f, "x-content-type-options"), Some("nosniff"));
    assert_eq!(header(&f, "content-security-policy"), Some(files::FILE_CSP));
    assert_eq!(header(&f, "content-disposition"), Some("attachment; filename=\"notes.pdf\"; filename*=UTF-8''notes.pdf"));
    let head = files::head_of(&f);
    assert!(head.starts_with("HTTP/1.1 206 Partial Content\r\n") && head.contains("Connection: close\r\n"), "{head}");
    let path = &api.fetches.lock().unwrap()[0].0;
    assert_eq!(path, "/api/v1/files/7002?exp=1&sig=s", "fetched on Sew's loopback path");

    let f = files::fetch(&alex(), &q(&[("id", "7002")]), &[("if-none-match".into(), "\"e1\"".into())]).expect("unchanged");
    assert_eq!(f.status, 304);
    // A browser's ranged read: `range=` in the query.
    let mut f = files::fetch(&alex(), &q(&[("id", "7002"), ("range", "7-9")]), &[]).expect("a query range");
    let mut got = Vec::new();
    f.body.read_to_end(&mut got).unwrap();
    assert_eq!((f.status, got.as_slice()), (206, b"789".as_slice()));
    let r = files::fetch(&alex(), &q(&[("id", "7002"), ("range", "x-1")]), &[]).map(|_| ()).unwrap_err();
    assert_eq!(r.code, "usage");

    sew_has_file(&api, "7003", "cat.png", "image/png", true, b"png!");
    let f = files::fetch(&alex(), &q(&[("id", "7003")]), &[]).expect("a png");
    assert!(header(&f, "content-disposition").unwrap().starts_with("inline;"));
    let f = files::fetch(&alex(), &q(&[("id", "7003"), ("dl", "1")]), &[]).expect("a png to save");
    assert!(header(&f, "content-disposition").unwrap().starts_with("attachment;"));
    let f = files::fetch(&alex(), &q(&[("id", "7003"), ("v", "thumb")]), &[]).expect("its thumbnail");
    assert_eq!((header(&f, "content-type"), header(&f, "content-disposition").map(|d| d.starts_with("inline;"))), (Some("image/webp"), Some(true)));
    let r = files::fetch(&alex(), &q(&[("id", "7003"), ("v", "poster")]), &[]).map(|_| ()).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (404, "no_variant"));
    let r = files::fetch(&alex(), &q(&[("id", "7003"), ("v", "big")]), &[]).map(|_| ()).unwrap_err();
    assert_eq!(r.code, "usage");

    sew_has_file(&api, "7004", "page \"x\".html", "text/html; charset=utf-8", false, b"<script>");
    let f = files::fetch(&alex(), &q(&[("id", "7004")]), &[]).expect("html");
    assert_eq!(header(&f, "content-type"), Some("application/octet-stream"));
    assert_eq!(header(&f, "content-disposition"), Some("attachment; filename=\"page _x_.html\"; filename*=UTF-8''page%20%22x%22.html"));
    done();
}

/// No Sew URL leaves the daemon: the messages page, and a frame on the
/// person's socket (`attachments_updated` included), carry K2's paths.
#[test]
fn attachments_carry_k2_paths_never_sews() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    let att = json!({
        "id": "7001", "filename": "cat.png", "kind": "image", "width": 800, "height": 600, "media_pending": false,
        "url": "/api/v1/files/7001?exp=1&sig=s", "thumbnail_url": "/api/v1/files/7001/thumb?exp=1&sig=t",
        "preview_url": null, "url_expires_at": "2026-11-24T00:00:00Z",
    });
    api.answer("/api/v1/channels/900/messages?limit=50", 200, json!({"items": [{"id": "1", "channel_id": "900", "attachments": [att]}]}));
    let page = call("/cli/home-chat/messages", json!({})).map(|_| ()).unwrap_err();
    assert_eq!(page.code, "usage", "a page names its conversation");
    let page = as_person(&alex(), "/cli/home-chat/messages", &q(&[("conv", "900")]), b"").expect("page");
    no_sew_url(&page);
    let a = &page["page"]["items"][0]["attachments"][0];
    assert_eq!(a["url"], json!("/cli/home-chat/file?id=7001"));
    assert_eq!(a["thumbnail_url"], json!("/cli/home-chat/file?id=7001&v=thumb"));
    assert!(a.get("preview_url").is_none() && a.get("url_expires_at").is_none(), "{a}");
    assert_eq!((a["width"].as_u64(), a["kind"].as_str()), (Some(800), Some("image")));

    let h = follower::hold(ALEX);
    let mut rx = h.events.subscribe();
    let frames = h.state.lock().unwrap().apply_event("attachments_updated", &json!({"channel_id": "900", "message_id": "1", "attachments": [att]}), false);
    h.publish(&frames);
    let f: Value = serde_json::from_str(&rx.try_recv().expect("a frame")).unwrap();
    no_sew_url(&f);
    assert_eq!((f["type"].as_str(), f["conv"].as_str(), f["msg"].as_str()), (Some("attachments_updated"), Some("900"), Some("1")));
    assert_eq!(f["attachments"][0]["url"], json!("/cli/home-chat/file?id=7001"));
    done();
}

/// The file route answers 405 to a POST; the upload POST routes answer
/// 405 to a GET (with every other POST route: `every_post_route_refuses_get`);
/// the capability key is off while Home chat is.
#[test]
fn file_routes_guard_their_methods_and_the_key_follows_home_chat() {
    let r = routes::handle(files::FILE, Ok(alex()), true, &HashMap::new(), b"");
    assert!(r.status.starts_with("405"), "{}", r.status);
    for p in [files::CREATE, files::DATA, files::ABORT] {
        assert!(routes::is_post_route(p), "{p}");
        let r = routes::handle(p, Ok(alex()), false, &HashMap::new(), b"");
        assert!(r.status.starts_with("405"), "{p}: {}", r.status);
    }
    assert!(routes::is_route(files::FILE) && routes::is_route(files::STATUS));
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    assert_eq!(super::boot_status_key(), None);
    assert_eq!(super::attachments_boot_status_key(), None, "listed only beside home-chat-v1");
}
