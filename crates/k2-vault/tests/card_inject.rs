//! Secret cards that say how a secret is sent (prd-secret-card-injection-v1,
//! vs-live SC-A1–SC-A18; Rosson's 0.46.6 rulings: no presets). Fakes only.

/// A daemon restart opens the vault again from disk. A card's saved session
/// secret, the card's state and a destroy all come back as they were, the
/// value only ever encrypted; one past its time is gone.
#[test]
fn a_saved_session_card_survives_a_restart_until_its_time_runs_out() {
    let t = TestDir::new("ci-restart");
    let ks = Arc::new(FakeKeyStore::new());
    let me = Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) };
    let readable = CardAnswer { readable_disclosed: true, ..Default::default() };
    let (kept, gone) = {
        let v = open(&t, &ks);
        let mut keep = card("card-rs-keep-012345678", "RS_KEEP", None);
        keep.proposed_scope = "session".into();
        keep.proposed_hosts = vec![];
        keep.proposed_delivery = Delivery::File;
        v.request_create(keep).expect("card");
        let kept = v.request_answer("card-rs-keep-012345678", SecretBytes::from_string(PW.to_string()), &readable, &Actor::Owner).expect("saved").meta;
        let mut drop_it = card("card-rs-gone-012345678", "RS_GONE", None);
        drop_it.proposed_scope = "session".into();
        drop_it.proposed_hosts = vec![];
        drop_it.proposed_delivery = Delivery::File;
        v.request_create(drop_it).expect("card");
        let gone = v.request_answer("card-rs-gone-012345678", SecretBytes::from_string(PW.to_string()), &readable, &Actor::Owner).expect("saved").meta;
        v.destroy(&Selector::Id(gone.id.clone()), &Actor::Owner).expect("destroyed before the restart");
        // A session secret near its end (a card's shortest time is a minute).
        let mut brief = PutRequest::new(Namespace::User, Scope::session("sess-1"), "RS_BRIEF", Kind::Generic);
        brief.default_delivery = Some(Delivery::File);
        brief.expires_at = Some((chrono::Utc::now() + chrono::Duration::seconds(2)).to_rfc3339());
        v.put(brief, SecretBytes::from_string(PW.to_string()), &Actor::Owner).expect("brief");
        (kept, gone)
    };
    for f in walk(&t.path().join("vault")) {
        let bytes = std::fs::read(&f).unwrap_or_default();
        assert!(!bytes.windows(PW.len()).any(|w| w == PW.as_bytes()), "plaintext in {}", f.display());
    }

    let v = open(&t, &ks);
    let r = v.resolve(&Selector::Id(kept.id.clone()), Use::File, &me).expect("the saved secret is there after the restart");
    assert_eq!(r.value.expose(), PW.as_bytes());
    assert_eq!(r.meta.expires_at, kept.expires_at, "its time is kept");
    assert_eq!(v.request_get("card-rs-keep-012345678").expect("card").status, "set");
    let names: Vec<String> = v.spawn_items(&me).expect("items").into_iter().map(|i| i.name).collect();
    assert!(names.contains(&"RS_KEEP".to_string()) && !names.contains(&"RS_GONE".to_string()), "{names:?}");
    assert!(v.resolve(&Selector::Id(gone.id.clone()), Use::File, &me).is_err(), "a destroy before the restart stays");
    std::thread::sleep(std::time::Duration::from_millis(3200));
    let brief = Selector::user(Scope::session("sess-1"), "RS_BRIEF");
    assert!(v.resolve(&brief, Use::File, &me).is_err(), "past its time it is gone");
    assert!(!v.spawn_items(&me).expect("items").iter().any(|i| i.name == "RS_BRIEF"));
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

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
        once: false,
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

fn self_grant(sel: Selector, to: Consumer, d: Delivery) -> GrantRequest {
    GrantRequest { selector: sel, consumer: to, delivery: d, env_name: None, file_name: None, hosts_narrowed: None, expires_at: None, version: None }
}

/// A card says how the agent gets the value, and `agentCanSee` follows it. An
/// agent can't get round that by making the name first with a readable ceiling
/// and sharing it with itself: a person's Save drops every share above the
/// card's delivery, in workspace and session scope, and the agent can't share
/// it again without the owner. A share at or below the delivery stays.
#[test]
fn a_persons_save_drops_shares_above_what_the_card_says() {
    let t = TestDir::new("ci-shares");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let me = Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) };
    for (card_id, name, scope, to, proposed) in [
        ("card-share-ws-01234567", "SHARE_WS", Scope::workspace("ws1"), Consumer::workspace("ws1"), "workspace"),
        ("card-share-se-01234567", "SHARE_SE", Scope::session("sess-1"), Consumer::session("sess-1"), "session"),
    ] {
        let sel = Selector::user(scope.clone(), name);
        let mut r = PutRequest::new(Namespace::User, scope, name, Kind::Password);
        r.hosts = vec!["robot-ws.your-server.de".into()];
        r.default_delivery = Some(Delivery::Fetch);
        r.delivery_ceiling = Some(Delivery::File);
        if proposed == "session" {
            r.expires_at = Some((chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339());
        }
        v.put(r, SecretBytes::from_string("agent-placeholder".into()), &agent("ws1")).expect("the agent makes the name");
        v.grant(self_grant(sel.clone(), to.clone(), Delivery::File), &agent("ws1")).expect("and shares it with itself");
        v.grant(self_grant(sel.clone(), Consumer::workspace("ws-other"), Delivery::Fetch), &Actor::Owner).expect("a fetch share");
        let mut c = card(card_id, name, None);
        c.proposed_scope = proposed.into();
        v.request_create(c).expect("a fetch-only card");
        let out = v.request_answer(card_id, SecretBytes::from_string(PW.to_string()), &CardAnswer::default(), &Actor::Owner).expect("the owner saves");
        assert!(!out.meta.agent_can_see(), "{name}");
        let e = v.resolve(&Selector::Id(out.meta.id.clone()), Use::File, &me).err().unwrap_or_else(|| panic!("{name}: the agent read a fetch-only value"));
        assert!(e.status == 403, "{name}: {e}");
        let left = v.grants(&sel, &Actor::Owner).expect("grants");
        assert_eq!(left.iter().map(|g| g.delivery).collect::<Vec<_>>(), vec![Delivery::Fetch], "{name}: the fetch share stays");
        let again = v.grant(self_grant(sel.clone(), to, Delivery::File), &agent("ws1"));
        assert!(again.is_err(), "{name}: the agent can't share a person's value with itself");
    }

    // A card that does say file keeps the file share, and says the agent can see it.
    let sel = Selector::user(Scope::workspace("ws1"), "SHARE_FILE");
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "SHARE_FILE", Kind::Password);
    r.default_delivery = Some(Delivery::Fetch);
    r.hosts = vec!["robot-ws.your-server.de".into()];
    r.delivery_ceiling = Some(Delivery::Env);
    v.put(r, SecretBytes::from_string("agent-placeholder".into()), &agent("ws1")).expect("put");
    v.grant(self_grant(sel.clone(), Consumer::workspace("ws1"), Delivery::File), &agent("ws1")).expect("file share");
    v.grant(self_grant(sel.clone(), Consumer::workspace("ws-env"), Delivery::Env), &agent("ws1")).expect("env share");
    let mut c = card("card-share-fi-01234567", "SHARE_FILE", None);
    c.proposed_delivery = Delivery::File;
    v.request_create(c).expect("card");
    let ans = CardAnswer { readable_disclosed: true, ..Default::default() };
    let out = v.request_answer("card-share-fi-01234567", SecretBytes::from_string(PW.to_string()), &ans, &Actor::Owner).expect("saved");
    assert!(out.meta.agent_can_see());
    let left = v.grants(&sel, &Actor::Owner).expect("grants");
    assert_eq!(left.iter().map(|g| g.delivery).collect::<Vec<_>>(), vec![Delivery::File], "the env share is above the card");
    assert_eq!(v.resolve(&sel, Use::File, &me).expect("file").value.expose(), PW.as_bytes());
}

/// A session-scope card binds to the session that asked: the passport id
/// spawn, resolve and list compare. The Thread's conversation id is another
/// id, and a secret bound to it reached nobody while the card said saved.
#[test]
fn a_session_card_binds_to_the_session_that_asked() {
    let t = TestDir::new("ci-session");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let readable = CardAnswer { readable_disclosed: true, ..Default::default() };
    let mut c = card("card-sess-0123456789ab", "SESS_FILE", None);
    c.proposed_scope = "session".into();
    c.proposed_hosts = vec![];
    c.proposed_delivery = Delivery::File;
    v.request_create(c).expect("card");
    let out = v.request_answer("card-sess-0123456789ab", SecretBytes::from_string(PW.to_string()), &readable, &Actor::Owner).expect("saved");
    assert_eq!(out.meta.scope, Scope::session("sess-1"), "the asking session, not the conversation");
    assert_eq!(out.reference, "k2v://session/sess-1/SESS_FILE");

    let me = Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) };
    let items = v.spawn_items(&me).expect("items");
    assert_eq!(items.iter().map(|i| (i.name.as_str(), i.delivery)).collect::<Vec<_>>(), vec![("SESS_FILE", Delivery::File)]);
    let r = v.resolve(&Selector::Id(out.meta.id.clone()), Use::File, &me).expect("resolve");
    assert_eq!(r.value.expose(), PW.as_bytes());
    let listed = v.list(&ListFilter::default(), &agent("ws1")).expect("list");
    assert!(listed.iter().any(|m| m.name == "SESS_FILE"), "k2 vault list shows it to that session");

    // Another session of the workspace, and a passport holding the chat's id
    // as if it were a session, get nothing.
    for other in ["sess-2", "conv-1"] {
        let who = Requester::Session { workspace_id: "ws1".into(), session_id: Some(other.into()) };
        assert!(v.spawn_items(&who).expect("items").is_empty(), "{other}");
        assert!(v.resolve(&Selector::Id(out.meta.id.clone()), Use::File, &who).is_err(), "{other}");
        let a = Actor::Agent(AgentActor { workspace_id: "ws1".into(), session_id: Some(other.into()), shell: false, vault_manage: false });
        assert!(!v.list(&ListFilter::default(), &a).expect("list").iter().any(|m| m.name == "SESS_FILE"), "{other}");
    }

    // No session asked: nothing could receive it, so Save is refused and
    // nothing is kept. The card can still be saved for the workspace.
    let mut none = card("card-nosess-0123456789", "NO_SESS", None);
    none.session_id = None;
    none.proposed_scope = "session".into();
    none.proposed_hosts = vec![];
    none.proposed_delivery = Delivery::File;
    v.request_create(none).expect("card");
    let e = v.request_answer("card-nosess-0123456789", SecretBytes::from_string(PW.to_string()), &readable, &Actor::Owner).err().expect("refused");
    assert_eq!(e.code, "card_no_session");
    assert_eq!(v.request_get("card-nosess-0123456789").expect("request").status, "pending");
    assert!(v.list(&ListFilter::default(), &Actor::Owner).expect("list").iter().all(|m| m.name != "NO_SESS"));
    let ws = CardAnswer { scope: Some("workspace".into()), readable_disclosed: true, ..Default::default() };
    v.request_answer("card-nosess-0123456789", SecretBytes::from_string(PW.to_string()), &ws, &Actor::Owner).expect("for the workspace");

    // Env reaches new sessions only, and a session's secret ends with it.
    let mut env = card("card-sessenv-012345678", "SESS_ENV", None);
    env.proposed_scope = "session".into();
    env.proposed_hosts = vec![];
    env.proposed_delivery = Delivery::Env;
    v.request_create(env).expect("card");
    let e = v.request_answer("card-sessenv-012345678", SecretBytes::from_string(PW.to_string()), &readable, &Actor::Owner).err().expect("refused");
    assert_eq!(e.code, "session_env_unreachable");
    assert_eq!(v.request_get("card-sessenv-012345678").expect("request").status, "pending");
}

/// A person's session card lasts what `--ttl` asks, 24 h by default and at
/// most 7 days; a guest's at most 24 h.
#[test]
fn a_session_card_lasts_at_most_seven_days() {
    let t = TestDir::new("ci-ttl");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let readable = CardAnswer { readable_disclosed: true, ..Default::default() };
    let hours_left = |m: &SecretMeta| {
        let at = chrono::DateTime::parse_from_rfc3339(m.expires_at.as_deref().expect("expires")).expect("rfc3339");
        (at.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_minutes() as f64 / 60.0
    };
    let save = |id: &str, name: &str, ttl: Option<i64>, who: &Actor, delivery: Delivery| {
        let mut c = card(id, name, None);
        c.proposed_scope = "session".into();
        c.ttl_secs = ttl;
        c.proposed_delivery = delivery;
        if delivery != Delivery::Fetch {
            c.proposed_hosts = vec![];
        }
        v.request_create(c).expect("card");
        v.request_answer(id, SecretBytes::from_string(PW.to_string()), &readable, who).expect("saved").meta
    };
    let guest = Actor::Guest { username: "gail".into(), workspace_id: "ws1".into() };
    let cases = [
        ("card-ttl-def-012345678", "TTL_DEFAULT", None, Actor::Owner, Delivery::File, 24.0),
        ("card-ttl-3d-0123456789", "TTL_THREE", Some(3 * 86400), Actor::Owner, Delivery::File, 72.0),
        ("card-ttl-30d-012345678", "TTL_THIRTY", Some(30 * 86400), Actor::Owner, Delivery::File, 168.0),
        ("card-ttl-gst-012345678", "TTL_GUEST", Some(7 * 86400), guest, Delivery::Fetch, 24.0),
    ];
    for (id, name, ttl, who, delivery, want) in cases {
        let got = hours_left(&save(id, name, ttl, &who, delivery));
        assert!((got - want).abs() < 0.1, "{name}: {got} h left, want {want}");
    }
}

/// A session file card for `name` with `ttl_secs`, saved by the owner with
/// `value`. Returns the outcome (or the vault's refusal).
fn save_session_card(v: &LocalVault, id: &str, name: &str, ttl_secs: i64, value: &str) -> Result<k2_vault::vault::CardResult> {
    let mut c = card(id, name, None);
    c.proposed_scope = "session".into();
    c.proposed_hosts = vec![];
    c.proposed_delivery = Delivery::File;
    c.ttl_secs = Some(ttl_secs);
    v.request_create(c).expect("card");
    let readable = CardAnswer { readable_disclosed: true, ..Default::default() };
    v.request_answer(id, SecretBytes::from_string(value.to_string()), &readable, &Actor::Owner)
}

fn secs_left(m: &SecretMeta) -> i64 {
    let at = chrono::DateTime::parse_from_rfc3339(m.expires_at.as_deref().expect("a session secret has an expiry")).expect("rfc3339");
    (at.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds()
}

/// GH#81: a second card for a name already saved for that session lasts the
/// NEW card's --ttl. It kept the first card's time, so a second card saved
/// near that time lost its value at the next minute tick.
#[test]
fn a_second_session_card_for_a_name_takes_the_new_cards_time() {
    let t = TestDir::new("ci-ttl2");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let me = Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) };
    let first = save_session_card(&v, "card-ttl2-first-01234567", "GH81_CODE", 120, "first-code").expect("first card").meta;
    assert!((60..=120).contains(&secs_left(&first)), "first card: 2 minutes, {}", secs_left(&first));

    let second = save_session_card(&v, "card-ttl2-second-0123456", "GH81_CODE", 7200, PW).expect("second card");
    assert!(second.replaced, "the same live secret got the new value");
    assert_eq!(second.meta.id, first.id);
    assert!(secs_left(&second.meta) > 7000, "GH#81: the second card's 2 h, not the first card's 2 min: {} s left", secs_left(&second.meta));
    let stored = v.meta(&Selector::Id(first.id.clone()), &Actor::Owner).expect("meta");
    assert_eq!(stored.expires_at, second.meta.expires_at, "the new time is what the vault keeps");
    assert_eq!(v.resolve(&Selector::Id(first.id), Use::File, &me).expect("resolves").value.expose(), PW.as_bytes());
}

/// GH#81: a session secret of that name whose time ran out is over. A new
/// card keeps a fresh secret (a new id, the new card's time) whether the
/// minute tick had already destroyed the old one ("pick a new name" before)
/// or not yet (the value went with the next tick before). The tick leaves
/// the new one alone. A workspace name a person destroyed stays retired.
#[test]
fn a_session_card_after_its_names_secret_ran_out_saves_a_fresh_secret() {
    let t = TestDir::new("ci-ttl-over");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let me = Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) };

    // Ran out, and the tick already destroyed it.
    let old = save_session_card(&v, "card-over-swept-0123456", "GH81_SWEPT", 120, "old-code").expect("first card").meta;
    v.expire_secret_for_tests(&old.id).expect("backdate");
    v.tick().expect("tick");
    assert_eq!(v.meta(&Selector::Id(old.id.clone()), &Actor::Owner).expect("old row").status, SecretStatus::Destroyed);
    let new = save_session_card(&v, "card-over-swept-new-0123", "GH81_SWEPT", 7200, PW).expect("GH#81: a new card after the old one was destroyed saves");
    assert!(!new.replaced, "a fresh secret, not the destroyed one");
    assert_ne!(new.meta.id, old.id);
    assert_eq!(new.meta.name, "GH81_SWEPT");
    assert!(secs_left(&new.meta) > 7000, "{} s left", secs_left(&new.meta));
    let gone = v.meta(&Selector::Id(old.id.clone()), &Actor::Owner).expect("the old row keeps its history");
    assert_eq!(gone.status, SecretStatus::Destroyed);
    assert_eq!(gone.name, format!("GH81_SWEPT~{}", old.id));

    // Ran out, and the tick hasn't come yet.
    let old2 = save_session_card(&v, "card-over-due-012345678", "GH81_DUE", 120, "old-code").expect("first card").meta;
    v.expire_secret_for_tests(&old2.id).expect("backdate");
    let new2 = save_session_card(&v, "card-over-due-new-012345", "GH81_DUE", 7200, PW).expect("a new card for an expired name saves");
    assert!(!new2.replaced);
    assert_ne!(new2.meta.id, old2.id);
    assert_eq!(v.meta(&Selector::Id(old2.id.clone()), &Actor::Owner).expect("old row").status, SecretStatus::Destroyed, "the expired one is destroyed, as the tick would");
    v.tick().expect("tick");
    for (sel, what) in [(Selector::Id(new2.meta.id.clone()), "by id"), (Selector::user(Scope::session("sess-1"), "GH81_DUE"), "by name")] {
        let r = v.resolve(&sel, Use::File, &me).unwrap_or_else(|e| panic!("GH#81: the new value is there after the tick ({what}): {}", e.code));
        assert_eq!(r.value.expose(), PW.as_bytes());
    }
    let items: Vec<String> = v.spawn_items(&me).expect("items").into_iter().map(|i| i.name).collect();
    assert!(items.contains(&"GH81_DUE".to_string()) && items.contains(&"GH81_SWEPT".to_string()), "{items:?}");

    // A workspace name a person destroyed is still retired (unchanged).
    let mut ws = card("card-over-ws-0123456789", "GH81_WS", None);
    ws.proposed_hosts = vec![];
    ws.proposed_delivery = Delivery::File;
    v.request_create(ws).expect("card");
    let readable = CardAnswer { readable_disclosed: true, ..Default::default() };
    let w = v.request_answer("card-over-ws-0123456789", SecretBytes::from_string(PW.to_string()), &readable, &Actor::Owner).expect("saved").meta;
    v.destroy(&Selector::Id(w.id), &Actor::Owner).expect("destroy");
    let mut again = card("card-over-ws-again-01234", "GH81_WS", None);
    again.proposed_hosts = vec![];
    again.proposed_delivery = Delivery::File;
    v.request_create(again).expect("card");
    let e = v.request_answer("card-over-ws-again-01234", SecretBytes::from_string(PW.to_string()), &readable, &Actor::Owner).expect_err("a destroyed workspace name stays retired");
    assert_eq!(e.code, "secret_destroyed");
}

/// A `--once` card (§3.5): kept for one session as a file.
fn once_card(id: &str, name: &str) -> SecretRequest {
    let mut c = card(id, name, None);
    c.proposed_scope = "session".into();
    c.proposed_hosts = vec![];
    c.proposed_delivery = Delivery::File;
    c.ttl_secs = Some(3600);
    c.once = true;
    c
}

fn readable() -> CardAnswer {
    CardAnswer { readable_disclosed: true, ..Default::default() }
}

/// §3.5: a one-time secret is never delivered and never resolves any other
/// way; `take_once` reads it once, destroys it (audited system:once) and
/// frees the name for the next card.
#[test]
fn a_one_time_secret_is_read_once_then_gone() {
    let t = TestDir::new("ci-once");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let me = Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) };
    v.request_create(once_card("card-once-01234567890", "ONCE_CODE")).expect("card");
    assert!(v.request_get("card-once-01234567890").expect("request").once, "the card keeps its mark");
    let saved = v.request_answer("card-once-01234567890", SecretBytes::from_string(PW.to_string()), &readable(), &Actor::Owner).expect("saved").meta;
    assert!(v.is_once(&saved.id));
    let names: Vec<String> = v.spawn_items(&me).expect("items").into_iter().map(|i| i.name).collect();
    assert!(!names.contains(&"ONCE_CODE".to_string()), "never delivered: {names:?}");

    let sel = Selector::user(Scope::session("sess-1"), "ONCE_CODE");
    let e = v.resolve(&sel, Use::File, &me).err().expect("only take_once reads it");
    assert_eq!(e.code, "once_only");

    let got = v.take_once(&sel, &me, |r| Ok(r.value.expose().to_vec())).expect("first read");
    assert_eq!(got, PW.as_bytes());
    let e = v.take_once(&sel, &me, |r| Ok(r.value.expose().to_vec())).expect_err("gone after the first read");
    assert_eq!(e.status, 404, "{e:?}");

    let gone = v.meta(&Selector::Id(saved.id.clone()), &Actor::Owner).expect("the old row keeps its history");
    assert_eq!(gone.status, SecretStatus::Destroyed);
    assert_eq!(gone.name, format!("ONCE_CODE~{}", saved.id));
    let audit = v.audit(&AuditQuery { secret_id: Some(saved.id.clone()), ..Default::default() }, &Actor::Owner).expect("audit");
    assert!(audit.iter().any(|r| r.actor == "system:once"), "{audit:?}");

    // A normal card for that name afterwards is an ordinary secret.
    v.request_create(once_card("card-once-next-0123456", "ONCE_CODE")).expect("a new card for the name");
    v.request_answer("card-once-next-0123456", SecretBytes::from_string(PW.to_string()), &readable(), &Actor::Owner).expect("the name is free again");
}

/// A failed write leaves the one-time secret for another try.
#[test]
fn a_one_time_secret_survives_a_failed_write() {
    let t = TestDir::new("ci-once-fail");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let me = Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) };
    v.request_create(once_card("card-once-fail-0123456", "ONCE_FAIL")).expect("card");
    v.request_answer("card-once-fail-0123456", SecretBytes::from_string(PW.to_string()), &readable(), &Actor::Owner).expect("saved");
    let sel = Selector::user(Scope::session("sess-1"), "ONCE_FAIL");
    let e = v.take_once::<()>(&sel, &me, |_| Err(VaultError::internal("disk full"))).expect_err("write failed");
    assert_eq!(e.code, "vault_internal");
    assert_eq!(v.take_once(&sel, &me, |r| Ok(r.value.expose().to_vec())).expect("still there"), PW.as_bytes());
}

/// §3.5: `--once` needs session scope and file delivery at ask time, and a
/// person can't save the card as anything else (it would stop being one-time).
#[test]
fn a_once_card_is_session_and_file_only() {
    let t = TestDir::new("ci-once-terms");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let mut c = once_card("card-once-ws-0123456789", "ONCE_WS");
    c.proposed_scope = "workspace".into();
    assert_eq!(v.request_create(c).expect_err("workspace").code, "once_needs_session_file");
    let mut c = once_card("card-once-fetch-012345", "ONCE_FETCH");
    c.proposed_delivery = Delivery::Fetch;
    c.proposed_hosts = vec!["robot-ws.your-server.de".into()];
    assert_eq!(v.request_create(c).expect_err("fetch").code, "once_needs_session_file");

    v.request_create(once_card("card-once-keep-0123456", "ONCE_KEEP")).expect("card");
    let as_workspace = CardAnswer { scope: Some("workspace".into()), readable_disclosed: true, ..Default::default() };
    let e = v.request_answer("card-once-keep-0123456", SecretBytes::from_string(PW.to_string()), &as_workspace, &Actor::Owner).expect_err("kept for the workspace");
    assert_eq!(e.code, "card_once_terms");
    assert_eq!(v.request_get("card-once-keep-0123456").expect("request").status, "pending", "nothing was saved");
}

/// A later normal card for the name clears the mark, so a value a person
/// meant to keep isn't deleted by one read.
#[test]
fn a_normal_card_clears_the_one_time_mark() {
    let t = TestDir::new("ci-once-clear");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let me = Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) };
    v.request_create(once_card("card-once-mark-0123456", "ONCE_MARK")).expect("card");
    let first = v.request_answer("card-once-mark-0123456", SecretBytes::from_string(PW.to_string()), &readable(), &Actor::Owner).expect("saved").meta;
    let mut normal = once_card("card-once-norm-0123456", "ONCE_MARK");
    normal.once = false;
    v.request_create(normal).expect("card");
    let second = v.request_answer("card-once-norm-0123456", SecretBytes::from_string(PW.to_string()), &readable(), &Actor::Owner).expect("replaced").meta;
    assert_eq!(second.id, first.id);
    assert!(!v.is_once(&second.id));
    v.resolve(&Selector::Id(second.id), Use::File, &me).expect("an ordinary session secret now");
}

/// A rekey re-MACs the one-time marks: a marked secret keeps working through
/// it, and a still-open card stays one-time.
#[test]
fn one_time_marks_survive_a_rekey() {
    let t = TestDir::new("ci-once-rekey");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let me = Requester::Session { workspace_id: "ws1".into(), session_id: Some("sess-1".into()) };
    v.request_create(once_card("card-once-rk-012345678", "ONCE_RK")).expect("card");
    v.request_answer("card-once-rk-012345678", SecretBytes::from_string(PW.to_string()), &readable(), &Actor::Owner).expect("saved");
    v.request_create(once_card("card-once-open-0123456", "ONCE_OPEN")).expect("card");
    v.rekey(&Actor::Owner, &HumanProof::Confirmed { method: "t".into() }).expect("rekey");
    let sel = Selector::user(Scope::session("sess-1"), "ONCE_RK");
    assert_eq!(v.take_once(&sel, &me, |r| Ok(r.value.expose().to_vec())).expect("marks re-MACed"), PW.as_bytes());
    assert!(v.request_get("card-once-open-0123456").expect("request").once);
}

/// §3.4: the notifier wakes a waiter on a card change, and a timeout returns
/// the unchanged generation.
#[test]
fn a_card_change_wakes_a_waiter() {
    let t = TestDir::new("ci-wait");
    let ks = Arc::new(FakeKeyStore::new());
    let v = Arc::new(open(&t, &ks));
    let seen = v.card_generation();
    assert_eq!(v.wait_card_change(seen, std::time::Duration::from_millis(20)), seen, "no change, the timeout ends it");

    v.request_create(once_card("card-wait-0123456789012", "WAIT_ME")).expect("card");
    let after_create = v.card_generation();
    let v2 = v.clone();
    let waiter = std::thread::spawn(move || v2.wait_card_change(after_create, std::time::Duration::from_secs(2)));
    std::thread::sleep(std::time::Duration::from_millis(50));
    v.request_answer("card-wait-0123456789012", SecretBytes::from_string(PW.to_string()), &readable(), &Actor::Owner).expect("saved");
    assert!(waiter.join().expect("join") > after_create, "a saved card wakes the waiter");

    v.request_create(once_card("card-wait-void-01234567", "WAIT_VOID")).expect("card");
    let g = v.card_generation();
    v.request_void("card-wait-void-01234567", &Actor::Owner).expect("void");
    assert!(v.card_generation() > g, "a dismissed card counts");

    v.request_create(once_card("card-wait-old-012345678", "WAIT_OLD")).expect("card");
    v.backdate_request_for_tests("card-wait-old-012345678", 8).expect("backdate");
    let g = v.card_generation();
    v.tick().expect("tick");
    assert!(v.card_generation() > g, "an expired card counts");
}
