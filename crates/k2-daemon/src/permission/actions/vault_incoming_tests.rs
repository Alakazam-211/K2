//! `vault.incoming.accept` end to end (0.46.10): a secret another agent sent
//! waits for a person; K2 presents the permission card on the receiving
//! agent's Thread (Beta or not); a Member, another agent and the passport
//! are refused; Allow lands it and Deny declines it through the decide
//! route's own entry; a clash asks for a free name; expiry and a decision
//! anywhere else close the card. Test vault (fake key store), test DB, temp
//! HOME; no real agent is woken (turns go to `inject`'s sink).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Grant, Proof, Undo, WallCoverage};
use k2_vault::handoff::{HandoffItem, Incoming};
use k2_vault::{Actor, Kind, LocalVault, Namespace, PutRequest, Requester, Scope, SecretBytes, SecretStore, Selector, Use};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, ask, ask_ok, below_floor, card_of, last_turn, login, row, seed, view, Ws};
use super::*;

/// The value these tests hand over; it must never surface.
const VALUE: &[u8] = b"k2test-incoming-card-not-real-5521";

fn uniq(prefix: &str) -> String {
    format!("{prefix}_{}", &uuid::Uuid::new_v4().simple().to_string()[..6].to_uppercase())
}

/// A secret `sales` (on this server) sent `ws`: it waits for a person (not
/// on the accept-from list), and K2 presents its card the way a send does
/// (`after_receive`; no knock, so no agent is woken).
fn pend(v: &LocalVault, ws: &Ws, name: &str, note: &str) -> HandoffRecord {
    let rec = receive(v, ws, name, note);
    crate::vault_handoff_routes::after_receive(
        v,
        &rec,
        &format!("/tmp/pc-e2-{}", ws.handle),
        &ws.id,
        "sales",
        None,
        None,
        false,
        crate::hire_routes::Sender { workspace_id: None, via: "owner", actor: "owner".into(), on_behalf_of: None },
    );
    rec
}

/// The pending row alone, with no card presented (a delivery from before
/// 0.46.10, or one whose card couldn't go up).
fn receive(v: &LocalVault, ws: &Ws, name: &str, note: &str) -> HandoffRecord {
    let item = HandoffItem {
        name: name.into(),
        as_name: None,
        kind: "api_key".into(),
        hosts: vec![],
        inject: None,
        description: String::new(),
        delivery: "file".into(),
        ceiling: "env".into(),
        human_given: false,
        ttl_secs: Some(7200),
    };
    let inc = Incoming {
        delivery_id: k2_vault::handoff::new_delivery_id(),
        workspace_id: ws.id.clone(),
        peer_addr: "sales".into(),
        peer_fp: None,
        sender_actor: "agent:ws-sales/s".into(),
        source_vault_id: None,
        item,
        note: note.into(),
        reply_to: None,
        auto_land: false,
        remote_off: false,
        sender_label: "sales on this server".into(),
    };
    let got = v.handoff_receive(inc, SecretBytes::from_slice(VALUE)).expect("receive");
    assert_eq!(got.record.status, "pending", "{:?}", got.record);
    got.record
}

fn has(v: &LocalVault, ws: &Ws, name: &str) -> bool {
    v.meta(&Selector::user(Scope::workspace(ws.id.clone()), name), &Actor::Owner).is_ok()
}

fn value_of(v: &LocalVault, ws: &Ws, name: &str) -> Vec<u8> {
    v.resolve(&Selector::user(Scope::workspace(ws.id.clone()), name), Use::System, &Requester::System("t".into())).expect("resolve").value.expose().to_vec()
}

fn status_of(v: &LocalVault, id: &str) -> String {
    v.handoff_get(id, "in").expect("row").status
}

/// K2's own request for a delivery (by its id: `held_ref`).
fn k2_request(id: &str) -> PermissionRequest {
    let r = row(id);
    assert_eq!((r.action.as_str(), r.held_ref.as_deref()), (ACTION, Some(id)), "{r:?}");
    r
}

fn card_doc(ws: &Ws, card: &str) -> k2_core::overlay::OverlayDoc {
    k2_core::overlay::get_thread_doc(&ws.conv, card).expect("doc read").expect("card doc")
}

#[test]
fn the_card_is_registered_admin_click_one_time_and_k2_presented() {
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (Floor::Admin, Proof::Click, Grant::OneTime));
    assert!(matches!(r.spec.undo, Undo::None));
    assert!(r.spec.walls.is_empty() && r.spec.coverage == WallCoverage::None, "an agent never reaches decide: no wall");
    assert_eq!(r.spec.params.iter().map(|p| p.key).collect::<Vec<_>>(), ["id", "as", "replace"]);
    assert!(r.spec.params[0].required && !r.spec.params[1].required && !r.spec.params[2].required);
    assert!(crate::routes::route_policy::lookup(ROUTE).and_then(|x| x.post).is_some(), "{ROUTE} has a POST row");
    assert!(crate::permission::present::find_not_cardable(ACTION).is_none(), "no longer 'can't be requested'");
    assert_eq!(ask_line("hd_1"), "k2 permission request vault.incoming.accept id=hd_1 --why \"<what you need it for>\"");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn k2_presents_the_card_people_at_the_floor_answer_and_allow_lands_it() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vin");
    let name = uniq("STRIPE");
    let rec = pend(&v, &ws, &name, "for the refund job");
    let id = rec.delivery_id.clone();

    // K2 put a standard permission card on the receiving agent's pinned
    // Thread, Beta or not: no switch is read (the old handoff card isn't
    // posted).
    assert!(v.handoff_get(&id, "in").expect("row").card.is_none(), "no old handoff card");
    let req = k2_request(&id);
    assert_eq!((req.session_id.as_deref(), req.floor.as_str(), req.state.as_str()), (Some(K2_SESSION), "admin", "pending"));
    assert_eq!(req.conversation_id.as_deref(), Some(ws.conv.as_str()));
    assert_eq!(req.params, json!({ "id": id }));
    let card = req.card_id.clone().expect("posted");
    let doc = card_doc(&ws, &card);
    let choice = doc.choice.as_ref().expect("choice");
    assert_eq!(choice.action.as_ref().expect("action").id, "permission");
    assert_eq!(choice.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["Allow", "Deny"]);

    // The card says what, from whom, the note, how long it's kept, and when
    // the delivery expires. Names only.
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], format!("Accept a secret sent to {}", ws.handle), "{vw}");
    let what = vw["whatChanges"].as_str().expect("whatChanges");
    assert!(what.contains(&format!("Accept {name} (api_key) from sales (on this server) into {}'s vault", ws.handle)), "{what}");
    let effect = vw["consequences"].as_str().expect("consequences");
    assert!(effect.contains("delivered as file, and may take it up to env"), "{effect}");
    assert!(effect.contains("kept for 2 hours"), "the TTL: {effect}");
    assert!(effect.contains("The sender's note: \"for the refund job\""), "{effect}");
    assert!(effect.contains("Unanswered, the delivery expires at"), "{effect}");
    assert_eq!(vw["scope"], format!("delivery {id} to {}", ws.handle), "{vw}");
    let wire = serde_json::to_string(&doc).expect("doc json");
    assert!(!wire.contains(std::str::from_utf8(VALUE).unwrap()), "never the value");

    // A Member is below the floor; an agent passport, a cell and a widget
    // are never people. Nothing lands.
    let member = login("vim", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow" })), "admin");
    for p in [
        vec![(crate::caller_workspace::PRINCIPAL_BOUND_KEY, "1"), ("project_id", ws.id.as_str()), ("from", ws.handle.as_str())],
        vec![("cell_session_id", "sess-x")],
    ] {
        let params: HashMap<String, String> = p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let b = json!({ "addr": ws.handle, "id": card, "answer": "Allow" });
        let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &params, b.to_string().as_bytes(), "owner");
        let e: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!((r.status, &e["error"]["code"]), ("403 Forbidden", &json!("action_card_person_only")), "{p:?}: {e}");
    }
    assert_eq!(status_of(&v, &id), "pending");
    assert!(!has(&v, &ws, &name));

    // An Admin taps Allow: the decide route accepts it, as them.
    let admin = login("via", Role::Admin);
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(status_of(&v, &id), "landed");
    assert_eq!(value_of(&v, &ws, &name), VALUE);
    let landed = v.handoff_get(&id, "in").expect("row");
    assert_eq!(landed.decided_by.as_deref(), Some(format!("connect:{admin}").as_str()), "decided as the person who clicked");
    let turn = last_turn(&ws);
    assert!(turn.starts_with(&format!("[permission:{id}] approved by {admin}")), "{turn}");
    assert!(turn.contains(&format!("{name} is in {}'s vault now", ws.handle)) && turn.contains("Use:"), "{turn}");
    assert!(!turn.contains(std::str::from_utf8(VALUE).unwrap()), "never the value");
    assert!(!card_doc(&ws, &card).is_pending_choice(), "the card closed");

    // Asking again: that delivery isn't waiting any more.
    let (s, e) = ask(&ws, ACTION, json!({ "id": id }));
    assert_eq!((s.as_str(), &e["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "{e}");
    assert!(e["error"]["hint"].as_str().unwrap_or("").contains("landed already"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deny_declines_the_delivery_through_the_decide_route() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vid");
    let name = uniq("TOKEN");
    let id = pend(&v, &ws, &name, "").delivery_id;
    let card = k2_request(&id).card_id.expect("posted");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Deny", "reason": "not this one" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state.as_str(), "denied");
    let rec = v.handoff_get(&id, "in").expect("row");
    assert_eq!((rec.status.as_str(), rec.decided_by.as_deref()), ("declined", Some("owner")), "declined, as the owner");
    assert!(!v.handoff_has_value(&id), "the pending copy is deleted");
    assert!(!has(&v, &ws, &name), "nothing landed");
    assert!(last_turn(&ws).starts_with(&format!("[permission:{id}] denied by owner")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_asks_for_its_own_pending_deliveries_only() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let (ws, other) = (seed("vio"), seed("vix"));
    let name = uniq("API");
    let id = pend(&v, &ws, &name, "").delivery_id;
    let k2_card = k2_request(&id).card_id.expect("posted");

    // Another agent can't ask for it; an unknown id reads the same.
    for (who, wid) in [(&other, id.as_str()), (&ws, "hd_00000000000000000000000000000000")] {
        let (s, e) = ask(who, ACTION, json!({ "id": wid }));
        assert_eq!((s.as_str(), &e["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "{e}");
        assert_eq!(e["error"]["hint"], format!("{} has no pending delivery {wid} (k2 vault incoming lists them)", who.handle), "{e}");
    }
    // Its own, the same values: K2's card is the one waiting.
    let (s, a) = ask(&ws, ACTION, json!({ "id": id }));
    assert_eq!((s.as_str(), &a["status"]), ("200 OK", &json!("already_pending")), "{a}");
    assert_eq!(a["card"], k2_card.as_str(), "{a}");
    // A bad name, or both as= and replace=, is refused when asked.
    let (s, e) = ask(&ws, ACTION, json!({ "id": id, "as": "bad name!" }));
    assert!(s.starts_with("400"), "{e}");
    let (s, e) = ask(&ws, ACTION, json!({ "id": id, "as": "X_ONE", "replace": "yes" }));
    assert!(s.starts_with("400"), "{e}");

    // Its own, under another name: a second card; answering it lands the
    // copy under that name and closes K2's card too.
    let other_name = uniq("RENAMED");
    let asked = ask_ok(&ws, ACTION, json!({ "id": id, "as": other_name }));
    let card = card_of(&asked);
    assert_ne!(card, k2_card);
    assert!(view(&ws, &card)["whatChanges"].as_str().unwrap_or("").ends_with(&format!("as {other_name}.")), "{}", view(&ws, &card));
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(value_of(&v, &ws, &other_name), VALUE);
    assert!(!has(&v, &ws, &name), "landed under the asked-for name only");
    assert!(!card_doc(&ws, &k2_card).is_pending_choice(), "K2's card for the same delivery closed");
    let k2_req = crate::permission::store::for_card(&ws.conv, &k2_card).expect("db").remove(0);
    assert_eq!((k2_req.state.as_str(), k2_req.answered_by.as_deref()), ("approved", Some("owner")), "{k2_req:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clash_asks_to_accept_it_under_a_free_name() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vic");
    let name = uniq("SHARED");
    // The workspace already has one of that name, from somewhere else.
    let mut p = PutRequest::new(Namespace::User, Scope::workspace(ws.id.clone()), &name, Kind::ApiKey);
    p.hosts = vec![];
    v.put(p, SecretBytes::from_slice(b"k2test-the-old-one"), &Actor::Owner).expect("put");

    let rec = pend(&v, &ws, &name, "");
    assert_eq!(rec.reason.as_deref(), Some("clash"));
    let req = k2_request(&rec.delivery_id);
    let free = format!("{name}_2");
    assert_eq!(req.params, json!({ "id": rec.delivery_id, "as": free }), "K2 picks a free name");
    let card = req.card_id.expect("posted");
    let vw = view(&ws, &card);
    assert!(vw["whatChanges"].as_str().unwrap_or("").ends_with(&format!("as {free}.")), "{vw}");
    assert!(vw["consequences"].as_str().unwrap_or("").contains(&format!("already has a secret named {name}; that one stays as it is")), "{vw}");
    assert!(vw["values"].as_array().expect("values").iter().any(|kv| kv["key"] == "as" && kv["value"] == free.as_str()), "{vw}");

    // Asked without a new name, it is refused and says how.
    let (s, e) = ask(&ws, ACTION, json!({ "id": rec.delivery_id, "as": name }));
    assert!(s.starts_with("400") && e["error"]["hint"].as_str().unwrap_or("").contains("already has"), "{e}");

    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(value_of(&v, &ws, &free), VALUE);
    assert_eq!(value_of(&v, &ws, &name), b"k2test-the-old-one", "the one it had stays");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expiry_and_a_late_look_close_or_restore_the_card() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vie");

    // Expiry: the vault ticker expires the delivery and closes its card.
    let id = pend(&v, &ws, &uniq("SOON"), "").delivery_id;
    let card = k2_request(&id).card_id.expect("posted");
    v.handoff_backdate_for_tests(&id, "in").expect("backdate");
    crate::vault_handoff_routes::tick(&v);
    assert_eq!(status_of(&v, &id), "expired");
    assert_eq!(row(&id).state.as_str(), "expired");
    assert!(!card_doc(&ws, &card).is_pending_choice(), "the card closed");
    assert!(last_turn(&ws).starts_with(&format!("[permission:{id}] expired")), "{}", last_turn(&ws));

    // A delivery with no card (from before 0.46.10, or one that couldn't
    // go up): the agent's own `k2 vault incoming` puts it up.
    let late = receive(&v, &ws, &uniq("LATE"), "").delivery_id;
    assert!(crate::permission::store::get_by_display_id(&late).expect("db").is_none());
    let caller = crate::vault::actor::VaultCaller {
        actor: Actor::Agent(k2_vault::AgentActor { workspace_id: ws.id.clone(), session_id: Some("s".into()), shell: false, vault_manage: false }),
        channel: crate::vault::actor::Channel::Plain,
        session_id: Some("s".into()),
    };
    let r = crate::vault_handoff_routes::handle(&v, crate::vault_handoff_routes::INCOMING, &caller, b"", &HashMap::new(), "test");
    assert!(r.status.starts_with("200"), "{}", r.body);
    let req = k2_request(&late);
    let j: Value = serde_json::from_str(&r.body).expect("json");
    let item = j["items"].as_array().expect("items").iter().find(|i| i["deliveryId"] == late.as_str()).expect("listed").clone();
    assert_eq!(item["card"]["card"], json!(req.card_id), "{item}");
    // Looking again doesn't post a second one.
    let _ = crate::vault_handoff_routes::handle(&v, crate::vault_handoff_routes::INCOMING, &caller, b"", &HashMap::new(), "test");
    assert_eq!(open_for(&ws.id, &late).len(), 1);
}

/// X15 (prd-vault-for-agents-v1 §5.1): `k2 permission list` shows the
/// action with no delivery id. It used to read "Accept delivery  into k2's
/// vault." (an empty id); now it says what the action does. A card whose id
/// K2 can't read keeps naming that id.
#[test]
fn the_listing_without_an_id_says_what_it_accepts() {
    let spec = super::super::find(ACTION).expect("registered").spec;
    let listed = Ctx { workspace_id: "w-1".into(), workspace_handle: "k2".into(), params: serde_json::Map::new() };
    let w = crate::permission::present::words(spec, &listed);
    assert_eq!(w.what_changes, "Accept a secret another agent sent into k2's vault.", "{}", w.what_changes);
    assert!(!w.what_changes.contains("  ") && !w.consequences.contains("  "), "no empty id: {} / {}", w.what_changes, w.consequences);
    assert!(w.consequences.contains("nothing is applied until a person allows that card"), "{}", w.consequences);
    let mut one = listed.clone();
    one.params.insert("id".into(), json!("hd_gone"));
    let computed = with_computed(ACTION, &one).expect("words");
    assert_eq!(computed.params["accept"], json!("Accept delivery hd_gone into k2's vault"), "{:?}", computed.params);
}
