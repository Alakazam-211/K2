//! `k2 app updates …` in process (TC-6, TC-8 … TC-11): the TEST minisign
//! key (TA-16), a stand-in download host on loopback, a runtime App on box
//! `box`. Each push uploads in the background; tests wait for it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine as _;
use k2_core::app_updates::{fake_host::FakeHost, store};
use k2_core::connect_users::Role;
use k2_root_proto::testsign;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;

const TOKEN_A: &str = "k2dl_0123456789abcdef0123456789abcdef";

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    _env: k2_core::test_env::EnvLock,
    _dl_base: k2_core::test_env::EnvVar,
    app_id: String,
    ws: String,
    ws_path: std::path::PathBuf,
    other_ws: String,
    host: FakeHost,
}

fn fx(tag: &str) -> Fx {
    let app_id = format!("upd{tag}");
    let host = FakeHost::start(&[(&format!("dev.k2.box.{app_id}"), TOKEN_A)]);
    let base = host.base.clone();
    fx_on(tag, "box", app_id, host, &base)
}

/// A runtime App `app_id` on box `sub`, the download host at `base`.
fn fx_on(tag: &str, sub: &str, app_id: String, host: FakeHost, base: &str) -> Fx {
    let env = k2_core::test_env::lock();
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    crate::vault::ensure_test_vault();
    let cfg = k2_core::tunnel::config::TunnelConfig { subdomain: sub.into(), ..Default::default() };
    k2_core::tunnel::config::save(&cfg).expect("tunnel config");
    let (ws, other_ws) = (format!("ws-{tag}"), format!("ot-{tag}"));
    let ws_path = home.path().join(&ws);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for r in [&ws, &other_ws] {
            let path = home.path().join(r.as_str());
            std::fs::create_dir_all(&path).expect("project dir");
            conn.execute(
                "INSERT INTO projects (id, name, path, handle, agents_can_manage_skin) VALUES (?1, ?1, ?2, ?1, 1)",
                params![r, path.to_string_lossy()],
            )
            .expect("project");
        }
        let a = k2_core::published_services::insert(&conn, &ws, &app_id, "node s.js", "/tmp", 4330, "tunnel", "running", "cmd", "").expect("svc");
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: &app_id,
                service_id: &a.id,
                seed_id: None,
                owner_workspace: &ws,
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "test",
            },
        )
        .expect("runtime app");
    }
    let dl_base = k2_core::test_env::EnvVar::set("K2_DL_BASE", base);
    super::super::updates_serve::reset_for_test();
    Fx { _db: db, _home: home, _env: env, _dl_base: dl_base, app_id, ws, ws_path, other_ws, host }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn call(path: &str, who: &Who, body: J, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    match handle(path, who, body.to_string().as_bytes(), &p) {
        Ok(v) if v.get("status").and_then(J::as_str) == Some("held") => (202, v),
        Ok(v) => (200, v),
        Err(e) => (e.status, json!({ "error": { "code": e.code, "hint": e.hint, "extra": e.extra } })),
    }
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

fn test_key_line() -> String {
    k2_root_proto::keys::TEST_KEY.public_key_b64.to_string()
}

/// A Tauri-style artifact and its `.sig` (base64 of the minisig) in the
/// workspace.
fn artifact(f: &Fx, name: &str, bytes: &[u8]) -> String {
    let p = f.ws_path.join(name);
    std::fs::write(&p, bytes).expect("write artifact");
    let sig = testsign::sign_with_test_key(bytes, &format!("timestamp:1\tfile:{name}"));
    std::fs::write(format!("{}.sig", p.display()), base64::engine::general_purpose::STANDARD.encode(sig)).expect("write sig");
    p.to_string_lossy().to_string()
}

fn ready(f: &Fx) {
    let (st, v) = call(KEY_SET, &agent(&f.ws), json!({ "app": f.app_id, "pubkey": test_key_line() }), &[]);
    assert_eq!(st, 200, "{v}");
    let (st, v) = call(TOKEN_SET, &agent(&f.ws), json!({ "app": f.app_id, "token": TOKEN_A }), &[]);
    assert_eq!(st, 200, "{v}");
}

fn wait_state(f: &Fx, release: &str) -> store::ReleaseRow {
    let start = Instant::now();
    loop {
        let r = {
            let h = k2_core::db::shared();
            let conn = h.lock();
            store::release(&conn, release).expect("query").expect("row")
        };
        if r.state != store::STATE_UPLOADING {
            return r;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "{} {release} still uploading", f.app_id);
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn push(f: &Fx, version: &str, ch: &str, arts: J) -> (u16, J) {
    call(PUSH, &agent(&f.ws), json!({ "app": f.app_id, "channel": ch, "version": version, "artifacts": arts }), &[])
}

fn push_live(f: &Fx, version: &str, ch: &str, arts: J) -> store::ReleaseRow {
    let (st, v) = push(f, version, ch, arts);
    assert_eq!(st, 200, "{v}");
    let r = wait_state(f, v["release"].as_str().expect("release id"));
    assert_eq!(r.state, store::STATE_LIVE, "{r:?}");
    r
}

fn serve(f: &Fx, path: &str) -> (String, String) {
    let bytes = super::super::updates_serve::answer(&f.app_id, "GET", path).expect("K2 answers update paths");
    let text = String::from_utf8(bytes).expect("utf8");
    let (head, body) = text.split_once("\r\n\r\n").expect("head");
    (head.lines().next().unwrap_or("").to_string(), body.to_string())
}

// ── key ──────────────────────────────────────────────────────────────

#[test]
fn the_key_is_public_only_and_a_change_after_releases_is_held() {
    let f = fx("key");
    let sk = base64::engine::general_purpose::STANDARD.encode(format!(
        "untrusted comment: rsign encrypted secret key\n{}\n",
        base64::engine::general_purpose::STANDARD.encode([b"EdSc".as_slice(), &[0u8; 154]].concat())
    ));
    let (st, v) = call(KEY_SET, &agent(&f.ws), json!({ "app": f.app_id, "pubkey": sk }), &[]);
    assert_eq!((st, code(&v)), (400, "secret_key_refused"), "{v}");
    ready(&f);
    let (st, v) = call(KEY, &agent(&f.ws), json!({}), &[("app", &f.app_id)]);
    assert_eq!(st, 200);
    assert_eq!(v["keyId"], k2_root_proto::keys::TEST_KEY.key_id.to_ascii_uppercase());
    // Another workspace's agent never sees the App.
    let (st, v) = call(KEY, &agent(&f.other_ws), json!({}), &[("app", &f.app_id)]);
    assert_eq!((st, code(&v)), (404, "app_not_found"), "{v}");
    // A Member reads, never sets.
    let member = Who::Login { username: "m".into(), role: Role::Member };
    let (st, v) = call(KEY_SET, &member, json!({ "app": f.app_id, "pubkey": test_key_line() }), &[]);
    assert_eq!(st, 403, "{v}");
    // With a release out, an agent's change is held for the owner.
    let p = artifact(&f, "A.app.tar.gz", b"one");
    push_live(&f, "1.0.0", "stable", json!([{ "platform": "darwin-aarch64", "path": p }]));
    let other = testsign::public_key_b64(&[9u8; 32], [9, 8, 7, 6, 5, 4, 3, 2]);
    let (st, v) = call(KEY_SET, &agent(&f.ws), json!({ "app": f.app_id, "pubkey": other }), &[]);
    assert_eq!(st, 202, "held: {v}");
    assert_eq!(v["code"], "key_change_held");
    let (_, shown) = call(KEY, &agent(&f.ws), json!({}), &[("app", &f.app_id)]);
    assert_eq!(shown["keyId"], k2_root_proto::keys::TEST_KEY.key_id.to_ascii_uppercase(), "unchanged until approved");
    // The owner applies it at once (and it's audited).
    let (st, v) = call(KEY_SET, &Who::Owner, json!({ "app": f.app_id, "pubkey": other }), &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["changed"], true);
}

// ── push ─────────────────────────────────────────────────────────────

#[test]
fn a_push_uploads_once_moves_the_channel_and_k2_answers_tauri() {
    let f = fx("push");
    ready(&f);
    let mac = artifact(&f, "My App.app.tar.gz", b"mac archive bytes");
    let win = artifact(&f, "setup.nsis.zip", b"windows archive bytes");
    let r = push_live(&f, "1.4.0", "stable", json!([{ "platform": "darwin-aarch64", "path": mac }, { "platform": "windows-x86_64", "path": win }]));
    {
        let g = f.host.lock();
        assert_eq!(g.uploads, 2, "each file once");
        assert_eq!(g.files.len(), 2);
        assert!(g.manifests.contains_key(&(format!("dev.k2.box.{}", f.app_id), "stable".to_string())), "the static fallback is published");
    }
    assert!(!super::staging_root(&f.app_id).join(&r.id).exists(), "staging goes once uploaded");
    // The static answer points at the download host, never the box.
    let (status, body) = serve(&f, "/_k2/updates/stable/latest.json");
    assert_eq!(status, "HTTP/1.1 200 OK");
    let m: J = serde_json::from_str(&body).expect("json");
    assert_eq!(m["version"], "1.4.0");
    let url = m["platforms"]["darwin-aarch64"]["url"].as_str().expect("url");
    assert!(url.starts_with(&format!("{}/dev.k2.box.{}/1.4.0/", f.host.base, f.app_id)), "{url}");
    assert!(url.ends_with("/My-App.app.tar.gz"), "{url}");
    assert!(!body.contains("mac archive bytes"), "the box never serves archive bytes");
    // The archive is fetchable from the stand-in host at that URL.
    let got = reqwest::blocking::get(url).expect("fetch").bytes().expect("bytes");
    assert_eq!(&got[..], b"mac archive bytes");
    // Dynamic: an update for an older caller, 204 for a current one.
    let (status, body) = serve(&f, "/_k2/updates/stable/darwin/aarch64/1.3.0?install=abcdef12-3456");
    assert_eq!(status, "HTTP/1.1 200 OK");
    let d: J = serde_json::from_str(&body).expect("json");
    assert_eq!(d["version"], "1.4.0");
    assert!(d["signature"].as_str().is_some_and(|s| !s.is_empty()));
    let (status, _) = serve(&f, "/_k2/updates/stable/darwin/aarch64/1.4.0?install=abcdef12-3456");
    assert_eq!(status, "HTTP/1.1 204 No Content");
    let (status, _) = serve(&f, "/_k2/updates/nightly/latest.json");
    assert_eq!(status, "HTTP/1.1 404 Not Found");
    let post = super::super::updates_serve::answer(&f.app_id, "POST", "/_k2/updates/stable/latest.json").expect("answer");
    assert!(String::from_utf8_lossy(&post).starts_with("HTTP/1.1 405"));
    assert!(super::super::updates_serve::answer(&f.app_id, "GET", "/api/x").is_none(), "other paths are the App's");
    // The per-App answerer serves it (App up or down, through the router).
    let a = super::super::local_answers::for_app(&f.app_id).expect("answerer");
    assert!(a("GET", "/_k2/updates/stable/latest.json", Some("x")).is_some());
    // Check-ins: two checks, one install, counted by platform and version.
    let (st, v) = call(LIST, &agent(&f.ws), json!({}), &[("app", &f.app_id)]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["channels"][0]["version"], "1.4.0");
    assert_eq!(v["activeUsers"]["counting"], "installs");
    assert_eq!(v["activeUsers"]["today"], 1, "{v}");
    let files = v["releases"][0]["files"].as_array().expect("files");
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|x| x["upload"] == "uploaded" && x["size"].as_i64().is_some_and(|n| n > 0)));
    let rows = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::checkins(&conn, &f.app_id, "2000-01-01").expect("rows")
    };
    // Two dynamic checks (one per caller version), one install (the same
    // id on the same day counts once, whatever version it was on).
    let mac_rows: Vec<_> = rows.iter().filter(|r| r.platform == "darwin-aarch64").collect();
    assert_eq!(mac_rows.iter().map(|r| r.checks).sum::<i64>(), 2, "{rows:?}");
    assert_eq!(mac_rows.iter().map(|r| r.installs).sum::<i64>(), 1, "{rows:?}");
    assert!(mac_rows.iter().any(|r| r.version == "1.3.0") && mac_rows.iter().any(|r| r.version == "1.4.0"), "counted by version: {rows:?}");
    // The same version again is refused; an older one too.
    let (st, v) = push(&f, "1.4.0", "stable", json!([{ "platform": "darwin-aarch64", "path": mac }, { "platform": "windows-x86_64", "path": win }]));
    assert_eq!((st, code(&v)), (409, "version_exists"));
    let (st, v) = push(&f, "1.3.9", "stable", json!([{ "platform": "darwin-aarch64", "path": mac }, { "platform": "windows-x86_64", "path": win }]));
    assert_eq!((st, code(&v)), (409, "version_not_newer"));
    // Dropping Windows needs --drop-platform (TA-12).
    let mac2 = artifact(&f, "My App 2.app.tar.gz", b"mac 1.5");
    let (st, v) = push(&f, "1.5.0", "stable", json!([{ "platform": "darwin-aarch64", "path": mac2 }]));
    assert_eq!((st, code(&v)), (409, "platform_dropped"), "{v}");
    let (st, v) = call(
        PUSH,
        &agent(&f.ws),
        json!({ "app": f.app_id, "channel": "stable", "version": "1.5.0", "artifacts": [{ "platform": "darwin-aarch64", "path": mac2 }], "dropPlatforms": ["windows-x86_64"] }),
        &[],
    );
    assert_eq!(st, 200, "{v}");
    assert_eq!(wait_state(&f, v["release"].as_str().expect("id")).state, store::STATE_LIVE);
    // Windows on 1.3.0 still gets 1.4.0 (the newest this channel carried for it).
    let (status, body) = serve(&f, "/_k2/updates/stable/windows/x86_64/1.3.0");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(serde_json::from_str::<J>(&body).expect("json")["version"], "1.4.0");
}

#[test]
fn a_push_refuses_bad_files_before_anything_leaves_the_box() {
    let f = fx("bad");
    // No key yet.
    let ok = artifact(&f, "A.app.tar.gz", b"good bytes");
    let (st, v) = push(&f, "1.0.0", "stable", json!([{ "platform": "darwin-aarch64", "path": ok }]));
    assert_eq!((st, code(&v)), (409, "no_update_key"));
    let (st, v) = call(KEY_SET, &agent(&f.ws), json!({ "app": f.app_id, "pubkey": test_key_line() }), &[]);
    assert_eq!(st, 200, "{v}");
    // No token yet.
    let (st, v) = push(&f, "1.0.0", "stable", json!([{ "platform": "darwin-aarch64", "path": ok }]));
    assert_eq!((st, code(&v)), (409, "dl_token_missing"), "{v}");
    let (st, v) = call(TOKEN_SET, &agent(&f.ws), json!({ "app": f.app_id, "token": "k2dlr_00000000000000000000000000000000" }), &[]);
    assert_eq!((st, code(&v)), (400, "dl_token_invalid"));
    ready(&f);
    let (st, v) = call(TOKEN, &agent(&f.ws), json!({}), &[("app", &f.app_id)]);
    assert_eq!(st, 200);
    assert_eq!(v["set"], true);
    assert!(!v.to_string().contains(TOKEN_A), "the token is never answered: {v}");
    // Tampered bytes (signed, then changed).
    let tampered = artifact(&f, "B.app.tar.gz", b"signed bytes");
    std::fs::write(&tampered, b"changed bytes").expect("tamper");
    let (st, v) = push(&f, "1.0.0", "stable", json!([{ "platform": "darwin-aarch64", "path": tampered }]));
    assert_eq!((st, code(&v)), (400, "signature_invalid"), "{v}");
    // Signed with another key.
    let other = f.ws_path.join("C.app.tar.gz");
    std::fs::write(&other, b"x").expect("w");
    let sig = testsign::sign(&[7u8; 32], [1, 2, 3, 4, 5, 6, 7, 8], b"x", "c");
    std::fs::write(format!("{}.sig", other.display()), base64::engine::general_purpose::STANDARD.encode(sig)).expect("w");
    let (st, v) = push(&f, "1.0.0", "stable", json!([{ "platform": "darwin-aarch64", "path": other.to_string_lossy() }]));
    assert_eq!((st, code(&v)), (400, "signature_invalid"));
    assert!(v["error"]["hint"].as_str().expect("hint").contains("another key"), "{v}");
    // Unknown platform, wrong type, bad version, bad channel.
    let (st, v) = push(&f, "1.0.0", "stable", json!([{ "platform": "macos-arm64", "path": ok }]));
    assert_eq!((st, code(&v)), (400, "platform_invalid"));
    let dmg = artifact(&f, "A.dmg", b"d");
    let (st, v) = push(&f, "1.0.0", "stable", json!([{ "platform": "darwin-aarch64", "path": dmg }]));
    assert_eq!((st, code(&v)), (415, "artifact_type_refused"));
    let (st, v) = push(&f, "1.0", "stable", json!([{ "platform": "darwin-aarch64", "path": ok }]));
    assert_eq!((st, code(&v)), (400, "version_invalid"));
    let (st, v) = push(&f, "1.0.0", "Stable", json!([{ "platform": "darwin-aarch64", "path": ok }]));
    assert_eq!((st, code(&v)), (400, "channel_invalid"));
    // A file outside the App's workspace.
    let outside = f._home.path().join(&f.other_ws).join("D.app.tar.gz");
    std::fs::write(&outside, b"o").expect("w");
    let (st, v) = push(&f, "1.0.0", "stable", json!([{ "platform": "darwin-aarch64", "path": outside.to_string_lossy() }]));
    assert_eq!((st, code(&v)), (403, "file_outside_workspace"));
    assert_eq!(f.host.lock().uploads, 0, "nothing left the box");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        assert!(store::releases(&conn, &f.app_id).expect("q").is_empty(), "no release recorded for a refused push");
    }
}

#[test]
fn an_upload_failure_fails_the_release_and_moves_no_channel() {
    let f = fx("fail");
    ready(&f);
    let p = artifact(&f, "A.app.tar.gz", b"bytes");
    push_live(&f, "1.0.0", "stable", json!([{ "platform": "darwin-aarch64", "path": p }]));
    // The health probe passes; every upload attempt then meets a 503.
    f.host.lock().fail_uploads = 64;
    let p2 = artifact(&f, "A2.app.tar.gz", b"bytes two");
    let (st, v) = push(&f, "1.1.0", "stable", json!([{ "platform": "darwin-aarch64", "path": p2 }]));
    assert_eq!(st, 200, "{v}");
    let r = wait_state(&f, v["release"].as_str().expect("id"));
    assert_eq!(r.state, store::STATE_FAILED, "{r:?}");
    assert!(r.error.as_deref().is_some_and(|e| e.contains("unavailable")), "{r:?}");
    let (_, l) = call(LIST, &agent(&f.ws), json!({}), &[("app", &f.app_id)]);
    assert_eq!(l["channels"][0]["version"], "1.0.0", "the channel didn't move: {l}");
    let (status, body) = serve(&f, "/_k2/updates/stable/latest.json");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(body.contains("\"1.0.0\""));
    assert!(!super::staging_root(&f.app_id).join(&r.id).exists(), "staging cleaned");
    // A host that doesn't answer at all: refused before any file is read.
    f.host.lock().fail_uploads = 0;
    f.host.lock().fail_next.extend([503u16; 8]);
    let p3 = artifact(&f, "A3.app.tar.gz", b"bytes three");
    let (st, v) = push(&f, "1.2.0", "stable", json!([{ "platform": "darwin-aarch64", "path": p3 }]));
    assert_eq!((st, code(&v)), (503, "updates_host_unavailable"), "{v}");
    // A retry of the failed version works once the host is back.
    f.host.lock().fail_next.clear();
    push_live(&f, "1.1.0", "stable", json!([{ "platform": "darwin-aarch64", "path": p2 }]));
}

// ── channel operations ───────────────────────────────────────────────

#[test]
fn promote_rollback_and_pull_move_channels_and_say_what_clients_do() {
    let f = fx("ops");
    ready(&f);
    let a = artifact(&f, "A.app.tar.gz", b"one");
    push_live(&f, "1.0.0", "stable", json!([{ "platform": "darwin-aarch64", "path": a }]));
    let b = artifact(&f, "B.app.tar.gz", b"two");
    push_live(&f, "1.1.0", "beta", json!([{ "platform": "darwin-aarch64", "path": b }]));
    let uploads = f.host.lock().uploads;
    let (st, v) = call(PROMOTE, &agent(&f.ws), json!({ "app": f.app_id, "from": "beta", "to": "stable" }), &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["version"], "1.1.0");
    assert_eq!(f.host.lock().uploads, uploads, "a promote never re-uploads");
    let (_, body) = serve(&f, "/_k2/updates/stable/latest.json");
    assert!(body.contains("\"1.1.0\""), "{body}");
    // Promote never goes backwards.
    let (st, v) = call(PROMOTE, &agent(&f.ws), json!({ "app": f.app_id, "from": "beta", "to": "stable", "version": "1.0.0" }), &[]);
    assert_eq!((st, code(&v)), (409, "version_not_newer"));
    // Rollback says plainly that clients on 1.1.0 stay there.
    let (st, v) = call(ROLLBACK, &agent(&f.ws), json!({ "app": f.app_id, "channel": "stable", "to": "1.0.0" }), &[]);
    assert_eq!(st, 200, "{v}");
    assert!(v["warning"].as_str().expect("warning").contains("NEWER"), "{v}");
    let (_, body) = serve(&f, "/_k2/updates/stable/darwin/aarch64/1.1.0");
    assert!(body.is_empty(), "1.1.0 clients get no update (no downgrade)");
    let (status, body) = serve(&f, "/_k2/updates/stable/darwin/aarch64/0.9.0");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(body.contains("\"1.0.0\""));
    // A rollback only goes to a release the channel served.
    let c = artifact(&f, "C.app.tar.gz", b"three");
    push_live(&f, "0.5.0", "nightly", json!([{ "platform": "darwin-aarch64", "path": c }]));
    let (st, v) = call(ROLLBACK, &agent(&f.ws), json!({ "app": f.app_id, "channel": "stable", "to": "0.5.0" }), &[]);
    assert_eq!((st, code(&v)), (409, "release_not_live"), "{v}");
    // Pull 1.1.0: beta falls back to nothing older on beta → empty. This
    // stand-in is a k2dl 1.3.0 host: no pull call, the files stay.
    let (st, v) = call(PULL, &agent(&f.ws), json!({ "app": f.app_id, "version": "1.1.0" }), &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["host"], "not_supported", "{v}");
    assert!(v["note"].as_str().expect("note").contains("can't pull"), "{v}");
    let (status, _) = serve(&f, "/_k2/updates/beta/latest.json");
    assert_eq!(status, "HTTP/1.1 404 Not Found", "beta is empty now");
    // Pull 1.0.0 on a k2dl 1.4.0 host: it stops serving the files too.
    f.host.lock().supports_pull = true;
    let (st, v) = call(PULL, &agent(&f.ws), json!({ "app": f.app_id, "version": "1.0.0" }), &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["host"], "pulled", "{v}");
    assert_eq!(f.host.lock().pulled, vec![(format!("dev.k2.box.{}", f.app_id), "1.0.0".to_string())]);
    let (status, _) = serve(&f, "/_k2/updates/stable/latest.json");
    assert_eq!(status, "HTTP/1.1 404 Not Found");
    // A Member can't change channels; audit has the moves.
    let member = Who::Login { username: "m".into(), role: Role::Member };
    let (st, _) = call(PROMOTE, &member, json!({ "app": f.app_id, "from": "nightly", "to": "stable" }), &[]);
    assert_eq!(st, 403);
    let events: Vec<String> = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let mut st = conn.prepare("SELECT event FROM deploy_audit WHERE subject_id = ?1 ORDER BY rowid").expect("q");
        st.query_map(params![f.app_id], |r| r.get(0)).expect("q").map(|r| r.expect("row")).collect()
    };
    for e in ["updates_key_set", "updates_token_set", "updates_push", "updates_live", "updates_promote", "updates_rollback", "updates_pull"] {
        assert!(events.iter().any(|x| x == e), "{e} audited: {events:?}");
    }
}

// ── stats and the fleet view ─────────────────────────────────────────

#[test]
fn stats_read_the_host_and_say_unavailable_never_zero() {
    let f = fx("stats");
    ready(&f);
    let id = format!("dev.k2.box.{}", f.app_id);
    f.host.lock().stats.insert(
        id.clone(),
        json!({ "app": id, "versions": [{ "version": "1.0.0", "files": [{ "days": [{ "day": "2026-10-29", "downloads": 7, "bytes": 70 }] }] }], "manifest_checks": [] }),
    );
    let (st, v) = call(STATS, &agent(&f.ws), json!({}), &[("app", &f.app_id)]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["downloads"]["available"], true);
    assert_eq!(v["downloads"]["total"], 7);
    // A host that doesn't answer.
    f.host.lock().fail_next.extend([503u16; 64]);
    let (st, v) = call(STATS, &agent(&f.ws), json!({}), &[("app", &f.app_id)]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["downloads"]["available"], false, "{v}");
    assert!(v["downloads"].get("total").is_none(), "unavailable is never a zero: {v}");
    f.host.lock().fail_next.clear();
    // The fleet view: Admins and the owner only, aggregates only.
    let (st, v) = call(FLEET, &agent(&f.ws), json!({}), &[]);
    assert_eq!(st, 403, "{v}");
    let (st, v) = call(FLEET, &Who::Login { username: "a".into(), role: Role::Admin }, json!({}), &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["schema"], 1);
    let app = v["apps"].as_array().expect("apps").iter().find(|a| a["app"] == f.app_id.as_str()).expect("this App");
    assert_eq!(app["dlId"], id);
    assert!(!v.to_string().contains(TOKEN_A));
}

#[test]
fn removing_the_app_forgets_its_feed_and_token() {
    let f = fx("gone");
    ready(&f);
    let p = artifact(&f, "A.app.tar.gz", b"one");
    push_live(&f, "1.0.0", "stable", json!([{ "platform": "darwin-aarch64", "path": p }]));
    let service = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::deploy::store::get_app(&conn, &f.app_id).expect("q").expect("row").service_id
    };
    super::forget_app(&f.app_id, &service);
    assert!(super::super::updates_serve::answer(&f.app_id, "GET", "/_k2/updates/stable/latest.json").is_some_and(|b| String::from_utf8_lossy(&b).starts_with("HTTP/1.1 404")));
    assert!(super::super::local_answers::for_app(&f.app_id).is_none() || crate::app_pairing::links::get(&f.app_id).is_some());
    let h = k2_core::db::shared();
    let conn = h.lock();
    assert!(store::key(&conn, &f.app_id).expect("q").is_none());
    assert!(store::releases(&conn, &f.app_id).expect("q").is_empty());
}

// ── the real download host (run on purpose only) ─────────────────────

/// ONE push against the live download host with K2 Web's test id
/// `dev.k2.rosson.k2test` (box `rosson`, App `k2test`), its token in
/// `K2DL_TOKEN` (a compute job's `--secret`, never argv). Ignored in normal
/// runs; run with `--run-ignored only -E 'test(real_download_host)'`. It
/// fails loudly without the token. Prints what the real host answered.
#[test]
#[ignore = "talks to the live download host; needs K2DL_TOKEN (K2 Web's test id)"]
fn real_download_host_push_download_manifest_pull() {
    use sha2::Digest as _;
    let token = std::env::var("K2DL_TOKEN").expect("K2DL_TOKEN (k2 compute run --secret K2DL_TOKEN_K2TEST=K2DL_TOKEN)");
    let token = token.trim().to_string();
    assert!(k2_core::app_updates::valid_dl_token(&token), "K2DL_TOKEN isn't a k2dl_ token");
    let base = std::env::var("K2_REAL_DL_BASE").unwrap_or_else(|_| "https://dl.k2.dev".into());
    // An idle stand-in (never called): the fixture wants one.
    let idle = FakeHost::start(&[]);
    let f = fx_on("real", "rosson", "k2test".into(), idle, &base);
    let id = "dev.k2.rosson.k2test";
    let (st, v) = call(KEY_SET, &agent(&f.ws), json!({ "app": f.app_id, "pubkey": test_key_line() }), &[]);
    assert_eq!(st, 200, "{v}");
    let (st, v) = call(TOKEN_SET, &agent(&f.ws), json!({ "app": f.app_id, "token": token }), &[]);
    assert_eq!(st, 200, "{v}");
    assert!(!v.to_string().contains(&token));
    assert_eq!(v["dlId"], id);
    // A version newer than any earlier run's: 0.1.<unix seconds>.
    let now = k2_core::deploy::now();
    let version = format!("0.1.{now}");
    let mut bytes = format!("K2 real-host test archive {version}\n").into_bytes();
    bytes.extend((0..4096u32).map(|i| (i % 251) as u8));
    let want_sha: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    let p = artifact(&f, "K2Test.app.tar.gz", &bytes);
    let started = Instant::now();
    let (st, v) = push(&f, &version, "stable", json!([{ "platform": "darwin-aarch64", "path": p }]));
    assert_eq!(st, 200, "push: {v}");
    let r = wait_state(&f, v["release"].as_str().expect("release"));
    println!("[real] push {version}: {} in {:?}", r.state, started.elapsed());
    assert_eq!(r.state, store::STATE_LIVE, "{r:?}");
    let arts = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::artifacts(&conn, &r.id).expect("artifacts")
    };
    let url = arts[0].url.clone().expect("url");
    println!("[real] immutable URL: {url}");
    assert!(url.starts_with(&format!("{base}/{id}/{version}/{want_sha}/")), "{url}");
    // The immutable URL serves the same bytes, with the host's headers.
    let http = reqwest::blocking::Client::new();
    let resp = http.get(&url).send().expect("download");
    let status = resp.status().as_u16();
    let headers: Vec<String> = ["content-type", "cache-control", "etag", "accept-ranges", "x-content-sha256", "content-length"]
        .iter()
        .map(|h| format!("{h}: {}", resp.headers().get(*h).and_then(|v| v.to_str().ok()).unwrap_or("-")))
        .collect();
    let got = resp.bytes().expect("body");
    let got_sha: String = sha2::Sha256::digest(&got).iter().map(|b| format!("{b:02x}")).collect();
    println!("[real] GET immutable: {status}; {}", headers.join("; "));
    assert_eq!(status, 200);
    assert_eq!(got_sha, want_sha, "the download host served other bytes");
    // A ranged request from 0 (Tauri doesn't need it; the host promises it).
    let ranged = http.get(&url).header("Range", "bytes=0-15").send().expect("range");
    println!("[real] GET Range 0-15: {}", ranged.status().as_u16());
    // The host's latest.json (the fallback K2 published) points at it.
    let latest_url = format!("{base}/{id}/stable/latest.json");
    let lr = http.get(&latest_url).send().expect("latest.json");
    let lst = lr.status().as_u16();
    let cc = lr.headers().get("cache-control").and_then(|v| v.to_str().ok()).unwrap_or("-").to_string();
    let latest: J = lr.json().expect("latest.json is JSON");
    println!("[real] GET {latest_url}: {lst}; cache-control: {cc}; body: {latest}");
    assert_eq!(lst, 200);
    assert_eq!(latest["version"], version.as_str());
    assert_eq!(latest["platforms"]["darwin-aarch64"]["url"], url.as_str());
    // K2's own answers on the App's host point at the same URL.
    let (status, body) = serve(&f, "/_k2/updates/stable/darwin/aarch64/0.0.1");
    assert_eq!(status, "HTTP/1.1 200 OK");
    let d: J = serde_json::from_str(&body).expect("json");
    assert_eq!(d["url"], url.as_str());
    // The same bytes again: the host says it already has them (idempotent).
    let c = k2_core::app_updates::dl_client::Client::new(&base, &token).expect("client");
    let staged = f.ws_path.join("K2Test.app.tar.gz");
    let again = c.upload(id, &version, &arts[0].file_name, &staged, &want_sha, bytes.len() as u64, "application/gzip").expect("idempotent re-upload");
    println!("[real] re-upload same bytes: created={} url-same={}", again.created, again.url == url);
    assert!(!again.created);
    // Stats answer for this id.
    match c.stats(id, &k2_core::app_updates::day_of(now - 86_400), &k2_core::app_updates::day_of(now)) {
        Ok(s) => println!("[real] stats: {}", s.to_string().chars().take(400).collect::<String>()),
        Err(e) => println!("[real] stats: {}", e.line()),
    }
    // Pull: K2 stops offering it; the host stops serving it if it can (1.4.0).
    let (st, v) = call(PULL, &agent(&f.ws), json!({ "app": f.app_id, "version": version }), &[]);
    assert_eq!(st, 200, "pull: {v}");
    println!("[real] pull: host={} channels={}", v["host"], v["channels"]);
    let after = http.get(&url).send().expect("download after pull").status().as_u16();
    let latest_after = http.get(&latest_url).send().map(|r| r.status().as_u16()).unwrap_or(0);
    println!("[real] after pull: immutable URL {after}; latest.json {latest_after}");
    let (status, _) = serve(&f, "/_k2/updates/stable/latest.json");
    assert_eq!(status, "HTTP/1.1 404 Not Found", "K2's own answers stop offering it");
    if v["host"] == "pulled" {
        assert_ne!(after, 200, "a host that says pulled still serves the file");
    }
}
