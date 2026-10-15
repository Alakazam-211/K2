//! Route tests for `k2 chat` and the chat approval queue, against the
//! recording fake Sew (`fake.rs`) and a scoped in-memory DB. Fail loudly:
//! every expectation is an assert on the exact code/status.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use k2_core::connect_users::Role;
use k2_core::sew_chat::{self as core, store, ApprovalMode, Level};

use super::fake::FakeSew;
use super::routes::{self, test_clock, test_hooks};
use super::routes_approvals::test_seams as approval_seams;
use super::sew::{test_seam, SewError};
use crate::compute_routes::Who;

const WA: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const WB: &str = "bbbbbbbb-0000-4000-8000-000000000002";
const WC: &str = "cccccccc-0000-4000-8000-000000000003";

struct Env {
    _db: k2_core::db::TestDbGuard,
    fake: Arc<FakeSew>,
    /// Sew member ids.
    alpha: String,
    rosson: String,
}

impl Drop for Env {
    fn drop(&mut self) {
        test_seam::set(None);
        approval_seams::set_vault(None);
        test_clock::set(0);
        let _ = test_hooks::take();
    }
}

fn env() -> Env {
    let db = k2_core::db::scoped_for_test();
    {
        let h = k2_core::db::shared();
        let c = h.lock();
        for (id, name) in [(WA, "alpha"), (WB, "beta"), (WC, "gamma")] {
            c.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
                rusqlite::params![id, name, format!("/t/{name}")],
            )
            .unwrap();
        }
    }
    let fake = Arc::new(FakeSew::new());
    fake.add_space("sp-hearth", "hearth", true);
    fake.add_space("sp-eng", "eng", false);
    let alpha = fake.add_agent(WA, "alpha");
    let beta = fake.add_agent(WB, "beta");
    fake.add_agent(WC, "gamma");
    let rosson = fake.add_person("rosson");
    fake.add_channel("sp-hearth", "c-general", "general", None, true);
    fake.add_channel("sp-hearth", "c-secret", "secret", Some(&[rosson.as_str()]), true);
    fake.add_channel("sp-hearth", "c-announce", "announce", None, false);
    fake.add_channel("sp-eng", "c-eng", "builds", None, true);
    fake.add_dm("c-dm-ar", &[alpha.as_str(), rosson.as_str()]);
    let _ = beta;
    test_seam::set(Some(fake.clone()));
    approval_seams::set_vault(Some(crate::compute_admin::test_seams::vault_handle()));
    let _ = test_hooks::take();
    Env { _db: db, fake, alpha, rosson }
}

fn agent(p: &str) -> Who {
    Who::Agent { project_id: p.into(), session_id: format!("sess-{}", &p[..8]) }
}

fn admin() -> Who {
    Who::Login { username: "ada".into(), role: Role::Admin }
}

fn grant(p: &str, space_id: &str, slug: &str, level: Level, approval: ApprovalMode) {
    let h = k2_core::db::shared();
    let c = h.lock();
    store::set_access(
        &c,
        &store::Access {
            project_id: p.into(),
            space_id: space_id.into(),
            space_slug: slug.into(),
            level,
            approval,
            updated_by: "owner".into(),
            updated_at: 1,
        },
    )
    .unwrap();
}

fn hearth(p: &str, level: Level, approval: ApprovalMode) {
    grant(p, "sp-hearth", "hearth", level, approval);
}

fn status_code(s: &str) -> u16 {
    s.split_whitespace().next().unwrap().parse().unwrap()
}

fn post(path: &str, who: &Who, body: Value) -> (u16, Value) {
    let r = routes::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new(), true);
    (status_code(r.status), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: {e}: {}", r.body)))
}

fn get(path: &str, who: &Who, params: &[(&str, &str)]) -> (u16, Value) {
    let p: HashMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = routes::handle(path, who.clone(), b"", &p, false);
    (status_code(r.status), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: {e}: {}", r.body)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error code in {v}"))
}

fn send(who: &Who, to: &str, text: &str) -> (u16, Value) {
    post("/cli/sew/chat/send", who, json!({"to": to, "text": text}))
}

fn mint(action: &str, id: &str, stamp: &str) -> String {
    crate::compute_admin::test_seams::vault_handle().confirm.mint(action, Some(id), stamp, "vault-password")
}

fn set_manage(p: &str, on: bool) {
    k2_core::workspace::settings::set_agents_can_manage_chat(p, on).unwrap();
}

fn set_policy(by: core::ApprovalsBy) {
    let h = k2_core::db::shared();
    let c = h.lock();
    store::set_approvals_by(&c, by, "owner", 1).unwrap();
}

fn outbox_row(id: &str) -> store::OutboxRow {
    let h = k2_core::db::shared();
    let c = h.lock();
    store::outbox(&c, id).unwrap().unwrap()
}

fn actions(p: &str) -> Vec<store::ActionRow> {
    let h = k2_core::db::shared();
    let c = h.lock();
    store::actions_for(&c, p, 500).unwrap()
}

fn queue_one(e: &Env, text: &str) -> String {
    let _ = e;
    let (s, v) = send(&agent(WA), "hearth/general", text);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "queued", "{v}");
    v["id"].as_str().unwrap().to_string()
}

// ── classification and transport ─────────────────────────────────────

#[test]
fn every_route_is_classified_post_guarded_and_an_agent_verb() {
    use crate::routes::route_policy::{lookup, Floor};
    for p in routes::ROUTES {
        let r = lookup(p).unwrap_or_else(|| panic!("{p} not in route_policy::ROUTES"));
        if routes::is_post_route(p) {
            assert!(r.post.is_some() && r.get.is_none(), "{p} is POST-only in the table");
            assert!(crate::routes::route_policy::get_refused(p, ""), "{p}: a GET is refused (405) before auth");
        } else {
            assert!(r.get.is_some() && r.post.is_none(), "{p} is GET-only in the table");
        }
        let floor = r.get.or(r.post).unwrap();
        if p.starts_with("/cli/sew/chat/") {
            assert_eq!(floor, Floor::NoLogin, "{p}: chat verbs take passports, never a login");
        } else {
            assert_eq!(floor, Floor::Admin, "{p}: approvals are owner/admin (or a Manage-Sew passport)");
        }
        assert!(crate::session_token::is_agent_verb(p), "{p} must be served for a passport");
    }
    for p in routes::POST_ROUTES {
        assert!(routes::is_route(p), "{p}");
    }
    // The Sew server surfaces are NOT agent verbs (S29 owns them).
    for p in ["/cli/sew/server/enable", "/cli/sew/pass", "/cli/sew/status", "/cli/sew/admin/agents/grant"] {
        assert!(!crate::session_token::is_agent_verb(p), "{p}");
    }
}

#[test]
fn get_on_a_post_route_is_405_and_bodies_are_checked() {
    let _e = env();
    for p in routes::POST_ROUTES {
        let (s, v) = get(p, &agent(WA), &[]);
        assert_eq!(s, 405, "{p}: {v}");
    }
    let r = routes::too_large();
    assert_eq!(status_code(r.status), 413);
}

#[test]
fn only_passports_use_chat_verbs_and_shell_tabs_are_agent_level() {
    let _e = env();
    hearth(WA, Level::Send, ApprovalMode::None);
    let (s, v) = send(&Who::Owner, "hearth/general", "hi");
    assert_eq!((s, code(&v)), (403, "agent_only"));
    let (s, v) = get("/cli/sew/chat/whoami", &admin(), &[]);
    assert_eq!((s, code(&v)), (403, "agent_only"));
    let (s, v) = get("/cli/sew/chat/whoami", &Who::Wrong, &[]);
    assert_eq!((s, code(&v)), (403, "wrong_credential"));
    let (s, v) = get("/cli/sew/chat/whoami", &Who::Invalid, &[]);
    assert_eq!((s, code(&v)), (401, "invalid_token"));
    // A K2 shell tab's passport is agent-level: it chats as its workspace.
    let shell = Who::Shell { project_id: WA.into(), session_id: "sess-shell".into() };
    let (s, v) = send(&shell, "hearth/general", "from a shell tab");
    assert_eq!(s, 200, "{v}");
    let w = _e.fake.writes();
    assert_eq!(w.len(), 1);
    assert_eq!((w[0].external_id.as_str(), w[0].k2_session.as_str()), (format!("k2:workspace:{WA}").as_str(), "sess-shell"));
}

#[test]
fn identity_comes_from_the_passport_never_the_body() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::None);
    for field in ["from", "project", "workspace", "agent"] {
        let (s, v) = post("/cli/sew/chat/send", &agent(WA), json!({"to": "hearth/general", "text": "x", field: WB}));
        assert_eq!((s, code(&v)), (400, "usage"), "{field}: {v}");
    }
    assert!(e.fake.writes().is_empty(), "nothing reached Sew");
    let (s, v) = send(&agent(WA), "hearth/general", "hello");
    assert_eq!(s, 200, "{v}");
    let w = e.fake.writes();
    assert_eq!(w[0].external_id, format!("k2:workspace:{WA}"));
    assert_eq!(w[0].k2_session, format!("sess-{}", &WA[..8]));
}

#[test]
fn sew_off_is_503_and_no_access_is_not_granted() {
    let _e = env();
    let (s, v) = send(&agent(WA), "hearth/general", "hi");
    assert_eq!((s, code(&v)), (403, "not_granted"), "no access row anywhere");
    hearth(WA, Level::Send, ApprovalMode::None);
    test_seam::set(None);
    let (s, v) = send(&agent(WA), "hearth/general", "hi");
    assert_eq!((s, code(&v)), (503, "chat_unavailable"));
    let (s, v) = get("/cli/sew/chat/spaces", &agent(WA), &[]);
    assert_eq!((s, code(&v)), (503, "chat_unavailable"));
}

// ── levels ───────────────────────────────────────────────────────────

#[test]
fn read_level_reads_but_every_write_is_level_denied() {
    let e = env();
    hearth(WA, Level::Read, ApprovalMode::None);
    let top = e.fake.post(&e.rosson, "c-general", "deploy done", None, &[&e.alpha]);
    let mine = e.fake.post(&e.alpha, "c-general", "old post of mine", None, &[]);
    let (s, v) = get("/cli/sew/chat/messages", &agent(WA), &[("in", "hearth/general")]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["messages"].as_array().unwrap().len(), 2);
    assert_eq!(v["messages"][0]["mentionsMe"], true);
    let attempts: Vec<(&str, Value)> = vec![
        ("/cli/sew/chat/send", json!({"to": "hearth/general", "text": "x"})),
        ("/cli/sew/chat/reply", json!({"to": format!("msg_{top}"), "text": "x"})),
        ("/cli/sew/chat/dm", json!({"to": "rosson", "text": "x"})),
        ("/cli/sew/chat/edit", json!({"id": format!("msg_{mine}"), "text": "x"})),
        ("/cli/sew/chat/delete", json!({"id": format!("msg_{mine}")})),
    ];
    for (p, b) in attempts {
        let (s, v) = post(p, &agent(WA), b);
        assert_eq!((s, code(&v)), (403, "level_denied"), "{p}: {v}");
        assert_eq!(v["error"]["details"]["level"], "read");
    }
    assert!(e.fake.writes().is_empty(), "no Sew write at level read");
    let refused = actions(WA).into_iter().filter(|a| a.status == "refused" && a.reason.as_deref() == Some("level_denied")).count();
    assert_eq!(refused, 5, "every refused write is audited");
}

#[test]
fn reply_level_replies_but_cannot_post_top_level_or_dm() {
    let e = env();
    hearth(WA, Level::Reply, ApprovalMode::None);
    let top = e.fake.post(&e.rosson, "c-general", "can someone check?", None, &[&e.alpha]);
    let (s, v) = send(&agent(WA), "hearth/general", "top level");
    assert_eq!((s, code(&v)), (403, "level_denied"), "{v}");
    assert_eq!(v["error"]["details"]["needs"], "send");
    let (s, v) = post("/cli/sew/chat/dm", &agent(WA), json!({"to": "@rosson", "text": "hi"}));
    assert_eq!((s, code(&v)), (403, "level_denied"), "{v}");
    let (s, v) = post("/cli/sew/chat/reply", &agent(WA), json!({"to": format!("msg_{top}"), "text": "on it"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "posted");
    let reply_id = v["id"].as_str().unwrap().to_string();
    // --broadcast is fine at reply level.
    let (s, v) = post("/cli/sew/chat/reply", &agent(WA), json!({"to": format!("msg_{top}"), "text": "also here", "broadcast": true}));
    assert_eq!(s, 200, "{v}");
    // The reply was addressed (the root mentions the agent): recorded.
    let a = actions(WA).into_iter().find(|a| a.action == "reply").unwrap();
    assert_eq!(a.addressed, Some(true));
    // Following a thread also counts (A3); a thread that doesn't address
    // the agent is still answerable at reply level, recorded as not addressed.
    let followed = e.fake.post(&e.rosson, "c-general", "status?", None, &[]);
    e.fake.follow(&e.alpha, &followed);
    let (s, v) = post("/cli/sew/chat/reply", &agent(WA), json!({"to": format!("msg_{followed}"), "text": "green"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(actions(WA)[0].addressed, Some(true), "following");
    let cold = e.fake.post(&e.rosson, "c-general", "anyone?", None, &[]);
    let (s, v) = post("/cli/sew/chat/reply", &agent(WA), json!({"to": format!("msg_{cold}"), "text": "me"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(actions(WA)[0].addressed, Some(false), "not addressed, still allowed at reply");
    // Editing your own reply needs reply; deleting your own needs reply.
    let (s, v) = post("/cli/sew/chat/edit", &agent(WA), json!({"id": reply_id, "text": "on it (edited)"}));
    assert_eq!(s, 200, "{v}");
    let mine_top = e.fake.post(&e.alpha, "c-general", "my old top-level post", None, &[]);
    let (s, v) = post("/cli/sew/chat/edit", &agent(WA), json!({"id": format!("msg_{mine_top}"), "text": "changed"}));
    assert_eq!((s, code(&v)), (403, "level_denied"), "editing a top-level post needs send now: {v}");
    let (s, v) = post("/cli/sew/chat/delete", &agent(WA), json!({"id": format!("msg_{mine_top}")}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "deleted");
    // Someone else's message: not_author, nothing sent.
    let before = e.fake.writes().len();
    let (s, v) = post("/cli/sew/chat/delete", &agent(WA), json!({"id": format!("msg_{top}")}));
    assert_eq!((s, code(&v)), (403, "not_author"));
    assert_eq!(e.fake.writes().len(), before);
}

#[test]
fn send_and_manage_levels_post_reply_and_dm() {
    let e = env();
    for level in [Level::Send, Level::Manage] {
        hearth(WA, level, ApprovalMode::None);
        let (s, v) = send(&agent(WA), "hearth/general", "hello all");
        assert_eq!(s, 200, "{level:?}: {v}");
        assert!(v["id"].as_str().unwrap().starts_with("msg_"));
        assert_eq!(v["status"], "posted");
        // A DM outside every Space is governed by the default Space (hearth).
        let (s, v) = post("/cli/sew/chat/dm", &agent(WA), json!({"to": "rosson", "text": "psst"}));
        assert_eq!(s, 200, "{level:?}: {v}");
        assert_eq!(v["addr"], "dm/alpha+rosson");
    }
    // Access is per Space: none in eng.
    let (s, v) = send(&agent(WA), "eng/builds", "x");
    assert_eq!((s, code(&v)), (403, "not_granted"), "{v}");
    assert_eq!(e.fake.writes().len(), 4);
}

#[test]
fn a_hidden_target_is_sews_masked_404_before_any_level_check() {
    let e = env();
    hearth(WA, Level::Read, ApprovalMode::None);
    let (s, v) = send(&agent(WA), "hearth/secret", "x");
    assert_eq!((s, code(&v)), (404, "not_found"), "never level_denied for a hidden channel: {v}");
    assert_eq!(v["error"]["hint"], "channel not found", "Sew's hint unchanged");
    let hidden = e.fake.post(&e.rosson, "c-secret", "private", None, &[]);
    let (s, v) = get("/cli/sew/chat/read", &agent(WA), &[("id", &format!("msg_{hidden}"))]);
    assert_eq!((s, code(&v)), (404, "not_found"));
    let (s, v) = post("/cli/sew/chat/dm", &agent(WA), json!({"to": "nobody", "text": "x"}));
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
}

#[test]
fn sew_denials_pass_through_with_teaching() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::None);
    let (s, v) = send(&agent(WA), "hearth/announce", "x");
    assert_eq!((s, code(&v)), (403, "missing_permission"), "{v}");
    assert_eq!(v["error"]["details"]["sewReason"], "SEND_MESSAGES");
    assert!(v["error"]["hint"].as_str().unwrap().contains("Sew roles decide"));
    e.fake.fail_next(SewError::new(403, "not_provisioned", "x"));
    let (s, v) = get("/cli/sew/chat/whoami", &agent(WA), &[]);
    assert_eq!((s, code(&v)), (503, "chat_not_ready"));
    let failed = actions(WA).into_iter().filter(|a| a.status == "failed").count();
    assert_eq!(failed, 1, "the refused post is audited as failed");
}

// ── fake Sew round trips (reads) ─────────────────────────────────────

#[test]
fn reads_round_trip_through_the_fake() {
    let e = env();
    hearth(WA, Level::Reply, ApprovalMode::Required);
    grant(WA, "sp-eng", "eng", Level::Read, ApprovalMode::None);
    let (s, v) = get("/cli/sew/chat/whoami", &agent(WA), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["memberId"].as_str(), v["username"].as_str()), (Some(e.alpha.as_str()), Some("alpha")));
    assert_eq!(v["chatManage"], false);
    let (s, v) = get("/cli/sew/chat/spaces", &agent(WA), &[]);
    assert_eq!(s, 200, "{v}");
    let sp: Vec<(&str, &str)> =
        v["spaces"].as_array().unwrap().iter().map(|x| (x["space"].as_str().unwrap(), x["level"].as_str().unwrap())).collect();
    assert_eq!(sp, vec![("hearth", "reply"), ("eng", "read")]);
    let (s, v) = get("/cli/sew/chat/channels", &agent(WA), &[("space", "hearth")]);
    assert_eq!(s, 200, "{v}");
    let addrs: Vec<&str> = v["channels"].as_array().unwrap().iter().map(|c| c["addr"].as_str().unwrap()).collect();
    assert_eq!(addrs, vec!["hearth/general", "hearth/announce"], "the hidden channel never shows");
    let (s, v) = get("/cli/sew/chat/access", &agent(WA), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["access"][0]["approval"], "none");
    let id = e.fake.post(&e.rosson, "c-general", "hello <@x>", None, &[]);
    let (s, v) = get("/cli/sew/chat/read", &agent(WA), &[("id", &format!("msg_{id}"))]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["message"]["body"], "hello <@x>");
    assert_eq!(v["message"]["author"]["kind"], "person");
    assert!(v["message"].get("kind").is_none(), "KM1 fields only when Sew sent them");
    let (s, v) = get("/cli/sew/chat/messages", &agent(WA), &[("in", "hearth/general"), ("limit", "0")]);
    assert_eq!((s, code(&v)), (400, "usage"));
}

#[test]
fn km1_fields_are_tolerated_but_not_required() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::None);
    let (s, v) = post("/cli/sew/chat/send", &agent(WA), json!({"to": "hearth/general", "text": "x", "kind": "text", "wake": true}));
    assert_eq!(s, 200, "{v}");
    for extra in [json!({"kind": "ask"}), json!({"wake": false}), json!({"respondTo": "msg_1"})] {
        let mut b = json!({"to": "hearth/general", "text": "x"});
        b.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let (s, v) = post("/cli/sew/chat/send", &agent(WA), b);
        assert_eq!((s, code(&v)), (422, "needs_km1"), "{v}");
    }
    assert_eq!(e.fake.writes().len(), 1);
}

#[test]
fn wait_returns_the_first_foreign_message_or_times_out() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::None);
    let first = e.fake.post(&e.rosson, "c-general", "older", None, &[]);
    e.fake.post(&e.alpha, "c-general", "my own", None, &[]);
    e.fake.post(&e.rosson, "c-general", "ping @alpha", None, &[&e.alpha]);
    let (s, v) = get("/cli/sew/chat/wait", &agent(WA), &[("in", "hearth/general"), ("after", &format!("msg_{first}")), ("timeout", "5")]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["timedOut"], false);
    let bodies: Vec<&str> = v["messages"].as_array().unwrap().iter().map(|m| m["body"].as_str().unwrap()).collect();
    assert_eq!(bodies, vec!["ping @alpha"], "own messages are skipped");
    let (s, v) = get("/cli/sew/chat/wait", &agent(WA), &[("in", "hearth/general"), ("timeout", "1")]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["timedOut"], true, "nothing new after the cursor: {v}");
    assert!(e.fake.calls().iter().filter(|c| c.op == "wait").count() >= 1);
    let (s, v) = get("/cli/sew/chat/wait", &agent(WA), &[("mention", "1")]);
    assert_eq!((s, code(&v)), (400, "needs_delivery_hub"));
}

// ── idempotency and limits ───────────────────────────────────────────

#[test]
fn a_key_replays_its_first_result_and_a_lost_response_never_double_posts() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::None);
    let b = json!({"to": "hearth/general", "text": "once", "key": "k-1"});
    let (s, v1) = post("/cli/sew/chat/send", &agent(WA), b.clone());
    assert_eq!(s, 200, "{v1}");
    let (s, v2) = post("/cli/sew/chat/send", &agent(WA), b);
    assert_eq!(s, 200, "{v2}");
    assert_eq!((v2["id"].clone(), v2["replayed"].clone()), (v1["id"].clone(), json!(true)));
    assert_eq!(e.fake.writes().len(), 1, "a replay never reaches Sew");
    let (s, v) = post("/cli/sew/chat/send", &agent(WA), json!({"to": "hearth/general", "text": "different", "key": "k-1"}));
    assert_eq!((s, code(&v)), (409, "idempotency_conflict"));
    // Lost response: Sew applied the post; the retry under the same key
    // is deduped by Sew's Idempotency-Key — one message.
    let n0 = e.fake.message_count();
    e.fake.lose_next_response();
    let b = json!({"to": "hearth/general", "text": "flaky", "key": "k-2"});
    let (s, v) = post("/cli/sew/chat/send", &agent(WA), b.clone());
    assert_eq!((s, code(&v)), (503, "chat_unavailable"), "{v}");
    assert_eq!(v["error"]["details"]["key"], "k-2", "the key is printed for the retry");
    let (s, v) = post("/cli/sew/chat/send", &agent(WA), b);
    assert_eq!(s, 200, "{v}");
    assert_eq!(e.fake.message_count(), n0 + 1, "exactly one message");
    let keys: Vec<Option<String>> = e.fake.writes().iter().map(|w| w.key.clone()).collect();
    assert_eq!(keys[1..], [Some("k2c-k-2".to_string()), Some("k2c-k-2".to_string())]);
}

#[test]
fn content_limit_is_20_per_minute() {
    let _e = env();
    hearth(WA, Level::Send, ApprovalMode::None);
    for i in 0..core::CONTENT_PER_MIN {
        let (s, v) = send(&agent(WA), "hearth/general", &format!("n{i}"));
        assert_eq!(s, 200, "{i}: {v}");
    }
    let (s, v) = send(&agent(WA), "hearth/general", "one too many");
    assert_eq!((s, code(&v)), (429, "rate_limited"), "{v}");
    assert!(v["error"]["details"]["retryAfter"].as_i64().unwrap() >= 1);
    // Another workspace has its own budget.
    hearth(WB, Level::Send, ApprovalMode::None);
    let (s, v) = send(&agent(WB), "hearth/general", "fine");
    assert_eq!(s, 200, "{v}");
}

// ── approval queue ───────────────────────────────────────────────────

#[test]
fn approval_required_queues_with_an_out_id_and_notifies_without_the_body() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::Required);
    let mark = crate::push_routes::test_push_capture::mark();
    let (s, v) = send(&agent(WA), "hearth/general", "TOP SECRET LAUNCH PLAN");
    assert_eq!(s, 200, "queued is still success (exit 0): {v}");
    assert_eq!(v["status"], "queued");
    let id = v["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("out_"), "{id}");
    assert!(e.fake.writes().is_empty(), "nothing posted while queued");
    // Reply and DM queue too; delete does not.
    let top = e.fake.post(&e.rosson, "c-general", "q?", None, &[]);
    let (_, v) = post("/cli/sew/chat/reply", &agent(WA), json!({"to": format!("msg_{top}"), "text": "a"}));
    assert_eq!(v["status"], "queued", "{v}");
    let (_, v) = post("/cli/sew/chat/dm", &agent(WA), json!({"to": "rosson", "text": "b"}));
    assert_eq!(v["status"], "queued", "{v}");
    let mine = e.fake.post(&e.alpha, "c-general", "mine", None, &[]);
    let (_, v) = post("/cli/sew/chat/delete", &agent(WA), json!({"id": format!("msg_{mine}")}));
    assert_eq!(v["status"], "deleted", "a delete is never queued: {v}");
    // The outbox shows it; no body there.
    let (s, v) = get("/cli/sew/chat/outbox", &agent(WA), &[("id", &id)]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["status"], "queued");
    assert!(!v.to_string().contains("TOP SECRET"), "the agent's outbox never echoes bodies");
    // Push: content-free, to the people who may approve.
    let pushes: Vec<_> = crate::push_routes::test_push_capture::since(mark)
        .into_iter()
        .filter(|p| matches!(p, k2_core::push::PushEvent::ChatApprovalRequested { outbox_id, .. } if outbox_id == &id))
        .collect();
    assert_eq!(pushes.len(), 1);
    let p = &pushes[0];
    let all = format!("{} {} {}", p.title(), p.body(), p.data().to_value());
    assert!(!all.contains("TOP SECRET") && !all.contains("general"), "push carries no body or channel: {all}");
    assert_eq!(p.data().get("kind"), Some("chat_approval"));
    assert!(p.target_usernames().unwrap().contains(&"owner".to_string()));
    // Hook: content-free too.
    let hooks = test_hooks::take();
    let req: Vec<_> = hooks.iter().filter(|(n, _)| n == "sew-chat:approval-requested").collect();
    assert_eq!(req.len(), 3);
    assert!(!format!("{hooks:?}").contains("TOP SECRET"));
    assert_eq!(req[0].1["outboxId"], id.as_str());
}

#[test]
fn the_agent_cancels_its_own_queued_post_only() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::Required);
    hearth(WB, Level::Send, ApprovalMode::Required);
    let id = queue_one(&e, "cancel me");
    let (s, v) = post("/cli/sew/chat/outbox/cancel", &agent(WB), json!({"id": id}));
    assert_eq!((s, code(&v)), (404, "not_found"), "another workspace's id is a plain 404");
    let (s, v) = post("/cli/sew/chat/outbox/cancel", &agent(WA), json!({"id": id}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "cancelled");
    assert_eq!(outbox_row(&id).body, None, "body gone");
    let (s, v) = post("/cli/sew/chat/outbox/cancel", &agent(WA), json!({"id": id}));
    assert_eq!((s, code(&v)), (409, "not_queued"));
    let (s, v) = get("/cli/sew/chat/wait", &agent(WA), &[("outbox", &id), ("timeout", "2")]);
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["timedOut"].clone(), v["item"]["status"].clone()), (json!(false), json!("cancelled")));
    assert!(e.fake.writes().is_empty());
}

#[test]
fn a_person_approves_with_a_single_use_confirm_bound_to_the_item() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::Required);
    let id = queue_one(&e, "ship it");
    let other = queue_one(&e, "and this");
    let approve = |who: &Who, id: &str, tok: Option<&str>| {
        let mut b = json!({"id": id});
        if let Some(t) = tok {
            b["confirmToken"] = t.into();
        }
        post("/cli/sew/approvals/approve", who, b)
    };
    let (s, v) = approve(&Who::Owner, &id, None);
    assert_eq!((s, code(&v)), (403, "confirm_required"), "{v}");
    assert_eq!((v["error"]["details"]["action"].clone(), v["error"]["details"]["secretId"].clone()), (json!("chat-approve"), json!(id)));
    let wrong_item = mint("chat-approve", &other, "owner");
    let (s, v) = approve(&Who::Owner, &id, Some(&wrong_item));
    assert_eq!((s, code(&v)), (403, "confirm_required"), "a confirm is bound to its item: {v}");
    let wrong_person = mint("chat-approve", &id, "connect:ada");
    let (s, v) = approve(&Who::Owner, &id, Some(&wrong_person));
    assert_eq!((s, code(&v)), (403, "confirm_required"), "and to its person: {v}");
    assert!(e.fake.writes().is_empty());
    let tok = mint("chat-approve", &id, "owner");
    let (s, v) = approve(&Who::Owner, &id, Some(&tok));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["status"].as_str(), v["delivery"].as_str(), v["decidedKind"].as_str()), (Some("approved"), Some("posted"), Some("person")));
    assert_eq!(v["confirmMethod"], "vault-password");
    let w = e.fake.writes();
    assert_eq!(w.len(), 1, "exactly one post");
    assert_eq!(w[0].external_id, format!("k2:workspace:{WA}"), "posted AS the agent");
    assert_eq!(w[0].actor.as_deref(), Some("person:owner via vault-password"), "Sew-Actor names the approver");
    assert_eq!(e.fake.body_of(v["messageId"].as_str().unwrap().trim_start_matches("msg_")).as_deref(), Some("ship it"));
    assert_eq!(outbox_row(&id).body, None, "no chat text kept after a decision");
    // Single use: the same token can't approve again (and the item is decided).
    let (s, v) = approve(&Who::Owner, &id, Some(&tok));
    assert_eq!((s, code(&v)), (409, "not_queued"), "{v}");
    let (s, v) = approve(&Who::Owner, &other, Some(&tok));
    assert_eq!((s, code(&v)), (403, "confirm_required"), "a spent token is refused: {v}");
    // An admin login approves with their own confirm.
    let tok = mint("chat-approve", &other, "connect:ada");
    let (s, v) = approve(&admin(), &other, Some(&tok));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["decidedBy"], "login:ada");
    // A member login never decides (the route floor stops it on TCP; the
    // handler refuses too).
    let (s, v) = get("/cli/sew/approvals", &Who::Login { username: "mo".into(), role: Role::Member }, &[]);
    assert_eq!((s, code(&v)), (403, "owner_only"));
    let decided: Vec<_> = test_hooks::take().into_iter().filter(|(n, _)| n == "sew-chat:approval-decided").collect();
    assert_eq!(decided.len(), 2);
}

#[test]
fn agents_approve_only_when_the_box_allows_and_manage_sew_is_on_never_their_own() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::Required);
    let id = queue_one(&e, "agent-approved post");
    // Policy people (default): refused even with Manage Sew.
    set_manage(WB, true);
    let (s, v) = post("/cli/sew/approvals/approve", &agent(WB), json!({"id": id}));
    assert_eq!((s, code(&v)), (403, "approvals_people_only"), "{v}");
    set_policy(core::ApprovalsBy::PeopleAndAgents);
    // Manage Sew off: refused.
    set_manage(WB, false);
    let (s, v) = post("/cli/sew/approvals/approve", &agent(WB), json!({"id": id}));
    assert_eq!((s, code(&v)), (403, "chat_manage_off"), "{v}");
    let (s, v) = get("/cli/sew/approvals", &agent(WC), &[]);
    assert_eq!((s, code(&v)), (403, "chat_manage_off"), "{v}");
    // Its own item: refused (it cancels instead).
    set_manage(WA, true);
    let (s, v) = post("/cli/sew/approvals/approve", &agent(WA), json!({"id": id}));
    assert_eq!((s, code(&v)), (403, "own_item"), "{v}");
    let (s, v) = get("/cli/sew/approvals", &agent(WA), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["canApprove"], false);
    assert!(e.fake.writes().is_empty());
    // Allowed: recorded and shown as an AGENT's approval, never a person's.
    set_manage(WB, true);
    let (s, v) = get("/cli/sew/approvals", &agent(WB), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["items"][0]["canApprove"].clone(), v["items"][0]["preview"].clone()), (json!(true), json!("agent-approved post")));
    let (s, v) = post("/cli/sew/approvals/approve", &agent(WB), json!({"id": id}));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["decidedKind"].as_str(), v["decidedBy"].as_str()), (Some("agent"), Some("agent:beta")));
    assert_eq!(v["confirmMethod"], Value::Null, "no person proof");
    let w = e.fake.writes();
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].actor.as_deref(), Some("agent:beta"), "Sew sees an agent approver");
    let r = outbox_row(&id);
    assert_eq!((r.decided_kind.as_deref(), r.confirm_method.as_deref()), (Some("agent"), None));
}

#[test]
fn deny_needs_a_note_and_the_agent_sees_it() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::Required);
    let id = queue_one(&e, "nope");
    let (s, v) = post("/cli/sew/approvals/deny", &Who::Owner, json!({"id": id}));
    assert_eq!((s, code(&v)), (400, "usage"), "{v}");
    let (s, v) = post("/cli/sew/approvals/deny", &Who::Owner, json!({"id": id, "note": "  "}));
    assert_eq!((s, code(&v)), (400, "usage"), "{v}");
    // An agent with Manage Sew may deny others' items, not its own.
    set_manage(WA, true);
    let (s, v) = post("/cli/sew/approvals/deny", &agent(WA), json!({"id": id, "note": "x"}));
    assert_eq!((s, code(&v)), (403, "own_item"));
    let (s, v) = post("/cli/sew/approvals/deny", &Who::Owner, json!({"id": id, "note": "wrong channel"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "denied");
    let (_, v) = get("/cli/sew/chat/outbox", &agent(WA), &[("id", &id)]);
    assert_eq!((v["items"][0]["status"].clone(), v["items"][0]["note"].clone()), (json!("denied"), json!("wrong channel")));
    assert_eq!(outbox_row(&id).body, None);
    assert!(e.fake.writes().is_empty());
    // A Manage-Sew agent may deny another agent's item whatever the policy.
    hearth(WC, Level::Send, ApprovalMode::Required);
    let (_, v) = send(&agent(WC), "hearth/general", "from gamma");
    let gid = v["id"].as_str().unwrap().to_string();
    let (s, v) = post("/cli/sew/approvals/deny", &agent(WA), json!({"id": gid, "note": "off topic"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["decidedKind"], "agent");
}

#[test]
fn queued_posts_expire_after_24h_into_a_denial() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::Required);
    let id = queue_one(&e, "stale");
    test_clock::set(core::APPROVAL_TTL_SECS + 5);
    let tok = mint("chat-approve", &id, "owner");
    let (s, v) = post("/cli/sew/approvals/approve", &Who::Owner, json!({"id": id, "confirmToken": tok}));
    assert_eq!((s, code(&v)), (410, "expired"), "{v}");
    let r = outbox_row(&id);
    assert_eq!((r.status.as_str(), r.decided_by.as_deref(), r.body.as_deref()), ("denied", Some("system:expired"), None));
    assert!(e.fake.writes().is_empty());
    let decided: Vec<_> = test_hooks::take().into_iter().filter(|(n, _)| n == "sew-chat:approval-decided").collect();
    assert_eq!(decided.len(), 1, "expiry announces the decision once");
    assert_eq!((decided[0].1["status"].clone(), decided[0].1["decidedKind"].clone()), (json!("denied"), json!("system")));
}

#[test]
fn a_batch_approval_takes_one_confirm_bound_to_the_exact_list() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::Required);
    let a = queue_one(&e, "one");
    let b = queue_one(&e, "two");
    let ids = vec![a.clone(), b.clone()];
    let (s, v) = post("/cli/sew/approvals/approve-batch", &Who::Owner, json!({"ids": ids}));
    assert_eq!((s, code(&v)), (403, "confirm_required"));
    assert_eq!(v["error"]["details"]["secretId"], core::batch_hash(&ids).as_str());
    let narrow = mint("chat-approve-batch", &core::batch_hash(&[a.clone()]), "owner");
    let (s, v) = post("/cli/sew/approvals/approve-batch", &Who::Owner, json!({"ids": ids, "confirmToken": narrow}));
    assert_eq!((s, code(&v)), (403, "confirm_required"), "{v}");
    let single = mint("chat-approve", &a, "owner");
    let (s, _) = post("/cli/sew/approvals/approve-batch", &Who::Owner, json!({"ids": ids, "confirmToken": single}));
    assert_eq!(s, 403, "a single-item confirm can't approve a batch");
    assert!(e.fake.writes().is_empty());
    let tok = mint("chat-approve-batch", &core::batch_hash(&ids), "owner");
    let (s, v) = post("/cli/sew/approvals/approve-batch", &Who::Owner, json!({"ids": [b.clone(), a.clone()], "confirmToken": tok}));
    assert_eq!(s, 200, "{v}");
    let st: Vec<&str> = v["results"].as_array().unwrap().iter().map(|r| r["delivery"].as_str().unwrap()).collect();
    assert_eq!(st, vec!["posted", "posted"]);
    assert_eq!(e.fake.writes().len(), 2);
}

#[test]
fn only_a_person_changes_who_may_approve() {
    let _e = env();
    set_manage(WA, true);
    let (s, v) = post("/cli/sew/approvals/policy/set", &agent(WA), json!({"approvalsBy": "people+agents"}));
    assert_eq!((s, code(&v)), (403, "owner_only"), "an agent never widens who approves");
    let (s, v) = get("/cli/sew/approvals/policy", &agent(WA), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["approvalsBy"].clone(), v["canChange"].clone()), (json!("people"), json!(false)));
    let (s, v) = post("/cli/sew/approvals/policy/set", &Who::Owner, json!({"approvalsBy": "agents"}));
    assert_eq!((s, code(&v)), (400, "usage"));
    let (s, v) = post("/cli/sew/approvals/policy/set", &admin(), json!({"approvalsBy": "people+agents"}));
    assert_eq!(s, 200, "{v}");
    let (_, v) = get("/cli/sew/approvals/policy", &Who::Owner, &[]);
    assert_eq!(v["approvalsBy"], "people+agents");
}

#[test]
fn the_queue_holds_at_most_20_per_workspace() {
    let _e = env();
    hearth(WA, Level::Send, ApprovalMode::Required);
    for i in 0..core::QUEUE_CAP {
        // Stay under the 20/min content limit: half now, half 2 min later.
        if i == core::QUEUE_CAP / 2 {
            test_clock::set(120);
        }
        let (s, v) = send(&agent(WA), "hearth/general", &format!("q{i}"));
        assert_eq!((s, v["status"].as_str()), (200, Some("queued")), "{i}: {v}");
    }
    let (s, v) = send(&agent(WA), "hearth/general", "overflow");
    assert_eq!((s, code(&v)), (429, "queue_full"), "{v}");
}

#[test]
fn a_failed_delivery_after_approval_is_recorded_not_retried_silently() {
    let e = env();
    hearth(WA, Level::Send, ApprovalMode::Required);
    let id = queue_one(&e, "will fail");
    e.fake.fail_next(SewError::new(503, "unavailable", "down"));
    let tok = mint("chat-approve", &id, "owner");
    let (s, v) = post("/cli/sew/approvals/approve", &Who::Owner, json!({"id": id, "confirmToken": tok}));
    assert_eq!((s, code(&v)), (503, "chat_unavailable"), "{v}");
    assert_eq!(v["error"]["details"]["delivery"], "failed");
    let r = outbox_row(&id);
    assert_eq!((r.status.as_str(), r.delivery.as_deref(), r.body.as_deref()), ("approved", Some("failed"), None));
}

#[test]
fn the_manage_sew_switch_defaults_off_and_only_owner_or_admin_sets_it() {
    let _e = env();
    assert!(!k2_core::workspace::settings::agents_can_manage_chat(WA));
    let body = json!({"workspace": WA, "toggle": "chat_manage", "value": true}).to_string();
    let r = crate::sidecar_routes::handle(
        "/cli/agent-access/set",
        crate::sidecar_routes::Caller::Agent(crate::sidecar_routes::AgentCaller {
            project_id: WA.into(),
            map_key: None,
            args: vec![],
            sandboxed: false,
        }),
        body.as_bytes(),
        &HashMap::new(),
        "cell",
    );
    assert_eq!(status_code(r.status), 403, "{}", r.body);
    assert!(!k2_core::workspace::settings::agents_can_manage_chat(WA));
    let r = crate::sidecar_routes::handle("/cli/agent-access/set", crate::sidecar_routes::Caller::Owner, body.as_bytes(), &HashMap::new(), "tcp");
    assert_eq!(status_code(r.status), 200, "{}", r.body);
    assert!(k2_core::workspace::settings::agents_can_manage_chat(WA));
}
