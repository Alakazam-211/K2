//! The remote Garden door (prd-gardens-on-phone-v1 §9): who gets in (every
//! route × every caller kind), the feed, the page, the frame document, the
//! call runner's refusals, blobs, reports, pause and Resume, agents order,
//! that no secret reaches the phone, and the golden samples the Companion
//! builds against (`crates/k2-daemon/tests/golden/gardens_remote/`).
//! Temp HOME, a scoped DB, the widget door's own fixture (`door`): a Zen
//! folder whose first Garden places `board` as `b1`. Fail loudly.
//!
//! **Updating the samples:** `UPDATE_GARDENS_REMOTE_SAMPLES=1 cargo test -p
//! k2-daemon --lib zen_remote_routes` rewrites them; without it the test
//! fails on any difference (the drift test).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde_json::{json, Value as J};

use k2_core::connect_users::{self as cu, Role};
use k2_core::garden_data::MadeBy;
use k2_core::zen::ZenFile;

use super::*;
use crate::garden_data::call::door_tests::{door, set_caps, Door};

const OWNER_TOKEN: &str = "owner-token-for-the-remote-garden-tests";
const PASSWORD: &str = "password-for-remote-gardens-9";

// ── fixtures ────────────────────────────────────────────────────────

fn login(name: &str, role: Role) -> String {
    cu::add_user(name, PASSWORD).expect("add_user");
    if role != Role::Member {
        cu::set_role(name, role).expect("set_role");
    }
    name.to_string()
}

fn link(users: &[&str]) {
    k2_core::app_settings::update(json!({ "ownerAccounts": users })).expect("link to the owner");
}

/// A device session for `user` (what `POST /cli/auth/device/pair` mints).
fn pair(user: &str, device: &str) -> String {
    let epoch = cu::live_user_epoch(user).expect("a live login");
    cu::create_device_session(user, epoch, device, "Test phone", "owner-token").expect("device session")
}

fn passport() -> String {
    crate::session_token::mint_session_token(
        &k2_core::session::SessionId::new(),
        "pane-remote-gardens",
        crate::session_token::HookPrincipal { workspace_uuid: "w-builder".into(), agent_address: "board-builder".into() },
        crate::session_token::CredMode::ApiKey,
        crate::session_token::Provider::Anthropic,
    )
}

/// Everyone the tests need, on top of the door's Zen folder.
struct Fx {
    d: Door,
    /// The owner's phone: a device session of their linked Admin login
    /// (pairing picks the linked Admin when no Owner login is linked).
    phone: String,
    /// The same linked login, signed in with its password (the web).
    password: String,
    /// Another person's unlinked Owner-role login, on a paired phone.
    other_owner_phone: String,
    admin_phone: String,
    member_phone: String,
    passport: String,
}

fn fx(caps: &[&str]) -> Fx {
    let d = door(caps);
    d.stamp(MadeBy::Owner);
    login("rosphone", Role::Admin);
    login("otherowner", Role::Owner);
    login("adminx", Role::Admin);
    login("memberx", Role::Member);
    link(&["rosphone"]);
    Fx {
        phone: pair("rosphone", "ios-rosson-1"),
        password: cu::create_session("rosphone"),
        other_owner_phone: pair("otherowner", "ios-other-1"),
        admin_phone: pair("adminx", "android-admin-1"),
        member_phone: pair("memberx", "android-member-1"),
        passport: passport(),
        d,
    }
}

fn status_code(r: &CliResponse) -> u16 {
    r.status.split(' ').next().and_then(|c| c.parse().ok()).unwrap_or_else(|| panic!("a status: {}", r.status))
}

fn req(path: &str, token: &str, params: &[(&str, &str)], body: J) -> (u16, J) {
    let who = who_of(token, OWNER_TOKEN);
    let params: HashMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let bytes = if body.is_null() { Vec::new() } else { body.to_string().into_bytes() };
    let r = handle(path, &who, &params, &bytes);
    if r.status.starts_with('2') && r.content_type.starts_with("text/javascript") {
        // A library file: its text, not JSON.
        return (status_code(&r), json!({ "text": r.body }));
    }
    (status_code(&r), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: {e}: {}", r.body)))
}

fn ok_of(path: &str, token: &str, params: &[(&str, &str)], body: J) -> J {
    let (s, v) = req(path, token, params, body);
    assert_eq!(s, 200, "{path}: {v}");
    v
}

fn refused(path: &str, token: &str, params: &[(&str, &str)], body: J) -> (u16, String) {
    let (s, v) = req(path, token, params, body);
    assert_ne!(s, 200, "{path} should refuse: {v}");
    assert_eq!(v["ok"], false, "the door's one envelope: {v}");
    assert!(v["error"]["hint"].is_string(), "a hint: {v}");
    (s, v["error"]["code"].as_str().unwrap_or_else(|| panic!("a code: {v}")).to_string())
}

fn call_body(fx: &Fx, verb: &str, args: J) -> J {
    json!({ "garden": fx.d.garden, "placement": "b1", "hash": fx.d.hash(), "verb": verb, "args": args })
}

/// A request that reaches each route's handler (the gate is what's tested).
fn any_request(fx: &Fx, path: &str) -> (Vec<(&'static str, String)>, J) {
    let g = fx.d.garden.clone();
    match path {
        GARDENS => (vec![], J::Null),
        PAGE | AGENTS_ORDER => (vec![("garden", g)], J::Null),
        FRAME => (vec![("garden", g), ("placement", "b1".into())], J::Null),
        LIB => (vec![("id", "uplot".into()), ("version", "1.6.32".into()), ("file", "uPlot.iife.min.js".into())], J::Null),
        LIB_FETCH => (vec![], json!({ "id": "uplot", "version": "1.6.32" })),
        CALL => (vec![], call_body(fx, "viewer.get", json!([]))),
        BLOB_GET => (vec![("garden", g), ("placement", "b1".into()), ("hash", fx.d.hash()), ("ref", "blob:sha256-00".into())], J::Null),
        BLOB_PUT => (vec![], json!({ "garden": g, "placement": "b1", "hash": fx.d.hash(), "offset": 0, "base64": "aGk=", "last": true })),
        REPORT => (vec![], json!({ "garden": g, "placement": "b1", "hash": fx.d.hash(), "event": "ready" })),
        PAUSE | RESUME => (vec![], json!({ "garden": g, "placement": "b1" })),
        other => panic!("no request for {other}"),
    }
}

fn all_routes() -> Vec<&'static str> {
    GET_ROUTES.iter().chain(POST_ROUTES.iter()).copied().collect()
}

// ── who ─────────────────────────────────────────────────────────────

#[test]
fn the_door_tells_every_credential_apart() {
    let fx = fx(&["store:read", "store:write"]);
    assert_eq!(who_of(OWNER_TOKEN, OWNER_TOKEN), RemoteWho::Owner);
    assert_eq!(who_of(&fx.phone, OWNER_TOKEN), RemoteWho::Phone { login: "rosphone".into(), device: "ios-rosson-1".into() });
    assert_eq!(who_of(&fx.password, OWNER_TOKEN), RemoteWho::Login { login: "rosphone".into() });
    assert_eq!(who_of("k2skn_notreal", OWNER_TOKEN), RemoteWho::Other { kind: "app-pass" });
    assert_eq!(who_of("k2sk_notreal", OWNER_TOKEN), RemoteWho::Other { kind: "api-key" });
    assert_eq!(who_of(&fx.passport, OWNER_TOKEN), RemoteWho::Other { kind: "passport" });
    assert_eq!(who_of("garbage", OWNER_TOKEN), RemoteWho::Invalid);
    assert_eq!(who_of("", OWNER_TOKEN), RemoteWho::Invalid);
    // Query `token=` first, then Bearer, as every route reads it.
    assert_eq!(who_from_tcp(&format!("token={OWNER_TOKEN}"), Some(&fx.phone), OWNER_TOKEN), RemoteWho::Owner);
    assert!(matches!(who_from_tcp("", Some(&fx.phone), OWNER_TOKEN), RemoteWho::Phone { .. }));
}

/// §9 "Routes": every route × every caller kind → the exact refusal; the
/// owner person's phone and the owner token get past the gate.
#[test]
fn only_the_owner_persons_paired_phone_gets_in_on_every_route() {
    let fx = fx(&["store:read", "store:write"]);
    let refusals: Vec<(&str, &str, &str)> = vec![
        ("the owner person's password login", fx.password.as_str(), "garden_device_only"),
        ("another person's unlinked Owner phone", fx.other_owner_phone.as_str(), "garden_person_only"),
        ("an unlinked Admin phone", fx.admin_phone.as_str(), "garden_person_only"),
        ("a Member phone", fx.member_phone.as_str(), "garden_person_only"),
        ("an agent passport", fx.passport.as_str(), "garden_person_only"),
        ("an App pass", "k2skn_notreal", "garden_person_only"),
        ("an API key", "k2sk_notreal", "garden_person_only"),
        ("no credential", "", "garden_person_only"),
    ];
    for path in all_routes() {
        let (params, body) = any_request(&fx, path);
        let params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        for (who, token, code) in &refusals {
            let (s, c) = refused(path, token, &params, body.clone());
            assert_eq!((s, c.as_str()), (403, *code), "{path} as {who}");
        }
        for (who, token) in [("the owner's phone", fx.phone.as_str()), ("the owner token", OWNER_TOKEN)] {
            let (_, v) = req(path, token, &params, body.clone());
            let code = v["error"]["code"].as_str().unwrap_or("");
            assert!(!code.starts_with("garden_"), "{path} as {who} is past the gate: {v}");
        }
    }
    // Every refusal is audited, without args.
    let audit = std::fs::read_to_string(k2_core::auth_audit::path()).expect("audit log");
    assert!(audit.contains("zen.remote.refused"), "{audit}");
    assert!(audit.contains("client=phone device=ios-rosson-1"), "{audit}");
}

#[test]
fn unlinking_the_login_or_ending_the_device_session_shuts_the_door() {
    let fx = fx(&["store:read", "store:write"]);
    ok_of(GARDENS, &fx.phone, &[], J::Null);
    link(&[]);
    assert_eq!(refused(GARDENS, &fx.phone, &[], J::Null), (403, "garden_person_only".into()), "unlinked: not the owner person now");
    link(&["rosphone"]);
    ok_of(GARDENS, &fx.phone, &[], J::Null);
    let session = cu::device_sessions(Some("rosphone")).expect("devices").into_iter().find(|s| s.device_id == "ios-rosson-1").expect("the phone");
    cu::revoke_device_session(&session.id, None).expect("revoke");
    // The dispatcher answers 403 device_session_ended first (routes/http.rs);
    // the door itself never takes an ended session.
    assert_eq!(who_of(&fx.phone, OWNER_TOKEN), RemoteWho::Invalid);
    assert_eq!(refused(GARDENS, &fx.phone, &[], J::Null), (403, "garden_person_only".into()));
}

#[test]
fn the_routes_are_post_guarded_and_have_policy_rows() {
    for p in POST_ROUTES {
        assert!(is_post_route(p) && is_route(p), "{p}");
        assert!(crate::zen_routes::POST_ROUTES.iter().all(|z| z != p), "{p} isn't a generic Zen row");
    }
    for p in GET_ROUTES {
        assert!(!is_post_route(p) && is_route(p), "{p}");
    }
    assert!(!is_route("/cli/zen/call") && !is_route("/cli/zen/get"), "the remote door never answers the local doors' paths");
    assert_eq!(max_body(CALL), 64 * 1024);
    assert!(max_body(BLOB_PUT) > 8 * 1024 * 1024, "a whole 8 MiB chunk in base64");
}

// ── the feed ────────────────────────────────────────────────────────

#[test]
fn the_feed_is_the_whole_list_or_nothing() {
    let fx = fx(&["store:read", "store:write"]);
    let v = ok_of(GARDENS, &fx.phone, &[], J::Null);
    assert_eq!((v["ok"].as_bool(), v["person"].as_str(), v["setUp"].as_bool(), v["full"].as_bool()), (Some(true), Some("owner"), Some(true), Some(true)), "{v}");
    let list = v["gardens"].as_array().expect("gardens");
    assert_eq!(list.len(), fx.d.f.gardens().len());
    for (i, g) in list.iter().enumerate() {
        let keys: Vec<&str> = g.as_object().expect("a row").keys().map(String::as_str).collect();
        assert_eq!(keys, ["id", "index", "locked", "name", "template", "theme", "version"], "{g}");
        assert_eq!(g["index"], json!(i + 1));
        assert!(g["version"].as_str().is_some_and(|s| !s.is_empty()), "{g}");
    }
    let cursor = v["cursor"].as_str().expect("cursor").to_string();
    // Same state: same cursor, nothing to send.
    let again = ok_of(GARDENS, &fx.phone, &[("since", &cursor)], J::Null);
    assert_eq!((again["full"].as_bool(), again["cursor"].as_str()), (Some(false), Some(cursor.as_str())), "{again}");
    assert_eq!(again["gardens"], json!([]));
    // A stale or unknown cursor: the whole list.
    assert_eq!(ok_of(GARDENS, &fx.phone, &[("since", "g-nope")], J::Null)["full"], true);
    // A rename moves the cursor.
    fx.d.f.rename_garden(&fx.d.garden, "Board room").expect("rename");
    let v2 = ok_of(GARDENS, &fx.phone, &[("since", &cursor)], J::Null);
    assert_eq!(v2["full"], true, "{v2}");
    assert_ne!(v2["cursor"], json!(cursor));
    assert_eq!(v2["gardens"][0]["name"], "Board room");
    // A page edit moves that Garden's version (and the cursor).
    let before = v2["gardens"][0]["version"].clone();
    let page = std::fs::read_to_string(fx.d.f.path_of(&ZenFile::Garden(fx.d.garden.clone()))).expect("page");
    std::fs::write(fx.d.f.path_of(&ZenFile::Garden(fx.d.garden.clone())), page.replace("min-width = 200", "min-width = 240")).expect("edit");
    let v3 = ok_of(GARDENS, &fx.phone, &[("since", v2["cursor"].as_str().expect("cursor"))], J::Null);
    assert_eq!(v3["full"], true, "{v3}");
    assert_ne!(v3["gardens"][0]["version"], before, "{v3}");
}

#[test]
fn a_server_without_gardens_says_so() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let v = ok_of(GARDENS, OWNER_TOKEN, &[], J::Null);
    assert_eq!((v["setUp"].as_bool(), v["full"].as_bool(), v["gardens"].clone()), (Some(false), Some(true), json!([])), "{v}");
    assert_eq!(refused(PAGE, OWNER_TOKEN, &[("garden", "g-00000000")], J::Null), (404, "zen_not_set_up".into()));
}

// ── the page ────────────────────────────────────────────────────────

#[test]
fn the_page_is_zen_get_without_sources_plus_how_the_phone_runs_each_placement() {
    let fx = fx(&["store:read", "store:write"]);
    let v = ok_of(PAGE, &fx.phone, &[("garden", &fx.d.garden)], J::Null);
    assert!(v.get("sources").is_none(), "no file names on the phone: {v}");
    assert_eq!(v["remote"]["widgets"], json!([{ "id": "b1", "run": "frame" }]), "{v}");
    assert_eq!(v["remote"]["features"], json!(phone_features()));
    // The rest is GET /cli/zen/get's answer, unchanged.
    let params: HashMap<String, String> = [("garden".to_string(), fx.d.garden.clone())].into();
    let r = crate::zen_routes::handle("/cli/zen/get", true, &crate::sidecar_routes::Caller::Owner, "test", &params, b"");
    let mut desk: J = serde_json::from_str(&r.body).expect("get");
    desk.as_object_mut().expect("object").remove("sources");
    let mut mine = v.clone();
    mine.as_object_mut().expect("object").remove("remote");
    assert_eq!(mine, desk);
    // Every kind maps: built-ins the phone draws itself, the rest are placeholders.
    for g in fx.d.f.gardens() {
        let v = ok_of(PAGE, &fx.phone, &[("garden", &g.id)], J::Null);
        let widgets = v["page"]["widgets"].as_array().expect("widgets");
        let runs = v["remote"]["widgets"].as_array().expect("runs");
        assert_eq!(widgets.len(), runs.len());
        for (w, r) in widgets.iter().zip(runs) {
            assert_eq!(w["id"], r["id"]);
            let want = match w["kind"].as_str().unwrap_or_default() {
                "custom" => "frame",
                "agents" | "conversation" | "garden-empty" => "native",
                _ => "placeholder",
            };
            assert_eq!(r["run"], want, "{w}");
            assert_eq!(r.get("reason").is_some(), want == "placeholder", "{r}");
        }
    }
    assert_eq!(refused(PAGE, &fx.phone, &[("garden", "g-ffffffff")], J::Null), (404, "unknown_garden".into()));
    assert_eq!(refused(PAGE, &fx.phone, &[], J::Null), (400, "usage".into()));
    assert_eq!(refused(PAGE, &fx.phone, &[("garden", &fx.d.garden), ("scheme", "sepia")], J::Null), (400, "usage".into()));
}

// ── the frame ───────────────────────────────────────────────────────

fn manifest_with_libs(caps: &[&str], libs: &[&str]) -> String {
    let caps: Vec<String> = caps.iter().map(|c| format!("\"{c}\"")).collect();
    let libs: Vec<String> = libs.iter().map(|c| format!("\"{c}\"")).collect();
    format!(
        "{{\n  \"schema\": 1,\n  \"name\": \"Board\",\n  \"entry\": \"index.html\",\n  \"caps\": [{}],\n  \"requires\": {{\"libs\": [{}]}}\n}}\n",
        caps.join(", "),
        libs.join(", ")
    )
}

#[test]
fn the_frame_is_the_desktop_wrapper_with_library_slots_and_the_hello() {
    let fx = fx(&["store:read", "store:write"]);
    let v = ok_of(FRAME, &fx.phone, &[("garden", &fx.d.garden), ("placement", "b1"), ("scheme", "dark")], J::Null);
    let nonce = v["nonce"].as_str().expect("nonce");
    let doc = v["srcdoc"].as_str().expect("srcdoc");
    assert_eq!(v["hash"], json!(fx.d.hash()));
    assert_eq!((v["garden"].as_str(), v["placement"].as_str(), v["widget"].as_str()), (Some(fx.d.garden.as_str()), Some("b1"), Some("board")));
    assert!(doc.contains(&format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">", k2_core::zen::widget_shot::widget_csp(nonce))), "the CSP first");
    assert!(doc.contains(&k2_core::zen::widget_shot::script_text(k2_core::zen::widget_shot::SEALED_FRAME_GUARD)), "the guard");
    assert!(doc.contains(&k2_core::zen::widget_shot::script_text(k2_core::zen::widget_shot::K2_FRAME_JS)), "the runtime");
    assert!(doc.contains("color-scheme:dark") || doc.contains("color-scheme:light"), "the Garden's scheme");
    assert!(!doc.contains("allow-same-origin"));
    assert_eq!(v["libs"], json!([]), "hello needs no libraries");
    let page = ok_of(PAGE, &fx.phone, &[("garden", &fx.d.garden)], J::Null);
    let w = &page["page"]["widgets"][0];
    assert_eq!(v["hello"]["caps"], w["caps"], "the placement's effective caps");
    assert_eq!(v["hello"]["features"], json!(phone_features()));
    assert_eq!(v["hello"]["widget"], json!({ "id": "b1", "name": w["name"], "garden": fx.d.garden }));
    assert_eq!((v["hello"]["k2"].as_str(), v["hello"]["v"].as_u64()), (Some("hello"), Some(1)));
    // Libraries: one slot each, bundled from the phone's copy, download
    // from the daemon's checked cache.
    std::fs::write(fx.d.f.widget_dir("board").join("manifest.json"), manifest_with_libs(&["store:read"], &["uplot@1", "plotly.js-dist-min@4"])).expect("manifest");
    let v = ok_of(FRAME, &fx.phone, &[("garden", &fx.d.garden), ("placement", "b1")], J::Null);
    let libs = v["libs"].as_array().expect("libs");
    assert_eq!(libs.len(), 2, "{v}");
    assert_eq!((libs[0]["slot"].as_str(), libs[0]["source"].as_str(), libs[0]["id"].as_str()), (Some("__K2_LIB_0__"), Some("bundled"), Some("uplot")));
    assert_eq!((libs[1]["slot"].as_str(), libs[1]["source"].as_str()), (Some("__K2_LIB_1__"), Some("download")));
    for l in libs {
        let slot = l["slot"].as_str().expect("slot");
        assert_eq!(v["srcdoc"].as_str().expect("srcdoc").matches(slot).count(), 1, "{slot}");
        assert!(l["files"].as_array().is_some_and(|f| !f.is_empty() && f.iter().all(|f| f["sha256"].as_str().is_some_and(|s| s.len() == 64))), "{l}");
    }
    // Q5: the daemon serves bundled libraries too (the phone carries none),
    // each file exactly its pin.
    for f in libs[0]["files"].as_array().expect("files") {
        let name = f["name"].as_str().expect("name");
        let v = ok_of(LIB, &fx.phone, &[("id", "uplot"), ("version", libs[0]["version"].as_str().expect("v")), ("file", name)], J::Null);
        let text = v["text"].as_str().unwrap_or_else(|| panic!("{name}: {v}"));
        use sha2::{Digest, Sha256};
        let sum: String = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!((text.len() as u64, sum.as_str()), (f["bytes"].as_u64().expect("bytes"), f["sha256"].as_str().expect("sha")), "{name}");
    }
    let fetched = ok_of(LIB_FETCH, &fx.phone, &[], json!({ "id": "uplot", "version": libs[0]["version"] }));
    assert_eq!((fetched["cached"].as_bool(), fetched["files"].clone()), (Some(true), libs[0]["files"].clone()), "{fetched}");
    assert_eq!(refused(LIB, &fx.phone, &[("id", "uplot"), ("version", libs[0]["version"].as_str().expect("v")), ("file", "nope.js")], J::Null), (404, "not_found".into()));
    // Every bundled library of K2's manifest is servable, bytes = pin.
    for e in k2_core::zen::stdlib::zen_lib_manifest().libs.iter().filter(|e| e.source == k2_core::zen::stdlib::LibSource::Bundled) {
        for f in &e.files {
            let (s, v) = req(LIB, &fx.phone, &[("id", &e.id), ("version", &e.version), ("file", &f.name)], J::Null);
            assert_eq!(s, 200, "{}/{}: {v}", e.key(), f.name);
            assert_eq!(v["text"].as_str().map(str::len), Some(f.bytes as usize), "{}/{}", e.key(), f.name);
        }
    }
    let file = libs[1]["files"][0]["name"].as_str().expect("file");
    let (s, c) = refused(LIB, &fx.phone, &[("id", "plotly.js-dist-min"), ("version", libs[1]["version"].as_str().expect("v")), ("file", file)], J::Null);
    assert_eq!((s, c.as_str()), (404, "not_cached"), "the phone fetches once, then reads");
    // Not a frame, unknown placement.
    set_caps(&fx.d.f, &["store:read", "store:write"]);
    // A built-in placement from a template Garden (Agents, Conversation, …).
    let (gid, pid) = fx
        .d
        .f
        .gardens()
        .into_iter()
        .find_map(|g| {
            let v = ok_of(PAGE, &fx.phone, &[("garden", &g.id)], J::Null);
            let w = v["page"]["widgets"].as_array()?.iter().find(|w| w["kind"] != "custom")?.clone();
            Some((g.id, w["id"].as_str()?.to_string()))
        })
        .expect("a template Garden with a built-in widget");
    assert_eq!(refused(FRAME, &fx.phone, &[("garden", &gid), ("placement", &pid)], J::Null), (409, "not_a_frame".into()));
    assert_eq!(refused(FRAME, &fx.phone, &[("garden", &fx.d.garden), ("placement", "zz")], J::Null), (404, "unknown_placement".into()));
}

// ── call ────────────────────────────────────────────────────────────

#[test]
fn call_runs_door_rows_with_the_daemons_checks_and_catalog_codes() {
    let fx = fx(&["store:read", "store:write"]);
    let put = ok_of(CALL, &fx.phone, &[], call_body(&fx, "store.put", json!(["obj/1", { "t": "hi" }])));
    assert_eq!(put["ok"], true, "{put}");
    let got = ok_of(CALL, &fx.phone, &[], call_body(&fx, "store.get", json!(["obj/1"])));
    assert_eq!(got["result"]["value"], json!({ "t": "hi" }), "{got}");
    let viewer = ok_of(CALL, &fx.phone, &[], call_body(&fx, "viewer.get", json!([])));
    assert_eq!(viewer["ok"], true);
    // Stale hash.
    let mut stale = call_body(&fx, "store.get", json!(["obj/1"]));
    stale["hash"] = json!("0".repeat(64));
    assert_eq!(refused(CALL, &fx.phone, &[], stale), (409, "stale_widget".into()));
    // The phone column.
    let (s, v) = req(CALL, &fx.phone, &[], call_body(&fx, "agents.list", json!([])));
    assert_eq!((s, v["error"]["code"].as_str(), v["error"]["feature"].as_str()), (501, Some("verb_unavailable"), Some("zen-v1")), "{v}");
    assert_eq!(refused(CALL, &fx.phone, &[], call_body(&fx, "guests.setRooms", json!([]))).1, "verb_unavailable");
    assert_eq!(refused(CALL, &fx.phone, &[], call_body(&fx, "theme.get", json!([]))).1, "not_exposed");
    assert_eq!(refused(CALL, &fx.phone, &[], call_body(&fx, "window.startDrag", json!([]))).1, "not_exposed");
    assert_eq!(refused(CALL, &fx.phone, &[], call_body(&fx, "homes.list", json!([]))).1, "not_exposed");
    assert_eq!(refused(CALL, &fx.phone, &[], call_body(&fx, "no.such", json!([]))).1, "unknown_verb");
    assert_eq!(refused(CALL, &fx.phone, &[], call_body(&fx, "blobs.put", json!([]))).1, "bad_args");
    // The phone never sends a confirm or an origin.
    let mut smuggle = call_body(&fx, "store.get", json!(["obj/1"]));
    smuggle["confirmToken"] = json!("x");
    assert_eq!(refused(CALL, &fx.phone, &[], smuggle).1, "bad_args");
    // A refusal outside the catalog's list keeps its cause.
    let mut gone = call_body(&fx, "store.get", json!(["obj/1"]));
    gone["placement"] = json!("nope");
    let (s, v) = req(CALL, &fx.phone, &[], gone);
    assert_eq!((s, v["error"]["code"].as_str(), v["error"]["cause"].as_str()), (404, Some("not_found"), Some("widget_not_found")), "{v}");
    // Caps: the manifest's, as on desktop.
    set_caps(&fx.d.f, &["store:read"]);
    fx.d.stamp(MadeBy::Owner);
    assert_eq!(refused(CALL, &fx.phone, &[], call_body(&fx, "store.put", json!(["obj/2", 1]))).1, "cap_not_granted");
    // Every code the call answers is in the catalog's closed list.
    let codes: Vec<String> = k2_core::contract::catalog().errors.iter().map(|e| e.code.clone()).collect();
    for c in ["stale_widget", "verb_unavailable", "not_exposed", "unknown_verb", "bad_args", "not_found", "cap_not_granted"] {
        assert!(codes.iter().any(|x| x == c), "{c} is a catalog code");
    }
}

#[test]
fn blobs_move_over_the_remote_blob_routes() {
    let fx = fx(&["store:read", "store:write"]);
    let put = ok_of(
        BLOB_PUT,
        &fx.phone,
        &[],
        json!({ "garden": fx.d.garden, "placement": "b1", "hash": fx.d.hash(), "offset": 0, "base64": "aGVsbG8gcGhvbmU=", "last": true, "mime": "text/plain", "name": "hi.txt" }),
    );
    let r = put["ref"].as_str().unwrap_or_else(|| panic!("a ref: {put}")).to_string();
    let got = ok_of(BLOB_GET, &fx.phone, &[("garden", &fx.d.garden), ("placement", "b1"), ("hash", &fx.d.hash()), ("ref", &r)], J::Null);
    assert_eq!((got["base64"].as_str(), got["eof"].as_bool()), (Some("aGVsbG8gcGhvbmU="), Some(true)), "{got}");
    let mut stale = fx.d.hash();
    stale.replace_range(0..1, if stale.starts_with('0') { "1" } else { "0" });
    let (s, c) = refused(BLOB_GET, &fx.phone, &[("garden", &fx.d.garden), ("placement", "b1"), ("hash", &stale), ("ref", &r)], J::Null);
    assert_eq!((s, c.as_str()), (409, "stale_widget"));
}

// ── report, pause, resume, agents order ─────────────────────────────

#[test]
fn reports_pauses_and_the_persons_resume() {
    let fx = fx(&["store:read", "store:write"]);
    let body = |event: &str| json!({ "garden": fx.d.garden, "placement": "b1", "hash": fx.d.hash(), "event": event });
    ok_of(REPORT, &fx.phone, &[], body("ready"));
    let mut err = body("error");
    err["message"] = json!("boom on the phone");
    ok_of(REPORT, &fx.phone, &[], err);
    let mut refusal = body("refusal");
    refusal["verb"] = json!("store.put");
    refusal["code"] = json!("cap_not_granted");
    ok_of(REPORT, &fx.phone, &[], refusal);
    assert_eq!(refused(REPORT, &fx.phone, &[], body("weird")).1, "usage");
    let runs = fx.d.f.widget_errors("board", None).expect("runs");
    assert!(runs.to_string().contains("boom on the phone"), "{runs}");
    // Pause, shown on the page; Resume from the owner's phone.
    let place = json!({ "garden": fx.d.garden, "placement": "b1" });
    let mut p = place.clone();
    p["reason"] = json!("runaway");
    ok_of(PAUSE, &fx.phone, &[], p);
    let page = ok_of(PAGE, &fx.phone, &[("garden", &fx.d.garden)], J::Null);
    assert_eq!(page["page"]["widgets"][0]["paused"]["reason"], "runaway", "{page}");
    let v = ok_of(RESUME, &fx.phone, &[], place.clone());
    assert_eq!(v["resumed"], true, "{v}");
    assert_eq!(ok_of(RESUME, &fx.phone, &[], place)["resumed"], false, "a second Resume is a no-op");
    let audit = std::fs::read_to_string(k2_core::auth_audit::path()).expect("audit");
    assert!(audit.contains("zen.remote.resume") && audit.contains("zen.widget.resume"), "{audit}");
}

#[test]
fn agents_order_is_read_only_on_the_phone() {
    let fx = fx(&["store:read", "store:write"]);
    let v = ok_of(AGENTS_ORDER, &fx.phone, &[("garden", &fx.d.garden)], J::Null);
    assert_eq!(v["ok"], true, "{v}");
    assert!(!is_route("/cli/zen/remote/agents-order/set"), "no setter on the phone");
}

// ── no secrets ──────────────────────────────────────────────────────

fn banned_keys(v: &J, at: &str, banned: &[String], out: &mut Vec<String>) {
    match v {
        J::Object(o) => {
            for (k, x) in o {
                // A widget's own config map is the person's data, not a field.
                if k == "config" || k == "props" {
                    continue;
                }
                if banned.iter().any(|b| b == k) {
                    out.push(format!("{at}.{k}"));
                }
                banned_keys(x, &format!("{at}.{k}"), banned, out);
            }
        }
        J::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                banned_keys(x, &format!("{at}[{i}]"), banned, out);
            }
        }
        _ => {}
    }
}

#[test]
fn no_secret_reaches_the_phone() {
    let fx = fx(&["store:read", "store:write"]);
    let banned = &k2_core::contract::catalog().banned_fields;
    let mut bodies = Vec::new();
    for path in all_routes() {
        let (params, body) = any_request(&fx, path);
        let params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let (_, v) = req(path, &fx.phone, &params, body);
        bodies.push((path, v));
    }
    for (path, v) in &bodies {
        let text = v.to_string();
        for secret in [fx.phone.as_str(), fx.password.as_str(), OWNER_TOKEN, PASSWORD, fx.passport.as_str()] {
            assert!(!text.contains(secret), "{path} leaks a credential");
        }
        let home = std::env::var("HOME").expect("HOME");
        assert!(!text.contains(&home), "{path} names a path under HOME: {text}");
        let mut found = Vec::new();
        banned_keys(v, "", banned, &mut found);
        assert!(found.is_empty(), "{path}: banned fields {found:?}");
    }
}

// ── golden samples (§7.3, §9) ───────────────────────────────────────

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/gardens_remote")
}

/// Fixed stand-ins for what changes run to run.
struct Norm {
    subs: Vec<(String, String)>,
}

impl Norm {
    fn text(&self, s: &str) -> String {
        let mut s = s.to_string();
        for (a, b) in &self.subs {
            if !a.is_empty() {
                s = s.replace(a.as_str(), b);
            }
        }
        s
    }

    fn value(&self, v: &J, key: Option<&str>) -> J {
        match v {
            J::Object(o) => J::Object(o.iter().map(|(k, x)| (k.clone(), self.value(x, Some(k)))).collect()),
            J::Array(a) => J::Array(a.iter().map(|x| self.value(x, key)).collect()),
            J::String(s) => match key {
                Some("version" | "cursor" | "since" | "serverId" | "lastGoodAt" | "at" | "addedAt" | "updatedAt" | "upload") => json!(format!("<{}>", key.unwrap_or_default())),
                Some("nonce") => json!("<nonce>"),
                _ => json!(self.text(s)),
            },
            J::Number(_) if matches!(key, Some("rev" | "seq" | "updatedAtMs" | "ms" | "updatedAt" | "addedAt" | "createdAt" | "at")) => json!(0),
            other => other.clone(),
        }
    }
}

fn samples() -> BTreeMap<String, String> {
    let fx = fx(&["store:read", "store:write"]);
    let mut out: BTreeMap<String, J> = BTreeMap::new();
    let mut put = |name: &str, path: &str, token: &str, params: &[(&str, &str)], body: J| {
        let (s, v) = req(path, token, params, body.clone());
        out.insert(name.to_string(), json!({ "request": { "path": path, "query": params.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>(), "body": body }, "status": s, "body": v }));
    };
    let g = fx.d.garden.clone();
    let h = fx.d.hash();
    put("gardens", GARDENS, &fx.phone, &[], J::Null);
    let cursor = ok_of(GARDENS, &fx.phone, &[], J::Null)["cursor"].as_str().expect("cursor").to_string();
    put("gardens-unchanged", GARDENS, &fx.phone, &[("since", &cursor)], J::Null);
    put("page", PAGE, &fx.phone, &[("garden", &g)], J::Null);
    put("frame", FRAME, &fx.phone, &[("garden", &g), ("placement", "b1"), ("scheme", "dark")], J::Null);
    put("call-store-put", CALL, &fx.phone, &[], call_body(&fx, "store.put", json!(["obj/1", { "t": "hi" }])));
    put("call-store-get", CALL, &fx.phone, &[], call_body(&fx, "store.get", json!(["obj/1"])));
    let mut stale = call_body(&fx, "store.get", json!(["obj/1"]));
    stale["hash"] = json!("0".repeat(64));
    put("call-stale-widget", CALL, &fx.phone, &[], stale);
    put("call-verb-unavailable", CALL, &fx.phone, &[], call_body(&fx, "thread.post", json!([])));
    put("call-not-exposed", CALL, &fx.phone, &[], call_body(&fx, "window.startDrag", json!([])));
    put("blob-put", BLOB_PUT, &fx.phone, &[], json!({ "garden": g, "placement": "b1", "hash": h, "offset": 0, "base64": "aGk=", "last": true, "name": "hi.txt" }));
    put("report-ready", REPORT, &fx.phone, &[], json!({ "garden": g, "placement": "b1", "hash": h, "event": "ready" }));
    put("resume", RESUME, &fx.phone, &[], json!({ "garden": g, "placement": "b1" }));
    put("agents-order", AGENTS_ORDER, &fx.phone, &[("garden", &g)], J::Null);
    put("lib-fetch-bundled", LIB_FETCH, &fx.phone, &[], json!({ "id": "uplot", "version": "1.6.32" }));
    put("refused-garden-person-only", GARDENS, &fx.other_owner_phone, &[], J::Null);
    put("refused-garden-device-only", GARDENS, &fx.password, &[], J::Null);
    // Every Garden id K2 minted (random), in list order.
    let ids: Vec<(String, String)> =
        fx.d.f.gardens().into_iter().enumerate().map(|(i, e)| (e.id, format!("g-{:08}", i + 1))).collect();
    assert_eq!(ids[0].0, g, "the door's Garden is the first");
    let norm = Norm {
        subs: [
            ids,
            vec![
            (h.clone(), "<hash>".into()),
            (k2_core::zen::widget_shot::script_text(k2_core::zen::widget_shot::SEALED_FRAME_GUARD), "{{GUARD}}".into()),
            (k2_core::zen::widget_shot::script_text(k2_core::zen::widget_shot::K2_FRAME_JS), "{{K2FRAME}}".into()),
            ],
        ]
        .concat(),
    };
    // The frame's nonce appears in its srcdoc too.
    let nonce = out["frame"]["body"]["nonce"].as_str().expect("frame nonce").to_string();
    let norm = Norm { subs: [norm.subs, vec![(nonce, "<nonce>".into())]].concat() };
    out.into_iter()
        .map(|(k, v)| (format!("{k}.json"), format!("{}\n", serde_json::to_string_pretty(&norm.value(&v, None)).expect("json"))))
        .collect()
}

#[test]
fn golden_samples_are_what_the_daemon_writes() {
    let fresh = samples();
    let dir = golden_dir();
    if std::env::var_os("UPDATE_GARDENS_REMOTE_SAMPLES").is_some() {
        std::fs::create_dir_all(&dir).expect("golden dir");
        for e in std::fs::read_dir(&dir).expect("read golden dir").flatten() {
            if e.path().extension().is_some_and(|x| x == "json") {
                std::fs::remove_file(e.path()).expect("remove an old sample");
            }
        }
        for (name, text) in &fresh {
            std::fs::write(dir.join(name), text).expect("write sample");
        }
        return;
    }
    let mut on_disk: BTreeMap<String, String> = BTreeMap::new();
    for e in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}; run with UPDATE_GARDENS_REMOTE_SAMPLES=1", dir.display())).flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".json") {
            on_disk.insert(name, std::fs::read_to_string(e.path()).expect("read sample"));
        }
    }
    assert_eq!(
        on_disk.keys().collect::<Vec<_>>(),
        fresh.keys().collect::<Vec<_>>(),
        "the sample set changed; run with UPDATE_GARDENS_REMOTE_SAMPLES=1"
    );
    for (name, text) in &fresh {
        assert_eq!(&on_disk[name], text, "{name} drifted from the daemon's answer; run with UPDATE_GARDENS_REMOTE_SAMPLES=1 and update the phone contract");
    }
    // No sample names a banned field (§9 "Projections").
    let banned = &k2_core::contract::catalog().banned_fields;
    for (name, text) in &fresh {
        let v: J = serde_json::from_str(text).expect("sample json");
        let mut found = Vec::new();
        banned_keys(&v["body"], "", banned, &mut found);
        assert!(found.is_empty(), "{name}: {found:?}");
    }
}
