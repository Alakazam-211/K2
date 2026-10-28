//! Secret cards that say how a secret is sent (prd-secret-card-injection-v1,
//! vs-live SC-A1–SC-A18; Rosson's 0.46.6 rulings: no presets). Fakes only.

use std::sync::Arc;

use base64::Engine as _;
use k2_vault::confirm::KdfCost;
use k2_vault::fetch::{inject_edit, InjectEdit};
use k2_vault::fsutil::TestDir;
use k2_vault::hosts::Origin;
use k2_vault::inject::{card_inject, Dst, Inject};
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::keystore::KeyStore;
use k2_vault::vault::{CardAnswer, SecretRequest};
use k2_vault::*;

const PW: &str = "k2test_card_pw_7c1d9e";
const ROBOT: &str = "https://robot-ws.your-server.de:443";

fn open(t: &TestDir, ks: &Arc<FakeKeyStore>) -> LocalVault {
    LocalVault::open(t.path().join("vault"), ks.clone() as Arc<dyn KeyStore>, OpenOptions { kdf: KdfCost::Floor }).expect("open vault")
}

fn card(id: &str, name: &str, inject: Option<Inject>) -> SecretRequest {
    SecretRequest {
        card_id: id.into(),
        conversation: "conv-1".into(),
        session_id: Some("sess-1".into()),
        workspace_id: "ws1".into(),
        requested_by: "agent:ws1/sess-1".into(),
        name: name.into(),
        kind: Kind::Password,
        proposed_scope: "workspace".into(),
        proposed_hosts: vec!["robot-ws.your-server.de".into()],
        proposed_delivery: Delivery::Fetch,
        prompt: String::new(),
        ttl_secs: None,
        status: String::new(),
        answered_by: None,
        secret_id: None,
        created_at: String::new(),
        answered_at: None,
        inject,
    }
}

fn inj(dst: &str, fmt: Option<&str>) -> Inject {
    card_inject(Some(dst), fmt).expect("inject").expect("some")
}

fn shown(i: &Inject) -> CardAnswer {
    CardAnswer { inject: Some(i.clone()), ..Default::default() }
}

fn agent(ws: &str) -> Actor {
    Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("sess-1".into()), shell: false, vault_manage: false })
}

fn member() -> Actor {
    Actor::Login { username: "mia".into(), role: Role::Member }
}

fn admin() -> Actor {
    Actor::Login { username: "ada".into(), role: Role::Admin }
}

fn sel(name: &str) -> Selector {
    Selector::user(Scope::workspace("ws1"), name)
}

/// What K2 puts on the wire for this secret, after the card was saved.
fn wire(v: &LocalVault, name: &str, url: &str) -> InjectEdit {
    let o = Origin::parse_url(url).expect("url").1;
    let who = Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) };
    let r = v.resolve(&sel(name), Use::Fetch(&o), &who).expect("resolve");
    let (d, f) = r.inject.choose(None, None).expect("defaults");
    inject_edit(&d, &f, &r.value).expect("edit")
}

#[test]
fn each_kind_round_trips_from_card_to_vault_to_the_wire() {
    let t = TestDir::new("ci-kinds");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    let cases: Vec<(&str, Inject, String, Box<dyn Fn(InjectEdit)>)> = vec![
        (
            "BASIC_PAIR",
            inj("basic", None),
            format!("robotuser:{PW}"),
            Box::new(move |e| match e {
                InjectEdit::Header { name, value } => {
                    assert_eq!(name, "Authorization");
                    assert_eq!(value.expose(), format!("Basic {}", b64(&format!("robotuser:{PW}"))).as_bytes());
                }
                _ => panic!("basic is a header"),
            }),
        ),
        (
            "BASIC_USER",
            inj("basic", Some("ws123:{}")),
            PW.to_string(),
            Box::new(move |e| match e {
                InjectEdit::Header { name, value } => {
                    assert_eq!(name, "Authorization");
                    assert_eq!(value.expose(), format!("Basic {}", b64(&format!("ws123:{PW}"))).as_bytes());
                }
                _ => panic!("basic is a header"),
            }),
        ),
        (
            "HEADER_KEY",
            inj("header:x-api-key", None),
            PW.to_string(),
            Box::new(|e| match e {
                InjectEdit::Header { name, value } => {
                    assert_eq!(name, "X-Api-Key");
                    assert_eq!(value.expose(), PW.as_bytes());
                }
                _ => panic!("header"),
            }),
        ),
        (
            "HEADER_FMT",
            inj("header:Authorization", Some("Token {}")),
            PW.to_string(),
            Box::new(|e| match e {
                InjectEdit::Header { name, value } => {
                    assert_eq!(name, "Authorization");
                    assert_eq!(value.expose(), format!("Token {PW}").as_bytes());
                }
                _ => panic!("header"),
            }),
        ),
        (
            "QUERY_KEY",
            inj("query:appid", None),
            PW.to_string(),
            Box::new(|e| match e {
                InjectEdit::Query { param, value } => {
                    assert_eq!(param, "appid");
                    assert_eq!(value.expose(), PW.as_bytes());
                }
                _ => panic!("query"),
            }),
        ),
    ];
    for (i, (name, inject, value, check)) in cases.into_iter().enumerate() {
        let id = format!("card-kind-{i:02}-0123456789");
        v.request_create(card(&id, name, Some(inject.clone()))).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(v.request_get(&id).expect("get").inject.as_ref(), Some(&inject), "{name}: sealed");
        let out = v.request_answer(&id, SecretBytes::from_string(value), &shown(&inject), &Actor::Owner).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!out.replaced);
        let saved = out.meta.inject.clone().expect("saved inject");
        assert_eq!(saved, inject, "{name}: saved as the card showed");
        // Saved without choices (§3.2): the agent can pick nothing else.
        assert!(saved.allowed_dst.is_empty() && saved.allowed_fmt.is_empty());
        assert_eq!(out.meta.default_delivery, Delivery::Fetch);
        assert_eq!(out.meta.hosts, vec![ROBOT.to_string()]);
        assert!(out.meta.human_given);
        check(wire(&v, name, "https://robot-ws.your-server.de/server"));
    }
}

#[test]
fn an_older_client_that_does_not_echo_the_injection_is_refused() {
    let t = TestDir::new("ci-unseen");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    v.request_create(card("card-unseen-0123456789", "HETZNER_ROBOT", Some(inj("basic", None)))).expect("card");
    let e = v
        .request_answer("card-unseen-0123456789", SecretBytes::from_string(format!("u:{PW}")), &CardAnswer::default(), &Actor::Owner)
        .expect_err("no echo");
    assert_eq!(e.code, "card_inject_unseen");
    assert_eq!(e.status, 409);
    assert!(e.hint.starts_with("Update K2 to answer this card."), "{}", e.hint);
    assert!(e.hint.contains("HETZNER_ROBOT") && e.hint.contains("Basic login") && e.hint.contains("robot-ws.your-server.de"), "{}", e.hint);
    assert!(!e.hint.contains(PW));
    // Nothing was saved and the card still waits.
    assert_eq!(v.meta(&sel("HETZNER_ROBOT"), &Actor::Owner).expect_err("not saved").status, 404);
    assert_eq!(v.request_get("card-unseen-0123456789").expect("get").status, "pending");
    // A card without an injection is answered as before, echo or not.
    v.request_create(card("card-plain-01234567890", "PLAIN", None)).expect("card");
    let out = v.request_answer("card-plain-01234567890", SecretBytes::from_string(PW.into()), &CardAnswer::default(), &Actor::Owner).expect("plain");
    // A password with no injection on the card: nothing chosen, as today.
    assert_eq!(out.meta.inject, None);
}

#[test]
fn basic_without_a_user_needs_user_colon_password() {
    let t = TestDir::new("ci-basic");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let b = inj("basic", None);
    v.request_create(card("card-basic-0123456789a", "B", Some(b.clone()))).expect("card");
    let e = v.request_answer("card-basic-0123456789a", SecretBytes::from_string(PW.into()), &shown(&b), &Actor::Owner).expect_err("no colon");
    assert_eq!(e.code, "basic_needs_user");
    assert!(!e.hint.contains(PW));
    v.request_answer("card-basic-0123456789a", SecretBytes::from_string(format!("u:{PW}")), &shown(&b), &Actor::Owner).expect("pair");
}

#[test]
fn body_is_never_on_a_card_and_injection_needs_fetch() {
    let t = TestDir::new("ci-body");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    // The agent writes the body, so it could echo the value back (SC-A13).
    assert_eq!(card_inject(Some("body"), None).expect_err("parse").code, "inject_body_not_on_cards");
    let body = Inject { dst: Dst::Body, fmt: "{}".into(), allowed_dst: vec![], allowed_fmt: vec![], sign: None };
    assert_eq!(v.request_create(card("card-body-0123456789ab", "B", Some(body.clone()))).expect_err("post").code, "inject_body_not_on_cards");
    // Nor as a person's edit on the answer.
    v.request_create(card("card-body2-0123456789a", "B2", None)).expect("card");
    assert_eq!(
        v.request_answer("card-body2-0123456789a", SecretBytes::from_string(PW.into()), &shown(&body), &Actor::Owner).expect_err("answer").code,
        "inject_body_not_on_cards"
    );
    // An injection with env or file delivery is refused at post (SC-A5).
    let mut env = card("card-env-0123456789abc", "E", Some(inj("basic", None)));
    env.proposed_delivery = Delivery::Env;
    assert_eq!(v.request_create(env).expect_err("env").code, "inject_needs_fetch");
}

#[test]
fn a_person_can_edit_how_it_is_sent_but_a_guest_cannot() {
    let t = TestDir::new("ci-edit");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    v.request_create(card("card-edit-0123456789ab", "EDITED", Some(inj("basic", None)))).expect("card");
    let mine = inj("header:X-Robot-Key", None);
    let out = v.request_answer("card-edit-0123456789ab", SecretBytes::from_string(PW.into()), &shown(&mine), &Actor::Owner).expect("edit");
    assert_eq!(out.meta.inject, Some(mine));
    let guest = Actor::Guest { username: "gail".into(), workspace_id: "ws1".into() };
    v.request_create(card("card-guest-0123456789a", "GUESTED", Some(inj("basic", None)))).expect("card");
    let e = v
        .request_answer("card-guest-0123456789a", SecretBytes::from_string(PW.into()), &shown(&inj("query:k", None)), &guest)
        .expect_err("guest edit");
    assert_eq!((e.code, e.status), ("inject_not_editable", 403));
    // The guest accepts what the card showed.
    let out = v.request_answer("card-guest-0123456789a", SecretBytes::from_string(format!("g:{PW}")), &shown(&inj("basic", None)), &guest).expect("guest");
    assert_eq!(out.meta.inject, Some(inj("basic", None)));
}

#[test]
fn after_the_card_an_agent_cannot_change_how_it_is_sent_without_the_owner() {
    let t = TestDir::new("ci-sealed");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let b = inj("basic", None);
    v.request_create(card("card-seal-0123456789ab", "SEALED", Some(b.clone()))).expect("card");
    v.request_answer("card-seal-0123456789ab", SecretBytes::from_string(format!("u:{PW}")), &shown(&b), &Actor::Owner).expect("saved");
    let p = PolicyPatch { inject: Some(inj("header:X-Leak", None)), ..Default::default() };
    let e = v.set_policy(&sel("SEALED"), p, &agent("ws1")).expect_err("agent widen");
    assert_eq!(e.code, "vault_owner_confirm_required");
    assert_eq!(v.meta(&sel("SEALED"), &Actor::Owner).expect("meta").inject, Some(b.clone()));
    // Per call, too: the sealed lists are empty.
    let o = Origin::parse_url("https://robot-ws.your-server.de/").expect("url").1;
    let who = Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) };
    let r = v.resolve(&sel("SEALED"), Use::Fetch(&o), &who).expect("resolve");
    assert_eq!(r.inject.choose(Some("header:X-Leak"), None).expect_err("dst").code, "inject_not_allowed");
    assert_eq!(r.inject.choose(None, Some("{}:x")).expect_err("fmt").code, "inject_not_allowed");
}

/// The existing secret K2 Web had: HETZNER_ROBOT saved as a Bearer token.
fn bearer_robot(v: &LocalVault, by: &Actor) {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "HETZNER_ROBOT", Kind::ApiKey);
    r.hosts = vec!["robot-ws.your-server.de".into()];
    r.inject = Some(Inject::bearer());
    r.default_delivery = Some(Delivery::Fetch);
    v.put(r, SecretBytes::from_string("old-bearer-value-1".into()), by).expect("put");
}

#[test]
fn sc_a1_an_owner_or_admin_card_for_an_existing_name_applies_its_terms() {
    for (who, actor) in [("owner", Actor::Owner), ("admin", admin())] {
        let t = TestDir::new("ci-a1-ok");
        let ks = Arc::new(FakeKeyStore::new());
        let v = open(&t, &ks);
        bearer_robot(&v, &Actor::Owner);
        let b = inj("basic", None);
        v.request_create(card("card-a1-0123456789abcd", "HETZNER_ROBOT", Some(b.clone()))).expect("card");
        let out = v
            .request_answer("card-a1-0123456789abcd", SecretBytes::from_string(format!("u:{PW}")), &shown(&b), &actor)
            .unwrap_or_else(|e| panic!("{who}: {e}"));
        assert!(out.replaced, "{who}");
        assert_eq!(out.inject_was.as_deref(), Some("as Authorization: Bearer ‹secret›"), "{who}");
        assert_eq!(out.meta.inject, Some(b.clone()), "{who}: Basic saved");
        assert_eq!(out.meta.latest_version, 2, "{who}");
        match wire(&v, "HETZNER_ROBOT", "https://robot-ws.your-server.de/") {
            InjectEdit::Header { value, .. } => assert!(value.expose().starts_with(b"Basic "), "{who}"),
            _ => panic!("{who}: header"),
        }
    }
}

#[test]
fn sc_a1_a_card_for_an_existing_name_also_applies_sites_and_delivery() {
    let t = TestDir::new("ci-a1-sites");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    bearer_robot(&v, &Actor::Owner);
    // A card with no injection: its sites change; how it is sent stays.
    let mut c = card("card-a1s-0123456789abc", "HETZNER_ROBOT", None);
    c.proposed_hosts = vec!["robot-ws.your-server.de".into(), "api.hetzner.cloud".into()];
    v.request_create(c).expect("card");
    let out = v.request_answer("card-a1s-0123456789abc", SecretBytes::from_string(PW.into()), &CardAnswer::default(), &Actor::Owner).expect("owner");
    assert_eq!(out.meta.hosts, vec!["https://api.hetzner.cloud:443".to_string(), ROBOT.to_string()]);
    assert_eq!(out.meta.inject, Some(Inject::bearer()));
    assert_eq!(out.inject_was, None);
    // A card that moves it to a file: the delivery and ceiling follow.
    let mut f = card("card-a1f-0123456789abc", "HETZNER_ROBOT", None);
    f.proposed_hosts = vec![];
    f.proposed_delivery = Delivery::File;
    v.request_create(f).expect("card");
    let ans = CardAnswer { readable_disclosed: true, ..Default::default() };
    let out = v.request_answer("card-a1f-0123456789abc", SecretBytes::from_string(PW.into()), &ans, &Actor::Owner).expect("file");
    // The ceiling (env) already covers a file: it stays.
    assert_eq!((out.meta.default_delivery, out.meta.delivery_ceiling), (Delivery::File, Delivery::Env));
}

#[test]
fn sc_a1_a_member_who_would_widen_is_refused_and_nothing_is_written() {
    let t = TestDir::new("ci-a1-member");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    // The member's own secret, so only the widening is in question.
    bearer_robot(&v, &member());
    let before = v.meta(&sel("HETZNER_ROBOT"), &Actor::Owner).expect("meta");
    let b = inj("basic", None);
    v.request_create(card("card-a1m-0123456789abc", "HETZNER_ROBOT", Some(b.clone()))).expect("card");
    let e = v
        .request_answer("card-a1m-0123456789abc", SecretBytes::from_string(format!("u:{PW}")), &shown(&b), &member())
        .expect_err("member widen");
    assert_eq!((e.code, e.status), ("vault_admin_required", 403));
    assert!(e.hint.contains("an Owner or Admin must save this card"), "{}", e.hint);
    let after = v.meta(&sel("HETZNER_ROBOT"), &Actor::Owner).expect("meta");
    assert_eq!(after, before, "policy unchanged");
    match wire(&v, "HETZNER_ROBOT", "https://robot-ws.your-server.de/") {
        InjectEdit::Header { value, .. } => assert_eq!(value.expose(), b"Bearer old-bearer-value-1", "value unchanged"),
        _ => panic!("header"),
    }
    assert_eq!(v.request_get("card-a1m-0123456789abc").expect("get").status, "pending");
    // The same member replaces the value when the terms match.
    v.request_create(card("card-a1n-0123456789abc", "HETZNER_ROBOT", Some(Inject::bearer()))).expect("card");
    let mut same = shown(&Inject::bearer());
    same.hosts = None;
    let out = v.request_answer("card-a1n-0123456789abc", SecretBytes::from_string("new-bearer-value-2".into()), &same, &member()).expect("same terms");
    assert!(out.replaced);
    assert_eq!(out.meta.inject, Some(Inject::bearer()));
    assert_eq!(out.meta.latest_version, 2);
}

#[test]
fn sc_a1_a_guest_only_replaces_matching_terms() {
    let t = TestDir::new("ci-a1-guest");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let guest = Actor::Guest { username: "gail".into(), workspace_id: "ws1".into() };
    // The guest's own chat secret, saved by an earlier card.
    let b = inj("basic", None);
    v.request_create(card("card-g1-0123456789abcd", "GUEST_ROBOT", Some(b.clone()))).expect("card");
    v.request_answer("card-g1-0123456789abcd", SecretBytes::from_string(format!("g:{PW}")), &shown(&b), &guest).expect("first");
    let mut wider = card("card-g2-0123456789abcd", "GUEST_ROBOT", Some(b.clone()));
    wider.proposed_hosts = vec!["robot-ws.your-server.de".into(), "api.hetzner.cloud".into()];
    v.request_create(wider).expect("card");
    let e = v
        .request_answer("card-g2-0123456789abcd", SecretBytes::from_string(format!("g:{PW}2")), &shown(&b), &guest)
        .expect_err("guest terms");
    assert_eq!(e.code, "vault_admin_required");
}

#[test]
fn sc_a1_an_agent_made_secret_becomes_a_persons_after_a_card() {
    let t = TestDir::new("ci-a1-agent");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    bearer_robot(&v, &agent("ws1"));
    assert!(!v.meta(&sel("HETZNER_ROBOT"), &Actor::Owner).expect("meta").human_given);
    let b = inj("basic", None);
    v.request_create(card("card-a1g-0123456789abc", "HETZNER_ROBOT", Some(b.clone()))).expect("card");
    let out = v.request_answer("card-a1g-0123456789abc", SecretBytes::from_string(format!("u:{PW}")), &shown(&b), &Actor::Owner).expect("owner");
    assert!(out.meta.human_given);
    let p = PolicyPatch { inject: Some(Inject::bearer()), ..Default::default() };
    assert_eq!(v.set_policy(&sel("HETZNER_ROBOT"), p, &agent("ws1")).expect_err("agent").code, "vault_owner_confirm_required");
}

#[test]
fn a_tampered_card_injection_is_refused_and_rekey_carries_a_pending_one() {
    let t = TestDir::new("ci-mac");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let b = inj("basic", None);
    v.request_create(card("card-rekey-0123456789a", "REKEYED", Some(b.clone()))).expect("card");
    v.rekey(&Actor::Owner, &HumanProof::Confirmed { method: "t".into() }).expect("rekey");
    assert_eq!(v.request_get("card-rekey-0123456789a").expect("get").inject, Some(b.clone()));
    v.request_answer("card-rekey-0123456789a", SecretBytes::from_string(format!("u:{PW}")), &shown(&b), &Actor::Owner).expect("answers after rekey");
    // Swap a pending card's sealed injection on disk (its MAC no longer fits).
    v.request_create(card("card-tamper-0123456789", "TAMPERED", Some(b.clone()))).expect("card");
    v.checkpoint();
    let c = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    let n = c
        .execute("UPDATE secret_request_inject SET inject = ?1 WHERE card_id = 'card-tamper-0123456789'", [r#"{"dst":"header:X-Leak","fmt":"{}"}"#])
        .expect("tamper");
    assert_eq!(n, 1);
    drop(c);
    let e = v.request_get("card-tamper-0123456789").expect_err("tamper");
    assert_eq!(e.code, "vault_tamper");
    let leak = inj("header:X-Leak", None);
    assert_eq!(
        v.request_answer("card-tamper-0123456789", SecretBytes::from_string(PW.into()), &shown(&leak), &Actor::Owner).expect_err("refused").code,
        "vault_tamper"
    );
    // And it stops a rekey (verify under the old key first), naming the way out.
    let e = v.rekey(&Actor::Owner, &HumanProof::Confirmed { method: "t".into() }).expect_err("rekey");
    assert_eq!(e.code, "vault_tamper");
    assert!(e.hint.contains("void that card"), "{}", e.hint);
    // The card can still be voided; its injection row goes with it, and the
    // rekey then runs.
    assert_eq!(v.request_void("card-tamper-0123456789", &Actor::Owner).expect("void").status, "void");
    v.rekey(&Actor::Owner, &HumanProof::Confirmed { method: "t".into() }).expect("rekey after void");
}

#[test]
fn a_card_value_never_reaches_the_request_rows_or_audit() {
    let t = TestDir::new("ci-novalue");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let b = inj("basic", None);
    v.request_create(card("card-nv-0123456789abcd", "NV", Some(b.clone()))).expect("card");
    v.request_create(card("card-nv2-0123456789abc", "NV2", Some(inj("query:k", None)))).expect("card");
    v.request_answer("card-nv-0123456789abcd", SecretBytes::from_string(format!("u:{PW}")), &shown(&b), &Actor::Owner).expect("saved");
    let reqs = v.requests(None).expect("reqs");
    // A pending card lists how it sends; a closed one's row is gone (the
    // saved secret holds it).
    assert_eq!(reqs.iter().find(|r| r.card_id == "card-nv2-0123456789abc").expect("pending").inject, Some(inj("query:k", None)));
    assert_eq!(reqs.iter().find(|r| r.card_id == "card-nv-0123456789abcd").expect("set").inject, None);
    let reqs = serde_json::to_string(&reqs).expect("json");
    assert!(!reqs.contains(PW));
    let audit = format!("{:?}", v.audit(&AuditQuery::default(), &Actor::Owner).expect("audit"));
    assert!(!audit.contains(PW));
}
