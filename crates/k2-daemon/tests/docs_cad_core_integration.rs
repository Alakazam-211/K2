//! The CAD core, headless: the real dispatcher (`test_harness`), the real
//! docs runtime (the vendored kit and the CAD and AV packs, a temp `~/.k2`),
//! driven over HTTP and the docs socket with no client. Every drawing is one
//! the packs' own tests generate (the DXF pack's sampler, the DWG reader's
//! container builder); no outside file.
//!
//! What it pins:
//!   1. Off (the default), a drawing is the any-file core's
//!      (`docs/cores?path`), and the CAD routes answer `403 core_off` for the
//!      CAD core; a file that isn't a drawing is `400 not_a_drawing`.
//!   2. On, the drawing is the CAD core's (`liveView: false`). Its versions
//!      follow outside saves; at version 7 the viewer's route
//!      (`/cli/docs/cad/draw`) answers the sampler's header and pages BYTE FOR
//!      BYTE as the pack's own goldens (the files K2 Design's viewer fixtures
//!      are copied from): the pack's JSON as the whole body.
//!   3. A cursor from another version is `409 stale_cursor` (`CAD-E013`).
//!   4. The read verbs (`/cli/docs/cad/read`) and the DXF index's `entity
//!      list` answer; the AV reading answers in full and as a summary, and a
//!      bad configuration is `400 bad_config`.
//!   5. A DWG: routed to the DWG reader; the builder's container (no drawing
//!      objects) is refused typed (`DWG-E…`, 422); a DXF-only verb is 400.
//!   6. Writes are refused `409 cad_writer_disabled` (exit 3, the fixed
//!      words): write, restore, undo, the pack's edit verbs, on a DXF and a
//!      DWG; the docs socket declines a drawing with the same code. The
//!      files are untouched.
//!   7. After every refusal the drawing reads as before, at the same version,
//!      its disk state clean.
//!   8. Off again: every CAD route is `core_off` once more.
//!
//! ISOLATION: one test (the docs runtime is process-wide), temp HOME.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use futures_util::StreamExt;
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value as J};
use tokio_tungstenite::tungstenite::Message;

/// The DWG reader's own test builder: a container with the sections it
/// chooses (no drawing objects), from the pack's tests.
#[path = "../../../vendor/k2-mpc-cad/crates/dwg/tests/support/mod.rs"]
#[allow(dead_code, unused_imports)]
mod dwg_support;

const OWNER: &str = "owner-token-docs-cad-core";
const WHY: &str = "K2 can't save drawing edits yet; this file opens read-only.";

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).expect("timeout");
    let b = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    );
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    match s.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("read: {e:?}"),
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_else(|| panic!("no body: {text:?}"));
    (status, body)
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn json_of(what: &str, body: &str) -> J {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{what}: not JSON ({e}): {body}"))
}

async fn flip(cores: &[(&str, bool)]) -> k2_daemon::docs::cores::Report {
    let after: std::collections::BTreeMap<String, bool> = cores.iter().map(|(c, on)| (c.to_string(), *on)).collect();
    tokio::task::spawn_blocking(move || k2_daemon::docs::cores::sync_multiplayer_cores(&after, "owner-token"))
        .await
        .expect("flip")
}

fn file_core(port: u16, path: &str) -> J {
    let (st, body) = http(port, "GET", &format!("/cli/docs/cores?token={OWNER}&path={}", enc(path)), None);
    assert_eq!(st, 200, "cores {path}: {body}");
    json_of("cores", &body)["file"].clone()
}

fn doc_status(port: u16, path: &str) -> J {
    let (st, body) = http(port, "GET", &format!("/cli/docs/status?token={OWNER}&path={}", enc(path)), None);
    assert_eq!(st, 200, "status {path}: {body}");
    json_of("status", &body)
}

/// `GET /cli/docs/cad/draw` on `path` with `q` (already encoded).
fn draw(port: u16, path: &str, q: &str) -> (u16, String) {
    http(port, "GET", &format!("/cli/docs/cad/draw?token={OWNER}&path={}{q}", enc(path)), None)
}

/// `GET /cli/docs/cad/read` on `path` with `argv`.
fn read(port: u16, path: &str, argv: &[&str]) -> (u16, J) {
    let a = serde_json::to_string(argv).expect("argv");
    let (st, body) = http(port, "GET", &format!("/cli/docs/cad/read?token={OWNER}&path={}&argv={}", enc(path), enc(&a)), None);
    (st, json_of("cad read", &body))
}

/// `GET /cli/docs/av/read`.
fn av(port: u16, config: &str, paths: &[&str], what: &str) -> (u16, J) {
    let p = serde_json::to_string(paths).expect("paths");
    let (st, body) = http(port, "GET", &format!("/cli/docs/av/read?token={OWNER}&config={}&paths={}&what={what}", enc(config), enc(&p)), None);
    (st, json_of("av read", &body))
}

/// `POST /cli/docs/tool` on `path`: (status, the answer).
fn tool(port: u16, path: &str, argv: &[&str]) -> (u16, J) {
    let (st, body) = http(port, "POST", &format!("/cli/docs/tool?token={OWNER}&path={}", enc(path)), Some(&json!({ "argv": argv }).to_string()));
    (st, json_of("tool", &body))
}

/// An index verb on a tier-0 doc: retried while its index is being built
/// (`index_pending`), at most 60 s. The answer, exit 0.
fn index_verb(port: u16, path: &str, argv: &[&str]) -> J {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let (st, v) = tool(port, path, argv);
        if st == 200 && v["exit"] == 0 {
            return v;
        }
        assert!(v.to_string().contains("index_pending"), "{argv:?} on {path}: {st} {v}");
        assert!(std::time::Instant::now() < deadline, "{argv:?} on {path}: still pending: {v}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// `body` is a refusal with `code` (and, when given, `status`).
fn refused(what: &str, st: u16, body: &str, status: u16, code: &str) -> J {
    let v = json_of(what, body);
    assert_eq!((st, &v["ok"], &v["error"]["code"]), (status, &json!(false), &json!(code)), "{what}: {body}");
    v
}

/// Save `bytes` over `path` as an app does (a new file renamed over the old
/// one), so a watcher never sees it half written.
fn save(path: &std::path::Path, bytes: &[u8]) {
    let tmp = path.with_extension("saving");
    std::fs::write(&tmp, bytes).expect("write the new file");
    std::fs::rename(&tmp, path).expect("rename it over the drawing");
}

/// The sampler's goldens, as the CAD pack wrote them at its vendored commit.
fn golden(name: &str) -> String {
    let p = format!("{}/../../vendor/k2-mpc-cad/crates/dxf/tests/golden/draw-sampler-2018-{name}.json", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cad_core_headless() {
    let tmp = std::env::temp_dir().join(format!("k2-docs-cad-core-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join(".k2")).expect("home");
    let tmp = tmp.canonicalize().expect("canonical home");
    std::env::set_var("HOME", &tmp);
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }

    let daemon = test_harness::start(OWNER).await;
    let port = daemon.port;
    tokio::task::spawn_blocking(k2_daemon::docs::start).await.expect("docs start");
    assert!(k2_daemon::docs::live_ready(), "the docs runtime is up: {}", k2_daemon::docs::pool_json());

    let ws_dir = tmp.join("ws-cad");
    std::fs::create_dir_all(ws_dir.join(".k2")).expect("ws");
    let ws_dir = ws_dir.canonicalize().expect("canonical ws");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'cadws', ?2, 'cadws')",
            params![uuid::Uuid::new_v4().to_string(), ws_dir.to_string_lossy()],
        )
        .expect("seed project");
    }
    let sampler = k2_mpc_dxf::testgen::draw_sampler(k2_mpc_dxf::testgen::Ver::R2018).build().bytes;
    let plan_p = ws_dir.join("plan.dxf");
    // The first version: the sampler with one byte after its end marker
    // (the reader keeps bytes there as they are).
    std::fs::write(&plan_p, [sampler.as_slice(), b"x"].concat()).expect("the drawing");
    let plan = plan_p.to_string_lossy().to_string();
    let dwg_bytes = dwg_support::container::build(&dwg_support::container::standard()).bytes;
    let site_p = ws_dir.join("site.dwg");
    std::fs::write(&site_p, &dwg_bytes).expect("the DWG");
    let site = site_p.to_string_lossy().to_string();
    let notes_p = ws_dir.join("notes.md");
    std::fs::write(&notes_p, "# notes\n").expect("notes");
    let notes = notes_p.to_string_lossy().to_string();
    let cfg_p = ws_dir.join("av.cfg");
    std::fs::write(&cfg_p, "device.tags = DEVICE, TAG\ntitle.number_from = blocks, file-stem\n").expect("the AV configuration");
    let cfg = cfg_p.to_string_lossy().to_string();
    let bad_cfg_p = ws_dir.join("bad.cfg");
    std::fs::write(&bad_cfg_p, "no.such.key = 1\n").expect("a bad configuration");
    let bad_cfg = bad_cfg_p.to_string_lossy().to_string();

    // ── 1. Off: any-file's; the CAD routes answer core_off for the CAD core ──
    for p in [&plan, &site] {
        assert_eq!(file_core(port, p), json!({"core": "any-file", "on": false, "liveView": false}), "{p}");
    }
    flip(&[("any-file", true)]).await;
    let core_off = |what: &str, st: u16, body: &str| {
        let v = refused(what, st, body, 403, "core_off");
        assert_eq!((&v["error"]["core"], &v["error"]["exit"]), (&json!("cad"), &json!(3)), "{what}: {body}");
    };
    for p in [&plan, &site] {
        let (st, body) = draw(port, p, "&what=header");
        core_off("draw, the core off", st, &body);
        let (st, v) = read(port, p, &["drawing", "info"]);
        core_off("read, the core off", st, &v.to_string());
    }
    let (st, v) = av(port, &cfg, &[&plan], "summary");
    core_off("av, the core off", st, &v.to_string());
    let (st, body) = draw(port, &notes, "&what=header");
    refused("draw on a .md", st, &body, 400, "not_a_drawing");

    // ── 2. On: the CAD core's; the viewer's route at version 7, byte for byte ──
    let on = flip(&[("any-file", true), ("cad", true)]).await;
    assert!(on.changed.contains(&("cad".to_string(), true)), "{on:?}");
    for p in [&plan, &site] {
        assert_eq!(file_core(port, p), json!({"core": "cad", "on": true, "liveView": false}), "{p}");
    }
    let header_version = |what: &str| -> u64 {
        let (st, body) = draw(port, &plan, "&what=header");
        assert_eq!(st, 200, "{what}: {body}");
        let h = json_of(what, &body);
        assert_eq!(h["schema"], "cad.drawing.header/1", "{what}: {body}");
        h["version"].as_u64().unwrap_or_else(|| panic!("{what}: a version: {body}"))
    };
    let mut v = header_version("the first read");
    assert!(v < 7, "the first version is {v}");
    let mut extra = 1;
    while v < 6 {
        extra += 1;
        save(&plan_p, &[sampler.as_slice(), "x".repeat(extra).as_bytes()].concat());
        let next = header_version("an outside save");
        assert_eq!(next, v + 1, "each outside save is one version");
        v = next;
    }
    save(&plan_p, &sampler);
    assert_eq!(header_version("the sampler itself"), 7, "the sampler at version 7");
    let (st, header) = draw(port, &plan, "&what=header");
    assert_eq!(st, 200, "{header}");
    assert!(header == golden("header"), "the header differs from the pack's golden:\n{header}");
    let h = json_of("header", &header);
    let block = |name: &str| -> String {
        h["blocks"]
            .as_array()
            .and_then(|b| b.iter().find(|r| r["name"] == name))
            .and_then(|r| r["handle"].as_str())
            .unwrap_or_else(|| panic!("no block {name} in the header"))
            .to_string()
    };
    for (space, name) in [("model".to_string(), "page-model"), (block("DOOR"), "page-door"), (block("NEST"), "page-nest"), (block("*D1"), "page-dimension")] {
        let (st, page) = draw(port, &plan, &format!("&what=page&space={}", enc(&space)));
        assert_eq!(st, 200, "{space}: {page}");
        assert!(page == golden(name), "{name} differs from the pack's golden:\n{page}");
        assert_eq!(json_of(name, &page)["schema"], "cad.drawing.page/1");
    }
    // Bad requests.
    let (st, body) = draw(port, &plan, "&what=page");
    refused("a page with no space", st, &body, 400, "bad_request");
    let (st, body) = draw(port, &plan, "&what=everything");
    refused("what=everything", st, &body, 400, "bad_request");

    // ── 3. A cursor from another version is stale ──
    let (st, body) = draw(port, &plan, &format!("&what=page&space=model&cursor={}", enc("v6:model:0")));
    let e = refused("a cursor from version 6", st, &body, 409, "stale_cursor");
    assert_eq!(e["error"]["id"], "CAD-E013", "{body}");

    // ── 4. The read verbs, the index, the AV reading ──
    let (st, info) = read(port, &plan, &["drawing", "info"]);
    assert_eq!(st, 200, "{info}");
    assert_eq!((&info["ok"], &info["version"], &info["verb"], &info["shape"]), (&json!(true), &json!(7), &json!("drawing info"), &json!("cad.drawing/1")), "{info}");
    let (st, layers) = read(port, &plan, &["layer", "list"]);
    assert_eq!((st, &layers["shape"]), (200, &json!("cad.layer-list/1")), "{layers}");
    assert!(layers["stdout"].as_str().is_some_and(|s| !s.is_empty()), "{layers}");
    let (st, dh) = read(port, &plan, &["draw", "header"]);
    assert_eq!((st, &dh["data"]), (200, &h), "draw header through the read verbs is the viewer's: {dh}");
    let (st, v) = read(port, &plan, &["draw", "header", "--version", "3"]);
    refused("a caller's --version", st, &v.to_string(), 400, "bad_request");
    let (st, v) = read(port, &plan, &["nothing", "here"]);
    refused("not a read verb", st, &v.to_string(), 400, "bad_request");
    let entities = index_verb(port, &plan, &["entity", "list"]);
    assert!(entities["stdout"].as_str().is_some_and(|s| s.contains("@h:")), "{entities}");
    let (st, set) = av(port, &cfg, &[&plan], "read");
    assert_eq!((st, &set["ok"], &set["shape"]), (200, &json!(true), &json!("av.set/1")), "{set}");
    assert_eq!(set["sheets_read"], json!([{"path": "plan.dxf", "version": 7}]), "{set}");
    assert!(set["data"]["sheets"].is_array(), "{set}");
    let (st, sum) = av(port, &cfg, &[&plan], "summary");
    assert_eq!((st, &sum["drawings"]), (200, &json!(1)), "{sum}");
    assert!(sum["summary"].as_str().is_some_and(|s| s.starts_with("sheets 1 ")), "{sum}");
    assert!(!sum.to_string().contains("plan.dxf"), "a summary names no file: {sum}");
    let (st, v) = av(port, &bad_cfg, &[&plan], "read");
    let e = refused("a bad configuration", st, &v.to_string(), 400, "bad_config");
    assert_eq!(e["error"]["id"], "AV-E001", "{e}");
    let (st, v) = av(port, &cfg, &[&notes], "read");
    refused("a set with a .md", st, &v.to_string(), 400, "not_a_drawing");

    // ── 5. A DWG: the DWG reader, refused typed (the builder writes no objects) ──
    let (st, body) = draw(port, &site, "&what=header");
    let e = json_of("draw header on the builder's DWG", &body);
    assert_eq!((st, &e["ok"]), (422, &json!(false)), "{body}");
    assert!(e["error"]["id"].as_str().is_some_and(|id| id.starts_with("DWG-E")), "the DWG reader's refusal: {body}");
    assert!(e["error"]["code"].as_str().is_some_and(|c| !c.is_empty()), "the refusal's name: {body}");
    let (st, v) = read(port, &site, &["layer", "list"]);
    assert_eq!(st, 422, "{v}");
    assert!(v["error"]["id"].as_str().is_some_and(|id| id.starts_with("DWG-E")), "{v}");
    let (st, v) = read(port, &site, &["check"]);
    refused("check on a DWG", st, &v.to_string(), 400, "bad_request");

    // ── 6. Writes refused, the socket declines; the files untouched ──
    let before = std::fs::read(&plan_p).expect("the drawing");
    let status_before = doc_status(port, &plan);
    assert_eq!((&status_before["version"], &status_before["disk"]["state"]), (&json!(7), &json!("clean")), "{status_before}");
    for p in [&plan, &site] {
        for (route, body) in [
            ("/cli/docs/write", json!({"path": p, "base": "7", "bytesB64": "AAAA"})),
            ("/cli/docs/restore", json!({"path": p, "version": 1})),
            ("/cli/docs/undo", json!({"path": p})),
        ] {
            let (st, out) = http(port, "POST", &format!("{route}?token={OWNER}"), Some(&body.to_string()));
            let v = refused(route, st, &out, 409, "cad_writer_disabled");
            assert_eq!((&v["error"]["hint"], &v["error"]["exit"]), (&json!(WHY), &json!(3)), "{route}: {out}");
        }
        let (st, v) = tool(port, p, &["entity", "move", "@h:2B1", "--by", "10,0", "--unit", "mm"]);
        refused("an edit verb", st, &v.to_string(), 409, "cad_writer_disabled");
        let url = format!("ws://127.0.0.1:{port}/cli/docs/live?token={OWNER}&doc={}", enc(p));
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("a socket");
        let f = loop {
            match tokio::time::timeout(Duration::from_secs(60), ws.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => break json_of("socket frame", &t),
                Ok(Some(Ok(_))) => continue,
                other => panic!("no frame from the socket on {p}: {other:?}"),
            }
        };
        assert_eq!((&f["type"], &f["reason"], &f["code"]), (&json!("closed"), &json!("declined"), &json!("cad_writer_disabled")), "{f}");
    }
    assert_eq!(std::fs::read(&plan_p).expect("the drawing"), before, "the DXF is untouched");
    assert_eq!(std::fs::read(&site_p).expect("the DWG"), dwg_bytes, "the DWG is untouched");

    // ── 7. After every refusal: the same reads at the same version ──
    for when in ["after the refusals", "after the disk ticks"] {
        let (st, again) = draw(port, &plan, "&what=header");
        assert_eq!(st, 200, "{when}: {again}");
        assert!(again == golden("header"), "{when}: the header differs");
        let (st, page) = draw(port, &plan, "&what=page&space=model");
        assert!(st == 200 && page == golden("page-model"), "{when}: the model page differs ({st})");
        let s = doc_status(port, &plan);
        assert_eq!((&s["version"], &s["disk"]["state"]), (&json!(7), &json!("clean")), "{when}: {s}");
        std::thread::sleep(Duration::from_millis(1500));
    }

    // ── 8. Off again: core_off ──
    let off = flip(&[("any-file", true), ("cad", false)]).await;
    assert!(off.changed.contains(&("cad".to_string(), false)), "{off:?}");
    let (st, body) = draw(port, &plan, "&what=header");
    core_off("draw, the core off again", st, &body);
    let (st, v) = av(port, &cfg, &[&plan], "read");
    core_off("av, the core off again", st, &v.to_string());
    assert_eq!(file_core(port, &plan), json!({"core": "any-file", "on": true, "liveView": false}));

    flip(&[]).await;
    drop(daemon);
}
