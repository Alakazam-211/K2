use super::*;
use crate::test_support::with_temp_home;
use k2_core::federation::{FederationPeer, PeerTrust};

fn body(v: serde_json::Value) -> Vec<u8> {
    v.to_string().into_bytes()
}

/// A remote initiator's offer, built with the core primitive for a
/// fresh key (the other server in these single-HOME tests).
fn remote_offer(sub: &str, base: &str) -> (k2_core::federation::pairing_v2::Offer, String) {
    let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let offer = k2_core::federation::pairing_v2::init(
        &peer_key.public_key_pem(),
        "rosson@laptop",
        sub,
        base,
        chrono::Utc::now(),
    )
    .expect("init");
    let fp = offer.fingerprint.clone();
    (offer, fp)
}

#[test]
fn a_new_v1_pair_request_is_refused_with_the_update_error() {
    with_temp_home(|| {
        // What a 0.45.1 server or app sends to start a pairing.
        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let req = handle_pair_request(&body(serde_json::json!({
            "label": "old", "subdomain": "old", "public_key_pem": peer_key.public_key_pem(),
        })));
        assert_eq!(req.status, "409 Conflict", "{}", req.body);
        assert!(req.body.contains("pairing_v2_required"), "{}", req.body);
        assert!(req.body.contains("Update the server"), "{}", req.body);
        assert!(PeerStore::load().unwrap().list().is_empty(), "nothing pinned");
    });
}

#[test]
fn a_v1_pair_request_never_rewrites_a_pinned_row() {
    with_temp_home(|| {
        // A v2 pairing is in progress: the initiator is pinned Pending.
        let (offer, fp) = remote_offer("alpha", "");
        let resp = handle_pair_v2_respond(&body(serde_json::json!({ "offer": offer.clone() })));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let before = std::fs::read(k2_core::federation::peers::store_path()).expect("store bytes");

        // Someone who knows the public key posts another address.
        let req = handle_pair_request(&body(serde_json::json!({
            "label": "evil",
            "subdomain": "evil",
            "baseUrl": "http://10.9.9.9:38471",
            "public_key_pem": offer.public_key_pem,
        })));
        assert_eq!(req.status, "409 Conflict", "{}", req.body);
        assert!(req.body.contains("pairing_v2_required"), "{}", req.body);
        let after = std::fs::read(k2_core::federation::peers::store_path()).expect("store bytes");
        assert_eq!(before, after, "the Pending v2 row must stay byte-identical");
        let p = PeerStore::load().unwrap().get(&fp).cloned().unwrap();
        assert_eq!(p.subdomain, "alpha");
        assert_eq!(p.pairing_version, 2);

        // A Trusted v1 row gets its v1 code back, still without a write.
        let old_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut v1 = FederationPeer::pin("old", "old", old_key.public_key_pem()).unwrap();
        v1.pairing_version = 1;
        v1.trust = PeerTrust::Trusted;
        let mut store = PeerStore::load().unwrap();
        let v1_fp = store.upsert(v1);
        store.save().unwrap();
        let before = std::fs::read(k2_core::federation::peers::store_path()).expect("store bytes");
        let req = handle_pair_request(&body(serde_json::json!({
            "subdomain": "moved", "public_key_pem": old_key.public_key_pem(),
        })));
        assert_eq!(req.status, "200 OK", "{}", req.body);
        let v: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(v["trust"], "trusted");
        assert_eq!(v["fingerprint"], v1_fp.as_str());
        assert_eq!(std::fs::read(k2_core::federation::peers::store_path()).unwrap(), before);
    });
}

#[test]
fn pair_v2_respond_then_verify_then_owner_confirm_promotes_to_trusted() {
    with_temp_home(|| {
        // This server is the responder; the initiator is a fresh key.
        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let offer = k2_core::federation::pairing_v2::init(
            &peer_key.public_key_pem(),
            "rosson@laptop",
            "rosson",
            "",
            chrono::Utc::now(),
        )
        .expect("init");
        let fp = offer.fingerprint.clone();
        let resp = handle_pair_v2_respond(&body(serde_json::json!({ "offer": offer })));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let answer: k2_core::federation::pairing_v2::Answer =
            serde_json::from_value(serde_json::from_str::<serde_json::Value>(&resp.body).unwrap()["answer"].clone())
                .expect("answer");
        let store = PeerStore::load().unwrap();
        assert_eq!(store.get(&fp).unwrap().trust, PeerTrust::Pending);
        assert_eq!(store.get(&fp).unwrap().pairing_version, 2);

        // The initiator's reveal (computed with the core primitive under
        // its own session) and its code.
        let mut initiator_store = PeerStore::default();
        // (This temp HOME has no tunnel: pin it under an explicit name.)
        let revealed = k2_core::federation::pairing_v2::reveal(
            &mut initiator_store,
            &answer,
            Some("responder"),
            None,
            chrono::Utc::now(),
        )
        .expect("reveal");
        let ver = handle_pair_v2_verify(&body(serde_json::json!({ "reveal": revealed.reveal })));
        assert_eq!(ver.status, "200 OK", "{}", ver.body);
        let v: serde_json::Value = serde_json::from_str(&ver.body).unwrap();
        assert_eq!(v["sas"], revealed.sas.as_str(), "both sides show the same code");
        assert_eq!(v["peer_fingerprint"], fp.as_str());

        let conf = handle_pair_confirm(&body(serde_json::json!({
            "fingerprint": fp,
            "sas": revealed.sas,
        })));
        assert_eq!(conf.status, "200 OK", "confirm body: {}", conf.body);
        let store = PeerStore::load().unwrap();
        let p = store.get(&fp).unwrap();
        assert_eq!(p.trust, PeerTrust::Trusted);
        assert_eq!(p.pairing_version, 2);
        assert!(p.capabilities.contains("inbound"));
        let peers: serde_json::Value = serde_json::from_str(&handle_peers().body).unwrap();
        assert_eq!(peers["peers"][0]["repair_recommended"], false);
    });
}

#[test]
fn pair_confirm_rejects_wrong_sas() {
    with_temp_home(|| {
        let (offer, fp) = remote_offer("p", "");
        let resp = handle_pair_v2_respond(&body(serde_json::json!({ "offer": offer })));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let conf = handle_pair_confirm(&body(serde_json::json!({
            "fingerprint": fp, "sas": "000000",
        })));
        assert_eq!(conf.status, "403 Forbidden", "no code is waiting (verify never ran) → 403");
        // Still Pending.
        let store = PeerStore::load().unwrap();
        assert_eq!(store.get(&fp).unwrap().trust, PeerTrust::Pending);
    });
}

#[test]
fn a_tampered_reveal_aborts_the_pairing() {
    with_temp_home(|| {
        let (offer, _fp) = remote_offer("p", "");
        let resp = handle_pair_v2_respond(&body(serde_json::json!({ "offer": offer.clone() })));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let bad = k2_core::federation::pairing_v2::Reveal {
            v: 2,
            session: offer.session.clone(),
            fingerprint: offer.fingerprint.clone(),
            nonce: "ab".repeat(32),
        };
        let ver = handle_pair_v2_verify(&body(serde_json::json!({ "reveal": bad })));
        assert_eq!(ver.status, "403 Forbidden", "{}", ver.body);
        assert!(ver.body.contains("commitment_mismatch"), "{}", ver.body);
    });
}

#[test]
fn v1_peers_are_flagged_for_re_pairing() {
    with_temp_home(|| {
        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut p = FederationPeer::pin("old", "old", peer_key.public_key_pem()).unwrap();
        p.pairing_version = 1;
        p.trust = PeerTrust::Trusted;
        let mut store = PeerStore::default();
        store.upsert(p);
        store.save().unwrap();
        let peers: serde_json::Value = serde_json::from_str(&handle_peers().body).unwrap();
        assert_eq!(peers["peers"][0]["pairing_version"], 1);
        assert_eq!(peers["peers"][0]["repair_recommended"], true);
    });
}

#[test]
fn inbound_rejects_unknown_peer() {
    with_temp_home(|| {
        // Seal an envelope from a key nobody pinned.
        let key = k2_core::federation::identity::load_or_create().unwrap();
        let signal = AgentSignal::new(
            AgentAddress::Broadcast,
            AgentAddress::Agent {
                workspace: WorkspaceId("ws".into()),
                name: "bob".into(),
            },
            SignalKind::Msg { text: "hi".into() },
        );
        let bytes = federation::seal(&signal, &key, "peer", 8).unwrap();
        let resp = handle_inbound(&bytes);
        assert_eq!(resp.status, "403 Forbidden", "unknown peer must reject: {}", resp.body);
    });
}

/// Register a local project row; returns its UUID. The verified `to`
/// address carries this UUID and `handle_inbound` resolves it → path.
///
/// The workspace is one this server lists to peers (the receiver gate
/// refuses anything else): its persona names the agent `bob` (the name
/// `seal_msg_to` / `seal_tray_to` address) and "Allow agents to create
/// connections" is on. `agent_enabled` stays 0, so live delivery reports
/// `no_agent_mode` and never wakes a session.
fn register_project(path: &str) -> String {
    let persona = k2_core::workspace::agent_identity::workspace_agent_md_path(path);
    std::fs::create_dir_all(persona.parent().unwrap()).unwrap();
    std::fs::write(&persona, "---\nname: bob\n---\n# bob\n").unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, agents_can_create_connections) \
             VALUES (?1, ?2, ?3, 1)",
        rusqlite::params![id, "fed-recv-ws", path],
    )
    .unwrap();
    id
}

/// A unique temp workspace path (not created on disk; inbox::compose
/// makes the `.k2/inbox/` dir on demand).
fn temp_ws_path() -> String {
    std::env::temp_dir()
        .join(format!("k2-fed-recv-{}-{}", std::process::id(), uuid::Uuid::new_v4()))
        .to_string_lossy()
        .into_owned()
}

/// Pin a DISTINCT peer key (not the local key, or the loop guard trips
/// on self-in-trace) as Trusted+inbound under `subdomain`. Returns the
/// peer keypair so the caller can seal an envelope with it.
fn pin_trusted_peer(subdomain: &str) -> rcgen::KeyPair {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::default();
    let fp = store.upsert(FederationPeer::pin("peer", subdomain, key.public_key_pem()).unwrap());
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();
    key
}

/// Seal `text` from `cortana` (peer-attributed at inbound as
/// `cortana::<host>`) addressed to agent `bob` in `ws_id`.
fn seal_msg_to(ws_id: &str, key: &rcgen::KeyPair, subdomain: &str, text: &str) -> Vec<u8> {
    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("/src/cortana".into()),
            name: "cortana".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId(ws_id.into()),
            name: "bob".into(),
        },
        SignalKind::Msg { text: text.into() },
    );
    federation::seal(&signal, key, subdomain, 8).unwrap()
}

// ── P3 receive: opt-in OFF → DECLINE. "Allow remote instruction = OFF"
//    means remote messages must NOT instruct this agent, and the
//    `.k2/inbox/` is inherently TRUSTED work — so the verified message is
//    written NOWHERE trusted and the sender is told it did not land
//    (200, `delivered:false`, `mode:"declined"`). The security gate still
//    passed (the peer is Trusted+inbound); only the trusted-inbox write is
//    gone.
#[test]
fn inbound_opt_in_off_declines_no_trusted_inbox() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let ws_path = temp_ws_path();
        let ws_id = register_project(&ws_path); // opt-in defaults OFF

        let key = pin_trusted_peer("peer");
        let bytes = seal_msg_to(&ws_id, &key, "peer", "hello over the wire");
        let resp = handle_inbound(&bytes);
        // Received-but-not-delivered: the dial succeeds so the sender can
        // tell a clean decline from a transport/security failure.
        assert_eq!(resp.status, "200 OK", "valid envelope dial must succeed: {}", resp.body);

        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["delivered"], false, "opt-in OFF must NOT report delivered");
        assert_eq!(v["mode"], "declined", "opt-in OFF must decline delivery");
        assert_eq!(
            v["reason"], "remote_instruction_disabled",
            "decline must carry the explicit reason"
        );

        // NOTHING was written to the TRUSTED inbox under `<ws>/.k2/inbox/`.
        let items = k2_core::inbox::list_root(Path::new(&ws_path));
        assert!(
            items.is_empty(),
            "opt-in OFF must NOT write the trusted inbox, found {}",
            items.len()
        );
    });
}

// ── P3 receive: opt-in ON → drive the canonical chat via the SAME
//    `deliver_live` local `k2 msg` uses (NOT the inbox). Here the
//    workspace has no agent/session so deliver_live reports
//    `no_agent_mode` — but crucially it took the LIVE path and wrote NO
//    inbox file (the orphaned-awareness-inbox bug regression-lock).
#[test]
fn inbound_opt_in_on_routes_to_live_delivery() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let ws_path = temp_ws_path();
        let ws_id = register_project(&ws_path);
        // Opt THIS workspace in to remote instruction.
        k2_core::workspace::settings::update_project_setting(
            &ws_path,
            "allow_remote_instruct",
            "1",
        )
        .expect("opt in");

        let key = pin_trusted_peer("peer");
        let bytes = seal_msg_to(&ws_id, &key, "peer", "drive my chat");
        let resp = handle_inbound(&bytes);
        assert_eq!(resp.status, "200 OK", "must deliver: {}", resp.body);

        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["mode"], "live", "opt-in ON must route to the canonical chat");

        // The LIVE path was taken — nothing was written to the inbox.
        let items = k2_core::inbox::list_root(Path::new(&ws_path));
        assert!(items.is_empty(), "opt-in ON must NOT write an inbox file");
    });
}

// ── 2026-10-16: the switch covers agents on other servers only ──────
//
// One agent, one switch: off → not in the roster a peer sees and live
// text is declined; on → listed and delivered (the LIVE path).
#[test]
fn federation_switch_off_declines_and_hides_on_delivers_and_lists() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        k2_core::app_settings::update(serde_json::json!({ "allowRemoteInstruct": false }))
            .expect("master off");
        // `register_project`: persona `bob`, connections on, switch off,
        // no agent_enabled → the live path ends `no_agent_mode` (never
        // spawns a CLI).
        let ws_path = temp_ws_path();
        let ws_id = register_project(&ws_path);
        let key = pin_trusted_peer("peer");
        let listed = |id: &str| {
            k2_core::federation::roster::build_local_roster()
                .agents
                .iter()
                .any(|a| a.workspace_id == id)
        };

        // OFF: hidden from the listing, live text declined.
        assert!(!listed(&ws_id), "switch off must not be listed");
        let resp = handle_inbound(&seal_msg_to(&ws_id, &key, "peer", "off"));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["delivered"], false, "{}", resp.body);
        assert_eq!(v["mode"], "declined", "{}", resp.body);
        assert_eq!(v["reason"], "remote_instruction_disabled", "{}", resp.body);

        // ON: listed, live text takes the LIVE path (never declined).
        k2_core::workspace::settings::update_project_setting(&ws_path, "allow_remote_instruct", "1")
            .expect("switch on");
        assert!(listed(&ws_id), "switch on must be listed");
        let resp = handle_inbound(&seal_msg_to(&ws_id, &key, "peer", "on"));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["mode"], "live", "switch on must deliver live: {}", resp.body);
        assert!(v.get("reason").is_none(), "no decline reason when on: {}", resp.body);
    });
}

/// Upgrade safety: a workspace opted in ONLY via the app master (its own
/// column 0) keeps the same effective switch after 2026-10-16 — same
/// column, same master — so it stays listed and its federated links keep
/// delivering exactly as before.
#[test]
fn upgrade_master_opted_in_workspace_keeps_effective_switch() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let ws_path = temp_ws_path();
        let ws_id = register_project(&ws_path);
        // Neither per-workspace flag: only the master can opt it in.
        k2_core::workspace::settings::update_project_setting(
            &ws_path,
            "agents_can_create_connections",
            "0",
        )
        .expect("connections off");
        // The master is ON (as on Rosson's server); the column stays 0.
        k2_core::app_settings::update(serde_json::json!({ "allowRemoteInstruct": true }))
            .expect("master on");
        assert!(!k2_core::workspace::settings::get_allow_remote_instruct(&ws_path));
        assert!(
            k2_core::workspace::settings::remote_instruct_allowed_for_path(&ws_path),
            "effective switch must stay ON via the master"
        );
        let projection = k2_core::federation::roster::build_local_roster();
        assert!(
            projection.agents.iter().any(|a| a.workspace_id == ws_id),
            "master-opted-in agent must stay listed: {:?}",
            projection.agents
        );
        let key = pin_trusted_peer("peer");
        let resp = handle_inbound(&seal_msg_to(&ws_id, &key, "peer", "still works"));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["mode"], "live", "must not be declined: {}", resp.body);

        // And the master is the only thing doing it: off → hidden (404).
        k2_core::app_settings::update(serde_json::json!({ "allowRemoteInstruct": false }))
            .expect("master off");
        assert!(!k2_core::workspace::settings::remote_instruct_allowed_for_path(&ws_path));
        let resp = handle_inbound(&seal_msg_to(&ws_id, &key, "peer", "now hidden"));
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
    });
}

// ── H0 / H4 (prd-agent-secret-handoff-v1 test 11): a `vault-*` kind on
//    `/inbound` is refused with 400 `wrong_route`, even to an opted-in
//    workspace, and nothing is injected, inboxed or marked seen.
#[test]
fn inbound_refuses_vault_kinds_with_wrong_route() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let ws_path = temp_ws_path();
        let ws_id = register_project(&ws_path);
        k2_core::workspace::settings::update_project_setting(&ws_path, "allow_remote_instruct", "1")
            .expect("opt in");
        let key = pin_trusted_peer("peer");
        for kind in ["vault-handoff/1", "vault-recipient/1", "vault-anything"] {
            let signal = AgentSignal::new(
                AgentAddress::Agent { workspace: WorkspaceId("src".into()), name: "cortana".into() },
                AgentAddress::Agent { workspace: WorkspaceId(ws_id.clone()), name: "bob".into() },
                SignalKind::Custom {
                    kind: kind.to_string(),
                    payload: serde_json::json!({ "sealedB64": "AAAA", "deliveryId": "hd_x" }),
                },
            );
            let bytes = federation::seal(&signal, &key, "peer", 8).unwrap();
            let msg_uuid = serde_json::from_slice::<federation::FederationEnvelope>(&bytes).unwrap().payload.msg_uuid;
            let resp = handle_inbound(&bytes);
            assert_eq!(resp.status, "400 Bad Request", "{kind}: {}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
            assert_eq!(v["code"], "wrong_route", "{kind}");
            let fp = k2_core::federation::peers::fingerprint_of_spki_pem(&key.public_key_pem()).unwrap();
            assert!(!k2_core::federation::seen::already_seen(&fp, &msg_uuid), "a refused vault kind is not recorded");
        }
        assert!(k2_core::inbox::list_root(Path::new(&ws_path)).is_empty(), "nothing inboxed");
    });
}

fn seal_tray_to(
    ws_id: &str,
    key: &rcgen::KeyPair,
    subdomain: &str,
    spec: federation::TraySpec,
) -> Vec<u8> {
    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("/src/cortana".into()),
            name: "cortana".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId(ws_id.into()),
            name: "bob".into(),
        },
        SignalKind::Custom {
            kind: federation::TRAY_SIGNAL_KIND.to_string(),
            payload: serde_json::to_value(&spec).unwrap(),
        },
    )
    .with_delivery(Delivery::Inbox);
    federation::seal(&signal, key, subdomain, 8).unwrap()
}

fn brief_tray() -> federation::TraySpec {
    federation::encode_tray_spec(
        Some("Ship it"),
        Some("please review"),
        true,
        &[("brief.md".to_string(), b"# Ship it\n\nDo the thing.\n".to_vec())],
    )
    .unwrap()
}

#[test]
fn inbound_tray_lands_inbox_when_opt_in_off() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let ws_path = temp_ws_path();
        std::fs::create_dir_all(&ws_path).unwrap();
        let ws_id = register_project(&ws_path); // opt-in defaults OFF

        let key = pin_trusted_peer("peer");
        let bytes = seal_tray_to(&ws_id, &key, "peer", brief_tray());
        let resp = handle_inbound(&bytes);
        assert_eq!(resp.status, "200 OK", "tray inbound: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["delivered"], true, "{}", resp.body);
        assert_eq!(v["mode"], "inbox", "{}", resp.body);
        assert_eq!(v["ok"], true);
        assert!(v["id"].as_str().unwrap_or("").len() > 0);
        assert_eq!(v["wake"]["success"], false);
        assert_eq!(v["wake"]["reason"], "remote_instruction_disabled");

        let items = k2_core::inbox::list_root(Path::new(&ws_path));
        assert_eq!(items.len(), 1, "tray must land in .k2/inbox/");
        let content = std::fs::read_to_string(
            k2_core::inbox::inbox_root(Path::new(&ws_path)).join(&items[0].filename),
        )
        .expect("cover md readable");
        assert!(content.contains("Do the thing."), "{content}");
        assert!(content.contains("please review"), "{content}");
        assert!(
            !content.contains("bytes_b64"),
            "must not dump wire encoding into the cover"
        );
    });
}

#[test]
fn inbound_tray_does_not_dump_file_into_live_path() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let ws_path = temp_ws_path();
        std::fs::create_dir_all(&ws_path).unwrap();
        let ws_id = register_project(&ws_path);
        k2_core::workspace::settings::update_project_setting(
            &ws_path,
            "allow_remote_instruct",
            "1",
        )
        .expect("opt in");

        let key = pin_trusted_peer("peer");
        let bytes = seal_tray_to(&ws_id, &key, "peer", brief_tray());
        let resp = handle_inbound(&bytes);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["mode"], "inbox");
        assert_eq!(v["ok"], true);
        // Wake may fail (no agent) but must be a POINTER attempt, not file dump.
        if let Some(hint) = v["wake"]["hint"].as_str() {
            assert!(
                !hint.contains("Do the thing."),
                "wake hint must not contain file body: {hint}"
            );
        }
        let items = k2_core::inbox::list_root(Path::new(&ws_path));
        assert_eq!(items.len(), 1, "package must land");
    });
}

#[test]
fn send_tray_over_cap_fails_loud() {
    with_temp_home(|| {
        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::default();
        let fp = store.upsert(
            FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap(),
        );
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();

        let too_big = vec![0u8; (federation::TRAY_MAX_BYTES as usize) + 8];
        let spec = federation::TraySpec {
            title: Some("huge".into()),
            wake: true,
            files: vec![federation::TrayFile {
                name: "huge.bin".into(),
                bytes_b64: {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.encode(&too_big)
                },
            }],
            ..federation::TraySpec::default()
        };
        let send_body = body(serde_json::json!({
            "to": "peer::ws::bob",
            "tray": spec,
        }));
        let resp = handle_send(&send_body);
        assert!(
            resp.status.starts_with("400"),
            "over-cap tray must fail loud: {} {}",
            resp.status,
            resp.body
        );
        assert!(
            resp.body.contains("v1 cap") || resp.body.contains("cap"),
            "must mention the size cap: {}",
            resp.body
        );
        assert!(
            outbox::list_for_peer(&fp).is_empty(),
            "over-cap must not enqueue"
        );
    });
}

// ── P3 receive: durable msg_uuid dedupe. The outbox drain is
//    at-least-once — a redelivery of an already-handled envelope (same
//    SIGNED msg_uuid, possibly re-sealed with a fresh nonce so the
//    in-memory replay LRU can't catch it) must 409 as a duplicate
//    instead of driving the chat / decline path twice. The drain maps
//    that 409 back to "already delivered".
#[test]
fn inbound_redelivery_of_handled_msg_uuid_is_409_duplicate() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let ws_path = temp_ws_path();
        let ws_id = register_project(&ws_path); // opt-in OFF → decline (terminal)

        let key = pin_trusted_peer("peer");
        let bytes = seal_msg_to(&ws_id, &key, "peer", "deliver once");
        let first = handle_inbound(&bytes);
        assert_eq!(first.status, "200 OK", "first delivery: {}", first.body);
        let v: serde_json::Value = serde_json::from_str(&first.body).unwrap();
        assert_eq!(v["mode"], "declined", "opt-in OFF declines (terminal)");

        // Redeliver the SAME message RE-SEALED (fresh nonce + at — the
        // exact thing the drain does for stale envelopes), so only the
        // durable msg_uuid store can catch it.
        let resealed = federation::reseal(&bytes, &key).expect("reseal");
        let second = handle_inbound(&resealed);
        assert_eq!(
            second.status, "409 Conflict",
            "redelivered msg_uuid must dedupe: {}",
            second.body
        );
        assert!(
            second.body.contains("duplicate message"),
            "the dedupe reject must carry the drain's marker: {}",
            second.body
        );
    });
}

// ── P3 receive: a message addressed to a workspace NOT on this server is
//    rejected (resolution failure, after the security gate passed).
#[test]
fn inbound_unknown_workspace_is_rejected() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let key = pin_trusted_peer("peer");
        // No project registered for this UUID.
        let bytes = seal_msg_to(&uuid::Uuid::new_v4().to_string(), &key, "peer", "nowhere");
        let resp = handle_inbound(&bytes);
        assert_eq!(resp.status, "404 Not Found", "unknown workspace must reject: {}", resp.body);
    });
}

// ── Receiver gate: the target workspace and name must match ──

/// How a test workspace is exposed under this box's roster rules.
#[derive(Clone, Copy)]
///
/// Remote Access (live drive) is deliberately not used here: it would
/// route a delivered message to `deliver_live`, which may wake a session.
/// "Connections only" lists the workspace without that path.
enum Exposure {
    /// "Let agents on other servers message this agent" ON — listed in
    /// the roster; live text and trays deliver.
    Listed,
    /// Only "Allow agents to create connections" ON. Not listed in the
    /// roster since 2026-10-16, but a direct ask gets the same answer as
    /// before: live text is declined, a tray lands.
    ConnectionsOnly,
    /// Neither flag (and the app master is off under the temp HOME) →
    /// hidden from the roster.
    Hidden,
}

/// Register a CONFIGURED workspace (`agent_enabled = 1`) with an
/// explicit display name + handle and the given exposure. Returns
/// (id, path). The path exists on disk so tray packages can land.
fn register_agent_ws(name: &str, handle: &str, exposure: Exposure) -> (String, String) {
    let path = temp_ws_path();
    std::fs::create_dir_all(&path).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let (remote, conns) = match exposure {
        Exposure::Listed => (1, 0),
        Exposure::ConnectionsOnly => (0, 1),
        Exposure::Hidden => (0, 0),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle, agent_enabled, \
             allow_remote_instruct, agents_can_create_connections) \
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
        rusqlite::params![id, name, path, handle, remote, conns],
    )
    .unwrap();
    (id, path)
}

/// Seal a live `Msg` addressed to `(ws_id, to_name)` from a pinned peer.
fn seal_msg_named(ws_id: &str, to_name: &str, key: &rcgen::KeyPair, text: &str) -> Vec<u8> {
    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("/src/cortana".into()),
            name: "cortana".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId(ws_id.into()),
            name: to_name.into(),
        },
        SignalKind::Msg { text: text.into() },
    );
    federation::seal(&signal, key, "peer", 8).unwrap()
}

/// Seal a tray package addressed to `(ws_id, to_name)` from a pinned peer.
fn seal_tray_named(ws_id: &str, to_name: &str, key: &rcgen::KeyPair) -> Vec<u8> {
    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("/src/cortana".into()),
            name: "cortana".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId(ws_id.into()),
            name: to_name.into(),
        },
        SignalKind::Custom {
            kind: federation::TRAY_SIGNAL_KIND.to_string(),
            payload: serde_json::to_value(&brief_tray()).unwrap(),
        },
    )
    .with_delivery(Delivery::Inbox);
    federation::seal(&signal, key, "peer", 8).unwrap()
}

/// The workspace id and the agent name in `to` must belong together:
/// a name from one workspace with another workspace's id is refused.
#[test]
fn inbound_refuses_name_that_belongs_to_a_different_workspace() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let (_seoca_id, _seoca_path) =
            register_agent_ws("Seoca", "seoca", Exposure::ConnectionsOnly);
        let (x_id, x_path) =
            register_agent_ws("Other", "other-agent", Exposure::ConnectionsOnly);

        let key = pin_trusted_peer("peer");
        // Live text: name = seoca, workspace = X.
        let bytes = seal_msg_named(&x_id, "seoca", &key, "not for you");
        let resp = handle_inbound(&bytes);
        assert_eq!(resp.status, "404 Not Found", "mismatched id must be refused: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["code"], "unknown_agent", "{}", resp.body);

        // Tray package: same mismatched pair → refused, nothing lands in X.
        let bytes = seal_tray_named(&x_id, "seoca", &key);
        let resp = handle_inbound(&bytes);
        assert_eq!(resp.status, "404 Not Found", "mismatched tray must be refused: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["code"], "unknown_agent", "{}", resp.body);
        assert!(
            k2_core::inbox::list_root(Path::new(&x_path)).is_empty(),
            "a refused tray must not write X's inbox"
        );
    });
}

/// Short unique suffix: the test DB is shared, and handles/aliases are
/// unique across it.
fn uniq() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// Every name a correct sender may put in `to.name` still delivers:
/// the handle, an alias (an old handle), the display name with spaces,
/// the persona's agent name, and each sender fold (`cli/k2` drops `.`,
/// the daemon tray resolver turns it into `-`).
#[test]
fn inbound_accepts_every_name_of_the_addressed_workspace() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let u = uniq();
        let handle = format!("quahog-website-{u}");
        let display = format!("Seoca Press {u}");
        let alias = format!("seoca.old.{u}");
        let persona_name = format!("seoca-{u}");
        let (w_id, w_path) = register_agent_ws(&display, &handle, Exposure::ConnectionsOnly);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace::handle::insert_alias_or_ignore(&conn, &w_id, &alias);
            assert_eq!(
                k2_core::workspace::handle::aliases_for(&conn, &w_id),
                vec![alias.clone()],
                "alias fixture must be stored"
            );
        }
        let persona = k2_core::workspace::agent_identity::workspace_agent_md_path(&w_path);
        std::fs::create_dir_all(persona.parent().unwrap()).unwrap();
        std::fs::write(&persona, format!("---\nname: {persona_name}\n---\n")).unwrap();

        let names = vec![
            handle.clone(),                       // handle
            handle.to_ascii_uppercase(),          // case
            alias.clone(),                        // alias, as stored
            format!("seoca-old-{u}"),             // alias, daemon tray fold
            format!("seocaold{u}"),               // alias, cli/k2 fold
            display.clone(),                      // display name with spaces
            format!("seoca-press-{u}"),           // display name, slugged
            persona_name.clone(),                 // resolved agent name
        ];
        let key = pin_trusted_peer("peer");
        for (i, name) in names.iter().enumerate() {
            let bytes = seal_tray_named(&w_id, name, &key);
            let resp = handle_inbound(&bytes);
            assert_eq!(resp.status, "200 OK", "name {name:?} must deliver: {}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
            assert_eq!(v["mode"], "inbox", "name {name:?}: {}", resp.body);
            assert_eq!(v["delivered"], true, "name {name:?}: {}", resp.body);
            assert_eq!(
                k2_core::inbox::list_root(Path::new(&w_path)).len(),
                i + 1,
                "name {name:?}: one package per delivery"
            );
        }

        // Live text by handle passes the gate too (declined only because
        // Remote Access is off — the consent gate, not the receiver gate).
        let bytes = seal_msg_named(&w_id, &handle, &key, "hello");
        let resp = handle_inbound(&bytes);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["mode"], "declined", "{}", resp.body);
    });
}

/// A name that is merely close (`sales` vs `sales-team`, D20) is not one
/// of the workspace's names.
#[test]
fn inbound_refuses_a_near_miss_name() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let u = uniq();
        let (w_id, w_path) =
            register_agent_ws("Sales Team", &format!("sales-team-{u}"), Exposure::ConnectionsOnly);
        let key = pin_trusted_peer("peer");
        let bytes = seal_tray_named(&w_id, &format!("sales-{u}"), &key);
        let resp = handle_inbound(&bytes);
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["code"], "unknown_agent", "{}", resp.body);
        assert!(k2_core::inbox::list_root(Path::new(&w_path)).is_empty());
    });
}

/// 0159 receiver side: the old workspace "sales-<u>" was removed and a
/// new one has that display name (handle "sales-<u>-2"). An older
/// sender that re-bound `sales-<u>::host` by name to the new id is
/// refused (same 404 as any unknown name); the new workspace's own
/// handle still delivers. The published roster never lists the
/// retired handle as the new workspace's alias.
#[test]
fn inbound_refuses_a_retired_handle_addressed_to_a_new_workspace() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let u = uniq();
        let old_handle = format!("sales-{u}");
        let (old_id, _old_path) =
            register_agent_ws("Old Sales", &old_handle, Exposure::ConnectionsOnly);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![old_id])
                .unwrap();
            assert!(
                k2_core::workspace::handle::retired_handle(&conn, &old_handle).is_some(),
                "delete must retire the handle"
            );
        }
        let new_handle = format!("{old_handle}-2");
        let (new_id, new_path) =
            register_agent_ws(&old_handle, &new_handle, Exposure::Listed);
        {
            // Slip the retired name in as an alias behind the guard's
            // back: the roster must still not publish it.
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO project_handle_aliases (project_id, alias) VALUES (?1, ?2)",
                rusqlite::params![new_id, old_handle],
            )
            .unwrap();
        }
        let roster = k2_core::federation::roster::build_local_roster();
        let entry = roster
            .agents
            .iter()
            .find(|a| a.workspace_id == new_id)
            .expect("new workspace is listed");
        assert_eq!(entry.agent, new_handle);
        assert!(
            !entry.aliases.iter().any(|a| a.eq_ignore_ascii_case(&old_handle)),
            "retired handle published as an alias: {:?}",
            entry.aliases
        );
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "DELETE FROM project_handle_aliases WHERE project_id = ?1",
                rusqlite::params![new_id],
            )
            .unwrap();
        }

        let key = pin_trusted_peer("peer");
        for name in [old_handle.clone(), old_handle.to_ascii_uppercase()] {
            let resp = handle_inbound(&seal_tray_named(&new_id, &name, &key));
            assert_eq!(resp.status, "404 Not Found", "{name}: {}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
            assert_eq!(v["code"], "unknown_agent", "{}", resp.body);
        }
        assert!(k2_core::inbox::list_root(Path::new(&new_path)).is_empty());

        let resp = handle_inbound(&seal_tray_named(&new_id, &new_handle, &key));
        assert_eq!(resp.status, "200 OK", "own handle delivers: {}", resp.body);
        assert_eq!(k2_core::inbox::list_root(Path::new(&new_path)).len(), 1);
    });
}

/// A workspace this server does not list to peers is refused by id even
/// with its own correct handle — live text and tray — and the answer is
/// the same as for an id that does not exist.
#[test]
fn inbound_refuses_hidden_workspace_even_with_its_own_name() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let u = uniq();
        let handle = format!("hidden-{u}");
        let (h_id, h_path) = register_agent_ws("Hidden", &handle, Exposure::Hidden);
        assert!(
            !k2_core::federation::roster::is_visible(&h_path),
            "fixture must be hidden from the roster"
        );
        let key = pin_trusted_peer("peer");

        let resp = handle_inbound(&seal_msg_named(&h_id, &handle, &key, "hi"));
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        let hidden_body = resp.body.clone();

        let resp = handle_inbound(&seal_tray_named(&h_id, &handle, &key));
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert!(
            k2_core::inbox::list_root(Path::new(&h_path)).is_empty(),
            "a hidden workspace must not receive a tray package"
        );

        let resp = handle_inbound(&seal_msg_named(
            &uuid::Uuid::new_v4().to_string(),
            &handle,
            &key,
            "hi",
        ));
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert_eq!(
            resp.body, hidden_body,
            "hidden and unregistered must answer identically"
        );
        assert_eq!(resp.body, unknown_agent_response().body);
    });
}

/// A refusal is terminal: the message id is not recorded as seen, so it
/// is the sender's drain (which dead-letters 4xx) that ends it — and a
/// later, correctly addressed message is unaffected.
#[test]
fn inbound_refusal_is_not_recorded_as_delivered() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let u = uniq();
        let (x_id, _x_path) =
            register_agent_ws("X", &format!("x-{u}"), Exposure::ConnectionsOnly);
        let key = pin_trusted_peer("peer");
        let bytes = seal_msg_named(&x_id, &format!("someone-else-{u}"), &key, "hi");
        let env: federation::FederationEnvelope = serde_json::from_slice(&bytes).unwrap();
        let resp = handle_inbound(&bytes);
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        let store = PeerStore::load().unwrap();
        let fp = store.list()[0].fingerprint.clone();
        assert!(
            !k2_core::federation::seen::already_seen(&fp, &env.payload.msg_uuid),
            "a refused message must not be marked delivered"
        );
    });
}

// ── P4 outbound: seal + durable enqueue + dial + POST over the wire ──

/// Stand up a loopback stub mimicking a peer's
/// `/cli/federation/inbound`. Captures the first inbound body, answers
/// 200, and ships the body back via a oneshot. Its roster route answers
/// 404 (a peer whose roster can't be read), so a send gate's roster
/// fetch for an unbound row falls back to the name rule. Returns the
/// bound port + the receiver.
async fn spawn_inbound_stub() -> (u16, tokio::sync::oneshot::Receiver<Vec<u8>>) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = std::sync::Mutex::new(Some(tx));
    let stub = spawn_peer_stub(None, "ok", move |b| {
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(b);
        }
    });
    (stub.port, rx)
}

/// Variant of [`spawn_inbound_stub`] that answers 200 with a CUSTOM JSON
/// body — used to simulate the receive-side DECLINE (200 `delivered:false`).
async fn spawn_inbound_stub_responding(resp_body: &'static str) -> u16 {
    spawn_peer_stub(None, resp_body, |_| {}).port
}

/// A loopback stand-in for a peer daemon (std thread, many
/// connections). `GET /cli/federation/roster` answers `roster` (404
/// when `None`, like a peer that can't be read); `POST
/// /cli/federation/inbound` answers `inbound_resp` and hands each body
/// to `on_inbound`. Counts roster reads.
struct PeerStub {
    port: u16,
    roster_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

fn spawn_peer_stub(
    roster: Option<String>,
    inbound_resp: &'static str,
    on_inbound: impl Fn(Vec<u8>) + Send + 'static,
) -> PeerStub {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind peer stub");
    let port = l.local_addr().unwrap().port();
    let roster_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits = roster_hits.clone();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut acc: Vec<u8> = Vec::new();
            let mut buf = [0u8; 4096];
            let mut head_end: Option<usize> = None;
            let mut clen = 0usize;
            loop {
                let n = match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                acc.extend_from_slice(&buf[..n]);
                if head_end.is_none() {
                    if let Some(pos) = acc.windows(4).position(|w| w == b"\r\n\r\n") {
                        head_end = Some(pos + 4);
                        let head = String::from_utf8_lossy(&acc[..pos]).to_string();
                        clen = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                    }
                }
                if let Some(he) = head_end {
                    if acc.len() >= he + clen {
                        break;
                    }
                }
            }
            let Some(he) = head_end else { continue };
            let request_line = String::from_utf8_lossy(&acc[..he])
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            let (status, body) = if request_line.starts_with("GET /cli/federation/roster") {
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                match &roster {
                    Some(r) => ("200 OK", r.clone()),
                    None => ("404 Not Found", r#"{"error":"not found"}"#.to_string()),
                }
            } else if request_line.starts_with("POST /cli/federation/inbound") {
                on_inbound(acc[he..].to_vec());
                ("200 OK", inbound_resp.to_string())
            } else {
                ("404 Not Found", r#"{"error":"not found"}"#.to_string())
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes());
            let _ = s.flush();
        }
    });
    PeerStub { port, roster_hits }
}

/// A roster body (the peer's `/cli/federation/roster` projection).
fn roster_json(entries: &[(&str, &str, &str, &[&str])]) -> String {
    let agents: Vec<serde_json::Value> = entries
        .iter()
        .map(|(ws, handle, display, aliases)| {
            serde_json::json!({
                "workspace_id": ws,
                "workspace_name": display,
                "agent": handle,
                "address": format!("{ws}::{handle}"),
                "aliases": aliases,
            })
        })
        .collect();
    serde_json::json!({ "agents": agents }).to_string()
}

// ── P4 send: a receive-side DECLINE (200 `delivered:false`) must surface
//    to the SENDER as `status:"declined"`, NOT `"sent"`. Regression lock
//    for the 0.40.20 bug where a remote-instruction-OFF decline was
//    reported to the sender as a successful send.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_surfaces_peer_decline_as_declined_not_sent() {
    let _home = crate::test_support::TempHome::new();

    let port = spawn_inbound_stub_responding(
        r#"{"delivered":false,"mode":"declined","reason":"remote_instruction_disabled"}"#,
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");

    let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::default();
    let fp = store.upsert(
        FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap(),
    );
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();

    let inbound_base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", &base);
    let send_body = body(serde_json::json!({ "to": "peer::ws-b::bob", "text": "are you in?" }));
    let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
        .await
        .unwrap();
    drop(inbound_base);

    assert_eq!(resp.status, "200 OK", "send body: {}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(
        v["status"], "declined",
        "a peer decline must NOT be reported as sent; got {}",
        resp.body
    );
    assert_eq!(v["delivered"], false, "declined send must report delivered:false");
    assert_eq!(
        v["reason"], "remote_instruction_disabled",
        "decline must propagate the peer's reason"
    );

    // A decline is terminal — the queued copy is consumed (re-dialing
    // would never flip the recipient's opt-out).
    assert!(
        outbox::list_for_peer(&fp).is_empty(),
        "a clean decline must drain the queued copy (not retry forever)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_seals_enqueues_and_posts_to_peer_inbound() {
    // Hold the crate-wide HOME lock for the whole test (TempHome) so the
    // keypair/store/outbox live in an isolated ~/.k2 and the env override is
    // serialized against other HOME-swapping tests.
    let _home = crate::test_support::TempHome::new();

    let (port, rx) = spawn_inbound_stub().await;
    let base = format!("http://127.0.0.1:{port}");

    // Pin a Trusted peer (subdomain "peer") with a fresh key, and capture
    // OUR local pubkey to verify the sealed envelope the stub receives.
    let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::default();
    let fp = store.upsert(
        FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap(),
    );
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();

    let local_key = k2_core::federation::identity::load_or_create().unwrap();
    let local_pem = local_key.public_key_pem();

    // Run the blocking send on a worker (reqwest::blocking can't run on the
    // async thread). Override the dial target to our loopback stub.
    let inbound_base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", &base);
    let send_body = body(serde_json::json!({ "to": "peer::ws-b::bob", "text": "hi over the wire" }));
    let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
        .await
        .unwrap();
    drop(inbound_base);

    assert_eq!(resp.status, "200 OK", "send body: {}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(v["status"], "sent", "must report sent on a 200 from the peer");

    // The peer received the EXACT sealed envelope; it opens against our
    // pinned key and carries the addressed signal.
    let received = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
        .await
        .expect("stub must receive within timeout")
        .expect("stub body");
    let signal = federation::open(&received, &local_pem).expect("envelope opens against sender key");
    match signal.to {
        AgentAddress::Agent { name, workspace } => {
            assert_eq!(name, "bob");
            assert_eq!(workspace.0, "ws-b");
        }
        other => panic!("unexpected to-address: {other:?}"),
    }
    // No `from_workspace` (owner-remote send) → attributed to `owner`.
    match signal.from {
        AgentAddress::Agent { name, .. } => assert_eq!(name, "owner"),
        other => panic!("unexpected from-address: {other:?}"),
    }

    // Delivered → outbox drained.
    assert!(
        outbox::list_for_peer(&fp).is_empty(),
        "a confirmed delivery must remove the queued copy"
    );
}

// ── P4 send: the SENDER AGENT IDENTITY is carried in `from` so the
//    recipient can render `[from <agent>::<host>]` and reply. Regression
//    lock for the dropped-identity bug (`from` used to be a bare
//    `peer:<fp>` Workspace address). `from.name` = source workspace
//    agent name (lowercased at inbound attribution).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_sets_from_agent_identity_from_source_basename() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();

    let (port, rx) = spawn_inbound_stub().await;
    let base = format!("http://127.0.0.1:{port}");

    // Source workspace whose folder basename is the agent name `cortana`.
    let src_path = std::env::temp_dir()
        .join(format!("k2-src-{}-{}", std::process::id(), uuid::Uuid::new_v4()))
        .join("cortana")
        .to_string_lossy()
        .into_owned();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), "cortana", src_path],
        )
        .unwrap();
    }

    let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::default();
    let fp = store.upsert(FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap());
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();

    let local_pem = k2_core::federation::identity::load_or_create().unwrap().public_key_pem();

    let inbound_base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", &base);
    // Agent-initiated send → satisfy the GAP #3 connection gate first.
    k2_core::connections::connections(
        &src_path,
        "add",
        Some(&format!("bob@127.0.0.1:{port}")),
        None,
    )
    .expect("add remote connection");

    let send_body = body(serde_json::json!({
        "to": "peer::ws-b::bob",
        "text": "from the source agent",
        "from_workspace": src_path,
    }));
    let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
        .await
        .unwrap();
    drop(inbound_base);

    assert_eq!(resp.status, "200 OK", "send body: {}", resp.body);

    let received = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
        .await
        .expect("stub must receive within timeout")
        .expect("stub body");
    let signal = federation::open(&received, &local_pem).expect("envelope opens against sender key");
    match signal.from {
        AgentAddress::Agent { name, .. } => {
            assert_eq!(name, "cortana", "from.name must be the source workspace basename");
        }
        other => panic!("from must carry the sender agent identity, got {other:?}"),
    }
}

/// `from.name` must come from the workspace's RESOLVED agent name
/// (persona/display `name:` first), NOT the bare folder basename. Repro
/// for the cross-server reply mismatch: when the persona name differs
/// from the folder basename, the `[from]` attribution + reply target
/// must use the resolved name so the recipient's reply matches the
/// roster/connection.
#[tokio::test]
async fn send_sets_from_agent_identity_from_resolved_name_not_basename() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();

    let (port, rx) = spawn_inbound_stub().await;
    let base = format!("http://127.0.0.1:{port}");

    // Source workspace whose FOLDER BASENAME ("agent-box") deliberately
    // differs from the persona `name:` ("Aria"). The resolved name must
    // win.
    let src_path = std::env::temp_dir()
        .join(format!("k2-src-{}-{}", std::process::id(), uuid::Uuid::new_v4()))
        .join("agent-box")
        .to_string_lossy()
        .into_owned();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), "agent-box", src_path],
        )
        .unwrap();
    }
    // Write a persona file whose declared name differs from the basename.
    let agent_dir = k2_core::workspace::agent_identity::workspace_agent_path(&src_path);
    std::fs::create_dir_all(&agent_dir).expect("mkdir .k2/agent");
    std::fs::write(
        agent_dir.join("AGENT.md"),
        "---\nname: Aria\ntype: custom\n---\n# persona\n",
    )
    .expect("write AGENT.md");
    // Sanity: the resolver returns the persona name, not the basename.
    assert_eq!(
        k2_core::workspace::agent_identity::resolve_agent_name(&src_path).as_deref(),
        Some("Aria"),
    );

    let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::default();
    let fp = store.upsert(FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap());
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();

    let local_pem = k2_core::federation::identity::load_or_create().unwrap().public_key_pem();

    let inbound_base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", &base);
    // Satisfy the GAP #3 connection gate first.
    k2_core::connections::connections(
        &src_path,
        "add",
        Some(&format!("bob@127.0.0.1:{port}")),
        None,
    )
    .expect("add remote connection");

    let send_body = body(serde_json::json!({
        "to": "peer::ws-b::bob",
        "text": "from the resolved agent",
        "from_workspace": src_path,
    }));
    let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
        .await
        .unwrap();
    drop(inbound_base);

    assert_eq!(resp.status, "200 OK", "send body: {}", resp.body);

    let received = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
        .await
        .expect("stub must receive within timeout")
        .expect("stub body");
    let signal = federation::open(&received, &local_pem).expect("envelope opens against sender key");
    match signal.from {
        AgentAddress::Agent { name, .. } => {
            assert_eq!(
                name, "aria",
                "from.name must be the RESOLVED persona name (lowercased), not the folder basename"
            );
        }
        other => panic!("from must carry the sender agent identity, got {other:?}"),
    }
}

// GH #47 — inbound `[from …]` must use `::`, never `@`, so the receiving
// agent routes replies through `k2 msg` rather than `k2 mail`.
#[test]
fn federated_from_label_uses_double_colon_never_at() {
    assert_eq!(
        federated_from_label("Cortana", "Z3thon.k2.dev"),
        "cortana::z3thon.k2.dev"
    );
    assert_eq!(
        federated_from_label("  argus  ", "  Peer.K2.DEV  "),
        "argus::peer.k2.dev"
    );
    assert_eq!(federated_from_label("cortana", ""), "cortana");
    assert_eq!(federated_from_label("", "host.k2.dev"), "peer::host.k2.dev");
    // Regression: the old `@` form must never reappear.
    assert!(!federated_from_label("a", "b.k2.dev").contains('@'));
}

// ── P5 roster: peer-authenticated GET + local owner helpers ──

fn peer_key() -> rcgen::KeyPair {
    rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap()
}

#[test]
fn roster_denies_unauthenticated_request() {
    with_temp_home(|| {
        // No fp/ts/sig → fail-closed 403 (no projection leaked).
        let resp = handle_roster(None, None, None);
        assert_eq!(resp.status, "403 Forbidden", "body: {}", resp.body);
    });
}

#[test]
fn roster_denies_untrusted_peer() {
    with_temp_home(|| {
        // Pin the peer Pending (not Trusted) but granted roster → denied.
        let key = peer_key();
        let mut store = PeerStore::default();
        let fp = store.upsert(FederationPeer::pin("p", "p", key.public_key_pem()).unwrap());
        store.grant(&fp, k2_core::federation::CAP_ROSTER);
        store.save().unwrap();

        let ts = chrono::Utc::now().timestamp();
        let (signed_fp, sig) = roster::sign_roster_request(&key, ts).unwrap();
        let resp = handle_roster(
            Some(&signed_fp),
            Some(&ts.to_string()),
            Some(&sig),
        );
        assert_eq!(resp.status, "403 Forbidden", "untrusted must deny: {}", resp.body);
    });
}

#[test]
fn roster_returns_agents_for_trusted_peer() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        // A configured workspace this daemon exposes.
        let ws_path = std::env::temp_dir().join(format!(
            "k2-fedrt-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let ws_path = ws_path.to_string_lossy().into_owned();
        let ws_id = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO projects (id, name, path, agent_enabled) VALUES (?1, ?2, ?3, 1)",
                rusqlite::params![id, "fed-roster-ws", ws_path],
            )
            .unwrap();
            id
        };
        // Roster contact filter: master remote may be off in tests —
        // opt this workspace into federated contact via Remote Access.
        k2_core::workspace::settings::update_project_setting(
            &ws_path,
            "allow_remote_instruct",
            "1",
        )
        .expect("opt workspace into remote contact for roster");

        // Pin the calling peer Trusted + roster, then sign a valid request.
        let key = peer_key();
        let mut store = PeerStore::default();
        let fp = store.upsert(FederationPeer::pin("peer", "peer", key.public_key_pem()).unwrap());
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, k2_core::federation::CAP_ROSTER);
        store.save().unwrap();

        let ts = chrono::Utc::now().timestamp();
        let (signed_fp, sig) = roster::sign_roster_request(&key, ts).unwrap();
        let resp = handle_roster(Some(&signed_fp), Some(&ts.to_string()), Some(&sig));
        assert_eq!(resp.status, "200 OK", "trusted+roster must serve: {}", resp.body);

        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        let agents = v["agents"].as_array().expect("agents array");
        assert!(
            agents.iter().any(|a| a["workspace_id"] == ws_id),
            "the configured workspace must be exposed in the roster: {}",
            resp.body
        );
    });
}

/// 2026-10-16: the roster route a paired peer reads omits an agent whose
/// "Let agents on other servers message this agent" switch is off —
/// including one that only allows agents to create connections.
#[test]
fn roster_route_omits_agents_with_the_switch_off() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        k2_core::app_settings::update(serde_json::json!({ "allowRemoteInstruct": false }))
            .expect("master off");
        let u = uniq();
        let (on_id, _) = register_agent_ws("On", &format!("on-{u}"), Exposure::Listed);
        let (conn_id, _) =
            register_agent_ws("ConnOnly", &format!("conn-{u}"), Exposure::ConnectionsOnly);
        let (off_id, _) = register_agent_ws("Off", &format!("off-{u}"), Exposure::Hidden);

        let key = peer_key();
        let mut store = PeerStore::default();
        let fp = store.upsert(FederationPeer::pin("peer", "peer", key.public_key_pem()).unwrap());
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, k2_core::federation::CAP_ROSTER);
        store.save().unwrap();
        let ts = chrono::Utc::now().timestamp();
        let (signed_fp, sig) = roster::sign_roster_request(&key, ts).unwrap();
        let resp = handle_roster(Some(&signed_fp), Some(&ts.to_string()), Some(&sig));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        let ids: Vec<&str> = v["agents"]
            .as_array()
            .expect("agents array")
            .iter()
            .map(|a| a["workspace_id"].as_str().expect("workspace_id"))
            .collect();
        assert!(ids.contains(&on_id.as_str()), "switch on must be served: {}", resp.body);
        assert!(!ids.contains(&conn_id.as_str()), "connections-only must be omitted: {}", resp.body);
        assert!(!ids.contains(&off_id.as_str()), "switch off must be omitted: {}", resp.body);
    });
}

#[test]
fn peers_lists_pinned_peers() {
    with_temp_home(|| {
        let key = peer_key();
        let mut store = PeerStore::default();
        let fp = store.upsert(
            FederationPeer::pin("teammate", "rosson", key.public_key_pem()).unwrap(),
        );
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, k2_core::federation::CAP_ROSTER);
        store.save().unwrap();

        let resp = handle_peers();
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        let peers = v["peers"].as_array().expect("peers array");
        let p = peers
            .iter()
            .find(|p| p["fingerprint"] == fp)
            .expect("pinned peer must be listed");
        assert_eq!(p["subdomain"], "rosson");
        assert_eq!(p["trust"], "trusted");
        // Secrets are NOT exposed.
        assert!(p.get("public_key_pem").is_none(), "must not leak the pinned key");
    });
}

// ── GAP #3: pubkey identity for owner-driven auto-pair ──

#[test]
fn pubkey_returns_spki_pem_and_matching_fingerprint() {
    with_temp_home(|| {
        let resp = handle_pubkey();
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        let pem = v["public_key_pem"].as_str().expect("public_key_pem present");
        assert!(
            pem.contains("BEGIN PUBLIC KEY"),
            "must return an SPKI PEM; got {pem}"
        );
        let fp = v["fingerprint"].as_str().expect("fingerprint present");
        // The advertised fingerprint MUST equal the local fingerprint a
        // peer would compute from the presented PEM (so the pin is
        // self-consistent).
        assert_eq!(fp, federation::local_fingerprint().unwrap());
        assert_eq!(
            fp,
            k2_core::federation::peers::fingerprint_of_spki_pem(pem).unwrap(),
            "fingerprint must be sha256(SPKI DER) of the presented PEM"
        );
        // `subdomain` is always present (empty when no tunnel configured).
        assert!(v.get("subdomain").is_some(), "subdomain key must be present");
        assert!(v.get("base_url").is_some(), "base_url key must be present");
        let base = v["base_url"].as_str().unwrap_or("");
        assert!(
            !base.contains("127.0.0.1") && !base.to_ascii_lowercase().contains("localhost"),
            "pubkey must never advertise loopback; got {base:?}"
        );
    });
}

#[test]
fn pubkey_base_url_round_trips_advertise_env() {
    with_temp_home(|| {
        let advertise = k2_core::test_env::EnvVar::set(
            k2_core::federation::ADVERTISE_URL_ENV,
            "http://192.168.1.40:38471",
        );
        let resp = handle_pubkey();
        drop(advertise);
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["base_url"], "http://192.168.1.40:38471");
        assert!(
            !v["base_url"].as_str().unwrap().contains("k2.dev"),
            "air-gap advertise URL must not concatenate .k2.dev"
        );
    });
}

#[test]
fn pubkey_base_url_skips_loopback_env() {
    with_temp_home(|| {
        let advertise = k2_core::test_env::EnvVar::set(
            k2_core::federation::ADVERTISE_URL_ENV,
            "http://127.0.0.1:38471",
        );
        let resp = handle_pubkey();
        drop(advertise);
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        let base = v["base_url"].as_str().unwrap_or("missing");
        assert!(
            !base.contains("127.0.0.1") && !base.to_ascii_lowercase().contains("localhost"),
            "loopback advertise env must be skipped; got {base:?}"
        );
    });
}

// ── GAP #3: the cross-daemon CONNECTION GATE on `handle_send` ──
//
// When the caller supplies `from_workspace`, the send is allowed ONLY
// IF that source workspace is connected to the target `<agent>@<host>`
// (a `workspace_remote_connections` row). Fail-closed: no connection ⇒
// 403, never a dial. With a connection it falls through to the normal
// seal/enqueue/dial path (here the dial fails on a dead port → queued,
// which proves the gate let it through).

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_is_gated_on_source_workspace_remote_connection() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();

    // Register a LOCAL source workspace (the agent's workspace).
    let src_path = std::env::temp_dir().join(format!(
        "k2-gate-src-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let src_path = src_path.to_string_lossy().into_owned();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), "gate-src-ws", src_path],
        )
        .unwrap();
    }

    // Pin a Trusted peer (subdomain "peer", inbound granted).
    let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::default();
    let fp = store.upsert(
        FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap(),
    );
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();

    // Override the dial host to a dead port → peer_host() == "127.0.0.1:1".
    let inbound_base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", "http://127.0.0.1:1");

    // (A) No connection for the source workspace → 403, fail-closed.
    let send_body = body(serde_json::json!({
        "to": "peer::ws::bob",
        "text": "should be blocked",
        "from_workspace": src_path,
    }));
    let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
        .await
        .unwrap();
    assert_eq!(
        resp.status, "403 Forbidden",
        "an agent send without a remote connection must be rejected: {}",
        resp.body
    );
    assert!(
        resp.body.contains("not a connection"),
        "the 403 must explain the missing connection; got {}",
        resp.body
    );

    // Add the matching remote connection `bob@127.0.0.1:1` for the source.
    k2_core::connections::connections(
        &src_path,
        "add",
        Some("bob@127.0.0.1:1"),
        None,
    )
    .expect("add remote connection");

    // (B) Now the gate opens; the dead-port dial then fails → queued.
    let send_body = body(serde_json::json!({
        "to": "peer::ws::bob",
        "text": "now allowed",
        "from_workspace": src_path,
    }));
    let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
        .await
        .unwrap();
    drop(inbound_base);

    assert_eq!(
        resp.status, "200 OK",
        "a connected source workspace must pass the gate: {}",
        resp.body
    );
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(
        v["status"], "queued",
        "past the gate, the dead-port dial leaves the message queued: {}",
        resp.body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_to_unreachable_peer_leaves_message_queued() {
    let _home = crate::test_support::TempHome::new();

    let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::default();
    let fp = store.upsert(
        FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap(),
    );
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();

    // Point at a dead port (nothing listening) → dial fails.
    let inbound_base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", "http://127.0.0.1:1");
    let send_body = body(serde_json::json!({ "to": "peer::ws::bob", "text": "queued please" }));
    let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
        .await
        .unwrap();
    drop(inbound_base);

    assert_eq!(resp.status, "200 OK");
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(v["status"], "queued", "an undeliverable send must stay queued");
    assert_eq!(
        outbox::list_for_peer(&fp).len(),
        1,
        "the durable outbox must retain the message for retry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_tray_to_unreachable_peer_queues_inbox_tray_signal() {
    let _home = crate::test_support::TempHome::new();

    let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::default();
    let fp = store.upsert(
        FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap(),
    );
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();

    let inbound_base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", "http://127.0.0.1:1");
    let spec = brief_tray();
    let send_body = body(serde_json::json!({
        "to": "peer::ws::bob",
        "tray": spec,
    }));
    let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
        .await
        .unwrap();
    drop(inbound_base);

    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(v["status"], "queued", "{}", resp.body);

    let queued = outbox::list_for_peer(&fp);
    assert_eq!(queued.len(), 1, "local daemon must accept and forward");
    let env: federation::FederationEnvelope =
        serde_json::from_slice(&queued[0].bytes).expect("queued envelope");
    match env.payload.signal.kind {
        SignalKind::Custom { kind, payload } => {
            assert_eq!(kind, federation::TRAY_SIGNAL_KIND);
            assert!(
                payload.get("files").and_then(|f| f.as_array()).map(|a| !a.is_empty())
                    == Some(true),
                "queued tray must carry files: {payload}"
            );
            let dump = payload.to_string();
            assert!(
                !dump.contains("Do the thing.") || dump.contains("bytes_b64"),
                "file body rides as bytes_b64, not live text"
            );
        }
        SignalKind::Msg { text } => panic!("must not fall back to live text: {text}"),
        other => panic!("expected inbox-tray Custom, got {other:?}"),
    }
}

// ── 0.45.6: federated `--inbox-wake --title/--body` wakes on the PEER ──
//
// appa 2026-10-11..13: `k2 msg chris::<box>.k2.dev --inbox-wake
// --title … --body …` landed the package but the wake always answered
// `workspace_not_found`. The compose route forced `wake=false` into the
// tray, and the CLI then knocked THIS box's `/cli/workspace/msg` with the
// federated token, which can never resolve here. The wake now rides the
// tray and the peer answers with its own wake result.

/// Sender half: the real compose route, a real sealed envelope to a
/// peer stub. The tray must ask the peer to wake (`wake: true`), and the
/// peer's wake result comes back as `wake` in the compose response.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compose_inbox_wake_to_agent_at_host_asks_the_peer_to_wake() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _fed = k2_core::test_env::EnvVar::set("K2_FEDERATION", "1");
    let w = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let tx = std::sync::Mutex::new(tx);
    let stub = spawn_peer_stub(
        Some(roster_json(&[(&w, "chris", "Chris", &[])])),
        r#"{"delivered":true,"mode":"inbox","ok":true,"id":"pkg-1","title":"Numbers","coverPath":"/home/k2/ai/Chris/.k2/inbox/pkg-1.md","wake":{"success":true,"reason":null,"targetSessionId":"s-1","woke":true,"attempts":1}}"#,
        move |b| {
            let _ = tx.lock().unwrap().send(b);
        },
    );
    let _base = InboundBase::set(&format!("http://127.0.0.1:{}", stub.port));
    let _fp = pin_trusted_peer_for_send("cyberdyne");

    let mut params = std::collections::HashMap::new();
    params.insert("workspace".to_string(), "chris::cyberdyne.k2.dev".to_string());
    params.insert("title".to_string(), "Numbers".to_string());
    params.insert("body".to_string(), "a to z numbers".to_string());
    params.insert("wake".to_string(), "true".to_string());
    params.insert("mode".to_string(), "wake".to_string());
    params.insert("source".to_string(), "msg-inbox".to_string());
    let resp = tokio::task::spawn_blocking(move || {
        crate::inbox_routes::handle_compose_post(&params)
    })
    .await
    .unwrap();
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(v["status"], "sent", "{}", resp.body);
    assert_eq!(v["coverPath"], "/home/k2/ai/Chris/.k2/inbox/pkg-1.md", "{}", resp.body);
    assert_eq!(v["wake"]["success"], true, "the peer's wake result must come back: {}", resp.body);

    let received = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("peer stub received the envelope");
    let local_pem = k2_core::federation::identity::load_or_create().unwrap().public_key_pem();
    let signal = federation::open(&received, &local_pem).expect("envelope opens");
    match signal.kind {
        SignalKind::Custom { kind, payload } => {
            assert_eq!(kind, federation::TRAY_SIGNAL_KIND);
            let spec: federation::TraySpec = serde_json::from_value(payload).unwrap();
            assert!(spec.wake, "the tray must ask the peer to wake its workspace");
            assert_eq!(spec.title.as_deref(), Some("Numbers"));
        }
        other => panic!("expected an inbox-tray, got {other:?}"),
    }
}

/// `--inbox-silent` (no wake param) still lands quietly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compose_inbox_silent_to_agent_at_host_does_not_ask_for_a_wake() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _fed = k2_core::test_env::EnvVar::set("K2_FEDERATION", "1");
    let w = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let tx = std::sync::Mutex::new(tx);
    let stub = spawn_peer_stub(
        Some(roster_json(&[(&w, "chris", "Chris", &[])])),
        r#"{"delivered":true,"mode":"inbox","ok":true,"id":"pkg-2"}"#,
        move |b| {
            let _ = tx.lock().unwrap().send(b);
        },
    );
    let _base = InboundBase::set(&format!("http://127.0.0.1:{}", stub.port));
    let _fp = pin_trusted_peer_for_send("cyberdyne");

    let mut params = std::collections::HashMap::new();
    params.insert("workspace".to_string(), "chris::cyberdyne.k2.dev".to_string());
    params.insert("title".to_string(), "Quiet".to_string());
    let resp = tokio::task::spawn_blocking(move || {
        crate::inbox_routes::handle_compose_post(&params)
    })
    .await
    .unwrap();
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let received = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("peer stub received the envelope");
    let local_pem = k2_core::federation::identity::load_or_create().unwrap().public_key_pem();
    let signal = federation::open(&received, &local_pem).expect("envelope opens");
    match signal.kind {
        SignalKind::Custom { payload, .. } => {
            let spec: federation::TraySpec = serde_json::from_value(payload).unwrap();
            assert!(!spec.wake, "a silent compose must not ask for a wake");
        }
        other => panic!("expected an inbox-tray, got {other:?}"),
    }
}

/// Receiver half: a tray asking for a wake, into an opted-in workspace
/// with an agent, wakes it by PATH on the receiving box and answers
/// `wake.success == true` (the agent is a `cat` shim, never a real CLI).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbound_tray_with_wake_wakes_the_receiving_workspace() {
    let _home = crate::test_support::TempHome::new();
    {
        k2_core::db::init_for_tests();
        crate::v2_session_map::clear_for_tests();
        let shim_dir = std::env::temp_dir()
            .join(format!("k2-fed-wake-shim-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&shim_dir).unwrap();
        let shim = shim_dir.join("cat");
        k2_core::test_env::write_executable(&shim, "#!/bin/sh\nexec cat\n");
        let _cmd = k2_core::test_env::EnvVar::set("K2SO_WAKE_HEADLESS_TEST_COMMAND", "cat");
        let _shim = k2_core::test_env::EnvVar::set("K2_TEST_AGENT_SHIM_DIR", &shim_dir);

        let ws_path = temp_ws_path();
        std::fs::create_dir_all(&ws_path).unwrap();
        let ws_id = register_project(&ws_path);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE projects SET agent_mode = 'manager', agent_enabled = 1 WHERE id = ?1",
                rusqlite::params![ws_id],
            )
            .unwrap();
        }
        k2_core::workspace::settings::update_project_setting(
            &ws_path,
            "allow_remote_instruct",
            "1",
        )
        .expect("opt in");

        let key = pin_trusted_peer("peer");
        let bytes = seal_tray_to(&ws_id, &key, "peer", brief_tray());
        let resp = tokio::task::spawn_blocking(move || handle_inbound(&bytes))
            .await
            .unwrap();
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["mode"], "inbox", "{}", resp.body);
        assert_eq!(v["ok"], true, "{}", resp.body);
        assert_eq!(v["wake"]["success"], true, "receiver must wake its workspace: {}", resp.body);
        assert!(
            v["wake"]["targetSessionId"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
            "wake names the session it reached: {}",
            resp.body
        );

        crate::v2_session_map::clear_for_tests();
        let _ = std::fs::remove_dir_all(&shim_dir);
    }
}

// ── PR1: dual-auth under passports — principal-bound from_workspace ──
//
// A5.1 Scoped + trusted peer + connection → send ok/queued
// A5.2 Scoped + no connection → not_connected
// A5.4 Spoofed from_workspace ignored with principal
// (A5.3 scoped cannot pair/confirm lives in session_token allowlist tests;
//  A5.5 owner send still works = existing tests above.)

/// Register a projects row and return (path, uuid) for a scoped principal.
fn register_src_workspace(label: &str) -> (String, String) {
    let src_path = std::env::temp_dir().join(format!(
        "k2-pr1-src-{}-{}-{}",
        label,
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let src_path = src_path.to_string_lossy().into_owned();
    let uuid = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![uuid, format!("pr1-{label}"), src_path],
        )
        .unwrap();
    }
    (src_path, uuid)
}

fn pin_trusted_peer_for_send(subdomain: &str) -> String {
    let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::default();
    let fp = store.upsert(
        FederationPeer::pin("teammate", subdomain, peer_key.public_key_pem()).unwrap(),
    );
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();
    fp
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pr1_scoped_principal_send_with_connection_queues() {
    // A5.1: principal present + trusted peer + remote connection → gate
    // opens; dead-port dial → queued.
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (src_path, ws_uuid) = register_src_workspace("ok");
    let _fp = pin_trusted_peer_for_send("peer");
    let inbound_base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", "http://127.0.0.1:1");
    k2_core::connections::connections(
        &src_path,
        "add",
        Some("bob@127.0.0.1:1"),
        None,
    )
    .expect("add remote connection");

    let principal = crate::session_token::HookPrincipal {
        workspace_uuid: ws_uuid,
        agent_address: "src-agent".into(),
    };
    // Body omits from_workspace — principal supplies it.
    let send_body = body(serde_json::json!({
        "to": "peer::ws::bob",
        "text": "passport send",
    }));
    let resp = tokio::task::spawn_blocking(move || {
        crate::caller_workspace::with_request_principal(Some(principal), || {
            handle_send(&send_body)
        })
    })
    .await
    .unwrap();
    drop(inbound_base);

    assert_eq!(
        resp.status, "200 OK",
        "scoped principal with connection must send: {}",
        resp.body
    );
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(v["status"], "queued", "dead-port dial → queued: {}", resp.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pr1_scoped_principal_send_without_connection_is_not_connected() {
    // A5.2: principal present + trusted peer + NO connection → not_connected.
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (_src_path, ws_uuid) = register_src_workspace("noconnect");
    let _fp = pin_trusted_peer_for_send("peer");
    let inbound_base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", "http://127.0.0.1:1");

    let principal = crate::session_token::HookPrincipal {
        workspace_uuid: ws_uuid,
        agent_address: "src-agent".into(),
    };
    let send_body = body(serde_json::json!({
        "to": "peer::ws::bob",
        "text": "blocked",
    }));
    let resp = tokio::task::spawn_blocking(move || {
        crate::caller_workspace::with_request_principal(Some(principal), || {
            handle_send(&send_body)
        })
    })
    .await
    .unwrap();
    drop(inbound_base);

    assert_eq!(resp.status, "403 Forbidden", "must fail closed: {}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(
        v["error"]["code"], "not_connected",
        "stable not_connected code for CLI exit 3: {}",
        resp.body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pr1_scoped_principal_ignores_spoofed_from_workspace() {
    // A5.4: body claims a DIFFERENT workspace that HAS a connection;
    // principal binds the real workspace which does NOT — spoof ignored
    // → not_connected on the principal workspace.
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (_real_path, real_uuid) = register_src_workspace("real");
    let (spoof_path, _spoof_uuid) = register_src_workspace("spoof");
    let _fp = pin_trusted_peer_for_send("peer");
    let inbound_base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", "http://127.0.0.1:1");

    // Only the spoofed workspace is connected — if body won, send would queue.
    k2_core::connections::connections(
        &spoof_path,
        "add",
        Some("bob@127.0.0.1:1"),
        None,
    )
    .expect("add spoof connection");

    let principal = crate::session_token::HookPrincipal {
        workspace_uuid: real_uuid,
        agent_address: "real-agent".into(),
    };
    let send_body = body(serde_json::json!({
        "to": "peer::ws::bob",
        "text": "spoof attempt",
        "from_workspace": spoof_path,
    }));
    let resp = tokio::task::spawn_blocking(move || {
        crate::caller_workspace::with_request_principal(Some(principal), || {
            handle_send(&send_body)
        })
    })
    .await
    .unwrap();
    drop(inbound_base);

    assert_eq!(
        resp.status, "403 Forbidden",
        "spoofed from_workspace must be ignored; principal workspace has no connection: {}",
        resp.body
    );
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(
        v["error"]["code"], "not_connected",
        "must gate on principal workspace, not body spoof: {}",
        resp.body
    );
}

#[test]
fn pr1_scoped_allowlist_admits_send_surface_not_pair_confirm() {
    // A5.3: scoped cannot pair/confirm (allowlist); peers/roster/send ok.
    use crate::session_token::is_agent_verb;
    assert!(is_agent_verb("/cli/federation/peers"));
    assert!(is_agent_verb("/cli/federation/peer-roster"));
    assert!(is_agent_verb("/cli/federation/send"));
    assert!(!is_agent_verb("/cli/federation/pair/confirm"));
    assert!(!is_agent_verb("/cli/federation/pair/request"));
    assert!(!is_agent_verb("/cli/federation/outbox"));
}

// ── Air-gap LAN federation (prd-airgap-lan-federation-v1) ──

/// `K2_AIRGAP` set/removed under the ONE shared env lock (restored on
/// drop), with the air-gap setting mirror reset on both ends.
struct AirgapEnv {
    _var: k2_core::test_env::EnvVar,
}
impl AirgapEnv {
    fn set(val: Option<&str>) -> Self {
        let var = match val {
            Some(v) => k2_core::test_env::EnvVar::set("K2_AIRGAP", v),
            None => k2_core::test_env::EnvVar::remove("K2_AIRGAP"),
        };
        k2_core::airgap::set_setting_enabled(false);
        Self { _var: var }
    }
}
impl Drop for AirgapEnv {
    fn drop(&mut self) {
        // Runs before the EnvVar field restores K2_AIRGAP (lock still held).
        k2_core::airgap::set_setting_enabled(false);
    }
}

#[test]
fn peer_base_url_airgap_never_concatenates_k2_dev() {
    with_temp_home(|| {
        let _env = AirgapEnv::set(Some("1"));
        let _inbound_base = k2_core::test_env::EnvVar::remove("K2_FEDERATION_INBOUND_BASE");
        let url = peer_base_url_from("rpm", "");
        assert!(
            !url.contains("k2.dev"),
            "air-gap peer_base_url must not append .k2.dev; got {url:?}"
        );
        let lan = peer_base_url_from("", "http://192.168.1.50:38471");
        assert_eq!(lan, "http://192.168.1.50:38471");
        assert!(!lan.contains("k2.dev"));
    });
}

#[cfg(not(feature = "airgap"))]
#[test]
fn peer_base_url_connect_unchanged_when_not_airgap() {
    with_temp_home(|| {
        let _env = AirgapEnv::set(Some("0"));
        let _inbound_base = k2_core::test_env::EnvVar::remove("K2_FEDERATION_INBOUND_BASE");
        assert_eq!(peer_base_url_from("rpm", ""), "https://rpm.k2.dev");
    });
}

#[test]
fn pair_request_url_then_confirm_is_trusted_no_k2_dev() {
    with_temp_home(|| {
        let _env = AirgapEnv::set(Some("1"));
        let (offer, fp) = remote_offer("", "http://192.168.1.50:38471");
        let resp = handle_pair_v2_respond(&body(serde_json::json!({ "offer": offer })));
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
        let answer: k2_core::federation::pairing_v2::Answer =
            serde_json::from_value(serde_json::from_str::<serde_json::Value>(&resp.body).unwrap()["answer"].clone())
                .expect("answer");
        let revealed = k2_core::federation::pairing_v2::reveal(
            &mut PeerStore::default(),
            &answer,
            None,
            Some("http://192.168.1.60:38471"),
            chrono::Utc::now(),
        )
        .expect("reveal");
        let ver = handle_pair_v2_verify(&body(serde_json::json!({ "reveal": revealed.reveal })));
        assert_eq!(ver.status, "200 OK", "{}", ver.body);
        let sas = revealed.sas.clone();
        let conf = handle_pair_confirm(&body(serde_json::json!({
            "fingerprint": fp,
            "sas": sas,
        })));
        assert_eq!(conf.status, "200 OK", "{}", conf.body);
        let store = PeerStore::load().unwrap();
        let p = store.get(&fp).unwrap();
        assert_eq!(p.trust, PeerTrust::Trusted);
        assert_eq!(p.base_url, "http://192.168.1.50:38471");
        assert!(
            !p.base_url.contains("k2.dev"),
            "stored dial URL must not contain .k2.dev"
        );
        let dial = peer_base_url(p);
        assert!(!dial.contains("k2.dev"), "got {dial}");
    });
}

#[test]
fn pair_request_empty_url_and_subdomain_is_400() {
    with_temp_home(|| {
        let _env = AirgapEnv::set(Some("1"));
        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut offer = k2_core::federation::pairing_v2::init(
            &peer_key.public_key_pem(),
            "x",
            "",
            "http://192.168.1.50:38471",
            chrono::Utc::now(),
        )
        .expect("init");
        offer.base_url = String::new();
        let req = handle_pair_v2_respond(&body(serde_json::json!({ "offer": offer })));
        assert!(
            req.status.starts_with("400"),
            "empty url+subdomain must 400; got {} {}",
            req.status,
            req.body
        );
        assert!(PeerStore::load().unwrap().list().is_empty());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn airgap_send_to_k2_dev_refuses_without_syn() {
    let _home = crate::test_support::TempHome::new();
    let _env = AirgapEnv::set(Some("1"));
    let _inbound_base = k2_core::test_env::EnvVar::remove("K2_FEDERATION_INBOUND_BASE");
    let spy = std::net::TcpListener::bind("127.0.0.1:0").expect("spy");
    spy.set_nonblocking(true).expect("nb");
    let fp = pin_trusted_peer_for_send("foo");
    let send_body = body(serde_json::json!({ "to": format!("{fp}::ws::bob"), "text": "nope" }));
    let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
        .await
        .unwrap();
    assert_eq!(resp.status, "403 Forbidden", "must refuse Connect zone: {}", resp.body);
    assert!(
        resp.body.contains("k2.dev") || resp.body.contains("Air-gap"),
        "got {}",
        resp.body
    );
    assert!(spy.accept().is_err(), "must not SYN");
    assert!(
        outbox::list_for_peer(&fp).is_empty(),
        "refused Connect send must not enqueue leftover k2.dev outbox"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn airgap_send_to_lan_http_allowed_fake_http() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _home = crate::test_support::TempHome::new();
    let _env = AirgapEnv::set(Some("1"));
    let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.expect("accept");
        let mut buf = [0u8; 4096];
        let _ = s.read(&mut buf).await;
        let body = r#"{"delivered":true,"mode":"live"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = s.write_all(resp.as_bytes()).await;
    });

    let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::default();
    let fp = store.upsert(
        FederationPeer::pin("lan", "127.0.0.1", peer_key.public_key_pem())
            .unwrap()
            .with_base_url(format!("http://127.0.0.1:{port}")),
    );
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();
    let _inbound_base = k2_core::test_env::EnvVar::remove("K2_FEDERATION_INBOUND_BASE");

    let to = format!("{fp}::ws::bob");
    let send_body = body(serde_json::json!({ "to": to, "text": "lan ping" }));
    let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
        .await
        .unwrap();
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(v["status"], "sent", "{}", resp.body);
}

// ── A7: a connection is to a remote WORKSPACE, not a spelling ──────
//
// Peer = a loopback PeerStub reached through K2_FEDERATION_INBOUND_BASE
// (roster + inbound on one port). The source workspace is an agent
// (scoped principal), so the connection gate always runs.

/// Run `handle_send` as the scoped agent of `ws_uuid`.
async fn send_as_agent(ws_uuid: &str, to: String, text: &str) -> CliResponse {
    let principal = crate::session_token::HookPrincipal {
        workspace_uuid: ws_uuid.to_string(),
        agent_address: "src-agent".into(),
    };
    let send_body = body(serde_json::json!({ "to": to, "text": text }));
    tokio::task::spawn_blocking(move || {
        crate::caller_workspace::with_request_principal(Some(principal), || {
            handle_send(&send_body)
        })
    })
    .await
    .unwrap()
}

/// The source workspace's stored remote rows.
fn remote_rows(src_uuid: &str) -> Vec<k2_core::db::schema::WorkspaceRemoteConnection> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceRemoteConnection::list_for_source(&conn, src_uuid).unwrap()
}

fn error_code(resp: &CliResponse) -> String {
    let v: serde_json::Value = serde_json::from_str(&resp.body)
        .unwrap_or_else(|e| panic!("non-JSON body ({e}): {}", resp.body));
    v["error"]["code"].as_str().unwrap_or("").to_string()
}

fn send_status(resp: &CliResponse) -> String {
    let v: serde_json::Value = serde_json::from_str(&resp.body)
        .unwrap_or_else(|e| panic!("non-JSON body ({e}): {}", resp.body));
    v["status"].as_str().unwrap_or("").to_string()
}

/// K2_FEDERATION_INBOUND_BASE under the ONE shared env lock; restores
/// the previous value even when an assert panics.
struct InboundBase {
    _var: k2_core::test_env::EnvVar,
}
impl InboundBase {
    fn set(base: &str) -> Self {
        Self { _var: k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", base) }
    }
}

/// PRD test 1 + test 6: a row saved under the display name ("seoca")
/// is bound by the gate's one roster fetch (no prior peer-roster
/// read), and then both the handle and the display name reach that
/// workspace. The stored `agent` stays as typed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a7_handle_and_display_name_share_one_connection() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (src_path, src_uuid) = register_src_workspace("a7-both");
    let w = uuid::Uuid::new_v4().to_string();
    let x = uuid::Uuid::new_v4().to_string();
    let stub = spawn_peer_stub(
        Some(roster_json(&[
            (&w, "quahog-website", "Seoca", &[]),
            (&x, "other-site", "Other", &[]),
        ])),
        r#"{"delivered":true,"mode":"live"}"#,
        |_| {},
    );
    let _base = InboundBase::set(&format!("http://127.0.0.1:{}", stub.port));
    let fp = pin_trusted_peer_for_send("peer");
    let host = format!("127.0.0.1:{}", stub.port);
    k2_core::connections::connections(&src_path, "add", Some(&format!("seoca::{host}")), None)
        .expect("add remote connection");

    let resp = send_as_agent(&src_uuid, format!("{fp}::{w}::quahog-website"), "by handle").await;
    assert_eq!(resp.status, "200 OK", "handle must pass the gate: {}", resp.body);
    assert_eq!(send_status(&resp), "sent", "{}", resp.body);
    assert_eq!(
        stub.roster_hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "an unbound row costs exactly one roster fetch"
    );
    let rows = remote_rows(&src_uuid);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].remote_workspace_id.as_deref(), Some(w.as_str()), "bound to W");
    assert_eq!(rows[0].remote_handle.as_deref(), Some("quahog-website"));
    assert_eq!(rows[0].remote_display_name.as_deref(), Some("Seoca"));
    assert_eq!(rows[0].agent, "seoca", "a display-name bind keeps the typed name");
    assert_eq!(rows[0].peer_fingerprint.as_deref(), Some(fp.as_str()));

    let resp = send_as_agent(&src_uuid, format!("{fp}::{w}::seoca"), "by display").await;
    assert_eq!(resp.status, "200 OK", "display name must pass too: {}", resp.body);
    assert_eq!(send_status(&resp), "sent", "{}", resp.body);
    assert_eq!(
        stub.roster_hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a bound row decides by id without another fetch"
    );
}

/// PRD test 2 / CA16 (F1 at the sender): the row is connected to W
/// as "seoca". A send naming "seoca" with another roster workspace's
/// id X is refused before it leaves this daemon. On v0.45.0 this send
/// passed the gate (string match on "seoca") — this test is red there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a7_forged_workspace_id_is_not_connected_at_the_sender() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (src_path, src_uuid) = register_src_workspace("a7-forged");
    let w = uuid::Uuid::new_v4().to_string();
    let x = uuid::Uuid::new_v4().to_string();
    let delivered = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = delivered.clone();
    let stub = spawn_peer_stub(
        Some(roster_json(&[
            (&w, "quahog-website", "Seoca", &[]),
            (&x, "other-site", "Other", &[]),
        ])),
        r#"{"delivered":true,"mode":"live"}"#,
        move |_| {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
    );
    let _base = InboundBase::set(&format!("http://127.0.0.1:{}", stub.port));
    let fp = pin_trusted_peer_for_send("peer");
    let host = format!("127.0.0.1:{}", stub.port);
    k2_core::connections::connections(&src_path, "add", Some(&format!("seoca::{host}")), None)
        .expect("add remote connection");

    let resp = send_as_agent(&src_uuid, format!("{fp}::{x}::seoca"), "forged id").await;
    assert_eq!(resp.status, "403 Forbidden", "forged id must be refused: {}", resp.body);
    assert_eq!(error_code(&resp), "not_connected", "{}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(
        hint.contains(&format!("quahog-website::{host}")) && hint.contains("different workspace"),
        "hint names the connection that exists: {hint}"
    );
    assert_eq!(
        delivered.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "nothing reached the peer"
    );
    assert!(
        outbox::list_for_peer(&fp).is_empty(),
        "a refused send is never queued"
    );
}

/// PRD test 3: two peer workspaces share the display name "Seoca", so
/// a row saved as "seoca" can't bind (D15). A send by one handle is
/// refused and the hint lists both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a7_ambiguous_display_name_stays_unbound_and_hint_lists_both() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (src_path, src_uuid) = register_src_workspace("a7-ambig");
    let w1 = uuid::Uuid::new_v4().to_string();
    let w2 = uuid::Uuid::new_v4().to_string();
    let stub = spawn_peer_stub(
        Some(roster_json(&[
            (&w1, "seoca-one", "Seoca", &[]),
            (&w2, "seoca-two", "Seoca", &[]),
        ])),
        r#"{"delivered":true,"mode":"live"}"#,
        |_| {},
    );
    let _base = InboundBase::set(&format!("http://127.0.0.1:{}", stub.port));
    let fp = pin_trusted_peer_for_send("peer");
    let host = format!("127.0.0.1:{}", stub.port);
    k2_core::connections::connections(&src_path, "add", Some(&format!("seoca::{host}")), None)
        .expect("add remote connection");

    let resp = send_as_agent(&src_uuid, format!("{fp}::{w1}::seoca-one"), "which one").await;
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    assert_eq!(error_code(&resp), "not_connected", "{}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(
        hint.contains(&format!("seoca-one::{host}")) && hint.contains(&format!("seoca-two::{host}")),
        "hint lists both workspaces: {hint}"
    );
    let rows = remote_rows(&src_uuid);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].remote_workspace_id.is_none(), "ambiguous display never binds: {rows:?}");
}

/// CA10: the peer is offline (no listener) and the row is unbound —
/// today's name rule still applies and the send queues; a name the
/// row doesn't carry is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a7_offline_peer_unbound_row_keeps_name_rule_and_queues() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (src_path, src_uuid) = register_src_workspace("a7-offline");
    let _base = InboundBase::set("http://127.0.0.1:1");
    let fp = pin_trusted_peer_for_send("peer");
    k2_core::connections::connections(&src_path, "add", Some("seoca::127.0.0.1:1"), None)
        .expect("add remote connection");
    let w = uuid::Uuid::new_v4().to_string();

    let resp = send_as_agent(&src_uuid, format!("{fp}::{w}::seoca"), "while offline").await;
    assert_eq!(resp.status, "200 OK", "offline + unbound must not 403: {}", resp.body);
    assert_eq!(send_status(&resp), "queued", "{}", resp.body);

    let resp = send_as_agent(&src_uuid, format!("{fp}::{w}::someone-else"), "nope").await;
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    assert_eq!(error_code(&resp), "not_connected", "{}", resp.body);
    let rows = remote_rows(&src_uuid);
    assert!(rows[0].remote_workspace_id.is_none(), "a failed fetch binds nothing");
}

/// PRD test 5 + CA14 (S4 + F3): `from.name` is the workspace handle —
/// the name the peer's roster lists — not the persona `name:`, and
/// `from.workspace` is the project id, not the sender's folder path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a7_from_is_stamped_with_handle_and_project_id() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let handle = format!("quahog-website-{}", uniq());
    let (src_uuid, src_path) = register_agent_ws("Seoca", &handle, Exposure::ConnectionsOnly);
    let persona = k2_core::workspace::agent_identity::workspace_agent_md_path(&src_path);
    std::fs::create_dir_all(persona.parent().unwrap()).unwrap();
    std::fs::write(&persona, "---\nname: seoca\n---\n# Seoca\n").unwrap();
    assert_eq!(
        k2_core::workspace::agent_identity::resolve_agent_name(&src_path).as_deref(),
        Some("seoca"),
        "the persona name differs from the handle"
    );

    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let tx = std::sync::Mutex::new(tx);
    let stub = spawn_peer_stub(None, r#"{"delivered":true,"mode":"live"}"#, move |b| {
        let _ = tx.lock().unwrap().send(b);
    });
    let _base = InboundBase::set(&format!("http://127.0.0.1:{}", stub.port));
    let fp = pin_trusted_peer_for_send("peer");
    k2_core::connections::connections(
        &src_path,
        "add",
        Some(&format!("bob::127.0.0.1:{}", stub.port)),
        None,
    )
    .expect("add remote connection");
    let local_pem = k2_core::federation::identity::load_or_create().unwrap().public_key_pem();

    let resp = send_as_agent(&src_uuid, format!("{fp}::ws-b::bob"), "stamp check").await;
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let received = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("peer stub received the envelope");
    let signal = federation::open(&received, &local_pem).expect("envelope opens");
    match signal.from {
        AgentAddress::Agent { name, workspace } => {
            assert_eq!(name, handle, "from.name must be the handle (the roster agent)");
            assert_eq!(workspace.0, src_uuid, "from.workspace must be the project id, not a path");
        }
        other => panic!("from must be an agent address, got {other:?}"),
    }
}

/// 0159 (replaces CA9's re-bind): the peer removed the workspace and a
/// NEW one (new id) now has the same handle and display name (an older
/// peer can hand names out again). A
/// successful roster read marks the row gone and keeps the old id; a
/// send naming the new id fails `remote_workspace_gone` and never
/// reaches the peer; a failed read leaves everything as it was. Only
/// the owner adding the connection again binds it to the new id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a7_gone_workspace_is_never_rebound_by_name() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (src_path, src_uuid) = register_src_workspace("a7-gone");
    let w_old = uuid::Uuid::new_v4().to_string();
    let w_new = uuid::Uuid::new_v4().to_string();
    let first = spawn_peer_stub(
        Some(roster_json(&[(&w_old, "quahog-website", "Seoca", &[])])),
        "{}",
        |_| {},
    );
    // Every stub of this test answers under one host: the base env
    // var decides where the dial goes, the row's host stays the same.
    let host = "quahog.example.test";
    let fp = pin_trusted_peer_for_send(host);
    k2_core::connections::connections(&src_path, "add", Some(&format!("seoca::{host}")), None)
        .expect("add remote connection");

    {
        let _base = InboundBase::set(&format!("http://127.0.0.1:{}", first.port));
        let r = tokio::task::spawn_blocking({
            let fp = fp.clone();
            move || handle_peer_roster(&fp)
        })
        .await
        .unwrap();
        assert_eq!(r.status, "200 OK", "{}", r.body);
    }
    assert_eq!(remote_rows(&src_uuid)[0].remote_workspace_id.as_deref(), Some(w_old.as_str()));
    assert!(remote_rows(&src_uuid)[0].remote_workspace_gone_at.is_none());

    let delivered = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = delivered.clone();
    let second = spawn_peer_stub(
        Some(roster_json(&[(&w_new, "quahog-website", "Seoca", &[])])),
        r#"{"delivered":true,"mode":"live"}"#,
        move |_| {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
    );
    {
        let _base = InboundBase::set(&format!("http://127.0.0.1:{}", second.port));
        let r = tokio::task::spawn_blocking({
            let fp = fp.clone();
            move || handle_peer_roster(&fp)
        })
        .await
        .unwrap();
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let row = &remote_rows(&src_uuid)[0];
        assert_eq!(
            row.remote_workspace_id.as_deref(),
            Some(w_old.as_str()),
            "the bound id left the roster: kept, never moved to the new id"
        );
        assert!(row.remote_workspace_gone_at.is_some(), "marked gone: {row:?}");

        for typed in ["seoca", "quahog-website"] {
            let resp =
                send_as_agent(&src_uuid, format!("{fp}::{w_new}::{typed}"), "hello").await;
            assert_eq!(resp.status, "410 Gone", "{typed}: {}", resp.body);
            assert_eq!(error_code(&resp), "remote_workspace_gone", "{}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
            let hint = v["error"]["hint"].as_str().unwrap();
            assert!(hint.contains("no longer on"), "{hint}");
            assert!(hint.contains("k2 connections add"), "{hint}");
        }
        assert_eq!(
            delivered.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "nothing reaches the new workspace"
        );
        assert_eq!(remote_rows(&src_uuid)[0].remote_workspace_id.as_deref(), Some(w_old.as_str()));
    }

    {
        let _base = InboundBase::set("http://127.0.0.1:1");
        let r = tokio::task::spawn_blocking({
            let fp = fp.clone();
            move || handle_peer_roster(&fp)
        })
        .await
        .unwrap();
        assert_eq!(r.status, "502 Bad Gateway", "{}", r.body);
    }
    assert_eq!(
        remote_rows(&src_uuid)[0].remote_workspace_id.as_deref(),
        Some(w_old.as_str()),
        "a failed fetch never touches a binding"
    );

    // An agent may not move it, even with "Allow agents to create
    // connections" on; the owner adding it again re-binds by name.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE projects SET agents_can_create_connections = 1 WHERE id = ?1",
            rusqlite::params![src_uuid],
        )
        .unwrap();
    }
    let err = k2_core::connections::connections_for_actor(
        &src_path,
        "add",
        Some(&format!("seoca::{host}")),
        None,
        false,
    )
    .expect_err("agent can't re-add a gone connection");
    assert!(err.contains("Only your human"), "{err}");
    assert!(err.contains("never reused"), "{err}");
    assert_eq!(remote_rows(&src_uuid)[0].remote_workspace_id.as_deref(), Some(w_old.as_str()));

    let _base = InboundBase::set(&format!("http://127.0.0.1:{}", second.port));
    let added: serde_json::Value = serde_json::from_str(
        &k2_core::connections::connections(&src_path, "add", Some(&format!("seoca::{host}")), None)
            .expect("owner re-add"),
    )
    .unwrap();
    assert_eq!(added["rebind"], true, "{added}");
    let out = tokio::task::spawn_blocking({
        let (p, t) = (src_path.clone(), format!("seoca::{host}"));
        move || bind_after_connection_add(&p, &t)
    })
    .await
    .unwrap();
    assert_eq!(out["bound"], true, "{out}");
    assert_eq!(out["remoteWorkspaceId"], w_new.as_str(), "{out}");
    let row = &remote_rows(&src_uuid)[0];
    assert!(row.remote_workspace_gone_at.is_none(), "{row:?}");
    assert_eq!(remote_rows(&src_uuid).len(), 1, "same row, not a second one");
}

/// CA11: a verified inbound from the peer kicks one background roster
/// fetch, which binds the unbound row without anyone sending.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a7_peer_seen_online_binds_unbound_rows() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (src_path, src_uuid) = register_src_workspace("a7-online");
    let w = uuid::Uuid::new_v4().to_string();
    let stub = spawn_peer_stub(
        Some(roster_json(&[(&w, "quahog-website", "Seoca", &[])])),
        "{}",
        |_| {},
    );
    let _base = InboundBase::set(&format!("http://127.0.0.1:{}", stub.port));
    let key = pin_trusted_peer("peer");
    let host = format!("127.0.0.1:{}", stub.port);
    k2_core::connections::connections(&src_path, "add", Some(&format!("seoca::{host}")), None)
        .expect("add remote connection");

    AUTO_BIND_IN_TESTS.store(true, std::sync::atomic::Ordering::SeqCst);
    // Addressed to a workspace this server doesn't have: the receiver
    // answers 404, but the envelope verified, so the peer is online.
    let bytes = seal_msg_named(&uuid::Uuid::new_v4().to_string(), "nobody", &key, "hello");
    let resp = tokio::task::spawn_blocking(move || handle_inbound(&bytes))
        .await
        .unwrap();
    assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while remote_rows(&src_uuid)[0].remote_workspace_id.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "row never bound after the peer was seen online"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    AUTO_BIND_IN_TESTS.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(remote_rows(&src_uuid)[0].remote_workspace_id.as_deref(), Some(w.as_str()));
    assert_eq!(stub.roster_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// CA8: `connections add` binds when the peer answers, warns with the
/// roster's names when the name isn't there, and says so when the peer
/// is offline. The row is stored in every case.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a7_connections_add_binds_or_warns() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (src_path, src_uuid) = register_src_workspace("a7-add");
    let w = uuid::Uuid::new_v4().to_string();
    let stub = spawn_peer_stub(
        Some(roster_json(&[(&w, "quahog-website", "Seoca", &["quahog"])])),
        "{}",
        |_| {},
    );
    let host = format!("127.0.0.1:{}", stub.port);
    let _base = InboundBase::set(&format!("http://127.0.0.1:{}", stub.port));
    // Pinned under the host the user types, so `agent::host` finds it.
    let _fp = pin_trusted_peer_for_send(&host);

    for (target, want_bound) in [
        (format!("seoca::{host}"), true),
        (format!("nobody::{host}"), false),
    ] {
        k2_core::connections::connections(&src_path, "add", Some(&target), None)
            .expect("add remote connection");
        let out = tokio::task::spawn_blocking({
            let (p, t) = (src_path.clone(), target.clone());
            move || bind_after_connection_add(&p, &t)
        })
        .await
        .unwrap();
        assert_eq!(out["bound"], want_bound, "{target}: {out}");
        if want_bound {
            assert_eq!(out["remoteWorkspaceId"], w.as_str(), "{out}");
            assert_eq!(out["handle"], "quahog-website", "{out}");
        } else {
            let warn = out["bindWarning"].as_str().unwrap();
            assert!(warn.contains("quahog-website"), "lists the roster's names: {warn}");
        }
    }
    assert_eq!(remote_rows(&src_uuid).len(), 2, "both rows stored");

    drop(_base);
    let _offline = InboundBase::set("http://127.0.0.1:1");
    k2_core::connections::connections(&src_path, "add", Some(&format!("later::{host}")), None)
        .expect("add while the peer is offline");
    let out = tokio::task::spawn_blocking({
        let (p, t) = (src_path.clone(), format!("later::{host}"));
        move || bind_after_connection_add(&p, &t)
    })
    .await
    .unwrap();
    assert_eq!(out["bound"], false, "{out}");
    assert!(
        out["bindWarning"].as_str().unwrap().contains("couldn't reach"),
        "{out}"
    );
}

/// 0.46.4 field report: `action=resolve` says up front what an add
/// would reach (hire --dry-run): a local workspace, an `agent::host` on
/// a paired server's roster, and `not_found` for an unpaired host, a
/// name the roster lacks or a local miss; an offline peer is found with
/// a warning. Nothing is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connections_resolve_checks_targets_without_writing() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (src_path, src_uuid) = register_src_workspace("resolve");
    let w = uuid::Uuid::new_v4().to_string();
    let stub = spawn_peer_stub(Some(roster_json(&[(&w, "quahog-website", "Seoca", &["quahog"])])), "{}", |_| {});
    let host = format!("127.0.0.1:{}", stub.port);
    let _base = InboundBase::set(&format!("http://127.0.0.1:{}", stub.port));
    let _fp = pin_trusted_peer_for_send(&host);
    let code = |r: &CliResponse| -> (String, String) {
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        (r.status.to_string(), v["error"]["code"].as_str().unwrap_or("").to_string())
    };
    let run = |t: String| async move { tokio::task::spawn_blocking(move || connections_resolve(&t, true)).await.unwrap() };
    let member = |t: String| async move { tokio::task::spawn_blocking(move || connections_resolve(&t, false)).await.unwrap() };

    // Federation off: a remote target can't be reached.
    let r = run(format!("quahog::{host}")).await;
    assert_eq!(code(&r), ("404 Not Found".into(), "not_found".into()), "{}", r.body);
    assert!(r.body.contains("federation is off"), "{}", r.body);

    let _fed = k2_core::test_env::EnvVar::set("K2_FEDERATION", "1");
    let r = run(format!("quahog::{host}")).await;
    assert_eq!(r.status, "200 OK", "an alias on the roster: {}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!((v["kind"].as_str(), v["onRoster"].as_bool(), v["handle"].as_str()), (Some("remote"), Some(true), Some("quahog-website")), "{v}");
    let r = run(format!("Seoca::{host}")).await;
    assert_eq!(r.status, "200 OK", "the display name: {}", r.body);
    let r = run(format!("nobody::{host}")).await;
    assert_eq!(code(&r), ("404 Not Found".into(), "not_found".into()), "{}", r.body);
    assert!(r.body.contains("quahog-website"), "names the roster: {}", r.body);
    let r = run("k2::nowhere.k2.dev".into()).await;
    assert_eq!(code(&r), ("404 Not Found".into(), "not_found".into()), "{}", r.body);
    assert!(r.body.contains("no paired server"), "{}", r.body);

    // Local: found by path; a miss is not_found.
    let r = run(src_path.clone()).await;
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!((v["kind"].as_str(), v["path"].as_str(), v["projectId"].as_str()), (Some("local"), Some(src_path.as_str()), Some(src_uuid.as_str())), "{v}");
    let r = run("no-such-workspace-xyz".into()).await;
    assert_eq!(code(&r), ("404 Not Found".into(), "not_found".into()), "{}", r.body);
    let r = run(String::new()).await;
    assert_eq!(code(&r), ("400 Bad Request".into(), "bad_usage".into()), "{}", r.body);

    // Security review (0.46.4): a Member never reads another server's
    // roster (403 before any lookup) and gets no path or id for a local
    // workspace, only found / not found.
    let r = member(format!("nobody::{host}")).await;
    assert_eq!(code(&r), ("403 Forbidden".into(), "owner_only".into()), "{}", r.body);
    assert!(!r.body.contains("quahog-website"), "no roster names: {}", r.body);
    let r = member(src_path.clone()).await;
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["kind"], "local", "{v}");
    assert!(v.get("path").is_none() && v.get("projectId").is_none(), "no path or id for a Member: {v}");
    let r = member("no-such-workspace-xyz".into()).await;
    assert_eq!(code(&r), ("404 Not Found".into(), "not_found".into()), "{}", r.body);

    // Offline peer: found, with a warning.
    drop(_base);
    let _offline = InboundBase::set("http://127.0.0.1:1");
    let r = run(format!("quahog::{host}")).await;
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert!(v["onRoster"].is_null() && v["warning"].as_str().unwrap().contains("couldn't reach"), "{v}");
    assert!(remote_rows(&src_uuid).is_empty(), "resolve writes nothing");
}

/// PRD test 7 + CA15: after a display-name bind, the row is removed by
/// the name it was saved under, or by the bound workspace's handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a7_remove_by_saved_name_or_bound_handle() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (src_path, src_uuid) = register_src_workspace("a7-remove");
    let w = uuid::Uuid::new_v4().to_string();
    let stub = spawn_peer_stub(
        Some(roster_json(&[(&w, "quahog-website", "Seoca", &[])])),
        "{}",
        |_| {},
    );
    let host = format!("127.0.0.1:{}", stub.port);
    let _base = InboundBase::set(&format!("http://127.0.0.1:{}", stub.port));
    let fp = pin_trusted_peer_for_send("peer");

    for remove_as in ["seoca", "quahog-website"] {
        k2_core::connections::connections(&src_path, "add", Some(&format!("seoca::{host}")), None)
            .expect("add remote connection");
        let r = tokio::task::spawn_blocking({
            let fp = fp.clone();
            move || handle_peer_roster(&fp)
        })
        .await
        .unwrap();
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            remote_rows(&src_uuid)[0].remote_workspace_id.as_deref(),
            Some(w.as_str())
        );
        k2_core::connections::connections(
            &src_path,
            "remove",
            Some(&format!("{remove_as}::{host}")),
            None,
        )
        .unwrap_or_else(|e| panic!("remove as {remove_as}: {e}"));
        assert!(remote_rows(&src_uuid).is_empty(), "removed as {remove_as}");
    }
}
