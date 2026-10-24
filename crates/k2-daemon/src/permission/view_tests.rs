//! `project()` against the phone contract's own examples (PRD rev 3.1
//! §17.11; `phone-contract-v1.md`), plus the fallback, the live check and
//! the sweep's `already_done` republish.

use super::*;
use k2_core::permission::card::Viewer;
use serde_json::json;

fn rosson() -> Viewer {
    Viewer::Login { user: "rosson".into(), role: "owner".into() }
}
fn maya_admin() -> Viewer {
    Viewer::Login { user: "maya".into(), role: "admin".into() }
}
fn dana() -> Viewer {
    Viewer::Login { user: "dana".into(), role: "member".into() }
}
fn lee() -> Viewer {
    Viewer::Login { user: "lee".into(), role: "member".into() }
}
fn billing_passport() -> Viewer {
    Viewer::Passport { workspace_id: "ws-billing".into() }
}

fn doc(v: Value) -> OverlayDoc {
    serde_json::from_value(v).expect("a Thread doc")
}

fn wire(d: &OverlayDoc) -> Value {
    serde_json::to_value(d).expect("serializes")
}

/// §17.11.1, exactly: the approver's pending card (stored with rosson's
/// viewer, as A1 may write it).
fn approver_pending() -> Value {
    serde_json::from_str(r#"{"id":"tc_7f3a2c90","kind":"choice","from":"billing","to":"billing","created_at":1761060000,"via":"permission",
     "choice":{"prompt":"billing asks: Let billing run jobs on this server's compute nodes (Allow compute nodes).",
      "options":[{"label":"Allow"},{"label":"Deny"}],"allow_custom":false,"status":"pending","expiresAt":1761319200,
      "action":{"id":"permission","params":{"request":"pr_7f3a2c"},"dedupe":"compute.use:9b1e40d2",
       "view":{"v":1,"kind":"permission","requestId":"pr_7f3a2c","state":"pending",
        "viewer":{"login":"rosson","role":"owner","via":"login","mode":"approve","canAnswer":true,"isAsker":false},
        "action":"compute.use","setting":"Allow compute nodes",
        "whatChanges":"Let billing run jobs on this server's compute nodes.",
        "direction":"loosen","grant":"standing","scope":"billing","duration":"until someone turns it off",
        "consequences":"Every active machine; default limits; jobs see only the code billing sends.",
        "values":[{"key":"workspace","value":"billing"}],
        "why":{"by":"billing","text":"Run the test suite on k2n2-m1; it takes 40 min here"},
        "notes":[{"by":"dana","role":"member","text":"I need this for the payroll run","at":1761060300}],
        "askedFor":"dana","askers":null,
        "approvers":{"floor":"admin","label":"owner or admin","names":["rosson","maya"]},
        "proof":{"kind":"click"},"expiresAt":1761319200,"settingsPage":"workspaces/billing/agent",
        "noteBox":{"allowed":false,"max":500,"mine":null},"answer":null,"undo":null}}}}"#).expect("sample JSON")
}

/// §17.11.2, exactly.
fn approver_approved() -> Value {
    serde_json::from_str(r#"{"id":"tc_7f3a2c90","kind":"choice","from":"billing","to":"billing","created_at":1761060000,"via":"permission",
     "choice":{"prompt":"billing asks: Let billing run jobs on this server's compute nodes (Allow compute nodes).",
      "options":[{"label":"Allow"},{"label":"Deny"}],"allow_custom":false,"status":"answered","answer":"Allow",
      "answeredBy":{"kind":"user","login":"rosson"},"answeredAt":1761061200,"expiresAt":1761319200,
      "action":{"id":"permission","params":{"request":"pr_7f3a2c"},"dedupe":"compute.use:9b1e40d2",
       "result":{"message":"Allowed. Turn it off again in Settings → Workspaces → billing → Agent.","outcome":"approved"},
       "view":{"v":1,"kind":"permission","requestId":"pr_7f3a2c","state":"approved",
        "viewer":{"login":"rosson","role":"owner","via":"login","mode":"approve","canAnswer":true,"isAsker":false},
        "action":"compute.use","setting":"Allow compute nodes",
        "whatChanges":"Let billing run jobs on this server's compute nodes.",
        "direction":"loosen","grant":"standing","scope":"billing","duration":"until someone turns it off",
        "consequences":"Every active machine; default limits; jobs see only the code billing sends.",
        "values":[{"key":"workspace","value":"billing"}],
        "why":{"by":"billing","text":"Run the test suite on k2n2-m1; it takes 40 min here"},
        "notes":[{"by":"dana","role":"member","text":"I need this for the payroll run","at":1761060300}],
        "askedFor":"dana","askers":null,
        "approvers":{"floor":"admin","label":"owner or admin","names":["rosson","maya"]},
        "proof":{"kind":"click"},"expiresAt":1761319200,"settingsPage":"workspaces/billing/agent",
        "noteBox":{"allowed":false,"max":500,"mine":null},
        "answer":{"decision":"allow","by":"rosson","via":"phone","at":1761061200,"reason":null},
        "undo":{"label":"Turn off again","until":1761666000}}}}}"#).expect("sample JSON")
}

/// §17.11.5: the asker's pending view (dana, member).
fn asker_pending() -> Value {
    let mut v = approver_pending();
    v["choice"]["options"] = json!([]);
    v["choice"]["action"]["view"]["viewer"] =
        json!({"login":"dana","role":"member","via":"login","mode":"wait","canAnswer":false,"isAsker":true});
    v["choice"]["action"]["view"]["noteBox"] = json!({"allowed":true,"max":500,"mine":"I need this for the payroll run"});
    v
}

#[test]
fn every_other_doc_is_the_identity() {
    let text = OverlayDoc::text("t1".into(), "billing".into(), "billing".into(), "hello".into(), "thread");
    let keep = doc(json!({"id":"kc1","kind":"choice","from":"pog","to":"pog","created_at":1,"via":"card",
        "choice":{"prompt":"Keep it as yours?","options":[{"label":"Keep as mine"}],"allow_custom":false,"status":"pending",
          "action":{"id":"zen.widget.keep","params":{"widget":"kanban"},"dedupe":"kanban"}}}));
    for d in [&text, &keep] {
        assert!(!is_permission_card(d));
        for v in [rosson(), dana(), billing_passport(), Viewer::OwnerToken, Viewer::Guest { app: None }] {
            for cards in [0, 2] {
                assert_eq!(wire(&project(d, &v, cards)), wire(d), "identity for {} ({v:?}, cards={cards})", d.id);
                assert!(matches!(Seen::new(v.clone(), cards).doc(d), Cow::Borrowed(_)), "never copied");
            }
        }
    }
}

#[test]
fn the_approver_gets_the_contract_view() {
    let stored = doc(approver_pending());
    assert_eq!(wire(&project(&stored, &rosson(), 2)), approver_pending(), "§17.11.1");
    // Stored for another viewer: rosson still gets exactly his own view.
    let for_dana = doc(asker_pending());
    assert_eq!(wire(&project(&for_dana, &rosson(), 2)), approver_pending(), "the viewer is always recomputed");
    // An Admin meets an admin floor.
    let maya = wire(&project(&stored, &maya_admin(), 2));
    assert_eq!(maya["choice"]["action"]["view"]["viewer"],
        json!({"login":"maya","role":"admin","via":"login","mode":"approve","canAnswer":true,"isAsker":false}));
    assert_eq!(maya["choice"]["options"], json!([{"label":"Allow"},{"label":"Deny"}]));
    // The owner token.
    let ot = wire(&project(&stored, &Viewer::OwnerToken, 2));
    assert_eq!(ot["choice"]["action"]["view"]["viewer"],
        json!({"login":"owner","role":"owner","via":"owner-token","mode":"approve","canAnswer":true,"isAsker":false}));
}

#[test]
fn the_asker_and_other_members_wait_with_a_note_box() {
    let stored = doc(approver_pending());
    assert_eq!(wire(&project(&stored, &dana(), 2)), asker_pending(), "§17.11.5");
    let mut lee_want = asker_pending();
    lee_want["choice"]["action"]["view"]["viewer"] =
        json!({"login":"lee","role":"member","via":"login","mode":"wait","canAnswer":false,"isAsker":false});
    lee_want["choice"]["action"]["view"]["noteBox"] = json!({"allowed":true,"max":500,"mine":null});
    assert_eq!(wire(&project(&stored, &lee(), 2)), lee_want, "§17.11.7");
    // An Admin on an owner-floor card waits too.
    let mut owner_floor = approver_pending();
    owner_floor["choice"]["action"]["view"]["approvers"] = json!({"floor":"owner","label":"owner","names":["rosson"]});
    let maya = wire(&project(&doc(owner_floor), &maya_admin(), 2));
    assert_eq!(maya["choice"]["action"]["view"]["viewer"]["mode"], "wait");
    assert_eq!(maya["choice"]["options"], json!([]));
}

#[test]
fn approved_asker_sees_who_allowed_it_and_no_undo() {
    let stored = doc(approver_approved());
    assert_eq!(wire(&project(&stored, &rosson(), 2)), approver_approved(), "§17.11.2");
    let mut want = approver_approved();
    want["choice"]["options"] = json!([]);
    want["choice"]["action"]["result"] = json!({"message":"Allowed by rosson.","outcome":"approved"});
    want["choice"]["action"]["view"]["viewer"] =
        json!({"login":"dana","role":"member","via":"login","mode":"wait","canAnswer":false,"isAsker":true});
    want["choice"]["action"]["view"]["noteBox"] = json!({"allowed":false,"max":500,"mine":"I need this for the payroll run"});
    want["choice"]["action"]["view"]["undo"] = Value::Null;
    assert_eq!(wire(&project(&stored, &dana(), 2)), want, "§17.11.6");
}

#[test]
fn passports_and_old_clients_get_the_fallback() {
    let stored = doc(approver_pending());
    // §17.11.10: an agent passport, whatever it sends.
    let want_agent = json!({"id":"tc_7f3a2c90","kind":"text","from":"billing","to":"billing","created_at":1761060000,"via":"permission",
        "body":"billing asked an admin to turn on Allow compute nodes."});
    for cards in [0, 2] {
        assert_eq!(wire(&project(&stored, &billing_passport(), cards)), want_agent, "cards={cards}");
    }
    // §17.11.11: the asker on an old build.
    assert_eq!(
        wire(&project(&stored, &dana(), 0)),
        json!({"id":"tc_7f3a2c90","kind":"text","from":"billing","to":"billing","created_at":1761060000,"via":"permission",
            "body":"Asked an admin to turn on Allow compute nodes for you. You can keep working with billing meanwhile."})
    );
    // The approver on an old build: a plain choice card, K2's line plus the
    // agent's words and the notes, `action` reduced to {id, params}.
    let a = wire(&project(&stored, &rosson(), 0));
    assert_eq!(a["kind"], "choice");
    assert_eq!(a["choice"]["options"], json!([{"label":"Allow"},{"label":"Deny"}]));
    assert_eq!(a["choice"]["action"], json!({"id":"permission","params":{"request":"pr_7f3a2c"}}), "{a}");
    assert_eq!(
        a["choice"]["prompt"],
        "billing asks: Let billing run jobs on this server's compute nodes (Allow compute nodes).\n\
         billing says: \"Run the test suite on k2n2-m1; it takes 40 min here\"\n\
         dana (member): \"I need this for the payroll run\""
    );
    // A confirm action is text on an old build.
    let mut confirm = approver_pending();
    confirm["choice"]["action"]["view"]["proof"] = json!({"kind":"confirm","action":"app-approve","secretId":"ar_22"});
    let c = wire(&project(&doc(confirm), &rosson(), 0));
    assert_eq!(c["kind"], "text");
    assert!(c.get("choice").is_none(), "{c}");
    assert!(c["body"].as_str().unwrap().ends_with("Approve on your desktop, or update K2 Companion."), "{c}");
    // Closed states are one text line.
    let ap = wire(&project(&doc(approver_approved()), &dana(), 0));
    assert_eq!(ap["body"], "Allowed by rosson: Let billing run jobs on this server's compute nodes.");
}

/// An answered card of either direction: `on` = the §17.11.2 compute card
/// (loosen), `off` = `db.agent-access-off` (tighten), whose Settings label
/// still reads "Allow this agent to create databases".
fn answered(off: bool, deny: Option<Option<&str>>) -> Value {
    let mut v = approver_approved();
    if off {
        v["from"] = json!("k2");
        v["to"] = json!("k2");
        let view = &mut v["choice"]["action"]["view"];
        view["action"] = json!("db.agent-access-off");
        view["setting"] = json!("Allow this agent to create databases");
        view["whatChanges"] = json!("Stop k2 creating new databases.");
        view["direction"] = json!("tighten");
        view["scope"] = json!("k2");
        view["why"] = json!({"by":"k2","text":"Lock it down for the audit"});
        view["askedFor"] = Value::Null;
        view["notes"] = json!([]);
        view["undo"] = Value::Null;
    }
    if let Some(reason) = deny {
        v["choice"]["answer"] = json!("Deny");
        v["choice"]["action"]["result"] = json!({"message":"Denied.","outcome":"denied"});
        let view = &mut v["choice"]["action"]["view"];
        view["state"] = json!("denied");
        view["answer"] = json!({"decision":"deny","by":"rosson","via":"phone","at":1761061200,"reason":reason});
        view["undo"] = Value::Null;
    }
    v
}

/// Phone test 2026-10-24: the Thread text after an answer named the
/// SETTING, so an `-off` card read "Allowed by z3thon: Allow this agent to
/// create databases for k2." when databases were turned OFF. The done line
/// now names the action (`whatChanges`), for both directions and both
/// answers, for every fallback viewer (old build, agent passport).
#[test]
fn done_line_names_the_action_for_on_and_off_cards() {
    let cases: [(bool, Option<Option<&str>>, &str); 6] = [
        (false, None, "Allowed by rosson: Let billing run jobs on this server's compute nodes."),
        (false, Some(None), "Denied by rosson: Let billing run jobs on this server's compute nodes."),
        (
            false,
            Some(Some("Not this week")),
            "Denied by rosson: Let billing run jobs on this server's compute nodes. Reason: Not this week",
        ),
        (true, None, "Allowed by rosson: Stop k2 creating new databases."),
        (true, Some(None), "Denied by rosson: Stop k2 creating new databases."),
        (true, Some(Some("Keep it on")), "Denied by rosson: Stop k2 creating new databases. Reason: Keep it on"),
    ];
    for (off, deny, want) in cases {
        let stored = doc(answered(off, deny));
        let passport = if off { Viewer::Passport { workspace_id: "ws-k2".into() } } else { billing_passport() };
        for (who, viewer) in [("old build", dana()), ("old build approver", rosson()), ("passport", passport)] {
            let got = wire(&project(&stored, &viewer, 0));
            assert_eq!(got["kind"], "text", "off={off} deny={deny:?} {who}: {got}");
            assert_eq!(got["body"], want, "off={off} deny={deny:?} {who}");
        }
        if off {
            let body = wire(&project(&stored, &dana(), 0))["body"].as_str().expect("body").to_string();
            assert!(!body.contains("Allow this agent"), "an -off card never names the on-setting: {body}");
        }
    }
}

/// A card whose stored view has no `whatChanges` sentence still gets a
/// done line: the old setting-for-agent wording, with one stop.
#[test]
fn done_line_falls_back_to_the_setting_without_a_sentence() {
    let mut v = approver_approved();
    v["choice"]["action"]["view"]["whatChanges"] = json!("  ");
    let got = wire(&project(&doc(v), &dana(), 0));
    assert_eq!(got["body"], "Allowed by rosson: Allow compute nodes for billing.");
}

#[test]
fn id_seq_and_guest_safety() {
    let stored = doc(approver_pending());
    let item = OverlayItem { collection: "thread".into(), seq: 41, id: stored.id.clone(), doc: stored.clone(), conversation_id: Some("c".into()) };
    for v in [rosson(), dana(), billing_passport()] {
        for cards in [0, 2] {
            let it = Seen::new(v.clone(), cards).item(&item).into_owned();
            assert_eq!((it.seq, it.id.as_str(), it.doc.id.as_str()), (41, "tc_7f3a2c90", "tc_7f3a2c90"));
        }
    }
    // A guest never reaches project(); if one did it learns nothing.
    let g = wire(&project(&stored, &Viewer::Guest { app: None }, 2));
    let s = g.to_string();
    for needle in ["Allow compute nodes", "pr_7f3a2c", "billing asks", "permission\"", "choice"] {
        if needle == "permission\"" {
            // `via:"permission"` is the doc's own label; nothing else.
            continue;
        }
        assert!(!s.contains(needle), "a guest saw {needle:?}: {s}");
    }
    assert_eq!(g["body"], "");
    // cards=2 is for people only.
    assert_eq!(Seen::new(billing_passport(), 2).cards, 0);
    assert_eq!(Seen::new(Viewer::Guest { app: None }, 2).cards, 0);
    assert_eq!(Seen::new(rosson(), 2).cards, 2);
    assert_eq!(Seen::new(rosson(), 1).cards, 0);
}

#[test]
fn after_close_the_card_stays_already_done_and_closed_cards_never_show_buttons() {
    let mut v = approver_pending();
    v["choice"]["status"] = json!("answered");
    v["choice"]["answer"] = json!("Close");
    v["choice"]["action"]["view"]["state"] = json!("already_done");
    v["choice"]["action"]["result"] = json!({"message":"Already on. Nothing was applied.","outcome":"already_done"});
    let a = wire(&project(&doc(v), &rosson(), 2));
    assert_eq!(a["choice"]["options"], json!([{"label":"Close"}]));
    assert_eq!(a["choice"]["action"]["view"]["state"], "already_done");
    // A card that expired before its view was rewritten.
    let mut e = approver_pending();
    e["choice"]["status"] = json!("expired");
    let x = wire(&project(&doc(e.clone()), &rosson(), 2));
    assert_eq!(x["choice"]["action"]["view"]["state"], "expired");
    assert_eq!(wire(&project(&doc(e), &dana(), 0))["body"], "Expired unanswered: Allow compute nodes for billing.");
}

fn batch_doc(confirm_item: bool) -> OverlayDoc {
    let third = if confirm_item {
        json!({"requestId":"ar_23","state":"pending","action":"apps.approve","setting":"App Approvals","whatChanges":"Let billing's App reach api.stripe.com.","proof":{"kind":"confirm","action":"app-approve","secretId":"ar_23"}})
    } else {
        json!({"requestId":"pr_a3","state":"pending","action":"chat.manage","setting":"Sew chat","whatChanges":"Let billing manage Sew chat.","proof":{"kind":"click"}})
    };
    let mut items = vec![
        json!({"requestId":"pr_a1","state":"pending","action":"apps.build","setting":"Build and run Apps","whatChanges":"Let billing build and run Apps.","proof":{"kind":"click"}}),
        json!({"requestId":"pr_a2","state":"pending","action":"apps.core","setting":"App cores","whatChanges":"Let billing's Apps use the Database core.","proof":{"kind":"click"}}),
        third,
    ];
    for it in items.iter_mut() {
        for (k, val) in [
            ("direction", json!("loosen")), ("grant", json!("standing")), ("scope", json!("billing")),
            ("duration", json!("until someone turns it off")), ("consequences", json!("c")),
            ("values", json!([])), ("why", json!({"by":"billing","text":"need it"})), ("notes", json!([])),
            ("askedFor", Value::Null), ("askers", Value::Null),
            ("approvers", json!({"floor":"owner","label":"owner","names":["rosson"]})), ("expiresAt", json!(1761319200)),
            ("settingsPage", json!("workspaces/billing/agent")), ("noteBox", json!({"allowed":false,"max":500,"mine":null})),
            ("answer", Value::Null), ("undo", Value::Null),
        ] {
            it[k] = val;
        }
    }
    doc(json!({"id":"tc_b4a1c0de","kind":"choice","from":"billing","to":"billing","created_at":1761060000,"via":"permission",
     "choice":{"prompt":"billing asks for 3 permissions.","options":[{"label":"Allow all"},{"label":"Deny all"}],
      "allow_custom":false,"status":"pending","expiresAt":1761319200,
      "action":{"id":"permission.batch","params":{"requests":["pr_a1","pr_a2","ar_23"]},"dedupe":"batch:sess_9:owner",
       "view":{"v":1,"kind":"batch","state":"pending",
        "viewer":{"login":"x","role":"member","via":"login","mode":"wait","canAnswer":false,"canAnswerAll":false,"isAsker":false},
        "approvers":{"floor":"owner","label":"owner","names":["rosson"]},"expiresAt":1761319200,"items":items}}}}))
}

#[test]
fn batches_answer_all_only_without_confirms() {
    let b = wire(&project(&batch_doc(true), &rosson(), 2));
    assert_eq!(b["choice"]["action"]["view"]["viewer"],
        json!({"login":"rosson","role":"owner","via":"login","mode":"approve","canAnswer":true,"canAnswerAll":false,"isAsker":false}), "§17.11.12");
    assert_eq!(b["choice"]["options"], json!([{"label":"Allow all"},{"label":"Deny all"}]));
    assert_eq!(b["choice"]["action"]["view"]["state"], "pending");
    let b = wire(&project(&batch_doc(false), &rosson(), 2));
    assert_eq!(b["choice"]["action"]["view"]["viewer"]["canAnswerAll"], true);
    // A member waits on a batch.
    let m = wire(&project(&batch_doc(false), &dana(), 2));
    assert_eq!((m["choice"]["action"]["view"]["viewer"]["mode"].clone(), m["choice"]["options"].clone()), (json!("wait"), json!([])));
    assert_eq!(m["choice"]["action"]["view"]["items"][0]["noteBox"]["allowed"], true);
    // Old approver: Allow all only when no item needs a confirm.
    let old = wire(&project(&batch_doc(false), &rosson(), 0));
    assert_eq!(old["kind"], "choice");
    assert_eq!(old["choice"]["action"], json!({"id":"permission.batch","params":{"requests":["pr_a1","pr_a2","ar_23"]}}));
    let old = wire(&project(&batch_doc(true), &rosson(), 0));
    assert_eq!(old["kind"], "text");
    assert!(old["body"].as_str().unwrap().contains("- App Approvals: waiting"), "{old}");
    let agent = wire(&project(&batch_doc(true), &billing_passport(), 2));
    assert_eq!(agent["kind"], "text");
    assert!(agent["body"].as_str().unwrap().starts_with("billing asked an owner for 3 permissions:"), "{agent}");
}

#[test]
fn cards_opt_in_parsing() {
    let mut q = HashMap::new();
    assert_eq!(cards_param(&q), 0);
    q.insert("cards".to_string(), "2".to_string());
    assert_eq!(cards_param(&q), 2);
    q.insert("cards".to_string(), "two".to_string());
    assert_eq!(cards_param(&q), 0);
    assert_eq!(cards_body(Some(&json!(2))), 2);
    assert_eq!(cards_body(Some(&json!("2"))), 0, "a string is ignored, as an unknown field was");
    assert_eq!(cards_body(None), 0);
}

// ── on A1's real requests (the test kit's actions) ──────────────────

use super::super::routes::{self as proutes, handle_gated};
use super::super::testkit;
use crate::compute_routes::Who;
use k2_core::db::schema::WorkspaceSession;

struct Ws {
    id: String,
    handle: String,
    conv: String,
}

fn seed() -> Ws {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("pv{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        rusqlite::params![id, handle, format!("/tmp/pc-a3-{handle}")],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    Ws { id, handle, conv }
}

/// The agent asks for `test.switch` (A1's request route, gate passed):
/// the request id and its card id.
fn ask(ws: &Ws) -> (String, String) {
    let who = Who::Agent { project_id: ws.id.clone(), session_id: format!("sess-{}", &ws.id[..8]) };
    let body = json!({ "action": "test.switch", "why": "Run the suite on the minis" });
    let r = handle_gated(proutes::REQUEST, &who, true, body.to_string().as_bytes(), &HashMap::new());
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body));
    assert_eq!((r.status, v["status"].as_str()), ("200 OK", Some("pending")), "{v}");
    let id = v["id"].as_str().expect("id").to_string();
    let card = store::get_by_display_id(&id).expect("db").expect("row").card_id.expect("a card");
    (id, card)
}

fn stored(ws: &Ws, card: &str) -> OverlayDoc {
    k2_core::overlay::get_thread_doc(&ws.conv, card).expect("read").expect("the card")
}

/// Rosson Q1 at render: a pending card whose Settings switch is already on
/// is drawn "Already on" with `Close` for approvers, nothing for others,
/// and "Already on: …" on old clients and for the agent.
#[test]
fn a_satisfied_pending_card_draws_already_on_with_close() {
    let ws = seed();
    let (_, card) = ask(&ws);
    testkit::set_on(&ws.id, "test.switch", true);
    let d = stored(&ws, &card);
    let a = wire(&project(&d, &rosson(), 2));
    assert_eq!(a["choice"]["action"]["view"]["state"], "already_done", "{a}");
    assert_eq!(a["choice"]["options"], json!([{"label":"Close"}]), "§17.11.9");
    assert_eq!(a["choice"]["status"], "pending");
    assert!(a["choice"]["action"].get("result").is_none(), "no result until Close: {a}");
    let m = wire(&project(&d, &dana(), 2));
    assert_eq!((m["choice"]["action"]["view"]["state"].clone(), m["choice"]["options"].clone()), (json!("already_done"), json!([])));
    assert_eq!(m["choice"]["action"]["view"]["noteBox"]["allowed"], false, "notes only while pending");
    let line = format!("Already on: Allow test things for {}.", ws.handle);
    for viewer in [rosson(), dana(), Viewer::Passport { workspace_id: ws.id.clone() }] {
        assert_eq!(wire(&project(&d, &viewer, 0))["body"], line.as_str());
    }
    // The stored doc is untouched (the projection never writes).
    assert_eq!(stored(&ws, &card), d);
}

/// The real stored view (A1's `present::view_for`, neutral viewer) projects
/// to the approve and wait views.
#[test]
fn a1s_stored_view_projects_per_viewer() {
    let ws = seed();
    let (id, card) = ask(&ws);
    let d = stored(&ws, &card);
    assert_eq!(d.choice.as_ref().unwrap().action.as_ref().unwrap().view.as_ref().unwrap()["viewer"]["login"], "", "stored neutral");
    let a = wire(&project(&d, &Viewer::OwnerToken, 2));
    assert_eq!(a["choice"]["action"]["view"]["viewer"],
        json!({"login":"owner","role":"owner","via":"owner-token","mode":"approve","canAnswer":true,"isAsker":false}));
    assert_eq!(a["choice"]["action"]["view"]["requestId"], id.as_str());
    assert_eq!(a["choice"]["options"], json!([{"label":"Allow"},{"label":"Deny"}]));
    let m = wire(&project(&d, &dana(), 2));
    assert_eq!((m["choice"]["action"]["view"]["viewer"]["mode"].clone(), m["choice"]["options"].clone()), (json!("wait"), json!([])));
    assert_eq!(m["choice"]["action"]["view"]["noteBox"]["allowed"], true);
    let p = wire(&project(&d, &Viewer::Passport { workspace_id: ws.id.clone() }, 2));
    assert_eq!(p["body"], format!("{} asked an admin to turn on Allow test things.", ws.handle));
}

/// §6.2: the minute sweep moves a pending request whose switch was turned
/// on in Settings to `already_done` (and back), rebuilds and republishes
/// its card; it never touches a request the click already took, nor one
/// that was answered with `Close`.
#[test]
fn the_sweep_moves_pending_to_already_done_and_back_and_republishes() {
    let ws = seed();
    let (id, card) = ask(&ws);
    let state = || store::get_by_display_id(&id).expect("db").expect("row").state;
    let view_state = || stored(&ws, &card).choice.unwrap().action.unwrap().view.unwrap()["state"].clone();
    let mut rx = crate::overlay_ws::subscribe();
    let now = k2_core::overlay::now_secs();

    assert!(!k2_core::permission::gate::server_on(), "the server switch is off in lib tests");
    assert_eq!(sweep_already_done(now), 0, "a no-op while the switch is off");

    assert_eq!(sweep_open_requests(Some(&ws.id), now), 0, "still needed: nothing moves");
    testkit::set_on(&ws.id, "test.switch", true);
    assert_eq!(sweep_open_requests(Some(&ws.id), now), 1);
    assert_eq!((state(), view_state()), (RequestState::AlreadyDone, json!("already_done")));
    let frame = loop {
        let f = rx.try_recv().expect("the card was republished");
        if f.id == card {
            break f;
        }
    };
    assert_eq!(frame.conversation_id.as_deref(), Some(ws.conv.as_str()));
    assert_eq!(frame.doc.unwrap().choice.unwrap().status, "pending");
    assert_eq!(sweep_open_requests(Some(&ws.id), now), 0, "already done: nothing more");

    testkit::set_on(&ws.id, "test.switch", false);
    assert_eq!(sweep_open_requests(Some(&ws.id), now), 1, "turned off again in Settings");
    assert_eq!((state(), view_state()), (RequestState::Pending, json!("pending")));

    // The click took it (applying): never touched.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE permission_requests SET state = 'applying' WHERE id = ?1", [&id]).expect("applying");
    }
    testkit::set_on(&ws.id, "test.switch", true);
    assert_eq!(sweep_open_requests(Some(&ws.id), now), 0);
    assert_eq!(state(), RequestState::Applying);
    // Answered with Close (answered_at set): never reopened.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE permission_requests SET state = 'already_done', answered_at = 5 WHERE id = ?1", [&id]).expect("closed");
    }
    testkit::set_on(&ws.id, "test.switch", false);
    assert_eq!(sweep_open_requests(Some(&ws.id), now), 0);
    assert_eq!(state(), RequestState::AlreadyDone);
}
