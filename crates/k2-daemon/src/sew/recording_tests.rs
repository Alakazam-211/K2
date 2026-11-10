//! `k2 hostchat recording` (0.46.11 K-a) over a fake box: the gates
//! (`sew_feature_missing` until Sew reports `app_recording`, a protocol-15
//! helper), who may enable (an Owner or Admin in person; an agent gets
//! `person_only` and the card), the preflight's named fixes, the hard model
//! check, and the two switches (disable keeps everything, uninstall keeps
//! recordings).

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use k2_core::connect_users::Role;
use serde_json::{json, Value};

use super::*;
use crate::sew::ops::fake::{helper_ok, FakeOps, GIB, INSTALL_ID};
use crate::sew::routes::{handle_env, Who};
use crate::sew::secrets::MemSecrets;

const TARBALL: &str = "/home/k2/sew.tar.gz";
const REC: &str = "/home/k2/sew-recorder.tar.gz";
const TRA: &str = "/home/k2/sew-transcriber.tar.gz";
const MODEL: &str = "/home/k2/sew-asr-test-q8_0.gguf";

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../k2-root-proto/testdata/sew").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

struct T {
    env: Env<'static>,
    ops: &'static FakeOps,
    _db: k2_core::db::TestDbGuard,
}

fn bare() -> T {
    let ops: &'static FakeOps = Box::leak(Box::new(FakeOps::new()));
    {
        let mut f = ops.files.lock().unwrap();
        f.insert(TARBALL.into(), fixture("sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz"));
        f.insert(REC.into(), fixture("sew-recorder-0.0.2-x86_64-unknown-linux-gnu.tar.gz"));
        f.insert(TRA.into(), fixture("sew-transcriber-0.0.2-x86_64-unknown-linux-gnu.tar.gz"));
        f.insert(MODEL.into(), fixture("sew-asr-test-q8_0.gguf"));
    }
    let env = Env {
        ops,
        secrets: Box::leak(Box::new(MemSecrets::default())),
        latch: Box::leak(Box::new(AtomicBool::new(false))),
        token: Box::leak(Box::new(Mutex::new(None))),
    };
    let db = k2_core::db::scoped_for_test();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for (id, on) in [("w-it", 1), ("w-plain", 0)] {
            conn.execute(
                "INSERT INTO projects (id, name, path, handle, agents_can_manage_chat) VALUES (?1, ?1, ?2, ?1, ?3)",
                rusqlite::params![id, format!("/t/{id}"), on],
            )
            .unwrap();
        }
    }
    T { env, ops, _db: db }
}

/// Chat running, voice installed, a protocol-15 helper; `features` is what
/// Sew reports.
fn t_with(features: &[&str]) -> T {
    let t = bare();
    let (st, v) = t.call(&Who::Owner, "/cli/sew/server/enable", json!({"tarballPath": TARBALL}));
    assert_eq!(st, "200 OK", "{v}");
    t.reset();
    let mut h = helper_ok(15, true, k2_root_proto::sew::pinned().version);
    h.version_json.as_mut().unwrap()["namespaces"]["sewrecording"] = json!(true);
    *t.ops.helper.lock().unwrap() = h;
    *t.ops.voice.lock().unwrap() = Some(json!({"installed": true}));
    t.ops.mgmt_status.lock().unwrap()["features"] = json!(features);
    for (u, mib) in [(k2_root_proto::sew::UNIT_DB, 1216u64), (k2_root_proto::sew::UNIT_SEW, 1216), (k2_root_proto::sew::UNIT_MEDIA, 768)] {
        t.ops.units.lock().unwrap().entry(u.to_string()).or_default().memory_max = Some(mib * 1024 * 1024);
    }
    *t.ops.mem.lock().unwrap() = Some(8 * GIB);
    t
}

/// A box on Sew 0.9.0 (it reports `app_recording`).
fn t() -> T {
    t_with(&["app_governed_spaces", "app_recording", "voice_record_cap", "recording_required", "mgmt_recording_reads"])
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: "s1".into() }
}
fn login(role: Role) -> Who {
    Who::Login { username: "ana".into(), role }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

fn files() -> Value {
    json!({"recorderTarballPath": REC, "transcriberTarballPath": TRA, "modelFilePath": MODEL})
}

impl T {
    fn call(&self, who: &Who, path: &str, body: Value) -> (String, Value) {
        let r = handle_env(self.env, path, who, body.to_string().as_bytes(), &HashMap::new(), false);
        (r.status.to_string(), serde_json::from_str(&r.body).unwrap())
    }
    fn get(&self, who: &Who, path: &str) -> Value {
        let r = handle_env(self.env, path, who, b"", &HashMap::new(), false);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        serde_json::from_str(&r.body).unwrap()
    }
    /// The owner's enable with a fresh confirm bound to this install.
    fn enable(&self, extra: Value) -> (String, Value) {
        let tok = crate::vault::ensure_test_vault().confirm.mint(CONFIRM_ENABLE, Some(INSTALL_ID), "owner", "vault-password");
        let mut body = files();
        body["confirmToken"] = json!(tok);
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            body[k] = v;
        }
        self.call(&Who::Owner, ENABLE_ROUTE, body)
    }
    /// Forget the calls so far (helper, mgmt and the mgmt bodies together).
    fn reset(&self) {
        self.ops.clear();
        self.ops.mgmt_bodies.lock().unwrap().clear();
    }
    fn recording_calls(&self) -> Vec<String> {
        self.ops.calls.lock().unwrap().iter().map(|c| c.argv.clone()).filter(|a| a.starts_with("engine sew recording")).collect()
    }
    fn policy_posts(&self) -> Vec<Value> {
        let calls = self.ops.mgmt_calls.lock().unwrap().clone();
        let bodies = self.ops.mgmt_bodies.lock().unwrap().clone();
        calls.iter().zip(bodies).filter(|(c, _)| *c == "POST /mgmt/v1/policies").map(|(_, b)| b.unwrap_or(Value::Null)).collect()
    }
}

// ── the gate ─────────────────────────────────────────────────────────────

/// PRD §8 K-a: on Sew 0.8.2's flags every write is `sew_feature_missing`
/// (exit 3 in the CLI) and nothing is staged, installed or set.
#[test]
fn on_sew_without_app_recording_every_write_is_sew_feature_missing() {
    // Sew 0.8.2's thirty-one flags have none of the four.
    let t = t_with(&["app_governed_spaces", "app_participant_links", "principal_erase", "voice_media_path"]);
    let (st, v) = t.enable(json!({}));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "sew_feature_missing"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("Sew 0.9.0"), "{v}");
    for (path, body) in [(DISABLE_ROUTE, json!({})), (UNINSTALL_ROUTE, json!({"confirm": true}))] {
        let (st, v) = t.call(&Who::Owner, path, body);
        assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "sew_feature_missing"), "{path}: {v}");
    }
    assert!(t.recording_calls().is_empty(), "{:?}", t.recording_calls());
    assert!(t.policy_posts().is_empty(), "no policy sent");
    // The read says why, so `k2 hostchat recording enable` stops before asking for Touch ID.
    let s = t.get(&Who::Owner, ROUTE);
    assert_eq!((s["ready"]["ok"].as_bool(), s["ready"]["code"].as_str()), (Some(false), Some("sew_feature_missing")), "{s}");
    assert_eq!(s["flags"]["app_recording"], false);
    let p = t.get(&Who::Owner, PREFLIGHT_ROUTE);
    assert_eq!(p["ready"], false);
}

/// Protocol-15 compatibility, the new daemon's side: a protocol-14 helper
/// (or a 15 without the namespace flag) is `helper_outdated`, a clear 409
/// naming protocol 15, and nothing runs. (The older daemon's side, a 15
/// helper serving 14's calls, is in k2-root-helper's sew_tests.)
#[test]
fn a_helper_below_protocol_15_is_helper_outdated_and_nothing_runs() {
    for h in [
        helper_ok(14, true, k2_root_proto::sew::pinned().version),
        {
            let mut h = helper_ok(15, true, k2_root_proto::sew::pinned().version);
            h.version_json.as_mut().unwrap()["namespaces"]["sewrecording"] = json!(false);
            h
        },
    ] {
        let t = t();
        let proto = h.protocol;
        *t.ops.helper.lock().unwrap() = h;
        let (st, v) = t.enable(json!({}));
        assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "helper_outdated"), "{proto}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("protocol 15"), "{v}");
        assert!(t.recording_calls().is_empty(), "{proto}");
        assert!(t.ops.calls.lock().unwrap().iter().all(|c| !c.argv.contains("recording")), "no recording argv reaches an old helper");
        let pf = t.get(&Who::Owner, PREFLIGHT_ROUTE);
        let helper = pf["checks"].as_array().unwrap().iter().find(|c| c["id"] == "helper").unwrap().clone();
        assert_eq!(helper["status"], "fail");
        assert!(helper["hint"].as_str().unwrap().contains("k2 root-helper update"), "{helper}");
    }
}

// ── who ──────────────────────────────────────────────────────────────────

/// PRD §8 K-a: an agent, even with Manage Chat / Voice Hosting, gets
/// `person_only` and the card's line; it never stages or installs anything.
#[test]
fn an_agent_gets_person_only_and_the_card_line_never_an_install() {
    let t = t();
    let (st, v) = t.call(&agent("w-it"), ENABLE_ROUTE, files());
    assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "person_only"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("k2 permission request sew.recording-enable --why"), "{v}");
    let (st, v) = t.call(&agent("w-plain"), ENABLE_ROUTE, files());
    assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "chat_manage_off"), "{v}");
    let (_, v) = t.call(&Who::Shell { project_id: "w-it".into(), session_id: "s2".into() }, ENABLE_ROUTE, files());
    assert_eq!(code(&v), "person_only", "a K2 terminal is agent-level: {v}");
    let (_, v) = t.call(&login(Role::Member), ENABLE_ROUTE, files());
    assert_eq!(code(&v), "owner_only", "{v}");
    // Even with a confirm token in hand an agent is refused.
    let tok = crate::vault::ensure_test_vault().confirm.mint(CONFIRM_ENABLE, Some(INSTALL_ID), "owner", "vault-password");
    let mut b = files();
    b["confirmToken"] = json!(tok);
    assert_eq!(code(&t.call(&agent("w-it"), ENABLE_ROUTE, b).1), "person_only");
    assert!(t.recording_calls().is_empty() && t.policy_posts().is_empty());
}

/// An Owner or Admin, in person: a fresh confirm bound to this install.
#[test]
fn enable_needs_a_fresh_confirm_bound_to_this_install() {
    let t = t();
    let (st, v) = t.call(&Who::Owner, ENABLE_ROUTE, files());
    assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "owner_confirm_required"), "{v}");
    let vault = crate::vault::ensure_test_vault();
    let mut b = files();
    b["confirmToken"] = json!(vault.confirm.mint(CONFIRM_ENABLE, Some("123"), "owner", "vault-password"));
    assert_eq!(code(&t.call(&Who::Owner, ENABLE_ROUTE, b).1), "owner_confirm_required", "another install");
    let mut b = files();
    b["confirmToken"] = json!(vault.confirm.mint(crate::sew::routes::CONFIRM_PURGE, Some(INSTALL_ID), "owner", "vault-password"));
    assert_eq!(code(&t.call(&Who::Owner, ENABLE_ROUTE, b).1), "owner_confirm_required", "another action");
    assert!(t.recording_calls().is_empty());
    // An Admin login's own confirm works.
    let mut b = files();
    b["confirmToken"] = json!(vault.confirm.mint(CONFIRM_ENABLE, Some(INSTALL_ID), "connect:ana", "connect-password"));
    let (st, v) = t.call(&login(Role::Admin), ENABLE_ROUTE, b);
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["enabled"], true);
}

// ── enable / disable / uninstall ─────────────────────────────────────────

#[test]
fn enable_stages_installs_then_sets_the_org_policy_on() {
    let t = t();
    let (st, v) = t.enable(json!({}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!((v["installed"].as_bool(), v["enabled"].as_bool(), v["model"].as_str()), (Some(true), Some(true), Some("test-q8_0")), "{v}");
    let calls: Vec<(String, Option<Vec<u8>>)> =
        t.ops.calls.lock().unwrap().iter().filter(|c| c.argv.starts_with("engine sew recording")).map(|c| (c.argv.clone(), c.stdin.clone())).collect();
    let argv: Vec<&str> = calls.iter().map(|(a, _)| a.as_str()).collect();
    assert_eq!(
        argv,
        [
            "engine sew recording plan",
            "engine sew recording status",
            "engine sew recording stage",
            "engine sew recording stage",
            "engine sew recording stage",
            "engine sew recording install",
            "engine sew recording status"
        ]
    );
    let staged: Vec<Vec<u8>> = calls[2..5].iter().map(|(_, s)| s.clone().unwrap()).collect();
    assert_eq!(staged, [fixture("sew-recorder-0.0.2-x86_64-unknown-linux-gnu.tar.gz"), fixture("sew-transcriber-0.0.2-x86_64-unknown-linux-gnu.tar.gz"), fixture("sew-asr-test-q8_0.gguf")]);
    let install: Value = serde_json::from_slice(calls[5].1.as_ref().unwrap()).unwrap();
    assert_eq!(install, json!({"model": "test-q8_0"}), "the pinned model by name, never auto");
    assert_eq!(t.policy_posts(), [json!({"changes": {"recording.enabled": true}})]);
    // k2 hostchat status carries it.
    let s = t.get(&Who::Owner, "/cli/sew/status");
    assert_eq!(s["recording"], json!({"installed": true, "enabled": true, "version": "0.0.2"}), "{s}");
    let r = t.get(&login(Role::Member), ROUTE);
    assert_eq!((r["installed"].as_bool(), r["enabled"].as_bool()), (Some(true), Some(true)), "{r}");
    assert_eq!(r["version"]["recorder"], "0.0.2");
    assert_eq!(r["policies"]["recording.enabled"], true);
    assert_eq!(r["flags"]["app_recording"], true);

    // Again: installed already, so only the policy.
    t.reset();
    let (st, v) = t.enable(json!({}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["alreadyInstalled"], true);
    assert!(!t.recording_calls().iter().any(|a| a.ends_with("stage") || a.ends_with("install")), "{:?}", t.recording_calls());
}

/// The Kessel Steward's condition R28: a model whose sha256 isn't the pin
/// is a hard failure before anything is staged or any Sew step runs; no
/// other model is tried.
#[test]
fn a_model_whose_sha256_isnt_the_pin_refuses_before_any_sew_call() {
    let t = t();
    let mut doctored = fixture("sew-asr-test-q8_0.gguf");
    doctored[0] ^= 1;
    t.ops.files.lock().unwrap().insert(MODEL.into(), doctored);
    let (st, v) = t.enable(json!({}));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "model_sha256_mismatch"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("nothing was installed"), "{v}");
    let calls = t.recording_calls();
    assert_eq!(calls, ["engine sew recording plan", "engine sew recording status"], "nothing staged or installed");
    assert!(t.policy_posts().is_empty());
    let r = t.get(&Who::Owner, ROUTE);
    assert_eq!(r["installed"], false);
    assert!(r["lastError"].as_str().unwrap().starts_with("model_sha256_mismatch"), "{r}");
    // The same for a sidecar tarball that isn't the pin.
    let t2 = self::t();
    t2.ops.files.lock().unwrap().insert(REC.into(), fixture("sew-media-0.0.2-x86_64-unknown-linux-gnu.tar.gz"));
    let (_, v) = t2.enable(json!({}));
    assert_eq!(code(&v), "sha256_mismatch", "{v}");
    assert!(!t2.recording_calls().iter().any(|a| a.ends_with("stage") || a.ends_with("install")), "{:?}", t2.recording_calls());
}

#[test]
fn disable_keeps_the_recorder_and_uninstall_needs_confirm_and_keeps_recordings() {
    let t = t();
    assert_eq!(t.enable(json!({})).0, "200 OK");
    t.reset();
    let (st, v) = t.call(&agent("w-it"), DISABLE_ROUTE, json!({}));
    assert_eq!(st, "200 OK", "a Manage-Sew agent may turn it off (narrowing): {v}");
    assert_eq!(v["enabled"], false);
    assert!(v["kept"].as_str().unwrap().contains("every recording is kept"), "{v}");
    assert_eq!(t.policy_posts(), [json!({"changes": {"recording.enabled": false}})]);
    assert!(t.recording_calls().is_empty(), "disable flips the policy only: {:?}", t.recording_calls());
    assert_eq!(t.get(&Who::Owner, "/cli/sew/status")["recording"], json!({"installed": true, "enabled": false, "version": "0.0.2"}));

    let (st, v) = t.call(&Who::Owner, UNINSTALL_ROUTE, json!({}));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "confirm_required"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("every recording is kept"), "{v}");
    let (st, v) = t.call(&Who::Owner, UNINSTALL_ROUTE, json!({"confirm": true}));
    assert_eq!(st, "200 OK", "{v}");
    assert!(v["kept"].as_str().unwrap().contains("every recording stays"), "{v}");
    assert_eq!(t.recording_calls(), ["engine sew recording uninstall"], "no stage, install or disable");
    assert_eq!(t.get(&Who::Owner, "/cli/sew/status")["recording"], json!({"installed": false, "enabled": false, "version": null}));
    let (_, v) = t.call(&login(Role::Member), UNINSTALL_ROUTE, json!({"confirm": true}));
    assert_eq!(code(&v), "owner_only");
}

// ── preflight and Sew's refusals ─────────────────────────────────────────

fn check_of<'a>(p: &'a Value, id: &str) -> &'a Value {
    p["checks"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("no {id} check: {p}"))
}

#[test]
fn every_failed_preflight_check_names_its_fix_and_enable_changes_nothing() {
    let t = t();
    let p = t.get(&Who::Owner, PREFLIGHT_ROUTE);
    assert_eq!(p["ready"], true, "{p}");
    assert_eq!(check_of(&p, "fit")["status"], "ok", "Sew's own 8 GiB table: {p}");
    assert_eq!(check_of(&p, "plan")["status"], "ok");

    *t.ops.mem.lock().unwrap() = Some(4 * GIB);
    *t.ops.voice.lock().unwrap() = Some(json!({"installed": false}));
    *t.ops.disk.lock().unwrap() = Some(5 * GIB);
    let p = t.get(&Who::Owner, PREFLIGHT_ROUTE);
    assert_eq!(p["ready"], false);
    for (id, fix) in [
        ("ram", "at least 8 GiB of RAM"),
        ("voice", "k2 hostchat media install"),
        ("disk", "k2 app resources"),
        ("fit", "bigger box"),
    ] {
        let c = check_of(&p, id);
        assert_eq!(c["status"], "fail", "{id}: {c}");
        assert!(c["hint"].as_str().unwrap().contains(fix), "{id}: {c}");
    }
    let (st, v) = t.enable(json!({}));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "preflight_failed"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("8 GiB"), "{v}");
    assert!(v["error"]["report"].is_array());
    assert!(!t.recording_calls().iter().any(|a| a.ends_with("stage") || a.ends_with("install")));
}

/// Sew's install refusals (all exit 3), each to a named fix, in Sew's order.
#[test]
fn sews_install_refusals_map_to_named_fixes() {
    for (msg, want) in [
        ("voice is not installed; run sew media install first", "voice_not_installed"),
        ("recording_needs_loopback_media: voice.ops.bind_v4 = 10.0.0.2 is a specific non-loopback address", "recording_needs_loopback_media"),
        ("recording_unpinned_build: this sew build has no baked sidecar pins (a dev build)", "recording_unpinned_build"),
        ("recorder tarball sha256 does not match the pin", "sidecar_sha256_mismatch"),
        ("recording needs at least 8 GiB of RAM; this box has 4096 MiB", "recording_needs_8gib"),
        ("model \"x\" is not a pinned, selectable model of this sew build", "model_unknown"),
        ("model_does_not_fit: 4787 > 4404; models that fit: none", "model_does_not_fit"),
        ("model_sha256_mismatch: /m has sha256 aa, the pin for base-q8_0 is bb", "model_sha256_mismatch"),
        ("service_account_unsafe: sew-recorder has a login shell", "service_account_unsafe"),
        ("refused: this box is K2-managed (k2_managed)", "sew_k2_managed"),
        ("something new", "recording_install_failed"),
    ] {
        let (code, fix) = refusal_of(msg);
        assert_eq!(code, want, "{msg}");
        assert!(!fix.is_empty(), "{want} names a fix");
    }
    // Through the machine: Sew's words and K2's fix together, nothing set.
    let t = t();
    t.ops.answers.lock().unwrap().entry("engine sew recording install".into()).or_default().push_back(Ok(json!({
        "sewExit": 3, "step": "install", "reason": "model_does_not_fit: 4787 > 4404; models that fit: none"
    })));
    let (st, v) = t.enable(json!({}));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "model_does_not_fit"), "{v}");
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("Sew said: model_does_not_fit") && hint.contains("free memory"), "{hint}");
    assert!(t.policy_posts().is_empty(), "no policy without the recorder");
}

#[test]
fn the_routes_are_post_only_with_policy_rows_and_reach_the_handler_for_agents() {
    use crate::routes::route_policy::lookup;
    for p in POST_ROUTES {
        assert!(crate::sew::routes::is_post_route(p), "{p}: POST only (405 on GET before auth)");
        assert!(lookup(p).and_then(|r| r.post).is_some(), "{p}");
        assert!(crate::session_token::is_agent_verb(p), "{p}");
    }
    for p in [ROUTE, PREFLIGHT_ROUTE] {
        assert!(crate::sew::routes::is_route(p) && !crate::sew::routes::is_post_route(p), "{p}");
        assert!(lookup(p).and_then(|r| r.get).is_some(), "{p}");
        assert!(crate::session_token::is_agent_verb(p), "{p}");
    }
}
